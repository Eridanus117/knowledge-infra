use memex_core::tantivy_schema::{
    FIELD_BODY, FIELD_COMPILED_HASH, FIELD_CONTENT, FIELD_DESCRIPTION, FIELD_DOMAIN,
    FIELD_DOMAIN_PREFIXES, FIELD_IDENTITY, FIELD_KEYWORDS, FIELD_KIND, FIELD_SOURCE,
    FIELD_SOURCE_HASH, FIELD_SOURCE_PATH, FIELD_STATUS, FIELD_TITLE, INDEX_PROFILE, IndexProfile,
    build_schema, build_tantivy,
};
use memex_core::{DOCUMENT_SCHEMA, DocumentRecord};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{Index, TantivyDocument, Term};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("memex-tantivy-{timestamp}-{sequence}"));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn record(
    identity: &str,
    source: &str,
    domain: &str,
    domain_prefixes: &[&str],
    title: &str,
    description: &str,
    keywords: &[&str],
    kind: &str,
    status: Option<&str>,
    body_text: &str,
    source_path: &str,
) -> DocumentRecord {
    DocumentRecord {
        schema: DOCUMENT_SCHEMA,
        identity: identity.to_owned(),
        source: source.to_owned(),
        domain: domain.to_owned(),
        domain_prefixes: domain_prefixes
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        title: title.to_owned(),
        description: description.to_owned(),
        keywords: keywords.iter().map(|value| (*value).to_owned()).collect(),
        kind: kind.to_owned(),
        kind_explicit: true,
        status: status.map(str::to_owned),
        body_text: body_text.to_owned(),
        source_path: source_path.to_owned(),
        source_hash: "11".repeat(32),
        compiled_hash: "22".repeat(32),
        commit_time: Some("2026-01-02T03:04:05Z".to_owned()),
    }
}

fn fixtures() -> Vec<DocumentRecord> {
    vec![
        record(
            "knowledge:alpha:search-guide",
            "knowledge",
            "alpha",
            &["alpha"],
            "检索指南",
            "中文检索",
            &["tantivy", "fixture"],
            "note",
            None,
            "# 搜索指南\n中央索引。",
            "alpha/search-guide.md",
        ),
        record(
            "archive:alpha:other",
            "archive",
            "alpha",
            &["alpha"],
            "其他资料",
            "中文检索",
            &["archive"],
            "reference",
            Some("frozen"),
            "# 其他资料\n归档内容。",
            "alpha/other.md",
        ),
        record(
            "knowledge:alpha/deep:runbook",
            "knowledge",
            "alpha/deep",
            &["alpha", "alpha/deep"],
            "运维手册",
            "部署检索",
            &["runbook", "fixture"],
            "runbook",
            None,
            "# 运维手册\n部署步骤。",
            "alpha/deep/runbook.md",
        ),
    ]
}
fn boost_fixtures() -> Vec<DocumentRecord> {
    vec![
        record(
            "fixture:boost:title",
            "fixture",
            "boost",
            &["boost"],
            "boostneedle",
            "title candidate",
            &["title"],
            "note",
            None,
            "Title candidate body.",
            "boost/title.md",
        ),
        record(
            "fixture:boost:content",
            "fixture",
            "boost",
            &["boost"],
            "content candidate",
            "boostneedle",
            &["content"],
            "note",
            None,
            "Content candidate body.",
            "boost/content.md",
        ),
        record(
            "fixture:boost:boostneedle",
            "fixture",
            "boost",
            &["boost"],
            "identity candidate",
            "identity candidate",
            &["identity"],
            "note",
            None,
            "Identity candidate body.",
            "boost/identity.md",
        ),
        record(
            "fixture:boost:path",
            "fixture",
            "boost",
            &["boost"],
            "path candidate",
            "path candidate",
            &["path"],
            "note",
            None,
            "Path candidate body.",
            "boost/boostneedle.md",
        ),
    ]
}

#[test]
fn field_boosts_produce_observable_title_identity_path_content_order() {
    let scratch = ScratchDirectory::new();
    let records = boost_fixtures();
    let index = build_tantivy(scratch.path(), &records).expect("central index should build");
    let schema = index.schema();
    let title = field(&schema, FIELD_TITLE);
    let content = field(&schema, FIELD_CONTENT);
    let identity = field(&schema, FIELD_IDENTITY);
    let source_path = field(&schema, FIELD_SOURCE_PATH);
    let mut parser = QueryParser::for_index(&index, vec![title, content, identity, source_path]);
    parser.set_field_boost(title, 5.0);
    parser.set_field_boost(content, 1.0);
    parser.set_field_boost(identity, 2.0);
    parser.set_field_boost(source_path, 2.0);

    let query = parser
        .parse_query("boostneedle")
        .expect("boost query should parse");
    let reader = index.reader().expect("index reader should open");
    let hits = reader
        .searcher()
        .search(&query, &TopDocs::with_limit(4).order_by_score())
        .expect("boost query should search");
    let identities = hits
        .iter()
        .map(|(_, address)| {
            reader
                .searcher()
                .doc::<TantivyDocument>(*address)
                .unwrap()
                .get_first(identity)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();

    assert_eq!(identities[0], "fixture:boost:title");
    assert!(identities[1..3].contains(&"fixture:boost:boostneedle".to_owned()));
    assert!(identities[1..3].contains(&"fixture:boost:path".to_owned()));
    assert_eq!(identities[3], "fixture:boost:content");
}

fn field(schema: &tantivy::schema::Schema, name: &str) -> tantivy::schema::Field {
    schema
        .get_field(name)
        .unwrap_or_else(|_| panic!("missing field {name}"))
}

#[test]
fn schema_declares_stored_content_and_exact_fast_fields() {
    let schema = build_schema();

    for name in [
        FIELD_IDENTITY,
        FIELD_SOURCE,
        FIELD_DOMAIN,
        FIELD_TITLE,
        FIELD_DESCRIPTION,
        FIELD_KEYWORDS,
        FIELD_KIND,
        FIELD_STATUS,
        FIELD_SOURCE_PATH,
        FIELD_SOURCE_HASH,
        FIELD_COMPILED_HASH,
        FIELD_BODY,
        FIELD_CONTENT,
    ] {
        let entry = schema.get_field_entry(field(&schema, name));
        assert!(entry.is_stored(), "{name} should be stored");
    }

    for name in [
        FIELD_SOURCE,
        FIELD_DOMAIN,
        FIELD_DOMAIN_PREFIXES,
        FIELD_KIND,
        FIELD_KEYWORDS,
        FIELD_STATUS,
    ] {
        let entry = schema.get_field_entry(field(&schema, name));
        assert!(entry.is_fast(), "{name} should be a fast field");
        assert!(entry.is_indexed(), "{name} should support exact filtering");
    }

    let title = schema.get_field_entry(field(&schema, FIELD_TITLE));
    let content = schema.get_field_entry(field(&schema, FIELD_CONTENT));
    assert!(title.is_indexed());
    assert!(content.is_indexed());
    assert_eq!(IndexProfile::V2.as_str(), INDEX_PROFILE);
    assert_eq!(IndexProfile::V2.to_string(), "tantivy-central-v2");
}

#[test]
fn central_index_persists_profile_and_stores_document_record_once() {
    let scratch = ScratchDirectory::new();
    let records = fixtures();
    let index = build_tantivy(scratch.path(), &records).expect("central index should build");
    assert_eq!(
        index.load_metas().unwrap().payload.as_deref(),
        Some("{\"index_profile\":\"tantivy-central-v2\"}")
    );
    assert!(!scratch.path().join("profile.json").exists());

    let content = field(&index.schema(), FIELD_CONTENT);
    let identity = field(&index.schema(), FIELD_IDENTITY);
    let reader = index.reader().expect("index reader should open");
    let searcher = reader.searcher();
    let parser = QueryParser::for_index(&index, vec![content]);
    let query = parser
        .parse_query("中央索引")
        .expect("content query should parse");
    let hit = searcher
        .search(&query, &TopDocs::with_limit(1).order_by_score())
        .expect("content query should search")
        .into_iter()
        .next()
        .expect("content query should find search guide");
    let document = searcher
        .doc::<TantivyDocument>(hit.1)
        .expect("stored document should load");
    assert_eq!(
        document.get_first(identity).unwrap().as_str(),
        Some("knowledge:alpha:search-guide")
    );
    assert_eq!(
        document.get_first(content).unwrap().as_str(),
        Some("中文检索\n\ntantivy fixture\n\n# 搜索指南\n中央索引。")
    );

    drop(reader);
    drop(index);
    let reopened =
        Index::open_in_dir(scratch.path()).expect("persistent central index should reopen");
    assert_eq!(reopened.schema(), build_schema());
}

#[test]
fn query_parser_boosts_title_content_identity_and_source_path() {
    let scratch = ScratchDirectory::new();
    let records = fixtures();
    let index = build_tantivy(scratch.path(), &records).expect("central index should build");
    let schema = index.schema();
    let title = field(&schema, FIELD_TITLE);
    let content = field(&schema, FIELD_CONTENT);
    let identity = field(&schema, FIELD_IDENTITY);
    let source_path = field(&schema, FIELD_SOURCE_PATH);
    let mut parser = QueryParser::for_index(&index, vec![title, content, identity, source_path]);
    parser.set_field_boost(title, 5.0);
    parser.set_field_boost(content, 1.0);
    parser.set_field_boost(identity, 2.0);
    parser.set_field_boost(source_path, 2.0);

    let query_text = include_str!("../../../fixtures/memex/lexical/rank.query").trim();
    let expected_identity = include_str!("../../../fixtures/memex/lexical/rank.expected").trim();
    let query = parser
        .parse_query(query_text)
        .expect("Chinese query should parse");
    let reader = index.reader().expect("index reader should open");
    let hits = reader
        .searcher()
        .search(&query, &TopDocs::with_limit(3).order_by_score())
        .expect("Chinese query should search");
    assert_eq!(hits.len(), 3);
    let identities = hits
        .iter()
        .map(|(_, address)| {
            reader
                .searcher()
                .doc::<TantivyDocument>(*address)
                .unwrap()
                .get_first(identity)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(identities[0], expected_identity);
}

#[test]
fn exact_fast_filter_fields_match_only_their_value() {
    let scratch = ScratchDirectory::new();
    let records = fixtures();
    let index = build_tantivy(scratch.path(), &records).expect("central index should build");
    let schema = index.schema();
    let identity = field(&schema, FIELD_IDENTITY);
    let reader = index.reader().expect("index reader should open");
    let searcher = reader.searcher();
    for (name, value, expected) in [
        (FIELD_SOURCE, "archive", "archive:alpha:other"),
        (FIELD_DOMAIN, "alpha/deep", "knowledge:alpha/deep:runbook"),
        (
            FIELD_DOMAIN_PREFIXES,
            "alpha/deep",
            "knowledge:alpha/deep:runbook",
        ),
        (FIELD_KIND, "runbook", "knowledge:alpha/deep:runbook"),
        (FIELD_KEYWORDS, "tantivy", "knowledge:alpha:search-guide"),
        (FIELD_STATUS, "frozen", "archive:alpha:other"),
    ] {
        let query = TermQuery::new(
            Term::from_field_text(field(&schema, name), value),
            IndexRecordOption::Basic,
        );
        let hits = searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .expect("exact filter should search");
        let identities = hits
            .iter()
            .map(|(_, address)| {
                searcher
                    .doc::<TantivyDocument>(*address)
                    .unwrap()
                    .get_first(identity)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(identities, vec![expected], "filter {name}={value}");
    }
}
