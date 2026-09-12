use std::error::Error;
use std::fmt;
use std::path::PathBuf;

/// Errors at the deterministic compiled-document and NDJSON protocol boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemexError {
    /// The injected commit-time provider could not answer for a source path.
    CommitTime { path: PathBuf, message: String },
    /// A document field does not satisfy the `knowledge-doc-v2` contract.
    InvalidDocument {
        line: Option<usize>,
        field: &'static str,
        message: &'static str,
    },
    /// A record carries a schema other than the one defined by this crate.
    InvalidSchema { line: usize, actual: String },
    /// A source or compiled hash is not a lowercase SHA-256 digest, or does
    /// not match the deterministic projection.
    InvalidHash { line: usize, field: &'static str },
    /// A source-relative POSIX path is malformed or escapes the source root.
    InvalidPath { line: usize, path: String },
    /// Two records identify the same logical document.
    DuplicateIdentity { identity: String },
    /// A line is not a valid NDJSON document record.
    InvalidNdjson { line: usize, message: &'static str },
    /// The bytes parse as JSON but are not the canonical byte representation.
    NonCanonicalNdjson { line: usize },
}

impl fmt::Display for MemexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommitTime { path, message } => {
                write!(
                    formatter,
                    "commit time failed for {}: {message}",
                    path.display()
                )
            }
            Self::InvalidDocument {
                line: Some(line),
                field,
                message,
            } => write!(
                formatter,
                "invalid document at line {line}, field {field}: {message}"
            ),
            Self::InvalidDocument {
                line: None,
                field,
                message,
            } => write!(formatter, "invalid document field {field}: {message}"),
            Self::InvalidSchema { line, actual } => {
                write!(
                    formatter,
                    "invalid document schema at line {line}: {actual}"
                )
            }
            Self::InvalidHash { line, field } => {
                write!(formatter, "invalid {field} at line {line}")
            }
            Self::InvalidPath { line, path } => {
                write!(formatter, "invalid source path at line {line}: {path}")
            }
            Self::DuplicateIdentity { identity } => {
                write!(formatter, "duplicate document identity: {identity}")
            }
            Self::InvalidNdjson { line, message } => {
                write!(formatter, "invalid NDJSON at line {line}: {message}")
            }
            Self::NonCanonicalNdjson { line } => {
                write!(formatter, "non-canonical NDJSON at line {line}")
            }
        }
    }
}

impl Error for MemexError {}
