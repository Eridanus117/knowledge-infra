use memex_core::generation::{IndexManager, build_generation, open_current, publish};
use memex_core::lock::PublicationLock;
use memex_core::manifest::{decode_manifest, encode_manifest};
use memex_core::{DocumentRecord, MemexError, decode_ndjson, encode_ndjson};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

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
        let path = std::env::temp_dir().join(format!("memex-generation-{timestamp}-{sequence}"));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
    }

    fn manager(&self) -> IndexManager {
        IndexManager::new(self.path.clone())
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

fn fixture_records() -> Vec<DocumentRecord> {
    decode_ndjson(include_bytes!("../../../fixtures/memex/generation/records.ndjson"))
        .expect("generation fixture must be canonical")
}

fn generation_path(manager: &IndexManager, id: &str) -> PathBuf {
    manager.root.join("generations").join(id)
}

fn current_bytes(manager: &IndexManager) -> Vec<u8> {
    fs::read(manager.root.join("CURRENT")).expect("CURRENT should exist")
}

fn publish_fixture(manager: &IndexManager, records: &[DocumentRecord]) -> memex_core::generation::GenerationId {
    let id = build_generation(manager, records).expect("fixture generation should build");
    publish(manager, &id).expect("fixture generation should publish");
    id
}

#[test]
fn deterministic_generation_id_is_order_independent_and_framed() {
    let first = ScratchDirectory::new();
    let second = ScratchDirectory::new();
    let first_manager = first.manager();
    let second_manager = second.manager();
    let records = fixture_records();
    let mut reversed = records.clone();
    reversed.reverse();

    let first_id = build_generation(&first_manager, &records).expect("first generation should build");
    let second_id = build_generation(&second_manager, &reversed).expect("second generation should build");

    assert_eq!(first_id, second_id);
    assert_eq!(first_id.as_str().len(), 64);
    assert!(first_id.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(first_id.as_str(), include_str!("../../../fixtures/memex/generation/generation.id").trim());
}

#[test]
fn build_writes_the_exact_generation_layout_and_strict_manifest() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let encoded_docs = encode_ndjson(&records).expect("fixture docs should encode");
    let id = build_generation(&manager, &records).expect("generation should build");
    let root = generation_path(&manager, id.as_str());

    assert_eq!(fs::read(root.join("docs.ndjson")).unwrap(), encoded_docs);
    assert!(root.join("manifest.json").is_file());
    assert!(root.join("tantivy").is_dir());
    let manifest = decode_manifest(&fs::read(root.join("manifest.json")).unwrap())
        .expect("manifest should decode");
    assert_eq!(manifest.id, id);
    assert_eq!(manifest.doc_count, records.len() as u64);
    assert_eq!(manifest.docs_sha256.len(), 64);
    assert!(root.join("tantivy/meta.json").is_file());
    assert!(!manager.root.join("CURRENT").exists());
}

#[test]
fn manifest_codec_is_strict_and_canonical() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let id = build_generation(&manager, &fixture_records()).expect("generation should build");
    let manifest_path = generation_path(&manager, id.as_str()).join("manifest.json");
    let bytes = fs::read(&manifest_path).unwrap();
    let manifest = decode_manifest(&bytes).expect("manifest should decode");
    assert_eq!(encode_manifest(&manifest).unwrap(), bytes);

    let mut unknown = bytes.clone();
    unknown.truncate(unknown.len() - 2);
    unknown.extend_from_slice(b",\"unknown\":true}\n");
    assert!(decode_manifest(&unknown).is_err());

    let mut noncanonical = bytes.clone();
    noncanonical.insert(0, b' ');
    assert!(decode_manifest(&noncanonical).is_err());
}

#[test]
fn interrupted_build_cleans_its_temporary_sibling() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let temp_root = manager.root.join("generations");
    fs::create_dir_all(&temp_root).unwrap();
    let interrupted = temp_root.join(".memex-generation-interrupted.tmp");
    fs::create_dir_all(&interrupted).unwrap();
    fs::write(interrupted.join("partial"), b"partial").unwrap();

    let id = build_generation(&manager, &fixture_records()).expect("build should recover");

    assert!(!interrupted.exists());
    assert!(generation_path(&manager, id.as_str()).is_dir());
}

#[test]
fn corrupt_manifest_cannot_be_published_and_current_stays_byte_identical() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let id = publish_fixture(&manager, &records);
    let before = current_bytes(&manager);
    let manifest_path = generation_path(&manager, id.as_str()).join("manifest.json");
    fs::write(&manifest_path, br#"{"schema":"unknown"}"#).unwrap();

    let error = publish(&manager, &id).expect_err("corrupt manifest must fail closed");

    assert!(error.to_string().contains("manifest"));
    assert_eq!(current_bytes(&manager), before);
    assert!(open_current(&manager).is_err());
}

#[test]
fn missing_tantivy_commit_cannot_be_published_and_current_stays_byte_identical() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let old_id = publish_fixture(&manager, &records);
    let new_id = build_generation(&manager, &records[..records.len() - 1])
        .expect("second generation should build");
    let before = current_bytes(&manager);
    let meta_path = generation_path(&manager, new_id.as_str()).join("tantivy/meta.json");
    fs::remove_file(meta_path).unwrap();

    let error = publish(&manager, &new_id).expect_err("missing Tantivy commit must fail closed");

    assert!(error.to_string().contains("Tantivy") || error.to_string().contains("tantivy"));
    assert_eq!(current_bytes(&manager), before);
    assert_eq!(fs::read_to_string(manager.root.join("CURRENT")).unwrap(), format!("{old_id}\n"));
}

#[test]
fn open_current_rejects_unknown_or_malformed_generation_without_fallback() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    fs::create_dir_all(&manager.root).unwrap();

    fs::write(manager.root.join("CURRENT"), b"not-a-generation\n").unwrap();
    let unknown = open_current(&manager).expect_err("unknown generation must fail");
    assert!(unknown.to_string().contains("CURRENT") || unknown.to_string().contains("generation"));

    fs::write(manager.root.join("CURRENT"), b"\n").unwrap();
    assert!(open_current(&manager).is_err());
}

#[test]
fn publication_lock_reports_contention_and_releases_on_drop() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let held = PublicationLock::try_acquire(&manager.root).expect("first lock should acquire");
    let error = PublicationLock::try_acquire(&manager.root)
        .expect_err("second lock should report contention");
    assert!(error.to_string().contains("index.lock"));
    assert!(error.to_string().contains("held") || error.to_string().contains("lock"));
    drop(held);
    let _released = PublicationLock::try_acquire(&manager.root).expect("lock should release");
}

#[test]
fn concurrent_publishers_leave_only_a_complete_old_or_new_snapshot() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let old_id = publish_fixture(&manager, &records);
    let new_id = build_generation(&manager, &records[..records.len() - 1])
        .expect("second generation should build");
    let manager = Arc::new(manager);
    let barrier = Arc::new(Barrier::new(3));
    let left_manager = Arc::clone(&manager);
    let left_barrier = Arc::clone(&barrier);
    let left_id = old_id.clone();
    let left = thread::spawn(move || {
        left_barrier.wait();
        publish(&left_manager, &left_id)
    });
    let right_manager = Arc::clone(&manager);
    let right_barrier = Arc::clone(&barrier);
    let right_id = new_id.clone();
    let right = thread::spawn(move || {
        right_barrier.wait();
        publish(&right_manager, &right_id)
    });
    barrier.wait();
    let left_result = left.join().unwrap();
    let right_result = right.join().unwrap();

    assert!(left_result.is_ok() || left_result.as_ref().is_err_and(is_lock_contention));
    assert!(right_result.is_ok() || right_result.as_ref().is_err_and(is_lock_contention));
    let current = current_bytes(&manager);
    assert!(current == format!("{old_id}\n").as_bytes() || current == format!("{new_id}\n").as_bytes());
    let reader = open_current(&manager).expect("CURRENT must name a complete generation");
    assert!(reader.id == old_id || reader.id == new_id);
}

fn is_lock_contention(error: &MemexError) -> bool {
    error.to_string().contains("index.lock")
}

#[test]
fn reader_snapshots_keep_old_generation_after_new_publish() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let old_id = publish_fixture(&manager, &records);
    let old_reader = open_current(&manager).expect("old reader should open");
    let new_id = build_generation(&manager, &records[..records.len() - 1])
        .expect("new generation should build");
    publish(&manager, &new_id).expect("new generation should publish");
    let new_reader = open_current(&manager).expect("new reader should open");

    assert_eq!(old_reader.id, old_id);
    assert_eq!(new_reader.id, new_id);
    assert_eq!(old_reader.manifest.doc_count, records.len() as u64);
    assert_eq!(new_reader.manifest.doc_count, (records.len() - 1) as u64);
    assert_eq!(old_reader.index.schema(), new_reader.index.schema());
    assert!(generation_path(&manager, old_id.as_str()).is_dir());
    assert!(generation_path(&manager, new_id.as_str()).is_dir());
}

#[cfg(windows)]
#[test]
fn windows_current_replacement_replaces_existing_file_atomically() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let old_id = publish_fixture(&manager, &records);
    let new_id = build_generation(&manager, &records[..records.len() - 1])
        .expect("second generation should build");

    publish(&manager, &new_id).expect("Windows replacement should succeed");

    assert_eq!(fs::read_to_string(manager.root.join("CURRENT")).unwrap(), format!("{new_id}\n"));
    assert_ne!(old_id, new_id);
}

#[test]
fn current_and_previous_generations_are_retained() {
    let scratch = ScratchDirectory::new();
    let manager = scratch.manager();
    let records = fixture_records();
    let old_id = publish_fixture(&manager, &records);
    let new_id = build_generation(&manager, &records[..records.len() - 1])
        .expect("second generation should build");
    publish(&manager, &new_id).expect("second generation should publish");

    assert_eq!(fs::read_to_string(manager.root.join("CURRENT")).unwrap(), format!("{new_id}\n"));
    assert!(generation_path(&manager, old_id.as_str()).exists());
    assert!(generation_path(&manager, new_id.as_str()).exists());
    assert_eq!(open_current(&manager).unwrap().id, new_id);
}
