#![forbid(unsafe_code)]

//! Source-plane boundaries for Git-backed Markdown knowledge.

pub mod source;

pub use source::{
    DomainNode, NoteLocator, SnapshotNote, SourceContext, SourceSnapshot, discover_source,
};
