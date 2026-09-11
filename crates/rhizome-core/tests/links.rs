use kb_contract::{RegistryLocator, Severity, resolve_registry};
use rhizome_core::{SourceContext, check_source};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("rhizome-core-links-{timestamp}-{sequence}"));
        fs::create_dir(&path).expect("scratch directory should be created");
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

fn write_note(path: &Path, description: &str, links: &[&str], frozen: bool) {
    fs::create_dir_all(path.parent().expect("note should have a parent"))
        .expect("note parent should be created");
    let mut text =
        format!("---\ndescription: \"{description}\"\nkeywords: [fixture]\nkind: note\n");
    if !links.is_empty() {
        text.push_str("links: [");
        for (index, link) in links.iter().enumerate() {
            if index != 0 {
                text.push_str(", ");
            }
            text.push('"');
            text.push_str(link);
            text.push('"');
        }
        text.push_str("]\n");
    }
    if frozen {
        text.push_str("status: frozen\n");
    }
    text.push_str("---\n# Note\n");
    fs::write(path, text).expect("note should be written");
}

fn write_index(path: &Path) {
    fs::create_dir_all(path.parent().expect("index should have a parent"))
        .expect("index parent should be created");
    fs::write(
        path,
        "---\ndescription: \"Topic\"\nkeywords: [fixture]\nkind: index\n---\n# Topic\n",
    )
    .expect("domain index should be written");
}

fn source_context(source_root: &Path, git_root: &Path) -> SourceContext {
    let registry_origin = git_root.join("links-registry.toml");
    fs::write(
        &registry_origin,
        "[[source]]\nname = \"knowledge\"\npath = \"vault\"\nsurface = \"core\"\n",
    )
    .expect("registry should be written");
    let locator = RegistryLocator {
        explicit: Some(registry_origin.clone()),
        cwd: git_root.to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: git_root.join("missing-user-registry.toml"),
    };
    let registry = match resolve_registry(&locator) {
        Ok(registry) => registry,
        Err(_) => panic!("test registry should resolve"),
    };
    let source = registry
        .sources
        .into_values()
        .next()
        .expect("test registry should contain knowledge");
    assert_eq!(source.root, fs::canonicalize(source_root).unwrap());
    SourceContext {
        source,
        git_root: git_root.to_path_buf(),
        registry_origin,
    }
}

fn source_fixture() -> (Scratch, SourceContext) {
    let scratch = Scratch::new();
    let git_root = scratch.path().join("desk");
    let source_root = git_root.join("vault");
    fs::create_dir_all(git_root.join(".git")).expect("git marker should be created");
    write_index(&source_root.join("topic/INDEX.md"));
    fs::write(
        source_root.join("INDEX.md"),
        "<!-- rhizome:generated-index:start -->\n<!-- rhizome:generated-index:end -->\n",
    )
    .expect("root human index should be written");
    write_note(&source_root.join("topic/target.md"), "Target", &[], false);
    (scratch, source_context(&source_root, &git_root))
}

fn report(context: &SourceContext) -> rhizome_core::CheckReport {
    match check_source(context) {
        Ok(report) => report,
        Err(_) => panic!("source check should complete for a valid source tree"),
    }
}

#[test]
fn links_accept_slug_and_logical_identity_references() {
    let (scratch, context) = source_fixture();
    write_note(
        &scratch.path().join("desk/vault/topic/ref.md"),
        "Referrer",
        &["target", "knowledge:topic:target"],
        false,
    );

    let report = report(&context);
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.field.as_deref() == Some("links"))
    );
}

#[test]
fn broken_slug_reference_reports_its_logical_identity() {
    let (scratch, context) = source_fixture();
    write_note(
        &scratch.path().join("desk/vault/topic/ref.md"),
        "Referrer",
        &["missing"],
        false,
    );

    let report = report(&context);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "KBV2-LINK-BROKEN")
        .expect("a broken slug link should be reported");
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.field.as_deref(), Some("links"));
    assert_eq!(
        finding.message,
        "link target does not resolve: knowledge:topic:missing"
    );
}

#[test]
fn broken_identity_reference_reports_the_same_logical_identity() {
    let (scratch, context) = source_fixture();
    write_note(
        &scratch.path().join("desk/vault/topic/ref.md"),
        "Referrer",
        &["knowledge:topic:missing"],
        false,
    );

    let report = report(&context);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "KBV2-LINK-BROKEN")
        .expect("a broken identity link should be reported");
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.field.as_deref(), Some("links"));
    assert_eq!(
        finding.message,
        "link target does not resolve: knowledge:topic:missing"
    );
}

#[test]
fn frozen_note_exempts_broken_links() {
    let (scratch, context) = source_fixture();
    write_note(
        &scratch.path().join("desk/vault/topic/frozen.md"),
        "Frozen",
        &["knowledge:topic:missing"],
        true,
    );

    let report = report(&context);
    let frozen_path = scratch.path().join("desk/vault/topic/frozen.md");
    assert!(!report.findings.iter().any(|finding| {
        finding.code == "KBV2-LINK-BROKEN" && finding.path.as_deref() == Some(frozen_path.as_path())
    }));
}
