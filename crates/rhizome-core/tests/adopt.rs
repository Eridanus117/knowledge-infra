use kb_contract::Diagnostic;
use rhizome_core::adopt::{AdoptRequest, apply_adopt, apply_adopt_with_hook, plan_adopt};
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
    let registry_text = registry_path.to_string_lossy().replace('\\', "/");
    let hook_before = format!(
        "pre-commit:\n  commands:\n    existing:\n      run: 'rhizome check --registry \"{registry_text}\" -- {{staged_files}}'\n"
    );
    fs::write(repo.join("lefthook.yml"), hook_before.as_bytes())
        .expect("existing gate should be written");

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

#[test]
fn adoption_uses_discovered_domains_and_does_not_require_a_docs_directory() {
    let scratch = Scratch::new();
    let registry_path = registry(scratch.path());
    let repo = scratch.path().join("existing-repo");
    init_git(&repo);
    fs::create_dir_all(repo.join("topic")).expect("domain directory should be created");
    fs::write(
        repo.join("topic/INDEX.md"),
        b"---\ndescription: topic\nkeywords: [topic]\nkind: index\n---\n",
    )
    .expect("domain index should be written");
    fs::write(repo.join("docs"), b"unrelated file").expect("docs file should be written");

    let request = AdoptRequest {
        registry: registry_path,
        logical_source: "knowledge".into(),
        repo: repo.clone(),
        description: "existing source".into(),
        keywords: vec!["source".into()],
    };
    let plan = plan_adopt(&request).expect("existing C2 domain should be sufficient");
    assert!(
        !plan.creates_index(),
        "existing domains must not create docs skeleton"
    );
    apply_adopt(&plan).expect("adoption should apply");
    assert!(!repo.join("docs/INDEX.md").exists());
}

#[test]
fn adoption_rejects_another_logical_source_in_the_same_git_repository() {
    let scratch = Scratch::new();
    let registry_path = scratch.path().join("sources.toml");
    let repo = scratch.path().join("existing-repo");
    init_git(&repo);
    fs::create_dir_all(repo.join("topic")).expect("domain directory should be created");
    fs::write(
        repo.join("topic/INDEX.md"),
        b"---\ndescription: topic\nkeywords: [topic]\nkind: index\n---\n",
    )
    .expect("domain index should be written");
    fs::write(
        &registry_path,
        format!(
            "[[source]]\nname = \"seed\"\npath = \"{}\"\nsurface = \"core\"\n",
            repo.join("topic").to_string_lossy().replace('\\', "/")
        ),
    )
    .expect("registry should be written");

    let request = AdoptRequest {
        registry: registry_path,
        logical_source: "knowledge".into(),
        repo,
        description: "conflict".into(),
        keywords: vec!["source".into()],
    };
    let error = plan_adopt(&request).expect_err("same Git repository must conflict");
    assert!(
        error
            .into_diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "KBV2-ADOPT-SOURCE-CONFLICT")
    );
}

#[test]
fn adoption_does_not_accept_a_skipped_gate_as_active() {
    let scratch = Scratch::new();
    let registry_path = registry(scratch.path());
    let repo = scratch.path().join("existing-repo");
    init_git(&repo);
    fs::create_dir_all(repo.join("topic")).expect("domain directory should be created");
    fs::write(
        repo.join("topic/INDEX.md"),
        b"---\ndescription: topic\nkeywords: [topic]\nkind: index\n---\n",
    )
    .expect("domain index should be written");
    let registry_text = registry_path.to_string_lossy().replace('\\', "/");
    fs::write(
        repo.join("lefthook.yml"),
        format!(
            "pre-commit:\n  commands:\n    rhizome-check:\n      skip: true\n      run: 'rhizome check --registry \"{registry_text}\" -- {{staged_files}}'\n"
        ),
    )
    .expect("gate should be written");

    let request = AdoptRequest {
        registry: registry_path,
        logical_source: "knowledge".into(),
        repo,
        description: "skipped".into(),
        keywords: vec!["source".into()],
    };
    let error = plan_adopt(&request).expect_err("skipped gate must fail closed");
    assert!(
        error
            .into_diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "KBV2-ADOPT-GATE")
    );
}

#[test]
fn adoption_rolls_back_registry_files_and_new_directories_on_gate_failure() {
    let scratch = Scratch::new();
    let registry_path = registry(scratch.path());
    let registry_before = fs::read(&registry_path).expect("registry should be readable");
    let repo = scratch.path().join("new-repo");
    init_git(&repo);

    let request = AdoptRequest {
        registry: registry_path.clone(),
        logical_source: "knowledge".into(),
        repo: repo.clone(),
        description: "rollback".into(),
        keywords: vec!["source".into()],
    };
    let plan = plan_adopt(&request).expect("adoption should produce a plan");
    fs::create_dir(repo.join("lefthook.yml")).expect("gate collision should be created");
    let error = apply_adopt(&plan).expect_err("gate collision should fail closed");
    assert!(!error.into_diagnostics().is_empty());
    assert_eq!(
        fs::read(&registry_path).expect("registry should remain readable"),
        registry_before
    );
    assert!(!repo.join("INDEX.md").exists());
    assert!(!repo.join("docs/INDEX.md").exists());
    assert!(repo.join("lefthook.yml").is_dir());
}

#[test]
fn adoption_hook_failure_rolls_back_all_new_source_files() {
    let scratch = Scratch::new();
    let registry_path = registry(scratch.path());
    let registry_before = fs::read(&registry_path).expect("registry should be readable");
    let repo = scratch.path().join("new-repo");
    init_git(&repo);

    let request = AdoptRequest {
        registry: registry_path.clone(),
        logical_source: "knowledge".into(),
        repo: repo.clone(),
        description: "hook failure".into(),
        keywords: vec!["source".into()],
    };
    let plan = plan_adopt(&request).expect("adoption should produce a plan");
    let error = apply_adopt_with_hook(&plan, |_| {
        Err(rhizome_core::adopt::AdoptError::Diagnostics(vec![
            Diagnostic::error("KBV2-TEST-HOOK", "test hook failed"),
        ]))
    })
    .expect_err("hook failure should fail adoption");
    assert!(
        error
            .into_diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "KBV2-TEST-HOOK")
    );
    assert_eq!(
        fs::read(&registry_path).expect("registry should remain readable"),
        registry_before
    );
    assert!(!repo.join("INDEX.md").exists());
    assert!(!repo.join("docs/INDEX.md").exists());
    assert!(!repo.join("lefthook.yml").exists());
}
