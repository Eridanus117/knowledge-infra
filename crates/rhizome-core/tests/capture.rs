use kb_contract::{RegistryLocator, resolve_registry};
use rhizome_core::capture::{CaptureRequest, apply_capture, plan_capture};
use rhizome_core::{SourceContext, discover_source};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
fn init_git(path: &Path) {
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .output()
        .expect("git should be installed for capture fixtures");
    assert!(output.status.success(), "git init should succeed");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let n = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rhizome-task8-capture-{}-{n}", std::process::id()));
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

fn source_context(scratch: &Scratch) -> SourceContext {
    let repo = scratch.path().join("repo");
    fs::create_dir_all(repo.join("docs")).expect("domain should be created");
    init_git(&repo);
    fs::write(
        repo.join("docs/INDEX.md"),
        b"---\ndescription: docs\nkeywords: [fixture]\nkind: index\n---\n# Docs\n",
    )
    .expect("domain index should be written");
    let registry_path = scratch.path().join("sources.toml");
    let repo_text = repo.to_string_lossy().replace('\\', "/");
    fs::write(
        &registry_path,
        format!("[[source]]\nname = \"knowledge\"\npath = \"{repo_text}\"\nsurface = \"core\"\n"),
    )
    .expect("registry should be written");
    let registry = resolve_registry(&RegistryLocator {
        explicit: Some(registry_path),
        cwd: scratch.path().to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: scratch.path().join("unused.toml"),
    })
    .expect("registry fixture should resolve");
    SourceContext {
        source: registry
            .sources
            .get("knowledge")
            .expect("source should exist")
            .clone(),
        git_root: repo,
        registry_origin: registry.origin,
    }
}

#[test]
fn capture_plan_appends_one_collapsed_line_outside_all_source_domains() {
    let scratch = Scratch::new();
    let context = source_context(&scratch);
    let inbox = context.git_root.join("inbox").join("capture.md");
    let request = CaptureRequest {
        inbox: inbox.clone(),
        text: "  remember   this\nfor later  ".into(),
        timestamp: "2026-09-11T12:34:56+00:00".into(),
    };

    let plan = plan_capture(&request).expect("capture should produce a typed plan");
    apply_capture(&plan).expect("capture plan should apply");
    let text = fs::read_to_string(&inbox).expect("capture inbox should be created");
    assert_eq!(
        text,
        "- 2026-09-11T12:34:56+00:00 remember this for later\n"
    );

    let snapshot = discover_source(&context).expect("source discovery should still succeed");
    assert!(snapshot.notes.iter().all(|note| note.locator.path != inbox));
    assert!(
        !snapshot
            .notes
            .iter()
            .any(|note| note.locator.slug == "capture")
    );
}

#[test]
fn capture_rejects_empty_thought_without_creating_an_inbox() {
    let scratch = Scratch::new();
    let inbox = scratch.path().join("inbox.md");
    let request = CaptureRequest {
        inbox: inbox.clone(),
        text: " \n\t ".into(),
        timestamp: "2026-09-11T12:34:56+00:00".into(),
    };

    assert!(
        plan_capture(&request).is_err(),
        "empty capture must fail closed"
    );
    assert!(!inbox.exists(), "failed capture must not create an inbox");
}

#[test]
fn capture_preserves_existing_inbox_bytes_when_appending() {
    let scratch = Scratch::new();
    let inbox = scratch.path().join("inbox.md");
    fs::write(&inbox, b"- earlier capture\n").expect("existing inbox should be written");
    let request = CaptureRequest {
        inbox: inbox.clone(),
        text: "later capture".into(),
        timestamp: "2026-09-11T12:34:56+00:00".into(),
    };
    let plan = plan_capture(&request).expect("capture should produce a typed plan");
    apply_capture(&plan).expect("capture plan should append");
    assert_eq!(
        fs::read(&inbox).expect("inbox should remain readable"),
        b"- earlier capture\n- 2026-09-11T12:34:56+00:00 later capture\n"
    );
}
#[test]
fn capture_inserts_separator_for_existing_inbox_without_final_newline() {
    let scratch = Scratch::new();
    let inbox = scratch.path().join("inbox.md");
    fs::write(&inbox, b"- earlier capture").expect("existing inbox should be written");
    let request = CaptureRequest {
        inbox: inbox.clone(),
        text: "later capture".into(),
        timestamp: "2026-09-11T12:34:56+00:00".into(),
    };
    let plan = plan_capture(&request).expect("capture should produce a typed plan");
    apply_capture(&plan).expect("capture plan should append");
    assert_eq!(
        fs::read(&inbox).expect("inbox should remain readable"),
        b"- earlier capture\n- 2026-09-11T12:34:56+00:00 later capture\n"
    );
}
