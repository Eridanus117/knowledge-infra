use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

fn init_git(path: &Path) {
    let output = Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .output()
        .expect("git should be installed for CLI fixtures");
    assert!(output.status.success(), "git init should succeed");
}

fn rhizome() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rhizome"))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let n = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rhizome-task8-cli-{}-{n}", std::process::id()));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source_fixture(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let repo = scratch.path().join("repo");
    fs::create_dir_all(repo.join("docs")).expect("domain should be created");
    fs::write(
        repo.join("docs/INDEX.md"),
        b"---\ndescription: docs\nkeywords: [fixture]\nkind: index\n---\n# Docs\n",
    )
    .expect("domain index should be written");
    fs::write(
        repo.join("INDEX.md"),
        b"<!-- rhizome:generated-index:start -->\n<!-- rhizome:generated-index:end -->",
    )
    .expect("human index should be written");
    init_git(&repo);
    let registry = scratch.path().join("sources.toml");
    let repo_text = repo.to_string_lossy().replace('\\', "/");
    fs::write(
        &registry,
        format!("[[source]]\nname = \"knowledge\"\npath = \"{repo_text}\"\nsurface = \"core\"\n"),
    )
    .expect("registry should be written");
    (repo, registry)
}

fn run(args: &[&str], cwd: &Path, registry: Option<&Path>, stdin: Option<&[u8]>) -> Output {
    let mut command = rhizome();
    command.args(args).current_dir(cwd);
    if let Some(registry) = registry {
        command.env("KB_SOURCES", registry);
    } else {
        command.env_remove("KB_SOURCES");
    }
    if let Some(stdin) = stdin {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().expect("rhizome should spawn");
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin should be piped")
            .write_all(stdin)
            .expect("stdin should write");
        return child.wait_with_output().expect("rhizome should finish");
    }
    command.output().expect("rhizome should run")
}

fn run_capture(args: &[&str], cwd: &Path, registry: &Path, inbox: &Path) -> Output {
    let mut command = rhizome();
    command
        .args(args)
        .current_dir(cwd)
        .env("KB_SOURCES", registry)
        .env("RHIZOME_INBOX", inbox);
    command.output().expect("rhizome should run")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout must contain exactly one JSON envelope: {error}; status={:?}; stderr={:?}; got {:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    })
}

#[test]
fn version_is_consumer_visible() {
    let output = rhizome()
        .arg("--version")
        .output()
        .expect("rhizome binary should run");

    assert!(output.status.success(), "--version should succeed");
    assert_eq!(
        String::from_utf8(output.stdout).expect("version output should be UTF-8"),
        concat!("rhizome ", env!("CARGO_PKG_VERSION"), "\n")
    );
    assert!(output.stderr.is_empty(), "--version should not use stderr");
}

#[test]
fn help_describes_the_source_plane_boundary() {
    let output = rhizome()
        .arg("--help")
        .output()
        .expect("rhizome binary should run");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8(output.stdout).expect("help output should be UTF-8");
    assert!(stdout.contains("Operate on Git-backed Markdown knowledge sources"));
    assert!(stdout.contains("Usage: rhizome"));
    assert!(output.stderr.is_empty(), "--help should not use stderr");
}

#[test]
fn help_lists_only_the_v2_command_surface_and_is_deterministic() {
    let first = rhizome().arg("--help").output().expect("help should run");
    let second = rhizome().arg("--help").output().expect("help should run");
    assert_eq!(
        first.stdout, second.stdout,
        "human help must be deterministic"
    );
    let text = String::from_utf8(first.stdout).expect("help should be UTF-8");
    for command in [
        "new", "check", "domains", "adopt", "doctor", "amend", "relocate", "capture", "stats",
    ] {
        assert!(text.contains(command), "help should list {command}");
    }
    assert!(text.contains("index"), "help should list index commands");
}

#[test]
fn every_v2_command_and_nested_index_command_is_recognized() {
    for args in [
        vec!["new", "--help"],
        vec!["check", "--help"],
        vec!["domains", "--help"],
        vec!["adopt", "--help"],
        vec!["doctor", "--help"],
        vec!["amend", "--help"],
        vec!["relocate", "--help"],
        vec!["capture", "--help"],
        vec!["stats", "--help"],
        vec!["index", "check", "--help"],
        vec!["index", "sync", "--help"],
    ] {
        let output = rhizome().args(&args).output().expect("command should run");
        assert!(
            output.status.success(),
            "command {:?} should be recognized: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn check_json_uses_v2_envelope_without_stdout_log_contamination() {
    let scratch = Scratch::new();
    let (repo, registry) = source_fixture(&scratch);
    let path = repo.join("docs/INDEX.md");
    let output = run(
        &["check", "--json", path.to_str().expect("UTF-8 path")],
        scratch.path(),
        Some(&registry),
        None,
    );
    assert_eq!(output.status.code(), Some(0), "valid check should succeed");
    let value = stdout_json(&output);
    assert_eq!(value["schema"], "rhizome-cli-v2");
    assert_eq!(value["ok"], true);
    assert!(value.get("data").is_some());
    assert!(value["diagnostics"].is_array());
}

#[test]
fn check_contract_failure_is_exit_one_with_diagnostic() {
    let scratch = Scratch::new();
    let path = scratch.path().join("invalid.md");
    fs::write(
        &path,
        b"---\ndescription: invalid\nkeywords: [fixture]\nstatus: draft\n---\nbody\n",
    )
    .expect("invalid fixture should be written");
    let output = run(
        &["check", "--json", path.to_str().expect("UTF-8 path")],
        scratch.path(),
        None,
        None,
    );
    assert_eq!(output.status.code(), Some(1), "contract errors use exit 1");
    let value = stdout_json(&output);
    assert_eq!(value["schema"], "rhizome-cli-v2");
    assert_eq!(value["ok"], false);
    assert!(
        value["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .any(|item| item["code"] == "KBV2-NOTE-INVALID-STATUS")
    );
}

#[test]
fn new_requires_logical_source_domain_and_unicode_safe_slug_and_never_overwrites() {
    let scratch = Scratch::new();
    let (repo, registry) = source_fixture(&scratch);
    let args = [
        "new",
        "--json",
        "--source",
        "knowledge",
        "--domain",
        "docs",
        "cafe\u{301}",
        "--description",
        "Authored",
        "--keywords",
        "fixture",
        "--body-file",
        "-",
    ];
    let output = run(&args, &repo, Some(&registry), Some(b"# Authored\n"));
    assert_eq!(output.status.code(), Some(0), "new should succeed");
    let value = stdout_json(&output);
    assert_eq!(value["schema"], "rhizome-cli-v2");
    assert_eq!(value["ok"], true);
    let target = repo.join("docs/caf\u{e9}.md");
    let before = fs::read(&target).expect("new should write NFC slug path");

    let second = run(&args, &repo, Some(&registry), Some(b"replacement\n"));
    assert_eq!(second.status.code(), Some(1), "new must reject overwrite");
    assert_eq!(
        fs::read(target).expect("existing note should remain"),
        before
    );
}

#[test]
fn capture_is_raw_and_outside_source_domains() {
    let scratch = Scratch::new();
    let (repo, registry) = source_fixture(&scratch);
    let inbox = scratch.path().join("inbox.md");
    let output = run_capture(
        &["capture", "--json", "remember", "this"],
        &repo,
        &registry,
        &inbox,
    );
    assert_eq!(output.status.code(), Some(0), "capture should succeed");
    let value = stdout_json(&output);
    assert_eq!(value["schema"], "rhizome-cli-v2");
    assert_eq!(value["ok"], true);
    assert!(value["data"]["path"].as_str().is_some());
    assert_eq!(
        fs::read_to_string(&inbox)
            .expect("configured inbox should be written")
            .lines()
            .count(),
        1,
    );
    assert!(
        !repo.join("capture.md").exists(),
        "capture must not write a source-domain note"
    );
}

#[test]
fn mermaid_adapter_failure_is_reported_as_check_diagnostic() {
    let scratch = Scratch::new();
    let path = scratch.path().join("mermaid.md");
    fs::write(&path, b"---\ndescription: diagram\nkeywords: [fixture]\nkind: note\n---\n```mermaid\nnot valid mermaid\n```\n").expect("Mermaid fixture should be written");
    let output = run(
        &["check", "--json", path.to_str().expect("UTF-8 path")],
        scratch.path(),
        None,
        None,
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "integrity findings use exit 3"
    );
    let value = stdout_json(&output);
    assert_eq!(value["schema"], "rhizome-cli-v2");
    assert!(
        value["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .any(|item| item["code"]
                .as_str()
                .unwrap_or_default()
                .contains("MERMAID"))
    );
}

#[test]
fn unsafe_index_sync_requires_force_and_uses_exit_two() {
    let scratch = Scratch::new();
    let (_, registry) = source_fixture(&scratch);
    let output = run(
        &["index", "sync", "--json"],
        scratch.path(),
        Some(&registry),
        None,
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "unsafe sync requires explicit force"
    );
    let value = stdout_json(&output);
    assert_eq!(value["schema"], "rhizome-cli-v2");
    assert!(!value["ok"].as_bool().expect("ok bool"));
}

#[test]
fn removed_diff_and_old_kb_alias_are_usage_exit_sixty_four() {
    let diff = run(&["domains", "--diff"], Path::new("."), None, None);
    assert_eq!(diff.status.code(), Some(64), "domains --diff is removed");
    let old = run(&["kb", "check"], Path::new("."), None, None);
    assert_eq!(old.status.code(), Some(64), "old kb alias is removed");
}
