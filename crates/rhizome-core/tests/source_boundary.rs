use rhizome_core::{SourcePath, SourceRoot};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
        let path = std::env::temp_dir().join(format!(
            "knowledge-infra-source-boundary-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("scratch directory should be created");
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

#[test]
fn source_root_accepts_an_existing_absolute_directory() {
    let scratch = ScratchDirectory::new();

    let root = SourceRoot::new(scratch.path()).expect("existing directory should be accepted");

    assert_eq!(root.as_path(), scratch.path());
}

#[test]
fn source_root_rejects_a_relative_path() {
    let path = PathBuf::from("notes");

    let error = SourceRoot::new(&path).expect_err("relative root should be rejected");

    assert_eq!(error.code(), "source.root.relative");
    assert_eq!(error.path(), Some(path.as_path()));
}

#[test]
fn source_root_rejects_a_missing_directory() {
    let scratch = ScratchDirectory::new();
    let path = scratch.path().join("missing");

    let error = SourceRoot::new(&path).expect_err("missing root should be rejected");

    assert_eq!(error.code(), "source.root.missing");
    assert_eq!(error.path(), Some(path.as_path()));
}

#[test]
fn source_root_rejects_a_file() {
    let scratch = ScratchDirectory::new();
    let path = scratch.path().join("note.md");
    fs::write(&path, "# Note\n").expect("scratch file should be created");

    let error = SourceRoot::new(&path).expect_err("file root should be rejected");

    assert_eq!(error.code(), "source.root.not_directory");
    assert_eq!(error.path(), Some(path.as_path()));
}

#[test]
fn source_path_accepts_normal_relative_components() {
    let path = PathBuf::from("topics").join("note.md");

    let source_path = SourcePath::new(&path).expect("normal relative path should be accepted");

    assert_eq!(source_path.as_path(), path.as_path());
}

#[test]
fn source_path_rejects_non_relative_components() {
    let scratch = ScratchDirectory::new();
    let invalid_paths = [
        PathBuf::new(),
        PathBuf::from("."),
        PathBuf::from(".."),
        PathBuf::from("topics").join("..").join("note.md"),
        scratch.path().join("note.md"),
    ];

    for path in invalid_paths {
        let error = SourcePath::new(&path).expect_err("invalid source path should be rejected");
        assert_eq!(error.code(), "source.path.invalid", "path: {path:?}");
        assert_eq!(error.path(), Some(path.as_path()), "path: {path:?}");
    }
}
