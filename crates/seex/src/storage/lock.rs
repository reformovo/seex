use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

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

/// Cross-process guard for Project get-or-create and its initial Run insert.
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

/// Exclusive, bounded cross-process guard for one catalog's write transaction.
#[derive(Debug)]
pub(crate) struct CatalogWriteGuard {
    lock_file: File,
}

impl CatalogWriteGuard {
    pub(crate) fn acquire(catalog_path: &Path, timeout: Duration) -> Result<Self, StorageError> {
        let parent = catalog_path
            .parent()
            .ok_or_else(|| StorageError::CatalogNotFound {
                name: path_basename(catalog_path),
            })?;
        std::fs::create_dir_all(parent).map_err(|source| StorageError::Storage {
            operation: "creating catalog lock directory",
            name: path_basename(parent),
            source,
        })?;
        let parent = std::fs::canonicalize(parent).map_err(|source| StorageError::Storage {
            operation: "resolving catalog lock directory",
            name: path_basename(parent),
            source,
        })?;
        let lock_name = format!(".{}.write.lock", path_basename(catalog_path));
        let lock_file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(lock_name))
            .map_err(|source| StorageError::Storage {
                operation: "opening catalog write lock",
                name: path_basename(catalog_path),
                source,
            })?;
        let deadline = Instant::now() + timeout;
        loop {
            match lock_file.try_lock() {
                Ok(()) => return Ok(Self { lock_file }),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(StorageError::CatalogWriteLockTimeout);
                }
                Err(source) => {
                    return Err(StorageError::Storage {
                        operation: "locking catalog for write",
                        name: path_basename(catalog_path),
                        source: source.into(),
                    });
                }
            }
        }
    }
}

impl Drop for CatalogWriteGuard {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_write_lock_distinguishes_contention_from_queue_pressure() {
        let root = tempfile::tempdir().expect("temporary catalog root");
        let catalog = root.path().join("catalog.sqlite");
        let _first = CatalogWriteGuard::acquire(&catalog, Duration::ZERO).expect("first lock");
        let error = CatalogWriteGuard::acquire(&catalog, Duration::ZERO).unwrap_err();
        assert!(matches!(error, StorageError::CatalogWriteLockTimeout));
    }
}
