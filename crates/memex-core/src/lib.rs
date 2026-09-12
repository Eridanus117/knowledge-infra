#![forbid(unsafe_code)]

//! Deterministic compiled-document and retrieval boundaries.

pub mod docs_ndjson;
pub mod document;
pub mod error;
pub mod analyzer;
pub mod tantivy_schema;
pub use analyzer::{NATURAL_V2, SLUG_V2, natural_v2, slug_v2};

pub use docs_ndjson::{decode_ndjson, encode_ndjson};
pub use document::{
    CommitTimeSource, DOCUMENT_SCHEMA, DocumentRecord, compile_snapshot, embedding_text,
};
pub use error::MemexError;
pub use tantivy_schema::{
    FIELD_BODY, FIELD_COMPILED_HASH, FIELD_COMMIT_TIME, FIELD_CONTENT, FIELD_DESCRIPTION,
    FIELD_DOMAIN, FIELD_DOMAIN_PREFIXES, FIELD_IDENTITY, FIELD_KEYWORDS, FIELD_KIND,
    FIELD_KIND_EXPLICIT, FIELD_SOURCE, FIELD_SOURCE_HASH, FIELD_SOURCE_PATH, FIELD_STATUS,
    FIELD_TITLE, INDEX_PROFILE, INDEX_PROFILE_V2, IndexProfile, build_schema, build_tantivy,
    register_analyzers,
};
pub use rhizome_core::{DomainNode, NoteLocator, SnapshotNote, SourceContext, SourceSnapshot};
