use serde_json::Value;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "frozen-ledger-v2";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerRecord {
    pub schema: String,
    pub operation: String,
    pub logical_source: String,
    pub old_identity: String,
    pub new_identity: String,
    pub old_path: String,
    pub new_path: String,
    pub head_oid: String,
    pub canonical_git_blob_sha256: String,
    pub reason: String,
}

#[derive(Debug)]
pub enum LedgerError {
    Io(PathBuf),
    Invalid,
}
impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => f.write_str("could not access frozen ledger"),
            Self::Invalid => f.write_str("frozen ledger is invalid"),
        }
    }
}
impl std::error::Error for LedgerError {}

impl LedgerRecord {
    pub(crate) fn validate(&self) -> Result<(), LedgerError> {
        if self.schema != SCHEMA
            || !matches!(self.operation.as_str(), "relocate" | "amend")
            || self.logical_source.is_empty()
            || self.old_identity.is_empty()
            || self.new_identity.is_empty()
            || self.old_path.is_empty()
            || self.new_path.is_empty()
            || !valid_git_oid(&self.head_oid)
            || self.canonical_git_blob_sha256.len() != 64
            || !self
                .canonical_git_blob_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.reason.is_empty()
            || self.reason.chars().any(char::is_control)
            || !valid_rel_path(&self.old_path)
            || !valid_rel_path(&self.new_path)
        {
            return Err(LedgerError::Invalid);
        }
        if self.operation == "amend"
            && (self.old_identity != self.new_identity || self.old_path != self.new_path)
        {
            return Err(LedgerError::Invalid);
        }
        Ok(())
    }
    pub(crate) fn line(&self) -> Result<String, LedgerError> {
        self.validate()?;
        fn q(value: &str) -> String {
            serde_json::to_string(value).expect("string serialization cannot fail")
        }
        Ok(format!(
            "{{\"schema\":{},\"operation\":{},\"logical_source\":{},\"old_identity\":{},\"new_identity\":{},\"old_path\":{},\"new_path\":{},\"head_oid\":{},\"canonical_git_blob_sha256\":{},\"reason\":{}}}\n",
            q(&self.schema),
            q(&self.operation),
            q(&self.logical_source),
            q(&self.old_identity),
            q(&self.new_identity),
            q(&self.old_path),
            q(&self.new_path),
            q(&self.head_oid),
            q(&self.canonical_git_blob_sha256),
            q(&self.reason)
        ))
    }
}

pub(crate) fn append(root: &Path, record: &LedgerRecord) -> Result<(), LedgerError> {
    record.validate()?;
    let path = safe_ledger_path(root, &record.operation, true)?;
    let mut file = crate::source::open_regular_file_for_append_nofollow(&path)
        .map_err(|_| LedgerError::Io(path.clone()))?;
    file.write_all(record.line()?.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| LedgerError::Io(path))
}
pub(crate) fn read(root: &Path, operation: &str) -> Result<Vec<LedgerRecord>, LedgerError> {
    const MAX_LEDGER_BYTES: usize = 16 * 1024 * 1024;
    let directory = root.join(".rhizome");
    if matches!(fs::symlink_metadata(&directory), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(Vec::new());
    }
    let path = safe_ledger_path(root, operation, false)?;
    let bytes = match crate::source::read_regular_file_nofollow_bounded(&path, MAX_LEDGER_BYTES) {
        Ok(v) => v,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(LedgerError::Io(path)),
    };
    parse_bytes(&bytes, operation)
}

pub(crate) fn parse_bytes(bytes: &[u8], operation: &str) -> Result<Vec<LedgerRecord>, LedgerError> {
    const MAX_LEDGER_BYTES: usize = 16 * 1024 * 1024;
    if bytes.is_empty() || bytes.len() > MAX_LEDGER_BYTES || !bytes.ends_with(b"\n") {
        return Err(LedgerError::Invalid);
    }
    let mut records = Vec::new();
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    for (index, line) in lines.iter().enumerate() {
        if index + 1 == lines.len() && line.is_empty() {
            continue;
        }
        let line = *line;
        if line.is_empty() || line.iter().all(u8::is_ascii_whitespace) || line.contains(&b'\r') {
            return Err(LedgerError::Invalid);
        }
        let value: Value = serde_json::from_slice(line).map_err(|_| LedgerError::Invalid)?;
        let object = value.as_object().ok_or(LedgerError::Invalid)?;
        const KEYS: [&str; 10] = [
            "schema",
            "operation",
            "logical_source",
            "old_identity",
            "new_identity",
            "old_path",
            "new_path",
            "head_oid",
            "canonical_git_blob_sha256",
            "reason",
        ];
        if object.len() != KEYS.len() || !KEYS.iter().all(|k| object.contains_key(*k)) {
            return Err(LedgerError::Invalid);
        }
        let s = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(LedgerError::Invalid)
        };
        let record = LedgerRecord {
            schema: s("schema")?,
            operation: s("operation")?,
            logical_source: s("logical_source")?,
            old_identity: s("old_identity")?,
            new_identity: s("new_identity")?,
            old_path: s("old_path")?,
            new_path: s("new_path")?,
            head_oid: s("head_oid")?,
            canonical_git_blob_sha256: s("canonical_git_blob_sha256")?,
            reason: s("reason")?,
        };
        record.validate()?;
        if record.operation != operation {
            return Err(LedgerError::Invalid);
        }
        if record.line()?.as_bytes().strip_suffix(b"\n") != Some(line) {
            return Err(LedgerError::Invalid);
        }
        records.push(record);
    }
    Ok(records)
}

fn safe_ledger_path(
    root: &Path,
    operation: &str,
    create_directory: bool,
) -> Result<PathBuf, LedgerError> {
    let directory = root.join(".rhizome");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(LedgerError::Io(directory));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_directory => {
            fs::create_dir(&directory).map_err(|_| LedgerError::Io(directory.clone()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(LedgerError::Io(directory));
        }
        Err(_) => return Err(LedgerError::Io(directory)),
    }
    let name = match operation {
        "relocate" => "relocate-ledger.ndjson",
        "amend" => "amend-ledger.ndjson",
        _ => return Err(LedgerError::Invalid),
    };
    let path = directory.join(name);
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LedgerError::Io(path));
        }
    }
    Ok(path)
}

fn valid_git_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn valid_rel_path(value: &str) -> bool {
    !value.starts_with('/')
        && !value.contains('\\')
        && !value.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.chars().any(char::is_control)
        })
}
