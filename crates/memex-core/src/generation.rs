use crate::document::{DocumentRecord, sha256_hex};
use crate::error::MemexError;
use crate::lock::PublicationLock;
use crate::manifest::{
    GENERATION_CONTRACT_VERSION, GENERATION_SCHEMA, decode_manifest_at, encode_manifest,
};
pub use crate::manifest::{GenerationId, GenerationManifest};
use crate::tantivy_schema::{INDEX_PROFILE, build_schema, build_tantivy, register_analyzers};
use crate::{decode_ndjson, encode_ndjson};
use sha2::{Digest, Sha256};
use fs4::fs_std::FileExt;
use std::fs::{self, File, OpenOptions};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use tantivy::Index;

const CURRENT_FILENAME: &str = "CURRENT";
const GENERATIONS_DIRECTORY: &str = "generations";
const TEMP_GENERATION_PREFIX: &str = ".memex-generation-";
const TEMP_GENERATION_SUFFIX: &str = ".tmp";
const BUILD_LEASE_SUFFIX: &str = ".lock";
const TEMP_CURRENT_PREFIX: &str = ".memex-current-";
const TEMP_CURRENT_SUFFIX: &str = ".tmp";

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Root of the immutable generation store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexManager {
    pub root: PathBuf,
}

impl IndexManager {
    /// Construct a manager rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

/// A validated, immutable snapshot suitable for query consumers.
#[derive(Debug)]
pub struct GenerationReader {
    pub id: GenerationId,
    pub manifest: GenerationManifest,
    pub index: Index,
}

/// Build one immutable generation without changing `CURRENT`.
pub fn build_generation(
    manager: &IndexManager,
    records: &[DocumentRecord],
) -> Result<GenerationId, MemexError> {
    let docs = encode_ndjson(records)?;
    let id = generation_id(&docs);
    let manifest = GenerationManifest::new(id.clone(), sha256_hex(&docs), records.len() as u64);
    let generations = manager.root.join(GENERATIONS_DIRECTORY);
    fs::create_dir_all(&generations).map_err(|source| io_error(&generations, source))?;
    cleanup_interrupted_temps(&generations);

    let final_directory = generation_directory(manager, &id);
    if final_directory.exists() {
        let reader = validate_generation(manager, &id)?;
        let existing_docs = read_regular_file(&final_directory.join("docs.ndjson"))?;
        if existing_docs != docs {
            return Err(invalid_generation(
                &final_directory,
                "same generation id has different docs bytes",
            ));
        }
        sync_directory(&generations).map_err(|source| io_error(&generations, source))?;
        drop(reader);
        return Ok(id);
    }
    let temporary_directory = temporary_generation_directory(&generations, &id);
    let lease_path = temporary_lease_path(&temporary_directory);
    let lease = match BuildLease::try_acquire(&lease_path) {
        Ok(lease) => lease,
        Err(error) => {
            let _ = fs::remove_file(&lease_path);
            return Err(error);
        }
    };
    if let Err(source) = fs::create_dir(&temporary_directory) {
        drop(lease);
        let _ = fs::remove_file(&lease_path);
        return Err(io_error(&temporary_directory, source));
    }
    let build_result = build_temporary_generation(&temporary_directory, &docs, &manifest, records);
    if let Err(error) = build_result {
        drop(lease);
        let _ = fs::remove_dir_all(&temporary_directory);
        let _ = fs::remove_file(&lease_path);
        return Err(error);
    }

    // Keep the lease while renaming so cleanup in another builder cannot
    // remove this fully-built directory in the rename window.
    let rename_result = fs::rename(&temporary_directory, &final_directory);
    drop(lease);
    if let Err(source) = fs::remove_file(&lease_path)
        && source.kind() != std::io::ErrorKind::NotFound
    {
        let _ = fs::remove_dir_all(&temporary_directory);
        return Err(io_error(&lease_path, source));
    }
    match rename_result {
        Ok(()) => {
            sync_directory(&generations).map_err(|source| io_error(&generations, source))?;
            Ok(id)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_dir_all(&temporary_directory);
            let reader = validate_generation(manager, &id)?;
            let existing_docs = read_regular_file(&final_directory.join("docs.ndjson"))?;
            if existing_docs != docs {
                return Err(invalid_generation(
                    &final_directory,
                    "concurrent same-id build has different docs bytes",
                ));
            }
            sync_directory(&generations).map_err(|source| io_error(&generations, source))?;
            drop(reader);
            Ok(id)
        }
        Err(source) => {
            let _ = fs::remove_dir_all(&temporary_directory);
            Err(io_error(&final_directory, source))
        }
    }
}

/// Open and validate the generation named by the exact `CURRENT` bytes.
pub fn open_current(manager: &IndexManager) -> Result<GenerationReader, MemexError> {
    let current_path = manager.root.join(CURRENT_FILENAME);
    let bytes = read_regular_file(&current_path)?;
    let id = parse_current(&bytes, &current_path)?;
    validate_generation(manager, &id)
}

pub fn publish(manager: &IndexManager, id: &GenerationId) -> Result<(), MemexError> {
    publish_inner(manager, id, &sync_directory)
}

fn publish_inner(
    manager: &IndexManager,
    id: &GenerationId,
    sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), MemexError> {
    let _ = validate_generation(manager, id)?;
    let _lock = PublicationLock::try_acquire(&manager.root)?;
    let _ = validate_generation(manager, id)?;

    let current_path = manager.root.join(CURRENT_FILENAME);
    let previous = match fs::symlink_metadata(&current_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(invalid_generation(
                    &current_path,
                    "CURRENT must be a regular file",
                ));
            }
            Some(read_regular_file(&current_path)?)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(io_error(&current_path, source)),
    };

    // Preflight the parent before the switch. The post-switch sync is retained
    // for durability; if it fails, the old bytes are restored while the lock is
    // still held.
    sync(&manager.root).map_err(|source| io_error(&manager.root, source))?;
    let current_bytes = format!("{id}\n").into_bytes();
    let temporary_current = temporary_current_path(&manager.root);
    let write_result = write_file_sync(&temporary_current, &current_bytes)
        .and_then(|_| atomic_replace(&temporary_current, &current_path));
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_current);
        return Err(error);
    }
    if let Err(source) = sync(&manager.root) {
        let rollback = restore_current(&manager.root, &current_path, previous.as_deref());
        let resync = sync(&manager.root).map_err(|error| io_error(&manager.root, error));
        if let Err(rollback_error) = rollback {
            let detail = match resync {
                Ok(()) => format!("CURRENT sync failed ({source}) and rollback failed: {rollback_error}"),
                Err(resync_error) => format!(
                    "CURRENT sync failed ({source}), rollback failed: {rollback_error}, and rollback sync failed: {resync_error}"
                ),
            };
            return Err(io_error(&current_path, std::io::Error::other(detail)));
        }
        if let Err(resync_error) = resync {
            return Err(io_error(
                &current_path,
                std::io::Error::other(format!(
                    "CURRENT sync failed ({source}) and rollback sync failed: {resync_error}"
                )),
            ));
        }
        return Err(io_error(&manager.root, source));
    }
    Ok(())
}

fn build_temporary_generation(
    temporary_directory: &Path,
    docs: &[u8],
    manifest: &GenerationManifest,
    records: &[DocumentRecord],
) -> Result<(), MemexError> {
    write_file_sync(&temporary_directory.join("docs.ndjson"), docs)?;
    let manifest_bytes = encode_manifest(manifest)?;
    write_file_sync(&temporary_directory.join("manifest.json"), &manifest_bytes)?;
    let tantivy_directory = temporary_directory.join("tantivy");
    fs::create_dir(&tantivy_directory).map_err(|source| io_error(&tantivy_directory, source))?;

    let index = build_tantivy(&tantivy_directory, records)
        .map_err(|source| tantivy_error(&tantivy_directory, source))?;
    use tantivy::directory::Directory;
    index
        .directory()
        .sync_directory()
        .map_err(|source| io_error(&tantivy_directory, source))?;
    drop(index);
    #[cfg(not(windows))]
    sync_tantivy(&tantivy_directory)?;
    sync_directory(temporary_directory).map_err(|source| io_error(temporary_directory, source))?;
    Ok(())
}

fn validate_generation(
    manager: &IndexManager,
    id: &GenerationId,
) -> Result<GenerationReader, MemexError> {
    let directory = generation_directory(manager, id);
    require_directory(&directory, "generation directory is missing or not a directory")?;
    let docs_path = directory.join("docs.ndjson");
    let docs = read_regular_file(&docs_path)?;
    let records = decode_ndjson(&docs).map_err(|source| {
        invalid_generation(&docs_path, format!("docs.ndjson is invalid: {source}"))
    })?;
    let manifest_path = directory.join("manifest.json");
    let manifest_bytes = read_regular_file(&manifest_path)?;
    let manifest = decode_manifest_at(&manifest_path, &manifest_bytes)?;
    let expected_docs_hash = sha256_hex(&docs);
    if manifest.id != *id {
        return Err(invalid_generation(
            &manifest_path,
            "manifest id does not match the requested generation",
        ));
    }
    if manifest.docs_sha256 != expected_docs_hash {
        return Err(invalid_generation(
            &manifest_path,
            "manifest docs_sha256 does not match docs.ndjson",
        ));
    }
    if manifest.doc_count != records.len() as u64 {
        return Err(invalid_generation(
            &manifest_path,
            "manifest doc_count does not match docs.ndjson",
        ));
    }
    if manifest.schema != GENERATION_SCHEMA
        || manifest.contract_version != GENERATION_CONTRACT_VERSION
        || manifest.index_profile != INDEX_PROFILE
    {
        return Err(invalid_generation(
            &manifest_path,
            "manifest constants do not match the v2 contract",
        ));
    }
    if generation_id_from_components(&manifest.contract_version, &manifest.index_profile, &docs)
        != *id
    {
        return Err(invalid_generation(
            &manifest_path,
            "manifest id does not match framed generation inputs",
        ));
    }
    let tantivy_directory = directory.join("tantivy");
    require_directory(&tantivy_directory, "Tantivy directory is missing")?;
    let managed_path = tantivy_directory.join(".managed.json");
    let managed_files = read_managed_files(&managed_path)?;
    let index = Index::open_in_dir(&tantivy_directory)
        .map_err(|source| tantivy_error(&tantivy_directory, source))?;
    if index.schema() != build_schema() {
        return Err(tantivy_error(
            &tantivy_directory,
            "index schema does not match tantivy-central-v2",
        ));
    }
    let metas = index
        .load_metas()
        .map_err(|source| tantivy_error(&tantivy_directory, source))?;
    let expected_payload = format!("{{\"index_profile\":\"{INDEX_PROFILE}\"}}");
    if metas.payload.as_deref() != Some(expected_payload.as_str()) {
        return Err(tantivy_error(
            &tantivy_directory,
            "last Tantivy commit payload does not match index profile",
        ));
    }
    let committed_count = metas
        .segments
        .iter()
        .map(|segment| u64::from(segment.num_docs()))
        .sum::<u64>();
    if committed_count != manifest.doc_count {
        return Err(tantivy_error(
            &tantivy_directory,
            "committed Tantivy document count does not match docs.ndjson",
        ));
    }
    validate_committed_files(&index, &tantivy_directory, &managed_files, &metas.segments)?;
    let corrupt_files = index
        .validate_checksum()
        .map_err(|source| tantivy_error(&tantivy_directory, source))?;
    if !corrupt_files.is_empty() {
        return Err(tantivy_error(
            &tantivy_directory,
            format!("Tantivy checksum validation failed for {} files", corrupt_files.len()),
        ));
    }
    register_analyzers(&index);
    Ok(GenerationReader {
        id: id.clone(),
        manifest,
        index,
    })
}

fn generation_id(docs: &[u8]) -> GenerationId {
    generation_id_from_components(GENERATION_CONTRACT_VERSION, INDEX_PROFILE, docs)
}

fn generation_id_from_components(contract_version: &str, index_profile: &str, docs: &[u8]) -> GenerationId {
    let mut hasher = Sha256::new();
    for component in [contract_version.as_bytes(), index_profile.as_bytes(), docs] {
        hasher.update((component.len() as u64).to_be_bytes());
        hasher.update(component);
    }
    GenerationId::from_digest(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn parse_current(bytes: &[u8], path: &Path) -> Result<GenerationId, MemexError> {
    if bytes.len() != 65 || bytes.last() != Some(&b'\n') || bytes[..64].contains(&b'\n') {
        return Err(invalid_generation(
            path,
            "CURRENT must contain exactly one generation id and one LF",
        ));
    }
    let value = std::str::from_utf8(&bytes[..64])
        .map_err(|_| invalid_generation(path, "CURRENT is not UTF-8"))?;
    GenerationId::parse(value).map_err(|_| invalid_generation(path, "CURRENT names an invalid generation"))
}

fn generation_directory(manager: &IndexManager, id: &GenerationId) -> PathBuf {
    manager.root.join(GENERATIONS_DIRECTORY).join(id.as_str())
}

fn temporary_generation_directory(generations: &Path, id: &GenerationId) -> PathBuf {
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    generations.join(format!(
        "{TEMP_GENERATION_PREFIX}{}-{}-{}.{}",
        id,
        std::process::id(),
        sequence,
        TEMP_GENERATION_SUFFIX.trim_start_matches('.')
    ))
}
fn temporary_lease_path(temporary_directory: &Path) -> PathBuf {
    let mut path = temporary_directory.as_os_str().to_os_string();
    path.push(BUILD_LEASE_SUFFIX);
    PathBuf::from(path)
}

fn temporary_current_path(root: &Path) -> PathBuf {
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    root.join(format!(
        "{TEMP_CURRENT_PREFIX}{}-{}.{}",
        std::process::id(),
        sequence,
        TEMP_CURRENT_SUFFIX.trim_start_matches('.')
    ))
}

fn cleanup_interrupted_temps(generations: &Path) {
    let Ok(entries) = fs::read_dir(generations) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(TEMP_GENERATION_PREFIX) && name.ends_with(TEMP_GENERATION_SUFFIX) {
            let lease_path = temporary_lease_path(&path);
            let Some(file) = try_open_unowned_lease(&lease_path) else {
                continue;
            };
            let removed = fs::remove_dir_all(&path).is_ok();
            drop(file);
            if removed {
                let _ = fs::remove_file(lease_path);
            }
        } else if name.starts_with(TEMP_GENERATION_PREFIX)
            && name.ends_with(&format!("{TEMP_GENERATION_SUFFIX}{BUILD_LEASE_SUFFIX}"))
        {
            let temp_name = &name[..name.len() - BUILD_LEASE_SUFFIX.len()];
            let temp_path = generations.join(temp_name);
            if !temp_path.exists()
                && let Some(file) = try_open_unowned_lease(&path)
            {
                drop(file);
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn try_open_unowned_lease(path: &Path) -> Option<File> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .ok()?;
    match file.try_lock_exclusive() {
        Ok(true) => Some(file),
        Ok(false) | Err(_) => None,
    }
}

struct BuildLease {
    file: File,
}

impl BuildLease {
    fn try_acquire(path: &Path) -> Result<Self, MemexError> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| MemexError::Lock {
                path: path.to_path_buf(),
                message: source.to_string(),
            })?;
        match file.try_lock_exclusive() {
            Ok(true) => Ok(Self { file }),
            Ok(false) => Err(MemexError::LockContended {
                path: path.to_path_buf(),
            }),
            Err(source) => Err(MemexError::Lock {
                path: path.to_path_buf(),
                message: source.to_string(),
            }),
        }
    }
}

impl Drop for BuildLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn read_managed_files(path: &Path) -> Result<HashSet<PathBuf>, MemexError> {
    let bytes = read_regular_file(path)?;
    if bytes.len() < 2
        || !bytes.ends_with(b"\n")
        || bytes[..bytes.len() - 1]
            .last()
            .is_some_and(u8::is_ascii_whitespace)
        || bytes.contains(&b'\r')
    {
        return Err(tantivy_error(
            path,
            "managed metadata is not a canonical Tantivy JSON stream",
        ));
    }
    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|source| tantivy_error(path, format!("managed metadata is invalid: {source}")))?;
    let entries = parsed.as_array().ok_or_else(|| {
        tantivy_error(
            path,
            "managed metadata must be a canonical JSON array of paths",
        )
    })?;
    let mut canonical = serde_json::to_vec(&parsed)
        .map_err(|source| tantivy_error(path, format!("managed metadata is invalid: {source}")))?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err(tantivy_error(
            path,
            "managed metadata is not a canonical Tantivy JSON stream",
        ));
    }

    let mut managed_files = HashSet::with_capacity(entries.len());
    for entry in entries {
        let value = entry.as_str().ok_or_else(|| {
            tantivy_error(
                path,
                "managed metadata must contain only JSON string paths",
            )
        })?;
        let relative = PathBuf::from(value);
        if !managed_files.insert(relative.clone()) {
            return Err(tantivy_error(
                path,
                "managed metadata contains a duplicate path",
            ));
        }
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_)
                        | Component::RootDir
                        | Component::ParentDir
                        | Component::CurDir
                )
            })
        {
            return Err(tantivy_error(
                path,
                "managed metadata contains an unsafe relative path",
            ));
        }
    }
    Ok(managed_files)
}

fn validate_committed_files(
    index: &Index,
    tantivy_directory: &Path,
    managed_files: &HashSet<PathBuf>,
    segments: &[tantivy::index::SegmentMeta],
) -> Result<(), MemexError> {
    for segment in segments {
        for component in tantivy::index::SegmentComponent::iterator() {
            let relative = segment.relative_path(*component);
            let optional_delete = *component == tantivy::index::SegmentComponent::Delete
                && !segment.has_deletes();
            if optional_delete && !managed_files.contains(&relative) {
                continue;
            }
            if !managed_files.contains(&relative) {
                return Err(tantivy_error(
                    tantivy_directory,
                    format!("committed component is absent from managed metadata: {relative:?}"),
                ));
            }
            let full_path = tantivy_directory.join(&relative);
            let metadata = fs::symlink_metadata(&full_path).map_err(|source| {
                tantivy_error(
                    &full_path,
                    format!("committed component is missing: {source}"),
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(tantivy_error(
                    &full_path,
                    "committed component is not a regular file",
                ));
            }
            let valid = index
                .directory()
                .validate_checksum(&relative)
                .map_err(|source| tantivy_error(&full_path, source))?;
            if !valid {
                return Err(tantivy_error(
                    &full_path,
                    "committed component checksum is invalid",
                ));
            }
        }
    }
    Ok(())
}

fn require_directory(path: &Path, message: &str) -> Result<(), MemexError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            invalid_generation(path, message)
        } else {
            io_error(path, source)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_generation(path, message));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, MemexError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_generation(path, "expected a regular file"));
    }
    fs::read(path).map_err(|source| io_error(path, source))
}

fn write_file_sync(path: &Path, bytes: &[u8]) -> Result<(), MemexError> {
    let mut file = File::create(path).map_err(|source| io_error(path, source))?;
    file.write_all(bytes).map_err(|source| io_error(path, source))?;
    file.sync_all().map_err(|source| io_error(path, source))
}

#[cfg(not(windows))]
fn sync_tantivy(path: &Path) -> Result<(), MemexError> {
    let entries = fs::read_dir(path).map_err(|source| io_error(path, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error(path, source))?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|source| io_error(&child, source))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_generation(&child, "Tantivy tree contains a symlink"));
        }
        if metadata.is_dir() {
            sync_tantivy(&child)?;
        } else if metadata.is_file() {
            File::open(&child)
                .map_err(|source| io_error(&child, source))?
                .sync_all()
                .map_err(|source| io_error(&child, source))?;
        }
    }
    sync_directory(path).map_err(|source| io_error(path, source))
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn atomic_replace(source: &Path, target: &Path) -> Result<(), MemexError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let source_wide = source.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let target_wide = target.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        // SAFETY: both vectors are NUL-terminated UTF-16 paths that remain
        // alive for the duration of the system call; MoveFileExW does not
        // retain either pointer.
        let result = unsafe {
            MoveFileExW(
                source_wide.as_ptr(),
                target_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            return Err(io_error(target, std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, target).map_err(|source_error| io_error(target, source_error))
    }
}

fn restore_current(root: &Path, current: &Path, previous: Option<&[u8]>) -> Result<(), MemexError> {
    match previous {
        Some(bytes) => {
            let temporary = temporary_current_path(root);
            let result = write_file_sync(&temporary, bytes)
                .and_then(|_| atomic_replace(&temporary, current));
            let _ = fs::remove_file(&temporary);
            result
        }
        None => fs::remove_file(current)
            .map_err(|source| io_error(current, source)),
    }
}

fn io_error(path: &Path, source: std::io::Error) -> MemexError {
    MemexError::Io {
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}

fn invalid_generation(path: &Path, message: impl Into<String>) -> MemexError {
    MemexError::InvalidGeneration {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

fn tantivy_error(path: &Path, source: impl ToString) -> MemexError {
    MemexError::Tantivy {
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn failed_post_swap_sync_restores_current_and_resyncs_parent() {
        let root = std::env::temp_dir().join(format!(
            "memex-generation-sync-fault-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("scratch directory should be created");
        let manager = IndexManager::new(root.clone());
        let records =
            decode_ndjson(include_bytes!("../../../fixtures/memex/generation/records.ndjson"))
                .expect("generation fixture must be canonical");
        let old_id = build_generation(&manager, &records).expect("old generation should build");
        publish(&manager, &old_id).expect("old generation should publish");
        let new_id =
            build_generation(&manager, &records[..records.len() - 1]).expect("new generation should build");
        let before = fs::read(root.join(CURRENT_FILENAME)).expect("CURRENT should exist");
        let calls = AtomicUsize::new(0);
        let sync = |path: &Path| {
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                Err(std::io::Error::other("injected post-swap sync failure"))
            } else {
                sync_directory(path)
            }
        };

        let error =
            publish_inner(&manager, &new_id, &sync).expect_err("injected sync must fail closed");

        assert!(error.to_string().contains("sync"));
        assert_eq!(fs::read(root.join(CURRENT_FILENAME)).unwrap(), before);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        fs::remove_dir_all(root).expect("scratch directory should be removed");
    }

    #[test]
    fn managed_metadata_parser_accepts_paths_with_spaces() {
        let path = std::env::temp_dir().join(format!(
            "memex-managed-metadata-space-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow the Unix epoch")
                .as_nanos()
        ));
        fs::write(&path, b"[\"segment with space\"]\n")
            .expect("managed metadata fixture should be written");

        let managed_files = read_managed_files(&path).expect("spaces are valid path content");

        assert!(managed_files.contains(Path::new("segment with space")));
        fs::remove_file(path).expect("managed metadata fixture should be removed");
    }
}
