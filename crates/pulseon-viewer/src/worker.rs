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
use crate::query::{CurveSnapshot, DetailRequest, OverviewRequest, QueryError};
use crate::source::{ReadSession, SourceError};

/// Monotonically increasing identity for one read request.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Generation(pub u64);

/// Independent result streams maintained by the viewer Core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadKind {
    Catalog,
    Overview,
    Detail,
}

/// Storage work accepted by the native read worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadRequest {
    Discover(DiscoveryRequest),
    Overview(OverviewRequest),
    Detail(DetailRequest),
}

impl ReadRequest {
    pub const fn kind(&self) -> ReadKind {
        match self {
            Self::Discover(_) => ReadKind::Catalog,
            Self::Overview(_) => ReadKind::Overview,
            Self::Detail(_) => ReadKind::Detail,
        }
    }
}

/// Immutable result payload returned by the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadSnapshot {
    Catalog(CatalogSnapshot),
    Overview(CurveSnapshot),
    Detail(CurveSnapshot),
}

impl ReadSnapshot {
    pub const fn kind(&self) -> ReadKind {
        match self {
            Self::Catalog(_) => ReadKind::Catalog,
            Self::Overview(_) => ReadKind::Overview,
            Self::Detail(_) => ReadKind::Detail,
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
        let root_path = root_path.to_path_buf();
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = read_event_channel();
        let thread = thread::Builder::new()
            .name("pulseon-native-reader".to_owned())
            .spawn(move || worker_loop(root_path, request_rx, event_tx))?;
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
    overview: Option<TaggedRequest>,
    detail: Option<TaggedRequest>,
}

impl PendingRequests {
    fn push(&mut self, tagged: TaggedRequest) {
        let slot = match &tagged.request {
            ReadRequest::Discover(_) => &mut self.discover,
            ReadRequest::Overview(_) => &mut self.overview,
            ReadRequest::Detail(_) => &mut self.detail,
        };
        if slot
            .as_ref()
            .is_none_or(|pending| tagged.generation >= pending.generation)
        {
            *slot = Some(tagged);
        }
    }

    fn take_next(&mut self) -> Option<TaggedRequest> {
        self.discover
            .take()
            .or_else(|| self.overview.take())
            .or_else(|| self.detail.take())
    }
}

fn worker_loop(root_path: PathBuf, requests: Receiver<TaggedRequest>, events: ReadEventSender) {
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
    use super::*;

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
}
