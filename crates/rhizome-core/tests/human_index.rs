use kb_contract::{RegistryLocator, resolve_registry};
use rhizome_core::{
    SourceContext, SourceSnapshot, apply_human_index, check_source, discover_source,
    plan_human_index,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const START_MARKER: &str = "<!-- rhizome:generated-index:start -->";
const END_MARKER: &str = "<!-- rhizome:generated-index:end -->";

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
        let path =
            std::env::temp_dir().join(format!("rhizome-core-human-index-{timestamp}-{sequence}"));
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

fn write_note(path: &Path, description: &str) {
    fs::create_dir_all(path.parent().expect("note should have a parent"))
        .expect("note parent should be created");
    fs::write(
        path,
        format!(
            "---\ndescription: \"{description}\"\nkeywords: [fixture]\nkind: note\n---\n# Note\n"
        ),
    )
    .expect("note should be written");
}

fn write_domain_index(path: &Path, description: &str) {
    fs::create_dir_all(path.parent().expect("index should have a parent"))
        .expect("index parent should be created");
    fs::write(
        path,
        format!(
            "---\ndescription: \"{description}\"\nkeywords: [fixture]\nkind: index\n---\n# Domain\n"
        ),
    )
    .expect("domain index should be written");
}

fn source_context(source_root: &Path, git_root: &Path) -> SourceContext {
    let registry_origin = git_root.join("human-index-registry.toml");
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

fn source_fixture() -> (Scratch, SourceContext, SourceSnapshot, PathBuf) {
    let scratch = Scratch::new();
    let git_root = scratch.path().join("desk");
    let source_root = git_root.join("vault");
    fs::create_dir_all(git_root.join(".git")).expect("git marker should be created");

    // Create the physical directories in reverse order. The snapshot and generated
    // catalog must still use logical domain and source-relative path ordering.
    write_domain_index(&source_root.join("z-domain/INDEX.md"), "Z landing");
    write_note(&source_root.join("z-domain/a.md"), "Z alpha");
    write_domain_index(&source_root.join("a-domain/INDEX.md"), "A landing");
    write_note(
        &source_root.join("a-domain/z.md"),
        "A *literal* description — preserve",
    );
    write_note(&source_root.join("a-domain/a.md"), "A alpha");

    let index = source_root.join("INDEX.md");
    fs::write(
        &index,
        format!(
            "manual prefix\r\n{START_MARKER}\r\nold generated rows\r\n{END_MARKER}\r\nmanual suffix\r\n"
        ),
    )
    .expect("root human index should be written");

    let context = source_context(&source_root, &git_root);
    let snapshot = match discover_source(&context) {
        Ok(snapshot) => snapshot,
        Err(_) => panic!("human-index source fixture should discover"),
    };
    (scratch, context, snapshot, index)
}

fn plan(snapshot: &SourceSnapshot, index: &Path) -> rhizome_core::HumanIndexPlan {
    match plan_human_index(snapshot, index) {
        Ok(plan) => plan,
        Err(_) => panic!("human-index plan should succeed for a marker-delimited root index"),
    }
}

fn generated_catalog(plan: &rhizome_core::HumanIndexPlan) -> String {
    let replacement =
        String::from_utf8(plan.replacement.clone()).expect("generated catalog should be UTF-8");
    assert!(replacement.starts_with(START_MARKER));
    assert!(replacement.ends_with(END_MARKER));
    replacement
}

fn marker_span(bytes: &[u8]) -> (usize, usize) {
    let start = bytes
        .windows(START_MARKER.len())
        .position(|window| window == START_MARKER.as_bytes())
        .expect("start marker should exist");
    let end_start = bytes
        .windows(END_MARKER.len())
        .position(|window| window == END_MARKER.as_bytes())
        .expect("end marker should exist");
    (start, end_start + END_MARKER.len())
}

fn assert_sha256_shape(hash: &str) {
    assert_eq!(
        hash.len(),
        64,
        "SHA-256 is 32 bytes rendered as 64 hex digits"
    );
    assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(hash, hash.to_ascii_lowercase());
}

#[test]
fn generated_catalog_is_deterministic_sorted_and_excludes_indexes() {
    let (_scratch, _context, snapshot, index) = source_fixture();
    let first = plan(&snapshot, &index);
    let second = plan(&snapshot, &index);

    assert_eq!(first.path, fs::canonicalize(&index).unwrap());
    assert_eq!(first.replacement, second.replacement);
    assert_eq!(first.prefix_sha256, second.prefix_sha256);
    assert_eq!(first.suffix_sha256, second.suffix_sha256);
    assert_sha256_shape(&first.prefix_sha256);
    assert_sha256_shape(&first.suffix_sha256);

    let catalog = generated_catalog(&first);
    let expected = format!(
        "{START_MARKER}\n### a-domain\n- [knowledge:a-domain:a](a-domain/a.md) — A alpha\n- [knowledge:a-domain:z](a-domain/z.md) — A *literal* description — preserve\n### z-domain\n- [knowledge:z-domain:a](z-domain/a.md) — Z alpha\n{END_MARKER}"
    );
    assert_eq!(catalog, expected);
    assert_eq!(catalog.matches("- [").count(), 3);
    assert!(!catalog.contains(":INDEX"));
    assert!(!catalog.contains("INDEX.md"));
}

#[test]
fn planning_is_side_effect_free_and_apply_changes_only_marker_region() {
    let (_scratch, _context, snapshot, index) = source_fixture();
    let before = fs::read(&index).expect("root human index should be readable");
    let (start, end) = marker_span(&before);
    let prefix = before[..start].to_vec();
    let suffix = before[end..].to_vec();

    let plan = plan(&snapshot, &index);
    let expected_replacement = plan.replacement.clone();
    assert_eq!(fs::read(&index).unwrap(), before);
    apply_human_index(plan).expect("fresh human-index plan should apply");

    let after = fs::read(&index).expect("updated human index should be readable");
    let (after_start, after_end) = marker_span(&after);
    assert_eq!(&after[..after_start], prefix.as_slice());
    assert_eq!(&after[after_end..], suffix.as_slice());
    assert_eq!(
        &after[after_start..after_end],
        expected_replacement.as_slice()
    );
}

#[test]
fn stale_prefix_or_suffix_hash_fails_closed_without_writing() {
    let (_scratch, _context, snapshot, index) = source_fixture();
    let plan = plan(&snapshot, &index);
    let mut tampered = fs::read(&index).expect("root human index should be readable");
    tampered.splice(..0, b"manual edit\n".iter().copied());
    fs::write(&index, &tampered).expect("tampered human index should be written");

    assert!(apply_human_index(plan).is_err());
    assert_eq!(fs::read(&index).unwrap(), tampered);
}

#[test]
fn marker_layout_errors_are_reported_and_rejected_by_planning() {
    let (_scratch, context, snapshot, index) = source_fixture();
    let cases = [
        (
            "missing",
            format!("manual prefix\n{END_MARKER}\nmanual suffix\n"),
            "human index markers are missing",
        ),
        (
            "reversed",
            format!("manual prefix\n{END_MARKER}\nold\n{START_MARKER}\nmanual suffix\n"),
            "human index end marker must follow start marker",
        ),
        (
            "duplicate",
            format!("{START_MARKER}\none\n{START_MARKER}\ntwo\n{END_MARKER}\n{END_MARKER}"),
            "human index markers must occur exactly once",
        ),
    ];

    for (name, bytes, message) in cases {
        fs::write(&index, bytes).expect("marker case should be written");
        assert!(
            plan_human_index(&snapshot, &index).is_err(),
            "{name} marker layout must not produce an apply plan"
        );
        let report = match check_source(&context) {
            Ok(report) => report,
            Err(_) => panic!("marker errors should be represented as check findings"),
        };
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.code == "KBV2-HUMAN-INDEX-MARKER")
            .expect("marker layout should produce a stable finding");
        assert_eq!(finding.message, message);
    }
}

#[test]
fn description_drift_is_reported_against_the_validated_note() {
    let (_scratch, context, snapshot, index) = source_fixture();
    fs::write(
        &index,
        format!(
            "manual prefix\n{START_MARKER}\n### a-domain\n- [knowledge:a-domain:a](a-domain/a.md) — stale description\n{END_MARKER}\nmanual suffix\n"
        ),
    )
    .expect("stale catalog should be written");

    let report = match check_source(&context) {
        Ok(report) => report,
        Err(_) => panic!("description drift should be represented as a check finding"),
    };
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "KBV2-HUMAN-INDEX-DRIFT")
        .expect("description drift should be reported");
    assert_eq!(
        finding.message,
        "human index generated catalog is out of date"
    );
    assert_eq!(finding.path.as_deref(), Some(index.as_path()));

    let repaired = plan(&snapshot, &index);
    assert!(generated_catalog(&repaired).contains("A alpha"));
    assert!(generated_catalog(&repaired).contains("A *literal* description — preserve"));
}

#[test]
fn index_outside_snapshot_source_root_is_rejected() {
    let (scratch, _context, snapshot, _index) = source_fixture();
    let outside = scratch.path().join("outside/INDEX.md");
    fs::create_dir_all(outside.parent().unwrap()).expect("outside directory should be created");
    fs::write(
        &outside,
        format!("prefix\n{START_MARKER}\nold\n{END_MARKER}\nsuffix\n"),
    )
    .expect("outside index should be written");

    assert!(plan_human_index(&snapshot, &outside).is_err());
}

#[test]
fn ancestor_of_snapshot_source_root_is_rejected() {
    let (scratch, _context, snapshot, _index) = source_fixture();
    let outside = scratch.path().join("INDEX.md");
    fs::write(
        &outside,
        format!("prefix\n{START_MARKER}\nold\n{END_MARKER}\nsuffix\n"),
    )
    .expect("ancestor index should be written");

    assert!(plan_human_index(&snapshot, &outside).is_err());
}

#[test]
fn wrong_root_filename_is_rejected() {
    let (scratch, _context, snapshot, _index) = source_fixture();
    let wrong_name = scratch.path().join("desk/vault/NOT-INDEX.md");
    fs::write(
        &wrong_name,
        format!("prefix\n{START_MARKER}\nold\n{END_MARKER}\nsuffix\n"),
    )
    .expect("wrongly named index should be written");

    assert!(plan_human_index(&snapshot, &wrong_name).is_err());
}

#[test]
fn domain_index_path_alias_is_rejected() {
    let (_scratch, _context, snapshot, _index) = source_fixture();
    let domain_index = &snapshot.domains[0].index_path;
    let alias = domain_index
        .parent()
        .expect("domain index should have a parent")
        .join(".")
        .join("INDEX.md");

    assert!(plan_human_index(&snapshot, &alias).is_err());
}

#[test]
fn root_index_alias_to_ordinary_file_is_rejected_without_writing() {
    let (scratch, _context, snapshot, index) = source_fixture();
    let plan = plan(&snapshot, &index);
    let ordinary = scratch.path().join("desk/vault/ordinary.md");
    let ordinary_bytes =
        format!("ordinary\n{START_MARKER}\nwould be overwritten\n{END_MARKER}\n").into_bytes();
    fs::write(&ordinary, &ordinary_bytes).expect("ordinary file should be written");
    fs::remove_file(&index).expect("root index should be removed before aliasing");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&ordinary, &index).expect("root index symlink should be created");
    #[cfg(windows)]
    fs::hard_link(&ordinary, &index).expect("root index hardlink should be created");
    #[cfg(not(any(unix, windows)))]
    return;

    assert!(apply_human_index(plan).is_err());
    assert_eq!(
        fs::read(&ordinary).expect("ordinary file should remain readable"),
        ordinary_bytes
    );
}

#[test]
fn embedded_marker_text_is_not_a_marker_line() {
    let (_scratch, context, snapshot, index) = source_fixture();
    fs::write(
        &index,
        format!("prefix{START_MARKER}suffix\n{END_MARKER}\n"),
    )
    .expect("embedded marker text should be written");

    assert!(plan_human_index(&snapshot, &index).is_err());
    let report = check_source(&context).expect("marker errors should be findings");
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "KBV2-HUMAN-INDEX-MARKER")
        .expect("embedded marker should be treated as missing");
    assert_eq!(finding.message, "human index markers are missing");
}

#[test]
fn bare_carriage_return_does_not_end_marker_line() {
    let (_scratch, _context, snapshot, index) = source_fixture();
    fs::write(&index, format!("{START_MARKER}\rX\n{END_MARKER}\n"))
        .expect("bare carriage-return marker text should be written");

    assert!(plan_human_index(&snapshot, &index).is_err());
}

#[cfg(unix)]
#[test]
fn source_root_parent_symlink_alias_is_rejected() {
    let (scratch, _context, snapshot, _index) = source_fixture();
    let source_root = scratch.path().join("desk/vault");
    let alias_root = scratch.path().join("desk/vault-alias");
    std::os::unix::fs::symlink(&source_root, &alias_root)
        .expect("source-root alias symlink should be created");

    assert!(plan_human_index(&snapshot, &alias_root.join("INDEX.md")).is_err());
}
