#![forbid(unsafe_code)]

//! Source-plane boundaries for Git-backed Markdown knowledge.

pub mod error;
pub mod source;

pub use error::CoreError;
pub use source::{SourcePath, SourceRoot};
