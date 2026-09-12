use crate::DocumentRecord;
use crate::analyzer::{NATURAL_V2, SLUG_V2, natural_v2, slug_v2};
use std::fmt;
use std::fs;
use std::path::Path;
use tantivy::schema::{IndexRecordOption, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, STORED};
use tantivy::{Index, TantivyDocument};

/// Stable profile identifier for the central lexical Tantivy index.
pub const INDEX_PROFILE_V2: &str = "tantivy-central-v2";
/// Alias used by generation and projection callers.
pub const INDEX_PROFILE: &str = INDEX_PROFILE_V2;

/// Versioned index profile selected by this schema.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IndexProfile {
    V2,
}

impl IndexProfile {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V2 => INDEX_PROFILE_V2,
        }
    }
}

impl Default for IndexProfile {
    fn default() -> Self {
        Self::V2
    }
}

impl fmt::Display for IndexProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str((*self).as_str())
    }
}

/// Field containing the stable logical identity.
pub const FIELD_IDENTITY: &str = "identity";
/// Field containing the logical source name.
pub const FIELD_SOURCE: &str = "source";
/// Field containing the exact C2 domain.
pub const FIELD_DOMAIN: &str = "domain";
/// Field containing cumulative C2 domain prefixes.
pub const FIELD_DOMAIN_PREFIXES: &str = "domain_prefixes";
/// Stored natural-language title.
pub const FIELD_TITLE: &str = "title";
/// Stored source description.
pub const FIELD_DESCRIPTION: &str = "description";
/// Stored/indexed individual keywords.
pub const FIELD_KEYWORDS: &str = "keywords";
/// Field containing the source note kind.
pub const FIELD_KIND: &str = "kind";
/// Stored flag indicating whether kind was explicit in source frontmatter.
pub const FIELD_KIND_EXPLICIT: &str = "kind_explicit";
/// Field containing the optional source status.
pub const FIELD_STATUS: &str = "status";
/// Stored source-relative POSIX path.
pub const FIELD_SOURCE_PATH: &str = "source_path";
/// Stored source-content SHA-256 digest.
pub const FIELD_SOURCE_HASH: &str = "source_hash";
/// Stored compiled-projection SHA-256 digest.
pub const FIELD_COMPILED_HASH: &str = "compiled_hash";
/// Stored normalized Markdown body.
pub const FIELD_BODY: &str = "body";
/// Natural-language aggregate used by the lexical query parser.
pub const FIELD_CONTENT: &str = "content";
/// Stored Git commit time, when available.
pub const FIELD_COMMIT_TIME: &str = "commit_time";

/// Build the versioned central-index schema.
#[must_use]
pub fn build_schema() -> Schema {
    let mut builder = SchemaBuilder::default();

    builder.add_text_field(FIELD_IDENTITY, slug_text_options(true, true));
    builder.add_text_field(FIELD_SOURCE, exact_text_options());
    builder.add_text_field(FIELD_DOMAIN, exact_text_options());
    builder.add_text_field(FIELD_DOMAIN_PREFIXES, exact_text_options());
    builder.add_text_field(FIELD_TITLE, natural_text_options());
    builder.add_text_field(FIELD_DESCRIPTION, stored_text_options());
    builder.add_text_field(FIELD_KEYWORDS, exact_text_options());
    builder.add_text_field(FIELD_KIND, exact_text_options());
    builder.add_bool_field(FIELD_KIND_EXPLICIT, STORED);
    builder.add_text_field(FIELD_STATUS, exact_text_options());
    builder.add_text_field(FIELD_SOURCE_PATH, slug_text_options(true, true));
    builder.add_text_field(FIELD_SOURCE_HASH, stored_text_options());
    builder.add_text_field(FIELD_COMPILED_HASH, stored_text_options());
    builder.add_text_field(FIELD_BODY, stored_text_options());
    builder.add_text_field(FIELD_CONTENT, natural_text_options());
    builder.add_text_field(FIELD_COMMIT_TIME, stored_text_options());

    builder.build()
}

fn natural_text_options() -> TextOptions {
    TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(NATURAL_V2)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored()
}

fn slug_text_options(stored: bool, fast: bool) -> TextOptions {
    let options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(SLUG_V2)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
    let options = if stored { options.set_stored() } else { options };
    if fast { options.set_fast(None) } else { options }
}

fn exact_text_options() -> TextOptions {
    TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        )
        .set_stored()
        .set_fast(None)
}

fn stored_text_options() -> TextOptions {
    TextOptions::default().set_stored()
}

/// Register both versioned analyzers on an index before writing or querying it.
pub fn register_analyzers(index: &Index) {
    index.tokenizers().register(NATURAL_V2, natural_v2());
    index.tokenizers().register(SLUG_V2, slug_v2());
}

/// Build one persistent central index from the strict compiled document stream.
///
/// The profile is persisted as Tantivy's commit payload so readers can validate
/// the index without consulting a sidecar file.
pub fn build_tantivy<P: AsRef<Path>>(
    index_dir: P,
    records: &[DocumentRecord],
) -> tantivy::Result<Index> {
    let index_dir = index_dir.as_ref();
    fs::create_dir_all(index_dir)?;
    let index = Index::create_in_dir(index_dir, build_schema())?;
    register_analyzers(&index);

    let fields = IndexFields::from_schema(&index.schema());
    let mut writer = index.writer_with_num_threads(1, 15 * 1024 * 1024)?;
    for record in records {
        writer.add_document(fields.document(record))?;
    }
    let mut prepared_commit = writer.prepare_commit()?;
    prepared_commit.set_payload(&format!("{{\"index_profile\":\"{}\"}}", INDEX_PROFILE));
    prepared_commit.commit()?;
    Ok(index)
}

struct IndexFields {
    identity: tantivy::schema::Field,
    source: tantivy::schema::Field,
    domain: tantivy::schema::Field,
    domain_prefixes: tantivy::schema::Field,
    title: tantivy::schema::Field,
    description: tantivy::schema::Field,
    keywords: tantivy::schema::Field,
    kind: tantivy::schema::Field,
    kind_explicit: tantivy::schema::Field,
    status: tantivy::schema::Field,
    source_path: tantivy::schema::Field,
    source_hash: tantivy::schema::Field,
    compiled_hash: tantivy::schema::Field,
    body: tantivy::schema::Field,
    content: tantivy::schema::Field,
    commit_time: tantivy::schema::Field,
}

impl IndexFields {
    fn from_schema(schema: &Schema) -> Self {
        let get = |name: &str| schema.get_field(name).expect("central schema field exists");
        Self {
            identity: get(FIELD_IDENTITY),
            source: get(FIELD_SOURCE),
            domain: get(FIELD_DOMAIN),
            domain_prefixes: get(FIELD_DOMAIN_PREFIXES),
            title: get(FIELD_TITLE),
            description: get(FIELD_DESCRIPTION),
            keywords: get(FIELD_KEYWORDS),
            kind: get(FIELD_KIND),
            kind_explicit: get(FIELD_KIND_EXPLICIT),
            status: get(FIELD_STATUS),
            source_path: get(FIELD_SOURCE_PATH),
            source_hash: get(FIELD_SOURCE_HASH),
            compiled_hash: get(FIELD_COMPILED_HASH),
            body: get(FIELD_BODY),
            content: get(FIELD_CONTENT),
            commit_time: get(FIELD_COMMIT_TIME),
        }
    }

    fn document(&self, record: &DocumentRecord) -> TantivyDocument {
        let mut document = TantivyDocument::new();
        document.add_text(self.identity, &record.identity);
        document.add_text(self.source, &record.source);
        document.add_text(self.domain, &record.domain);
        for prefix in &record.domain_prefixes {
            document.add_text(self.domain_prefixes, prefix);
        }
        document.add_text(self.title, &record.title);
        document.add_text(self.description, &record.description);
        for keyword in &record.keywords {
            document.add_text(self.keywords, keyword);
        }
        document.add_text(self.kind, &record.kind);
        document.add_bool(self.kind_explicit, record.kind_explicit);
        if let Some(status) = &record.status {
            document.add_text(self.status, status);
        }
        document.add_text(self.source_path, &record.source_path);
        document.add_text(self.source_hash, &record.source_hash);
        document.add_text(self.compiled_hash, &record.compiled_hash);
        document.add_text(self.body, &record.body_text);
        document.add_text(self.content, record.embedding_text());
        if let Some(commit_time) = &record.commit_time {
            document.add_text(self.commit_time, commit_time);
        }
        document
    }
}
