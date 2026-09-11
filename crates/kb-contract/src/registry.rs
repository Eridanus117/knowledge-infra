use crate::Diagnostic;
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use toml::{Table, Value};

const NOT_FOUND: &str = "KBV2-REGISTRY-NOT-FOUND";
const READ: &str = "KBV2-REGISTRY-READ";
const TOML: &str = "KBV2-REGISTRY-TOML";
const UNKNOWN_FIELD: &str = "KBV2-REGISTRY-UNKNOWN-FIELD";
const REMOVED_FIELD: &str = "KBV2-REGISTRY-REMOVED-FIELD";
const MISSING_FIELD: &str = "KBV2-REGISTRY-MISSING-FIELD";
const WRONG_TYPE: &str = "KBV2-REGISTRY-WRONG-TYPE";
const INVALID_NAME: &str = "KBV2-REGISTRY-INVALID-NAME";
const DUPLICATE_NAME: &str = "KBV2-REGISTRY-DUPLICATE-NAME";
const INVALID_SURFACE: &str = "KBV2-REGISTRY-INVALID-SURFACE";
const SOURCE_ROOT: &str = "KBV2-REGISTRY-SOURCE-ROOT";
const DUPLICATE_ROOT: &str = "KBV2-REGISTRY-DUPLICATE-ROOT";
const UNKNOWN_OVERLAY: &str = "KBV2-REGISTRY-UNKNOWN-OVERLAY";
const OVERLAY_FIELD: &str = "KBV2-REGISTRY-OVERLAY-FIELD";

const READ_MESSAGE: &str = "source registry could not be read";
const TOML_MESSAGE: &str = "source registry is not valid TOML";
const UNKNOWN_FIELD_MESSAGE: &str = "registry field is not allowed by source-contract-v2";
const REMOVED_FIELD_MESSAGE: &str = "registry field was removed in source-contract-v2";
const MISSING_FIELD_MESSAGE: &str = "required registry field is missing";
const WRONG_TYPE_MESSAGE: &str = "registry field has the wrong TOML type";
const INVALID_NAME_MESSAGE: &str = "source name must match ^[a-z][a-z0-9-]*$";
const DUPLICATE_NAME_MESSAGE: &str = "source name is duplicated";
const INVALID_SURFACE_MESSAGE: &str = "source surface must be `core` or `vertical`";
const SOURCE_ROOT_MESSAGE: &str = "source root must be an existing directory";
const DUPLICATE_ROOT_MESSAGE: &str = "canonical source root is already registered";
const UNKNOWN_OVERLAY_MESSAGE: &str = "overlay source does not exist in the main registry";
const OVERLAY_FIELD_MESSAGE: &str = "local overlay may override only source.path";

/// Stable logical name for a registered knowledge source.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceName(String);

impl SourceName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for SourceName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SourceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Human-facing placement of a source in the knowledge surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Surface {
    Core,
    Vertical,
}

/// A validated logical source and its canonical physical root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpec {
    pub name: SourceName,
    pub root: PathBuf,
    pub surface: Surface,
}

/// A validated source registry ordered by logical source name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registry {
    pub origin: PathBuf,
    pub sources: BTreeMap<SourceName, SourceSpec>,
}

/// Fully injected registry-location inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryLocator {
    pub explicit: Option<PathBuf>,
    pub cwd: PathBuf,
    pub env_path: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub user_config: PathBuf,
}

struct PendingSource {
    name: SourceName,
    root: PathBuf,
    surface: Surface,
    row: usize,
}

struct OpenRegistry {
    path: PathBuf,
    file: File,
}

/// Resolve, parse, overlay, and validate the first configured source registry.
pub fn resolve_registry(locator: &RegistryLocator) -> Result<Registry, Vec<Diagnostic>> {
    resolve_registry_inner(locator).map_err(|diagnostic| vec![diagnostic])
}

fn resolve_registry_inner(locator: &RegistryLocator) -> Result<Registry, Diagnostic> {
    let selected = select_registry(locator)?;
    let origin = selected.path.clone();
    let table = read_table(selected)?;
    let mut sources = parse_main_registry(&origin, table)?;
    apply_local_overlay(&origin, &mut sources)?;
    finish_registry(origin, sources)
}

fn select_registry(locator: &RegistryLocator) -> Result<OpenRegistry, Diagnostic> {
    if let Some(explicit) = &locator.explicit {
        if let Some(selected) = open_candidate(explicit)? {
            return Ok(selected);
        }
        return Err(
            Diagnostic::error(NOT_FOUND, "explicit registry does not exist")
                .at_path(explicit.clone()),
        );
    }

    if let Some(env_path) = &locator.env_path {
        if let Some(selected) = open_candidate(env_path)? {
            return Ok(selected);
        }
        return Err(
            Diagnostic::error(NOT_FOUND, "KB_SOURCES registry does not exist")
                .at_path(env_path.clone()),
        );
    }

    if let Some(workspace_root) = &locator.workspace_root {
        let workspace_registry = workspace_root.join("kb-sources.toml");
        if let Some(selected) = open_candidate(&workspace_registry)? {
            return Ok(selected);
        }
    }

    if let Some(selected) = open_candidate(&locator.user_config)? {
        return Ok(selected);
    }

    Err(Diagnostic::error(
        NOT_FOUND,
        "no source registry exists in the configured workspace or user locations",
    ))
}

fn open_candidate(path: &Path) -> Result<Option<OpenRegistry>, Diagnostic> {
    let file = match open_read_only(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(Diagnostic::error(READ, READ_MESSAGE).at_path(path.to_path_buf()));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|_| Diagnostic::error(READ, READ_MESSAGE).at_path(path.to_path_buf()))?;
    if !metadata.is_file() {
        return Err(Diagnostic::error(READ, READ_MESSAGE).at_path(path.to_path_buf()));
    }

    Ok(Some(OpenRegistry {
        path: path.to_path_buf(),
        file,
    }))
}

fn open_read_only(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    options.open(path)
}

fn read_table(selected: OpenRegistry) -> Result<Table, Diagnostic> {
    let OpenRegistry { path, mut file } = selected;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| Diagnostic::error(READ, READ_MESSAGE).at_path(path.clone()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Diagnostic::error(TOML, TOML_MESSAGE).at_path(path.clone()))?;
    text.parse::<Table>()
        .map_err(|_| Diagnostic::error(TOML, TOML_MESSAGE).at_path(path))
}

fn parse_main_registry(origin: &Path, table: Table) -> Result<Vec<PendingSource>, Diagnostic> {
    if table.contains_key("legacy") {
        return Err(field_diagnostic(
            REMOVED_FIELD,
            REMOVED_FIELD_MESSAGE,
            origin,
            "legacy",
        ));
    }
    if let Some(field) = first_unknown_field(&table, &["workspace_root", "source"]) {
        return Err(field_diagnostic(
            UNKNOWN_FIELD,
            UNKNOWN_FIELD_MESSAGE,
            origin,
            field,
        ));
    }

    let origin_dir = registry_parent(origin);
    let base = match table.get("workspace_root") {
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| {
                field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, origin, "workspace_root")
            })?;
            resolve_from_origin(origin_dir, raw)
        }
        None => origin_dir.to_path_buf(),
    };

    let source_value: &Value = table
        .get("source")
        .ok_or_else(|| field_diagnostic(MISSING_FIELD, MISSING_FIELD_MESSAGE, origin, "source"))?;
    let rows = source_value
        .as_array()
        .ok_or_else(|| field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, origin, "source"))?;
    if rows.is_empty() {
        return Err(field_diagnostic(
            MISSING_FIELD,
            MISSING_FIELD_MESSAGE,
            origin,
            "source",
        ));
    }

    let mut names = BTreeSet::new();
    let mut sources = Vec::with_capacity(rows.len());
    for (row_index, value) in rows.iter().enumerate() {
        let row_field = source_field(row_index, "");
        let row = value.as_table().ok_or_else(|| {
            field_diagnostic(
                WRONG_TYPE,
                WRONG_TYPE_MESSAGE,
                origin,
                row_field.trim_end_matches('.'),
            )
        })?;

        if row.contains_key("legacy") {
            return Err(field_diagnostic(
                REMOVED_FIELD,
                REMOVED_FIELD_MESSAGE,
                origin,
                &source_field(row_index, "legacy"),
            ));
        }
        if let Some(field) = first_unknown_field(row, &["name", "path", "surface"]) {
            return Err(field_diagnostic(
                UNKNOWN_FIELD,
                UNKNOWN_FIELD_MESSAGE,
                origin,
                &source_field(row_index, field),
            ));
        }

        let name_field = source_field(row_index, "name");
        let name = row
            .get("name")
            .ok_or_else(|| {
                field_diagnostic(MISSING_FIELD, MISSING_FIELD_MESSAGE, origin, &name_field)
            })?
            .as_str()
            .ok_or_else(|| field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, origin, &name_field))?;
        if !valid_source_name(name) {
            return Err(field_diagnostic(
                INVALID_NAME,
                INVALID_NAME_MESSAGE,
                origin,
                &name_field,
            ));
        }
        if !names.insert(name.to_owned()) {
            return Err(field_diagnostic(
                DUPLICATE_NAME,
                DUPLICATE_NAME_MESSAGE,
                origin,
                &name_field,
            ));
        }

        let root = match row.get("path") {
            Some(value) => {
                let path_field = source_field(row_index, "path");
                let raw = value.as_str().ok_or_else(|| {
                    field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, origin, &path_field)
                })?;
                resolve_from_origin(origin_dir, raw)
            }
            None => base.join(name),
        };

        let surface = match row.get("surface") {
            None => Surface::Vertical,
            Some(value) => {
                let surface_field = source_field(row_index, "surface");
                let raw = value.as_str().ok_or_else(|| {
                    field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, origin, &surface_field)
                })?;
                match raw {
                    "core" => Surface::Core,
                    "vertical" => Surface::Vertical,
                    _ => {
                        return Err(field_diagnostic(
                            INVALID_SURFACE,
                            INVALID_SURFACE_MESSAGE,
                            origin,
                            &surface_field,
                        ));
                    }
                }
            }
        };

        sources.push(PendingSource {
            name: SourceName(name.to_owned()),
            root,
            surface,
            row: row_index,
        });
    }

    Ok(sources)
}

fn apply_local_overlay(origin: &Path, sources: &mut [PendingSource]) -> Result<(), Diagnostic> {
    let Some(overlay_path) = local_overlay_path(origin) else {
        return Ok(());
    };
    let Some(selected) = open_candidate(&overlay_path)? else {
        return Ok(());
    };

    let table = read_table(selected)?;
    if table.contains_key("legacy") {
        return Err(field_diagnostic(
            REMOVED_FIELD,
            REMOVED_FIELD_MESSAGE,
            &overlay_path,
            "legacy",
        ));
    }
    if let Some(field) = first_unknown_field(&table, &["source"]) {
        return Err(field_diagnostic(
            OVERLAY_FIELD,
            OVERLAY_FIELD_MESSAGE,
            &overlay_path,
            field,
        ));
    }

    let source_value: &Value = table.get("source").ok_or_else(|| {
        field_diagnostic(
            MISSING_FIELD,
            MISSING_FIELD_MESSAGE,
            &overlay_path,
            "source",
        )
    })?;
    let rows = source_value
        .as_array()
        .ok_or_else(|| field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, &overlay_path, "source"))?;
    if rows.is_empty() {
        return Err(field_diagnostic(
            MISSING_FIELD,
            MISSING_FIELD_MESSAGE,
            &overlay_path,
            "source",
        ));
    }

    let origin_dir = registry_parent(origin);
    for (row_index, value) in rows.iter().enumerate() {
        let row_field = source_field(row_index, "");
        let row = value.as_table().ok_or_else(|| {
            field_diagnostic(
                WRONG_TYPE,
                WRONG_TYPE_MESSAGE,
                &overlay_path,
                row_field.trim_end_matches('.'),
            )
        })?;

        if row.contains_key("legacy") {
            return Err(field_diagnostic(
                REMOVED_FIELD,
                REMOVED_FIELD_MESSAGE,
                &overlay_path,
                &source_field(row_index, "legacy"),
            ));
        }
        if let Some(field) = first_unknown_field(row, &["name", "path"]) {
            return Err(field_diagnostic(
                OVERLAY_FIELD,
                OVERLAY_FIELD_MESSAGE,
                &overlay_path,
                &source_field(row_index, field),
            ));
        }

        let name_field = source_field(row_index, "name");
        let name = row
            .get("name")
            .ok_or_else(|| {
                field_diagnostic(
                    MISSING_FIELD,
                    MISSING_FIELD_MESSAGE,
                    &overlay_path,
                    &name_field,
                )
            })?
            .as_str()
            .ok_or_else(|| {
                field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, &overlay_path, &name_field)
            })?;
        if !valid_source_name(name) {
            return Err(field_diagnostic(
                INVALID_NAME,
                INVALID_NAME_MESSAGE,
                &overlay_path,
                &name_field,
            ));
        }

        let path_field = source_field(row_index, "path");
        let raw_path = row
            .get("path")
            .ok_or_else(|| {
                field_diagnostic(
                    MISSING_FIELD,
                    MISSING_FIELD_MESSAGE,
                    &overlay_path,
                    &path_field,
                )
            })?
            .as_str()
            .ok_or_else(|| {
                field_diagnostic(WRONG_TYPE, WRONG_TYPE_MESSAGE, &overlay_path, &path_field)
            })?;

        let source = sources
            .iter_mut()
            .find(|source| source.name.0 == name)
            .ok_or_else(|| {
                field_diagnostic(
                    UNKNOWN_OVERLAY,
                    UNKNOWN_OVERLAY_MESSAGE,
                    &overlay_path,
                    &name_field,
                )
            })?;
        source.root = resolve_from_origin(origin_dir, raw_path);
    }

    Ok(())
}

fn finish_registry(origin: PathBuf, sources: Vec<PendingSource>) -> Result<Registry, Diagnostic> {
    let mut roots = BTreeSet::new();
    let mut resolved = BTreeMap::new();

    for source in sources {
        let path_field = source_field(source.row, "path");
        let canonical_root = fs::canonicalize(&source.root).map_err(|_| {
            field_diagnostic(SOURCE_ROOT, SOURCE_ROOT_MESSAGE, &source.root, &path_field)
        })?;
        if !canonical_root.is_dir() {
            return Err(field_diagnostic(
                SOURCE_ROOT,
                SOURCE_ROOT_MESSAGE,
                &source.root,
                &path_field,
            ));
        }
        if !roots.insert(canonical_root.clone()) {
            return Err(field_diagnostic(
                DUPLICATE_ROOT,
                DUPLICATE_ROOT_MESSAGE,
                &canonical_root,
                &path_field,
            ));
        }

        let name = source.name;
        resolved.insert(
            name.clone(),
            SourceSpec {
                name,
                root: canonical_root,
                surface: source.surface,
            },
        );
    }

    Ok(Registry {
        origin,
        sources: resolved,
    })
}

fn first_unknown_field<'a>(table: &'a Table, allowed: &[&str]) -> Option<&'a str> {
    table
        .keys()
        .map(String::as_str)
        .filter(|field| !allowed.contains(field))
        .min()
}

fn field_diagnostic(
    code: &'static str,
    message: &'static str,
    path: &Path,
    field: &str,
) -> Diagnostic {
    Diagnostic::error(code, message)
        .at_path(path.to_path_buf())
        .for_field(field)
}

fn source_field(row: usize, field: &str) -> String {
    format!("source[{row}].{field}")
}

fn valid_source_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn registry_parent(registry: &Path) -> &Path {
    registry
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn resolve_from_origin(origin: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        origin.join(path)
    }
}

fn local_overlay_path(registry: &Path) -> Option<PathBuf> {
    let mut name = registry.file_stem()?.to_os_string();
    name.push(".local.toml");
    Some(registry.with_file_name(name))
}
