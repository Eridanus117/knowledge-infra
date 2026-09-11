use crate::human_index::check_human_index;
use crate::links::check_links_and_code;
use crate::source::{SourceContext, discover_source};
use kb_contract::{Diagnostic, SourceName};
use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

pub(crate) const HUMAN_INDEX_MARKER_CODE: &str = "KBV2-HUMAN-INDEX-MARKER";
pub(crate) const HUMAN_INDEX_DRIFT_CODE: &str = "KBV2-HUMAN-INDEX-DRIFT";

/// A complete source-plane check report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckReport {
    pub schema: &'static str,
    pub source: SourceName,
    pub findings: Vec<Diagnostic>,
}

/// Errors that prevent a source check or projection operation from completing.
#[derive(Debug)]
pub enum CoreError {
    Discovery(Vec<Diagnostic>),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    HumanIndex {
        path: PathBuf,
        message: &'static str,
    },
    OutsideSourceRoot {
        path: PathBuf,
    },
    ConcurrentModification {
        path: PathBuf,
    },
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(diagnostics) => {
                write!(formatter, "source discovery failed ({})", diagnostics.len())
            }
            Self::Io { path, .. } => {
                write!(formatter, "could not read or write {}", path.display())
            }
            Self::HumanIndex { message, .. } => formatter.write_str(message),
            Self::OutsideSourceRoot { path } => write!(
                formatter,
                "human index is outside the source root: {}",
                path.display()
            ),
            Self::ConcurrentModification { path } => write!(
                formatter,
                "human index changed while applying: {}",
                path.display()
            ),
        }
    }
}

impl Error for CoreError {}

/// Discover and check one registered source without producing a partial snapshot.
pub fn check_source(context: &SourceContext) -> Result<CheckReport, CoreError> {
    let snapshot = discover_source(context).map_err(CoreError::Discovery)?;
    let mut findings = check_links_and_code(&snapshot, context);
    let index = context.source.root.join("INDEX.md");
    findings.extend(check_human_index(&snapshot, &index)?);
    Ok(CheckReport {
        schema: "rhizome-check-v2",
        source: snapshot.source,
        findings,
    })
}
