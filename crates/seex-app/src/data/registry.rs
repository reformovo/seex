use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::ConfiguredSource;
use crate::data::CatalogSnapshot;
use crate::data::worker::{
    Generation, ReadConcurrencyGate, ReadEvent, ReadEventReceiver, ReadRequest, ReadWorker,
    WorkerClosed,
};
use crate::domain::DataSourceId;
use seex_model::types::ProjectId;

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
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedSource {
    pub source_id: DataSourceId,
    pub root_path: PathBuf,
    pub project_allowlist: Vec<ProjectId>,
    pub status: SourceStatus,
    pub catalog: CatalogSnapshot,
}

struct SourceEntry {
    source: ImportedSource,
    worker: Option<ReadWorker>,
    catalog_requests: HashMap<Generation, Option<ProjectId>>,
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
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    pub(crate) fn snapshot(&self) -> Arc<[ImportedSource]> {
        self.entries
            .iter()
            .map(|entry| entry.source.clone())
            .collect()
    }

    /// Installs or replaces one validated configured Source by stable alias.
    pub fn configure(&mut self, configured: ConfiguredSource) -> DataSourceId {
        let source_id = DataSourceId::from_alias(&configured.alias);
        let source = ImportedSource {
            source_id: source_id.clone(),
            root_path: configured.root_path,
            project_allowlist: configured.projects,
            status: SourceStatus::Dormant,
            catalog: CatalogSnapshot {
                projects: Vec::new(),
                runs: Vec::new(),
                metric_keys: Vec::new(),
            },
        };
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.source.source_id == source_id)
        {
            entry.source = source;
            entry.worker = None;
            entry.catalog_requests.clear();
        } else {
            self.entries.push(SourceEntry {
                source,
                worker: None,
                catalog_requests: HashMap::new(),
            });
        }
        source_id
    }

    pub fn sources(&self) -> impl ExactSizeIterator<Item = &ImportedSource> {
        self.entries.iter().map(|entry| &entry.source)
    }

    #[cfg(feature = "test-support")]
    pub fn resource_snapshot(&self) -> crate::performance::ReadSchedulingSnapshot {
        self.entries
            .iter()
            .filter_map(|entry| entry.worker.as_ref())
            .map(ReadWorker::resource_snapshot)
            .fold(
                crate::performance::ReadSchedulingSnapshot::default(),
                |mut total, snapshot| {
                    total.concurrent_reads = total.concurrent_reads.max(snapshot.concurrent_reads);
                    total.peak_concurrent_reads = total
                        .peak_concurrent_reads
                        .max(snapshot.peak_concurrent_reads);
                    total.superseded_reads += snapshot.superseded_reads;
                    total
                },
            )
    }

    pub fn source(&self, source_id: &DataSourceId) -> Option<&ImportedSource> {
        self.entry(source_id).map(|entry| &entry.source)
    }

    pub fn mark_unavailable(&mut self, source_id: &DataSourceId, message: String) {
        if let Ok(entry) = self.entry_mut(source_id) {
            entry.source.status = SourceStatus::Failed(message);
        }
    }

    /// Removes viewer ownership without touching the native source path.
    pub fn remove(&mut self, source_id: &DataSourceId) -> Option<ImportedSource> {
        let index = self
            .entries
            .iter()
            .position(|entry| &entry.source.source_id == source_id)?;
        Some(self.entries.remove(index).source)
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
        mut request: ReadRequest,
    ) -> Result<(), SourceRegistryError> {
        let entry = self.entry_mut(source_id)?;
        let Some(worker) = entry.worker.as_ref() else {
            return Err(SourceRegistryError::Inactive(source_id.clone()));
        };
        entry.source.status = SourceStatus::Loading;
        if let ReadRequest::Discover(discovery) = &mut request {
            if !entry.source.project_allowlist.is_empty() {
                discovery.project_allowlist = Some(entry.source.project_allowlist.clone());
            }
            entry.source.catalog.metric_keys.clear();
            entry
                .catalog_requests
                .insert(generation, discovery.project_id.clone());
        }
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
        let catalog_project = entry.catalog_requests.remove(&event.generation).flatten();
        if let Ok(crate::data::worker::ReadSnapshot::Catalog(snapshot)) = &event.result {
            merge_catalog(
                &mut entry.source.catalog,
                snapshot,
                catalog_project.as_ref(),
            );
        }
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

fn merge_catalog(
    retained: &mut CatalogSnapshot,
    snapshot: &CatalogSnapshot,
    project_id: Option<&ProjectId>,
) {
    retained.projects.clone_from(&snapshot.projects);
    if let Some(project_id) = project_id {
        retained.runs.retain(|run| {
            snapshot
                .projects
                .iter()
                .any(|project| project.project_id == run.project_id)
                && &run.project_id != project_id
        });
    } else {
        retained.runs.clear();
    }
    retained.runs.extend(snapshot.runs.iter().cloned());
    retained.metric_keys.clone_from(&snapshot.metric_keys);
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

    use seex_model::run::{Run, RunId, RunStatus};
    use seex_model::types::{Project, ProjectId};

    use crate::config::ConfiguredSource;
    use crate::data::CatalogSnapshot;
    use crate::data::worker::{ReadKind, ReadSnapshot};
    use crate::domain::SourceAlias;

    use super::{
        DataSourceId, Generation, ReadEvent, ReadRequest, SourceRegistry, SourceRegistryError,
        SourceStatus, merge_catalog,
    };

    fn configured_source(alias: &str, path: &Path) -> ConfiguredSource {
        ConfiguredSource {
            alias: SourceAlias::new(alias).expect("test alias should be valid"),
            root_path: path.to_path_buf(),
            projects: vec![ProjectId::from_string("project")],
        }
    }

    #[test]
    fn configured_sources_are_retained_ordered_and_lazy() {
        let mut registry = SourceRegistry::default();
        let first = registry.configure(configured_source("first", Path::new("first")));
        registry.configure(configured_source("second", Path::new("second")));
        registry.configure(configured_source("first", Path::new("first")));

        assert_eq!(
            registry
                .sources()
                .map(|source| (&source.source_id, &source.status))
                .collect::<Vec<_>>(),
            [
                (&first, &SourceStatus::Dormant),
                (
                    &DataSourceId::new("second").expect("test alias should be valid"),
                    &SourceStatus::Dormant
                ),
            ]
        );
        assert!(registry.entries.iter().all(|entry| entry.worker.is_none()));
    }

    #[test]
    fn configured_source_keeps_alias_identity_when_its_path_changes() {
        let mut registry = SourceRegistry::default();
        let alias = SourceAlias::new("research").expect("test alias should be valid");
        let source_id = registry.configure(ConfiguredSource {
            alias: alias.clone(),
            root_path: Path::new("first").to_path_buf(),
            projects: vec![ProjectId::from_string("project-1")],
        });
        let configured_again = registry.configure(ConfiguredSource {
            alias,
            root_path: Path::new("second").to_path_buf(),
            projects: vec![ProjectId::from_string("project-2")],
        });

        assert_eq!(
            source_id,
            DataSourceId::new("research").expect("test alias should be valid")
        );
        assert_eq!(configured_again, source_id);
        assert_eq!(registry.entries.len(), 1);
        assert_eq!(registry.entries[0].source.root_path, Path::new("second"));
        assert_eq!(
            registry.entries[0].source.project_allowlist[0].as_str(),
            "project-2"
        );
        assert!(registry.entries[0].worker.is_none());
    }

    #[test]
    fn submitting_to_a_dormant_source_is_explicit() {
        let mut registry = SourceRegistry::default();
        let source_id = registry.configure(configured_source("missing", Path::new("missing")));

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
        let source_id = registry.configure(configured_source("missing", Path::new("missing")));

        let events = registry
            .activate(&source_id)
            .expect("worker thread should start")
            .expect("first activation should return its event stream");

        assert_eq!(
            registry.source(&source_id).map(|source| &source.status),
            Some(&SourceStatus::Loading)
        );
        drop(events);
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
        let first = registry.configure(configured_source("first", Path::new("first")));
        let second = registry.configure(configured_source("second", Path::new("second")));

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

    #[test]
    fn catalog_refreshes_retain_runs_loaded_for_other_projects() {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("fixed timestamp should parse");
        let projects = ["project-a", "project-b"]
            .map(|id| Project {
                project_id: ProjectId::from_string(id),
                name: id.to_owned(),
                created_at: timestamp,
            })
            .to_vec();
        let mut retained = CatalogSnapshot {
            projects: projects.clone(),
            runs: Vec::new(),
            metric_keys: Vec::new(),
        };
        for project in &projects {
            merge_catalog(
                &mut retained,
                &CatalogSnapshot {
                    projects: projects.clone(),
                    runs: vec![Run {
                        run_id: RunId::from_string(format!("run-{}", project.project_id.as_str())),
                        project_id: project.project_id.clone(),
                        name: "Run".to_owned(),
                        status: RunStatus::Finished,
                        created_at: timestamp,
                        started_at: timestamp,
                        finished_at: Some(timestamp),
                    }],
                    metric_keys: Vec::new(),
                },
                Some(&project.project_id),
            );
        }

        assert_eq!(retained.runs.len(), 2);
        assert!(projects.iter().all(|project| {
            retained
                .runs
                .iter()
                .any(|run| run.project_id == project.project_id)
        }));

        let replacement = retained.runs[1].clone();
        merge_catalog(
            &mut retained,
            &CatalogSnapshot {
                projects,
                runs: vec![replacement.clone()],
                metric_keys: Vec::new(),
            },
            None,
        );
        assert_eq!(retained.runs, [replacement]);
    }

    #[test]
    fn removing_a_source_does_not_delete_native_data() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let marker = root.path().join("native-data");
        std::fs::write(&marker, "retained")?;
        let mut registry = SourceRegistry::default();
        let source_id = registry.configure(configured_source("source", root.path()));

        let removed = registry
            .remove(&source_id)
            .expect("configured Source should be removable");

        assert_eq!(removed.root_path, root.path());
        assert_eq!(std::fs::read_to_string(marker)?, "retained");
        assert!(registry.source(&source_id).is_none());
        Ok(())
    }
}
