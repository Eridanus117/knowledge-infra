use crate::git::{GitBackend, GitError};
use kb_contract::{
    DomainId, NoteKind, NoteStatus, SourceSpec, derive_identity, parse_and_validate_note,
};
use std::fmt;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalMarker {
    pub path: PathBuf,
    pub reason: String,
}
impl ApprovalMarker {
    pub fn for_one_file(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        let path = path.into();
        let path = if path.is_absolute() {
            std::fs::canonicalize(&path).unwrap_or(path)
        } else {
            path
        };
        Self {
            path,
            reason: reason.into(),
        }
    }
}

#[derive(Debug)]
pub enum FrozenError {
    Git(GitError),
    InvalidPath(PathBuf),
    InvalidNote(PathBuf),
    Frozen(PathBuf),
    Approval(PathBuf),
    Staged(PathBuf),
}
impl fmt::Display for FrozenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(e) => e.fmt(f),
            Self::InvalidPath(_) => f.write_str("path is outside the Git root"),
            Self::InvalidNote(_) => f.write_str("committed note is invalid"),
            Self::Frozen(_) => f.write_str("frozen note change is not approved"),
            Self::Approval(_) => f.write_str("approval is not valid for this file"),
            Self::Staged(_) => f.write_str("staged frozen change is not approved"),
        }
    }
}
impl std::error::Error for FrozenError {}
impl From<GitError> for FrozenError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

pub fn is_head_frozen(git: &GitBackend, path: &Path) -> Result<bool, FrozenError> {
    let bytes = match git.head_blob(path) {
        Ok(bytes) => bytes,
        Err(GitError::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !git.head_regular(path)? {
        return Ok(false);
    }
    let note = parse_and_validate_note(path, &bytes)
        .map_err(|_| FrozenError::InvalidNote(path.to_path_buf()))?;
    Ok(note.frontmatter.status == Some(NoteStatus::Frozen)
        || note.frontmatter.kind == NoteKind::Decision)
}

pub fn check_worktree_change(
    git: &GitBackend,
    path: &Path,
    approval: Option<&ApprovalMarker>,
) -> Result<(), FrozenError> {
    git.relative_path(path)
        .map_err(|_| FrozenError::InvalidPath(path.to_path_buf()))?;
    if !is_head_frozen(git, path)? {
        return Ok(());
    }
    if git.worktree_matches_head(path)? {
        return Ok(());
    }
    let Some(marker) = approval else {
        return Err(FrozenError::Frozen(path.to_path_buf()));
    };
    let marker_is_canonical = marker.path.is_absolute()
        && std::fs::canonicalize(&marker.path).ok().as_ref() == Some(&marker.path)
        && crate::source::validate_regular_file_nofollow(&marker.path).is_ok();
    if !marker_is_canonical
        || marker.reason.is_empty()
        || marker.reason.chars().any(char::is_control)
        || !same_path(path, &marker.path)
    {
        return Err(FrozenError::Approval(path.to_path_buf()));
    }
    Ok(())
}

/// Conservative hook-facing gate: without registered source context it never authorizes ledger exceptions.
/// Operation paths must use `check_staged_frozen_for_specs` or `check_staged_frozen_pair`.
pub fn check_staged_frozen(git: &GitBackend) -> Result<(), FrozenError> {
    check_staged_frozen_for_specs(git, &[])
}
pub fn check_staged_frozen_pair(
    source_git: &GitBackend,
    source_specs: &[SourceSpec],
    target_git: &GitBackend,
    target_specs: &[SourceSpec],
) -> Result<(), FrozenError> {
    let changes = source_git.staged_status()?;
    let target_changes = target_git.staged_status()?;
    let source_records = staged_ledger(source_git, &changes, "relocate")?
        .ok_or_else(|| FrozenError::Staged(source_git.root.clone()))?;
    let target_records = staged_ledger(target_git, &target_changes, "relocate")?
        .ok_or_else(|| FrozenError::Staged(target_git.root.clone()))?;
    if target_records.iter().any(|record| {
        target_changes.iter().any(|change| {
            change.new_path.as_deref() == Some(&record.new_path)
                || change.old_path == record.new_path
        }) && !target_changes
            .iter()
            .any(|change| change.old_path == record.new_path && change.status == "A")
    }) {
        return Err(FrozenError::Staged(target_git.root.clone()));
    }
    check_staged_target_for_specs(target_git, target_specs)?;
    let source_head = source_git.head_oid()?;
    let target_head = target_git.head_oid()?;
    for change in changes {
        let old = change.old_path.as_str();
        let old_abs = source_git.absolute_path(old);
        let head_bytes = match source_git.head_blob(&old_abs) {
            Ok(bytes) => bytes,
            Err(crate::git::GitError::NotFound(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        if parse_and_validate_note(&old_abs, &head_bytes).is_err() {
            continue;
        }
        if change.status != "D" {
            return Err(FrozenError::Staged(old_abs));
        }
        let frozen = matches!(is_head_frozen(source_git, &old_abs), Ok(true));
        let old_hash = GitBackend::canonical_blob_sha256(&head_bytes);
        let source_record = source_records
            .iter()
            .find(|record| {
                let expected_source = record.old_identity.split(':').next().unwrap_or("");
                record.logical_source == expected_source
                    && record.old_path == old
                    && record.head_oid == source_head
                    && (!frozen || record.canonical_git_blob_sha256 == old_hash)
                    && identity_matches_path(&record.old_identity, old)
                    && expected_identity(source_git, source_specs, old)
                        .is_some_and(|(_, identity)| identity == record.old_identity)
                    && record.old_identity.split(':').next()
                        != record.new_identity.split(':').next()
            })
            .ok_or_else(|| FrozenError::Staged(old_abs.clone()))?;
        let paired = target_records.iter().any(|target| {
            target.old_identity == source_record.old_identity
                && target.new_identity == source_record.new_identity
                && target.old_path == source_record.old_path
                && target.new_path == source_record.new_path
                && target.canonical_git_blob_sha256 == source_record.canonical_git_blob_sha256
                && target.reason == source_record.reason
                && target.head_oid == target_head
                && target.logical_source == target.new_identity.split(':').next().unwrap_or("")
                && context_new_matches(target_git, target_specs, target)
                && staged_hash(target_git, &target.new_path)
                    .is_ok_and(|hash| hash == target.canonical_git_blob_sha256)
        });
        if !paired {
            return Err(FrozenError::Staged(old_abs));
        }
    }
    Ok(())
}

pub fn check_staged_frozen_for_specs(
    git: &GitBackend,
    specs: &[SourceSpec],
) -> Result<(), FrozenError> {
    check_staged_frozen_for_specs_mode(git, specs, false)
}
pub(crate) fn check_staged_target_for_specs(
    git: &GitBackend,
    specs: &[SourceSpec],
) -> Result<(), FrozenError> {
    check_staged_frozen_for_specs_mode(git, specs, true)
}
fn check_staged_frozen_for_specs_mode(
    git: &GitBackend,
    specs: &[SourceSpec],
    allow_target_only: bool,
) -> Result<(), FrozenError> {
    let changes = git.staged_status()?;
    let relocate_records = staged_ledger(git, &changes, "relocate")?;
    let amend_records = staged_ledger(git, &changes, "amend")?;
    let head = git.head_oid()?;
    if let Some(records) = relocate_records.as_ref() {
        for record in records {
            if changes.iter().any(|change| {
                change.old_path == record.old_path
                    && (change.status == "D" || change.status.starts_with('R'))
            }) && !operation_old_note_valid(git, specs, record)
            {
                return Err(FrozenError::Staged(git.absolute_path(&record.old_path)));
            }
            let old_used = changes.iter().any(|change| {
                if change.old_path != record.old_path
                    || (change.status != "D" && !change.status.starts_with('R'))
                {
                    return false;
                }
                operation_old_note_valid(git, specs, record)
            });
            let target_used = changes.iter().any(|change| {
                let added = (change.old_path == record.new_path && change.status == "A")
                    || (change.new_path.as_deref() == Some(&record.new_path)
                        && change.status.starts_with('R'));
                added
                    && context_new_matches(git, specs, record)
                    && staged_hash(git, &record.new_path)
                        .map_or(false, |hash| hash == record.canonical_git_blob_sha256)
            });
            if if allow_target_only {
                !old_used && !target_used
            } else {
                !old_used || !target_used
            } {
                return Err(FrozenError::Staged(git.absolute_path(&record.new_path)));
            }
        }
    }
    if let Some(records) = amend_records.as_ref() {
        for record in records {
            let used = changes.iter().any(|change| {
                if change.old_path != record.old_path || change.status != "M" {
                    return false;
                }
                let old_abs = git.absolute_path(&record.old_path);
                matches!(is_head_frozen(git, &old_abs), Ok(true))
            });
            if !used {
                return Err(FrozenError::Staged(git.absolute_path(&record.old_path)));
            }
        }
    }
    if let Some(records) = relocate_records.as_ref() {
        for record in records {
            let target = git.absolute_path(&record.new_path);
            let target_change = changes.iter().find(|change| {
                change.old_path == record.new_path
                    || change.new_path.as_deref() == Some(&record.new_path)
            });
            if let Some(change) = target_change {
                let destination_added = change.status == "A"
                    || (change.status.starts_with('R')
                        && change.new_path.as_deref() == Some(&record.new_path));
                if !destination_added {
                    return Err(FrozenError::Staged(target));
                }
                if git.head_blob(&target).is_ok() {
                    return Err(FrozenError::Staged(target));
                }
                if record.head_oid != head {
                    return Err(FrozenError::Staged(target));
                }
                if !identity_matches_path(&record.new_identity, &record.new_path) {
                    return Err(FrozenError::Staged(target));
                }
                if staged_hash(git, &record.new_path)? != record.canonical_git_blob_sha256 {
                    return Err(FrozenError::Staged(target));
                }
                if !context_new_matches(git, specs, record) {
                    return Err(FrozenError::Staged(target));
                }
            }
        }
    }
    for change in changes {
        let status = change.status.as_str();
        let old = change.old_path.as_str();
        let new = change.new_path.as_deref();
        let old_abs = git.absolute_path(old);
        let head_frozen = match is_head_frozen(git, &old_abs) {
            Ok(value) => value,
            Err(FrozenError::InvalidNote(_)) => false,
            Err(error) => return Err(error),
        };
        if !head_frozen {
            continue;
        }
        let old_hash = git.head_blob_sha256(&old_abs)?;
        if status == "M" {
            let staged = git.staged_blob(&old_abs)?;
            let digest = GitBackend::canonical_blob_sha256(&staged);
            let approved = amend_records.as_ref().map_or(false, |records| {
                records.iter().any(|record| {
                    let expected_source = record.old_identity.split(':').next().unwrap_or("");
                    record.logical_source == expected_source
                        && record.old_path == old
                        && record.new_path == old
                        && record.head_oid == head
                        && record.canonical_git_blob_sha256 == digest
                        && record.old_identity == record.new_identity
                        && identity_matches_path(&record.old_identity, old)
                        && parse_and_validate_note(&old_abs, &staged)
                            .map_or(false, |note| note.frontmatter.kind != NoteKind::Index)
                        && context_record_matches(git, specs, record)
                })
            });
            if !approved {
                return Err(FrozenError::Staged(old_abs));
            }
            continue;
        }
        let matching = relocate_records.as_ref().and_then(|records| {
            records.iter().find(|record| {
                let expected_source = record.old_identity.split(':').next().unwrap_or("");
                record.logical_source == expected_source
                    && record.old_path == old
                    && record.head_oid == head
                    && record.canonical_git_blob_sha256 == old_hash
                    && identity_matches_path(&record.old_identity, old)
                    && context_record_matches(git, specs, record)
            })
        });
        let Some(record) = matching else {
            return Err(FrozenError::Staged(old_abs));
        };
        if status.starts_with('R') {
            let Some(new) = new else {
                return Err(FrozenError::Staged(old_abs));
            };
            if record.new_path != new || staged_hash(git, new)? != record.canonical_git_blob_sha256
            {
                return Err(FrozenError::Staged(old_abs));
            }
        } else if status == "D" {
            let target = git.absolute_path(&record.new_path);
            let target_parent_exists = target.parent().map_or(false, Path::is_dir);
            if target_parent_exists {
                if staged_hash(git, &record.new_path)? != record.canonical_git_blob_sha256 {
                    return Err(FrozenError::Staged(old_abs));
                }
            } else if record.old_identity.split(':').next() == record.new_identity.split(':').next()
            {
                return Err(FrozenError::Staged(old_abs));
            }
        } else {
            return Err(FrozenError::Staged(old_abs));
        }
    }
    Ok(())
}
fn staged_ledger(
    git: &GitBackend,
    changes: &[crate::git::StagedChange],
    operation: &str,
) -> Result<Option<Vec<crate::ledger::LedgerRecord>>, FrozenError> {
    let relative = match operation {
        "relocate" => ".rhizome/relocate-ledger.ndjson",
        "amend" => ".rhizome/amend-ledger.ndjson",
        _ => return Err(FrozenError::Staged(git.root.clone())),
    };
    let Some(change) = changes
        .iter()
        .find(|change| change.old_path == relative && change.new_path.is_none())
    else {
        return Ok(None);
    };
    if change.status != "A" && change.status != "M" {
        return Err(FrozenError::Staged(git.absolute_path(relative)));
    }
    let path = git.absolute_path(relative);
    if !git.staged_regular(&path)? {
        return Err(FrozenError::Staged(path));
    }
    let staged_bytes = git.staged_blob(&path)?;
    let head_bytes = match git.head_blob(&path) {
        Ok(bytes) => bytes,
        Err(crate::git::GitError::NotFound(_)) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if staged_bytes == head_bytes || !staged_bytes.starts_with(&head_bytes) {
        return Err(FrozenError::Staged(path));
    }
    let staged = crate::ledger::parse_bytes(&staged_bytes, operation)
        .map_err(|_| FrozenError::Staged(path.clone()))?;
    let history = if head_bytes.is_empty() {
        Vec::new()
    } else {
        crate::ledger::parse_bytes(&head_bytes, operation)
            .map_err(|_| FrozenError::Staged(path.clone()))?
    };
    if staged.len() <= history.len() || staged[..history.len()] != history[..] {
        return Err(FrozenError::Staged(path));
    }
    Ok(Some(staged[history.len()..].to_vec()))
}
fn context_record_matches(
    git: &GitBackend,
    specs: &[SourceSpec],
    record: &crate::ledger::LedgerRecord,
) -> bool {
    let Some((old_source, old_identity)) = expected_identity(git, specs, &record.old_path) else {
        return false;
    };
    if record.logical_source != old_source || record.old_identity != old_identity {
        return false;
    }
    match expected_identity(git, specs, &record.new_path) {
        Some((new_source, new_identity)) => {
            record.logical_source == old_source
                && record.new_identity == new_identity
                && new_source == record.new_identity.split(':').next().unwrap_or("")
        }
        None => false,
    }
}
fn context_new_matches(
    git: &GitBackend,
    specs: &[SourceSpec],
    record: &crate::ledger::LedgerRecord,
) -> bool {
    let old_source = record.old_identity.split(':').next().unwrap_or("");
    expected_identity(git, specs, &record.new_path).is_some_and(|(source, identity)| {
        (record.logical_source == source || record.logical_source == old_source)
            && record.new_identity == identity
    })
}
fn operation_old_note_valid(
    git: &GitBackend,
    specs: &[SourceSpec],
    record: &crate::ledger::LedgerRecord,
) -> bool {
    let path = git.absolute_path(&record.old_path);
    if !git.head_regular(&path).unwrap_or(false) {
        return false;
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return false;
    }
    let Ok(bytes) = git.head_blob(&path) else {
        return false;
    };
    let Ok(note) = parse_and_validate_note(&path, &bytes) else {
        return false;
    };
    note.frontmatter.kind != NoteKind::Index
        && expected_identity(git, specs, &record.old_path)
            .is_some_and(|(_, identity)| identity == record.old_identity)
}
fn expected_identity(
    git: &GitBackend,
    specs: &[SourceSpec],
    relative: &str,
) -> Option<(String, String)> {
    let absolute = git.absolute_path(relative);
    specs
        .iter()
        .filter_map(|spec| {
            let root = std::fs::canonicalize(&spec.root).ok()?;
            let context = crate::source::SourceContext { source: spec.clone(), git_root: git.root.clone(), registry_origin: git.root.clone() };
            if let Ok(snapshot) = crate::source::discover_source(&context) {
                if let Some(note) = snapshot.notes.iter().find(|note| note.locator.path == absolute) {
                    return Some((root.components().count(), spec.name.to_string(), note.locator.identity.to_string()));
                }
            }
            let source_relative = absolute.strip_prefix(&root).ok()?;
            if source_relative.components().any(|component| matches!(component, std::path::Component::Normal(name) if name.to_str().map_or(false, |name| name.starts_with('.') || matches!(name, "node_modules" | "target" | "dist")))) { return None; }
            if source_relative.extension().and_then(|ext| ext.to_str()) != Some("md") {
                return None;
            }
            let slug = source_relative.file_stem()?.to_str()?;
            if slug == "INDEX" {
                return None;
            }
            let mut ancestor = absolute.parent()?.to_path_buf();
            let mut domain_segments = Vec::new();
            loop {
                    let index = ancestor.join("INDEX.md");
                    match git.head_blob(&index) {
                        Ok(bytes) => {
                            if !git.head_regular(&index).unwrap_or(false) { return None; }
                            if !parse_and_validate_note(&index, &bytes)
                                .map_or(false, |note| note.frontmatter.kind == NoteKind::Index)
                            {
                                return None;
                            }
                            domain_segments.push(ancestor.file_name()?.to_str()?.to_owned());
                        }
                        Err(crate::git::GitError::NotFound(_)) => {}
                        Err(_) => return None,
                    }
                if ancestor == root {
                    break;
                }
                ancestor = ancestor.parent()?.to_path_buf();
            }
            if domain_segments.is_empty() {
                return None;
            }
            domain_segments.reverse();
            let domain = domain_segments.join("/");
            let domain = DomainId::new(&domain).ok()?;
            let identity = derive_identity(&spec.name, &domain, slug).ok()?.to_string();
            Some((root.components().count(), spec.name.to_string(), identity))
        })
        .max_by_key(|(depth, _, _)| *depth)
        .map(|(_, source, identity)| (source, identity))
}
fn identity_matches_path(identity: &str, path: &str) -> bool {
    let Some(slug) = identity.rsplit(':').next() else {
        return false;
    };
    let Some(file) = path.rsplit('/').next() else {
        return false;
    };
    file.strip_suffix(".md")
        .map(|value| value.nfc().collect::<String>())
        == Some(slug.to_owned())
}
fn staged_hash(git: &GitBackend, relative: &str) -> Result<String, FrozenError> {
    let path = git.absolute_path(relative);
    if !git.staged_regular(&path)? {
        return Err(FrozenError::Staged(path));
    }
    let bytes = git.staged_blob(&path)?;
    Ok(GitBackend::canonical_blob_sha256(&bytes))
}
fn same_path(a: &Path, b: &Path) -> bool {
    std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf())
        == std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf())
}
