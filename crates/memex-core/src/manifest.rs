use crate::error::MemexError;
use crate::tantivy_schema::INDEX_PROFILE;
use serde_json::{Map, Value};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Stable schema identifier for an immutable Tantivy generation.
pub const GENERATION_SCHEMA: &str = "tantivy-generation-v2";
/// Stable contract version included in every generation id frame.
pub const GENERATION_CONTRACT_VERSION: &str = "tantivy-generation-v2";

const MANIFEST_FIELDS: [&str; 6] = [
    "schema",
    "id",
    "docs_sha256",
    "doc_count",
    "contract_version",
    "index_profile",
];

/// The content-addressed identifier of one immutable generation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GenerationId(String);

impl GenerationId {
    /// Parse a generation id, accepting only a lowercase SHA-256 digest.
    pub fn parse(value: &str) -> Result<Self, MemexError> {
        validate_digest(value, "id")?;
        Ok(Self(value.to_owned()))
    }

    /// Return the canonical lowercase hexadecimal identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_digest(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for GenerationId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for GenerationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for GenerationId {
    type Err = MemexError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Strict metadata binding a docs stream to its Tantivy projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationManifest {
    pub schema: String,
    pub id: GenerationId,
    pub docs_sha256: String,
    pub doc_count: u64,
    pub contract_version: String,
    pub index_profile: String,
}

impl GenerationManifest {
    pub(crate) fn new(id: GenerationId, docs_sha256: String, doc_count: u64) -> Self {
        Self {
            schema: GENERATION_SCHEMA.to_owned(),
            id,
            docs_sha256,
            doc_count,
            contract_version: GENERATION_CONTRACT_VERSION.to_owned(),
            index_profile: INDEX_PROFILE.to_owned(),
        }
    }
}

/// Encode one manifest in its canonical compact JSON representation.
pub fn encode_manifest(manifest: &GenerationManifest) -> Result<Vec<u8>, MemexError> {
    validate_manifest(manifest)?;
    let mut output = Vec::with_capacity(256);
    output.push(b'{');
    push_string_field(&mut output, "schema", &manifest.schema)?;
    output.push(b',');
    push_string_field(&mut output, "id", manifest.id.as_str())?;
    output.push(b',');
    push_string_field(&mut output, "docs_sha256", &manifest.docs_sha256)?;
    output.push(b',');
    output.extend_from_slice(b"\"doc_count\":");
    output.extend_from_slice(manifest.doc_count.to_string().as_bytes());
    output.push(b',');
    push_string_field(
        &mut output,
        "contract_version",
        &manifest.contract_version,
    )?;
    output.push(b',');
    push_string_field(&mut output, "index_profile", &manifest.index_profile)?;
    output.extend_from_slice(b"}\n");
    Ok(output)
}

/// Decode one canonical manifest, rejecting all alternate spellings.
pub fn decode_manifest(bytes: &[u8]) -> Result<GenerationManifest, MemexError> {
    if bytes.len() < 2 || !bytes.ends_with(b"\n") {
        return Err(manifest_error("manifest must end with exactly one LF"));
    }
    let content = &bytes[..bytes.len() - 1];
    if content.is_empty()
        || content.ends_with(b"\n")
        || content.contains(&b'\r')
        || content.first().is_some_and(u8::is_ascii_whitespace)
        || content.last().is_some_and(u8::is_ascii_whitespace)
    {
        return Err(manifest_error("manifest contains non-canonical whitespace"));
    }

    let value: Value = serde_json::from_slice(content)
        .map_err(|_| manifest_error("manifest is not valid JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| manifest_error("manifest must be a JSON object"))?;
    if object.len() != MANIFEST_FIELDS.len()
        || MANIFEST_FIELDS
            .iter()
            .any(|field| !object.contains_key(*field))
    {
        return Err(manifest_error("manifest fields do not match the v2 protocol"));
    }

    let schema = required_string(object, "schema")?;
    let id = GenerationId::parse(&required_string(object, "id")?)?;
    let docs_sha256 = required_string(object, "docs_sha256")?;
    validate_digest(&docs_sha256, "docs_sha256")?;
    let doc_count = object
        .get("doc_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| manifest_error("doc_count must be an unsigned integer"))?;
    let contract_version = required_string(object, "contract_version")?;
    let index_profile = required_string(object, "index_profile")?;
    let manifest = GenerationManifest {
        schema,
        id,
        docs_sha256,
        doc_count,
        contract_version,
        index_profile,
    };
    let canonical = encode_manifest(&manifest)?;
    if canonical != bytes {
        return Err(manifest_error("manifest is not canonical"));
    }
    Ok(manifest)
}

pub(crate) fn decode_manifest_at(
    path: &Path,
    bytes: &[u8],
) -> Result<GenerationManifest, MemexError> {
    decode_manifest(bytes).map_err(|error| match error {
        MemexError::InvalidManifest { message, .. } => MemexError::InvalidManifest {
            path: path.to_path_buf(),
            message,
        },
        other => other,
    })
}

fn validate_manifest(manifest: &GenerationManifest) -> Result<(), MemexError> {
    if manifest.schema != GENERATION_SCHEMA {
        return Err(manifest_error("schema is not tantivy-generation-v2"));
    }
    if manifest.contract_version != GENERATION_CONTRACT_VERSION {
        return Err(manifest_error("contract_version is not tantivy-generation-v2"));
    }
    if manifest.index_profile != INDEX_PROFILE {
        return Err(manifest_error("index_profile is not tantivy-central-v2"));
    }
    validate_digest(manifest.id.as_str(), "id")?;
    validate_digest(&manifest.docs_sha256, "docs_sha256")?;
    Ok(())
}

fn validate_digest(value: &str, field: &'static str) -> Result<(), MemexError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(manifest_error(format!("{field} must be a lowercase SHA-256 digest")));
    }
    Ok(())
}

fn push_string_field(output: &mut Vec<u8>, name: &str, value: &str) -> Result<(), MemexError> {
    let encoded = serde_json::to_vec(value)
        .map_err(|_| manifest_error("manifest string cannot be encoded as JSON"))?;
    output.push(b'"');
    output.extend_from_slice(name.as_bytes());
    output.extend_from_slice(b"\":");
    output.extend_from_slice(&encoded);
    Ok(())
}

fn required_string(object: &Map<String, Value>, field: &'static str) -> Result<String, MemexError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| manifest_error(format!("{field} must be a string")))
}

fn manifest_error(message: impl Into<String>) -> MemexError {
    MemexError::InvalidManifest {
        path: PathBuf::from("manifest.json"),
        message: message.into(),
    }
}
