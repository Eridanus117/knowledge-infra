use kb_contract::Diagnostic;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Failures produced while operating on a knowledge source.
#[derive(Debug)]
pub enum CoreError {
    Diagnostic(Diagnostic),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(diagnostic) => fmt::Display::fmt(diagnostic, formatter),
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
            Self::Diagnostic(diagnostic) => Some(diagnostic),
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl From<Diagnostic> for CoreError {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}
