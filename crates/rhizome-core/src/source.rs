use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, File, OpenOptions};
use focaccia::CaseFold;
use kb_contract::{
    Diagnostic, DomainId, Identity, NoteKind, SourceName, SourceSpec, ValidatedNote,
    derive_identity, parse_and_validate_note,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
#[cfg(windows)]
use std::path::Prefix;
use std::path::{Component, Path, PathBuf};
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
    pub(crate) source_root: PathBuf,
}

struct OpenedSource {
    root: PathBuf,
    directory: Dir,
    _git_root: Dir,
}

struct WalkedDirectory {
    relative_path: PathBuf,
    nearest_domain: Option<DomainId>,
    domain_lineage_valid: bool,
}

struct WalkedSource {
    domains: Vec<DomainNode>,
    notes: Vec<SnapshotNote>,
    diagnostics: Vec<Diagnostic>,
}

struct WalkedEntry {
    file_name: OsString,
    is_symlink: bool,
    is_dir: bool,
    is_file: bool,
}

/// Discover and validate every domain and in-domain Markdown note in a source.
pub fn discover_source(context: &SourceContext) -> Result<SourceSnapshot, Vec<Diagnostic>> {
    let opened = validate_roots(context).map_err(|diagnostic| vec![diagnostic])?;
    let source_root = opened.root;
    let walked = walk_source(&context.source.name, &source_root, &opened.directory)?;

    let mut domains = walked.domains;
    domains.sort_by_cached_key(|domain| {
        (
            domain.id.clone(),
            source_relative_sort_key(&source_root, &domain.index_path),
        )
    });
    reject_duplicate_domains(&source_root, &domains)?;

    let mut diagnostics = walked.diagnostics;
    if !diagnostics.is_empty() {
        sort_diagnostics(&source_root, &mut diagnostics);
        return Err(diagnostics);
    }

    let mut notes = walked.notes;
    reject_identity_collisions(&source_root, &notes)?;
    notes.sort_by_cached_key(|note| {
        (
            note.locator.domain.clone(),
            source_relative_sort_key(&source_root, &note.locator.path),
        )
    });
    Ok(SourceSnapshot {
        source: context.source.name.clone(),
        domains,
        notes,
        source_root,
    })
}

fn validate_roots(context: &SourceContext) -> Result<OpenedSource, Diagnostic> {
    // Canonicalization is used only to resolve the configured roots. All subsequent
    // authority comes from the retained handles and handle-relative nofollow opens.
    let git_root = fs::canonicalize(&context.git_root)
        .map_err(|_| git_root_diagnostic(context.git_root.clone()))?;
    let source_root = fs::canonicalize(&context.source.root)
        .map_err(|_| read_diagnostic(context.source.root.clone()))?;

    let git_directory = open_absolute_dir_nofollow(&git_root)
        .map_err(|_| git_root_diagnostic(context.git_root.clone()))?;
    let git_metadata = git_directory
        .dir_metadata()
        .map_err(|_| git_root_diagnostic(context.git_root.clone()))?;
    if !git_metadata.is_dir()
        || !has_exact_git_marker(&git_directory)
            .map_err(|_| git_root_diagnostic(context.git_root.clone()))?
    {
        return Err(git_root_diagnostic(context.git_root.clone()));
    }

    if source_root != git_root && !source_root.starts_with(&git_root) {
        return Err(Diagnostic::error(OUTSIDE_GIT, OUTSIDE_GIT_MESSAGE).at_path(source_root));
    }

    let relative_source = source_root
        .strip_prefix(&git_root)
        .expect("contained canonical source root is relative to canonical Git root");
    let source_directory = open_relative_directory(&git_directory, relative_source)
        .map_err(|_| read_diagnostic(context.source.root.clone()))?;

    Ok(OpenedSource {
        root: source_root,
        directory: source_directory,
        _git_root: git_directory,
    })
}

#[cfg(unix)]
pub(crate) fn open_absolute_dir_nofollow(path: &Path) -> io::Result<Dir> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let root = Dir::open_ambient_dir(Path::new("/"), ambient_authority())?;
    open_relative_directory(&root, components.as_path())
}

#[cfg(windows)]
pub(crate) fn open_absolute_dir_nofollow(path: &Path) -> io::Result<Dir> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        || !matches!(components.next(), Some(Component::RootDir))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }

    let mut anchor = PathBuf::from(prefix.as_os_str());
    anchor.push(Path::new(r"\"));
    let root = Dir::open_ambient_dir(anchor, ambient_authority())?;
    open_relative_directory(&root, components.as_path())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_absolute_dir_nofollow(_path: &Path) -> io::Result<Dir> {
    Err(io::ErrorKind::Unsupported.into())
}

fn open_relative_directory(root: &Dir, relative: &Path) -> io::Result<Dir> {
    let mut directory = root.try_clone()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        directory = open_directory_nofollow(&directory, name)?;
    }
    Ok(directory)
}

fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    // cap-std opens Windows directory handles without FILE_SHARE_DELETE; keeping
    // that retained handle while enumerating prevents rename/delete races.
    let directory = parent.open_dir_nofollow(Path::new(name))?;
    if directory.dir_metadata()?.is_dir() {
        Ok(directory)
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

fn open_regular_file_nofollow(parent: &Dir, name: &OsStr) -> io::Result<(File, u64)> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);

    let file = parent.open_with(Path::new(name), &options)?;
    let metadata = file.metadata()?;
    if metadata.is_file() {
        Ok((file, metadata.len()))
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

pub(crate) fn open_regular_file_for_update_nofollow(
    parent: &Dir,
    name: &OsStr,
) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);

    let file = parent.open_with(Path::new(name), &options)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

fn has_exact_git_marker(git_root: &Dir) -> io::Result<bool> {
    for entry in git_root.entries()? {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name != OsStr::new(".git") {
            continue;
        }

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            return Ok(open_directory_nofollow(git_root, &file_name).is_ok());
        }
        if file_type.is_file() {
            return Ok(open_regular_file_nofollow(git_root, &file_name).is_ok());
        }
        return Ok(false);
    }
    Ok(false)
}

fn walk_source(
    source_name: &SourceName,
    source_root: &Path,
    source_directory: &Dir,
) -> Result<WalkedSource, Vec<Diagnostic>> {
    let mut stack = vec![WalkedDirectory {
        relative_path: PathBuf::new(),
        nearest_domain: None,
        domain_lineage_valid: true,
    }];
    let mut domains = Vec::new();
    let mut notes = Vec::new();
    let mut diagnostics = Vec::new();

    while let Some(directory) = stack.pop() {
        // Reopen only relative to the retained source capability. The handle is
        // dropped at the end of this iteration, so directory handles stay bounded.
        let directory_handle = open_relative_directory(source_directory, &directory.relative_path)
            .map_err(|_| vec![read_diagnostic(source_root.join(&directory.relative_path))])?;
        let directory_path = source_root.join(&directory.relative_path);
        let entries = directory_handle
            .entries()
            .map_err(|_| vec![read_diagnostic(directory_path.clone())])?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| vec![read_diagnostic(directory_path.clone())])?;

        let mut entries = entries
            .into_iter()
            .map(|entry| {
                let file_name = entry.file_name();
                entry.file_type().map(|file_type| (file_name, file_type))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| vec![read_diagnostic(directory_path.clone())])?;
        entries.sort_by_cached_key(|(file_name, _)| {
            relative_posix_sort_key(&directory.relative_path.join(file_name))
        });

        let entries = entries
            .into_iter()
            .map(|(file_name, file_type)| WalkedEntry {
                file_name,
                is_symlink: file_type.is_symlink(),
                is_dir: file_type.is_dir(),
                is_file: file_type.is_file(),
            })
            .collect::<Vec<_>>();

        let has_index = !directory.relative_path.as_os_str().is_empty()
            && entries.iter().any(|entry| {
                !entry.is_symlink && entry.is_file && entry.file_name == OsStr::new(INDEX_FILENAME)
            });
        let current_domain = if has_index {
            let index_path = directory_path.join(INDEX_FILENAME);
            if !directory.domain_lineage_valid {
                diagnostics.push(invalid_domain_diagnostic().at_path(index_path));
                None
            } else {
                let Some(segment) = directory.relative_path.file_name().and_then(OsStr::to_str)
                else {
                    diagnostics.push(invalid_domain_diagnostic().at_path(index_path));
                    return Err(diagnostics);
                };
                let raw = match &directory.nearest_domain {
                    Some(parent) => format!("{parent}/{segment}"),
                    None => segment.to_owned(),
                };
                match DomainId::new(&raw) {
                    Ok(id) => {
                        domains.push(DomainNode {
                            id: id.clone(),
                            physical_dir: directory_path.clone(),
                            index_path,
                        });
                        Some(id)
                    }
                    Err(diagnostic) => {
                        diagnostics.push(diagnostic.at_path(index_path));
                        None
                    }
                }
            }
        } else if directory.domain_lineage_valid {
            directory.nearest_domain.clone()
        } else {
            None
        };
        let current_lineage_valid =
            directory.domain_lineage_valid && (!has_index || current_domain.is_some());

        let mut child_directories = Vec::new();
        for entry in entries {
            let relative_path = directory.relative_path.join(&entry.file_name);
            let path = source_root.join(&relative_path);
            if entry.is_symlink {
                continue;
            }
            if entry.is_dir {
                if !skip_directory(&entry.file_name) {
                    child_directories.push(relative_path);
                }
                continue;
            }
            if !entry.is_file || Path::new(&entry.file_name).extension() != Some(OsStr::new("md")) {
                continue;
            }
            if entry.file_name == OsStr::new(INDEX_FILENAME) {
                if current_domain.is_none() {
                    continue;
                }
                let (file, expected_len) =
                    open_regular_file_nofollow(&directory_handle, &entry.file_name)
                        .map_err(|_| vec![read_diagnostic(path.clone())])?;
                let note = match read_note(&path, file, expected_len) {
                    Ok(note) => note,
                    Err(mut failures) => {
                        diagnostics.append(&mut failures);
                        continue;
                    }
                };
                if note.frontmatter.kind != NoteKind::Index {
                    diagnostics.push(
                        Diagnostic::error(INDEX_KIND, INDEX_KIND_MESSAGE)
                            .at_path(path.clone())
                            .for_field("kind"),
                    );
                    continue;
                }
                let domain = current_domain
                    .as_ref()
                    .expect("an exact non-root INDEX.md has a domain");
                let identity = match derive_identity(source_name, domain, "INDEX") {
                    Ok(identity) => identity,
                    Err(diagnostic) => {
                        diagnostics.push(diagnostic.at_path(path));
                        continue;
                    }
                };
                notes.push(SnapshotNote {
                    locator: NoteLocator {
                        identity,
                        domain: domain.clone(),
                        slug: "INDEX".to_owned(),
                        path,
                        is_domain_index: true,
                    },
                    note,
                });
                continue;
            }
            let Some(domain) = current_domain.as_ref() else {
                continue;
            };
            let slug = match Path::new(&entry.file_name)
                .file_stem()
                .and_then(OsStr::to_str)
            {
                Some(slug) => slug,
                None => {
                    diagnostics.push(invalid_slug_diagnostic(path));
                    continue;
                }
            };
            let normalized_slug = slug.nfc().collect::<String>();
            let identity = match derive_identity(source_name, domain, &normalized_slug) {
                Ok(identity) => identity,
                Err(diagnostic) => {
                    diagnostics.push(diagnostic.at_path(path));
                    continue;
                }
            };
            let (file, expected_len) =
                open_regular_file_nofollow(&directory_handle, &entry.file_name)
                    .map_err(|_| vec![read_diagnostic(path.clone())])?;
            let note = match read_note(&path, file, expected_len) {
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
                    path,
                    is_domain_index: false,
                },
                note,
            });
        }

        child_directories.sort_by_cached_key(|path| relative_posix_sort_key(path));
        stack.extend(
            child_directories
                .into_iter()
                .rev()
                .map(|relative_path| WalkedDirectory {
                    relative_path,
                    nearest_domain: current_domain.clone(),
                    domain_lineage_valid: current_lineage_valid,
                }),
        );
    }

    Ok(WalkedSource {
        domains,
        notes,
        diagnostics,
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

fn reject_duplicate_domains(
    source_root: &Path,
    domains: &[DomainNode],
) -> Result<(), Vec<Diagnostic>> {
    let mut ordered = domains
        .iter()
        .map(|domain| {
            (
                source_relative_sort_key(source_root, &domain.index_path),
                domain,
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|(left_path, left), (right_path, right)| {
        CaseFold::Full
            .casecmp(left.id.as_str(), right.id.as_str())
            .then_with(|| left_path.cmp(right_path))
    });

    let mut diagnostics = Vec::new();
    let mut start = 0;
    while start < ordered.len() {
        let mut end = start + 1;
        while end < ordered.len()
            && CaseFold::Full.case_eq(ordered[start].1.id.as_str(), ordered[end].1.id.as_str())
        {
            end += 1;
        }
        if end - start > 1 {
            diagnostics.extend(ordered[start..end].iter().map(|(_, domain)| {
                Diagnostic::error(DUPLICATE_DOMAIN, DUPLICATE_DOMAIN_MESSAGE)
                    .at_path(domain.index_path.clone())
            }));
        }
        start = end;
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn reject_identity_collisions(
    source_root: &Path,
    notes: &[SnapshotNote],
) -> Result<(), Vec<Diagnostic>> {
    let mut ordered = notes
        .iter()
        .map(|note| {
            (
                source_relative_sort_key(source_root, &note.locator.path),
                note,
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|(left_path, left), (right_path, right)| {
        CaseFold::Full
            .casecmp(
                left.locator.identity.as_str(),
                right.locator.identity.as_str(),
            )
            .then_with(|| left_path.cmp(right_path))
    });

    let mut diagnostics = Vec::new();
    let mut start = 0;
    while start < ordered.len() {
        let mut end = start + 1;
        while end < ordered.len()
            && CaseFold::Full.case_eq(
                ordered[start].1.locator.identity.as_str(),
                ordered[end].1.locator.identity.as_str(),
            )
        {
            end += 1;
        }
        if end - start > 1 {
            diagnostics.extend(ordered[start..end].iter().map(|(_, note)| {
                Diagnostic::error(IDENTITY_COLLISION, IDENTITY_COLLISION_MESSAGE)
                    .at_path(note.locator.path.clone())
            }));
        }
        start = end;
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn read_note(path: &Path, file: File, expected_len: u64) -> Result<ValidatedNote, Vec<Diagnostic>> {
    let limit = expected_len
        .checked_add(1)
        .ok_or_else(|| vec![read_diagnostic(path.to_path_buf())])?;
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| vec![read_diagnostic(path.to_path_buf())])?;
    if u64::try_from(bytes.len()).ok() != Some(expected_len) {
        return Err(vec![read_diagnostic(path.to_path_buf())]);
    }
    parse_and_validate_note(path, &bytes)
}

fn relative_posix_sort_key(path: &Path) -> String {
    let mut key = String::new();
    for component in path.components() {
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(&component.as_os_str().to_string_lossy());
    }
    key
}

fn source_relative_sort_key(source_root: &Path, path: &Path) -> String {
    relative_posix_sort_key(path.strip_prefix(source_root).unwrap_or(path))
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
