use kb_contract::{SourceName, SourceSpec, Surface};
use memex_core::{
    CommitTimeSource, DocumentRecord, MemexError, SourceContext, compile_snapshot, decode_ndjson,
    encode_ndjson,
};
use rhizome_core::discover_source;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn from_fixture() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "memex-docs-ndjson-{timestamp}-{sequence}"
        ));
        copy_tree(&fixture_root(), &path);
        Self { path }
    }

    fn root(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy)]
struct FixedCommitTime;

impl CommitTimeSource for FixedCommitTime {
    fn commit_time(&self, path: &Path) -> Result<Option<String>, MemexError> {
        assert!(path.is_absolute());
        Ok(Some("2026-01-02T03:04:05Z".to_owned()))
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/memex/docs/tree")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("fixture destination should be created");
    let mut entries = fs::read_dir(source)
        .expect("fixture should be readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture entries should be readable");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let mut name = entry.file_name();
        if name == "GIT_MARKER" {
            name = ".git".into();
        }
        let destination_path = destination.join(name);
        let file_type = entry.file_type().expect("fixture type should be readable");
        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, destination_path).expect("fixture file should be copied");
        }
    }
}

fn snapshot() -> (ScratchDirectory, rhizome_core::SourceSnapshot) {
    let scratch = ScratchDirectory::from_fixture();
    let source_root = scratch.root().join("vault");
    let source = SourceName::new("knowledge").expect("fixture source name should be valid");
    let context = SourceContext {
        source: SourceSpec {
            name: source,
            root: source_root,
            surface: Surface::Core,
        },
        git_root: scratch.root().to_path_buf(),
        registry_origin: scratch.root().join("sources.toml"),
    };
    let discovered = discover_source(&context).expect("docs fixture should discover");
    (scratch, discovered)
}

fn compiled_records() -> (ScratchDirectory, Vec<DocumentRecord>) {
    let (scratch, snapshot) = snapshot();
    let records = compile_snapshot(&snapshot, &FixedCommitTime)
        .expect("docs fixture should compile");
    (scratch, records)
}

#[test]
fn compile_filters_root_index_and_domain_outside_files() {
    let (_scratch, records) = compiled_records();
    let identities = records
        .iter()
        .map(|record| record.identity.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            "knowledge:alpha:INDEX",
            "knowledge:alpha:a-note",
            "knowledge:alpha:z-note",
            "knowledge:beta:INDEX",
            "knowledge:beta:no-title",
        ]
    );
    assert!(records.iter().all(|record| record.source_path != "INDEX.md"));
    assert!(records
        .iter()
        .all(|record| record.source_path != "outside.md"));
    assert_eq!(records[0].kind, "index");
    assert_eq!(records[0].domain_prefixes, ["alpha"]);
}

#[test]
fn compile_uses_first_h1_or_filename_stem_and_exact_embedding_text() {
    let (_scratch, records) = compiled_records();
    let alpha = records
        .iter()
        .find(|record| record.identity == "knowledge:alpha:a-note")
        .expect("alpha note should exist");
    assert_eq!(alpha.title, "Alpha Heading");
    assert_eq!(
        alpha.embedding_text(),
        "Alpha description\n\nalpha keyword second\n\n# Alpha Heading\nBody text."
    );

    let fallback = records
        .iter()
        .find(|record| record.identity == "knowledge:beta:no-title")
        .expect("fallback note should exist");
    assert_eq!(fallback.title, "no-title");
}

#[test]
fn encode_is_canonical_fixed_order_escaped_and_lf_terminated() {
    let (_scratch, records) = compiled_records();
    let bytes = encode_ndjson(&records).expect("compiled records should encode");
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
    assert!(!bytes.contains(&b'\r'));
    let text = String::from_utf8(bytes.clone()).expect("canonical docs should be UTF-8");
    let first = text.lines().next().expect("fixture should have one record");
    assert!(first.starts_with(
        "{\"schema\":\"knowledge-doc-v2\",\"identity\":\"knowledge:alpha:INDEX\",\"source\":\"knowledge\",\"domain\":\"alpha\",\"domain_prefixes\":[\"alpha\"],\"title\":"
    ));
    assert!(
        text.lines()
            .any(|line| line.contains("\\\"quote\\\"")),
        "fixture should exercise JSON escaping"
    );
    let expected = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/memex/docs/expected.ndjson"),
    )
    .expect("public expected docs stream should be readable");
    assert_eq!(bytes, expected);
    let decoded = decode_ndjson(&bytes).expect("canonical docs should decode");
    assert_eq!(decoded, records);
}

#[test]
fn source_hash_normalizes_crlf_and_lone_cr_without_changing_input() {
    let (scratch, snapshot) = snapshot();
    let before = snapshot.clone();
    let crlf_path = scratch.root().join("vault/alpha/a-note.md");
    let original = fs::read(&crlf_path).expect("fixture note should be readable");
    let crlf = String::from_utf8(original)
        .expect("fixture note should be UTF-8")
        .replace('\n', "\r\n")
        .into_bytes();
    fs::write(&crlf_path, &crlf).expect("CRLF fixture note should be written");
    let changed = discover_source(&SourceContext {
        source: SourceSpec {
            name: SourceName::new("knowledge").unwrap(),
            root: scratch.root().join("vault"),
            surface: Surface::Core,
        },
        git_root: scratch.root().to_path_buf(),
        registry_origin: scratch.root().join("sources.toml"),
    })
    .expect("CRLF note should remain valid");
    let records = compile_snapshot(&changed, &FixedCommitTime).expect("CRLF note should compile");
    let record = records
        .iter()
        .find(|record| record.identity == "knowledge:alpha:a-note")
        .unwrap();
    let lf_source_hash = {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(
            String::from_utf8(crlf)
                .unwrap()
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .as_bytes(),
        );
        let mut hex = String::with_capacity(64);
        for byte in digest.finalize() {
            use std::fmt::Write as _;
            write!(hex, "{byte:02x}").unwrap();
        }
        hex
    };
    assert_eq!(record.source_hash, lf_source_hash);
    assert_eq!(snapshot, before);
}

#[test]
fn encode_does_not_mutate_records_and_is_deterministic() {
    let (_scratch, mut records) = compiled_records();
    records.reverse();
    let before = records.clone();
    let first = encode_ndjson(&records).expect("records should encode");
    let second = encode_ndjson(&records).expect("same records should encode identically");
    assert_eq!(first, second);
    assert_eq!(records, before);
}

#[test]
fn duplicate_identity_is_rejected_by_encode_and_decode() {
    let (_scratch, records) = compiled_records();
    let mut duplicate = records.clone();
    duplicate.push(records[0].clone());
    assert!(matches!(
        encode_ndjson(&duplicate),
        Err(MemexError::DuplicateIdentity { .. })
    ));

    let bytes = encode_ndjson(&records).unwrap();
    let first_line_end = bytes.iter().position(|byte| *byte == b'\n').unwrap();
    let mut duplicate_bytes = bytes[..first_line_end + 1].to_vec();
    duplicate_bytes.extend_from_slice(&bytes[..first_line_end + 1]);
    duplicate_bytes.extend_from_slice(&bytes[first_line_end + 1..]);
    assert!(matches!(
        decode_ndjson(&duplicate_bytes),
        Err(MemexError::DuplicateIdentity { .. })
    ));
}

#[test]
fn decode_rejects_invalid_schema_hash_path_and_noncanonical_ndjson() {
    let (_scratch, records) = compiled_records();
    let bytes = encode_ndjson(&records).unwrap();

    let bad_schema = bytes
        .windows(b"knowledge-doc-v2".len())
        .position(|window| window == b"knowledge-doc-v2")
        .map(|position| {
            let mut changed = bytes.clone();
            changed[position..position + b"knowledge-doc-v2".len()]
                .copy_from_slice(b"wrong-doc-schema");
            changed
        })
        .unwrap();
    assert!(matches!(
        decode_ndjson(&bad_schema),
        Err(MemexError::InvalidSchema { .. })
    ));

    let source_hash_start = bytes
        .windows(b"source_hash\":\"".len())
        .position(|window| window == b"source_hash\":\"")
        .unwrap()
        + b"source_hash\":\"".len();
    let mut bad_hash = bytes.clone();
    bad_hash[source_hash_start..source_hash_start + 64].fill(b'x');
    assert!(matches!(
        decode_ndjson(&bad_hash),
        Err(MemexError::InvalidHash { .. })
    ));

    let bad_path = String::from_utf8(bytes.clone())
        .unwrap()
        .replacen(
            "\"source_path\":\"alpha/INDEX.md\"",
            "\"source_path\":\"../escape.md\"",
            1,
        )
        .into_bytes();
    assert!(matches!(
        decode_ndjson(&bad_path),
        Err(MemexError::InvalidPath { .. })
    ));

    let first_line_end = bytes.iter().position(|byte| *byte == b'\n').unwrap();
    let mut pretty = Vec::with_capacity(first_line_end + 2);
    pretty.push(b' ');
    pretty.extend_from_slice(&bytes[..first_line_end]);
    pretty.push(b'\n');
    assert!(matches!(
        decode_ndjson(&pretty),
        Err(MemexError::NonCanonicalNdjson { .. })
    ));
}

#[test]
fn document_record_exposes_schema_and_stable_hash_projection() {
    let (_scratch, records) = compiled_records();
    let record = &records[0];
    assert_eq!(record.schema, "knowledge-doc-v2");
    assert_eq!(record.compiled_hash.len(), 64);
    assert!(record.compiled_hash.chars().all(|character| character.is_ascii_hexdigit()));
    let _: &str = record.schema;
}
fn recompute_compiled_hash(record: &DocumentRecord) -> String {
    let json = |value: &str| serde_json::to_string(value).unwrap();
    let json_list = |value: &[String]| serde_json::to_string(value).unwrap();
    let json_option = |value: &Option<String>| serde_json::to_string(value).unwrap();
    let projection = format!(
        "{{\"schema\":{},\"identity\":{},\"source\":{},\"domain\":{},\"domain_prefixes\":{},\"title\":{},\"description\":{},\"keywords\":{},\"kind\":{},\"kind_explicit\":{},\"status\":{},\"body_text\":{},\"source_path\":{},\"source_hash\":{},\"commit_time\":{}}}",
        json(record.schema),
        json(&record.identity),
        json(&record.source),
        json(&record.domain),
        json_list(&record.domain_prefixes),
        json(&record.title),
        json(&record.description),
        json_list(&record.keywords),
        json(&record.kind),
        record.kind_explicit,
        json_option(&record.status),
        json(&record.body_text),
        json(&record.source_path),
        json(&record.source_hash),
        json_option(&record.commit_time),
    );
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(projection.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}

#[test]
fn decode_rejects_domain_prefixes_that_are_not_cumulative_domain_prefixes() {
    let (_scratch, records) = compiled_records();
    let mut malformed = records[0].clone();
    malformed.domain_prefixes = vec!["not-alpha".to_owned()];
    malformed.compiled_hash = recompute_compiled_hash(&malformed);
    let original_hash = records[0].compiled_hash.clone();
    let bytes = String::from_utf8(encode_ndjson(&records).unwrap())
        .unwrap()
        .replacen(
            "\"domain_prefixes\":[\"alpha\"]",
            "\"domain_prefixes\":[\"not-alpha\"]",
            1,
        )
        .replacen(
            &format!("\"compiled_hash\":\"{original_hash}\""),
            &format!("\"compiled_hash\":\"{}\"", malformed.compiled_hash),
            1,
        )
        .into_bytes();
    assert!(matches!(
        decode_ndjson(&bytes),
        Err(MemexError::InvalidDocument {
            field: "domain_prefixes",
            ..
        })
    ));
}
