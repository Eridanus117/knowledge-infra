use kb_contract::ContractError;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Failures produced while operating on a knowledge source.
#[derive(Debug)]
pub enum CoreError {
    Contract(ContractError),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl CoreError {
    pub(crate) fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => fmt::Display::fmt(error, formatter),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} source path {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Contract(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl From<ContractError> for CoreError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}
