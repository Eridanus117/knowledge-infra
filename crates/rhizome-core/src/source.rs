use kb_contract::{
    Diagnostic, DomainId, Identity, NoteKind, SourceName, SourceSpec, ValidatedNote,
    derive_identity, parse_and_validate_note,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

const GIT_ROOT: &str = "KBV2-SOURCE-GIT-ROOT";
const OUTSIDE_GIT: &str = "KBV2-SOURCE-OUTSIDE-GIT";
const READ: &str = "KBV2-SOURCE-READ";
const DUPLICATE_DOMAIN: &str = "KBV2-SOURCE-DUPLICATE-DOMAIN";
const INDEX_KIND: &str = "KBV2-SOURCE-INDEX-KIND";
const IDENTITY_COLLISION: &str = "KBV2-IDENTITY-COLLISION";

const GIT_ROOT_MESSAGE: &str = "Git root must be an existing directory containing .git";
const OUTSIDE_GIT_MESSAGE: &str = "source root must be contained by the Git root";
const READ_MESSAGE: &str = "source entry could not be read";
const DUPLICATE_DOMAIN_MESSAGE: &str = "domain is duplicated";
const INDEX_KIND_MESSAGE: &str = "domain INDEX.md must declare kind `index`";
const IDENTITY_COLLISION_MESSAGE: &str = "note identity is duplicated";

const INDEX_FILENAME: &str = "INDEX.md";
const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    ".obsidian",
    ".venv",
    ".legacy-index",
    "node_modules",
    "target",
    "dist",
];

/// Logical and physical boundaries required to discover one registered source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceContext {
    pub source: SourceSpec,
    pub git_root: PathBuf,
    pub registry_origin: PathBuf,
}

/// One non-root directory that owns an exact `INDEX.md` domain landing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainNode {
    pub id: DomainId,
    pub physical_dir: PathBuf,
    pub index_path: PathBuf,
}

/// Position-derived identity and physical location of one validated source note.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteLocator {
    pub identity: Identity,
    pub domain: DomainId,
    pub slug: String,
    pub path: PathBuf,
    pub is_domain_index: bool,
}

/// A validated note paired with its source-owned locator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotNote {
    pub locator: NoteLocator,
    pub note: ValidatedNote,
}

/// A deterministic, all-or-nothing view of one registered Markdown source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSnapshot {
    pub source: SourceName,
    pub domains: Vec<DomainNode>,
    pub notes: Vec<SnapshotNote>,
}

struct WalkedSource {
    domain_indexes: Vec<PathBuf>,
    markdown: Vec<PathBuf>,
}

/// Discover and validate every domain and in-domain Markdown note in a source.
pub fn discover_source(context: &SourceContext) -> Result<SourceSnapshot, Vec<Diagnostic>> {
    let source_root = validate_roots(context).map_err(|diagnostic| vec![diagnostic])?;
    let walked = walk_source(&source_root)?;
    let domains = build_domains(&source_root, &walked.domain_indexes)?;
    reject_duplicate_domains(&source_root, &domains)?;

    let mut diagnostics = Vec::new();
    let mut notes = load_domain_indexes(context, &domains, &mut diagnostics);
    load_ordinary_notes(
        context,
        &source_root,
        &domains,
        &walked.markdown,
        &mut notes,
        &mut diagnostics,
    );
    if !diagnostics.is_empty() {
        sort_diagnostics(&source_root, &mut diagnostics);
        return Err(diagnostics);
    }

    reject_identity_collisions(&source_root, &notes)?;
    notes.sort_by(|left, right| {
        left.locator
            .domain
            .cmp(&right.locator.domain)
            .then_with(|| left.locator.path.cmp(&right.locator.path))
    });

    Ok(SourceSnapshot {
        source: context.source.name.clone(),
        domains,
        notes,
    })
}

fn validate_roots(context: &SourceContext) -> Result<PathBuf, Diagnostic> {
    let git_root = fs::canonicalize(&context.git_root)
        .map_err(|_| git_root_diagnostic(context.git_root.clone()))?;
    let git_metadata =
        fs::metadata(&git_root).map_err(|_| git_root_diagnostic(context.git_root.clone()))?;
    if !git_metadata.is_dir() || !has_exact_git_marker(&git_root)? {
        return Err(git_root_diagnostic(context.git_root.clone()));
    }

    let source_root = fs::canonicalize(&context.source.root)
        .map_err(|_| read_diagnostic(context.source.root.clone()))?;
    let source_metadata =
        fs::metadata(&source_root).map_err(|_| read_diagnostic(context.source.root.clone()))?;
    if !source_metadata.is_dir() {
        return Err(read_diagnostic(context.source.root.clone()));
    }
    if source_root != git_root && !source_root.starts_with(&git_root) {
        return Err(Diagnostic::error(OUTSIDE_GIT, OUTSIDE_GIT_MESSAGE).at_path(source_root));
    }

    Ok(source_root)
}

fn has_exact_git_marker(git_root: &Path) -> Result<bool, Diagnostic> {
    let entries =
        fs::read_dir(git_root).map_err(|_| git_root_diagnostic(git_root.to_path_buf()))?;
    for entry in entries {
        let entry = entry.map_err(|_| git_root_diagnostic(git_root.to_path_buf()))?;
        if entry.file_name() != OsStr::new(".git") {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|_| git_root_diagnostic(git_root.to_path_buf()))?;
        return Ok(file_type.is_file() || file_type.is_dir());
    }
    Ok(false)
}

fn walk_source(source_root: &Path) -> Result<WalkedSource, Vec<Diagnostic>> {
    let mut stack = vec![source_root.to_path_buf()];
    let mut domain_indexes = Vec::new();
    let mut markdown = Vec::new();

    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => return Err(vec![read_diagnostic(directory)]),
        };
        let mut entries = match entries.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(_) => return Err(vec![read_diagnostic(directory)]),
        };
        entries.sort_by_key(|entry| entry.file_name());

        let mut child_directories = Vec::new();
        for entry in entries {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => return Err(vec![read_diagnostic(path)]),
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !skip_directory(&entry.file_name()) {
                    child_directories.push(path);
                }
                continue;
            }
            if !file_type.is_file() || path.extension() != Some(OsStr::new("md")) {
                continue;
            }
            if entry.file_name() == OsStr::new(INDEX_FILENAME) {
                if directory != source_root {
                    domain_indexes.push(path);
                }
            } else {
                markdown.push(path);
            }
        }

        child_directories.sort();
        stack.extend(child_directories.into_iter().rev());
    }

    domain_indexes.sort();
    markdown.sort();
    Ok(WalkedSource {
        domain_indexes,
        markdown,
    })
}

fn skip_directory(name: &OsStr) -> bool {
    if name.as_encoded_bytes().first() == Some(&b'.') {
        return true;
    }
    SKIPPED_DIRECTORIES
        .iter()
        .any(|skipped| name == OsStr::new(skipped))
}

fn build_domains(
    source_root: &Path,
    index_paths: &[PathBuf],
) -> Result<Vec<DomainNode>, Vec<Diagnostic>> {
    let index_directories = index_paths
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    let mut domains = Vec::with_capacity(index_paths.len());
    let mut diagnostics = Vec::new();

    for index_path in index_paths {
        let physical_dir = index_path
            .parent()
            .expect("walked INDEX.md has a parent")
            .to_path_buf();
        match derive_domain_id(source_root, &physical_dir, &index_directories) {
            Ok(id) => domains.push(DomainNode {
                id,
                physical_dir,
                index_path: index_path.clone(),
            }),
            Err(diagnostic) => diagnostics.push(diagnostic.at_path(index_path.clone())),
        }
    }
    if !diagnostics.is_empty() {
        sort_diagnostics(source_root, &mut diagnostics);
        return Err(diagnostics);
    }

    domains.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.index_path.cmp(&right.index_path))
    });
    Ok(domains)
}

fn derive_domain_id(
    source_root: &Path,
    physical_dir: &Path,
    index_directories: &BTreeSet<PathBuf>,
) -> Result<DomainId, Diagnostic> {
    let relative = physical_dir
        .strip_prefix(source_root)
        .expect("walked domain directory is inside its source root");
    let mut current = source_root.to_path_buf();
    let mut raw = String::new();

    for component in relative.components() {
        current.push(component.as_os_str());
        if !index_directories.contains(&current) {
            continue;
        }
        let segment = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| invalid_domain_diagnostic())?;
        if !raw.is_empty() {
            raw.push('/');
        }
        raw.push_str(segment);
    }

    DomainId::new(&raw)
}

fn reject_duplicate_domains(
    source_root: &Path,
    domains: &[DomainNode],
) -> Result<(), Vec<Diagnostic>> {
    let mut by_key: BTreeMap<String, Vec<&DomainNode>> = BTreeMap::new();
    for domain in domains {
        by_key
            .entry(casefold_key(domain.id.as_str()))
            .or_default()
            .push(domain);
    }

    let mut diagnostics = Vec::new();
    for mut repeated in by_key.into_values().filter(|nodes| nodes.len() > 1) {
        repeated
            .sort_by_cached_key(|domain| source_relative_sort_key(source_root, &domain.index_path));
        diagnostics.extend(repeated.into_iter().map(|domain| {
            Diagnostic::error(DUPLICATE_DOMAIN, DUPLICATE_DOMAIN_MESSAGE)
                .at_path(domain.index_path.clone())
        }));
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn load_domain_indexes(
    context: &SourceContext,
    domains: &[DomainNode],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SnapshotNote> {
    let mut notes = Vec::with_capacity(domains.len());
    for domain in domains {
        let note = match read_note(&domain.index_path) {
            Ok(note) => note,
            Err(mut failures) => {
                diagnostics.append(&mut failures);
                continue;
            }
        };
        if note.frontmatter.kind != NoteKind::Index {
            diagnostics.push(
                Diagnostic::error(INDEX_KIND, INDEX_KIND_MESSAGE)
                    .at_path(domain.index_path.clone())
                    .for_field("kind"),
            );
            continue;
        }
        let identity = derive_identity(&context.source.name, &domain.id, "INDEX")
            .expect("the fixed INDEX slug is valid");
        notes.push(SnapshotNote {
            locator: NoteLocator {
                identity,
                domain: domain.id.clone(),
                slug: "INDEX".to_owned(),
                path: domain.index_path.clone(),
                is_domain_index: true,
            },
            note,
        });
    }
    notes
}

fn load_ordinary_notes(
    context: &SourceContext,
    source_root: &Path,
    domains: &[DomainNode],
    paths: &[PathBuf],
    notes: &mut Vec<SnapshotNote>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let domains_by_directory = domains
        .iter()
        .map(|domain| (domain.physical_dir.clone(), domain.id.clone()))
        .collect::<BTreeMap<_, _>>();

    for path in paths {
        let Some(domain) = nearest_domain(path, source_root, &domains_by_directory) else {
            continue;
        };
        let slug = match path.file_stem().and_then(OsStr::to_str) {
            Some(slug) => slug,
            None => {
                diagnostics.push(invalid_slug_diagnostic(path.clone()));
                continue;
            }
        };
        let normalized_slug = slug.nfc().collect::<String>();
        let identity = match derive_identity(&context.source.name, domain, &normalized_slug) {
            Ok(identity) => identity,
            Err(diagnostic) => {
                diagnostics.push(diagnostic.at_path(path.clone()));
                continue;
            }
        };
        let note = match read_note(path) {
            Ok(note) => note,
            Err(mut failures) => {
                diagnostics.append(&mut failures);
                continue;
            }
        };

        notes.push(SnapshotNote {
            locator: NoteLocator {
                identity,
                domain: domain.clone(),
                slug: normalized_slug,
                path: path.clone(),
                is_domain_index: false,
            },
            note,
        });
    }
}

fn nearest_domain<'a>(
    note_path: &Path,
    source_root: &Path,
    domains: &'a BTreeMap<PathBuf, DomainId>,
) -> Option<&'a DomainId> {
    let mut directory = note_path.parent()?;
    loop {
        if let Some(domain) = domains.get(directory) {
            return Some(domain);
        }
        if directory == source_root {
            return None;
        }
        directory = directory.parent()?;
    }
}

fn read_note(path: &Path) -> Result<ValidatedNote, Vec<Diagnostic>> {
    let bytes = fs::read(path).map_err(|_| vec![read_diagnostic(path.to_path_buf())])?;
    parse_and_validate_note(path, &bytes)
}

fn reject_identity_collisions(
    source_root: &Path,
    notes: &[SnapshotNote],
) -> Result<(), Vec<Diagnostic>> {
    let mut by_key: BTreeMap<String, Vec<&Path>> = BTreeMap::new();
    for note in notes {
        by_key
            .entry(casefold_key(note.locator.identity.as_str()))
            .or_default()
            .push(&note.locator.path);
    }

    let mut diagnostics = Vec::new();
    for mut repeated in by_key.into_values().filter(|paths| paths.len() > 1) {
        repeated.sort_by_cached_key(|path| source_relative_sort_key(source_root, path));
        diagnostics.extend(repeated.into_iter().map(|path| {
            Diagnostic::error(IDENTITY_COLLISION, IDENTITY_COLLISION_MESSAGE)
                .at_path(path.to_path_buf())
        }));
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn casefold_key(value: &str) -> String {
    value.case_fold().collect()
}

fn source_relative_sort_key(source_root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(source_root).unwrap_or(path);
    let mut key = String::new();
    for component in relative.components() {
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(&component.as_os_str().to_string_lossy());
    }
    key
}

fn sort_diagnostics(source_root: &Path, diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by_cached_key(|diagnostic| {
        (
            diagnostic
                .path
                .as_deref()
                .map(|path| source_relative_sort_key(source_root, path)),
            diagnostic.code,
            diagnostic.field.clone(),
            diagnostic.message.clone(),
        )
    });
}

fn git_root_diagnostic(path: PathBuf) -> Diagnostic {
    Diagnostic::error(GIT_ROOT, GIT_ROOT_MESSAGE).at_path(path)
}

fn read_diagnostic(path: PathBuf) -> Diagnostic {
    Diagnostic::error(READ, READ_MESSAGE).at_path(path)
}

fn invalid_domain_diagnostic() -> Diagnostic {
    Diagnostic::error(
        "KBV2-DOMAIN-INVALID",
        "domain must contain only safe non-empty path segments",
    )
    .for_field("domain")
}

fn invalid_slug_diagnostic(path: PathBuf) -> Diagnostic {
    Diagnostic::error(
        "KBV2-IDENTITY-INVALID-SLUG",
        "note slug must be one safe non-empty path segment",
    )
    .at_path(path)
    .for_field("slug")
}
