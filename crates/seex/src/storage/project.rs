use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::model::run::{Run, RunId, RunStatus};
use crate::model::types::{Project, ProjectId};
use chrono::{DateTime, Utc};

use crate::storage::bootstrap::{
    NativeStorageConfig, open_existing_native_connection_with_config,
    open_existing_writer_connection_with_config,
};
use crate::storage::rows::{StoredRun, status_as_str};
use crate::storage::time::{timestamp_as_rfc3339, timestamp_from_millis};
use crate::storage::write::NativeWriteStore;
use crate::storage::{CatalogWriteGuard, StorageError, percent_encode_metric_key};

const CATALOG_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Owning connection for one native Seex project store.
pub struct ProjectConnection {
    connection: duckdb::Connection,
    write_config: Option<NativeStorageConfig>,
}

/// Cloneable cancellation capability bound to one native connection.
#[derive(Clone)]
pub struct ReadInterrupt(Arc<duckdb::InterruptHandle>);

impl ReadInterrupt {
    pub fn interrupt(&self) {
        self.0.interrupt();
    }
}

impl ProjectConnection {
    pub const fn new(connection: duckdb::Connection) -> Self {
        Self {
            connection,
            write_config: None,
        }
    }

    pub(crate) fn with_catalog_write_lock(
        connection: duckdb::Connection,
        config: NativeStorageConfig,
    ) -> Self {
        Self {
            connection,
            write_config: Some(config),
        }
    }

    fn write_guard(&self) -> Result<Option<CatalogWriteGuard>, StorageError> {
        self.write_config
            .as_ref()
            .map(|config| CatalogWriteGuard::acquire(config.catalog_path(), CATALOG_WRITE_TIMEOUT))
            .transpose()
    }

    fn with_write<T>(
        &self,
        write: impl FnOnce(&duckdb::Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let _guard = self.write_guard()?;
        let fresh = self.fresh_writer()?;
        write(fresh.as_ref().unwrap_or(&self.connection))
    }

    fn fresh_writer(&self) -> Result<Option<duckdb::Connection>, StorageError> {
        self.write_config
            .as_ref()
            .map(|config| open_existing_writer_connection_with_config(config.clone()))
            .transpose()
    }

    pub fn try_clone(&self) -> Result<Self, StorageError> {
        let connection = self.connection.try_clone()?;
        connection.execute_batch("USE seex_catalog;")?;
        Ok(Self {
            connection,
            write_config: self.write_config.clone(),
        })
    }

    pub(crate) fn fresh_read(&self) -> Result<Self, StorageError> {
        let Some(config) = self.write_config.as_ref() else {
            return self.try_clone();
        };
        Ok(Self::new(open_existing_native_connection_with_config(
            config.clone(),
        )?))
    }

    #[doc(hidden)]
    pub fn interrupt_handle(&self) -> ReadInterrupt {
        ReadInterrupt(self.connection.interrupt_handle())
    }

    pub fn create_project(&self, project: &Project) -> Result<(), StorageError> {
        let _guard = self.write_guard()?;
        self.connection.execute(
            "INSERT INTO seex_projects (project_id, name, created_at) VALUES (?, ?, ?)",
            (
                project.project_id.as_str(),
                project.name.as_str(),
                timestamp_as_rfc3339(project.created_at),
            ),
        )?;
        Ok(())
    }

    pub fn get_project(&self, project_id: &ProjectId) -> Result<Option<Project>, StorageError> {
        let result = self.connection.query_row(
            "SELECT project_id, name, epoch_ms(created_at::TIMESTAMPTZ)
             FROM seex_projects WHERE project_id = ?",
            [project_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        );
        match result {
            Ok((project_id, name, created_at_millis)) => Ok(Some(Project {
                project_id: ProjectId::from_string(project_id),
                name,
                created_at: timestamp_from_millis("created_at", created_at_millis)?,
            })),
            Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
            Err(source) => Err(source.into()),
        }
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT project_id, name, epoch_ms(created_at::TIMESTAMPTZ)
             FROM seex_projects ORDER BY created_at, project_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (project_id, name, created_at_millis) = row?;
            Ok(Project {
                project_id: ProjectId::from_string(project_id),
                name,
                created_at: timestamp_from_millis("created_at", created_at_millis)?,
            })
        })
        .collect()
    }

    pub fn project_exists(&self, project_id: &ProjectId) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM seex_projects WHERE project_id = ?
                 )",
                [project_id.as_str()],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
    }

    pub fn create_run(
        &self,
        project_id: &ProjectId,
        name: &str,
        run_id: RunId,
    ) -> Result<Run, StorageError> {
        let _guard = self.write_guard()?;
        NativeWriteStore::new(&self.connection).create_run(project_id, name, Some(run_id))
    }

    pub fn get_run(&self, run_id: &RunId) -> Result<Run, StorageError> {
        NativeWriteStore::new(&self.connection).resume_run(run_id)
    }

    pub fn get_runs(&self, run_ids: &[RunId]) -> Result<Vec<Run>, StorageError> {
        if run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let requested_rows = (0..run_ids.len())
            .map(|ordinal| format!("(?, {ordinal})"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "WITH requested(run_id, ordinal) AS (VALUES {requested_rows})
             SELECT requested.run_id, stored.run_id IS NOT NULL,
                    stored.project_id, stored.name, stored.status,
                    epoch_ms(stored.created_at::TIMESTAMPTZ),
                    epoch_ms(stored.started_at::TIMESTAMPTZ),
                    epoch_ms(stored.finished_at::TIMESTAMPTZ)
             FROM requested
             LEFT JOIN seex_runs AS stored USING (run_id)
             ORDER BY requested.ordinal"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            duckdb::params_from_iter(run_ids.iter().map(RunId::as_str)),
            |row| {
                let requested_run_id: String = row.get(0)?;
                let found: bool = row.get(1)?;
                let stored = if found {
                    Some(StoredRun {
                        run_id: requested_run_id.clone(),
                        project_id: row.get(2)?,
                        name: row.get(3)?,
                        status: row.get(4)?,
                        created_at_millis: row.get(5)?,
                        started_at_millis: row.get(6)?,
                        finished_at_millis: row.get(7)?,
                    })
                } else {
                    None
                };
                Ok((requested_run_id, stored))
            },
        )?;
        rows.map(|row| {
            let (run_id, stored) = row?;
            stored
                .ok_or(StorageError::RunNotFound { run_id })?
                .into_run()
        })
        .collect()
    }

    pub fn list_runs(
        &self,
        project_id: &ProjectId,
        status: Option<RunStatus>,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<Run>, StorageError> {
        let mut sql = String::from(
            "SELECT run_id
             FROM seex_runs
             WHERE project_id = ? AND status = COALESCE(?, status)
             ORDER BY created_at, run_id",
        );
        if let Some(limit) = limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        } else if offset > 0 {
            sql.push_str(" LIMIT ALL");
        }
        if offset > 0 {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement
            .query_map((project_id.as_str(), status.map(status_as_str)), |row| {
                Ok(RunId::from_string(row.get::<_, String>(0)?))
            })?;
        let run_ids = rows.collect::<Result<Vec<_>, _>>()?;
        run_ids.iter().map(|run_id| self.get_run(run_id)).collect()
    }

    pub fn list_orphan_runs(
        &self,
        project_id: Option<&ProjectId>,
    ) -> Result<Vec<Run>, StorageError> {
        let run_ids = match project_id {
            Some(project_id) => {
                let mut statement = self.connection.prepare(
                    "SELECT run_id FROM seex_runs
                     WHERE project_id = ? AND status = 'running'
                     ORDER BY created_at, run_id",
                )?;
                statement
                    .query_map([project_id.as_str()], |row| {
                        Ok(RunId::from_string(row.get::<_, String>(0)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            }
            None => {
                let mut statement = self.connection.prepare(
                    "SELECT run_id FROM seex_runs
                     WHERE status = 'running' ORDER BY created_at, run_id",
                )?;
                statement
                    .query_map([], |row| Ok(RunId::from_string(row.get::<_, String>(0)?)))?
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        run_ids.iter().map(|run_id| self.get_run(run_id)).collect()
    }

    pub fn rebuild_metric_aggregates_for_run(&self, run_id: &RunId) -> Result<(), StorageError> {
        self.with_write(|connection| {
            NativeWriteStore::new(connection).rebuild_metric_aggregates_for_run(run_id)
        })
    }

    pub fn mark_run_terminal(
        &self,
        run_id: &RunId,
        status: RunStatus,
        finished_at: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let _guard = self.write_guard()?;
        let updated = self.connection.execute(
            "UPDATE seex_runs
             SET status = ?, finished_at = ?
             WHERE run_id = ? AND status = 'running'",
            (
                status_as_str(status),
                timestamp_as_rfc3339(finished_at),
                run_id.as_str(),
            ),
        )?;
        Ok(updated > 0)
    }

    pub fn flush_metric_points(&self) -> Result<(), StorageError> {
        self.flush_metric_points_with_deadline(None).0
    }

    pub(crate) fn flush_metric_points_with_deadline(
        &self,
        deadline: Option<Instant>,
    ) -> (Result<(), StorageError>, bool) {
        let completed = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let result = self.with_write(|connection| {
            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let interrupt = connection.interrupt_handle();
                let timer_completed = Arc::clone(&completed);
                let timer_timed_out = Arc::clone(&timed_out);
                std::thread::spawn(move || {
                    std::thread::sleep(remaining);
                    if !timer_completed.load(Ordering::Relaxed) {
                        timer_timed_out.store(true, Ordering::Relaxed);
                        interrupt.interrupt();
                    }
                });
            }
            let result = connection
                .execute_batch(
                    "CALL ducklake_flush_inlined_data('dl', table_name => 'metric_points');",
                )
                .map_err(StorageError::from);
            completed.store(true, Ordering::Relaxed);
            result
        });
        let timed_out = timed_out.load(Ordering::Relaxed)
            || deadline.is_some_and(|deadline| Instant::now() >= deadline);
        (result, timed_out)
    }

    pub fn append_metric_batch(&self, rows: &[MetricWrite]) -> Result<(), StorageError> {
        let _guard = self.write_guard()?;
        let fresh = self.fresh_writer()?;
        let connection = fresh.as_ref().unwrap_or(&self.connection);
        let mut appender = connection.appender_with_columns_to_catalog_and_db(
            "metric_points",
            "dl",
            "main",
            &[
                "run_id",
                "metric_key",
                "metric_key_encoded",
                "step",
                "timestamp",
                "value_f64",
                "ingested_at",
            ],
        )?;
        for row in rows {
            let timestamp = timestamp_from_millis("timestamp", row.timestamp_millis)?;
            let ingested_at = timestamp_from_millis("ingested_at", row.ingested_at_millis)?;
            let encoded_key = percent_encode_metric_key(&row.metric_key);
            appender.append_row(duckdb::params![
                row.run_id.as_str(),
                row.metric_key.as_str(),
                encoded_key.as_str(),
                row.step,
                timestamp_as_rfc3339(timestamp),
                row.value_f64,
                timestamp_as_rfc3339(ingested_at),
            ])?;
        }
        appender.flush()?;
        Ok(())
    }
}

impl Deref for ProjectConnection {
    type Target = duckdb::Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

/// One accepted metric write prepared by the Core background worker.
pub struct MetricWrite {
    pub run_id: String,
    pub metric_key: String,
    pub step: i64,
    pub timestamp_millis: i64,
    pub value_f64: f64,
    pub ingested_at_millis: i64,
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn read_interrupt_stops_the_bound_connection_query() -> Result<(), Box<dyn std::error::Error>> {
        let connection = ProjectConnection::new(duckdb::Connection::open_in_memory()?);
        let interrupt = connection.interrupt_handle();
        let query = thread::spawn(move || {
            connection.query_row(
                "SELECT sum(sin(i::DOUBLE)) FROM range(1000000000) AS values(i)",
                [],
                |row| row.get::<_, f64>(0),
            )
        });

        thread::sleep(Duration::from_millis(25));
        interrupt.interrupt();

        assert!(
            query
                .join()
                .expect("query thread should not panic")
                .is_err()
        );
        Ok(())
    }
}
