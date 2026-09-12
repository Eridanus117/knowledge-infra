#![forbid(unsafe_code)]

//! Deterministic compiled-document and retrieval boundaries.

pub mod document;
pub mod docs_ndjson;
pub mod error;

pub use document::{
    CommitTimeSource, DOCUMENT_SCHEMA, DocumentRecord, compile_snapshot, embedding_text,
};
pub use docs_ndjson::{decode_ndjson, encode_ndjson};
pub use error::MemexError;
pub use rhizome_core::{DomainNode, NoteLocator, SnapshotNote, SourceContext, SourceSnapshot};
