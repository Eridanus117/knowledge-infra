use crate::git::GitBackend;
use crate::human_index::check_human_index;
use crate::source::{
    SourceContext, SourceSnapshot, create_directory_tree_nofollow, create_regular_file_nofollow,
    discover_source, read_regular_file_nofollow_bounded, remove_file_nofollow,
};
use kb_contract::{
    Diagnostic, NoteFrontmatter, NoteKind, RegistryLocator, SourceName, SourceSpec, Surface,
    parse_and_validate_note, render_note, resolve_registry,
};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const INVALID_SOURCE: &str = "KBV2-ADOPT-SOURCE";
const INVALID_SOURCE_MESSAGE: &str = "logical source must match ^[a-z][a-z0-9-]*$";
const WORKTREE: &str = "KBV2-ADOPT-WORKTREE";
const WORKTREE_MESSAGE: &str = "adoption does not support Git worktrees";
const SOURCE_CONFLICT: &str = "KBV2-ADOPT-SOURCE-CONFLICT";
const SOURCE_CONFLICT_MESSAGE: &str = "logical source is already registered at another repository";
const REGISTRY_IO: &str = "KBV2-ADOPT-REGISTRY";
const REGISTRY_IO_MESSAGE: &str = "source registry could not be read or updated";
const REPO: &str = "KBV2-ADOPT-REPOSITORY";
const PATH_INVALID: &str = "KBV2-ADOPT-PATH";
const PATH_INVALID_MESSAGE: &str = "adoption path contains unsupported control characters";
const REPO_MESSAGE: &str = "adoption repository must be an existing Git directory";
const GATE_INVALID: &str = "KBV2-ADOPT-GATE";
const GATE_INVALID_MESSAGE: &str = "lefthook adoption gate is missing or invalid";

/// Input to an idempotent repository adoption plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptRequest {
    pub registry: PathBuf,
    pub logical_source: String,
    pub repo: PathBuf,
    pub description: String,
    pub keywords: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptPlan {
    registry: PathBuf,
    registry_before: Vec<u8>,
    overlay_path: PathBuf,
    overlay_before: Option<Vec<u8>>,
    registry_after: Option<Vec<u8>>,
    repo: PathBuf,
    human_index: Option<(PathBuf, Vec<u8>)>,
    index: Option<(PathBuf, Vec<u8>)>,
    gate: Option<(PathBuf, Vec<u8>)>,
}

impl AdoptPlan {
    #[must_use]
    pub fn registry(&self) -> &Path {
        &self.registry
    }
    #[must_use]
    pub fn repo(&self) -> &Path {
        &self.repo
    }
    #[must_use]
    pub fn changes_registry(&self) -> bool {
        self.registry_after.is_some()
    }
    #[must_use]
    pub fn creates_index(&self) -> bool {
        self.index.is_some()
    }
    #[must_use]
    pub fn installs_gate(&self) -> bool {
        self.gate.is_some()
    }
}

#[derive(Debug)]
pub enum AdoptError {
    Diagnostics(Vec<Diagnostic>),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
impl fmt::Display for AdoptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostics(diagnostics) => match diagnostics.first() {
                Some(diagnostic) => formatter.write_str(&diagnostic.message),
                None => formatter.write_str("adoption failed"),
            },
            Self::Io { path, .. } => {
                write!(formatter, "could not adopt repository {}", path.display())
            }
        }
    }
}
impl AdoptError {
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        match self {
            Self::Diagnostics(diagnostics) => diagnostics,
            Self::Io { path, .. } => {
                vec![Diagnostic::error("KBV2-ADOPT-IO", "could not adopt repository").at_path(path)]
            }
        }
    }
}
impl std::error::Error for AdoptError {}

/// Build an adoption plan without running Git tools or writing any bytes.
pub fn plan_adopt(request: &AdoptRequest) -> Result<AdoptPlan, AdoptError> {
    if !valid_source_name(&request.logical_source) {
        return Err(diag(INVALID_SOURCE, INVALID_SOURCE_MESSAGE, "source"));
    }
    let repo = fs::canonicalize(&request.repo).map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
    if !repo.is_dir() {
        return Err(diag(REPO, REPO_MESSAGE, "repo"));
    }
    GitBackend::new(&repo).map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
    let marker =
        fs::symlink_metadata(repo.join(".git")).map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
    if marker.file_type().is_file() {
        return Err(diag(WORKTREE, WORKTREE_MESSAGE, "repo"));
    }
    if !marker.is_dir() {
        return Err(diag(REPO, REPO_MESSAGE, "repo"));
    }

    let registry_path = if request.registry.is_absolute() {
        request.registry.clone()
    } else {
        std::path::absolute(&request.registry).map_err(|source| AdoptError::Io {
            path: request.registry.clone(),
            source,
        })?
    };
    if registry_path
        .to_string_lossy()
        .chars()
        .any(char::is_control)
        || repo.to_string_lossy().chars().any(char::is_control)
    {
        return Err(diag(PATH_INVALID, PATH_INVALID_MESSAGE, "path"));
    }
    let registry_before = read_registry(&registry_path).map_err(|source| AdoptError::Io {
        path: registry_path.clone(),
        source,
    })?;
    let overlay_path = registry_path.with_file_name(format!(
        "{}.local.toml",
        registry_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("sources")
    ));
    let overlay_before = match read_registry(&overlay_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(AdoptError::Io {
                path: overlay_path.clone(),
                source,
            });
        }
    };
    let locator = RegistryLocator {
        explicit: Some(registry_path.clone()),
        cwd: registry_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: registry_path.with_file_name("unused-user-registry.toml"),
    };
    let registry = resolve_registry(&locator).map_err(AdoptError::Diagnostics)?;
    let mut registry_after = None;
    if registry.sources.values().any(|existing| {
        existing.name.as_str() != request.logical_source
            && fs::canonicalize(&existing.root).ok().as_deref() == Some(repo.as_path())
    }) {
        return Err(diag(SOURCE_CONFLICT, SOURCE_CONFLICT_MESSAGE, "source"));
    }
    if let Some(existing) = registry.sources.get(request.logical_source.as_str()) {
        if fs::canonicalize(&existing.root).ok().as_deref() != Some(repo.as_path()) {
            return Err(diag(SOURCE_CONFLICT, SOURCE_CONFLICT_MESSAGE, "source"));
        }
    } else {
        let repo_text = repo.to_string_lossy();
        let row = format!(
            "\n[[source]]\nname = \"{}\"\npath = \"{}\"\nsurface = \"core\"\n",
            request.logical_source,
            toml_quote(&repo_text),
        );
        let mut bytes = registry_before.clone();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(row.trim_start_matches('\n').as_bytes());
        let valid_toml = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .is_some();
        if !valid_toml {
            return Err(diag(REGISTRY_IO, REGISTRY_IO_MESSAGE, "registry"));
        }
        registry_after = Some(bytes);
    }

    let discovery_name = SourceName::new(&request.logical_source)
        .map_err(|diagnostic| AdoptError::Diagnostics(vec![diagnostic]))?;
    let discovery_context = SourceContext {
        source: SourceSpec {
            name: discovery_name,
            root: repo.clone(),
            surface: Surface::Core,
        },
        git_root: repo.clone(),
        registry_origin: registry_path.clone(),
    };
    let discovered = discover_source(&discovery_context).map_err(AdoptError::Diagnostics)?;
    let has_domain = !discovered.domains.is_empty();

    let docs_path = repo.join("docs");
    if let Ok(metadata) = fs::symlink_metadata(&docs_path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(diag(REPO, REPO_MESSAGE, "repo"));
        }
    }
    let index_path = docs_path.join("INDEX.md");
    let index = if exact_entry(&docs_path, "INDEX.md") {
        let metadata =
            fs::symlink_metadata(&index_path).map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(diag(REPO, REPO_MESSAGE, "repo"));
        }
        let bytes = read_regular_file_nofollow_bounded(&index_path, 64 * 1024 * 1024)
            .map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
        let note = parse_and_validate_note(&index_path, &bytes).map_err(AdoptError::Diagnostics)?;
        if note.frontmatter.kind != NoteKind::Index {
            return Err(diag(REPO, REPO_MESSAGE, "repo"));
        }
        None
    } else if has_domain {
        None
    } else {
        let frontmatter = NoteFrontmatter {
            description: request.description.clone(),
            keywords: request.keywords.clone(),
            kind: NoteKind::Index,
            links: Vec::new(),
            code: Vec::new(),
            assets: Vec::new(),
            supersedes: None,
            status: None,
        };
        let bytes = render_note(&frontmatter, b"# Docs\n");
        parse_and_validate_note(&index_path, &bytes).map_err(AdoptError::Diagnostics)?;
        Some((index_path, bytes))
    };
    let human_index_path = repo.join("INDEX.md");
    let human_index = if exact_entry(&repo, "INDEX.md") {
        let metadata = fs::symlink_metadata(&human_index_path)
            .map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(diag(REPO, REPO_MESSAGE, "repo"));
        }
        let findings = check_human_index(&discovered, &human_index_path)
            .map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
        if !findings.is_empty() {
            return Err(AdoptError::Diagnostics(findings));
        }
        None
    } else {
        Some((
            human_index_path,
            render_human_index(&discovered, &repo).into_bytes(),
        ))
    };
    let gate_path = repo.join("lefthook.yml");
    let gate = if exact_entry(&repo, "lefthook.yml") {
        let metadata = fs::symlink_metadata(&gate_path)
            .map_err(|_| diag(GATE_INVALID, GATE_INVALID_MESSAGE, "lefthook"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(diag(GATE_INVALID, GATE_INVALID_MESSAGE, "lefthook"));
        }
        let gate_valid = read_regular_file_nofollow_bounded(&gate_path, 1024 * 1024)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .is_some_and(|text| active_gate(&text, &registry_path));
        if !gate_valid {
            return Err(diag(GATE_INVALID, GATE_INVALID_MESSAGE, "lefthook"));
        }
        None
    } else {
        Some((gate_path, gate_file(&registry_path).into_bytes()))
    };
    Ok(AdoptPlan {
        registry: registry_path,
        registry_before,
        overlay_path,
        overlay_before,
        registry_after,
        repo,
        human_index,
        index,
        gate,
    })
}

pub fn apply_adopt(plan: &AdoptPlan) -> Result<(), AdoptError> {
    let current_registry = read_registry(&plan.registry).map_err(|source| AdoptError::Io {
        path: plan.registry.clone(),
        source,
    })?;
    if current_registry != plan.registry_before {
        return Err(diag(REGISTRY_IO, REGISTRY_IO_MESSAGE, "registry"));
    }
    let overlay_current = match read_registry(&plan.overlay_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(AdoptError::Io {
                path: plan.overlay_path.clone(),
                source,
            });
        }
    };
    if overlay_current != plan.overlay_before {
        return Err(diag(REGISTRY_IO, REGISTRY_IO_MESSAGE, "overlay"));
    }

    let mut registry_changed = false;
    let mut human_index_created = false;
    let mut index_created = false;
    let gate_created = false;
    let mut created_dirs = Vec::new();
    if let Some(bytes) = &plan.registry_after {
        registry_changed = true;
        if let Err(error) = write_registry(&plan.registry, bytes) {
            if !rollback_adopt(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
            ) {
                return Err(diag(GATE_INVALID, "adoption rollback failed", "rollback"));
            }
            return Err(error);
        }
    }
    if let Some((path, bytes)) = &plan.human_index {
        if let Err(error) = create_new(path, bytes) {
            if !rollback_adopt(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
            ) {
                return Err(diag(GATE_INVALID, "adoption rollback failed", "rollback"));
            }
            return Err(error);
        }
        human_index_created = true;
    }
    if let Some((path, bytes)) = &plan.index {
        match create_parent_dirs(path) {
            Ok(mut dirs) => created_dirs.append(&mut dirs),
            Err(error) => {
                if !rollback_adopt(
                    plan,
                    &created_dirs,
                    registry_changed,
                    human_index_created,
                    index_created,
                    gate_created,
                ) {
                    return Err(diag(GATE_INVALID, "adoption rollback failed", "rollback"));
                }
                return Err(error);
            }
        }
        if let Err(error) = create_new(path, bytes) {
            if !rollback_adopt(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
            ) {
                return Err(diag(GATE_INVALID, "adoption rollback failed", "rollback"));
            }
            return Err(error);
        }
        index_created = true;
    }
    if let Some((path, bytes)) = &plan.gate {
        if let Err(error) = create_new(path, bytes) {
            if !rollback_adopt(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
            ) {
                return Err(diag(GATE_INVALID, "adoption rollback failed", "rollback"));
            }
            return Err(error);
        }
    }
    Ok(())
}
fn create_new(path: &Path, bytes: &[u8]) -> Result<(), AdoptError> {
    let mut file = create_regular_file_nofollow(path).map_err(|source| AdoptError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Err(source) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        if !remove_if_created(path) {
            return Err(diag(GATE_INVALID, "adoption rollback failed", "rollback"));
        }
        return Err(AdoptError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn rollback_adopt(
    plan: &AdoptPlan,
    created_dirs: &[PathBuf],
    registry_changed: bool,
    human_index_created: bool,
    index_created: bool,
    gate_created: bool,
) -> bool {
    let mut ok = true;
    if gate_created {
        if let Some((path, _)) = &plan.gate {
            ok &= remove_if_created(path);
        }
    }
    if index_created {
        if let Some((path, _)) = &plan.index {
            ok &= remove_if_created(path);
        }
    }
    if human_index_created {
        if let Some((path, _)) = &plan.human_index {
            ok &= remove_if_created(path);
        }
    }
    for path in created_dirs.iter().rev() {
        match fs::remove_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => ok = false,
        }
    }
    if registry_changed {
        ok &= write_registry(&plan.registry, &plan.registry_before).is_ok();
    }
    ok
}

fn remove_if_created(path: &Path) -> bool {
    match remove_file_nofollow(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}
fn render_human_index(snapshot: &SourceSnapshot, root: &Path) -> String {
    let mut output = String::from("<!-- rhizome:generated-index:start -->\n");
    for domain in &snapshot.domains {
        let notes = snapshot
            .notes
            .iter()
            .filter(|note| note.locator.domain == domain.id && !note.locator.is_domain_index)
            .collect::<Vec<_>>();
        if notes.is_empty() {
            continue;
        }
        output.push_str("### ");
        output.push_str(domain.id.as_str());
        output.push('\n');
        for note in notes {
            let relative = note
                .locator
                .path
                .strip_prefix(root)
                .unwrap_or(&note.locator.path)
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let _ = std::fmt::Write::write_fmt(
                &mut output,
                format_args!(
                    "- [{}]({}) — {}\n",
                    note.locator.identity, relative, note.note.frontmatter.description
                ),
            );
        }
    }
    output.push_str("<!-- rhizome:generated-index:end -->");
    output
}
fn exact_entry(parent: &Path, expected: &str) -> bool {
    fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name() == OsStr::new(expected))
}
fn gate_file(registry: &Path) -> String {
    let path = registry.to_string_lossy().into_owned();
    format!(
        "pre-commit:\n  commands:\n    rhizome-check:\n      run: rhizome check --registry {} -- {{staged_files}}\n",
        shell_quote(&path)
    )
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn active_gate(text: &str, registry: &Path) -> bool {
    let expected = format!(
        "run: rhizome check --registry {} -- {{staged_files}}",
        shell_quote(&registry.to_string_lossy())
    );
    let mut pre_commit = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            pre_commit = trimmed == "pre-commit:";
            continue;
        }
        if pre_commit && trimmed == expected {
            return true;
        }
    }
    false
}

fn write_registry(path: &Path, bytes: &[u8]) -> Result<(), AdoptError> {
    let mut options = OpenOptions::new();
    options.write(true).truncate(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let mut file = options.open(path).map_err(|source| AdoptError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !file
        .metadata()
        .map_err(|source| AdoptError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .is_file()
    {
        return Err(AdoptError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::InvalidData),
        });
    }
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| AdoptError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn create_parent_dirs(path: &Path) -> Result<Vec<PathBuf>, AdoptError> {
    let mut created = Vec::new();
    if let Some(parent) = path.parent() {
        let existed = parent
            .parent()
            .map(|base| {
                exact_entry(
                    base,
                    parent
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                )
            })
            .unwrap_or(true);
        create_directory_tree_nofollow(parent).map_err(|source| AdoptError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        if !existed {
            created.push(parent.to_path_buf());
        }
    }
    Ok(created)
}

fn valid_source_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
fn read_registry(path: &Path) -> std::io::Result<Vec<u8>> {
    const MAX_REGISTRY_BYTES: usize = 16 * 1024 * 1024;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_REGISTRY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}
fn toml_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
fn diag(code: &'static str, message: &'static str, field: &'static str) -> AdoptError {
    AdoptError::Diagnostics(vec![Diagnostic::error(code, message).for_field(field)])
}
