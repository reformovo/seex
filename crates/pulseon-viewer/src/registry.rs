use std::path::PathBuf;
use std::sync::Arc;

use crate::core::DataSourceId;
use crate::worker::{
    Generation, ReadConcurrencyGate, ReadEvent, ReadEventReceiver, ReadRequest, ReadWorker,
    WorkerClosed,
};

const MAX_CONCURRENT_SOURCE_READS: usize = 4;

/// Viewer-owned lifecycle state for one imported native source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceStatus {
    Dormant,
    Loading,
    Ready,
    Failed(String),
}

/// Persistent metadata and current health for one imported native source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedSource {
    pub source_id: DataSourceId,
    pub root_path: PathBuf,
    pub status: SourceStatus,
}

struct SourceEntry {
    source: ImportedSource,
    worker: Option<ReadWorker>,
}

/// Retains imported sources and bounds native reads across their workers.
pub struct SourceRegistry {
    entries: Vec<SourceEntry>,
    gate: Arc<ReadConcurrencyGate>,
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            gate: Arc::new(ReadConcurrencyGate::new(MAX_CONCURRENT_SOURCE_READS)),
        }
    }
}

impl SourceRegistry {
    /// Retains a source without opening its native catalog.
    pub fn import(&mut self, root_path: PathBuf) -> DataSourceId {
        let source_id = DataSourceId::from_path(&root_path);
        if self.entry(&source_id).is_none() {
            self.entries.push(SourceEntry {
                source: ImportedSource {
                    source_id: source_id.clone(),
                    root_path,
                    status: SourceStatus::Dormant,
                },
                worker: None,
            });
        }
        source_id
    }

    pub fn sources(&self) -> impl ExactSizeIterator<Item = &ImportedSource> {
        self.entries.iter().map(|entry| &entry.source)
    }

    pub fn source(&self, source_id: &DataSourceId) -> Option<&ImportedSource> {
        self.entry(source_id).map(|entry| &entry.source)
    }

    /// Lazily starts the source worker and returns its event stream once.
    ///
    /// # Errors
    ///
    /// Returns [`SourceRegistryError`] if the source is unknown or its worker
    /// thread cannot be started.
    pub fn activate(
        &mut self,
        source_id: &DataSourceId,
    ) -> Result<Option<ReadEventReceiver>, SourceRegistryError> {
        let gate = Arc::clone(&self.gate);
        let entry = self.entry_mut(source_id)?;
        if entry.worker.is_some() {
            return Ok(None);
        }
        entry.source.status = SourceStatus::Loading;
        let mut worker =
            ReadWorker::spawn_with_gate(&entry.source.root_path, gate).map_err(|error| {
                entry.source.status = SourceStatus::Failed(error.to_string());
                SourceRegistryError::WorkerStart {
                    source_id: source_id.clone(),
                    error,
                }
            })?;
        let events = worker.take_event_receiver().ok_or_else(|| {
            entry.source.status = SourceStatus::Failed("worker event stream is missing".to_owned());
            SourceRegistryError::MissingEventStream(source_id.clone())
        })?;
        entry.worker = Some(worker);
        Ok(Some(events))
    }

    /// Queues one request on an activated source worker.
    ///
    /// # Errors
    ///
    /// Returns [`SourceRegistryError`] if the source is unknown, inactive, or
    /// no longer accepts work.
    pub fn submit(
        &mut self,
        source_id: &DataSourceId,
        generation: Generation,
        request: ReadRequest,
    ) -> Result<(), SourceRegistryError> {
        let entry = self.entry_mut(source_id)?;
        let Some(worker) = entry.worker.as_ref() else {
            return Err(SourceRegistryError::Inactive(source_id.clone()));
        };
        entry.source.status = SourceStatus::Loading;
        worker
            .submit(source_id.clone(), generation, request)
            .map_err(|error| {
                entry.source.status = SourceStatus::Failed(error.to_string());
                SourceRegistryError::WorkerClosed {
                    source_id: source_id.clone(),
                    error,
                }
            })
    }

    /// Reconciles one worker event into its source-specific health state.
    pub fn apply_event(&mut self, event: &ReadEvent) {
        let Ok(entry) = self.entry_mut(&event.source_id) else {
            return;
        };
        entry.source.status = match &event.result {
            Ok(_) => SourceStatus::Ready,
            Err(error) => SourceStatus::Failed(error.to_string()),
        };
    }

    fn entry(&self, source_id: &DataSourceId) -> Option<&SourceEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.source.source_id == source_id)
    }

    fn entry_mut(
        &mut self,
        source_id: &DataSourceId,
    ) -> Result<&mut SourceEntry, SourceRegistryError> {
        self.entries
            .iter_mut()
            .find(|entry| &entry.source.source_id == source_id)
            .ok_or_else(|| SourceRegistryError::UnknownSource(source_id.clone()))
    }
}

/// Failures while coordinating an imported source.
#[derive(Debug, thiserror::Error)]
pub enum SourceRegistryError {
    #[error("source {0} is not imported")]
    UnknownSource(DataSourceId),
    #[error("source {0} has not been activated")]
    Inactive(DataSourceId),
    #[error("failed to start worker for source {source_id}: {error}")]
    WorkerStart {
        source_id: DataSourceId,
        #[source]
        error: std::io::Error,
    },
    #[error("worker for source {0} has no event stream")]
    MissingEventStream(DataSourceId),
    #[error("worker for source {source_id} is closed: {error}")]
    WorkerClosed {
        source_id: DataSourceId,
        #[source]
        error: WorkerClosed,
    },
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::model::CatalogSnapshot;
    use crate::worker::{ReadKind, ReadSnapshot};

    use super::*;

    #[test]
    fn importing_sources_is_retained_ordered_and_lazy() {
        let mut registry = SourceRegistry::default();
        let first = registry.import(Path::new("first").to_path_buf());
        registry.import(Path::new("second").to_path_buf());
        registry.import(Path::new("first").to_path_buf());

        assert_eq!(
            registry
                .sources()
                .map(|source| (&source.source_id, &source.status))
                .collect::<Vec<_>>(),
            [
                (&first, &SourceStatus::Dormant),
                (&DataSourceId::from_string("second"), &SourceStatus::Dormant),
            ]
        );
        assert!(registry.entries.iter().all(|entry| entry.worker.is_none()));
    }

    #[test]
    fn submitting_to_a_dormant_source_is_explicit() {
        let mut registry = SourceRegistry::default();
        let source_id = registry.import(Path::new("missing").to_path_buf());

        let error = registry
            .submit(
                &source_id,
                Generation(1),
                ReadRequest::Discover(Default::default()),
            )
            .expect_err("dormant source should reject direct submission");

        assert!(matches!(error, SourceRegistryError::Inactive(id) if id == source_id));
    }

    #[test]
    fn activation_starts_a_worker_without_opening_the_catalog() {
        let mut registry = SourceRegistry::default();
        let source_id = registry.import(Path::new("missing").to_path_buf());

        let events = registry
            .activate(&source_id)
            .expect("worker thread should start")
            .expect("first activation should return its event stream");

        assert_eq!(
            registry.source(&source_id).map(|source| &source.status),
            Some(&SourceStatus::Loading)
        );
        assert!(events.try_event().is_none());
        assert!(
            registry
                .activate(&source_id)
                .expect("active source should remain valid")
                .is_none()
        );
    }

    #[test]
    fn events_update_only_their_matching_source_status() {
        let mut registry = SourceRegistry::default();
        let first = registry.import(Path::new("first").to_path_buf());
        let second = registry.import(Path::new("second").to_path_buf());

        registry.apply_event(&ReadEvent {
            source_id: first.clone(),
            generation: Generation(1),
            kind: ReadKind::Catalog,
            result: Ok(ReadSnapshot::Catalog(CatalogSnapshot {
                projects: Vec::new(),
                runs: Vec::new(),
                metric_keys: Vec::new(),
            })),
        });

        assert_eq!(
            registry.source(&first).map(|source| &source.status),
            Some(&SourceStatus::Ready)
        );
        assert_eq!(
            registry.source(&second).map(|source| &source.status),
            Some(&SourceStatus::Dormant)
        );
    }
}
