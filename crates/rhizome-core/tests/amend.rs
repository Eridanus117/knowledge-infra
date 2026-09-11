use kb_contract::{Registry, RegistryLocator, parse_and_validate_note, resolve_registry};
use rhizome_core::SourceContext;
use rhizome_core::amend::{apply_amend, plan_amend};
use rhizome_core::frozen::{ApprovalMarker, check_staged_frozen_for_specs};
use rhizome_core::git::GitBackend;
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
            "rhizome-task7-amend-{millis}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
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

fn git_status(root: &Path, path: &str) -> String {
    String::from_utf8(git(root, &["status", "--porcelain", "--", path]).stdout)
        .expect("git status should be UTF-8")
}

fn git_show(root: &Path, path: &str) -> Vec<u8> {
    git(root, &["show", &format!("HEAD:{path}")]).stdout
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
    format!("---\ndescription: frozen decision\nkeywords: [fixture]\nkind: decision\n---\n{body}")
        .into_bytes()
}

fn write_domain(root: &Path) -> PathBuf {
    let domain = root.join("docs");
    fs::create_dir_all(&domain).expect("domain should be created");
    let index = domain.join("INDEX.md");
    let bytes = index_bytes();
    parse_and_validate_note(&index, &bytes).expect("index fixture must satisfy frontmatter v2");
    fs::write(index, bytes).expect("domain index should be written");
    domain
}

fn registry(root: &Path, repo: &Path) -> Registry {
    let path = root.join("sources.toml");
    let repo = repo.to_string_lossy().replace('\\', "/");
    fs::write(
        &path,
        format!("[[source]]\nname = \"knowledge\"\npath = \"{repo}\"\nsurface = \"core\"\n"),
    )
    .expect("registry should be written");
    resolve_registry(&RegistryLocator {
        explicit: Some(path.clone()),
        cwd: root.to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: root.join("unused.toml"),
    })
    .expect("temporary registry should resolve")
}

fn source_context(registry: &Registry, repo: &Path) -> SourceContext {
    SourceContext {
        source: registry
            .sources
            .get("knowledge")
            .expect("registry source should exist")
            .clone(),
        git_root: repo.to_path_buf(),
        registry_origin: registry.origin.clone(),
    }
}
fn ledger_line(
    old_identity: &str,
    new_identity: &str,
    old_path: &str,
    hash: &str,
    head: &str,
) -> String {
    format!(
        "{{\"schema\":\"frozen-ledger-v2\",\"operation\":\"amend\",\"logical_source\":\"knowledge\",\"old_identity\":\"{old_identity}\",\"new_identity\":\"{new_identity}\",\"old_path\":\"{old_path}\",\"new_path\":\"{old_path}\",\"head_oid\":\"{head}\",\"canonical_git_blob_sha256\":\"{hash}\",\"reason\":\"forged\"}}\n"
    )
}

#[test]
fn one_file_amend_approval_changes_only_named_head_frozen_note() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let first = domain.join("first.md");
    let first_before = frozen_bytes("# First\n\noriginal\n");
    fs::write(&first, &first_before).expect("first note should be written");
    commit_all(&repo, "seed frozen decisions");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# First\n\namended detail\n");
    let marker = ApprovalMarker::for_one_file(first.clone(), "fix typo in first decision");

    let plan = plan_amend(
        &context,
        &first,
        &replacement,
        "fix typo in first decision",
        &marker,
    )
    .expect("one-file amend should produce a plan");
    apply_amend(&plan).expect("approved amend should apply");

    assert_eq!(git_show(&repo, "docs/first.md"), replacement);
    let ledger = repo.join(".rhizome/amend-ledger.ndjson");
    let text = fs::read_to_string(ledger).expect("amend ledger should be created");
    assert_eq!(text.lines().count(), 1);
    let line = text.lines().next().expect("one ledger record should exist");
    assert!(line.starts_with("{\"schema\":\"frozen-ledger-v2\",\"operation\":\"amend\""));
    assert!(line.contains("\"logical_source\":\"knowledge\""));
    assert!(line.contains("\"old_identity\":\"knowledge:docs:first\""));
    assert!(line.contains("\"new_identity\":\"knowledge:docs:first\""));
    assert!(line.contains("\"old_path\":\"docs/first.md\""));
    assert!(line.contains("\"new_path\":\"docs/first.md\""));
    assert!(line.contains("\"canonical_git_blob_sha256\":"));
    assert!(line.contains("\"head_oid\":"));
    assert!(line.contains("\"reason\":\"fix typo in first decision\""));
}

#[test]
fn amend_approval_for_a_different_file_is_rejected() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let first = domain.join("first.md");
    let second = domain.join("second.md");
    fs::write(&first, frozen_bytes("# First\n")).expect("first note should be written");
    fs::write(&second, frozen_bytes("# Second\n")).expect("second note should be written");
    commit_all(&repo, "seed frozen decisions");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# First\n\nforged approval\n");
    let marker = ApprovalMarker::for_one_file(second.clone(), "approve second only");

    assert!(
        plan_amend(&context, &first, &replacement, "forged approval", &marker,).is_err(),
        "an approval marker must name exactly the amended file"
    );
    assert_eq!(
        fs::read(&first).expect("first file should remain"),
        frozen_bytes("# First\n")
    );
    assert!(!repo.join(".rhizome/amend-ledger.ndjson").exists());
}

#[test]
fn amend_accepts_crlf_materialization_of_head_blob() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    fs::write(repo.join(".gitattributes"), b"*.md text eol=lf\n")
        .expect("attributes should be written");
    let domain = write_domain(&repo);
    let note = domain.join("crlf.md");
    let original = frozen_bytes("# CRLF\n\noriginal\n");
    fs::write(&note, &original).expect("frozen note should be written");
    commit_all(&repo, "seed CRLF amend");
    let mut materialized = Vec::new();
    for byte in &original {
        if *byte == b'\n' {
            materialized.extend_from_slice(b"\r\n");
        } else {
            materialized.push(*byte);
        }
    }
    fs::write(&note, &materialized).expect("CRLF materialization should be written");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# CRLF\n\namended\n");
    let marker = ApprovalMarker::for_one_file(note.clone(), "normalize CRLF note");
    let plan = plan_amend(
        &context,
        &note,
        &replacement,
        "normalize CRLF note",
        &marker,
    )
    .expect("CRLF materialization should plan");
    apply_amend(&plan).expect("CRLF materialization should amend");
    assert_eq!(git_show(&repo, "docs/crlf.md"), replacement);
}

#[test]
fn amend_rejects_hook_before_commit_and_rolls_back() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("failed.md");
    let original = frozen_bytes("# Failed\n\noriginal\n");
    fs::write(&note, &original).expect("frozen note should be written");
    commit_all(&repo, "seed failed amend");
    fs::write(repo.join(".git/hooks/pre-commit"), b"#!/bin/sh\nexit 1\n")
        .expect("rejecting hook should be written");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# Failed\n\nreplacement\n");
    let marker = ApprovalMarker::for_one_file(note.clone(), "exercise rollback");
    let plan = plan_amend(&context, &note, &replacement, "exercise rollback", &marker)
        .expect("rollback amend should plan");
    assert!(apply_amend(&plan).is_err());
    assert_eq!(git_show(&repo, "docs/failed.md"), original);
    assert!(!repo.join(".rhizome/amend-ledger.ndjson").exists());
}

#[test]
fn amend_rejects_hardlinked_note_alias() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("hardlink.md");
    fs::write(&note, frozen_bytes("# Hardlink\n")).expect("note should be written");
    commit_all(&repo, "seed hardlink note");
    let alias = scratch.path.join("outside-hardlink.md");
    if fs::hard_link(&note, &alias).is_err() {
        return;
    }
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# Hardlink\n\namended\n");
    let marker = ApprovalMarker::for_one_file(note.clone(), "hardlink rejection");
    assert!(plan_amend(&context, &note, &replacement, "hardlink rejection", &marker).is_err());
}

#[test]
fn amend_stages_literal_wildcard_path_only() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("[wild].md");
    fs::write(&note, frozen_bytes("# Wild\n")).expect("wildcard note should be written");
    commit_all(&repo, "seed wildcard note");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# Wild\n\namended\n");
    let marker = ApprovalMarker::for_one_file(note.clone(), "wildcard path");
    let plan = plan_amend(&context, &note, &replacement, "wildcard path", &marker)
        .expect("wildcard amend should plan");
    apply_amend(&plan).expect("wildcard amend should apply");
    assert_eq!(git_show(&repo, "docs/[wild].md"), replacement);
}

#[test]
fn amend_does_not_run_hook_added_commit_path() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("hook.md");
    let original = frozen_bytes("# Hook\n");
    fs::write(&note, &original).expect("note should be written");
    commit_all(&repo, "seed hook note");
    fs::write(
        repo.join(".git/hooks/pre-commit"),
        b"#!/bin/sh\necho hook > hook-added.txt\ngit add -- hook-added.txt\n",
    )
    .expect("hook should be written");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# Hook\n\namended\n");
    let marker = ApprovalMarker::for_one_file(note.clone(), "hook path check");
    let plan = plan_amend(&context, &note, &replacement, "hook path check", &marker)
        .expect("amend should plan");
    assert!(
        apply_amend(&plan).is_err(),
        "hook-added path must fail post-commit verification"
    );
    assert_eq!(git_show(&repo, "docs/hook.md"), original);
}

#[test]
fn amend_rejects_preexisting_untracked_before_hook_execution() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("untracked.md");
    let original = frozen_bytes("# Untracked\n");
    fs::write(&note, &original).expect("note");
    commit_all(&repo, "seed untracked amend");
    let unrelated = repo.join("preexisting.txt");
    fs::write(&unrelated, b"keep me\n").expect("untracked file");
    fs::write(repo.join(".git/hooks/pre-commit"), b"#!/bin/sh\nexit 1\n").expect("hook");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# Untracked\n\namended\n");
    let marker = ApprovalMarker::for_one_file(note.clone(), "untracked guard");
    assert!(plan_amend(&context, &note, &replacement, "untracked guard", &marker).is_err());
    assert_eq!(fs::read(unrelated).unwrap(), b"keep me\n");
}

#[test]
fn amend_rejects_relative_approval_marker_path() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("relative.md");
    let original = frozen_bytes("# Relative\n");
    fs::write(&note, &original).expect("note");
    commit_all(&repo, "seed relative approval");
    fs::write(&note, frozen_bytes("# Relative\n\nchanged\n")).expect("edit");
    let registry = registry(&scratch.path, &repo);
    let context = source_context(&registry, &repo);
    let replacement = frozen_bytes("# Relative\n\namended\n");
    let marker = ApprovalMarker::for_one_file(PathBuf::from("docs/relative.md"), "relative marker");
    assert!(plan_amend(&context, &note, &replacement, "relative marker", &marker).is_err());
}

#[test]
fn forged_amend_domain_identity_is_rejected_by_staged_gate() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("first.md");
    let original = frozen_bytes("# First\n");
    let replacement = frozen_bytes("# First\n\namended\n");
    fs::write(&note, &original).expect("note");
    commit_all(&repo, "seed amend identity");
    let backend = GitBackend::new(repo.clone()).expect("backend");
    let head = backend.head_oid().expect("head");
    let hash = GitBackend::canonical_blob_sha256(&replacement);
    fs::write(&note, &replacement).expect("replacement");
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger");
    fs::write(
        repo.join(".rhizome/amend-ledger.ndjson"),
        ledger_line(
            "knowledge:other:first",
            "knowledge:other:first",
            "docs/first.md",
            &hash,
            &head,
        ),
    )
    .expect("forged ledger");
    git(
        &repo,
        &["add", "--", "docs/first.md", ".rhizome/amend-ledger.ndjson"],
    );
    let registry = registry(&scratch.path, &repo);
    let spec = registry.sources.get("knowledge").expect("spec").clone();
    assert!(check_staged_frozen_for_specs(&backend, &[spec]).is_err());
}

#[test]
fn invalid_staged_amend_replacement_is_rejected() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let domain = write_domain(&repo);
    let note = domain.join("invalid.md");
    let original = frozen_bytes("# Invalid\n");
    fs::write(&note, &original).expect("note");
    commit_all(&repo, "seed invalid amend");
    let backend = GitBackend::new(repo.clone()).expect("backend");
    let head = backend.head_oid().expect("head");
    let invalid = b"not v2\n";
    fs::write(&note, invalid).expect("invalid replacement");
    fs::create_dir_all(repo.join(".rhizome")).expect("ledger");
    fs::write(
        repo.join(".rhizome/amend-ledger.ndjson"),
        ledger_line(
            "knowledge:docs:invalid",
            "knowledge:docs:invalid",
            "docs/invalid.md",
            &GitBackend::canonical_blob_sha256(invalid),
            &head,
        ),
    )
    .expect("ledger");
    git(
        &repo,
        &[
            "add",
            "--",
            "docs/invalid.md",
            ".rhizome/amend-ledger.ndjson",
        ],
    );
    let registry = registry(&scratch.path, &repo);
    let spec = registry.sources.get("knowledge").expect("spec").clone();
    assert!(check_staged_frozen_for_specs(&backend, &[spec]).is_err());
}
