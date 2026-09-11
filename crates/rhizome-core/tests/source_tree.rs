use kb_contract::{
    Diagnostic, DomainId, Identity, NoteKind, RegistryLocator, Severity, SourceName, SourceSpec,
    ValidatedNote, resolve_registry,
};
use rhizome_core::{
    DomainNode, NoteLocator, SnapshotNote, SourceContext, SourceSnapshot, discover_source,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const GIT_ROOT_MESSAGE: &str = "Git root must be an existing directory containing .git";
const OUTSIDE_GIT_MESSAGE: &str = "source root must be contained by the Git root";
const READ_MESSAGE: &str = "source entry could not be read";
const DUPLICATE_DOMAIN_MESSAGE: &str = "domain is duplicated";
const INDEX_KIND_MESSAGE: &str = "domain INDEX.md must declare kind `index`";
const IDENTITY_COLLISION_MESSAGE: &str = "note identity is duplicated";

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
static NEXT_REGISTRY: AtomicU64 = AtomicU64::new(0);

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "knowledge-infra-source-tree-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("scratch directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
struct ReadBlock {
    path: PathBuf,
    permissions: fs::Permissions,
}

#[cfg(unix)]
impl Drop for ReadBlock {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, self.permissions.clone());
    }
}

#[cfg(windows)]
struct ReadBlock {
    _file: fs::File,
}

#[cfg(not(any(unix, windows)))]
struct ReadBlock;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/source-contract-v2")
}

fn make_dir(path: impl AsRef<Path>) {
    fs::create_dir_all(path).expect("fixture directory should be created");
}

fn write_note(path: impl AsRef<Path>, description: &str) {
    let path = path.as_ref();
    make_dir(path.parent().expect("note should have a parent"));
    fs::write(
        path,
        format!(
            "---\ndescription: \"{description}\"\nkeywords: [fixture]\nkind: note\n---\n# Fixture\n"
        ),
    )
    .expect("fixture note should be written");
}

fn write_index(path: impl AsRef<Path>, description: &str) {
    let path = path.as_ref();
    make_dir(path.parent().expect("index should have a parent"));
    fs::write(
        path,
        format!(
            "---\ndescription: \"{description}\"\nkeywords: [fixture]\nkind: index\n---\n# Domain\n"
        ),
    )
    .expect("fixture index should be written");
}

fn copy_tree(source: &Path, destination: &Path) {
    make_dir(destination);
    let mut entries = fs::read_dir(source)
        .expect("fixture tree should be readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture entries should be readable");
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source_path = entry.path();
        let file_name = entry.file_name();
        let destination_name = match file_name.to_str() {
            Some("GIT_MARKER") | Some("DOT_GIT") => ".git",
            _ => file_name
                .to_str()
                .expect("sanitized fixture names should be UTF-8"),
        };
        let destination_path = destination.join(destination_name);
        let file_type = entry.file_type().expect("fixture type should be readable");
        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("fixture file should be copied");
        }
    }
}

fn install_fixture(scratch: &ScratchDirectory, relative: &str, destination: &str) -> PathBuf {
    let destination = scratch.path().join(destination);
    copy_tree(&fixtures().join(relative), &destination);
    destination
}

fn source_context(source_root: &Path, git_root: &Path) -> SourceContext {
    let origin_dir = source_root
        .parent()
        .expect("fixture source root should have a parent");
    let source_dir = source_root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("fixture source directory should be UTF-8");
    assert!(
        source_dir
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-'),
        "test registry writer accepts only its controlled directory names"
    );
    let sequence = NEXT_REGISTRY.fetch_add(1, Ordering::Relaxed);
    let registry_origin = origin_dir.join(format!("source-tree-{sequence}.toml"));
    fs::write(
        &registry_origin,
        format!("[[source]]\nname = \"knowledge\"\npath = \"{source_dir}\"\nsurface = \"core\"\n"),
    )
    .expect("fixture registry should be written");
    let locator = RegistryLocator {
        explicit: Some(registry_origin.clone()),
        cwd: origin_dir.to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: origin_dir.join("unused-user-registry.toml"),
    };
    let registry = match resolve_registry(&locator) {
        Ok(registry) => registry,
        Err(diagnostics) => panic!(
            "fixture registry should resolve, but returned {} diagnostics",
            diagnostics.len()
        ),
    };
    let source = registry
        .sources
        .into_values()
        .next()
        .expect("fixture registry should contain one source");

    SourceContext {
        source,
        git_root: git_root.to_path_buf(),
        registry_origin,
    }
}

fn discover_ok(context: &SourceContext) -> SourceSnapshot {
    match discover_source(context) {
        Ok(snapshot) => snapshot,
        Err(diagnostics) => panic!(
            "source fixture should discover, but returned {} diagnostics: {diagnostics:?}",
            diagnostics.len()
        ),
    }
}

fn discover_err(context: &SourceContext) -> Vec<Diagnostic> {
    match discover_source(context) {
        Ok(_) => panic!("source fixture should fail discovery"),
        Err(diagnostics) => diagnostics,
    }
}

fn source_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("snapshot path should be inside the canonical source root")
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn domain_rows(snapshot: &SourceSnapshot, root: &Path) -> Vec<(String, String, String)> {
    snapshot
        .domains
        .iter()
        .map(|node| {
            (
                node.id.as_str().to_owned(),
                source_relative(root, &node.physical_dir),
                source_relative(root, &node.index_path),
            )
        })
        .collect()
}

fn note_rows(
    snapshot: &SourceSnapshot,
    root: &Path,
) -> Vec<(String, String, String, bool, String, NoteKind)> {
    snapshot
        .notes
        .iter()
        .map(|snapshot_note| {
            let locator = &snapshot_note.locator;
            (
                locator.domain.as_str().to_owned(),
                source_relative(root, &locator.path),
                locator.slug.clone(),
                locator.is_domain_index,
                locator.identity.as_str().to_owned(),
                snapshot_note.note.frontmatter.kind,
            )
        })
        .collect()
}

fn identities(snapshot: &SourceSnapshot) -> Vec<String> {
    snapshot
        .notes
        .iter()
        .map(|note| note.locator.identity.as_str().to_owned())
        .collect()
}

fn assert_public_shapes(context: &SourceContext, snapshot: &SourceSnapshot) {
    let _: &SourceSpec = &context.source;
    let _: &PathBuf = &context.git_root;
    let _: &PathBuf = &context.registry_origin;

    let _: &SourceName = &snapshot.source;
    let _: &Vec<DomainNode> = &snapshot.domains;
    let _: &Vec<SnapshotNote> = &snapshot.notes;

    for node in &snapshot.domains {
        let _: &DomainId = &node.id;
        let _: &PathBuf = &node.physical_dir;
        let _: &PathBuf = &node.index_path;
    }
    for snapshot_note in &snapshot.notes {
        let _: &NoteLocator = &snapshot_note.locator;
        let _: &ValidatedNote = &snapshot_note.note;
        let _: &Identity = &snapshot_note.locator.identity;
        let _: &DomainId = &snapshot_note.locator.domain;
        let _: &String = &snapshot_note.locator.slug;
        let _: &PathBuf = &snapshot_note.locator.path;
        let _: bool = snapshot_note.locator.is_domain_index;
    }
}

fn assert_diagnostic(
    diagnostic: &Diagnostic,
    code: &'static str,
    path: &Path,
    field: Option<&str>,
    message: &str,
) {
    assert_eq!(diagnostic.code, code);
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.path.as_deref(), Some(path));
    assert_eq!(diagnostic.field.as_deref(), field);
    assert_eq!(diagnostic.message, message);
}

fn install_valid_tree(scratch: &ScratchDirectory) -> (PathBuf, PathBuf) {
    let git_root = install_fixture(scratch, "domains/valid-tree/desk", "desk");
    let source_root = git_root.join("vault");
    (git_root, source_root)
}

#[cfg(unix)]
fn block_reads(path: &Path) -> Option<ReadBlock> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::metadata(path)
        .expect("blocked fixture metadata should be readable")
        .permissions();
    let mut blocked = permissions.clone();
    blocked.set_mode(0);
    fs::set_permissions(path, blocked).expect("fixture permissions should be changed");
    Some(ReadBlock {
        path: path.to_path_buf(),
        permissions,
    })
}

#[cfg(windows)]
fn block_reads(path: &Path) -> Option<ReadBlock> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .expect("fixture file should be exclusively opened");
    Some(ReadBlock { _file: file })
}

#[cfg(not(any(unix, windows)))]
fn block_reads(_path: &Path) -> Option<ReadBlock> {
    None
}

#[cfg(unix)]
fn make_directory_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).expect("fixture directory symlink should be created");
    true
}

#[cfg(windows)]
fn make_directory_symlink(target: &Path, link: &Path) -> bool {
    match std::os::windows::fs::symlink_dir(target, link) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("fixture directory symlink should be created: {error}"),
    }
}

#[cfg(not(any(unix, windows)))]
fn make_directory_symlink(_target: &Path, _link: &Path) -> bool {
    false
}

#[test]
fn discovery_uses_c2_domains_and_the_nearest_ancestor_domain() {
    let scratch = ScratchDirectory::new();
    let (git_root, source_root) = install_valid_tree(&scratch);
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);

    assert_public_shapes(&context, &snapshot);
    assert_eq!(snapshot.source.as_str(), "knowledge");
    assert_eq!(
        domain_rows(&snapshot, &context.source.root),
        vec![
            (
                "01-基础".to_owned(),
                "z-physical/01-基础".to_owned(),
                "z-physical/01-基础/INDEX.md".to_owned(),
            ),
            (
                "10-知识笔记".to_owned(),
                "a-physical/10-知识笔记".to_owned(),
                "a-physical/10-知识笔记/INDEX.md".to_owned(),
            ),
            (
                "10-知识笔记/20-系统设计".to_owned(),
                "a-physical/10-知识笔记/bridge/20-系统设计".to_owned(),
                "a-physical/10-知识笔记/bridge/20-系统设计/INDEX.md".to_owned(),
            ),
            (
                "Café".to_owned(),
                "u-physical/Cafe\u{301}".to_owned(),
                "u-physical/Cafe\u{301}/INDEX.md".to_owned(),
            ),
        ]
    );

    let membership = snapshot
        .notes
        .iter()
        .map(|note| {
            (
                source_relative(&context.source.root, &note.locator.path),
                note.locator.domain.as_str().to_owned(),
                note.locator.slug.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert!(membership.contains(&(
        "a-physical/10-知识笔记/guides/20-细节.md".to_owned(),
        "10-知识笔记".to_owned(),
        "20-细节".to_owned(),
    )));
    assert!(membership.contains(&(
        "a-physical/10-知识笔记/bridge/20-系统设计/10-架构.md".to_owned(),
        "10-知识笔记/20-系统设计".to_owned(),
        "10-架构".to_owned(),
    )));
}

#[test]
fn root_and_lowercase_indexes_do_not_create_domains_or_notes() {
    let scratch = ScratchDirectory::new();
    let (git_root, source_root) = install_valid_tree(&scratch);
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);
    let paths = snapshot
        .notes
        .iter()
        .map(|note| source_relative(&context.source.root, &note.locator.path))
        .collect::<Vec<_>>();

    assert!(
        !snapshot
            .domains
            .iter()
            .any(|domain| domain.id.as_str().is_empty())
    );
    assert!(!paths.iter().any(|path| path == "INDEX.md"));
    assert!(!paths.iter().any(|path| path == "lowercase/index.md"));
    assert!(
        !snapshot
            .domains
            .iter()
            .any(|domain| domain.id.as_str() == "lowercase")
    );
}

#[test]
fn outside_domain_markdown_is_ignored_before_note_parsing() {
    let scratch = ScratchDirectory::new();
    let (git_root, source_root) = install_valid_tree(&scratch);
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);
    let paths = snapshot
        .notes
        .iter()
        .map(|note| source_relative(&context.source.root, &note.locator.path))
        .collect::<Vec<_>>();

    assert!(!paths.iter().any(|path| path == "outside/valid.md"));
    assert!(!paths.iter().any(|path| path == "outside/malformed.md"));
}

#[test]
fn skipped_directories_are_not_read_or_returned() {
    let scratch = ScratchDirectory::new();
    let (git_root, source_root) = install_valid_tree(&scratch);
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);
    let paths = snapshot
        .notes
        .iter()
        .map(|note| source_relative(&context.source.root, &note.locator.path))
        .chain(
            snapshot
                .domains
                .iter()
                .map(|domain| source_relative(&context.source.root, &domain.index_path)),
        )
        .collect::<Vec<_>>();
    let skipped = [
        ".git",
        ".obsidian",
        ".private",
        ".venv",
        ".legacy-index",
        "node_modules",
        "target",
        "dist",
    ];

    for directory in skipped {
        assert!(
            paths
                .iter()
                .all(|path| path != directory && !path.starts_with(&format!("{directory}/"))),
            "{directory} must be pruned before any entry is read"
        );
    }
}

#[test]
fn each_non_root_index_is_one_validated_index_locator() {
    let scratch = ScratchDirectory::new();
    let (git_root, source_root) = install_valid_tree(&scratch);
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);
    let index_notes = snapshot
        .notes
        .iter()
        .filter(|note| note.locator.is_domain_index)
        .collect::<Vec<_>>();

    assert_eq!(index_notes.len(), snapshot.domains.len());
    for domain in &snapshot.domains {
        let matches = index_notes
            .iter()
            .filter(|note| note.locator.path == domain.index_path)
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "each domain landing appears exactly once");
        let landing = matches[0];
        assert_eq!(landing.locator.domain, domain.id);
        assert_eq!(landing.locator.slug, "INDEX");
        assert_eq!(landing.note.frontmatter.kind, NoteKind::Index);
        assert_eq!(
            landing.locator.identity.as_str(),
            format!("knowledge:{}:INDEX", domain.id)
        );
    }
}

#[test]
fn snapshot_order_is_domain_then_source_relative_path() {
    let scratch = ScratchDirectory::new();
    let (git_root, source_root) = install_valid_tree(&scratch);
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);

    assert_eq!(
        note_rows(&snapshot, &context.source.root),
        vec![
            (
                "01-基础".to_owned(),
                "z-physical/01-基础/05-引导.md".to_owned(),
                "05-引导".to_owned(),
                false,
                "knowledge:01-基础:05-引导".to_owned(),
                NoteKind::Note,
            ),
            (
                "01-基础".to_owned(),
                "z-physical/01-基础/INDEX.md".to_owned(),
                "INDEX".to_owned(),
                true,
                "knowledge:01-基础:INDEX".to_owned(),
                NoteKind::Index,
            ),
            (
                "10-知识笔记".to_owned(),
                "a-physical/10-知识笔记/10-概览.md".to_owned(),
                "10-概览".to_owned(),
                false,
                "knowledge:10-知识笔记:10-概览".to_owned(),
                NoteKind::Note,
            ),
            (
                "10-知识笔记".to_owned(),
                "a-physical/10-知识笔记/INDEX.md".to_owned(),
                "INDEX".to_owned(),
                true,
                "knowledge:10-知识笔记:INDEX".to_owned(),
                NoteKind::Index,
            ),
            (
                "10-知识笔记".to_owned(),
                "a-physical/10-知识笔记/guides/20-细节.md".to_owned(),
                "20-细节".to_owned(),
                false,
                "knowledge:10-知识笔记:20-细节".to_owned(),
                NoteKind::Note,
            ),
            (
                "10-知识笔记/20-系统设计".to_owned(),
                "a-physical/10-知识笔记/bridge/20-系统设计/10-架构.md".to_owned(),
                "10-架构".to_owned(),
                false,
                "knowledge:10-知识笔记/20-系统设计:10-架构".to_owned(),
                NoteKind::Note,
            ),
            (
                "10-知识笔记/20-系统设计".to_owned(),
                "a-physical/10-知识笔记/bridge/20-系统设计/INDEX.md".to_owned(),
                "INDEX".to_owned(),
                true,
                "knowledge:10-知识笔记/20-系统设计:INDEX".to_owned(),
                NoteKind::Index,
            ),
            (
                "Café".to_owned(),
                "u-physical/Cafe\u{301}/10-résumé.md".to_owned(),
                "10-résumé".to_owned(),
                false,
                "knowledge:Café:10-résumé".to_owned(),
                NoteKind::Note,
            ),
            (
                "Café".to_owned(),
                "u-physical/Cafe\u{301}/INDEX.md".to_owned(),
                "INDEX".to_owned(),
                true,
                "knowledge:Café:INDEX".to_owned(),
                NoteKind::Index,
            ),
        ]
    );
}

#[test]
fn snapshot_note_paths_use_source_relative_posix_string_order() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "domains/note-order/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);

    assert_eq!(
        snapshot
            .notes
            .iter()
            .map(|note| source_relative(&context.source.root, &note.locator.path))
            .collect::<Vec<_>>(),
        vec![
            "topic/INDEX.md",
            "topic/blue-template/b.md",
            "topic/blue/a.md",
        ]
    );
}

#[test]
fn source_and_git_root_moves_leave_all_logical_identities_unchanged() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "identity/stable/desk", "desk");
    let source_root = git_root.join("vault");
    let baseline_context = source_context(&source_root, &git_root);
    let baseline = identities(&discover_ok(&baseline_context));

    assert_eq!(
        baseline,
        vec![
            "knowledge:10-知识笔记:10-概览".to_owned(),
            "knowledge:10-知识笔记:INDEX".to_owned(),
            "knowledge:10-知识笔记:20-细节".to_owned(),
            "knowledge:10-知识笔记/30-系统设计:10-架构".to_owned(),
            "knowledge:10-知识笔记/30-系统设计:INDEX".to_owned(),
        ]
    );
    assert!(
        baseline
            .iter()
            .all(|identity| identity.starts_with("knowledge:"))
    );
    assert!(
        baseline
            .iter()
            .all(|identity| !identity.starts_with("desk:"))
    );

    let moved_source_root = git_root.join("relocated-vault");
    fs::rename(&source_root, &moved_source_root).expect("source root should move physically");
    let moved_source_context = source_context(&moved_source_root, &git_root);
    let after_source_move = identities(&discover_ok(&moved_source_context));
    assert_eq!(after_source_move, baseline);

    let moved_git_root = scratch.path().join("desk-worktree");
    fs::rename(&git_root, &moved_git_root).expect("Git root should move like a worktree");
    let worktree_source_root = moved_git_root.join("relocated-vault");
    let moved_git_context = source_context(&worktree_source_root, &moved_git_root);
    let after_git_move = identities(&discover_ok(&moved_git_context));
    assert_eq!(after_git_move, baseline);
}

#[test]
fn wrong_kind_on_a_domain_index_has_one_stable_diagnostic() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "domains/wrong-index/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 1);
    assert_diagnostic(
        &diagnostics[0],
        "KBV2-SOURCE-INDEX-KIND",
        &context.source.root.join("topic/INDEX.md"),
        Some("kind"),
        INDEX_KIND_MESSAGE,
    );
}

#[test]
fn duplicate_c2_domain_reports_both_index_paths_in_deterministic_order() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "domains/duplicate-c2/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 2);
    let expected = [
        "domain-map/blue-template/use-cases/INDEX.md",
        "domain-map/blue/use-cases/INDEX.md",
    ];
    for (diagnostic, relative) in diagnostics.iter().zip(expected) {
        assert_diagnostic(
            diagnostic,
            "KBV2-SOURCE-DUPLICATE-DOMAIN",
            &context.source.root.join(relative),
            None,
            DUPLICATE_DOMAIN_MESSAGE,
        );
    }
}

#[test]
fn nfc_and_full_unicode_casefold_domain_collision_reports_both_indexes() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "domains/unicode-collision/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 2);
    let expected = ["left/ﬀe\u{301}/INDEX.md", "right/FFÉ/INDEX.md"];
    for (diagnostic, relative) in diagnostics.iter().zip(expected) {
        let expected_path = fs::canonicalize(context.source.root.join(relative))
            .expect("fixture path should exist");
        assert_diagnostic(
            diagnostic,
            "KBV2-SOURCE-DUPLICATE-DOMAIN",
            &expected_path,
            None,
            DUPLICATE_DOMAIN_MESSAGE,
        );
    }
}

#[test]
fn nfc_and_full_unicode_casefold_note_collision_reports_both_paths() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "identity/unicode-collision/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 2);
    let expected = ["topic/left/ﬀe\u{301}.md", "topic/right/FFÉ.md"];
    for (diagnostic, relative) in diagnostics.iter().zip(expected) {
        let expected_path = fs::canonicalize(context.source.root.join(relative))
            .expect("fixture path should exist");
        assert_diagnostic(
            diagnostic,
            "KBV2-IDENTITY-COLLISION",
            &expected_path,
            None,
            IDENTITY_COLLISION_MESSAGE,
        );
    }
}

#[test]
fn unicode_seventeen_full_casefold_note_collision_reports_both_paths() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "identity/unicode-17-collision/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 2);
    let expected = ["topic/left/Ა.md", "topic/right/ა.md"];
    for (diagnostic, relative) in diagnostics.iter().zip(expected) {
        assert_diagnostic(
            diagnostic,
            "KBV2-IDENTITY-COLLISION",
            &context.source.root.join(relative),
            None,
            IDENTITY_COLLISION_MESSAGE,
        );
    }
}

#[test]
fn filename_stem_identity_ignores_non_domain_subdirectories() {
    let scratch = ScratchDirectory::new();
    let git_root = install_fixture(&scratch, "identity/stem-collision/desk", "desk");
    let source_root = git_root.join("vault");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 2);
    let expected = ["topic/left/same.md", "topic/right/same.md"];
    for (diagnostic, relative) in diagnostics.iter().zip(expected) {
        assert_diagnostic(
            diagnostic,
            "KBV2-IDENTITY-COLLISION",
            &context.source.root.join(relative),
            None,
            IDENTITY_COLLISION_MESSAGE,
        );
    }
}

#[test]
fn source_root_may_equal_git_root_or_be_nested_with_either_git_marker_shape() {
    let scratch = ScratchDirectory::new();

    let equal_root = scratch.path().join("equal-root");
    make_dir(equal_root.join(".git"));
    write_index(equal_root.join("topic/INDEX.md"), "Equal root domain");
    write_note(equal_root.join("topic/note.md"), "Equal root note");
    let equal_context = source_context(&equal_root, &equal_root);
    assert_eq!(discover_ok(&equal_context).domains.len(), 1);

    let nested_git_root = scratch.path().join("nested-root");
    make_dir(&nested_git_root);
    fs::write(nested_git_root.join(".git"), "gitdir: fixture\n")
        .expect("file-shaped .git marker should be written");
    let nested_source_root = nested_git_root.join("vault");
    write_index(
        nested_source_root.join("topic/INDEX.md"),
        "Nested root domain",
    );
    write_note(nested_source_root.join("topic/note.md"), "Nested root note");
    let nested_context = source_context(&nested_source_root, &nested_git_root);
    assert_eq!(discover_ok(&nested_context).domains.len(), 1);
}

#[test]
fn invalid_git_roots_have_one_stable_diagnostic_each() {
    let scratch = ScratchDirectory::new();
    let source_root = scratch.path().join("source");
    write_index(source_root.join("topic/INDEX.md"), "Source domain");

    let missing = scratch.path().join("missing-git-root");
    let file = scratch.path().join("git-root-file");
    fs::write(&file, "not a directory").expect("file-shaped invalid Git root should be written");
    let no_marker = scratch.path().join("no-marker");
    make_dir(&no_marker);

    for git_root in [&missing, &file, &no_marker] {
        let context = source_context(&source_root, git_root);
        let diagnostics = discover_err(&context);
        assert_eq!(diagnostics.len(), 1);
        assert_diagnostic(
            &diagnostics[0],
            "KBV2-SOURCE-GIT-ROOT",
            git_root,
            None,
            GIT_ROOT_MESSAGE,
        );
    }
}

#[test]
fn canonical_source_root_must_be_contained_by_git_root() {
    let scratch = ScratchDirectory::new();
    let git_root = scratch.path().join("desk");
    make_dir(git_root.join(".git"));
    let source_root = scratch.path().join("outside");
    write_index(source_root.join("topic/INDEX.md"), "Outside domain");
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 1);
    assert_diagnostic(
        &diagnostics[0],
        "KBV2-SOURCE-OUTSIDE-GIT",
        &context.source.root,
        None,
        OUTSIDE_GIT_MESSAGE,
    );
}

#[test]
fn source_root_symlink_cannot_hide_an_escape_from_git_root() {
    let scratch = ScratchDirectory::new();
    let git_root = scratch.path().join("desk");
    make_dir(git_root.join(".git"));
    let outside = scratch.path().join("outside");
    write_index(outside.join("topic/INDEX.md"), "Outside domain");
    let link = git_root.join("linked-source");
    if !make_directory_symlink(&outside, &link) {
        return;
    }
    let context = source_context(&link, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 1);
    assert_diagnostic(
        &diagnostics[0],
        "KBV2-SOURCE-OUTSIDE-GIT",
        &context.source.root,
        None,
        OUTSIDE_GIT_MESSAGE,
    );
    assert_eq!(context.source.root, fs::canonicalize(&outside).unwrap());
}

#[test]
fn directory_symlink_escapes_are_never_followed() {
    let scratch = ScratchDirectory::new();
    let git_root = scratch.path().join("desk");
    make_dir(git_root.join(".git"));
    let source_root = git_root.join("vault");
    write_index(source_root.join("inside/INDEX.md"), "Inside domain");
    write_note(source_root.join("inside/note.md"), "Inside note");

    let outside = scratch.path().join("outside-domain");
    write_index(outside.join("INDEX.md"), "Escaped domain");
    write_note(outside.join("escaped.md"), "Escaped note");
    if !make_directory_symlink(&outside, &source_root.join("linked-domain")) {
        return;
    }
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);

    assert_eq!(
        snapshot
            .domains
            .iter()
            .map(|domain| domain.id.as_str())
            .collect::<Vec<_>>(),
        vec!["inside"]
    );
    assert!(snapshot.notes.iter().all(|note| {
        !source_relative(&context.source.root, &note.locator.path).starts_with("linked-domain/")
    }));
}

#[cfg(unix)]
#[test]
fn deep_walk_does_not_retain_one_directory_handle_per_level() {
    let scratch = ScratchDirectory::new();
    let git_root = scratch.path().join("desk");
    make_dir(git_root.join(".git"));
    let source_root = git_root.join("vault");
    let mut deepest = source_root.clone();
    for _ in 0..300 {
        deepest.push("d");
    }
    write_index(deepest.join("INDEX.md"), "Deep domain");
    write_note(deepest.join("note.md"), "Deep note");
    let context = source_context(&source_root, &git_root);

    let snapshot = discover_ok(&context);

    assert_eq!(snapshot.domains.len(), 1);
    assert_eq!(snapshot.domains[0].id.as_str(), "d");
    assert_eq!(snapshot.notes.len(), 2);
}

#[test]
fn unreadable_eligible_note_fails_loud_without_os_detail() {
    let scratch = ScratchDirectory::new();
    let git_root = scratch.path().join("desk");
    make_dir(git_root.join(".git"));
    let source_root = git_root.join("vault");
    write_index(source_root.join("topic/INDEX.md"), "Readable domain");
    let blocked = source_root.join("topic/blocked.md");
    write_note(&blocked, "Blocked note");
    let Some(_block) = block_reads(&blocked) else {
        return;
    };
    let context = source_context(&source_root, &git_root);

    let diagnostics = discover_err(&context);

    assert_eq!(diagnostics.len(), 1);
    assert_diagnostic(
        &diagnostics[0],
        "KBV2-SOURCE-READ",
        &context.source.root.join("topic/blocked.md"),
        None,
        READ_MESSAGE,
    );
    assert!(!diagnostics[0].message.contains("os error"));
    assert!(
        !diagnostics[0]
            .message
            .contains(&blocked.display().to_string())
    );
}
