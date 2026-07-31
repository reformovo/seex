//! Desktop-owned, syntax-preserving configuration updates.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use toml_edit::{DocumentMut, value};

const SCHEMA_VERSION: i64 = 1;
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigScope {
    Global,
    Project,
}

/// One loaded configuration document and its exact stale-read fingerprint.
pub struct EditableConfig {
    path: PathBuf,
    scope: ConfigScope,
    original: Option<Vec<u8>>,
    document: DocumentMut,
}

impl EditableConfig {
    /// Loads an existing schema-v1 document or creates a new in-memory one.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError`] for I/O, UTF-8, TOML, or schema failures.
    pub fn load(path: impl Into<PathBuf>, scope: ConfigScope) -> Result<Self, ConfigEditError> {
        let path = path.into();
        let original = read_optional(&path)?;
        let document = match original.as_deref() {
            Some(bytes) => {
                let raw = std::str::from_utf8(bytes).map_err(|_| ConfigEditError::InvalidUtf8)?;
                let document = raw.parse::<DocumentMut>()?;
                if document["schema_version"].as_integer() != Some(SCHEMA_VERSION) {
                    return Err(ConfigEditError::UnsupportedSchema);
                }
                document
            }
            None => {
                let mut document = DocumentMut::new();
                document["schema_version"] = value(SCHEMA_VERSION);
                document
            }
        };
        Ok(Self {
            path,
            scope,
            original,
            document,
        })
    }

    /// Atomically saves the document if its original bytes are still current.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError::StaleRead`] rather than overwriting an
    /// external edit. I/O failures leave the destination untouched.
    pub fn save(&self) -> Result<(), ConfigEditError> {
        let parent = self.path.parent().ok_or(ConfigEditError::MissingParent)?;
        fs::create_dir_all(parent)?;
        let temporary = temporary_path(&self.path);
        let result = self.write_and_replace(&temporary);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn write_and_replace(&self, temporary: &Path) -> Result<(), ConfigEditError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary)?;
        #[cfg(unix)]
        if self.scope == ConfigScope::Global {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(self.document.to_string().as_bytes())?;
        file.sync_all()?;
        if read_optional(&self.path)? != self.original {
            return Err(ConfigEditError::StaleRead);
        }
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, io::Error> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.{}.{id}.tmp", std::process::id()))
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigEditError {
    #[error("configuration file is not UTF-8")]
    InvalidUtf8,
    #[error("configuration schema_version must be 1")]
    UnsupportedSchema,
    #[error("configuration path has no parent directory")]
    MissingParent,
    #[error("configuration changed since it was read")]
    StaleRead,
    #[error("invalid configuration TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    #[error("configuration I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_saves_schema_version_one() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        let config = EditableConfig::load(&path, ConfigScope::Global)?;

        config.save()?;

        assert_eq!(fs::read_to_string(&path)?, "schema_version = 1\n");
        Ok(())
    }

    #[test]
    fn save_rejects_an_external_edit() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        fs::write(&path, "schema_version = 1\n# original\n")?;
        let config = EditableConfig::load(&path, ConfigScope::Project)?;
        fs::write(&path, "schema_version = 1\n# external\n")?;

        assert!(matches!(config.save(), Err(ConfigEditError::StaleRead)));
        assert_eq!(
            fs::read_to_string(path)?,
            "schema_version = 1\n# external\n"
        );
        Ok(())
    }
}
