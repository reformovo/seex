use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use seex::{LocalReaderError, Reader, ReaderInterrupt};
use seex_storage::StorageError;

use crate::data::{CatalogSnapshot, DiscoveryRequest};

/// Failures while opening an existing viewer source.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("S3 data paths are unsupported by seex-app")]
    UnsupportedS3,
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Sdk(#[from] seex::Error),
}

/// Read session for one existing local native Seex store.
pub struct ReadSession {
    root_path: PathBuf,
    reader: Reader,
}

impl ReadSession {
    /// Opens an existing store through the shared project configuration.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::UnsupportedS3`] before credential resolution for
    /// an S3 data path. Other configuration and storage failures are preserved.
    pub fn open_existing(root_path: &Path) -> Result<Self, SourceError> {
        let reader = open_local_reader(root_path)?;
        Ok(Self {
            root_path: root_path.to_owned(),
            reader,
        })
    }

    pub(crate) fn try_clone(&self) -> Result<Self, SourceError> {
        Ok(Self {
            root_path: self.root_path.clone(),
            reader: open_local_reader(&self.root_path)?,
        })
    }

    /// Discovers Projects, Runs, and the selected Runs' metric union.
    ///
    /// Runs are newest-first. A Project or Run removed since the request was
    /// created is reconciled to an empty or reduced result rather than an error.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when a catalog query fails.
    pub fn discover(&self, request: &DiscoveryRequest) -> Result<CatalogSnapshot, SourceError> {
        let mut projects = self.reader.projects()?;
        if let Some(allowlist) = &request.project_allowlist {
            let allowed = allowlist.iter().collect::<HashSet<_>>();
            projects.retain(|project| allowed.contains(&project.project_id));
        }
        let project_id = request.project_id.as_ref().filter(|project_id| {
            projects
                .iter()
                .any(|project| &project.project_id == *project_id)
        });
        let projects_to_load = project_id.map_or_else(
            || projects.iter().map(|project| &project.project_id).collect(),
            |project_id| vec![project_id],
        );
        let mut runs = Vec::new();
        for project_id in projects_to_load {
            let mut project_runs = self.reader.runs(project_id)?;
            project_runs.reverse();
            runs.extend(project_runs);
        }
        let mut requested = request.metric_runs.iter().cloned().collect::<HashSet<_>>();
        if let Some(project_id) = project_id {
            requested.extend(
                request
                    .selected_run_ids
                    .iter()
                    .cloned()
                    .map(|run_id| (project_id.clone(), run_id)),
            );
        }
        requested.retain(|(project_id, _)| {
            projects
                .iter()
                .any(|project| &project.project_id == project_id)
        });
        let mut known_runs = runs
            .iter()
            .map(|run| ((run.project_id.clone(), run.run_id.clone()), run.clone()))
            .collect::<HashMap<_, _>>();
        for requested_project in requested
            .iter()
            .map(|(project_id, _)| project_id)
            .collect::<HashSet<_>>()
        {
            if project_id == Some(requested_project) {
                continue;
            }
            known_runs.extend(
                self.reader
                    .runs(requested_project)?
                    .into_iter()
                    .map(|run| ((run.project_id.clone(), run.run_id.clone()), run)),
            );
        }
        let mut metric_keys = BTreeMap::new();
        for (project_id, run_id) in requested {
            let Some(run) = known_runs.get(&(project_id, run_id)) else {
                continue;
            };
            for aggregate in self.reader.metrics(run)? {
                metric_keys.insert(
                    aggregate.metric_key.as_str().to_owned(),
                    aggregate.metric_key,
                );
            }
        }
        Ok(CatalogSnapshot {
            projects,
            runs,
            metric_keys: metric_keys.into_values().collect(),
        })
    }

    pub(crate) const fn reader(&self) -> &Reader {
        &self.reader
    }

    #[doc(hidden)]
    pub fn interrupt_handles(&self) -> Vec<ReaderInterrupt> {
        self.reader.interrupt_handle().into_iter().collect()
    }
}

fn open_local_reader(root_path: &Path) -> Result<Reader, SourceError> {
    Reader::builder(root_path)
        .open_local()
        .map_err(|error| match error {
            LocalReaderError::UnsupportedS3 => SourceError::UnsupportedS3,
            LocalReaderError::Sdk(seex::Error::CatalogNotFound { name }) => {
                SourceError::Storage(StorageError::CatalogNotFound { name })
            }
            LocalReaderError::Sdk(error) => SourceError::Sdk(error),
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use seex_core::engine::client::NativeClient;
    use seex_model::run::RunId;
    use seex_model::types::ProjectId;

    use super::{DiscoveryRequest, ReadSession, SourceError};

    #[test]
    fn s3_is_rejected_before_missing_credentials_are_resolved()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config_dir = root.path().join(".seex");
        fs::create_dir(&config_dir)?;
        fs::write(
            config_dir.join("config.toml"),
            "schema_version = 1\ndata_path = \"s3://bucket/data\"\n",
        )?;

        let error = ReadSession::open_existing(root.path()).err();

        assert!(matches!(error, Some(SourceError::UnsupportedS3)));
        Ok(())
    }

    #[test]
    fn discovery_returns_newest_runs_and_selected_metric_union()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = NativeClient::open(root.path())?;
        let project = client.create_project("viewer", Some(ProjectId::from_string("project-1")))?;
        let first = client.create_run(
            &project.project_id,
            "first",
            Some(RunId::from_string("run-1")),
        )?;
        client
            .run_handle(first.clone())
            .log_metric_at_step("loss", 0, 1.0)?;
        client.finish_run(&first.run_id)?;
        let second = client.create_run(
            &project.project_id,
            "second",
            Some(RunId::from_string("run-2")),
        )?;
        client
            .run_handle(second.clone())
            .log_metric_at_step("accuracy", 0, 0.5)?;
        client.finish_run(&second.run_id)?;
        let other_project =
            client.create_project("other", Some(ProjectId::from_string("project-2")))?;
        let other = client.create_run(
            &other_project.project_id,
            "other",
            Some(RunId::from_string("run-3")),
        )?;
        client
            .run_handle(other.clone())
            .log_metric_at_step("latency", 0, 2.)?;
        client.finish_run(&other.run_id)?;
        client.shutdown(None)?;

        let session = ReadSession::open_existing(root.path())?;
        let snapshot = session.discover(&DiscoveryRequest {
            project_allowlist: None,
            project_id: Some(project.project_id),
            selected_run_ids: vec![first.run_id, RunId::from_string("removed")],
            metric_runs: vec![(other_project.project_id, other.run_id)],
        })?;

        assert_eq!(
            snapshot
                .runs
                .iter()
                .map(|run| run.run_id.as_str())
                .collect::<Vec<_>>(),
            ["run-2", "run-1"]
        );
        assert_eq!(
            snapshot
                .metric_keys
                .iter()
                .map(|key| key.as_str())
                .collect::<Vec<_>>(),
            ["latency", "loss"]
        );
        let all_runs = session.try_clone()?.discover(&DiscoveryRequest {
            project_allowlist: Some(vec![ProjectId::from_string("project-1")]),
            ..DiscoveryRequest::default()
        })?;
        assert_eq!(all_runs.projects.len(), 1);
        assert_eq!(
            all_runs
                .runs
                .iter()
                .map(|run| run.run_id.as_str())
                .collect::<Vec<_>>(),
            ["run-2", "run-1"]
        );
        Ok(())
    }
}
