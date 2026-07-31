use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use seex_storage::bootstrap::{
    NativeStorageConfig, is_s3_data_path, open_existing_native_connection_with_config,
};
use seex_storage::config::{InitConfigError, resolve_storage_config};
use seex_storage::{ProjectConnection, ProjectMetricReader, StorageError};

use crate::data::{CatalogSnapshot, DiscoveryRequest};

/// Failures while opening an existing viewer source.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("S3 data paths are unsupported by seex-app")]
    UnsupportedS3,
    #[error(transparent)]
    Config(#[from] InitConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Read session for one existing local native Seex store.
pub struct ReadSession {
    connection: ProjectConnection,
}

impl ReadSession {
    /// Opens an existing store through the shared project configuration.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::UnsupportedS3`] before credential resolution for
    /// an S3 data path. Other configuration and storage failures are preserved.
    pub fn open_existing(root_path: &Path) -> Result<Self, SourceError> {
        let resolved = resolve_storage_config(root_path, None, None, None)?;
        if resolved.data_path.as_deref().is_some_and(is_s3_data_path) {
            return Err(SourceError::UnsupportedS3);
        }
        let config = NativeStorageConfig::with_backend_and_s3_config(
            resolved.catalog_backend,
            root_path,
            resolved.catalog_path,
            resolved.data_path,
            None,
        );
        let connection = open_existing_native_connection_with_config(config)?;
        Ok(Self {
            connection: ProjectConnection::new(connection),
        })
    }

    pub(crate) fn try_clone(&self) -> Result<Self, SourceError> {
        Ok(Self {
            connection: self.connection.try_clone()?,
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
        let projects = self.connection.list_projects()?;
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
            let mut project_runs = self.connection.list_runs(project_id, None, None, 0)?;
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
        let mut statuses = runs
            .iter()
            .map(|run| ((run.project_id.clone(), run.run_id.clone()), run.status))
            .collect::<HashMap<_, _>>();
        for requested_project in requested
            .iter()
            .map(|(project_id, _)| project_id)
            .collect::<HashSet<_>>()
        {
            if project_id == Some(requested_project) {
                continue;
            }
            statuses.extend(
                self.connection
                    .list_runs(requested_project, None, None, 0)?
                    .into_iter()
                    .map(|run| ((run.project_id, run.run_id), run.status)),
            );
        }
        let reader = ProjectMetricReader::new(&self.connection);
        let mut metric_keys = BTreeMap::new();
        for (project_id, run_id) in requested {
            let Some(status) = statuses.get(&(project_id, run_id.clone())) else {
                continue;
            };
            for aggregate in reader.list_metrics(&run_id, *status)? {
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

    pub(crate) const fn connection(&self) -> &ProjectConnection {
        &self.connection
    }
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
            "data_path = \"s3://bucket/data\"\n",
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
        let all_runs = session
            .try_clone()?
            .discover(&DiscoveryRequest::default())?;
        assert_eq!(
            all_runs
                .runs
                .iter()
                .map(|run| run.run_id.as_str())
                .collect::<Vec<_>>(),
            ["run-2", "run-1", "run-3"]
        );
        Ok(())
    }
}
