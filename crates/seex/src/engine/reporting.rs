use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::storage::{MetricWrite, ProjectConnection};

use crate::engine::EngineError;
use crate::model::metric::{MetricKey, Step};
use crate::model::run::RunId;

const DEFAULT_METRIC_BUFFER_CAPACITY: usize = 65_536;
const METRIC_BATCH_MAX_REPORTS: usize = 8_192;
const METRIC_BATCH_MAX_AGE: Duration = Duration::from_millis(10);
const WRITER_MAX_RETRIES: usize = 5;
const WRITER_INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(50);
const WRITER_MAX_RETRY_BACKOFF: Duration = Duration::from_millis(1_000);
const WRITER_RUNNING: u8 = 0;
const WRITER_RETRYING: u8 = 1;
const WRITER_FAILED: u8 = 2;
const WRITER_CLOSED: u8 = 3;
const FLUSH_RUNNING: u8 = 1;
const FLUSH_SUCCEEDED: u8 = 2;
const FLUSH_FAILED: u8 = 3;
const FLUSH_TIMED_OUT: u8 = 4;

#[derive(Clone)]
pub struct MetricReporter {
    inner: Arc<MetricReporterInner>,
}

impl MetricReporter {
    pub fn open(connection: Arc<Mutex<ProjectConnection>>) -> Self {
        Self::open_with_capacity(connection, DEFAULT_METRIC_BUFFER_CAPACITY)
    }

    pub fn open_with_capacity(connection: Arc<Mutex<ProjectConnection>>, capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let diagnostics = Arc::new(MetricReporterDiagnosticsInner::default());
        let queued_points = Arc::new(AtomicUsize::new(0));
        let worker_diagnostics = Arc::clone(&diagnostics);
        let failure_diagnostics = Arc::clone(&diagnostics);
        let worker_queued_points = Arc::clone(&queued_points);
        let worker = std::thread::spawn(move || {
            let result = metric_worker(
                connection,
                receiver,
                worker_diagnostics,
                worker_queued_points,
            );
            if result.is_err() {
                failure_diagnostics.set_writer_failed();
            }
        });

        Self {
            inner: Arc::new(MetricReporterInner {
                sender: Mutex::new(Some(sender)),
                diagnostics,
                worker: Mutex::new(Some(worker)),
                next_enqueue_sequence: AtomicU64::new(1),
                queue_capacity: capacity,
                queued_points,
            }),
        }
    }

    pub fn report_metric(
        &self,
        run_id: RunId,
        metric_key: MetricKey,
        step: Step,
        value_f64: f64,
    ) -> Result<(), EngineError> {
        self.report_metrics(
            run_id,
            vec![MetricValue {
                metric_key,
                step,
                value_f64,
            }],
        )
    }

    pub fn report_metrics(
        &self,
        run_id: RunId,
        metrics: Vec<MetricValue>,
    ) -> Result<(), EngineError> {
        let point_count = metrics.len();
        if point_count == 0 || point_count > METRIC_BATCH_MAX_REPORTS {
            return Err(EngineError::InvalidMetricBatch { count: point_count });
        }
        let sender = self
            .inner
            .sender
            .lock()
            .map_err(|_| EngineError::ConnectionLockPoisoned)?;
        let Some(sender) = sender.as_ref() else {
            return Err(EngineError::ClientClosed);
        };
        if self.inner.diagnostics.is_writer_failed() {
            return Err(EngineError::MetricWriterFailed {
                message: self
                    .inner
                    .diagnostics
                    .last_write_error()
                    .unwrap_or_else(|| "metric writer failed".to_owned()),
            });
        }
        let enqueue_sequence = self.inner.next_enqueue_sequence.load(Ordering::Relaxed);
        let timestamp_millis = chrono::Utc::now().timestamp_millis();
        if !self.inner.reserve_queue_points(point_count) {
            self.inner.diagnostics.increment_queue_full();
            return Err(EngineError::MetricQueueFull);
        }
        let reports = metrics
            .into_iter()
            .map(|metric| MetricReport {
                run_id: run_id.clone(),
                metric_key: metric.metric_key,
                step: metric.step,
                value_f64: metric.value_f64,
                timestamp_millis,
                enqueue_sequence,
            })
            .collect();
        let batch = MetricBatch { reports };
        self.inner.diagnostics.increment_pending_by(point_count);
        match sender.try_send(batch) {
            Ok(()) => {
                self.inner
                    .next_enqueue_sequence
                    .fetch_add(1, Ordering::Relaxed);
                self.inner.diagnostics.increment_accepted();
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.inner.release_queue_points(point_count);
                self.inner
                    .diagnostics
                    .decrement_pending_by_usize(point_count);
                self.inner.diagnostics.increment_queue_full();
                Err(EngineError::MetricQueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.inner.release_queue_points(point_count);
                self.inner
                    .diagnostics
                    .decrement_pending_by_usize(point_count);
                Err(EngineError::ClientClosed)
            }
        }
    }

    pub fn diagnostics(&self) -> MetricReporterDiagnostics {
        self.inner.diagnostics.snapshot()
    }

    pub(crate) fn set_flush_running(&self, run_id: &RunId) {
        self.inner.diagnostics.set_flush_running(run_id);
    }

    pub(crate) fn set_flush_succeeded(&self, run_id: &RunId) {
        self.inner.diagnostics.set_flush_succeeded(run_id);
    }

    pub(crate) fn set_flush_failed(&self, run_id: &RunId, message: String) {
        self.inner.diagnostics.set_flush_failed(run_id, message);
    }

    pub(crate) fn set_flush_timed_out(&self, run_id: &RunId) {
        self.inner.diagnostics.set_flush_timed_out(run_id);
    }

    pub fn drain_for(&self, timeout: Duration) -> bool {
        self.drain(Some(timeout)).is_ok()
    }

    pub fn drain(&self, timeout: Option<Duration>) -> Result<(), EngineError> {
        let barrier_sequence = self.enqueue_barrier_sequence()?;
        self.drain_through(barrier_sequence, timeout)
    }

    fn drain_through(
        &self,
        barrier_sequence: u64,
        timeout: Option<Duration>,
    ) -> Result<(), EngineError> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        while self.inner.diagnostics.persisted_through_sequence() < barrier_sequence {
            if self.inner.diagnostics.is_writer_failed() {
                return Err(EngineError::MetricWriterFailed {
                    message: self
                        .inner
                        .diagnostics
                        .last_write_error()
                        .unwrap_or_else(|| "metric writer failed".to_owned()),
                });
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(EngineError::MetricDrainTimeout);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.inner.diagnostics.is_writer_failed() {
            return Err(EngineError::MetricWriterFailed {
                message: self
                    .inner
                    .diagnostics
                    .last_write_error()
                    .unwrap_or_else(|| "metric writer failed".to_owned()),
            });
        }
        Ok(())
    }

    fn enqueue_barrier_sequence(&self) -> Result<u64, EngineError> {
        let _sender = self
            .inner
            .sender
            .lock()
            .map_err(|_| EngineError::ConnectionLockPoisoned)?;
        Ok(self
            .inner
            .next_enqueue_sequence
            .load(Ordering::Relaxed)
            .saturating_sub(1))
    }

    pub fn shutdown_for(&self, timeout: Duration) -> bool {
        self.shutdown(Some(timeout)).is_ok()
    }

    pub fn shutdown(&self, timeout: Option<Duration>) -> Result<(), EngineError> {
        let drain_result = self.drain(timeout);
        if matches!(drain_result, Err(EngineError::MetricDrainTimeout)) {
            return drain_result;
        }
        if let Some(sender) = take_mutex_value(&self.inner.sender) {
            drop(sender);
        }
        if let Some(worker) = take_mutex_value(&self.inner.worker) {
            let _ = worker.join();
        }
        if drain_result.is_ok() {
            self.inner.diagnostics.set_writer_closed();
        }
        drain_result
    }

    #[cfg(test)]
    pub(crate) fn blocked_for_test(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let queued_points = Arc::new(AtomicUsize::new(0));
        // Keep the receiver alive without draining it to simulate a blocked writer.
        std::mem::forget(receiver);
        Self {
            inner: Arc::new(MetricReporterInner {
                sender: Mutex::new(Some(sender)),
                diagnostics: Arc::new(MetricReporterDiagnosticsInner::default()),
                worker: Mutex::new(None),
                next_enqueue_sequence: AtomicU64::new(1),
                queue_capacity: capacity,
                queued_points,
            }),
        }
    }
}

struct MetricReporterInner {
    sender: Mutex<Option<SyncSender<MetricBatch>>>,
    diagnostics: Arc<MetricReporterDiagnosticsInner>,
    worker: Mutex<Option<JoinHandle<()>>>,
    next_enqueue_sequence: AtomicU64,
    queue_capacity: usize,
    queued_points: Arc<AtomicUsize>,
}

impl MetricReporterInner {
    fn reserve_queue_points(&self, count: usize) -> bool {
        self.queued_points
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(count)
                    .filter(|next| *next <= self.queue_capacity)
            })
            .is_ok()
    }

    fn release_queue_points(&self, count: usize) {
        self.queued_points.fetch_sub(count, Ordering::AcqRel);
    }
}

impl Drop for MetricReporterInner {
    fn drop(&mut self) {
        if let Some(sender) = take_mutex_value(&self.sender) {
            drop(sender);
        }
        if let Some(worker) = take_mutex_value(&self.worker) {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetricReporterDiagnostics {
    pub pending_reports: u64,
    pub queue_full_errors: u64,
    pub persisted_reports: u64,
    pub writer_state: &'static str,
    pub last_write_error: Option<String>,
    pub last_flush_run_id: Option<String>,
    pub last_flush_status: &'static str,
    pub last_flush_error: Option<String>,
}

#[derive(Default)]
struct MetricReporterDiagnosticsInner {
    pending_reports: AtomicU64,
    queue_full_errors: AtomicU64,
    persisted_reports: AtomicU64,
    writer_state: AtomicU64,
    last_write_error: Mutex<Option<String>>,
    last_flush_run_id: Mutex<Option<String>>,
    last_flush_status: AtomicU64,
    last_flush_error: Mutex<Option<String>>,
    persisted_through_sequence: AtomicU64,
}

impl MetricReporterDiagnosticsInner {
    fn increment_accepted(&self) {
        self.writer_state
            .store(WRITER_RUNNING.into(), Ordering::Relaxed);
    }

    fn increment_queue_full(&self) {
        self.queue_full_errors.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_persisted_by(&self, count: u64) {
        self.persisted_reports.fetch_add(count, Ordering::Relaxed);
    }

    fn set_persisted_through_sequence(&self, sequence: u64) {
        self.persisted_through_sequence
            .fetch_max(sequence, Ordering::Relaxed);
    }

    fn set_writer_running(&self) {
        self.writer_state
            .store(WRITER_RUNNING.into(), Ordering::Relaxed);
    }

    fn set_writer_retrying(&self) {
        self.writer_state
            .store(WRITER_RETRYING.into(), Ordering::Relaxed);
    }

    fn set_writer_failed(&self) {
        self.writer_state
            .store(WRITER_FAILED.into(), Ordering::Relaxed);
    }

    fn set_writer_closed(&self) {
        self.writer_state
            .store(WRITER_CLOSED.into(), Ordering::Relaxed);
    }

    fn set_last_write_error(&self, message: String) {
        if let Ok(mut last_write_error) = self.last_write_error.lock() {
            *last_write_error = Some(message);
        }
    }

    fn set_flush_running(&self, run_id: &RunId) {
        self.set_last_flush_run_id(run_id);
        self.last_flush_status
            .store(FLUSH_RUNNING.into(), Ordering::Relaxed);
    }

    fn set_flush_succeeded(&self, run_id: &RunId) {
        self.set_last_flush_run_id(run_id);
        self.last_flush_status
            .store(FLUSH_SUCCEEDED.into(), Ordering::Relaxed);
    }

    fn set_flush_failed(&self, run_id: &RunId, message: String) {
        self.set_last_flush_run_id(run_id);
        if let Ok(mut last_flush_error) = self.last_flush_error.lock() {
            *last_flush_error = Some(message);
        }
        self.last_flush_status
            .store(FLUSH_FAILED.into(), Ordering::Relaxed);
    }

    fn set_flush_timed_out(&self, run_id: &RunId) {
        self.set_last_flush_run_id(run_id);
        self.last_flush_status
            .store(FLUSH_TIMED_OUT.into(), Ordering::Relaxed);
    }

    fn set_last_flush_run_id(&self, run_id: &RunId) {
        if let Ok(mut last_flush_run_id) = self.last_flush_run_id.lock() {
            *last_flush_run_id = Some(run_id.as_str().to_owned());
        }
    }

    fn last_write_error(&self) -> Option<String> {
        self.last_write_error
            .lock()
            .ok()
            .and_then(|last_write_error| last_write_error.clone())
    }

    fn is_writer_failed(&self) -> bool {
        self.writer_state.load(Ordering::Relaxed) as u8 == WRITER_FAILED
    }

    fn increment_pending_by(&self, count: usize) {
        self.pending_reports
            .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    fn decrement_pending_by_usize(&self, count: usize) {
        self.decrement_pending_by(u64::try_from(count).unwrap_or(u64::MAX));
    }

    fn decrement_pending_by(&self, count: u64) {
        self.pending_reports.fetch_sub(count, Ordering::Relaxed);
    }

    fn pending_reports(&self) -> u64 {
        self.pending_reports.load(Ordering::Relaxed)
    }

    fn persisted_through_sequence(&self) -> u64 {
        self.persisted_through_sequence.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> MetricReporterDiagnostics {
        let pending_reports = self.pending_reports();
        let writer_state = match self.writer_state.load(Ordering::Relaxed) as u8 {
            WRITER_RETRYING => "retrying",
            WRITER_FAILED => "failed",
            WRITER_CLOSED => "closed",
            _ if pending_reports == 0 => "drained",
            _ => "running",
        };
        MetricReporterDiagnostics {
            pending_reports,
            queue_full_errors: self.queue_full_errors.load(Ordering::Relaxed),
            persisted_reports: self.persisted_reports.load(Ordering::Relaxed),
            writer_state,
            last_write_error: self
                .last_write_error
                .lock()
                .ok()
                .and_then(|last_write_error| last_write_error.clone()),
            last_flush_run_id: self
                .last_flush_run_id
                .lock()
                .ok()
                .and_then(|last_flush_run_id| last_flush_run_id.clone()),
            last_flush_status: match self.last_flush_status.load(Ordering::Relaxed) as u8 {
                FLUSH_RUNNING => "running",
                FLUSH_SUCCEEDED => "succeeded",
                FLUSH_FAILED => "failed",
                FLUSH_TIMED_OUT => "timed_out",
                _ => "none",
            },
            last_flush_error: self
                .last_flush_error
                .lock()
                .ok()
                .and_then(|last_flush_error| last_flush_error.clone()),
        }
    }
}

struct MetricReport {
    run_id: RunId,
    metric_key: MetricKey,
    step: Step,
    value_f64: f64,
    timestamp_millis: i64,
    enqueue_sequence: u64,
}

pub struct MetricValue {
    pub metric_key: MetricKey,
    pub step: Step,
    pub value_f64: f64,
}

struct MetricBatch {
    reports: Vec<MetricReport>,
}

fn take_mutex_value<T>(mutex: &Mutex<Option<T>>) -> Option<T> {
    let mut guard: MutexGuard<'_, Option<T>> = mutex.lock().ok()?;
    guard.take()
}

fn metric_worker(
    connection: Arc<Mutex<ProjectConnection>>,
    receiver: mpsc::Receiver<MetricBatch>,
    diagnostics: Arc<MetricReporterDiagnosticsInner>,
    queued_points: Arc<AtomicUsize>,
) -> Result<(), EngineError> {
    let mut next_ingested_at_millis = chrono::Utc::now().timestamp_millis();
    while let Ok(first_batch) = receiver.recv() {
        let mut batch = first_batch.reports;
        queued_points.fetch_sub(batch.len(), Ordering::AcqRel);
        collect_metric_batch(&receiver, &mut batch, &queued_points);
        let batch_len = u64::try_from(batch.len()).unwrap_or(u64::MAX);
        match write_metric_batch_with_retries(
            &connection,
            &batch,
            next_ingested_at_millis,
            &diagnostics,
            std::thread::sleep,
        ) {
            Ok(next_millis) => {
                next_ingested_at_millis = next_millis;
                diagnostics.increment_persisted_by(batch_len);
                if let Some(last_report) = batch.last() {
                    diagnostics.set_persisted_through_sequence(last_report.enqueue_sequence);
                }
                diagnostics.decrement_pending_by(batch_len);
            }
            Err(error) => {
                let message = sanitize_metric_write_error(&error);
                diagnostics.set_last_write_error(message.clone());
                diagnostics.set_writer_failed();
                return Err(EngineError::MetricWriterFailed { message });
            }
        }
    }
    Ok(())
}

fn collect_metric_batch(
    receiver: &mpsc::Receiver<MetricBatch>,
    batch: &mut Vec<MetricReport>,
    queued_points: &AtomicUsize,
) {
    let deadline = Instant::now() + METRIC_BATCH_MAX_AGE;
    while batch.len() < METRIC_BATCH_MAX_REPORTS {
        match receiver.try_recv() {
            Ok(next) => {
                queued_points.fetch_sub(next.reports.len(), Ordering::AcqRel);
                batch.extend(next.reports);
            }
            Err(mpsc::TryRecvError::Empty) => {
                let now = Instant::now();
                if now >= deadline {
                    return;
                }
                match receiver.recv_timeout(deadline - now) {
                    Ok(next) => {
                        queued_points.fetch_sub(next.reports.len(), Ordering::AcqRel);
                        batch.extend(next.reports);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                        return;
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
        }
    }
}

fn write_metric_batch_with_retries(
    connection: &Arc<Mutex<ProjectConnection>>,
    batch: &[MetricReport],
    next_ingested_at_millis: i64,
    diagnostics: &MetricReporterDiagnosticsInner,
    sleep: impl FnMut(Duration),
) -> Result<i64, EngineError> {
    retry_metric_batch_write(
        || write_metric_batch(connection, batch, next_ingested_at_millis),
        diagnostics,
        sleep,
    )
}

fn retry_metric_batch_write(
    mut write: impl FnMut() -> Result<i64, EngineError>,
    diagnostics: &MetricReporterDiagnosticsInner,
    mut sleep: impl FnMut(Duration),
) -> Result<i64, EngineError> {
    let mut retry_count = 0;
    let mut backoff = WRITER_INITIAL_RETRY_BACKOFF;
    loop {
        match write() {
            Ok(next_millis) => {
                diagnostics.set_writer_running();
                return Ok(next_millis);
            }
            Err(error) if retry_count < WRITER_MAX_RETRIES => {
                diagnostics.set_last_write_error(sanitize_metric_write_error(&error));
                diagnostics.set_writer_retrying();
                sleep(backoff);
                retry_count += 1;
                backoff = (backoff * 2).min(WRITER_MAX_RETRY_BACKOFF);
            }
            Err(error) => return Err(error),
        }
    }
}

fn write_metric_batch(
    connection: &Arc<Mutex<ProjectConnection>>,
    batch: &[MetricReport],
    next_ingested_at_millis: i64,
) -> Result<i64, EngineError> {
    let connection = connection
        .lock()
        .map_err(|_| EngineError::ConnectionLockPoisoned)?;
    let mut rows = Vec::with_capacity(batch.len());
    let mut ingested_at_millis = next_ingested_at_millis.max(chrono::Utc::now().timestamp_millis());
    for report in batch {
        rows.push(MetricWrite {
            run_id: report.run_id.as_str().to_owned(),
            metric_key: report.metric_key.as_str().to_owned(),
            step: report.step.value(),
            timestamp_millis: report.timestamp_millis,
            value_f64: report.value_f64,
            ingested_at_millis,
        });
        ingested_at_millis += 1;
    }
    connection.append_metric_batch(&rows)?;
    Ok(ingested_at_millis)
}

fn sanitize_metric_write_error(error: &EngineError) -> String {
    match error {
        EngineError::StorageFailure(crate::storage::StorageError::DuckDb(_)) => {
            "append metric batch failed".to_owned()
        }
        EngineError::Storage { operation, .. } => {
            format!("metric writer storage operation failed while {operation}")
        }
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bootstrap::open_native_connection;
    use crate::engine::write::NativeWriteStore;
    use crate::model::types::ProjectId;

    #[test]
    fn metric_report_struct_footprint_matches_capacity_planning() {
        assert_eq!(std::mem::size_of::<MetricReport>(), 80);
        assert_eq!(std::mem::align_of::<MetricReport>(), 8);
    }

    #[test]
    fn report_metric_returns_queue_full_when_buffer_is_full() {
        let reporter = MetricReporter::blocked_for_test(1);

        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(0),
                0.25,
            )
            .expect("first report should enter the queue");
        let result = reporter.report_metric(
            RunId::from_string("run-1"),
            MetricKey::from_string("train/loss"),
            Step::new(1),
            0.125,
        );

        assert!(matches!(result, Err(EngineError::MetricQueueFull)));
        let diagnostics = reporter.diagnostics();
        assert_eq!(diagnostics.queue_full_errors, 1);
        assert_eq!(diagnostics.pending_reports, 1);
        assert_eq!(diagnostics.writer_state, "running");
    }

    #[test]
    fn report_metrics_reserves_point_capacity_atomically() {
        let reporter = MetricReporter::blocked_for_test(3);
        let values = || {
            vec![
                MetricValue {
                    metric_key: MetricKey::from_string("loss"),
                    step: Step::new(0),
                    value_f64: 0.25,
                },
                MetricValue {
                    metric_key: MetricKey::from_string("accuracy"),
                    step: Step::new(0),
                    value_f64: 0.75,
                },
            ]
        };

        reporter
            .report_metrics(RunId::from_string("run-1"), values())
            .expect("two-point batch should fit");
        let rejected = reporter.report_metrics(RunId::from_string("run-1"), values());

        assert!(matches!(rejected, Err(EngineError::MetricQueueFull)));
        let diagnostics = reporter.diagnostics();
        assert_eq!(diagnostics.pending_reports, 2);
        assert_eq!(diagnostics.queue_full_errors, 1);

        assert!(matches!(
            reporter.report_metrics(RunId::from_string("run-1"), Vec::new()),
            Err(EngineError::InvalidMetricBatch { count: 0 })
        ));
        let oversized = (0..=METRIC_BATCH_MAX_REPORTS)
            .map(|index| MetricValue {
                metric_key: MetricKey::from_string(format!("metric-{index}")),
                step: Step::new(0),
                value_f64: index as f64,
            })
            .collect();
        assert!(matches!(
            reporter.report_metrics(RunId::from_string("run-1"), oversized),
            Err(EngineError::InvalidMetricBatch { count }) if count == METRIC_BATCH_MAX_REPORTS + 1
        ));
    }

    #[test]
    fn report_metrics_enqueues_one_timestamped_batch() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let reporter = MetricReporter {
            inner: Arc::new(MetricReporterInner {
                sender: Mutex::new(Some(sender)),
                diagnostics: Arc::new(MetricReporterDiagnosticsInner::default()),
                worker: Mutex::new(None),
                next_enqueue_sequence: AtomicU64::new(1),
                queue_capacity: 4,
                queued_points: Arc::new(AtomicUsize::new(0)),
            }),
        };
        reporter
            .report_metrics(
                RunId::from_string("run-1"),
                vec![
                    MetricValue {
                        metric_key: MetricKey::from_string("loss"),
                        step: Step::new(0),
                        value_f64: 0.25,
                    },
                    MetricValue {
                        metric_key: MetricKey::from_string("accuracy"),
                        step: Step::new(0),
                        value_f64: 0.75,
                    },
                ],
            )
            .expect("batch should enter the queue");

        let batch = receiver.try_recv().expect("one batch should be queued");
        assert_eq!(batch.reports.len(), 2);
        assert_eq!(
            batch.reports[0].timestamp_millis,
            batch.reports[1].timestamp_millis
        );
        assert_eq!(
            batch.reports[0].enqueue_sequence,
            batch.reports[1].enqueue_sequence
        );
    }

    #[test]
    fn report_metric_can_succeed_after_queue_full_error() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let queued_points = Arc::new(AtomicUsize::new(0));
        let reporter = MetricReporter {
            inner: Arc::new(MetricReporterInner {
                sender: Mutex::new(Some(sender)),
                diagnostics: Arc::new(MetricReporterDiagnosticsInner::default()),
                worker: Mutex::new(None),
                next_enqueue_sequence: AtomicU64::new(1),
                queue_capacity: 1,
                queued_points,
            }),
        };

        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(0),
                0.25,
            )
            .expect("first report should enter the queue");
        assert!(matches!(
            reporter.report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(1),
                0.125,
            ),
            Err(EngineError::MetricQueueFull)
        ));

        let first_batch = receiver
            .try_recv()
            .expect("queued report should be available");
        assert_eq!(first_batch.reports[0].enqueue_sequence, 1);
        reporter.inner.release_queue_points(1);
        reporter.inner.diagnostics.decrement_pending_by(1);
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(2),
                0.0625,
            )
            .expect("later report should enter the queue after capacity is freed");
        let later_batch = receiver
            .try_recv()
            .expect("later queued report should be available");

        let diagnostics = reporter.diagnostics();
        assert_eq!(later_batch.reports[0].enqueue_sequence, 2);
        assert_eq!(diagnostics.queue_full_errors, 1);
        assert_eq!(diagnostics.pending_reports, 1);
    }

    #[test]
    fn drain_for_returns_false_when_pending_report_is_not_written() {
        let reporter = MetricReporter::blocked_for_test(1);
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(0),
                0.25,
            )
            .expect("report should enter the queue");

        assert!(!reporter.drain_for(Duration::from_millis(1)));
        assert_eq!(reporter.diagnostics().pending_reports, 1);
    }

    #[test]
    fn drain_waits_for_current_enqueue_sequence_barrier_only() {
        let reporter = MetricReporter::blocked_for_test(4);
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(0),
                0.25,
            )
            .expect("first report should enter the queue");
        reporter.inner.diagnostics.set_persisted_through_sequence(1);
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(1),
                0.125,
            )
            .expect("later report should enter the queue");

        let drain = reporter.drain_through(1, None);

        assert!(
            drain.is_ok(),
            "expected first barrier to drain, got {drain:?}"
        );
        assert_eq!(reporter.diagnostics().pending_reports, 2);
    }

    #[test]
    fn shutdown_for_closes_report_sender() {
        let reporter = MetricReporter::blocked_for_test(1);

        assert!(reporter.shutdown_for(Duration::from_millis(1)));
        let result = reporter.report_metric(
            RunId::from_string("run-1"),
            MetricKey::from_string("train/loss"),
            Step::new(0),
            0.25,
        );

        assert!(matches!(result, Err(EngineError::ClientClosed)));
        let diagnostics = reporter.diagnostics();
        assert_eq!(diagnostics.pending_reports, 0);
        assert_eq!(diagnostics.writer_state, "closed");
    }

    #[test]
    fn shutdown_timeout_keeps_admission_open() {
        let reporter = MetricReporter::blocked_for_test(2);
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(0),
                0.25,
            )
            .expect("report should enter the queue");

        let shutdown = reporter.shutdown(Duration::from_millis(1).into());

        assert!(matches!(shutdown, Err(EngineError::MetricDrainTimeout)));
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(1),
                0.125,
            )
            .expect("shutdown timeout should leave admission open");

        let diagnostics = reporter.diagnostics();
        assert_eq!(diagnostics.pending_reports, 2);
        assert_eq!(diagnostics.writer_state, "running");
    }

    #[test]
    fn bounded_shutdown_can_timeout_while_another_thread_is_logging() {
        let reporter = MetricReporter::blocked_for_test(128);
        let logging_reporter = reporter.clone();
        let (started_sender, started_receiver) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let logging_thread = std::thread::spawn(move || {
            let mut sent_started = false;
            let mut step = 0;
            while stop_receiver.try_recv().is_err() {
                let result = logging_reporter.report_metric(
                    RunId::from_string("run-1"),
                    MetricKey::from_string("train/loss"),
                    Step::new(step),
                    step as f64,
                );
                if result.is_ok() && !sent_started {
                    started_sender
                        .send(())
                        .expect("test should observe first admitted report");
                    sent_started = true;
                }
                step += 1;
            }
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("logging thread should admit at least one report");

        let shutdown = reporter.shutdown(Some(Duration::from_millis(1)));
        stop_sender
            .send(())
            .expect("test should stop logging thread");
        logging_thread.join().expect("logging thread should join");

        assert!(matches!(shutdown, Err(EngineError::MetricDrainTimeout)));
        assert_ne!(reporter.diagnostics().writer_state, "closed");
    }

    #[test]
    fn shutdown_after_writer_failure_releases_sender_without_closing_diagnostics() {
        let reporter = MetricReporter::blocked_for_test(1);
        reporter
            .report_metric(
                RunId::from_string("run-1"),
                MetricKey::from_string("train/loss"),
                Step::new(0),
                0.25,
            )
            .expect("report should enter the queue");
        reporter
            .inner
            .diagnostics
            .set_last_write_error("append metric batch failed".to_owned());
        reporter.inner.diagnostics.set_writer_failed();

        let shutdown = reporter.shutdown(None);
        let repeated_shutdown = reporter.shutdown(None);
        let log_after_shutdown = reporter.report_metric(
            RunId::from_string("run-1"),
            MetricKey::from_string("train/loss"),
            Step::new(1),
            0.125,
        );

        assert!(matches!(
            shutdown,
            Err(EngineError::MetricWriterFailed { .. })
        ));
        assert!(matches!(
            repeated_shutdown,
            Err(EngineError::MetricWriterFailed { .. })
        ));
        assert!(matches!(log_after_shutdown, Err(EngineError::ClientClosed)));
        let diagnostics = reporter.diagnostics();
        assert_eq!(diagnostics.pending_reports, 1);
        assert_eq!(diagnostics.writer_state, "failed");
    }

    #[test]
    fn diagnostics_reports_drained_when_no_reports_are_pending() {
        let reporter = MetricReporter::blocked_for_test(1);

        let diagnostics = reporter.diagnostics();

        assert_eq!(diagnostics.pending_reports, 0);
        assert_eq!(diagnostics.writer_state, "drained");
        assert_eq!(diagnostics.last_write_error, None);
        assert_eq!(diagnostics.last_flush_status, "none");
        assert_eq!(diagnostics.last_flush_run_id, None);
        assert_eq!(diagnostics.last_flush_error, None);
    }

    #[test]
    fn report_metric_returns_writer_failed_after_failure_latches() {
        let reporter = MetricReporter::blocked_for_test(1);
        reporter
            .inner
            .diagnostics
            .set_last_write_error("append metric batch failed".to_owned());
        reporter.inner.diagnostics.set_writer_failed();

        let result = reporter.report_metric(
            RunId::from_string("run-1"),
            MetricKey::from_string("train/loss"),
            Step::new(0),
            0.25,
        );

        assert!(
            matches!(result, Err(EngineError::MetricWriterFailed { .. })),
            "expected latched writer failure, got {result:?}",
        );
        assert_eq!(reporter.diagnostics().pending_reports, 0);
    }

    #[test]
    fn retry_metric_batch_write_retries_five_times_before_error() {
        let diagnostics = MetricReporterDiagnosticsInner::default();
        let mut attempts = 0;
        let mut sleeps = Vec::new();

        let result = retry_metric_batch_write(
            || {
                attempts += 1;
                Err(EngineError::InvalidTimestamp {
                    field: "ingested_at",
                    millis: -1,
                })
            },
            &diagnostics,
            |duration| sleeps.push(duration),
        );

        assert!(matches!(result, Err(EngineError::InvalidTimestamp { .. })));
        assert_eq!(attempts, WRITER_MAX_RETRIES + 1);
        assert_eq!(
            sleeps,
            vec![
                Duration::from_millis(50),
                Duration::from_millis(100),
                Duration::from_millis(200),
                Duration::from_millis(400),
                Duration::from_millis(800),
            ],
        );
        assert_eq!(diagnostics.snapshot().writer_state, "retrying");
        assert!(
            diagnostics
                .snapshot()
                .last_write_error
                .as_deref()
                .is_some_and(|message| message.contains("invalid stored timestamp"))
        );
    }

    #[test]
    fn retry_metric_batch_write_recovers_after_transient_errors() {
        let diagnostics = MetricReporterDiagnosticsInner::default();
        let mut attempts = 0;
        let mut sleeps = Vec::new();

        let next_millis = retry_metric_batch_write(
            || {
                attempts += 1;
                if attempts < 3 {
                    return Err(EngineError::InvalidTimestamp {
                        field: "ingested_at",
                        millis: -1,
                    });
                }
                Ok(42)
            },
            &diagnostics,
            |duration| sleeps.push(duration),
        )
        .expect("transient writer errors should recover before retry exhaustion");

        assert_eq!(next_millis, 42);
        assert_eq!(attempts, 3);
        assert_eq!(
            sleeps,
            vec![Duration::from_millis(50), Duration::from_millis(100)]
        );
        assert_eq!(diagnostics.snapshot().writer_state, "drained");
        assert!(
            diagnostics.snapshot().last_write_error.is_some(),
            "last write error should retain the most recent retry error"
        );
    }

    #[test]
    fn retry_metric_batch_write_sanitizes_duckdb_errors() {
        let diagnostics = MetricReporterDiagnosticsInner::default();

        let result = retry_metric_batch_write(
            || {
                Err(EngineError::StorageFailure(
                    crate::storage::StorageError::DuckDb(duckdb::Error::InvalidQuery),
                ))
            },
            &diagnostics,
            |_| {},
        );

        assert!(matches!(
            result,
            Err(EngineError::StorageFailure(
                crate::storage::StorageError::DuckDb(_)
            ))
        ));
        assert_eq!(
            diagnostics.snapshot().last_write_error.as_deref(),
            Some("append metric batch failed"),
        );
    }

    #[test]
    fn collect_metric_batch_stops_at_report_threshold() {
        let (sender, receiver) = mpsc::sync_channel(METRIC_BATCH_MAX_REPORTS + 1);
        for step in 0..=METRIC_BATCH_MAX_REPORTS {
            sender
                .try_send(MetricBatch {
                    reports: vec![metric_report(step)],
                })
                .expect("test channel should have capacity for queued reports");
        }

        let first = receiver.recv().expect("first report should be queued");
        let mut batch = first.reports;
        let queued_points = AtomicUsize::new(METRIC_BATCH_MAX_REPORTS);
        collect_metric_batch(&receiver, &mut batch, &queued_points);

        assert_eq!(batch.len(), METRIC_BATCH_MAX_REPORTS);
        assert!(
            receiver.try_recv().is_ok(),
            "one report should remain after the size-capped batch"
        );
    }

    #[test]
    fn write_metric_batch_preserves_observation_time_and_orders_ingested_at()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_path =
            std::env::temp_dir().join(format!("seex-reporter-{}", uuid::Uuid::new_v4()));
        let connection = Arc::new(Mutex::new(ProjectConnection::new(open_native_connection(
            &root_path,
        )?)));
        {
            let connection = connection.lock().expect("test connection lock");
            connection.execute(
                "INSERT INTO seex_projects (project_id, name, created_at)
                 VALUES ('project-1', 'local training', now())",
                [],
            )?;
            NativeWriteStore::new(&connection).create_run(
                &ProjectId::from_string("project-1"),
                "baseline",
                Some(RunId::from_string("run-1")),
            )?;
        }
        let batch = vec![metric_report(0), metric_report(1), metric_report(2)];
        let first_ingested_at_millis = 1_700_000_000_000;

        let next_ingested_at_millis =
            write_metric_batch(&connection, &batch, first_ingested_at_millis)?;

        let connection = connection.lock().expect("test connection lock");
        let stored: Vec<(i64, i64, i64)> = connection
            .prepare(
                "SELECT step, epoch_ms(timestamp), epoch_ms(ingested_at)
                 FROM dl.metric_points
                 WHERE run_id = 'run-1'
                 ORDER BY ingested_at",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            stored.iter().map(|(step, _, _)| *step).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            stored
                .iter()
                .map(|(_, timestamp, _)| *timestamp)
                .collect::<Vec<_>>(),
            vec![1_700_000_000_000, 1_700_000_000_001, 1_700_000_000_002]
        );
        assert_eq!(stored[1].2, stored[0].2 + 1);
        assert_eq!(stored[2].2, stored[1].2 + 1);
        assert_eq!(next_ingested_at_millis, stored[2].2 + 1);
        assert!(stored[0].2 >= first_ingested_at_millis);
        std::fs::remove_dir_all(root_path)?;
        Ok(())
    }

    #[test]
    fn write_metric_batch_does_not_refresh_aggregates_before_accounting()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_path =
            std::env::temp_dir().join(format!("seex-reporter-{}", uuid::Uuid::new_v4()));
        let connection = Arc::new(Mutex::new(ProjectConnection::new(open_native_connection(
            &root_path,
        )?)));
        {
            let connection = connection.lock().expect("test connection lock");
            connection.execute(
                "INSERT INTO seex_projects (project_id, name, created_at)
                 VALUES ('project-1', 'local training', now())",
                [],
            )?;
            NativeWriteStore::new(&connection).create_run(
                &ProjectId::from_string("project-1"),
                "baseline",
                Some(RunId::from_string("run-1")),
            )?;
        }

        write_metric_batch(&connection, &[metric_report(0)], 1_700_000_000_000)?;

        let connection = connection.lock().expect("test connection lock");
        let aggregate_count: u64 = connection.query_row(
            "SELECT count(*)
             FROM seex_metric_aggregates
             WHERE run_id = 'run-1'
               AND metric_key = 'train/loss'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(aggregate_count, 0);
        std::fs::remove_dir_all(root_path)?;
        Ok(())
    }

    fn metric_report(step: usize) -> MetricReport {
        MetricReport {
            run_id: RunId::from_string("run-1"),
            metric_key: MetricKey::from_string("train/loss"),
            step: Step::new(i64::try_from(step).expect("test step should fit i64")),
            value_f64: step as f64,
            timestamp_millis: 1_700_000_000_000
                + i64::try_from(step).expect("test step should fit i64"),
            enqueue_sequence: u64::try_from(step + 1).expect("test step should fit u64"),
        }
    }
}
