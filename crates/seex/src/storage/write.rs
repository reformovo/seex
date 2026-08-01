use chrono::{DateTime, Utc};

use crate::storage::rows::{StoredRun, status_as_str};
use crate::storage::time::{current_timestamp, timestamp_as_rfc3339};
use crate::storage::{StorageError, percent_encode_metric_key};
use seex_model::metric::{MetricKey, MetricPoint, Step};
use seex_model::run::{Run, RunId, RunStatus};
use seex_model::types::ProjectId;

pub struct NativeWriteStore<'connection> {
    connection: &'connection duckdb::Connection,
}

impl<'connection> NativeWriteStore<'connection> {
    pub const fn new(connection: &'connection duckdb::Connection) -> Self {
        Self { connection }
    }

    pub fn create_run(
        &self,
        project_id: &ProjectId,
        name: &str,
        run_id: Option<RunId>,
    ) -> Result<Run, StorageError> {
        let run_id = run_id.unwrap_or_else(|| RunId::from_string(uuid::Uuid::new_v4().to_string()));
        if self.run_exists(&run_id)? {
            return Err(StorageError::RunAlreadyExists {
                run_id: run_id.as_str().to_owned(),
            });
        }

        let now = current_timestamp("created_at")?;
        self.connection.execute(
            "INSERT INTO seex_runs
                 (run_id, project_id, name, status, created_at, started_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, NULL)",
            (
                run_id.as_str(),
                project_id.as_str(),
                name,
                status_as_str(RunStatus::Running),
                timestamp_as_rfc3339(now),
                timestamp_as_rfc3339(now),
            ),
        )?;

        Ok(Run {
            run_id,
            project_id: project_id.clone(),
            name: name.to_owned(),
            status: RunStatus::Running,
            created_at: now,
            started_at: now,
            finished_at: None,
        })
    }

    pub fn resume_run(&self, run_id: &RunId) -> Result<Run, StorageError> {
        let result = self.connection.query_row(
            "SELECT run_id, project_id, name, status,
                    epoch_ms(created_at::TIMESTAMPTZ),
                    epoch_ms(started_at::TIMESTAMPTZ),
                    epoch_ms(finished_at::TIMESTAMPTZ)
             FROM seex_runs
             WHERE run_id = ?",
            [run_id.as_str()],
            |row| {
                Ok(StoredRun {
                    run_id: row.get(0)?,
                    project_id: row.get(1)?,
                    name: row.get(2)?,
                    status: row.get(3)?,
                    created_at_millis: row.get(4)?,
                    started_at_millis: row.get(5)?,
                    finished_at_millis: row.get(6)?,
                })
            },
        );
        let stored = match result {
            Ok(stored) => stored,
            Err(duckdb::Error::QueryReturnedNoRows) => {
                return Err(StorageError::RunNotFound {
                    run_id: run_id.as_str().to_owned(),
                });
            }
            Err(source) => return Err(source.into()),
        };

        stored.into_run()
    }

    pub fn log_metric_at_step(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        step: Step,
        value_f64: f64,
    ) -> Result<MetricPoint, StorageError> {
        let timestamp = current_timestamp("timestamp")?;
        let ingested_at = current_timestamp("ingested_at")?;
        self.log_metric_at_step_with_timestamps(
            run_id,
            metric_key,
            step,
            value_f64,
            timestamp,
            ingested_at,
        )
    }

    pub fn log_metric_at_step_with_timestamps(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        step: Step,
        value_f64: f64,
        timestamp: DateTime<Utc>,
        ingested_at: DateTime<Utc>,
    ) -> Result<MetricPoint, StorageError> {
        self.connection.execute(
            "INSERT INTO dl.metric_points
                 (run_id, metric_key, metric_key_encoded, step, timestamp, value_f64, ingested_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                run_id.as_str(),
                metric_key.as_str(),
                percent_encode_metric_key(metric_key.as_str()),
                step.value(),
                timestamp_as_rfc3339(timestamp),
                value_f64,
                timestamp_as_rfc3339(ingested_at),
            ),
        )?;
        Ok(MetricPoint {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            step,
            timestamp,
            value_f64,
            ingested_at,
        })
    }

    pub fn rebuild_metric_aggregates_for_run(&self, run_id: &RunId) -> Result<(), StorageError> {
        self.connection.execute(
            "DELETE FROM seex_metric_aggregates
             WHERE run_id = ?",
            [run_id.as_str()],
        )?;
        self.connection.execute(
            "INSERT INTO seex_metric_aggregates
                 (run_id, metric_key, effective_count, last_step, last_value_f64,
                  min_value_f64, max_value_f64)
             SELECT run_id, metric_key, count(*), arg_max(step, step), arg_max(value_f64, step),
                    min(value_f64), max(value_f64)
             FROM (
                 SELECT *,
                        row_number() OVER (
                            PARTITION BY run_id, metric_key, step
                            ORDER BY ingested_at DESC, rowid DESC
                        ) AS write_rank
                 FROM dl.metric_points
                 WHERE run_id = ?
             )
             WHERE write_rank = 1
             GROUP BY run_id, metric_key",
            [run_id.as_str()],
        )?;
        Ok(())
    }

    fn run_exists(&self, run_id: &RunId) -> Result<bool, StorageError> {
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM seex_runs WHERE run_id = ?",
            [run_id.as_str()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_metric_key_preserves_unreserved_ascii() {
        assert_eq!(percent_encode_metric_key("AZaz09._~-"), "AZaz09._~-");
    }

    #[test]
    fn percent_encode_metric_key_encodes_path_and_utf8_bytes() {
        assert_eq!(
            percent_encode_metric_key("train/loss 零"),
            "train%2Floss%20%E9%9B%B6"
        );
    }
}
