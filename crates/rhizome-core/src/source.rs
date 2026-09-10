use kb_contract::Diagnostic;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// An absolute path that was an existing directory when the value was created.
///
/// `SourceRoot` records source identity; filesystem access is outside its contract.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceRoot(PathBuf);

impl SourceRoot {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, Diagnostic> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(Diagnostic::error(
                "KBV2-SOURCE-ROOT-EMPTY",
                "source root must not be empty",
            ));
        }
        if !path.is_absolute() {
            return Err(Diagnostic::error(
                "KBV2-SOURCE-ROOT-RELATIVE",
                "source root must be absolute",
            )
            .at_path(path));
        }

        let metadata = fs::metadata(&path).map_err(|source| {
            let (code, description) = if source.kind() == ErrorKind::NotFound {
                ("KBV2-SOURCE-ROOT-MISSING", "source root does not exist")
            } else {
                (
                    "KBV2-SOURCE-ROOT-UNAVAILABLE",
                    "source root is not accessible",
                )
            };
            Diagnostic::error(code, format!("{description}: {}: {source}", path.display()))
                .at_path(path.clone())
        })?;

        if !metadata.is_dir() {
            return Err(Diagnostic::error(
                "KBV2-SOURCE-ROOT-NOT-DIRECTORY",
                format!("source root is not a directory: {}", path.display()),
            )
            .at_path(path));
        }

        Ok(Self(path))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A non-empty, normalized sequence of path components relative to a source.
/// This value is a lexical source identifier.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourcePath(PathBuf);

impl SourcePath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, Diagnostic> {
        let path = path.into();
        let mut normalized = PathBuf::new();

        for component in path.components() {
            match component {
                Component::Normal(segment) => normalized.push(segment),
                Component::Prefix(_)
                | Component::RootDir
                | Component::ParentDir
                | Component::CurDir => return Err(invalid_source_path(path.clone())),
            }
        }

        if normalized.as_os_str().is_empty() {
            return Err(invalid_source_path(path));
        }

        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

fn invalid_source_path(path: PathBuf) -> Diagnostic {
    Diagnostic::error(
        "KBV2-SOURCE-PATH-INVALID",
        "source path must contain only normalized relative components",
    )
    .at_path(path)
}
