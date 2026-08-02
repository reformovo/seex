use std::fs::File;
use std::path::Path;

use crate::model::run::RunId;
use crate::model::types::ProjectId;

use crate::storage::{StorageError, percent_encode_metric_key};

/// Exclusive local writer guard for one run.
pub struct RunWriterGuard {
    lock_file: File,
}

impl RunWriterGuard {
    /// Acquires the project-local advisory run-writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::RunAlreadyActive`] when another process owns the
    /// lock, or a storage error when the lock file cannot be prepared.
    pub fn acquire(lock_namespace: &Path, run_id: &RunId) -> Result<Self, StorageError> {
        let lock_dir = lock_namespace.join("runs");
        std::fs::create_dir_all(&lock_dir).map_err(|source| StorageError::Storage {
            operation: "creating run lock directory",
            name: path_basename(&lock_dir),
            source,
        })?;
        let lock_path = lock_dir.join(format!(
            "{}.lock",
            percent_encode_metric_key(run_id.as_str())
        ));
        let lock_file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| StorageError::Storage {
                operation: "opening run lock file",
                name: path_basename(&lock_path),
                source,
            })?;
        match lock_file.try_lock() {
            Ok(()) => Ok(Self { lock_file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(StorageError::RunAlreadyActive {
                run_id: run_id.as_str().to_owned(),
            }),
            Err(source) => Err(StorageError::Storage {
                operation: "locking run lock file",
                name: path_basename(&lock_path),
                source: source.into(),
            }),
        }
    }
}

impl Drop for RunWriterGuard {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

/// Short-lived cross-process guard for Project get-or-create.
pub(crate) struct ProjectCreateGuard {
    lock_file: File,
}

impl ProjectCreateGuard {
    pub(crate) fn acquire(
        lock_namespace: &Path,
        project_id: &ProjectId,
    ) -> Result<Self, StorageError> {
        let lock_dir = lock_namespace.join("projects");
        std::fs::create_dir_all(&lock_dir).map_err(|source| StorageError::Storage {
            operation: "creating project lock directory",
            name: path_basename(&lock_dir),
            source,
        })?;
        let lock_path = lock_dir.join(format!(
            "{}.lock",
            percent_encode_metric_key(project_id.as_str())
        ));
        let lock_file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| StorageError::Storage {
                operation: "opening project lock file",
                name: path_basename(&lock_path),
                source,
            })?;
        lock_file.lock().map_err(|source| StorageError::Storage {
            operation: "locking project lock file",
            name: path_basename(&lock_path),
            source,
        })?;
        Ok(Self { lock_file })
    }
}

impl Drop for ProjectCreateGuard {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

fn path_basename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("storage path")
        .to_owned()
}
