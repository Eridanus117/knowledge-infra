use kb_contract::{Registry, RegistryLocator, parse_and_validate_note, resolve_registry};
use rhizome_core::git::GitBackend;
use rhizome_core::relocate::{apply_relocate, plan_relocate};
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
            "rhizome-task7-relocate-{millis}-{}-{sequence}",
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

fn index_bytes(label: &str) -> Vec<u8> {
    format!("---\ndescription: {label} domain\nkeywords: [fixture]\nkind: index\n---\n# {label}\n")
        .into_bytes()
}

fn frozen_note() -> Vec<u8> {
    b"---\ndescription: frozen reference\nkeywords: [fixture]\nkind: reference\nstatus: frozen\n---\n# Frozen\n\nbody\n".to_vec()
}

fn living_note() -> Vec<u8> {
    b"---\ndescription: living reference\nkeywords: [fixture]\nkind: reference\n---\n# Living\n\nbody\n".to_vec()
}

fn living_note_with_body(body: &str) -> Vec<u8> {
    format!("---\ndescription: living reference\nkeywords: [fixture]\nkind: reference\n---\n{body}")
        .into_bytes()
}

fn write_domain(root: &Path, name: &str) -> PathBuf {
    let domain = root.join(name);
    fs::create_dir_all(&domain).expect("domain should be created");
    let index = domain.join("INDEX.md");
    let bytes = index_bytes(name);
    parse_and_validate_note(&index, &bytes).expect("domain fixture must satisfy frontmatter v2");
    fs::write(index, bytes).expect("domain index should be written");
    domain
}

fn registry(root: &Path, sources: &[(&str, &Path)]) -> Registry {
    let path = root.join("sources.toml");
    let mut text = String::new();
    for (name, source) in sources {
        let source = source.to_string_lossy().replace('\\', "/");
        text.push_str(&format!(
            "[[source]]\nname = \"{name}\"\npath = \"{source}\"\nsurface = \"core\"\n\n"
        ));
    }
    fs::write(&path, text).expect("registry should be written");
    resolve_registry(&RegistryLocator {
        explicit: Some(path.clone()),
        cwd: root.to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: root.join("unused.toml"),
    })
    .expect("temporary registry should resolve")
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
fn valid_frozen_relocate_preserves_bytes_and_writes_deterministic_ledger() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("frozen.md");
    let before = frozen_note();
    fs::write(&note, &before).expect("frozen note should be written");
    commit_all(&repo, "seed frozen note");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);

    let plan = plan_relocate(&sources, &note, "knowledge:archive")
        .expect("content-preserving frozen relocate should be planned");
    apply_relocate(&plan).expect("valid relocate should apply");

    let destination = archive.join("frozen.md");
    assert!(
        !note.exists(),
        "source path should be removed after relocate"
    );
    assert_eq!(
        fs::read(&destination).expect("destination should exist"),
        before,
        "relocate must preserve exact source bytes"
    );
    let ledger = repo.join(".rhizome/relocate-ledger.ndjson");
    let line = fs::read_to_string(ledger).expect("relocate ledger should be created");
    assert_eq!(line.lines().count(), 1);
    assert!(line.starts_with("{\"schema\":\"frozen-ledger-v2\",\"operation\":\"relocate\""));
    assert!(line.contains("\"logical_source\":\"knowledge\""));
    assert!(line.contains("\"old_identity\":\"knowledge:docs:frozen\""));
    assert!(line.contains("\"new_identity\":\"knowledge:archive:frozen\""));
    assert!(line.contains("\"old_path\":\"docs/frozen.md\""));
    assert!(line.contains("\"new_path\":\"archive/frozen.md\""));
    assert!(line.contains("\"head_oid\":"));
    assert!(line.contains("\"canonical_git_blob_sha256\":"));
    assert!(line.contains("\"reason\":"));
}

#[test]
fn path_collision_is_rejected_without_overwriting_or_ledger_append() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("same.md");
    let collision = archive.join("same.md");
    let source_bytes = living_note();
    let destination_bytes = b"existing destination\n".to_vec();
    fs::write(&note, &source_bytes).expect("source note should be written");
    fs::write(&collision, &destination_bytes).expect("collision file should be written");
    commit_all(&repo, "seed collision");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);

    assert!(
        plan_relocate(&sources, &note, "knowledge:archive").is_err(),
        "relocate must reject an existing destination path"
    );
    assert_eq!(fs::read(&note).expect("source should remain"), source_bytes);
    assert_eq!(
        fs::read(&collision).expect("collision should remain"),
        destination_bytes
    );
    assert!(
        !repo.join(".rhizome/relocate-ledger.ndjson").exists(),
        "a rejected collision must not append provenance"
    );
}

#[test]
fn cross_source_target_is_registered_and_records_both_repository_ledgers() {
    let scratch = Scratch::new();
    let source_repo = scratch.path.join("source-repo");
    let target_repo = scratch.path.join("target-repo");
    init_repo(&source_repo);
    init_repo(&target_repo);
    let source_domain = write_domain(&source_repo, "docs");
    let target_domain = write_domain(&target_repo, "archive");
    let note = source_domain.join("cross.md");
    let before = frozen_note();
    fs::write(&note, &before).expect("source note should be written");
    commit_all(&source_repo, "seed source");
    commit_all(&target_repo, "seed target");
    let sources = registry(
        &scratch.path,
        &[("knowledge", &source_repo), ("shared", &target_repo)],
    );

    let plan = plan_relocate(&sources, &note, "shared:archive")
        .expect("registered cross-source target should be accepted");
    apply_relocate(&plan).expect("cross-source relocate should apply");

    let destination = target_domain.join("cross.md");
    assert!(!note.exists());
    assert_eq!(
        fs::read(destination).expect("cross-source destination should exist"),
        before
    );
    assert!(
        source_repo
            .join(".rhizome/relocate-ledger.ndjson")
            .is_file(),
        "source repository must retain provenance"
    );
    assert!(
        target_repo
            .join(".rhizome/relocate-ledger.ndjson")
            .is_file(),
        "target repository must retain provenance"
    );
    let source_line = fs::read_to_string(source_repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("source ledger should be readable");
    let target_line = fs::read_to_string(target_repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("target ledger should be readable");
    assert!(source_line.contains("\"old_identity\":\"knowledge:docs:cross\""));
    assert!(source_line.contains("\"new_identity\":\"shared:archive:cross\""));
    assert!(target_line.contains("\"new_identity\":\"shared:archive:cross\""));
}

#[test]
fn historical_target_ledger_is_ignored_when_current_pair_matches() {
    let scratch = Scratch::new();
    let source_repo = scratch.path.join("source-repo");
    let target_repo = scratch.path.join("target-repo");
    init_repo(&source_repo);
    init_repo(&target_repo);
    let source_domain = write_domain(&source_repo, "docs");
    let target_domain = write_domain(&target_repo, "archive");
    let note = source_domain.join("cross.md");
    let before = frozen_note();
    fs::write(&note, &before).expect("source note should be written");
    commit_all(&source_repo, "seed source");
    commit_all(&target_repo, "seed target");
    let target_backend = GitBackend::new(target_repo.clone()).expect("target backend");
    let target_head = target_backend.head_oid().expect("target head");
    fs::create_dir_all(target_repo.join(".rhizome")).expect("target ledger directory");
    fs::write(
        target_repo.join(".rhizome/relocate-ledger.ndjson"),
        ledger_line(
            "relocate",
            "shared",
            "forged:docs:cross",
            "shared:archive:cross",
            "docs/cross.md",
            "archive/cross.md",
            &target_head,
            &"00".repeat(32),
            "historical provenance",
        ),
    )
    .expect("historical target ledger");
    commit_all(&target_repo, "seed historical target ledger");
    let sources = registry(
        &scratch.path,
        &[("knowledge", &source_repo), ("shared", &target_repo)],
    );
    let plan =
        plan_relocate(&sources, &note, "shared:archive").expect("cross-repo plan should be valid");
    apply_relocate(&plan).expect("current pair should apply despite historical row");
    assert!(!note.exists());
    assert_eq!(fs::read(target_domain.join("cross.md")).unwrap(), before);
}

#[test]
fn unregistered_cross_source_target_is_rejected_before_touching_source() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let note = docs.join("cross.md");
    fs::write(&note, living_note()).expect("source note should be written");
    commit_all(&repo, "seed source");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);

    assert!(plan_relocate(&sources, &note, "missing:archive").is_err());
    assert!(
        note.exists(),
        "invalid cross-source target must not remove source"
    );
}

#[test]
fn living_modified_relocate_records_moved_bytes_not_head_bytes() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("living.md");
    fs::write(&note, living_note()).expect("living note should be written");
    commit_all(&repo, "seed living note");
    let modified = living_note_with_body("# Living\n\nmodified\n");
    fs::write(&note, &modified).expect("modified note should be written");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);

    let plan = plan_relocate(&sources, &note, "knowledge:archive")
        .expect("modified living note should plan");
    apply_relocate(&plan).expect("modified living relocate should apply");
    let line = fs::read_to_string(repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("relocate ledger should exist");
    let expected = GitBackend::canonical_blob_sha256(&modified);
    assert!(line.contains(&format!("\"canonical_git_blob_sha256\":\"{expected}\"")));
    assert_eq!(fs::read(archive.join("living.md")).unwrap(), modified);
}

#[test]
fn nested_registered_source_uses_most_specific_root() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    let outer_root = repo.join("vault");
    let inner_root = outer_root.join("team");
    let docs = write_domain(&inner_root, "docs");
    let archive = write_domain(&outer_root, "archive");
    let note = docs.join("nested.md");
    fs::write(&note, living_note()).expect("nested note should be written");
    commit_all(&repo, "seed nested sources");
    let sources = registry(
        &scratch.path,
        &[("outer", &outer_root), ("inner", &inner_root)],
    );
    let plan = plan_relocate(&sources, &note, "outer:archive")
        .expect("most-specific source should own note");
    apply_relocate(&plan).expect("nested source relocate should apply");
    let ledger = fs::read_to_string(repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("ledger should exist");
    assert!(ledger.contains("\"old_identity\":\"inner:docs:nested\""));
    assert!(archive.join("nested.md").is_file());
}

#[test]
fn living_crlf_relocate_preserves_crlf_when_conversion_disabled() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    fs::write(repo.join(".gitattributes"), b"*.md -crlf\n").expect("attributes should be written");
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("crlf.md");
    let lf = living_note_with_body("# CRLF\n\nbody\n");
    fs::write(&note, &lf).expect("note should be written");
    commit_all(&repo, "seed CRLF living note");
    let mut crlf = Vec::new();
    for byte in &lf {
        if *byte == b'\n' {
            crlf.extend_from_slice(b"\r\n");
        } else {
            crlf.push(*byte);
        }
    }
    fs::write(&note, &crlf).expect("CRLF note should be written");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);
    let plan =
        plan_relocate(&sources, &note, "knowledge:archive").expect("CRLF relocate should plan");
    apply_relocate(&plan).expect("CRLF relocate should apply");
    let line = fs::read_to_string(repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("ledger should exist");
    let expected = GitBackend::canonical_blob_sha256(&crlf);
    assert!(line.contains(&format!("\"canonical_git_blob_sha256\":\"{expected}\"")));
    assert_eq!(fs::read(archive.join("crlf.md")).unwrap(), crlf);
}

#[test]
fn living_crlf_relocate_uses_default_text_auto_lf_blob_hash() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    fs::write(repo.join(".gitattributes"), b"* text=auto eol=lf\n")
        .expect("attributes should be written");
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("crlf.md");
    let lf = living_note_with_body("# CRLF default\n\nbody\n");
    fs::write(&note, &lf).expect("note should be written");
    commit_all(&repo, "seed default CRLF living note");
    let mut crlf = Vec::new();
    for byte in &lf {
        if *byte == b'\n' {
            crlf.extend_from_slice(b"\r\n");
        } else {
            crlf.push(*byte);
        }
    }
    fs::write(&note, &crlf).expect("CRLF note should be written");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);
    let plan =
        plan_relocate(&sources, &note, "knowledge:archive").expect("CRLF relocate should plan");
    apply_relocate(&plan).expect("CRLF relocate should apply");
    let line = fs::read_to_string(repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("ledger should exist");
    let expected = GitBackend::canonical_blob_sha256(&lf);
    assert!(line.contains(&format!("\"canonical_git_blob_sha256\":\"{expected}\"")));
    assert_eq!(fs::read(archive.join("crlf.md")).unwrap(), crlf);
}

#[test]
fn later_gitattributes_disable_overrides_default_lf_conversion() {
    let scratch = Scratch::new();
    let repo = scratch.path.join("repo");
    init_repo(&repo);
    fs::write(
        repo.join(".gitattributes"),
        b"* text=auto eol=lf\n*.md -text\n",
    )
    .expect("attributes should be written");
    let docs = write_domain(&repo, "docs");
    let archive = write_domain(&repo, "archive");
    let note = docs.join("precedence.md");
    let lf = living_note_with_body("# Precedence\n\nbody\n");
    fs::write(&note, &lf).expect("note should be written");
    commit_all(&repo, "seed precedence note");
    let mut crlf = Vec::new();
    for byte in &lf {
        if *byte == b'\n' {
            crlf.extend_from_slice(b"\r\n");
        } else {
            crlf.push(*byte);
        }
    }
    fs::write(&note, &crlf).expect("CRLF note should be written");
    let sources = registry(&scratch.path, &[("knowledge", &repo)]);
    let plan = plan_relocate(&sources, &note, "knowledge:archive")
        .expect("precedence relocate should plan");
    apply_relocate(&plan).expect("precedence relocate should apply");
    let line = fs::read_to_string(repo.join(".rhizome/relocate-ledger.ndjson"))
        .expect("ledger should exist");
    let expected = GitBackend::canonical_blob_sha256(&crlf);
    assert!(line.contains(&format!("\"canonical_git_blob_sha256\":\"{expected}\"")));
    assert_eq!(fs::read(archive.join("precedence.md")).unwrap(), crlf);
}
