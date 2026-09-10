#![forbid(unsafe_code)]

//! Shared value and error boundaries for knowledge-source contracts.

pub mod error;
pub mod frontmatter;
pub mod registry;

pub use error::{Diagnostic, Severity};
pub use frontmatter::{
    NoteFrontmatter, NoteKind, NoteStatus, ValidatedNote, parse_and_validate_note, render_note,
};
pub use registry::{Registry, RegistryLocator, SourceName, SourceSpec, Surface, resolve_registry};

/// Schema identifier carried by source-contract diagnostics and artifacts.
pub const SOURCE_CONTRACT_SCHEMA: &str = "source-contract-v2";
