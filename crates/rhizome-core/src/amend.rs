use crate::frozen::{ApprovalMarker, FrozenError, is_head_frozen};
use crate::git::{GitBackend, GitError};
use crate::ledger::{self, LedgerError, LedgerRecord, SCHEMA};
use crate::source::{SourceContext, discover_source};
use kb_contract::{SourceSpec, parse_and_validate_note};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum AmendError {
    Git(GitError),
    Frozen(FrozenError),
    Ledger(LedgerError),
    Invalid(String),
    Io(PathBuf),
    Discovery,
}
impl fmt::Display for AmendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(e) => e.fmt(f),
            Self::Frozen(e) => e.fmt(f),
            Self::Ledger(e) => e.fmt(f),
            Self::Invalid(_) => f.write_str("amend request is invalid"),
            Self::Io(_) => f.write_str("amend could not access a path"),
            Self::Discovery => f.write_str("source discovery failed"),
        }
    }
}
impl std::error::Error for AmendError {}
impl From<GitError> for AmendError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}
impl From<FrozenError> for AmendError {
    fn from(e: FrozenError) -> Self {
        Self::Frozen(e)
    }
}
impl From<LedgerError> for AmendError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}

#[derive(Clone, Debug)]
pub struct AmendPlan {
    repo: PathBuf,
    path: PathBuf,
    old_bytes: Vec<u8>,
    worktree_before: Vec<u8>,
    worktree_status_before: Vec<u8>,
    replacement: Vec<u8>,
    old_identity: String,
    logical_source: String,
    old_path: String,
    head_oid: String,
    old_hash: String,
    reason: String,
    approval: ApprovalMarker,
    source_root: PathBuf,
    source_spec: SourceSpec,
    registry_bytes: Vec<u8>,
    registry_origin: PathBuf,
    overlay_path: PathBuf,
    overlay_bytes: Option<Vec<u8>>,
}
pub fn plan_amend(
    context: &SourceContext,
    path: &Path,
    replacement: &[u8],
    reason: &str,
    approval: &ApprovalMarker,
) -> Result<AmendPlan, AmendError> {
    if reason.is_empty() || reason.chars().any(char::is_control) {
        return Err(AmendError::Invalid("amend reason is invalid".into()));
    }
    let git = GitBackend::new(context.git_root.clone())?;
    if !git.attributes_clean() {
        return Err(AmendError::Invalid(
            "Git attributes changed outside operation".into(),
        ));
    }
    let canonical_path = fs::canonicalize(path).map_err(|_| AmendError::Io(path.to_path_buf()))?;
    crate::source::validate_regular_file_nofollow(&canonical_path).map_err(|_| {
        AmendError::Invalid("amend source path is not a single-link regular file".into())
    })?;
    if !approval.path.is_absolute()
        || fs::canonicalize(&approval.path).ok().as_ref() != Some(&approval.path)
        || crate::source::validate_regular_file_nofollow(&approval.path).is_err()
    {
        return Err(AmendError::Invalid(
            "approval path must be one canonical regular file".into(),
        ));
    }
    if !same_path(&canonical_path, &approval.path)
        || approval.reason.is_empty()
        || approval.reason.chars().any(char::is_control)
        || approval.reason != reason
    {
        return Err(AmendError::Invalid(
            "approval does not identify this file".into(),
        ));
    }
    if !git.staged_status()?.is_empty() {
        return Err(AmendError::Invalid("amend requires a clean index".into()));
    }
    let worktree_status_before = git.worktree_status()?;
    let amend_relative = git.relative_path(&canonical_path)?;
    if !filtered_worktree_status(&worktree_status_before, &[amend_relative.as_str()]).is_empty() {
        return Err(AmendError::Invalid(
            "amend has unrelated worktree changes".into(),
        ));
    }
    if worktree_status_before
        .split(|byte| *byte == 0)
        .any(|entry| entry.starts_with(b"?? "))
    {
        return Err(AmendError::Invalid(
            "amend requires no pre-existing untracked files".into(),
        ));
    }
    let _ = amend_ledger_baseline(&git)?;
    if !is_head_frozen(&git, &canonical_path)? {
        return Err(AmendError::Invalid(
            "amend requires a frozen HEAD note".into(),
        ));
    }
    let snapshot = discover_source(context).map_err(|_| AmendError::Discovery)?;
    let source_note = snapshot
        .notes
        .iter()
        .find(|note| note.locator.path == canonical_path)
        .ok_or_else(|| AmendError::Invalid("path is not a discovered note".into()))?;
    parse_and_validate_note(&canonical_path, replacement)
        .map_err(|_| AmendError::Invalid("replacement note is invalid".into()))?;
    let old_bytes = git.head_blob(&canonical_path)?;
    let worktree_before = git.worktree_bytes(&canonical_path)?;
    if !git.worktree_matches_head(&canonical_path)? {
        return Err(AmendError::Invalid("amend source changed".into()));
    }
    let source_root = fs::canonicalize(&context.source.root)
        .map_err(|_| AmendError::Io(context.registry_origin.clone()))?;
    let registry_bytes = fs::read(&context.registry_origin)
        .map_err(|_| AmendError::Io(context.registry_origin.clone()))?;
    let (overlay_path, overlay_bytes) = registry_overlay_snapshot(&context.registry_origin)?;
    Ok(AmendPlan {
        repo: git.root.clone(),
        path: canonical_path.clone(),
        worktree_status_before,
        worktree_before,
        old_bytes: old_bytes.clone(),
        replacement: replacement.to_vec(),
        old_identity: source_note.locator.identity.to_string(),
        logical_source: snapshot.source.to_string(),
        old_path: git.relative_path(&canonical_path)?,
        head_oid: git.head_oid()?,
        old_hash: GitBackend::canonical_blob_sha256(&old_bytes),
        reason: reason.to_owned(),
        approval: ApprovalMarker {
            path: canonical_path,
            reason: approval.reason.clone(),
        },
        source_root,
        source_spec: context.source.clone(),
        registry_origin: context.registry_origin.clone(),
        overlay_path,
        overlay_bytes,
        registry_bytes,
    })
}
pub fn apply_amend(plan: &AmendPlan) -> Result<(), AmendError> {
    let git = GitBackend::new(plan.repo.clone())?;
    if !git.attributes_clean() {
        return Err(AmendError::Invalid(
            "Git attributes changed outside operation".into(),
        ));
    }
    if git.head_oid()? != plan.head_oid {
        return Err(AmendError::Invalid("amend plan is stale".into()));
    }
    if fs::canonicalize(&plan.source_spec.root)
        .map_err(|_| AmendError::Invalid("source registry changed".into()))?
        != plan.source_root
        || fs::read(&plan.registry_origin)
            .map_err(|_| AmendError::Invalid("source registry changed".into()))?
            != plan.registry_bytes
    {
        return Err(AmendError::Invalid("source registry changed".into()));
    }
    let (overlay_path, overlay_bytes) = registry_overlay_snapshot(&plan.registry_origin)?;
    if overlay_path != plan.overlay_path || overlay_bytes != plan.overlay_bytes {
        return Err(AmendError::Invalid(
            "source registry overlay changed".into(),
        ));
    }
    let context = SourceContext {
        source: plan.source_spec.clone(),
        git_root: git.root.clone(),
        registry_origin: plan.registry_origin.clone(),
    };
    let snapshot = discover_source(&context).map_err(|_| AmendError::Discovery)?;
    if snapshot
        .notes
        .iter()
        .find(|note| {
            note.locator.path == plan.path && note.locator.identity.to_string() == plan.old_identity
        })
        .is_none()
    {
        return Err(AmendError::Invalid("source identity changed".into()));
    }
    if !same_path(&plan.path, &plan.approval.path) || plan.approval.reason != plan.reason {
        return Err(AmendError::Invalid("approval changed".into()));
    }
    if !git.staged_status()?.is_empty() {
        return Err(AmendError::Invalid("amend requires a clean index".into()));
    }
    if git.head_oid()? != plan.head_oid
        || git.head_blob(&plan.path)? != plan.old_bytes
        || git.head_blob_sha256(&plan.path)? != plan.old_hash
        || !is_head_frozen(&git, &plan.path)?
        || !git.worktree_matches_head(&plan.path)?
    {
        return Err(AmendError::Invalid("amend predecessor changed".into()));
    }
    parse_and_validate_note(&plan.path, &plan.replacement)
        .map_err(|_| AmendError::Invalid("replacement note is invalid".into()))?;
    let ledger_path = git.root.join(".rhizome/amend-ledger.ndjson");
    let ledger_rel = git.relative_path(&ledger_path)?;
    let _ = ledger::read(&git.root, "amend")?;
    let ledger_before = amend_ledger_baseline(&git)?;
    if !is_regular_nosymlink(&plan.path) {
        return Err(AmendError::Invalid("amend source path changed".into()));
    }
    if GitBackend::write_regular_nosymlink(&plan.path, &plan.replacement).is_err() {
        return Err(AmendError::Io(plan.path.clone()));
    }
    if let Err(error) = git.add(std::slice::from_ref(&plan.old_path)) {
        let rollback = rollback_amend(
            &git,
            &plan.path,
            &plan.old_path,
            &plan.worktree_before,
            &ledger_path,
            &ledger_rel,
            ledger_before.clone(),
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    let staged = match git.staged_blob(&plan.path) {
        Ok(value) => value,
        Err(error) => {
            let rollback = rollback_amend(
                &git,
                &plan.path,
                &plan.old_path,
                &plan.worktree_before,
                &ledger_path,
                &ledger_rel,
                ledger_before.clone(),
            );
            return match rollback {
                Ok(()) => Err(error.into()),
                Err(error) => Err(error),
            };
        }
    };
    if !head_matches(&git, &plan.head_oid) || !only_staged_path(&git, &plan.old_path) {
        let rollback = rollback_amend(
            &git,
            &plan.path,
            &plan.old_path,
            &plan.worktree_before,
            &ledger_path,
            &ledger_rel,
            ledger_before.clone(),
        );
        return match rollback {
            Ok(()) => Err(AmendError::Invalid("amend became stale".into())),
            Err(error) => Err(error),
        };
    }
    let record = LedgerRecord {
        schema: SCHEMA.into(),
        operation: "amend".into(),
        logical_source: plan.logical_source.clone(),
        old_identity: plan.old_identity.clone(),
        new_identity: plan.old_identity.clone(),
        old_path: plan.old_path.clone(),
        new_path: plan.old_path.clone(),
        head_oid: plan.head_oid.clone(),
        canonical_git_blob_sha256: GitBackend::canonical_blob_sha256(&staged),
        reason: plan.reason.clone(),
    };
    let expected_ledger = {
        let mut bytes = ledger_before.clone().unwrap_or_default();
        bytes.extend_from_slice(
            record
                .line()
                .map_err(|_| AmendError::Invalid("amend ledger serialization failed".into()))?
                .as_bytes(),
        );
        bytes
    };
    if let Err(error) = ledger::append(&git.root, &record) {
        let rollback = rollback_amend(
            &git,
            &plan.path,
            &plan.old_path,
            &plan.worktree_before,
            &ledger_path,
            &ledger_rel,
            ledger_before.clone(),
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    if let Err(error) = git.add(std::slice::from_ref(&ledger_rel)) {
        let rollback = rollback_amend(
            &git,
            &plan.path,
            &plan.old_path,
            &plan.worktree_before,
            &ledger_path,
            &ledger_rel,
            ledger_before.clone(),
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    let gate_ok =
        crate::frozen::check_staged_frozen_for_specs(&git, std::slice::from_ref(&plan.source_spec))
            .is_ok();
    let head_ok = head_matches(&git, &plan.head_oid);
    let paths_ok = only_staged_paths(&git, &[plan.old_path.as_str(), ledger_rel.as_str()]);
    if !gate_ok || !head_ok || !paths_ok {
        let rollback = rollback_amend(
            &git,
            &plan.path,
            &plan.old_path,
            &plan.worktree_before,
            &ledger_path,
            &ledger_rel,
            ledger_before.clone(),
        );
        return match rollback {
            Ok(()) => Err(AmendError::Invalid(format!(
                "amend became stale gate={gate_ok} head={head_ok} paths={paths_ok}"
            ))),
            Err(error) => Err(error),
        };
    }
    if let Err(error) = git.commit(&format!("amend(frozen): {}", plan.old_path), &plan.reason) {
        let rollback = rollback_amend(
            &git,
            &plan.path,
            &plan.old_path,
            &plan.worktree_before,
            &ledger_path,
            &ledger_rel,
            ledger_before.clone(),
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    if let Err(error) = verify_post_commit(
        &git,
        plan,
        &ledger_path,
        &ledger_rel,
        &staged,
        &expected_ledger,
    ) {
        let rollback = rollback_postcommit(&git, plan, &ledger_path, ledger_before.clone());
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(AmendError::Invalid(format!(
                "post-commit verification failed and rollback failed: {rollback_error}"
            ))),
        };
    }
    Ok(())
}

fn rollback_postcommit(
    git: &GitBackend,
    plan: &AmendPlan,
    ledger_path: &Path,
    ledger_before: Option<Vec<u8>>,
) -> Result<(), AmendError> {
    git.reset_hard(&plan.head_oid)?;
    GitBackend::write_regular_nosymlink(&plan.path, &plan.worktree_before)?;
    match ledger_before {
        Some(bytes) => GitBackend::write_regular_nosymlink(ledger_path, &bytes)?,
        None => {
            if fs::symlink_metadata(ledger_path).is_ok() {
                crate::source::remove_file_nofollow(ledger_path)
                    .map_err(|_| AmendError::Invalid("post-commit rollback is unsafe".into()))?;
            }
        }
    }
    cleanup_untracked_operation_files(git)?;
    git.reset_index()?;
    if git.head_oid()? != plan.head_oid
        || !git.staged_status()?.is_empty()
        || fs::read(&plan.path).map_err(|_| AmendError::Io(plan.path.clone()))?
            != plan.worktree_before
    {
        return Err(AmendError::Invalid("post-commit rollback is unsafe".into()));
    }
    Ok(())
}
fn cleanup_untracked_operation_files(git: &GitBackend) -> Result<(), AmendError> {
    for entry in git.worktree_status()?.split(|byte| *byte == 0) {
        if entry.starts_with(b"?? ") {
            let relative = std::str::from_utf8(&entry[3..])
                .map_err(|_| AmendError::Invalid("post-commit rollback is unsafe".into()))?;
            let path = git.absolute_path(relative);
            crate::source::remove_file_nofollow(&path)
                .map_err(|_| AmendError::Invalid("post-commit rollback is unsafe".into()))?;
        }
    }
    Ok(())
}

fn is_regular_nosymlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}
fn head_matches(git: &GitBackend, expected: &str) -> bool {
    matches!(git.head_oid(), Ok(actual) if actual == expected)
}
fn only_staged_path(git: &GitBackend, path: &str) -> bool {
    only_staged_paths(git, &[path])
}
fn verify_post_commit(
    git: &GitBackend,
    plan: &AmendPlan,
    ledger_path: &Path,
    ledger_rel: &str,
    staged_note: &[u8],
    expected_ledger: &[u8],
) -> Result<(), AmendError> {
    let head = git.head_oid().map_err(AmendError::Git)?;
    if head == plan.head_oid {
        return Err(AmendError::Invalid(
            "amend commit did not advance HEAD".into(),
        ));
    }
    let parent = git
        .run(&["rev-parse", "HEAD^"], None)
        .map_err(AmendError::Git)?;
    if !parent.status.success() || String::from_utf8_lossy(&parent.stdout).trim() != plan.head_oid {
        return Err(AmendError::Invalid(
            "amend commit has unexpected parent".into(),
        ));
    }
    if !git.head_regular(&plan.path).map_err(AmendError::Git)?
        || !git.head_regular(ledger_path).map_err(AmendError::Git)?
    {
        return Err(AmendError::Invalid(
            "amend commit contains non-regular operation entry".into(),
        ));
    }
    let expected_message = format!(
        "amend(frozen): {}\n\nFrozen-Amend-Approved: {}",
        plan.old_path, plan.reason
    );
    if git
        .commit_message()
        .map_err(AmendError::Git)?
        .trim_end_matches('\n')
        != expected_message
    {
        return Err(AmendError::Invalid(
            "amend commit message or trailer was altered".into(),
        ));
    }
    let changes = git.commit_changes().map_err(AmendError::Git)?;
    if changes.len() != 2
        || !changes.iter().all(|change| {
            change.new_path.is_none()
                && (change.old_path == plan.old_path || change.old_path == ledger_rel)
        })
    {
        return Err(AmendError::Invalid(
            "amend commit contains unexpected paths".into(),
        ));
    }
    if git.head_blob(&plan.path).map_err(AmendError::Git)? != staged_note
        || git.head_blob(ledger_path).map_err(AmendError::Git)? != expected_ledger
    {
        return Err(AmendError::Invalid(
            "amend commit tree differs from approved files".into(),
        ));
    }
    if fs::read(&plan.path).map_err(|_| AmendError::Io(plan.path.clone()))? != staged_note
        || fs::read(ledger_path).map_err(|_| AmendError::Io(ledger_path.to_path_buf()))?
            != expected_ledger
    {
        return Err(AmendError::Invalid(
            "amend worktree differs from committed operation files".into(),
        ));
    }
    let before_status = filtered_worktree_status(
        &plan.worktree_status_before,
        &[plan.old_path.as_str(), ledger_rel],
    );
    let after_status = filtered_worktree_status(
        &git.worktree_status().map_err(AmendError::Git)?,
        &[plan.old_path.as_str(), ledger_rel],
    );
    if !git.staged_status().map_err(AmendError::Git)?.is_empty() || after_status != before_status {
        return Err(AmendError::Invalid(
            "amend left unexpected staged or worktree changes".into(),
        ));
    }
    Ok(())
}
fn registry_overlay_snapshot(origin: &Path) -> Result<(PathBuf, Option<Vec<u8>>), AmendError> {
    let stem = origin
        .file_stem()
        .ok_or_else(|| AmendError::Invalid("registry origin has no stem".into()))?;
    let mut name = stem.to_os_string();
    name.push(".local.toml");
    let path = origin.with_file_name(name);
    let bytes = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::canonicalize(&path).map_err(|_| AmendError::Io(path.clone()))?;
            let target_metadata =
                fs::metadata(&target).map_err(|_| AmendError::Io(path.clone()))?;
            if !target_metadata.is_file() {
                return Err(AmendError::Invalid(
                    "registry overlay is not a regular file".into(),
                ));
            }
            Some(fs::read(&target).map_err(|_| AmendError::Io(path.clone()))?)
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(AmendError::Invalid(
                "registry overlay is not a regular file".into(),
            ));
        }
        Ok(_) => Some(fs::read(&path).map_err(|_| AmendError::Io(path.clone()))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(AmendError::Io(path.clone())),
    };
    Ok((path, bytes))
}
fn filtered_worktree_status(status: &[u8], paths: &[&str]) -> Vec<Vec<u8>> {
    status
        .split(|byte| *byte == 0)
        .filter(|entry| {
            if entry.is_empty() {
                return false;
            }
            let path = entry.get(3..).unwrap_or_default();
            !paths.iter().any(|candidate| path == candidate.as_bytes())
        })
        .map(ToOwned::to_owned)
        .collect()
}
fn only_staged_paths(git: &GitBackend, paths: &[&str]) -> bool {
    let Ok(changes) = git.staged_status() else {
        return false;
    };
    changes.len() == paths.len()
        && changes
            .iter()
            .all(|change| change.new_path.is_none() && paths.contains(&change.old_path.as_str()))
}
fn rollback_amend(
    git: &GitBackend,
    path: &Path,
    _relative_path: &str,
    worktree_before: &[u8],
    ledger_path: &Path,
    _ledger_rel: &str,
    ledger_before: Option<Vec<u8>>,
) -> Result<(), AmendError> {
    git.reset_hard("HEAD")?;
    cleanup_untracked_operation_files(git)?;
    if !is_regular_nosymlink(path)
        || GitBackend::write_regular_nosymlink(path, worktree_before).is_err()
        || fs::read(path).ok().as_deref() != Some(worktree_before)
    {
        return Err(AmendError::Invalid("amend rollback is unsafe".into()));
    }
    git.reset_index()?;
    match &ledger_before {
        Some(bytes) => {
            if GitBackend::write_regular_nosymlink(ledger_path, &bytes).is_err()
                || fs::read(ledger_path).ok().as_deref() != Some(bytes.as_slice())
            {
                return Err(AmendError::Invalid("amend rollback is unsafe".into()));
            }
        }
        None => match fs::symlink_metadata(ledger_path) {
            Ok(_) => crate::source::remove_file_nofollow(ledger_path)
                .map_err(|_| AmendError::Invalid("amend rollback is unsafe".into()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AmendError::Invalid("amend rollback is unsafe".into())),
        },
    }
    if !git.staged_status()?.is_empty() {
        return Err(AmendError::Invalid("amend rollback is unsafe".into()));
    }
    Ok(())
}
fn amend_ledger_baseline(git: &GitBackend) -> Result<Option<Vec<u8>>, AmendError> {
    let path = git.root.join(".rhizome/amend-ledger.ndjson");
    let head = match git.head_blob(&path) {
        Ok(bytes) => Some(bytes),
        Err(crate::git::GitError::NotFound(_)) => None,
        Err(error) => return Err(error.into()),
    };
    let worktree = match crate::source::read_regular_file_nofollow_bounded(&path, 16 * 1024 * 1024)
    {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(AmendError::Io(path)),
    };
    match (head, worktree) {
        (Some(head), Some(worktree)) if head == worktree => Ok(Some(worktree)),
        (None, None) => Ok(None),
        _ => Err(AmendError::Invalid(
            "amend ledger changed outside this operation".into(),
        )),
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf())
        == fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf())
}
