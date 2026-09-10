use crate::CoreError;
use kb_contract::ContractError;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// An absolute filesystem boundary containing one logical knowledge source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRoot(PathBuf);

impl SourceRoot {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ContractError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(ContractError::new(
                "source.root.empty",
                "source root must not be empty",
            ));
        }
        if !path.is_absolute() {
            return Err(ContractError::new(
                "source.root.relative",
                "source root must be absolute",
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

/// A normalized path relative to a [`SourceRoot`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourcePath(PathBuf);

impl SourcePath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ContractError> {
        let path = path.into();
        let invalid = path.as_os_str().is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_)
                        | Component::RootDir
                        | Component::ParentDir
                        | Component::CurDir
                )
            });

        if invalid {
            return Err(ContractError::new(
                "source.path.invalid",
                "source path must be a non-empty normalized relative path",
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

/// Byte-preserving source access used by contract and Git-aware operations.
pub trait SourceReader {
    fn read(&self, root: &SourceRoot, path: &SourcePath) -> Result<Vec<u8>, CoreError>;
}

/// Filesystem implementation that rejects symlink escapes from the source root.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileSystemSourceReader;

impl SourceReader for FileSystemSourceReader {
    fn read(&self, root: &SourceRoot, path: &SourcePath) -> Result<Vec<u8>, CoreError> {
        let canonical_root = fs::canonicalize(root.as_path())
            .map_err(|error| CoreError::io("resolve", root.as_path(), error))?;
        let requested = root.as_path().join(path.as_path());
        let canonical_path = fs::canonicalize(&requested)
            .map_err(|error| CoreError::io("resolve", &requested, error))?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(ContractError::new(
                "source.path.escape",
                "source path resolves outside its source root",
            )
            .at_path(requested)
            .into());
        }

        fs::read(&canonical_path).map_err(|error| CoreError::io("read", &canonical_path, error))
    }
}
