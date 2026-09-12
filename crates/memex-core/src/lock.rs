use crate::error::MemexError;
use fs4::fs_std::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

const LOCK_FILENAME: &str = "index.lock";

/// An advisory exclusive lock for the short publication critical section.
#[derive(Debug)]
pub struct PublicationLock {
    file: File,
    path: PathBuf,
}

impl PublicationLock {
    /// Acquire the publication lock without waiting behind another publisher.
    pub fn try_acquire(root: &Path) -> Result<Self, MemexError> {
        fs::create_dir_all(root).map_err(|source| io_error(root, source))?;
        let path = root.join(LOCK_FILENAME);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| MemexError::Lock {
                path: path.clone(),
                message: source.to_string(),
            })?;
        match file.try_lock_exclusive() {
            Ok(true) => Ok(Self { file, path }),
            Ok(false) => Err(MemexError::LockContended { path }),
            Err(source) => Err(MemexError::Lock {
                path,
                message: source.to_string(),
            }),
        }
    }

    /// Return the lock path used for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PublicationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn io_error(path: &Path, source: std::io::Error) -> MemexError {
    MemexError::Io {
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}
