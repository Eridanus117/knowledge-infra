use crate::git::GitBackend;
use crate::human_index::check_human_index;
use crate::source::{
    SourceContext, SourceSnapshot, create_directory_tree_nofollow, create_regular_file_nofollow,
    discover_source, read_regular_file_nofollow_bounded, remove_file_nofollow,
    write_regular_file_nofollow,
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

const MAX_REGISTRY_BYTES: usize = 16 * 1024 * 1024;
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
    if registry_path.to_str().is_none()
        || repo.to_str().is_none()
        || repo
            .to_str()
            .is_some_and(|path| path.chars().any(char::is_control))
        || !hook_path_safe(&registry_path)
    {
        return Err(diag(PATH_INVALID, PATH_INVALID_MESSAGE, "path"));
    }
    if fs::symlink_metadata(&registry_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(diag(REGISTRY_IO, REGISTRY_IO_MESSAGE, "registry"));
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
            && git_repository_root(&existing.root).as_deref() == Some(repo.as_path())
    }) {
        return Err(diag(SOURCE_CONFLICT, SOURCE_CONFLICT_MESSAGE, "source"));
    }
    if let Some(existing) = registry.sources.get(request.logical_source.as_str()) {
        if existing.root != repo {
            return Err(diag(SOURCE_CONFLICT, SOURCE_CONFLICT_MESSAGE, "source"));
        }
    } else {
        let repo_text = repo
            .to_str()
            .ok_or_else(|| diag(PATH_INVALID, PATH_INVALID_MESSAGE, "path"))?;
        let row = format!(
            "\n[[source]]\nname = \"{}\"\npath = \"{}\"\nsurface = \"core\"\n",
            request.logical_source,
            toml_quote(repo_text),
        );
        let mut bytes = registry_before.clone();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(row.trim_start_matches('\n').as_bytes());
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err(diag(REGISTRY_IO, REGISTRY_IO_MESSAGE, "registry"));
        }
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
    let index = if discovered.domains.is_empty() {
        // A source with no discovered C2 domain gets the approved starter domain.
        // Existing domains are authoritative; never infer a physical path from a
        // logical domain string or require a conventional `docs` directory.
        let docs_path = discovered.source_root.join("docs");
        if let Ok(metadata) = fs::symlink_metadata(&docs_path) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(diag(REPO, REPO_MESSAGE, "repo"));
            }
        }
        let index_path = docs_path.join("INDEX.md");
        if exact_entry(&docs_path, "INDEX.md") {
            let metadata =
                fs::symlink_metadata(&index_path).map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(diag(REPO, REPO_MESSAGE, "repo"));
            }
            let bytes = read_regular_file_nofollow_bounded(&index_path, 64 * 1024 * 1024)
                .map_err(|_| diag(REPO, REPO_MESSAGE, "repo"))?;
            let note =
                parse_and_validate_note(&index_path, &bytes).map_err(AdoptError::Diagnostics)?;
            if note.frontmatter.kind != NoteKind::Index {
                return Err(diag(REPO, REPO_MESSAGE, "repo"));
            }
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
        }
    } else {
        None
    };
    let human_index_path = discovered.source_root.join("INDEX.md");
    let human_index = if exact_entry(&discovered.source_root, "INDEX.md") {
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
            render_human_index(&discovered).into_bytes(),
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
    apply_adopt_with_hook(plan, |_| Ok(()))
}

pub fn apply_adopt_with_hook<F>(plan: &AdoptPlan, hook: F) -> Result<(), AdoptError>
where
    F: FnOnce(&Path) -> Result<(), AdoptError>,
{
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
    let mut gate_created = false;
    let mut created_dirs = Vec::new();
    if let Some(bytes) = &plan.registry_after {
        registry_changed = true;
        if let Err(error) = write_registry(&plan.registry, bytes) {
            return Err(rollback_or_preserve(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
                error,
            ));
        }
    }
    if let Some((path, bytes)) = &plan.human_index {
        if let Err(error) = create_new(path, bytes) {
            return Err(rollback_or_preserve(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
                error,
            ));
        }
        human_index_created = true;
    }
    if let Some((path, bytes)) = &plan.index {
        match create_parent_dirs(path) {
            Ok(mut dirs) => created_dirs.append(&mut dirs),
            Err(error) => {
                return Err(rollback_or_preserve(
                    plan,
                    &created_dirs,
                    registry_changed,
                    human_index_created,
                    index_created,
                    gate_created,
                    error,
                ));
            }
        }
        if let Err(error) = create_new(path, bytes) {
            return Err(rollback_or_preserve(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
                error,
            ));
        }
        index_created = true;
    }
    if let Some((path, bytes)) = &plan.gate {
        if let Err(error) = create_new(path, bytes) {
            return Err(rollback_or_preserve(
                plan,
                &created_dirs,
                registry_changed,
                human_index_created,
                index_created,
                gate_created,
                error,
            ));
        }
        gate_created = true;
    }
    if let Err(error) = hook(&plan.repo) {
        return Err(rollback_or_preserve(
            plan,
            &created_dirs,
            registry_changed,
            human_index_created,
            index_created,
            gate_created,
            error,
        ));
    }
    debug_assert!(!gate_created || plan.gate.is_some());
    Ok(())
}
fn create_new(path: &Path, bytes: &[u8]) -> Result<(), AdoptError> {
    let mut file = create_regular_file_nofollow(path).map_err(|source| AdoptError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Err(source) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        if let Err(cleanup) = remove_if_created(path) {
            let mut diagnostics = AdoptError::Io {
                path: path.to_path_buf(),
                source,
            }
            .into_diagnostics();
            diagnostics.extend(
                AdoptError::Io {
                    path: path.to_path_buf(),
                    source: cleanup,
                }
                .into_diagnostics(),
            );
            return Err(AdoptError::Diagnostics(diagnostics));
        }
        return Err(AdoptError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn rollback_or_preserve(
    plan: &AdoptPlan,
    created_dirs: &[PathBuf],
    registry_changed: bool,
    human_index_created: bool,
    index_created: bool,
    gate_created: bool,
    original: AdoptError,
) -> AdoptError {
    match rollback_adopt(
        plan,
        created_dirs,
        registry_changed,
        human_index_created,
        index_created,
        gate_created,
    ) {
        Ok(()) => original,
        Err(rollback) => {
            let mut diagnostics = original.into_diagnostics();
            diagnostics.extend(rollback.into_diagnostics());
            AdoptError::Diagnostics(diagnostics)
        }
    }
}

fn rollback_adopt(
    plan: &AdoptPlan,
    created_dirs: &[PathBuf],
    registry_changed: bool,
    human_index_created: bool,
    index_created: bool,
    gate_created: bool,
) -> Result<(), AdoptError> {
    let mut failures = Vec::new();
    let mut record_failure = |error: AdoptError| failures.extend(error.into_diagnostics());
    if gate_created {
        if let Some((path, _)) = &plan.gate {
            if let Err(source) = remove_if_created(path) {
                record_failure(AdoptError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    if index_created {
        if let Some((path, _)) = &plan.index {
            if let Err(source) = remove_if_created(path) {
                record_failure(AdoptError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    if human_index_created {
        if let Some((path, _)) = &plan.human_index {
            if let Err(source) = remove_if_created(path) {
                record_failure(AdoptError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    for path in created_dirs.iter().rev() {
        match fs::remove_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => record_failure(AdoptError::Io {
                path: path.clone(),
                source,
            }),
        }
    }
    if registry_changed {
        if let Err(error) = write_registry(&plan.registry, &plan.registry_before) {
            record_failure(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AdoptError::Diagnostics(failures))
    }
}
fn remove_if_created(path: &Path) -> std::io::Result<()> {
    match remove_file_nofollow(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
fn render_human_index(snapshot: &SourceSnapshot) -> String {
    let root = &snapshot.source_root;
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
                .filter_map(|component| component.as_os_str().to_str())
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
    let path = hook_path(registry);
    format!(
        "pre-commit:\n  commands:\n    rhizome-check:\n      run: 'rhizome check --registry \"{path}\" -- {{staged_files}}'\n"
    )
}
fn hook_path(registry: &Path) -> String {
    let value = registry.to_str().unwrap_or_default();
    #[cfg(windows)]
    {
        value.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        value.to_owned()
    }
}
fn hook_path_safe(registry: &Path) -> bool {
    registry.to_str().is_some()
        && hook_path(registry).chars().all(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\'' | '"'
                        | '$'
                        | '`'
                        | '%'
                        | '&'
                        | '|'
                        | '<'
                        | '>'
                        | '^'
                        | '!'
                        | '\\'
                        | ';'
                        | '('
                        | ')'
                        | '{'
                        | '}'
                        | '['
                        | ']'
                        | '*'
                        | '?'
                        | '~'
                        | '#'
                )
        })
}

/// Recognize only the active, registry-bound pre-commit command emitted above.
pub(crate) fn active_gate(text: &str, registry: &Path) -> bool {
    if !hook_path_safe(registry) {
        return false;
    }
    let expected = format!(
        "run: 'rhizome check --registry \"{}\" -- {{staged_files}}'",
        hook_path(registry)
    );
    let mut in_pre_commit = false;
    let mut pre_commit_count = 0usize;
    let mut commands_count = 0usize;
    let mut in_commands = false;
    let mut in_command = false;
    let mut run_count = 0usize;
    let mut disabled = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        if line[..indent].contains('\t') {
            return false;
        }
        if indent == 0 {
            in_pre_commit = trimmed == "pre-commit:";
            if in_pre_commit {
                pre_commit_count += 1;
            }
            in_commands = false;
            in_command = false;
            continue;
        }
        if !in_pre_commit {
            continue;
        }
        if is_enabled_skip(trimmed) {
            disabled = true;
        }
        if in_command && indent == 6 && (trimmed.starts_with("<<:") || trimmed.starts_with("glob:"))
        {
            // YAML merge keys and command globs can hide or exclude staged
            // files from the strict gate command.
            return false;
        }
        match indent {
            2 => {
                in_commands = trimmed == "commands:";
                if in_commands {
                    commands_count += 1;
                }
                in_command = false;
            }
            4 if in_commands => {
                in_command = trimmed.ends_with(':');
            }
            6 if in_command && trimmed == expected => {
                run_count += 1;
            }
            _ => {}
        }
    }
    pre_commit_count == 1 && commands_count == 1 && run_count == 1 && !disabled
}

fn is_enabled_skip(line: &str) -> bool {
    let Some((key, value)) = line.split_once(':') else {
        return false;
    };
    let key = key.trim();
    if matches!(key, "\"skip\"" | "'skip'") {
        // Quoted keys are valid YAML, but are outside the narrowly recognized
        // emitted form. Reject them rather than treating an existing gate as active.
        return true;
    }
    if key != "skip" {
        return false;
    }
    let value = value.split('#').next().unwrap_or_default().trim();
    let value = value.strip_prefix("!!bool").unwrap_or(value).trim();
    if matches!(
        value,
        "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF"
    ) {
        return false;
    }
    // A skip key with any other value is either an enabled YAML boolean or
    // malformed/indirect YAML. Both must fail closed rather than activating
    // a gate whose command will not run.
    true
}

fn write_registry(path: &Path, bytes: &[u8]) -> Result<(), AdoptError> {
    write_regular_file_nofollow(path, bytes).map_err(|source| AdoptError::Io {
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
fn git_repository_root(path: &Path) -> Option<PathBuf> {
    let mut current = fs::canonicalize(path).ok()?;
    loop {
        if let Ok(git) = GitBackend::new(&current) {
            return Some(git.root().to_path_buf());
        }
        if !current.pop() {
            return None;
        }
    }
}
fn read_registry(path: &Path) -> std::io::Result<Vec<u8>> {
    let link_metadata = fs::symlink_metadata(path)?;
    if !link_metadata.is_file() && !link_metadata.file_type().is_symlink() {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
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
#[cfg(test)]
mod tests {
    use super::{MAX_REGISTRY_BYTES, active_gate, plan_adopt};
    use std::fs;
    use std::path::Path;

    #[test]
    fn active_gate_rejects_quoted_skip_key() {
        let registry = Path::new("sources.toml");
        let text = r#"pre-commit:
  commands:
    rhizome-check:
      "skip": true
      run: 'rhizome check --registry "sources.toml" -- {staged_files}'
"#;
        assert!(!active_gate(text, registry));
    }
    #[test]
    fn active_gate_rejects_unresolved_yaml_merge_keys() {
        let registry = Path::new("sources.toml");
        let text = r#"pre-commit:
  commands:
    rhizome-check:
      <<: {skip: true}
      run: 'rhizome check --registry "sources.toml" -- {staged_files}'
"#;
        assert!(!active_gate(text, registry));
    }

    #[test]
    fn plan_adopt_rejects_registry_append_over_reader_limit() {
        let root = std::env::temp_dir().join(format!(
            "rhizome-adopt-registry-limit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("scratch directory should be created");
        let registry_path = root.join("sources.toml");
        let repo = root.join("repo");
        let mut registry = br#"workspace_root = "."

[[source]]
name = "seed"
path = "seed"
surface = "core"
"#
        .to_vec();
        registry.resize(MAX_REGISTRY_BYTES, b'#');
        fs::create_dir_all(root.join("seed")).expect("seed source should be created");
        fs::write(&registry_path, registry).expect("registry should be written");
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .expect("git should be installed");
        assert!(status.success(), "git init should succeed");

        let request = super::AdoptRequest {
            registry: registry_path,
            logical_source: "knowledge".into(),
            repo,
            description: "size limit".into(),
            keywords: Vec::new(),
        };
        let error = plan_adopt(&request).expect_err("oversized registry append must fail");
        let diagnostics = error.into_diagnostics();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "KBV2-ADOPT-REGISTRY"),
            "{diagnostics:?}"
        );
        let _ = fs::remove_dir_all(root);
    }
}
