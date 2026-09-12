use crate::error::MemexError;
use kb_contract::{NoteKind, NoteStatus};
use rhizome_core::SourceSnapshot;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Component, Path};

/// Versioned compiled-document schema identifier.
pub const DOCUMENT_SCHEMA: &str = "knowledge-doc-v2";

/// A source-contract note compiled into the retrieval document protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentRecord {
    pub schema: &'static str,
    pub identity: String,
    pub source: String,
    pub domain: String,
    pub domain_prefixes: Vec<String>,
    pub title: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub kind: String,
    pub kind_explicit: bool,
    pub status: Option<String>,
    pub body_text: String,
    pub source_path: String,
    pub source_hash: String,
    pub compiled_hash: String,
    pub commit_time: Option<String>,
}

/// Supplies Git commit metadata without coupling compilation to a subprocess
/// or to a particular Git implementation.
pub trait CommitTimeSource {
    fn commit_time(&self, path: &Path) -> Result<Option<String>, MemexError>;
}

/// Compile all validated notes in a source snapshot into identity-sorted
/// deterministic documents.
pub fn compile_snapshot(
    snapshot: &SourceSnapshot,
    git: &dyn CommitTimeSource,
) -> Result<Vec<DocumentRecord>, MemexError> {
    let source_root = snapshot.source_root();
    let source = snapshot.source.as_str().to_owned();
    let mut records = Vec::with_capacity(snapshot.notes.len());

    for snapshot_note in &snapshot.notes {
        let locator = &snapshot_note.locator;
        let note = &snapshot_note.note;
        let source_path = source_relative_path(source_root, &locator.path)?;
        let original = std::str::from_utf8(&note.original).map_err(|_| MemexError::InvalidDocument {
            line: None,
            field: "source_hash",
            message: "source bytes must be valid UTF-8",
        })?;
        let body = std::str::from_utf8(&note.body).map_err(|_| MemexError::InvalidDocument {
            line: None,
            field: "body_text",
            message: "body bytes must be valid UTF-8",
        })?;
        let body_text = normalize_lf(body).trim_matches('\n').to_owned();
        let title = title_from_body(&body_text, &locator.path)?;
        let domain = locator.domain.as_str().to_owned();
        let domain_prefixes = domain_prefixes(&domain);
        let frontmatter = &note.frontmatter;
        let status = frontmatter.status.map(note_status);
        let commit_time = git.commit_time(&locator.path)?;
        let mut record = DocumentRecord {
            schema: DOCUMENT_SCHEMA,
            identity: locator.identity.as_str().to_owned(),
            source: source.clone(),
            domain,
            domain_prefixes,
            title,
            description: frontmatter.description.clone(),
            keywords: frontmatter.keywords.clone(),
            kind: note_kind(frontmatter.kind).to_owned(),
            kind_explicit: note.kind_explicit,
            status,
            body_text,
            source_path,
            source_hash: sha256_hex(normalize_lf(original).as_bytes()),
            compiled_hash: String::new(),
            commit_time,
        };
        record.compiled_hash = sha256_hex(&compiled_projection_json(&record));
        records.push(record);
    }

    records.sort_by(|left, right| left.identity.cmp(&right.identity));
    for pair in records.windows(2) {
        if pair[0].identity == pair[1].identity {
            return Err(MemexError::DuplicateIdentity {
                identity: pair[0].identity.clone(),
            });
        }
    }
    Ok(records)
}

/// Build the exact text submitted to an embedding provider. Titles and Git
/// metadata are intentionally absent from this text.
#[must_use]
pub fn embedding_text(description: &str, keywords: &[String], body: &str) -> String {
    let keyword_text = keywords.join(" ");
    [description.trim(), keyword_text.trim(), body.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl DocumentRecord {
    /// Return the deterministic embedding input for this document.
    #[must_use]
    pub fn embedding_text(&self) -> String {
        embedding_text(&self.description, &self.keywords, &self.body_text)
    }
}

pub(crate) fn compiled_projection_json(record: &DocumentRecord) -> Vec<u8> {
    let mut output = Vec::new();
    output.push(b'{');
    push_json_string_field(&mut output, "schema", record.schema);
    output.push(b',');
    push_json_string_field(&mut output, "identity", &record.identity);
    output.push(b',');
    push_json_string_field(&mut output, "source", &record.source);
    output.push(b',');
    push_json_string_field(&mut output, "domain", &record.domain);
    output.push(b',');
    push_json_strings_field(&mut output, "domain_prefixes", &record.domain_prefixes);
    output.push(b',');
    push_json_string_field(&mut output, "title", &record.title);
    output.push(b',');
    push_json_string_field(&mut output, "description", &record.description);
    output.push(b',');
    push_json_strings_field(&mut output, "keywords", &record.keywords);
    output.push(b',');
    push_json_string_field(&mut output, "kind", &record.kind);
    output.push(b',');
    push_json_bool_field(&mut output, "kind_explicit", record.kind_explicit);
    output.push(b',');
    push_json_option_field(&mut output, "status", &record.status);
    output.push(b',');
    push_json_string_field(&mut output, "body_text", &record.body_text);
    output.push(b',');
    push_json_string_field(&mut output, "source_path", &record.source_path);
    output.push(b',');
    push_json_string_field(&mut output, "source_hash", &record.source_hash);
    output.push(b',');
    push_json_option_field(&mut output, "commit_time", &record.commit_time);
    output.push(b'}');
    output
}

pub(crate) fn canonical_record_json(record: &DocumentRecord) -> Vec<u8> {
    let mut output = Vec::new();
    output.push(b'{');
    push_json_string_field(&mut output, "schema", record.schema);
    output.push(b',');
    push_json_string_field(&mut output, "identity", &record.identity);
    output.push(b',');
    push_json_string_field(&mut output, "source", &record.source);
    output.push(b',');
    push_json_string_field(&mut output, "domain", &record.domain);
    output.push(b',');
    push_json_strings_field(&mut output, "domain_prefixes", &record.domain_prefixes);
    output.push(b',');
    push_json_string_field(&mut output, "title", &record.title);
    output.push(b',');
    push_json_string_field(&mut output, "description", &record.description);
    output.push(b',');
    push_json_strings_field(&mut output, "keywords", &record.keywords);
    output.push(b',');
    push_json_string_field(&mut output, "kind", &record.kind);
    output.push(b',');
    push_json_bool_field(&mut output, "kind_explicit", record.kind_explicit);
    output.push(b',');
    push_json_option_field(&mut output, "status", &record.status);
    output.push(b',');
    push_json_string_field(&mut output, "body_text", &record.body_text);
    output.push(b',');
    push_json_string_field(&mut output, "source_path", &record.source_path);
    output.push(b',');
    push_json_string_field(&mut output, "source_hash", &record.source_hash);
    output.push(b',');
    push_json_string_field(&mut output, "compiled_hash", &record.compiled_hash);
    output.push(b',');
    push_json_option_field(&mut output, "commit_time", &record.commit_time);
    output.push(b'}');
    output
}

fn source_relative_path(root: &Path, path: &Path) -> Result<String, MemexError> {
    let relative = path.strip_prefix(root).map_err(|_| MemexError::InvalidPath {
        line: 0,
        path: path.display().to_string(),
    })?;
    let mut output = String::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(MemexError::InvalidPath {
                line: 0,
                path: path.display().to_string(),
            });
        };
        let component = component.to_str().ok_or_else(|| MemexError::InvalidPath {
            line: 0,
            path: path.display().to_string(),
        })?;
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.contains('\\')
            || component.contains(':')
            || component.chars().any(char::is_control)
        {
            return Err(MemexError::InvalidPath {
                line: 0,
                path: path.display().to_string(),
            });
        }
        if !output.is_empty() {
            output.push('/');
        }
        output.push_str(component);
    }
    if output.is_empty() {
        return Err(MemexError::InvalidPath {
            line: 0,
            path: path.display().to_string(),
        });
    }
    Ok(output)
}

fn title_from_body(body: &str, path: &Path) -> Result<String, MemexError> {
    for line in body.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let title = rest.trim();
        if !title.is_empty() {
            return Ok(title.to_owned());
        }
    }
    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| MemexError::InvalidPath {
            line: 0,
            path: path.display().to_string(),
        })?;
    Ok(title.to_owned())
}

fn domain_prefixes(domain: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut current = String::new();
    for segment in domain.split('/') {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        prefixes.push(current.clone());
    }
    prefixes
}

fn note_kind(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Spec => "spec",
        NoteKind::Reference => "reference",
        NoteKind::Runbook => "runbook",
        NoteKind::Decision => "decision",
        NoteKind::Research => "research",
        NoteKind::Note => "note",
        NoteKind::Index => "index",
    }
}

fn note_status(status: NoteStatus) -> String {
    match status {
        NoteStatus::Frozen => "frozen".to_owned(),
    }
}

fn normalize_lf(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn push_json_string_field(output: &mut Vec<u8>, name: &str, value: &str) {
    push_json_name(output, name);
    output.extend_from_slice(
        &serde_json::to_vec(value).expect("serializing a string into JSON cannot fail"),
    );
}

fn push_json_strings_field(output: &mut Vec<u8>, name: &str, value: &[String]) {
    push_json_name(output, name);
    output.extend_from_slice(
        &serde_json::to_vec(value).expect("serializing a string list into JSON cannot fail"),
    );
}

fn push_json_bool_field(output: &mut Vec<u8>, name: &str, value: bool) {
    push_json_name(output, name);
    output.extend_from_slice(if value { b"true" } else { b"false" });
}

fn push_json_option_field(output: &mut Vec<u8>, name: &str, value: &Option<String>) {
    push_json_name(output, name);
    output.extend_from_slice(
        &serde_json::to_vec(value).expect("serializing an option into JSON cannot fail"),
    );
}

fn push_json_name(output: &mut Vec<u8>, name: &str) {
    output.extend_from_slice(
        &serde_json::to_vec(name).expect("serializing a field name into JSON cannot fail"),
    );
    output.push(b':');
}
