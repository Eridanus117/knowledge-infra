use crate::document::{
    DOCUMENT_SCHEMA, DocumentRecord, canonical_record_json, compiled_projection_json, sha256_hex,
};
use crate::error::MemexError;
use serde_json::{Map, Value};
use std::collections::HashSet;

const FIELD_NAMES: [&str; 16] = [
    "schema",
    "identity",
    "source",
    "domain",
    "domain_prefixes",
    "title",
    "description",
    "keywords",
    "kind",
    "kind_explicit",
    "status",
    "body_text",
    "source_path",
    "source_hash",
    "compiled_hash",
    "commit_time",
];

/// Encode records as canonical identity-sorted NDJSON bytes.
pub fn encode_ndjson(records: &[DocumentRecord]) -> Result<Vec<u8>, MemexError> {
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.identity.cmp(&right.identity));

    let mut identities = HashSet::with_capacity(ordered.len());
    for record in &ordered {
        if !identities.insert(record.identity.as_str()) {
            return Err(MemexError::DuplicateIdentity {
                identity: record.identity.clone(),
            });
        }
        validate_record(record, 0)?;
    }

    let mut output = Vec::new();
    for record in ordered {
        output.extend_from_slice(&canonical_record_json(record));
        output.push(b'\n');
    }
    Ok(output)
}

/// Decode canonical docs NDJSON, rejecting malformed, stale, reordered, or
/// non-canonical records.
pub fn decode_ndjson(bytes: &[u8]) -> Result<Vec<DocumentRecord>, MemexError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(b"\n") {
        return Err(MemexError::NonCanonicalNdjson {
            line: line_count(bytes),
        });
    }
    let content = &bytes[..bytes.len() - 1];
    if content.is_empty() {
        return Err(MemexError::InvalidNdjson {
            line: 1,
            message: "blank records are not allowed",
        });
    }
    if content.ends_with(b"\n") {
        return Err(MemexError::NonCanonicalNdjson {
            line: line_count(bytes),
        });
    }

    let mut records = Vec::new();
    let mut identities = HashSet::new();
    let mut previous_identity: Option<String> = None;
    for (index, line) in content.split(|byte| *byte == b'\n').enumerate() {
        let line_number = index + 1;
        if line.is_empty() {
            return Err(MemexError::InvalidNdjson {
                line: line_number,
                message: "blank records are not allowed",
            });
        }
        if line.contains(&b'\r')
            || line.first().is_some_and(u8::is_ascii_whitespace)
            || line.last().is_some_and(u8::is_ascii_whitespace)
        {
            return Err(MemexError::NonCanonicalNdjson { line: line_number });
        }

        let value: Value = serde_json::from_slice(line).map_err(|_| MemexError::InvalidNdjson {
            line: line_number,
            message: "record is not valid JSON",
        })?;
        let record = parse_record(&value, line_number)?;
        validate_record(&record, line_number)?;
        if canonical_record_json(&record) != line {
            return Err(MemexError::NonCanonicalNdjson { line: line_number });
        }
        if !identities.insert(record.identity.clone()) {
            return Err(MemexError::DuplicateIdentity {
                identity: record.identity,
            });
        }
        if let Some(previous) = previous_identity.as_deref() {
            if previous >= record.identity.as_str() {
                if previous == record.identity {
                    return Err(MemexError::DuplicateIdentity {
                        identity: record.identity,
                    });
                }
                return Err(MemexError::NonCanonicalNdjson { line: line_number });
            }
        }
        previous_identity = Some(record.identity.clone());
        records.push(record);
    }
    Ok(records)
}

fn parse_record(value: &Value, line: usize) -> Result<DocumentRecord, MemexError> {
    let object = value.as_object().ok_or(MemexError::InvalidNdjson {
        line,
        message: "record must be a JSON object",
    })?;
    if object.len() != FIELD_NAMES.len()
        || FIELD_NAMES.iter().any(|name| !object.contains_key(*name))
    {
        return Err(MemexError::InvalidNdjson {
            line,
            message: "record fields do not match the protocol",
        });
    }

    let schema = required_string(object, "schema", line)?;
    if schema != DOCUMENT_SCHEMA {
        return Err(MemexError::InvalidSchema {
            line,
            actual: schema,
        });
    }

    Ok(DocumentRecord {
        schema: DOCUMENT_SCHEMA,
        identity: required_string(object, "identity", line)?,
        source: required_string(object, "source", line)?,
        domain: required_string(object, "domain", line)?,
        domain_prefixes: required_strings(object, "domain_prefixes", line)?,
        title: required_string(object, "title", line)?,
        description: required_string(object, "description", line)?,
        keywords: required_strings(object, "keywords", line)?,
        kind: required_string(object, "kind", line)?,
        kind_explicit: required_bool(object, "kind_explicit", line)?,
        status: optional_string(object, "status", line)?,
        body_text: required_string(object, "body_text", line)?,
        source_path: required_string(object, "source_path", line)?,
        source_hash: required_string(object, "source_hash", line)?,
        compiled_hash: required_string(object, "compiled_hash", line)?,
        commit_time: optional_string(object, "commit_time", line)?,
    })
}

fn validate_record(record: &DocumentRecord, line: usize) -> Result<(), MemexError> {
    if record.schema != DOCUMENT_SCHEMA {
        return Err(MemexError::InvalidSchema {
            line,
            actual: record.schema.to_owned(),
        });
    }
    if record.identity.is_empty() || record.identity.chars().any(char::is_control) {
        return Err(invalid_document(
            line,
            "identity",
            "identity must be non-empty and one-line",
        ));
    }
    if record.source.is_empty() || record.source.chars().any(char::is_control) {
        return Err(invalid_document(
            line,
            "source",
            "source must be non-empty and one-line",
        ));
    }
    if record.domain.is_empty() || record.domain.chars().any(char::is_control) {
        return Err(invalid_document(
            line,
            "domain",
            "domain must be non-empty and one-line",
        ));
    }
    if record.domain_prefixes.is_empty()
        || record
            .domain_prefixes
            .iter()
            .any(|prefix| prefix.is_empty() || prefix.chars().any(char::is_control))
    {
        return Err(invalid_document(
            line,
            "domain_prefixes",
            "domain prefixes must be non-empty strings",
        ));
    }
    if record.domain_prefixes != cumulative_domain_prefixes(&record.domain) {
        return Err(invalid_document(
            line,
            "domain_prefixes",
            "domain prefixes must be the cumulative prefixes of domain",
        ));
    }
    if record.title.is_empty() {
        return Err(invalid_document(line, "title", "title must be non-empty"));
    }
    if record.description.trim().is_empty() {
        return Err(invalid_document(
            line,
            "description",
            "description must be non-empty",
        ));
    }
    if record.keywords.is_empty()
        || record
            .keywords
            .iter()
            .any(|keyword| keyword.trim().is_empty())
    {
        return Err(invalid_document(
            line,
            "keywords",
            "keywords must contain non-empty strings",
        ));
    }
    if !matches!(
        record.kind.as_str(),
        "spec" | "reference" | "runbook" | "decision" | "research" | "note" | "index"
    ) {
        return Err(invalid_document(
            line,
            "kind",
            "kind is not a known note kind",
        ));
    }
    if let Some(status) = &record.status {
        if status != "frozen" {
            return Err(invalid_document(
                line,
                "status",
                "status is not a known note status",
            ));
        }
    }
    if let Some(commit_time) = &record.commit_time {
        if commit_time.is_empty() || commit_time.chars().any(char::is_control) {
            return Err(invalid_document(
                line,
                "commit_time",
                "commit time must be non-empty and one-line",
            ));
        }
    }
    validate_source_path(&record.source_path, line)?;
    validate_hash(&record.source_hash, "source_hash", line)?;
    validate_hash(&record.compiled_hash, "compiled_hash", line)?;
    if sha256_hex(&compiled_projection_json(record)) != record.compiled_hash {
        return Err(MemexError::InvalidHash {
            line,
            field: "compiled_hash",
        });
    }
    Ok(())
}

fn validate_source_path(path: &str, line: usize) -> Result<(), MemexError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.chars().any(char::is_control)
    {
        return Err(MemexError::InvalidPath {
            line,
            path: path.to_owned(),
        });
    }
    let mut segments = path.split('/');
    let mut last = None;
    for segment in &mut segments {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(MemexError::InvalidPath {
                line,
                path: path.to_owned(),
            });
        }
        last = Some(segment);
    }
    let Some(last) = last else {
        return Err(MemexError::InvalidPath {
            line,
            path: path.to_owned(),
        });
    };
    if !last.ends_with(".md") || last == ".md" {
        return Err(MemexError::InvalidPath {
            line,
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn validate_hash(value: &str, field: &'static str, line: usize) -> Result<(), MemexError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(MemexError::InvalidHash { line, field });
    }
    Ok(())
}

fn invalid_document(line: usize, field: &'static str, message: &'static str) -> MemexError {
    MemexError::InvalidDocument {
        line: (line != 0).then_some(line),
        field,
        message,
    }
}

fn required_value<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
    line: usize,
) -> Result<&'a Value, MemexError> {
    object.get(field).ok_or(MemexError::InvalidNdjson {
        line,
        message: "record fields do not match the protocol",
    })
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
    line: usize,
) -> Result<String, MemexError> {
    required_value(object, field, line)?
        .as_str()
        .map(str::to_owned)
        .ok_or(MemexError::InvalidNdjson {
            line,
            message: "record field has the wrong JSON type",
        })
}

fn required_strings(
    object: &Map<String, Value>,
    field: &'static str,
    line: usize,
) -> Result<Vec<String>, MemexError> {
    let Some(values) = required_value(object, field, line)?.as_array() else {
        return Err(MemexError::InvalidNdjson {
            line,
            message: "record field has the wrong JSON type",
        });
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(MemexError::InvalidNdjson {
                    line,
                    message: "record list item has the wrong JSON type",
                })
        })
        .collect()
}

fn required_bool(
    object: &Map<String, Value>,
    field: &'static str,
    line: usize,
) -> Result<bool, MemexError> {
    required_value(object, field, line)?
        .as_bool()
        .ok_or(MemexError::InvalidNdjson {
            line,
            message: "record field has the wrong JSON type",
        })
}

fn optional_string(
    object: &Map<String, Value>,
    field: &'static str,
    line: usize,
) -> Result<Option<String>, MemexError> {
    let value = required_value(object, field, line)?;
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or(MemexError::InvalidNdjson {
            line,
            message: "record field has the wrong JSON type",
        })
}

fn line_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count().max(1)
}

fn cumulative_domain_prefixes(domain: &str) -> Vec<String> {
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
