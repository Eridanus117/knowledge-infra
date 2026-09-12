use crate::frozen::{FrozenError, is_head_frozen};
use crate::git::{GitBackend, GitError};
use crate::ledger::{self, LedgerError, LedgerRecord, SCHEMA};
use crate::source::{SourceContext, discover_source};
use focaccia::CaseFold;
use kb_contract::{
    DomainId, NoteKind, Registry, SourceSpec, derive_identity, parse_and_validate_note,
};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
#[derive(Debug)]
pub enum RelocateError {
    Git(GitError),
    Frozen(FrozenError),
    Ledger(LedgerError),
    Invalid(String),
    Io(PathBuf),
    Discovery,
}
impl fmt::Display for RelocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(e) => e.fmt(f),
            Self::Frozen(e) => e.fmt(f),
            Self::Ledger(e) => e.fmt(f),
            Self::Invalid(_) => f.write_str("relocate request is invalid"),
            Self::Io(_) => f.write_str("relocate could not access a path"),
            Self::Discovery => f.write_str("source discovery failed"),
        }
    }
}
impl std::error::Error for RelocateError {}
impl From<GitError> for RelocateError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}
impl From<FrozenError> for RelocateError {
    fn from(e: FrozenError) -> Self {
        Self::Frozen(e)
    }
}
impl From<LedgerError> for RelocateError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}

#[derive(Clone, Debug)]
pub struct RelocatePlan {
    source_repo: PathBuf,
    target_repo: PathBuf,
    source_root: PathBuf,
    target_root: PathBuf,
    source_path: PathBuf,
    target_path: PathBuf,
    target_domain: DomainId,
    old_identity: String,
    new_identity: String,
    logical_source: String,
    target_source: String,
    old_path: String,
    new_path: String,
    bytes: Vec<u8>,
    blob_hash: String,
    source_head: String,
    target_head: String,
    reason: String,
    frozen: bool,
    source_spec: SourceSpec,
    target_spec: SourceSpec,
    registry_origin: PathBuf,
    registry_bytes: Vec<u8>,
    overlay_path: PathBuf,
    overlay_bytes: Option<Vec<u8>>,
    target_index_bytes: Vec<u8>,
}
pub fn plan_relocate(
    registry: &Registry,
    source: &Path,
    target: &str,
) -> Result<RelocatePlan, RelocateError> {
    let source_abs =
        fs::canonicalize(source).map_err(|_| RelocateError::Io(source.to_path_buf()))?;
    let source_spec = owning_source(registry, &source_abs)?.0.clone();
    let source_root = fs::canonicalize(&source_spec.root)
        .map_err(|_| RelocateError::Io(source_spec.root.clone()))?;
    let source_git = GitBackend::new(repo_root(&source_spec.root)?)?;
    source_git
        .head_blob(&source_abs)
        .map_err(|_| RelocateError::Invalid("relocate source must be tracked in HEAD".into()))?;
    if !source_git.head_regular(&source_abs)? {
        return Err(RelocateError::Invalid(
            "relocate source HEAD entry is not a regular file".into(),
        ));
    }
    let context = SourceContext {
        source: source_spec.clone(),
        git_root: source_git.root.clone(),
        registry_origin: registry.origin.clone(),
    };
    let snapshot = discover_source(&context).map_err(|_| RelocateError::Discovery)?;
    let source_note = snapshot
        .notes
        .iter()
        .find(|note| note.locator.path == source_abs)
        .ok_or_else(|| RelocateError::Invalid("source is not a discovered note".into()))?;
    if source_note.locator.is_domain_index {
        return Err(RelocateError::Invalid(
            "domain index cannot relocate".into(),
        ));
    }
    let (target_name, target_domain, target_slug) = parse_target(target)?;
    let target_spec = registry
        .sources
        .get(target_name.as_str())
        .ok_or_else(|| RelocateError::Invalid("target source is not registered".into()))?
        .clone();
    let target_git = GitBackend::new(repo_root(&target_spec.root)?)?;
    let target_root = fs::canonicalize(&target_spec.root)
        .map_err(|_| RelocateError::Io(target_spec.root.clone()))?;
    if !source_git.staged_status()?.is_empty() || !target_git.staged_status()?.is_empty() {
        return Err(RelocateError::Invalid(
            "relocate requires clean indexes".into(),
        ));
    }
    if !source_git.attributes_clean() || !target_git.attributes_clean() {
        return Err(RelocateError::Invalid(
            "Git attributes changed outside operation".into(),
        ));
    }
    relocate_ledger_baseline(&source_git, "relocate")?;
    if source_git.root != target_git.root {
        relocate_ledger_baseline(&target_git, "relocate")?;
    }
    let target_context = SourceContext {
        source: target_spec.clone(),
        git_root: target_git.root.clone(),
        registry_origin: registry.origin.clone(),
    };
    let target_snapshot = discover_source(&target_context).map_err(|_| RelocateError::Discovery)?;
    let target_domain_node = target_snapshot
        .domains
        .iter()
        .find(|domain| domain.id == target_domain)
        .ok_or_else(|| {
            RelocateError::Invalid("target domain is not a discovered C2 domain".into())
        })?;
    let target_domain_path = fs::canonicalize(&target_domain_node.physical_dir)
        .map_err(|_| RelocateError::Invalid("target domain disappeared".into()))?;
    if !target_domain_path.starts_with(&target_root) {
        return Err(RelocateError::Invalid(
            "target domain is outside source root".into(),
        ));
    }
    let index = target_domain_path.join("INDEX.md");
    let index_bytes = fs::read(&index)
        .map_err(|_| RelocateError::Invalid("target domain INDEX.md is missing".into()))?;
    let index_note = parse_and_validate_note(&index, &index_bytes)
        .map_err(|_| RelocateError::Invalid("target INDEX.md is invalid".into()))?;
    if index_note.frontmatter.kind != NoteKind::Index {
        return Err(RelocateError::Invalid(
            "target INDEX.md is not an index".into(),
        ));
    }
    if target_git.head_blob(&index)? != index_bytes {
        return Err(RelocateError::Invalid(
            "target INDEX.md worktree differs from HEAD".into(),
        ));
    }
    let slug = target_slug
        .map(|slug| slug.nfc().collect::<String>())
        .unwrap_or_else(|| source_note.locator.slug.clone());
    let new_identity = derive_identity(&target_spec.name, &target_domain, &slug)
        .map_err(|_| RelocateError::Invalid("target slug is invalid".into()))?;
    if target_snapshot
        .notes
        .iter()
        .any(|note| CaseFold::Full.case_eq(note.locator.identity.as_str(), new_identity.as_str()))
    {
        return Err(RelocateError::Invalid(
            "target identity collides under full casefold".into(),
        ));
    }
    let target_path = target_domain_path.join(format!("{slug}.md"));
    if !target_path.starts_with(&target_root)
        || target_path.exists()
        || target_git.head_blob(&target_path).is_ok()
    {
        return Err(RelocateError::Invalid(
            "target path is invalid or already exists in worktree or HEAD".into(),
        ));
    }
    let bytes = fs::read(&source_abs).map_err(|_| RelocateError::Io(source_abs.clone()))?;
    let frozen = is_head_frozen(&source_git, &source_abs)?;
    if frozen && !source_git.worktree_exact_matches_head(&source_abs)? {
        return Err(RelocateError::Frozen(FrozenError::Frozen(
            source_abs.clone(),
        )));
    }
    let target_head = target_git.head_oid()?;
    let source_head = source_git.head_oid()?;
    let old_path = source_git.relative_path(&source_abs)?;
    let new_path = target_git.relative_path(&target_path)?;
    let canonical_target = target_git.canonical_blob_bytes_for_path(&target_path, &bytes)?;
    let target_hash = GitBackend::canonical_blob_sha256(&canonical_target);
    let blob_hash = if frozen {
        let source_hash = source_git.head_blob_sha256(&source_abs)?;
        if source_hash != target_hash {
            return Err(RelocateError::Invalid(
                "frozen relocate attributes would change canonical blob".into(),
            ));
        }
        source_hash
    } else {
        target_hash
    };
    let reason = "approved content-preserving relocate".to_owned();
    let registry_bytes =
        fs::read(&registry.origin).map_err(|_| RelocateError::Io(registry.origin.clone()))?;
    let (overlay_path, overlay_bytes) = registry_overlay_snapshot(&registry.origin)?;
    Ok(RelocatePlan {
        source_repo: source_git.root.clone(),
        target_repo: target_git.root.clone(),
        source_root,
        target_root: target_root.clone(),
        source_path: source_abs,
        target_path,
        target_domain,
        old_identity: source_note.locator.identity.to_string(),
        new_identity: new_identity.to_string(),
        logical_source: source_spec.name.to_string(),
        target_source: target_spec.name.to_string(),
        old_path,
        new_path,
        bytes,
        blob_hash,
        source_head,
        target_head,
        reason,
        frozen,
        source_spec,
        target_spec,
        registry_origin: registry.origin.clone(),
        registry_bytes,
        overlay_path,
        overlay_bytes,
        target_index_bytes: index_bytes,
    })
}

pub fn apply_relocate(plan: &RelocatePlan) -> Result<(), RelocateError> {
    let source_git = GitBackend::new(plan.source_repo.clone())?;
    let target_git = GitBackend::new(plan.target_repo.clone())?;
    if !source_git.staged_status()?.is_empty() || !target_git.staged_status()?.is_empty() {
        return Err(RelocateError::Invalid(
            "relocate requires clean indexes".into(),
        ));
    }
    if !source_git.attributes_clean() || !target_git.attributes_clean() {
        return Err(RelocateError::Invalid(
            "Git attributes changed outside operation".into(),
        ));
    }
    source_git
        .head_blob(&plan.source_path)
        .map_err(|_| RelocateError::Invalid("relocate source must be tracked in HEAD".into()))?;
    relocate_ledger_baseline(&source_git, "relocate")?;
    if source_git.root != target_git.root {
        relocate_ledger_baseline(&target_git, "relocate")?;
    }
    if source_git.root == target_git.root {
        crate::frozen::check_staged_frozen_for_specs(
            &source_git,
            &[plan.source_spec.clone(), plan.target_spec.clone()],
        )?;
    } else {
        crate::frozen::check_staged_target_for_specs(
            &target_git,
            std::slice::from_ref(&plan.target_spec),
        )?;
        crate::frozen::check_staged_frozen_for_specs(
            &source_git,
            std::slice::from_ref(&plan.source_spec),
        )?;
    }
    if source_git.head_oid()? != plan.source_head || target_git.head_oid()? != plan.target_head {
        return Err(RelocateError::Invalid("relocate plan is stale".into()));
    }
    let (overlay_path, overlay_bytes) = registry_overlay_snapshot(&plan.registry_origin)?;
    if overlay_path != plan.overlay_path || overlay_bytes != plan.overlay_bytes {
        return Err(RelocateError::Invalid(
            "source registry overlay changed".into(),
        ));
    }
    if fs::canonicalize(&plan.source_spec.root)
        .map_err(|_| RelocateError::Invalid("source registry changed".into()))?
        != plan.source_root
        || fs::canonicalize(&plan.target_spec.root)
            .map_err(|_| RelocateError::Invalid("target registry changed".into()))?
            != plan.target_root
        || fs::read(&plan.registry_origin)
            .map_err(|_| RelocateError::Invalid("source registry changed".into()))?
            != plan.registry_bytes
    {
        return Err(RelocateError::Invalid("source registry changed".into()));
    }
    let source_context = SourceContext {
        source: plan.source_spec.clone(),
        git_root: source_git.root.clone(),
        registry_origin: plan.registry_origin.clone(),
    };
    let snapshot = discover_source(&source_context).map_err(|_| RelocateError::Discovery)?;
    snapshot
        .notes
        .iter()
        .find(|note| {
            note.locator.path == plan.source_path
                && note.locator.identity.to_string() == plan.old_identity
        })
        .ok_or_else(|| RelocateError::Invalid("source identity changed".into()))?;
    let current =
        fs::read(&plan.source_path).map_err(|_| RelocateError::Io(plan.source_path.clone()))?;
    let current_canonical =
        target_git.canonical_blob_bytes_for_path(&plan.target_path, &current)?;
    if current != plan.bytes
        || GitBackend::canonical_blob_sha256(&current_canonical) != plan.blob_hash
    {
        return Err(RelocateError::Invalid(
            "relocate source bytes changed".into(),
        ));
    }
    if plan.frozen && !source_git.worktree_exact_matches_head(&plan.source_path)? {
        return Err(RelocateError::Frozen(FrozenError::Frozen(
            plan.source_path.clone(),
        )));
    }
    let target_parent = plan
        .target_path
        .parent()
        .ok_or_else(|| RelocateError::Invalid("target has no parent".into()))?;
    let target_parent_real = fs::canonicalize(target_parent)
        .map_err(|_| RelocateError::Invalid("target domain disappeared".into()))?;
    if !target_parent_real.starts_with(&plan.target_root) || target_parent_real != target_parent {
        return Err(RelocateError::Invalid("target parent changed".into()));
    }
    let target_context = SourceContext {
        source: plan.target_spec.clone(),
        git_root: target_git.root.clone(),
        registry_origin: plan.registry_origin.clone(),
    };
    let target_snapshot = discover_source(&target_context).map_err(|_| RelocateError::Discovery)?;
    if !target_snapshot
        .domains
        .iter()
        .any(|domain| domain.id == plan.target_domain)
    {
        return Err(RelocateError::Invalid("target domain changed".into()));
    }
    let target_index = target_parent_real.join("INDEX.md");
    let target_index_bytes = fs::read(&target_index)
        .map_err(|_| RelocateError::Invalid("target domain changed".into()))?;
    if target_index_bytes != plan.target_index_bytes
        || parse_and_validate_note(&target_index, &target_index_bytes)
            .map_err(|_| RelocateError::Invalid("target domain changed".into()))?
            .frontmatter
            .kind
            != NoteKind::Index
    {
        return Err(RelocateError::Invalid("target domain changed".into()));
    }
    if !plan.target_path.starts_with(&plan.target_root)
        || plan.target_path.exists()
        || target_git.head_blob(&plan.target_path).is_ok()
    {
        return Err(RelocateError::Invalid(
            "target path changed or collides with HEAD".into(),
        ));
    }
    let _ = ledger::read(&source_git.root, "relocate")?;
    if source_git.root != target_git.root {
        let _ = ledger::read(&target_git.root, "relocate")?;
    }
    let source_ledger = source_git.root.join(".rhizome/relocate-ledger.ndjson");
    let target_ledger = target_git.root.join(".rhizome/relocate-ledger.ndjson");
    let source_before = read_ledger_snapshot(&source_ledger)?;
    let target_before = if source_git.root == target_git.root {
        source_before.clone()
    } else {
        read_ledger_snapshot(&target_ledger)?
    };
    let source_ledger_rel = source_git.relative_path(&source_ledger)?;
    let target_ledger_rel = target_git.relative_path(&target_ledger)?;
    if source_git.head_oid()? != plan.source_head || target_git.head_oid()? != plan.target_head {
        return Err(RelocateError::Invalid("relocate plan became stale".into()));
    }
    if source_git.root != target_git.root {
        crate::frozen::check_staged_target_for_specs(
            &target_git,
            std::slice::from_ref(&plan.target_spec),
        )?;
        crate::frozen::check_staged_frozen_for_specs(
            &source_git,
            std::slice::from_ref(&plan.source_spec),
        )?;
    }
    let metadata = fs::symlink_metadata(&plan.target_path).ok();
    if metadata.is_some()
        || fs::canonicalize(target_parent).ok().as_ref() != Some(&target_parent_real)
    {
        return Err(RelocateError::Invalid("target changed before write".into()));
    }
    if fs::read(&plan.source_path).ok().as_deref() != Some(current.as_slice()) {
        return Err(RelocateError::Invalid(
            "relocate source changed before write".into(),
        ));
    }
    let mut destination = crate::source::create_regular_file_nofollow(&plan.target_path)
        .map_err(|_| RelocateError::Io(plan.target_path.clone()))?;
    use std::io::Write;
    if destination
        .write_all(&current)
        .and_then(|_| destination.sync_all())
        .is_err()
    {
        drop(destination);
        let rollback = remove_created_file(&plan.target_path);
        return match rollback {
            Ok(()) => Err(RelocateError::Io(plan.target_path.clone())),
            Err(error) => Err(error),
        };
    }
    drop(destination);
    let destination_bytes = fs::read(&plan.target_path).unwrap_or_default();
    let destination_canonical =
        target_git.canonical_blob_bytes_for_path(&plan.target_path, &destination_bytes)?;
    if destination_bytes != current
        || GitBackend::canonical_blob_sha256(&destination_canonical) != plan.blob_hash
    {
        let rollback = remove_created_file(&plan.target_path);
        return match rollback {
            Ok(()) => Err(RelocateError::Invalid("destination bytes changed".into())),
            Err(error) => Err(error),
        };
    }
    let source_record = LedgerRecord {
        schema: SCHEMA.into(),
        operation: "relocate".into(),
        logical_source: plan.logical_source.clone(),
        old_identity: plan.old_identity.clone(),
        new_identity: plan.new_identity.clone(),
        old_path: plan.old_path.clone(),
        new_path: plan.new_path.clone(),
        head_oid: plan.source_head.clone(),
        canonical_git_blob_sha256: plan.blob_hash.clone(),
        reason: plan.reason.clone(),
    };
    if !heads_match(
        &source_git,
        &plan.source_head,
        &target_git,
        &plan.target_head,
    ) {
        let rollback = remove_created_file(&plan.target_path);
        return match rollback {
            Ok(()) => Err(RelocateError::Invalid("relocate plan became stale".into())),
            Err(error) => Err(error),
        };
    }
    if let Err(error) = ledger::append(&source_git.root, &source_record) {
        let rollback = rollback_relocate(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &source_ledger_rel,
            &target_ledger_rel,
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    let target_record = if source_git.root != target_git.root {
        Some(LedgerRecord {
            schema: SCHEMA.into(),
            operation: "relocate".into(),
            logical_source: plan.target_source.clone(),
            old_identity: plan.old_identity.clone(),
            new_identity: plan.new_identity.clone(),
            old_path: plan.old_path.clone(),
            new_path: plan.new_path.clone(),
            head_oid: plan.target_head.clone(),
            canonical_git_blob_sha256: plan.blob_hash.clone(),
            reason: plan.reason.clone(),
        })
    } else {
        None
    };
    if let Some(record) = target_record.as_ref() {
        if let Err(error) = ledger::append(&target_git.root, record) {
            let rollback = rollback_relocate(
                &source_git,
                &target_git,
                &source_ledger,
                &target_ledger,
                source_before.clone(),
                target_before.clone(),
                &plan.target_path,
                &source_ledger_rel,
                &target_ledger_rel,
            );
            return match rollback {
                Ok(()) => Err(error.into()),
                Err(error) => Err(error),
            };
        }
    }
    let source_paths = vec![source_ledger_rel.clone()];
    if let Err(error) = source_git.add(&source_paths) {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    if source_git.root != target_git.root {
        if let Err(error) = target_git.add(std::slice::from_ref(&target_ledger_rel)) {
            let rollback = rollback_relocate_staged(
                &source_git,
                &target_git,
                &source_ledger,
                &target_ledger,
                source_before.clone(),
                target_before.clone(),
                &plan.target_path,
                &plan.source_path,
                &plan.old_path,
                &plan.new_path,
                &source_ledger_rel,
                &target_ledger_rel,
                &current,
            );
            return match rollback {
                Ok(()) => Err(error.into()),
                Err(error) => Err(error),
            };
        }
    }
    let destination_add = target_git.add(std::slice::from_ref(&plan.new_path));
    if let Err(error) = destination_add {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    if !heads_match(
        &source_git,
        &plan.source_head,
        &target_git,
        &plan.target_head,
    ) || fs::read(&plan.source_path).ok().as_deref() != Some(current.as_slice())
        || !is_regular_nosymlink(&plan.source_path)
    {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(RelocateError::Invalid(
                "relocate source changed before removal".into(),
            )),
            Err(error) => Err(error),
        };
    }
    if crate::source::remove_file_nofollow(&plan.source_path).is_err() {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(RelocateError::Io(plan.source_path.clone())),
            Err(error) => Err(error),
        };
    }
    if let Err(error) = source_git.add_all(std::slice::from_ref(&plan.old_path)) {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(error) => Err(error),
        };
    }
    let source_record_ok = verify_staged_record(&source_git, &source_ledger, &source_record);
    let target_record_ok = target_record.as_ref().map_or(true, |record| {
        verify_staged_record(&target_git, &target_ledger, record)
    });
    if !source_record_ok || !target_record_ok {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(RelocateError::Invalid(
                "relocate ledger provenance changed".into(),
            )),
            Err(error) => Err(error),
        };
    }
    let source_specs = [plan.source_spec.clone(), plan.target_spec.clone()];
    let gate_ok = if source_git.root == target_git.root {
        crate::frozen::check_staged_frozen_for_specs(&source_git, &source_specs).is_ok()
    } else {
        crate::frozen::check_staged_frozen_pair(
            &source_git,
            std::slice::from_ref(&plan.source_spec),
            &target_git,
            std::slice::from_ref(&plan.target_spec),
        )
        .is_ok()
    };
    if !gate_ok {
        let rollback = rollback_relocate_staged(
            &source_git,
            &target_git,
            &source_ledger,
            &target_ledger,
            source_before.clone(),
            target_before.clone(),
            &plan.target_path,
            &plan.source_path,
            &plan.old_path,
            &plan.new_path,
            &source_ledger_rel,
            &target_ledger_rel,
            &current,
        );
        return match rollback {
            Ok(()) => Err(RelocateError::Invalid(
                "relocate staged gate rejected provenance".into(),
            )),
            Err(error) => Err(error),
        };
    }
    Ok(())
}
fn relocate_ledger_baseline(
    git: &GitBackend,
    operation: &str,
) -> Result<Option<Vec<u8>>, RelocateError> {
    let path = git
        .root
        .join(".rhizome")
        .join(format!("{operation}-ledger.ndjson"));
    let head = match git.head_blob(&path) {
        Ok(bytes) => Some(bytes),
        Err(crate::git::GitError::NotFound(_)) => None,
        Err(error) => return Err(error.into()),
    };
    let worktree = match crate::source::read_regular_file_nofollow_bounded(&path, 16 * 1024 * 1024)
    {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(RelocateError::Io(path)),
    };
    match (head, worktree) {
        (Some(head), Some(worktree)) if head == worktree => Ok(Some(worktree)),
        (None, None) => Ok(None),
        _ => Err(RelocateError::Invalid(
            "relocate ledger changed outside this operation".into(),
        )),
    }
}
fn heads_match(
    source: &GitBackend,
    source_head: &str,
    target: &GitBackend,
    target_head: &str,
) -> bool {
    matches!(source.head_oid(), Ok(actual) if actual == source_head)
        && matches!(target.head_oid(), Ok(actual) if actual == target_head)
}
fn read_ledger_snapshot(path: &Path) -> Result<Option<Vec<u8>>, RelocateError> {
    match crate::source::read_regular_file_nofollow_bounded(path, 16 * 1024 * 1024) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(RelocateError::Io(path.to_path_buf())),
    }
}

fn is_regular_nosymlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}
fn remove_created_file(path: &Path) -> Result<(), RelocateError> {
    match fs::symlink_metadata(path) {
        Ok(_) => crate::source::remove_file_nofollow(path)
            .map_err(|_| RelocateError::Invalid("relocate rollback is unsafe".into()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(RelocateError::Invalid("relocate rollback is unsafe".into())),
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Err(RelocateError::Invalid("relocate rollback is unsafe".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RelocateError::Invalid("relocate rollback is unsafe".into())),
    }
}
fn registry_overlay_snapshot(origin: &Path) -> Result<(PathBuf, Option<Vec<u8>>), RelocateError> {
    let stem = origin
        .file_stem()
        .ok_or_else(|| RelocateError::Invalid("registry origin has no stem".into()))?;
    let mut name = stem.to_os_string();
    name.push(".local.toml");
    let path = origin.with_file_name(name);
    let bytes = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::canonicalize(&path).map_err(|_| RelocateError::Io(path.clone()))?;
            let target_metadata =
                fs::metadata(&target).map_err(|_| RelocateError::Io(path.clone()))?;
            if !target_metadata.is_file() {
                return Err(RelocateError::Invalid(
                    "registry overlay is not a regular file".into(),
                ));
            }
            Some(fs::read(&target).map_err(|_| RelocateError::Io(path.clone()))?)
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(RelocateError::Invalid(
                "registry overlay is not a regular file".into(),
            ));
        }
        Ok(_) => Some(fs::read(&path).map_err(|_| RelocateError::Io(path.clone()))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(RelocateError::Io(path.clone())),
    };
    Ok((path, bytes))
}
fn restore_file(path: &Path, bytes: Option<Vec<u8>>) -> Result<(), RelocateError> {
    match bytes {
        Some(bytes) => {
            if !is_regular_nosymlink(path) && fs::symlink_metadata(path).is_ok() {
                return Err(RelocateError::Invalid("relocate rollback is unsafe".into()));
            }
            GitBackend::write_regular_nosymlink(path, &bytes)
                .map_err(|_| RelocateError::Invalid("relocate rollback is unsafe".into()))?;
            if fs::read(path).ok().as_deref() != Some(bytes.as_slice()) {
                return Err(RelocateError::Invalid("relocate rollback is unsafe".into()));
            }
        }
        None => remove_created_file(path)?,
    }
    Ok(())
}
fn verify_staged_record(git: &GitBackend, ledger_path: &Path, expected: &LedgerRecord) -> bool {
    let Ok(bytes) = git.staged_blob(ledger_path) else {
        return false;
    };
    crate::ledger::parse_bytes(&bytes, "relocate").map_or(false, |records| {
        records.iter().any(|record| record == expected)
    })
}
fn rollback_relocate_staged(
    source_git: &GitBackend,
    target_git: &GitBackend,
    source_ledger: &Path,
    target_ledger: &Path,
    source_before: Option<Vec<u8>>,
    target_before: Option<Vec<u8>>,
    destination: &Path,
    source_path: &Path,
    _source_rel: &str,
    _target_path: &str,
    _source_ledger_rel: &str,
    _target_ledger_rel: &str,
    source_bytes: &[u8],
) -> Result<(), RelocateError> {
    remove_created_file(destination)?;
    if fs::symlink_metadata(source_path).is_err() {
        let mut file = crate::source::create_regular_file_nofollow(source_path)
            .map_err(|_| RelocateError::Invalid("relocate rollback is unsafe".into()))?;
        use std::io::Write;
        file.write_all(source_bytes)
            .map_err(|_| RelocateError::Invalid("relocate rollback is unsafe".into()))?;
        file.sync_all()
            .map_err(|_| RelocateError::Invalid("relocate rollback is unsafe".into()))?;
    }
    restore_file(source_ledger, source_before.clone())?;
    if source_git.root != target_git.root {
        restore_file(target_ledger, target_before.clone())?;
    }
    source_git.reset_index()?;
    if source_git.root != target_git.root {
        target_git.reset_index()?;
    }
    let source_status = source_git.staged_status()?;
    let target_status = if source_git.root != target_git.root {
        target_git.staged_status()?
    } else {
        Vec::new()
    };
    if !source_status.is_empty() || !target_status.is_empty() {
        let names = source_status
            .iter()
            .map(|change| change.old_path.clone())
            .collect::<Vec<_>>()
            .join(",");
        return Err(RelocateError::Invalid(format!(
            "relocate rollback is unsafe (source {}, target {}, paths {names})",
            source_status.len(),
            target_status.len()
        )));
    }
    Ok(())
}
fn rollback_relocate(
    source_git: &GitBackend,
    target_git: &GitBackend,
    source_ledger: &Path,
    target_ledger: &Path,
    source_before: Option<Vec<u8>>,
    target_before: Option<Vec<u8>>,
    destination: &Path,
    _source_ledger_rel: &str,
    _target_ledger_rel: &str,
) -> Result<(), RelocateError> {
    remove_created_file(destination)?;
    restore_file(source_ledger, source_before.clone())?;
    if source_git.root != target_git.root {
        restore_file(target_ledger, target_before.clone())?;
    }
    source_git.reset_index()?;
    if source_git.root != target_git.root {
        target_git.reset_index()?;
    }
    Ok(())
}
fn owning_source<'a>(
    registry: &'a Registry,
    path: &Path,
) -> Result<(&'a SourceSpec, PathBuf), RelocateError> {
    let mut found: Option<(&SourceSpec, PathBuf)> = None;
    for spec in registry.sources.values() {
        let root =
            fs::canonicalize(&spec.root).map_err(|_| RelocateError::Io(spec.root.clone()))?;
        if path.starts_with(&root)
            && found.as_ref().map_or(true, |(_, current)| {
                root.components().count() > current.components().count()
            })
        {
            found = Some((spec, root));
        }
    }
    found.ok_or_else(|| RelocateError::Invalid("source is not inside a registered source".into()))
}
fn repo_root(source_root: &Path) -> Result<PathBuf, RelocateError> {
    let mut current = source_root.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Ok(current);
        }
        let Some(parent) = current.parent() else {
            return Err(RelocateError::Invalid("source Git root is missing".into()));
        };
        if parent == current {
            return Err(RelocateError::Invalid("source Git root is missing".into()));
        }
        current = parent.to_path_buf();
    }
}
fn parse_target(target: &str) -> Result<(String, DomainId, Option<String>), RelocateError> {
    let fields: Vec<&str> = target.split(':').collect();
    if fields.len() != 2 && fields.len() != 3 || fields[0].is_empty() || fields[1].is_empty() {
        return Err(RelocateError::Invalid(
            "target must be logical-source:domain[:slug]".into(),
        ));
    }
    let domain = DomainId::new(fields[1])
        .map_err(|_| RelocateError::Invalid("target domain is invalid".into()))?;
    let slug = fields.get(2).map(|s| (*s).to_owned());
    Ok((fields[0].to_owned(), domain, slug))
}
