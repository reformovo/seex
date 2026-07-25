use std::collections::VecDeque;
use std::future::poll_fn;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::core::DataSourceId;
use crate::model::{CatalogSnapshot, DiscoveryRequest};
use crate::query::{
    CurveSnapshot, DetailRequest, InspectorRequest, InspectorSnapshot, OverviewRequest, QueryError,
};
use crate::source::{ReadSession, SourceError};

/// Monotonically increasing identity for one read request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Generation(pub u64);

/// Independent result streams maintained by the viewer Core.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReadKind {
    Catalog,
    Overview,
    Detail,
    Inspector,
}

/// Storage work accepted by the native read worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadRequest {
    Discover(DiscoveryRequest),
    Overview(OverviewRequest),
    Detail(DetailRequest),
    Inspector(InspectorRequest),
}

impl ReadRequest {
    pub const fn kind(&self) -> ReadKind {
        match self {
            Self::Discover(_) => ReadKind::Catalog,
            Self::Overview(_) => ReadKind::Overview,
            Self::Detail(_) => ReadKind::Detail,
            Self::Inspector(_) => ReadKind::Inspector,
        }
    }
}

/// Immutable result payload returned by the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadSnapshot {
    Catalog(CatalogSnapshot),
    Overview(CurveSnapshot),
    Detail(CurveSnapshot),
    Inspector(InspectorSnapshot),
}

impl ReadSnapshot {
    pub const fn kind(&self) -> ReadKind {
        match self {
            Self::Catalog(_) => ReadKind::Catalog,
            Self::Overview(_) => ReadKind::Overview,
            Self::Detail(_) => ReadKind::Detail,
            Self::Inspector(_) => ReadKind::Inspector,
        }
    }
}

/// Worker execution failures.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error(transparent)]
    Query(Box<QueryError>),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("native read session was not initialized")]
    SessionUnavailable,
}

impl From<QueryError> for WorkerError {
    fn from(error: QueryError) -> Self {
        Self::Query(Box::new(error))
    }
}

/// One generation-tagged result or failure.
#[derive(Debug)]
pub struct ReadEvent {
    pub source_id: DataSourceId,
    pub generation: Generation,
    pub kind: ReadKind,
    pub result: Result<ReadSnapshot, WorkerError>,
}

/// Returned when the read worker no longer accepts requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("native read worker is closed")]
pub struct WorkerClosed;

#[derive(Default)]
struct ReadEventState {
    queue: VecDeque<ReadEvent>,
    waker: Option<Waker>,
    sender_closed: bool,
    receiver_alive: bool,
}

#[derive(Default)]
struct ReadEventChannel {
    state: Mutex<ReadEventState>,
    available: Condvar,
}

impl ReadEventChannel {
    fn lock(&self) -> MutexGuard<'_, ReadEventState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

struct ReadEventSender(Arc<ReadEventChannel>);

impl ReadEventSender {
    fn send(&self, event: ReadEvent) -> bool {
        let waker = {
            let mut state = self.0.lock();
            if !state.receiver_alive {
                return false;
            }
            state.queue.push_back(event);
            state.waker.take()
        };
        self.0.available.notify_one();
        if let Some(waker) = waker {
            waker.wake();
        }
        true
    }
}

impl Drop for ReadEventSender {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.0.lock();
            state.sender_closed = true;
            state.waker.take()
        };
        self.0.available.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

fn read_event_channel() -> (ReadEventSender, ReadEventReceiver) {
    let channel = Arc::new(ReadEventChannel::default());
    channel.lock().receiver_alive = true;
    (
        ReadEventSender(Arc::clone(&channel)),
        ReadEventReceiver(channel),
    )
}

/// Movable event stream for event-driven viewer integrations.
pub struct ReadEventReceiver(Arc<ReadEventChannel>);

impl ReadEventReceiver {
    pub async fn recv(&self) -> Option<ReadEvent> {
        poll_fn(|cx| {
            let mut state = self.0.lock();
            if let Some(event) = state.queue.pop_front() {
                Poll::Ready(Some(event))
            } else if state.sender_closed {
                Poll::Ready(None)
            } else {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    pub fn try_event(&self) -> Option<ReadEvent> {
        self.0.lock().queue.pop_front()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<ReadEvent, mpsc::RecvTimeoutError> {
        let state = self.0.lock();
        let (mut state, wait) = self
            .0
            .available
            .wait_timeout_while(state, timeout, |state| {
                state.queue.is_empty() && !state.sender_closed
            })
            .unwrap_or_else(|error| error.into_inner());
        if let Some(event) = state.queue.pop_front() {
            Ok(event)
        } else if state.sender_closed {
            Err(mpsc::RecvTimeoutError::Disconnected)
        } else if wait.timed_out() {
            Err(mpsc::RecvTimeoutError::Timeout)
        } else {
            Err(mpsc::RecvTimeoutError::Disconnected)
        }
    }
}

impl Drop for ReadEventReceiver {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.receiver_alive = false;
        state.waker = None;
    }
}

struct TaggedRequest {
    source_id: DataSourceId,
    generation: Generation,
    request: ReadRequest,
}

/// Handle for one background thread that exclusively owns its read session.
pub struct ReadWorker {
    requests: Option<Sender<TaggedRequest>>,
    events: Option<ReadEventReceiver>,
    thread: Option<JoinHandle<()>>,
}

impl ReadWorker {
    /// Starts a worker for one local native source.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the operating system cannot spawn the thread.
    pub fn spawn(root_path: &Path) -> Result<Self, std::io::Error> {
        Self::spawn_with_gate(root_path, Arc::new(ReadConcurrencyGate::new(1)))
    }

    pub(crate) fn spawn_with_gate(
        root_path: &Path,
        gate: Arc<ReadConcurrencyGate>,
    ) -> Result<Self, std::io::Error> {
        let root_path = root_path.to_path_buf();
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = read_event_channel();
        let thread = thread::Builder::new()
            .name("pulseon-native-reader".to_owned())
            .spawn(move || worker_loop(root_path, request_rx, event_tx, gate))?;
        Ok(Self {
            requests: Some(request_tx),
            events: Some(event_rx),
            thread: Some(thread),
        })
    }

    /// Queues storage work without waiting for its execution.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerClosed`] after the worker has stopped.
    pub fn submit(
        &self,
        source_id: DataSourceId,
        generation: Generation,
        request: ReadRequest,
    ) -> Result<(), WorkerClosed> {
        self.requests
            .as_ref()
            .ok_or(WorkerClosed)?
            .send(TaggedRequest {
                source_id,
                generation,
                request,
            })
            .map_err(|_| WorkerClosed)
    }

    pub fn try_event(&self) -> Option<ReadEvent> {
        self.events.as_ref().and_then(ReadEventReceiver::try_event)
    }

    /// Waits for an event from background coordination or test code.
    ///
    /// # Errors
    ///
    /// Returns a receive error if the timeout expires or the worker stops.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ReadEvent, mpsc::RecvTimeoutError> {
        self.events
            .as_ref()
            .ok_or(mpsc::RecvTimeoutError::Disconnected)?
            .recv_timeout(timeout)
    }

    /// Transfers the event stream to an event-driven integration.
    pub fn take_event_receiver(&mut self) -> Option<ReadEventReceiver> {
        self.events.take()
    }
}

pub(crate) struct ReadConcurrencyGate {
    active: Mutex<usize>,
    available: Condvar,
    limit: usize,
}

impl ReadConcurrencyGate {
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "read concurrency limit must be positive");
        Self {
            active: Mutex::new(0),
            available: Condvar::new(),
            limit,
        }
    }

    fn acquire(&self) -> ReadPermit<'_> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while *active == self.limit {
            active = self
                .available
                .wait(active)
                .unwrap_or_else(|error| error.into_inner());
        }
        *active += 1;
        ReadPermit { gate: self }
    }
}

struct ReadPermit<'a> {
    gate: &'a ReadConcurrencyGate,
}

impl Drop for ReadPermit<'_> {
    fn drop(&mut self) {
        let mut active = self
            .gate
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *active -= 1;
        self.gate.available.notify_one();
    }
}

impl Drop for ReadWorker {
    fn drop(&mut self) {
        self.requests.take();
        // A native query cannot currently be cancelled. Detach it so dropping
        // viewer state never blocks the UI thread while the query finishes.
        drop(self.thread.take());
    }
}

#[derive(Default)]
struct PendingRequests {
    discover: Option<TaggedRequest>,
    overview: Vec<TaggedRequest>,
    detail: Vec<TaggedRequest>,
    inspector: Vec<TaggedRequest>,
}

impl PendingRequests {
    fn push(&mut self, tagged: TaggedRequest) {
        match &tagged.request {
            ReadRequest::Discover(_) => {
                if self
                    .discover
                    .as_ref()
                    .is_none_or(|pending| tagged.generation >= pending.generation)
                {
                    self.discover = Some(tagged);
                }
            }
            ReadRequest::Overview(_) => push_curve_request(&mut self.overview, tagged),
            ReadRequest::Detail(_) => push_curve_request(&mut self.detail, tagged),
            ReadRequest::Inspector(_) => push_curve_request(&mut self.inspector, tagged),
        }
    }

    fn take_next(&mut self) -> Option<TaggedRequest> {
        self.discover
            .take()
            .or_else(|| (!self.overview.is_empty()).then(|| self.overview.remove(0)))
            .or_else(|| (!self.detail.is_empty()).then(|| self.detail.remove(0)))
            .or_else(|| (!self.inspector.is_empty()).then(|| self.inspector.remove(0)))
    }
}

fn push_curve_request(pending: &mut Vec<TaggedRequest>, tagged: TaggedRequest) {
    let metric_key = match &tagged.request {
        ReadRequest::Overview(request) => &request.selection.metric_key,
        ReadRequest::Detail(request) => &request.selection.metric_key,
        ReadRequest::Inspector(request) => &request.metric_key,
        ReadRequest::Discover(_) => return,
    };
    if let Some(index) = pending.iter().position(|candidate| {
        candidate.source_id == tagged.source_id
            && match &candidate.request {
                ReadRequest::Overview(request) => &request.selection.metric_key == metric_key,
                ReadRequest::Detail(request) => &request.selection.metric_key == metric_key,
                ReadRequest::Inspector(request) => &request.metric_key == metric_key,
                ReadRequest::Discover(_) => false,
            }
    }) {
        if tagged.generation >= pending[index].generation {
            pending[index] = tagged;
        }
    } else {
        pending.push(tagged);
    }
}

fn worker_loop(
    root_path: PathBuf,
    requests: Receiver<TaggedRequest>,
    events: ReadEventSender,
    gate: Arc<ReadConcurrencyGate>,
) {
    let mut session = None;
    while let Ok(first) = requests.recv() {
        let mut pending = PendingRequests::default();
        pending.push(first);
        loop {
            for request in requests.try_iter() {
                pending.push(request);
            }
            let Some(request) = pending.take_next() else {
                break;
            };
            let _permit = gate.acquire();
            let event = execute(&root_path, &mut session, request);
            if !events.send(event) {
                return;
            }
        }
    }
}

fn execute(
    root_path: &Path,
    session: &mut Option<ReadSession>,
    tagged: TaggedRequest,
) -> ReadEvent {
    let kind = tagged.request.kind();
    let result = (|| {
        if session.is_none() {
            *session = Some(ReadSession::open_existing(root_path)?);
        }
        let session = session.as_ref().ok_or(WorkerError::SessionUnavailable)?;
        Ok(match tagged.request {
            ReadRequest::Discover(request) => ReadSnapshot::Catalog(session.discover(&request)?),
            ReadRequest::Overview(request) => {
                ReadSnapshot::Overview(session.query_overview(&request)?)
            }
            ReadRequest::Detail(request) => ReadSnapshot::Detail(session.query_detail(&request)?),
            ReadRequest::Inspector(request) => {
                ReadSnapshot::Inspector(session.query_inspector(&request)?)
            }
        })
    })();
    ReadEvent {
        source_id: tagged.source_id,
        generation: tagged.generation,
        kind,
        result,
    }
}

#[cfg(test)]
mod tests {
    use pulseon_model::metric::MetricKey;

    use crate::query::{CurveAxis, CurveSelection};

    use super::*;

    fn overview_request(metric: &str) -> ReadRequest {
        ReadRequest::Overview(OverviewRequest {
            selection: CurveSelection {
                source_id: DataSourceId::from_string("source"),
                runs: Vec::new(),
                metric_key: MetricKey::from_string(metric),
                axis: CurveAxis::Step,
            },
            physical_width: 1_000,
        })
    }

    #[test]
    fn dropping_worker_does_not_wait_for_running_thread() {
        let (request_tx, _request_rx) = mpsc::channel();
        let (_event_tx, event_rx) = read_event_channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let _ = release_rx.recv();
            let _ = finished_tx.send(());
        });
        let worker = ReadWorker {
            requests: Some(request_tx),
            events: Some(event_rx),
            thread: Some(thread),
        };
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(worker);
            let _ = dropped_tx.send(());
        });

        let dropped = dropped_rx.recv_timeout(Duration::from_secs(1));
        release_tx.send(()).expect("test worker should still exist");
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("test worker should finish after release");
        dropper.join().expect("dropper should not panic");

        assert!(dropped.is_ok(), "worker drop waited for its running thread");
    }

    #[test]
    fn pending_requests_keep_the_latest_generation_per_kind() {
        let mut pending = PendingRequests::default();
        for generation in [1, 2] {
            pending.push(TaggedRequest {
                source_id: DataSourceId::from_string("source"),
                generation: Generation(generation),
                request: ReadRequest::Discover(DiscoveryRequest::default()),
            });
        }

        assert_eq!(
            pending.take_next().map(|request| request.generation),
            Some(Generation(2))
        );
    }

    #[test]
    fn pending_curve_requests_coalesce_per_metric_panel() {
        let mut pending = PendingRequests::default();
        for (generation, metric) in [(1, "loss"), (2, "accuracy"), (3, "loss")] {
            pending.push(TaggedRequest {
                source_id: DataSourceId::from_string("source"),
                generation: Generation(generation),
                request: overview_request(metric),
            });
        }

        assert_eq!(
            [pending.take_next(), pending.take_next()].map(|request| {
                request
                    .expect("both metric panels should retain pending work")
                    .generation
            }),
            [Generation(3), Generation(2)]
        );
    }

    #[test]
    fn event_receiver_can_only_be_taken_once() {
        let (request_tx, _request_rx) = mpsc::channel();
        let (_event_tx, event_rx) = read_event_channel();
        let mut worker = ReadWorker {
            requests: Some(request_tx),
            events: Some(event_rx),
            thread: None,
        };

        assert!(worker.take_event_receiver().is_some());
        assert!(worker.take_event_receiver().is_none());
    }

    #[test]
    fn concurrency_gate_blocks_reads_beyond_its_limit() {
        let gate = Arc::new(ReadConcurrencyGate::new(1));
        let first = gate.acquire();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let waiting_gate = Arc::clone(&gate);
        let waiting = thread::spawn(move || {
            let _permit = waiting_gate.acquire();
            acquired_tx
                .send(())
                .expect("test receiver should remain open");
        });

        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(first);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiting read should acquire the released permit");
        waiting.join().expect("waiting thread should not panic");
    }
}
