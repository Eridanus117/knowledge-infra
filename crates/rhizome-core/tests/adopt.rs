use rhizome_core::adopt::{AdoptRequest, apply_adopt, plan_adopt};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
fn init_git(path: &Path) {
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .output()
        .expect("git should be installed for adoption fixtures");
    assert!(output.status.success(), "git init should succeed");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let n = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rhizome-task8-adopt-{}-{n}", std::process::id()));
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

fn registry(path: &Path) -> PathBuf {
    let registry = path.join("sources.toml");
    fs::write(
        &registry,
        "workspace_root = \".\"\n\n[[source]]\nname = \"seed\"\npath = \"seed\"\nsurface = \"core\"\n",
    )
    .expect("registry should be written");
    fs::create_dir_all(path.join("seed")).expect("seed source should exist");
    registry
}

#[test]
fn adopt_plan_adds_logical_source_index_and_gate_without_running_tools() {
    let scratch = Scratch::new();
    let registry_path = registry(scratch.path());
    let repo = scratch.path().join("new-repo");
    init_git(&repo);

    let request = AdoptRequest {
        registry: registry_path.clone(),
        logical_source: "knowledge".into(),
        repo: repo.clone(),
        description: "Knowledge source domain".into(),
        keywords: vec!["knowledge".into(), "docs".into()],
    };
    let plan = plan_adopt(&request).expect("adoption should produce a typed plan");
    apply_adopt(&plan).expect("adoption plan should apply");

    let registry_text =
        fs::read_to_string(&registry_path).expect("registry should remain readable");
    assert!(registry_text.contains("name = \"knowledge\""));
    let index = repo.join("docs/INDEX.md");
    assert!(
        index.is_file(),
        "adoption should create the initial C2 domain"
    );
    let index_text = fs::read_to_string(index).expect("domain index should be readable");
    assert!(index_text.contains("kind: index"));
    assert!(index_text.contains("description: \"Knowledge source domain\""));
    let hook = fs::read_to_string(repo.join("lefthook.yml")).expect("gate should be written");
    assert!(hook.contains("rhizome check"));
}

#[test]
fn adopt_is_idempotent_and_does_not_rewrite_existing_domain_or_registry_bytes() {
    let scratch = Scratch::new();
    let registry_path = registry(scratch.path());
    let repo = scratch.path().join("new-repo");
    init_git(&repo);
    fs::create_dir_all(repo.join("docs")).expect("existing domain should be created");
    fs::write(
        repo.join("docs/INDEX.md"),
        b"---\ndescription: hand-authored\nkeywords: [keep]\nkind: index\n---\n# Keep\n",
    )
    .expect("existing index should be written");
    let hook_before =
        b"pre-commit:\n  commands:\n    existing:\n      run: rhizome check -- {staged_files}\n";
    fs::write(repo.join("lefthook.yml"), hook_before).expect("existing gate should be written");

    let request = AdoptRequest {
        registry: registry_path.clone(),
        logical_source: "knowledge".into(),
        repo: repo.clone(),
        description: "must not replace existing metadata".into(),
        keywords: vec!["new".into()],
    };
    let plan = plan_adopt(&request).expect("adoption should produce a typed plan");
    apply_adopt(&plan).expect("first adoption should apply");
    let registry_after_first = fs::read(&registry_path).expect("registry should be readable");
    let index_after_first = fs::read(repo.join("docs/INDEX.md")).expect("index should be readable");
    let hook_after_first = fs::read(repo.join("lefthook.yml")).expect("gate should be readable");

    let second = plan_adopt(&request).expect("repeat adoption should be planable");
    apply_adopt(&second).expect("repeat adoption should apply idempotently");
    assert_eq!(
        fs::read(registry_path).expect("registry should remain readable"),
        registry_after_first
    );
    assert_eq!(
        fs::read(repo.join("docs/INDEX.md")).expect("index should remain readable"),
        index_after_first
    );
    assert_eq!(
        fs::read(repo.join("lefthook.yml")).expect("gate should remain readable"),
        hook_after_first
    );
}
