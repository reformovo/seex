use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::model::metric::MetricKey;
use crate::model::run::{Run, RunId, RunStatus};
use crate::model::types::{Project, ProjectId};
use chrono::{DateTime, Utc};

use crate::storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, flush_pending_in_locked_connection,
    open_existing_native_connection_with_config, open_existing_writer_connection_with_config,
    record_write_timing,
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
        self.write_guard_with_timeout(CATALOG_WRITE_TIMEOUT)
    }

    fn write_guard_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<CatalogWriteGuard>, StorageError> {
        self.write_config
            .as_ref()
            .map(|config| CatalogWriteGuard::acquire(config.catalog_path(), timeout))
            .transpose()
    }

    fn with_write<T>(
        &self,
        write: impl FnOnce(&duckdb::Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let started = Instant::now();
        let _guard = self.write_guard()?;
        record_write_timing("summary_lock_wait", started.elapsed());
        let held_since = Instant::now();
        let fresh = self.fresh_writer()?;
        let summary_started = Instant::now();
        let result = write(fresh.as_ref().unwrap_or(&self.connection));
        record_write_timing("summary_rebuild", summary_started.elapsed());
        record_write_timing("summary_lock_hold", held_since.elapsed());
        result
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

    pub(crate) fn uses_sqlite_catalog(&self) -> bool {
        self.write_config
            .as_ref()
            .is_some_and(|config| config.catalog_backend() == CatalogBackend::Sqlite)
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

    pub fn list_runs_page(
        &self,
        project_id: &ProjectId,
        after: Option<&Run>,
        limit: usize,
    ) -> Result<Vec<Run>, StorageError> {
        let sql = "SELECT run_id, project_id, name, status,
                          epoch_ms(created_at::TIMESTAMPTZ),
                          epoch_ms(started_at::TIMESTAMPTZ),
                          epoch_ms(finished_at::TIMESTAMPTZ)
                   FROM seex_runs WHERE project_id = ?";
        let tail = " ORDER BY created_at DESC, run_id DESC LIMIT ?";
        let cursor = " AND (created_at < ? OR (created_at = ? AND run_id < ?))";
        let decode = |row: &duckdb::Row<'_>| {
            Ok(StoredRun {
                run_id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                status: row.get(3)?,
                created_at_millis: row.get(4)?,
                started_at_millis: row.get(5)?,
                finished_at_millis: row.get(6)?,
            })
        };
        let stored = if let Some(after) = after {
            let mut statement = self.connection.prepare(&format!("{sql}{cursor}{tail}"))?;
            let created_at = timestamp_as_rfc3339(after.created_at);
            statement
                .query_map(
                    (
                        project_id.as_str(),
                        created_at.as_str(),
                        created_at.as_str(),
                        after.run_id.as_str(),
                        limit as i64,
                    ),
                    decode,
                )?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut statement = self.connection.prepare(&format!("{sql}{tail}"))?;
            statement
                .query_map((project_id.as_str(), limit as i64), decode)?
                .collect::<Result<Vec<_>, _>>()?
        };
        stored.into_iter().map(StoredRun::into_run).collect()
    }

    pub fn metric_key_exists(&self, metric_key: &MetricKey) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM seex_metric_keys WHERE metric_key = ?)",
                [metric_key.as_str()],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
    }

    pub fn metric_keys_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MetricKey>, StorageError> {
        let upper = prefix_upper_bound(prefix);
        let sql = "SELECT metric_key FROM seex_metric_keys
                   WHERE metric_key >= ? AND metric_key > ? AND starts_with(metric_key, ?)";
        let tail = " ORDER BY metric_key LIMIT ?";
        let keys = if let Some(upper) = upper {
            let mut statement = self
                .connection
                .prepare(&format!("{sql} AND metric_key < ?{tail}"))?;
            statement
                .query_map(
                    (
                        prefix,
                        after.unwrap_or(""),
                        prefix,
                        upper.as_str(),
                        limit as i64,
                    ),
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut statement = self.connection.prepare(&format!("{sql}{tail}"))?;
            statement
                .query_map((prefix, after.unwrap_or(""), prefix, limit as i64), |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(keys.into_iter().map(MetricKey::from_string).collect())
    }

    pub fn series_keys_page(
        &self,
        run_id: &RunId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MetricKey>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT directory.metric_key FROM seex_series_directory AS directory
             JOIN seex_runs AS run ON run.run_id = directory.run_id
             JOIN seex_projects AS project ON project.project_id = run.project_id
             WHERE directory.run_id = ? AND directory.status = 'published'
               AND directory.metric_key > ?
             ORDER BY directory.metric_key LIMIT ?",
        )?;
        let rows = statement.query_map(
            (run_id.as_str(), after.unwrap_or(""), limit as i64),
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| Ok(MetricKey::from_string(row?))).collect()
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
        let fresh = if self.uses_sqlite_catalog() {
            self.fresh_writer()?
        } else {
            None
        };
        let connection = fresh.as_ref().unwrap_or(&self.connection);
        connection.execute_batch("BEGIN TRANSACTION")?;
        let result = (|| {
            let updated = connection.execute(
                "UPDATE seex_runs
                 SET status = ?, finished_at = ?
                 WHERE run_id = ? AND status = 'running'",
                (
                    status_as_str(status),
                    timestamp_as_rfc3339(finished_at),
                    run_id.as_str(),
                ),
            )?;
            if updated > 0 {
                connection.execute(
                    "UPDATE seex_storage_metadata SET value = 1 WHERE key = 'flush_pending'",
                    [],
                )?;
            }
            Ok(updated > 0)
        })();
        match result {
            Ok(updated) => {
                if let Err(error) = connection.execute_batch("COMMIT") {
                    let _ = connection.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                Ok(updated)
            }
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn flush_metric_points(&self) -> Result<(), StorageError> {
        self.flush_metric_points_with_deadline(None).0
    }

    pub(crate) fn flush_metric_points_with_deadline(
        &self,
        deadline: Option<Instant>,
    ) -> (Result<(), StorageError>, bool) {
        self.flush_metric_points_impl(deadline, false)
    }

    pub(crate) fn flush_pending_metric_points_with_deadline(
        &self,
        deadline: Option<Instant>,
    ) -> (Result<(), StorageError>, bool) {
        self.flush_metric_points_impl(deadline, true)
    }

    fn flush_metric_points_impl(
        &self,
        deadline: Option<Instant>,
        only_pending: bool,
    ) -> (Result<(), StorageError>, bool) {
        let started = Instant::now();
        let guard_timeout = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(CATALOG_WRITE_TIMEOUT);
        let guard = match self.write_guard_with_timeout(guard_timeout) {
            Ok(guard) => guard,
            Err(error) => {
                let timed_out = matches!(error, StorageError::CatalogWriteLockTimeout);
                return (Err(error), timed_out);
            }
        };
        record_write_timing("flush_lock_wait", started.elapsed());
        let held_since = Instant::now();
        let completed = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let fresh = match self.fresh_writer() {
            Ok(fresh) => fresh,
            Err(error) => return (Err(error), false),
        };
        let connection = fresh.as_ref().unwrap_or(&self.connection);
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
        let flush_started = Instant::now();
        let result = if only_pending {
            if self.uses_sqlite_catalog() {
                flush_pending_in_locked_connection(connection).map(|_| ())
            } else {
                (|| {
                    let pending: i64 = self.connection.query_row(
                        "SELECT value FROM seex_storage_metadata WHERE key = 'flush_pending'",
                        [],
                        |row| row.get(0),
                    )?;
                    if pending != 0 {
                        connection.execute_batch(
                            "CALL ducklake_flush_inlined_data('dl', table_name => 'metric_points');",
                        )?;
                        self.connection.execute(
                            "UPDATE seex_storage_metadata SET value = 0 WHERE key = 'flush_pending'",
                            [],
                        )?;
                    }
                    Ok(())
                })()
            }
        } else {
            connection
                .execute_batch(
                    "CALL ducklake_flush_inlined_data('dl', table_name => 'metric_points');",
                )
                .map_err(StorageError::from)
        };
        record_write_timing("inline_flush", flush_started.elapsed());
        completed.store(true, Ordering::Relaxed);
        drop(guard);
        record_write_timing("flush_lock_hold", held_since.elapsed());
        let timed_out = timed_out.load(Ordering::Relaxed)
            || deadline.is_some_and(|deadline| Instant::now() >= deadline);
        (result, timed_out)
    }

    pub fn append_metric_batch(&self, rows: &[MetricWrite]) -> Result<(), StorageError> {
        let started = Instant::now();
        let _guard = self.write_guard()?;
        record_write_timing("append_lock_wait", started.elapsed());
        let held_since = Instant::now();
        let fresh = self.fresh_writer()?;
        let connection = fresh.as_ref().unwrap_or(&self.connection);
        let series = rows
            .iter()
            .map(|row| (row.run_id.as_str(), row.metric_key.as_str()))
            .collect::<HashSet<_>>();
        connection.execute_batch("BEGIN TRANSACTION")?;
        let directory_result: Result<(), StorageError> = (|| {
            for (run_id, metric_key) in &series {
                connection.execute(
                    "INSERT INTO seex_metric_keys(metric_key)
                     SELECT ? WHERE NOT EXISTS (
                         SELECT 1 FROM seex_metric_keys WHERE metric_key = ?
                     )",
                    [metric_key, metric_key],
                )?;
                connection.execute(
                    "INSERT INTO seex_series_directory(run_id, metric_key, status)
                     SELECT ?, ?, 'pending' WHERE NOT EXISTS (
                         SELECT 1 FROM seex_series_directory WHERE run_id = ? AND metric_key = ?
                     )",
                    [run_id, metric_key, run_id, metric_key],
                )?;
            }
            Ok(())
        })();
        if let Err(error) = directory_result {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        connection.execute_batch("COMMIT")?;
        let append_started = Instant::now();
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
        drop(appender);
        connection.execute_batch("BEGIN TRANSACTION")?;
        for (run_id, metric_key) in series {
            if let Err(error) = connection.execute(
                "UPDATE seex_series_directory SET status = 'published'
                 WHERE run_id = ? AND metric_key = ? AND status = 'pending'",
                [run_id, metric_key],
            ) {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error.into());
            }
        }
        connection.execute_batch("COMMIT")?;
        record_write_timing("batch_append", append_started.elapsed());
        record_write_timing("append_lock_hold", held_since.elapsed());
        Ok(())
    }
}

impl Deref for ProjectConnection {
    type Target = duckdb::Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut chars = prefix.chars().collect::<Vec<_>>();
    while let Some(last) = chars.pop() {
        let mut next = u32::from(last) + 1;
        while next <= 0x10ffff {
            if let Some(next_char) = char::from_u32(next) {
                chars.push(next_char);
                return Some(chars.into_iter().collect());
            }
            next += 1;
        }
    }
    None
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
    use crate::storage::bootstrap::{CatalogBackend, open_native_connection_with_config};

    #[test]
    fn metric_key_prefix_pages_are_exact_and_stable() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            CatalogBackend::Sqlite,
            root.path(),
            None,
            None,
            None,
        );
        let store = ProjectConnection::new(open_native_connection_with_config(config)?);
        for key in ["loss", "loss/train", "loss/val", "other", "零/损失"] {
            store.execute("INSERT INTO seex_metric_keys VALUES (?)", [key])?;
        }
        let first = store.metric_keys_page("loss", None, 2)?;
        let second = store.metric_keys_page("loss", first.last().map(MetricKey::as_str), 2)?;
        assert_eq!(
            first.iter().map(MetricKey::as_str).collect::<Vec<_>>(),
            ["loss", "loss/train"]
        );
        assert_eq!(
            second.iter().map(MetricKey::as_str).collect::<Vec<_>>(),
            ["loss/val"]
        );
        let after_exact_prefix = store.metric_keys_page("loss", Some("loss"), 2)?;
        assert_eq!(after_exact_prefix.len(), 2);
        assert!(store.metric_key_exists(&MetricKey::from_string("零/损失"))?);
        assert!(!store.metric_key_exists(&MetricKey::from_string("missing"))?);
        assert_eq!(prefix_upper_bound("loss").as_deref(), Some("lost"));
        Ok(())
    }

    #[test]
    fn run_page_cursor_handles_equal_creation_times() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            CatalogBackend::Sqlite,
            root.path(),
            None,
            None,
            None,
        );
        let connection = ProjectConnection::new(open_native_connection_with_config(config)?);
        for index in 0..105 {
            connection.execute(
                "INSERT INTO seex_runs VALUES (?, 'project', 'run', 'running',
                 '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00', NULL)",
                [format!("run-{index:03}")],
            )?;
        }
        let project = ProjectId::from_string("project");
        let first = connection.list_runs_page(&project, None, 100)?;
        let second = connection.list_runs_page(&project, first.last(), 100)?;
        assert_eq!(first.len(), 100);
        assert_eq!(second.len(), 5);
        assert_eq!(first[0].run_id.as_str(), "run-104");
        assert_eq!(second[4].run_id.as_str(), "run-000");
        Ok(())
    }

    #[test]
    fn series_directory_publishes_only_actual_sequences_and_repairs_pending()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            CatalogBackend::Sqlite,
            root.path(),
            None,
            None,
            None,
        );
        let connection = open_native_connection_with_config(config.clone())?;
        let store = ProjectConnection::with_catalog_write_lock(connection, config.clone());
        store.execute(
            "INSERT INTO seex_projects VALUES ('project', 'project', '2026-01-01T00:00:00+00:00')",
            [],
        )?;
        store.execute(
            "INSERT INTO seex_runs VALUES ('run-a', 'project', 'run-a', 'running',
             '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00', NULL)",
            [],
        )?;
        store.append_metric_batch(&[MetricWrite {
            run_id: "run-a".to_owned(),
            metric_key: "loss".to_owned(),
            step: 0,
            timestamp_millis: 1_700_000_000_000,
            value_f64: 0.5,
            ingested_at_millis: 1_700_000_000_000,
        }])?;
        let reader = store.fresh_read()?;
        let published: i64 = reader.query_row(
            "SELECT count(*) FROM seex_series_directory WHERE status = 'published'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(published, 1);
        drop(reader);
        store.execute(
            "UPDATE seex_series_directory SET status = 'pending' WHERE run_id = 'run-a'",
            [],
        )?;
        store.execute("INSERT INTO seex_metric_keys VALUES ('orphan')", [])?;
        store.execute(
            "INSERT INTO seex_series_directory VALUES ('run-a', 'orphan', 'pending')",
            [],
        )?;
        drop(store);

        let reopened = ProjectConnection::new(open_native_connection_with_config(config)?);
        let statuses = reopened
            .prepare("SELECT metric_key, status FROM seex_series_directory ORDER BY metric_key")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(statuses, vec![("loss".to_owned(), "published".to_owned())]);
        let keys: i64 = reopened.query_row("SELECT count(*) FROM seex_metric_keys", [], |row| {
            row.get(0)
        })?;
        assert_eq!(keys, 1);
        assert_eq!(
            reopened
                .series_keys_page(&RunId::from_string("run-a"), None, 100)?
                .len(),
            1
        );
        reopened.execute("DELETE FROM seex_runs WHERE run_id = 'run-a'", [])?;
        assert!(
            reopened
                .series_keys_page(&RunId::from_string("run-a"), None, 100)?
                .is_empty()
        );
        Ok(())
    }

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
