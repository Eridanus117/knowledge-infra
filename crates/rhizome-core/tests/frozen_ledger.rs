use kb_contract::{Registry, RegistryLocator, parse_and_validate_note, resolve_registry};
use rhizome_core::frozen::{
    ApprovalMarker, check_staged_frozen, check_staged_frozen_for_specs, check_staged_frozen_pair,
    check_worktree_change, is_head_frozen,
};
use rhizome_core::git::GitBackend;
use rhizome_core::relocate::plan_relocate;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after the epoch")
            .as_millis();
        let path = std::env::temp_dir().join(format!(
            "rhizome-task7-frozen-{millis}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "rhizome-test")
        .env("GIT_AUTHOR_EMAIL", "rhizome-test@example.invalid")
        .env("GIT_COMMITTER_NAME", "rhizome-test")
        .env("GIT_COMMITTER_EMAIL", "rhizome-test@example.invalid")
        .output()
        .expect("system git must be installed for Task7 tests");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_stdout(root: &Path, args: &[&str]) -> Vec<u8> {
    git(root, args).stdout
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).expect("repository directory should be created");
    git(root, &["init", "--quiet"]);
    git(root, &["config", "user.name", "rhizome-test"]);
    git(
        root,
        &["config", "user.email", "rhizome-test@example.invalid"],
    );
}

fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "--all"]);
    git(root, &["commit", "--quiet", "-m", message]);
}

fn index_bytes() -> Vec<u8> {
    b"---\ndescription: domain index\nkeywords: [fixture]\nkind: index\n---\n# Domain\n".to_vec()
}

fn frozen_bytes(body: &str) -> Vec<u8> {
    format!(
        "---\ndescription: frozen reference\nkeywords: [fixture]\nkind: reference\nstatus: frozen\n---\n{body}"
    )
    .into_bytes()
}

fn living_bytes(body: &str) -> Vec<u8> {
    format!("---\ndescription: living reference\nkeywords: [fixture]\nkind: reference\n---\n{body}")
        .into_bytes()
}

fn write_domain(root: &Path, name: &str) -> PathBuf {
    let domain = root.join(name);
    fs::create_dir_all(&domain).expect("domain should be created");
    let index = domain.join("INDEX.md");
    let bytes = index_bytes();
    parse_and_validate_note(&index, &bytes).expect("index fixture must satisfy frontmatter v2");
    fs::write(&index, bytes).expect("index should be written");
    domain
}

fn write_registry(root: &Path, sources: &[(&str, &Path)]) -> Registry {
    let registry_path = root.join("sources.toml");
    let mut text = String::new();
    for (name, source) in sources {
        let path = source.to_string_lossy().replace('\\', "/");
        text.push_str(&format!(
            "[[source]]\nname = \"{name}\"\npath = \"{path}\"\nsurface = \"core\"\n\n"
        ));
    }
    fs::write(&registry_path, text).expect("registry should be written");
    let locator = RegistryLocator {
        explicit: Some(registry_path.clone()),
        cwd: root.to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: root.join("unused.toml"),
    };
    resolve_registry(&locator).expect("temporary registry should resolve")
}

fn canonical_source_context(
    registry: &Registry,
    name: &str,
    git_root: &Path,
) -> rhizome_core::SourceContext {
    let source = registry
        .sources
        .get(name)
        .expect("registry source should exist")
        .clone();
    rhizome_core::SourceContext {
        source,
        git_root: git_root.to_path_buf(),
        registry_origin: registry.origin.clone(),
    }
}

fn ledger_line(
    operation: &str,
    logical_source: &str,
    old_identity: &str,
    new_identity: &str,
    old_path: &str,
    new_path: &str,
    head_oid: &str,
    hash: &str,
    reason: &str,
) -> String {
    format!(
        "{{\"schema\":\"frozen-ledger-v2\",\"operation\":\"{operation}\",\"logical_source\":\"{logical_source}\",\"old_identity\":\"{old_identity}\",\"new_identity\":\"{new_identity}\",\"old_path\":\"{old_path}\",\"new_path\":\"{new_path}\",\"head_oid\":\"{head_oid}\",\"canonical_git_blob_sha256\":\"{hash}\",\"reason\":\"{reason}\"}}\n"
    )
}

#[test]
fn new_frozen_note_is_not_head_frozen() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("new.md");
    let bytes = frozen_bytes("# New\n");
    parse_and_validate_note(&note, &bytes).expect("new frozen fixture must be valid");
    fs::write(&note, &bytes).expect("new frozen note should be written");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");

    assert_eq!(
        is_head_frozen(&backend, &note).expect("missing HEAD path is a valid query"),
        false,
        "new frozen notes are not frozen predecessors until committed"
    );
    check_worktree_change(&backend, &note, None)
        .expect("a new frozen note must not be blocked by the HEAD gate");
}

#[test]
fn editing_head_frozen_note_is_rejected_from_committed_frontmatter() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");

    fs::write(&note, living_bytes("# Frozen\n\nchanged\n")).expect("downgrade should be written");
    assert!(
        check_worktree_change(&backend, &note, None).is_err(),
        "a valid downgrade must still be judged against frozen HEAD frontmatter"
    );
}

#[test]
fn direct_edit_of_head_frozen_note_is_rejected() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");

    fs::write(&note, frozen_bytes("# Frozen\n\nedited in place\n"))
        .expect("edit should be written");
    assert!(check_worktree_change(&backend, &note, None).is_err());
}

#[test]
fn staged_delete_of_head_frozen_note_is_rejected() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    git(&repo, &["rm", "--quiet", "--", "docs/frozen.md"]);
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");

    assert!(check_staged_frozen(&backend).is_err());
}

#[test]
fn staged_rename_of_head_frozen_note_is_rejected() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    git(&repo, &["mv", "--", "docs/frozen.md", "docs/renamed.md"]);
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");

    assert!(check_staged_frozen(&backend).is_err());
}

#[test]
fn forged_canonical_blob_hash_does_not_unlock_staged_delete() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");
    let head = backend.head_oid().expect("HEAD OID should be readable");
    let ledger_dir = repo.join(".rhizome");
    fs::create_dir_all(&ledger_dir).expect("ledger directory should be created");
    let forged = ledger_line(
        "relocate",
        "knowledge",
        "knowledge:docs:frozen",
        "knowledge:archive:frozen",
        "docs/frozen.md",
        "archive/frozen.md",
        &head,
        &"00".repeat(32),
        "forged hash",
    );
    fs::write(ledger_dir.join("relocate-ledger.ndjson"), forged)
        .expect("forged ledger should be written");
    git(&repo, &["rm", "--quiet", "--", "docs/frozen.md"]);

    assert!(
        check_staged_frozen(&backend).is_err(),
        "a ledger row with a forged canonical digest must not unlock a delete"
    );
}

#[test]
fn stale_relocate_plan_is_rejected_before_touching_bytes() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let registry = write_registry(scratch.path(), &[("knowledge", &repo)]);
    let plan = plan_relocate(&registry, &note, "knowledge:archive")
        .expect("valid relocate plan should be produced");
    let unrelated = archive.join("unrelated.md");
    fs::write(&unrelated, living_bytes("# Unrelated\n")).expect("unrelated note should be written");
    commit_all(&repo, "advance HEAD after planning");

    assert!(rhizome_core::relocate::apply_relocate(&plan).is_err());
    assert!(
        note.is_file(),
        "stale apply must leave the source bytes untouched"
    );
    assert!(
        !archive.join("frozen.md").exists(),
        "stale apply must not create a destination"
    );
    assert!(
        !repo.join(".rhizome/relocate-ledger.ndjson").exists(),
        "stale apply must not append provenance"
    );
}

#[test]
fn canonical_hash_uses_lf_git_blob_not_crlf_worktree_bytes() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join(".gitattributes"), b"*.md text eol=lf\n")
        .expect("attributes should be written");
    let domain = write_domain(&repo, "docs");
    let note = domain.join("crlf.md");
    let lf = frozen_bytes("# LF blob\n");
    let mut crlf = Vec::with_capacity(lf.len() + 8);
    for byte in &lf {
        if *byte == b'\n' {
            crlf.extend_from_slice(b"\r\n");
        } else {
            crlf.push(*byte);
        }
    }
    fs::write(&note, &lf).expect("LF note should be written");
    commit_all(&repo, "seed LF blob");
    fs::write(&note, &crlf).expect("CRLF worktree materialization should be written");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");

    let expected = GitBackend::canonical_blob_sha256(&lf);
    let worktree_hash = GitBackend::canonical_blob_sha256(&crlf);
    assert_ne!(
        expected, worktree_hash,
        "CRLF bytes must have a different digest"
    );
    assert_eq!(
        backend
            .head_blob(&note)
            .expect("HEAD blob should be readable"),
        lf
    );
    assert_eq!(
        backend
            .head_blob_sha256(&note)
            .expect("canonical Git-blob SHA256 should be readable"),
        expected,
        "canonical digest must be computed from LF bytes returned by git show"
    );
}

#[allow(dead_code)]
#[test]
fn relative_approval_marker_cannot_unlock_frozen_edit() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("note");
    commit_all(&repo, "seed frozen");
    fs::write(&note, frozen_bytes("# Frozen\n\nchanged\n")).expect("edit");
    let backend = GitBackend::new(repo).expect("backend");
    let marker = ApprovalMarker::for_one_file(PathBuf::from("docs/frozen.md"), "relative");
    assert!(check_worktree_change(&backend, &note, Some(&marker)).is_err());
}
fn _assert_fixture_shape(path: &Path, bytes: &[u8]) {
    parse_and_validate_note(path, bytes).expect("Task7 fixture should be valid frontmatter");
}

#[allow(dead_code)]
fn _unused_git_show(root: &Path, path: &str) -> Vec<u8> {
    git_stdout(root, &["show", &format!("HEAD:{path}")])
}

#[allow(dead_code)]
fn _approval(path: &Path) -> ApprovalMarker {
    ApprovalMarker::for_one_file(path.to_path_buf(), "test approval")
}

#[test]
fn staged_invalid_head_note_is_ignored_by_frozen_gate() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    fs::write(domain.join("README.md"), b"ordinary README\n").expect("README should be written");
    commit_all(&repo, "seed ordinary README");
    fs::write(domain.join("README.md"), b"ordinary README changed\n")
        .expect("README should change");
    git(&repo, &["add", "--", "docs/README.md"]);
    let backend = GitBackend::new(repo).expect("Git backend should resolve");
    check_staged_frozen(&backend).expect("ordinary invalid v2 notes must not trigger frozen gate");
}

#[test]
fn duplicate_ledger_key_fails_closed_before_staged_gate() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    fs::write(&note, frozen_bytes("# Frozen\n")).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory should be created");
    let head = GitBackend::new(repo.clone())
        .expect("Git backend should resolve")
        .head_oid()
        .expect("head should resolve");
    let hash = GitBackend::canonical_blob_sha256(&frozen_bytes("# Frozen\n"));
    let line = ledger_line(
        "relocate",
        "knowledge",
        "knowledge:docs:frozen",
        "knowledge:archive:frozen",
        "docs/frozen.md",
        "archive/frozen.md",
        &head,
        &hash,
        "reason",
    );
    let duplicate = line.replacen(
        "\"schema\":\"frozen-ledger-v2\"",
        "\"schema\":\"frozen-ledger-v2\",\"schema\":\"frozen-ledger-v2\"",
        1,
    );
    fs::write(repo.join(".rhizome/relocate-ledger.ndjson"), duplicate)
        .expect("malformed ledger should be written");
    git(&repo, &["rm", "--quiet", "--", "docs/frozen.md"]);
    let backend = GitBackend::new(repo).expect("Git backend should resolve");
    assert!(
        check_staged_frozen(&backend).is_err(),
        "duplicate ledger keys must fail closed"
    );
}

#[test]
fn unstaged_valid_relocate_ledger_does_not_unlock_staged_delete() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    let bytes = frozen_bytes("# Frozen\n");
    fs::write(&note, &bytes).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");
    let head = backend.head_oid().expect("head should resolve");
    let hash = backend
        .head_blob_sha256(&note)
        .expect("blob hash should resolve");
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory should be created");
    fs::write(
        repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "knowledge",
            "knowledge:docs:frozen",
            "knowledge:archive:frozen",
            "docs/frozen.md",
            "archive/frozen.md",
            &head,
            &hash,
            "unstaged approval",
        ),
    )
    .expect("ledger should be written");
    git(&repo, &["rm", "--quiet", "--", "docs/frozen.md"]);
    assert!(
        check_staged_frozen(&backend).is_err(),
        "unstaged ledger rows must not authorize staged deletes"
    );
}

#[test]
fn staged_relocate_requires_logical_source_match() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    let note = domain.join("frozen.md");
    let bytes = frozen_bytes("# Frozen\n");
    fs::write(&note, &bytes).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");
    let head = backend.head_oid().expect("head should resolve");
    let hash = backend
        .head_blob_sha256(&note)
        .expect("blob hash should resolve");
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory should be created");
    fs::write(
        repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "forged",
            "knowledge:docs:frozen",
            "knowledge:archive:frozen",
            "docs/frozen.md",
            "archive/frozen.md",
            &head,
            &hash,
            "same reason",
        ),
    )
    .expect("ledger should be written");
    git(&repo, &["rm", "--quiet", "--", "docs/frozen.md"]);
    assert!(
        check_staged_frozen(&backend).is_err(),
        "logical_source must match the approved identity"
    );
}

#[test]
fn staged_same_repo_cross_source_relocate_requires_destination_blob() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo, "docs");
    write_domain(&repo, "shared/archive");
    let note = domain.join("frozen.md");
    let bytes = frozen_bytes("# Frozen\n");
    fs::write(&note, &bytes).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let backend = GitBackend::new(repo.clone()).expect("Git backend should resolve");
    let head = backend.head_oid().expect("head should resolve");
    let hash = backend
        .head_blob_sha256(&note)
        .expect("blob hash should resolve");
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory should be created");
    fs::write(
        repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "knowledge",
            "knowledge:docs:frozen",
            "shared:archive:frozen",
            "docs/frozen.md",
            "shared/archive/frozen.md",
            &head,
            &hash,
            "cross-source",
        ),
    )
    .expect("ledger should be written");
    git(&repo, &["add", "--", ".rhizome/relocate-ledger.ndjson"]);
    git(&repo, &["rm", "--quiet", "--", "docs/frozen.md"]);
    assert!(
        check_staged_frozen(&backend).is_err(),
        "cross-source relocation must prove its staged destination bytes"
    );
}

#[test]
fn cross_repo_relocate_staged_gates_validate_each_repository_role() {
    let scratch = Scratch::new();
    let source_repo = scratch.path().join("source-repo");
    let target_repo = scratch.path().join("target-repo");
    init_repo(&source_repo);
    init_repo(&target_repo);
    let source_domain = write_domain(&source_repo, "docs");
    let target_domain = write_domain(&target_repo, "archive");
    let note = source_domain.join("cross.md");
    let bytes = frozen_bytes("# Cross\n");
    fs::write(&note, &bytes).expect("source note should be written");
    commit_all(&source_repo, "seed source");
    commit_all(&target_repo, "seed target");
    let registry = write_registry(
        &scratch.path(),
        &[("knowledge", &source_repo), ("shared", &target_repo)],
    );
    let source_backend = GitBackend::new(source_repo.clone()).expect("source backend");
    let target_backend = GitBackend::new(target_repo.clone()).expect("target backend");
    let source_head = source_backend.head_oid().expect("source head");
    let target_head = target_backend.head_oid().expect("target head");
    let hash = source_backend.head_blob_sha256(&note).expect("source hash");
    let source_line = ledger_line(
        "relocate",
        "knowledge",
        "knowledge:docs:cross",
        "shared:archive:cross",
        "docs/cross.md",
        "archive/cross.md",
        &source_head,
        &hash,
        "cross repo",
    );
    let target_line = ledger_line(
        "relocate",
        "shared",
        "knowledge:docs:cross",
        "shared:archive:cross",
        "docs/cross.md",
        "archive/cross.md",
        &target_head,
        &hash,
        "cross repo",
    );
    fs::create_dir_all(source_repo.join(".rhizome")).expect("source ledger directory");
    fs::create_dir_all(target_repo.join(".rhizome")).expect("target ledger directory");
    fs::write(
        source_repo.join(".rhizome/relocate-ledger.ndjson"),
        source_line,
    )
    .expect("source ledger");
    fs::write(
        target_repo.join(".rhizome/relocate-ledger.ndjson"),
        target_line,
    )
    .expect("target ledger");
    git(
        &source_repo,
        &["add", "--", ".rhizome/relocate-ledger.ndjson"],
    );
    git(&source_repo, &["rm", "--quiet", "--", "docs/cross.md"]);
    fs::write(target_domain.join("cross.md"), &bytes).expect("target note");
    git(
        &target_repo,
        &[
            "add",
            "--",
            ".rhizome/relocate-ledger.ndjson",
            "archive/cross.md",
        ],
    );
    let source_spec = registry
        .sources
        .get("knowledge")
        .expect("source spec")
        .clone();
    let target_spec = registry.sources.get("shared").expect("target spec").clone();
    let pair = check_staged_frozen_pair(
        &source_backend,
        &[source_spec],
        &target_backend,
        &[target_spec],
    );
    assert!(pair.is_ok(), "pair gate: {pair:?}");
}

#[test]
fn cross_repo_staged_frozen_rename_is_rejected() {
    let scratch = Scratch::new();
    let source_repo = scratch.path().join("source-rename");
    let target_repo = scratch.path().join("target-rename");
    init_repo(&source_repo);
    init_repo(&target_repo);
    let source_domain = write_domain(&source_repo, "docs");
    let target_domain = write_domain(&target_repo, "archive");
    let note = source_domain.join("cross.md");
    let bytes = frozen_bytes("# Cross rename\n");
    fs::write(&note, &bytes).expect("source note");
    commit_all(&source_repo, "seed source");
    commit_all(&target_repo, "seed target");
    let source_backend = GitBackend::new(source_repo.clone()).expect("source backend");
    let target_backend = GitBackend::new(target_repo.clone()).expect("target backend");
    let source_head = source_backend.head_oid().expect("source head");
    let target_head = target_backend.head_oid().expect("target head");
    let hash = source_backend.head_blob_sha256(&note).expect("source hash");
    fs::create_dir_all(source_repo.join(".rhizome")).expect("source ledger directory");
    fs::create_dir_all(target_repo.join(".rhizome")).expect("target ledger directory");
    fs::write(
        source_repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "knowledge",
            "knowledge:docs:cross",
            "shared:archive:cross",
            "docs/cross.md",
            "archive/cross.md",
            &source_head,
            &hash,
            "cross rename",
        ),
    )
    .expect("source ledger");
    fs::write(
        target_repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "shared",
            "knowledge:docs:cross",
            "shared:archive:cross",
            "docs/cross.md",
            "archive/cross.md",
            &target_head,
            &hash,
            "cross rename",
        ),
    )
    .expect("target ledger");
    git(
        &source_repo,
        &["add", "--", ".rhizome/relocate-ledger.ndjson"],
    );
    git(
        &source_repo,
        &["mv", "--", "docs/cross.md", "docs/renamed.md"],
    );
    fs::write(target_domain.join("cross.md"), &bytes).expect("target note");
    git(
        &target_repo,
        &[
            "add",
            "--",
            ".rhizome/relocate-ledger.ndjson",
            "archive/cross.md",
        ],
    );
    let registry = write_registry(
        &scratch.path(),
        &[("knowledge", &source_repo), ("shared", &target_repo)],
    );
    let source_spec = registry
        .sources
        .get("knowledge")
        .expect("source spec")
        .clone();
    let target_spec = registry.sources.get("shared").expect("target spec").clone();
    assert!(
        check_staged_frozen_pair(
            &source_backend,
            &[source_spec],
            &target_backend,
            &[target_spec]
        )
        .is_err()
    );
}

#[test]
fn staged_frozen_note_in_non_domain_subdir_uses_nearest_c2_domain() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("subdir/nested.md");
    fs::create_dir_all(note.parent().unwrap()).expect("subdir should be created");
    let bytes = frozen_bytes("# Nested\n");
    fs::write(&note, &bytes).expect("nested note should be written");
    commit_all(&repo, "seed nested frozen note");
    let registry = write_registry(&scratch.path(), &[("knowledge", &repo)]);
    let backend = GitBackend::new(repo.clone()).expect("backend");
    let head = backend.head_oid().expect("head");
    let hash = backend.head_blob_sha256(&note).expect("hash");
    fs::write(archive.join("nested.md"), &bytes).expect("destination note");
    git(&repo, &["add", "--", "archive/nested.md"]);
    git(&repo, &["rm", "--quiet", "--", "docs/subdir/nested.md"]);
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory");
    fs::write(
        repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "knowledge",
            "knowledge:docs:nested",
            "knowledge:archive:nested",
            "docs/subdir/nested.md",
            "archive/nested.md",
            &head,
            &hash,
            "nested domain",
        ),
    )
    .expect("ledger");
    git(&repo, &["add", "--", ".rhizome/relocate-ledger.ndjson"]);
    let spec = registry.sources.get("knowledge").expect("spec").clone();
    assert!(check_staged_frozen_for_specs(&backend, &[spec]).is_ok());
}

#[test]
fn invalid_nested_index_blocks_staged_note_identity() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let nested = docs.join("subdir");
    fs::create_dir_all(&nested).expect("nested directory");
    fs::write(nested.join("INDEX.md"), living_bytes("# Wrong index\n"))
        .expect("wrong nested index");
    let note = nested.join("note.md");
    let bytes = frozen_bytes("# Note\n");
    fs::write(&note, &bytes).expect("note");
    commit_all(&repo, "seed invalid nested index");
    let registry = write_registry(&scratch.path(), &[("knowledge", &repo)]);
    let backend = GitBackend::new(repo.clone()).expect("backend");
    let head = backend.head_oid().expect("head");
    let hash = backend.head_blob_sha256(&note).expect("hash");
    fs::write(archive.join("note.md"), &bytes).expect("destination");
    git(&repo, &["add", "--", "archive/note.md"]);
    git(&repo, &["rm", "--quiet", "--", "docs/subdir/note.md"]);
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory");
    fs::write(
        repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "knowledge",
            "knowledge:docs:note",
            "knowledge:archive:note",
            "docs/subdir/note.md",
            "archive/note.md",
            &head,
            &hash,
            "invalid ancestor",
        ),
    )
    .expect("ledger");
    git(&repo, &["add", "--", ".rhizome/relocate-ledger.ndjson"]);
    let spec = registry.sources.get("knowledge").expect("spec").clone();
    assert!(check_staged_frozen_for_specs(&backend, &[spec]).is_err());
}

#[test]
fn pruned_hidden_path_cannot_be_authorized_by_frozen_gate() {
    let scratch = Scratch::new();
    let repo = scratch.path().join("repo");
    init_repo(&repo);
    let hidden = repo.join(".private/frozen.md");
    fs::create_dir_all(hidden.parent().unwrap()).expect("hidden directory");
    let bytes = frozen_bytes("# Hidden\n");
    fs::write(&hidden, &bytes).expect("hidden note");
    commit_all(&repo, "seed hidden note");
    let registry = write_registry(&scratch.path(), &[("knowledge", &repo)]);
    let backend = GitBackend::new(repo.clone()).expect("backend");
    let head = backend.head_oid().expect("head");
    let hash = backend.head_blob_sha256(&hidden).expect("hash");
    git(&repo, &["rm", "--quiet", "--", ".private/frozen.md"]);
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger directory");
    fs::write(
        repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "knowledge",
            "knowledge:docs:frozen",
            "knowledge:archive:frozen",
            ".private/frozen.md",
            "archive/frozen.md",
            &head,
            &hash,
            "hidden",
        ),
    )
    .expect("ledger");
    git(&repo, &["add", "--", ".rhizome/relocate-ledger.ndjson"]);
    let spec = registry.sources.get("knowledge").expect("spec").clone();
    assert!(check_staged_frozen_for_specs(&backend, &[spec]).is_err());
}
