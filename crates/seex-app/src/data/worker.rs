use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Poll, Waker};
use std::thread::{self, JoinHandle};
#[cfg(all(test, feature = "desktop", target_os = "macos"))]
use std::time::{Duration, Instant};

use seex_storage::ReadInterrupt;

use crate::data::query::{
    CurveSnapshot, DetailRequest, InspectorRequest, InspectorSnapshot, OverviewRequest, QueryError,
};
use crate::data::source::{ReadSession, SourceError};
use crate::data::{CatalogSnapshot, DiscoveryRequest};
use crate::domain::DataSourceId;

/// Monotonically increasing identity for one read request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Generation(pub u64);

/// Independent result streams routed by source and panel coordinators.
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
}

#[cfg(all(test, feature = "desktop", target_os = "macos"))]
pub(crate) fn recv_event_for_test(
    receiver: &ReadEventReceiver,
    timeout: Duration,
) -> Option<ReadEvent> {
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct ThreadWake(std::thread::Thread);

    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let deadline = Instant::now() + timeout;
    let mut future = std::pin::pin!(receiver.recv());
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(event) = future.as_mut().poll(&mut cx) {
            return event;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        std::thread::park_timeout(remaining);
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
    token: RequestToken,
    request: ReadRequest,
    _ticket: RequestTicket,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RequestKey {
    source_id: DataSourceId,
    kind: ReadKind,
    metric_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RequestToken(u64);

struct RequestIdentity {
    key: RequestKey,
    generation: Generation,
    token: RequestToken,
}

impl RequestIdentity {
    fn new(tagged: &TaggedRequest) -> Self {
        let metric_key = match &tagged.request {
            ReadRequest::Overview(request) => {
                Some(request.selection.metric_key.as_str().to_owned())
            }
            ReadRequest::Detail(request) => Some(request.selection.metric_key.as_str().to_owned()),
            ReadRequest::Inspector(request) => Some(request.metric_key.as_str().to_owned()),
            ReadRequest::Discover(_) => None,
        };
        let key = RequestKey {
            source_id: tagged.source_id.clone(),
            kind: tagged.request.kind(),
            metric_key,
        };
        Self {
            key,
            generation: tagged.generation,
            token: tagged.token,
        }
    }
}

#[derive(Default)]
struct RequestRegistry {
    latest: HashMap<RequestKey, (Generation, RequestToken)>,
    active: HashMap<RequestKey, ActiveRequest>,
}

struct ActiveRequest {
    generation: Generation,
    token: RequestToken,
    interrupts: Vec<ReadInterrupt>,
}

impl RequestRegistry {
    fn mark_latest(&mut self, identity: &RequestIdentity) -> Option<Vec<ReadInterrupt>> {
        let value = (identity.generation, identity.token);
        if self
            .latest
            .get(&identity.key)
            .is_some_and(|latest| value <= *latest)
        {
            return None;
        }
        self.latest.insert(identity.key.clone(), value);
        self.active
            .get(&identity.key)
            .filter(|active| (active.generation, active.token) < value)
            .map(|active| active.interrupts.clone())
    }

    fn begin(&mut self, identity: &RequestIdentity, interrupts: Vec<ReadInterrupt>) -> bool {
        if !self.is_current(identity) {
            return false;
        }
        self.active.insert(
            identity.key.clone(),
            ActiveRequest {
                generation: identity.generation,
                token: identity.token,
                interrupts,
            },
        );
        true
    }

    fn finish(&mut self, identity: &RequestIdentity) {
        let value = (identity.generation, identity.token);
        if self
            .active
            .get(&identity.key)
            .is_some_and(|active| (active.generation, active.token) == value)
        {
            self.active.remove(&identity.key);
        }
    }

    fn is_current(&self, identity: &RequestIdentity) -> bool {
        self.latest.get(&identity.key) == Some(&(identity.generation, identity.token))
    }
}

#[derive(Default)]
struct RequestTicket(Option<Arc<AtomicUsize>>);

impl RequestTicket {
    fn tracked(outstanding: Arc<AtomicUsize>) -> Self {
        outstanding.fetch_add(1, Ordering::Relaxed);
        Self(Some(outstanding))
    }
}

impl Drop for RequestTicket {
    fn drop(&mut self) {
        if let Some(outstanding) = &self.0 {
            outstanding.fetch_sub(1, Ordering::Release);
        }
    }
}

/// Handle for a bounded set of background readers with thread-owned sessions.
pub struct ReadWorker {
    requests: Option<Sender<TaggedRequest>>,
    events: Option<ReadEventReceiver>,
    threads: Vec<JoinHandle<()>>,
    outstanding: Arc<AtomicUsize>,
    registry: Arc<Mutex<RequestRegistry>>,
    next_token: AtomicU64,
    #[cfg(feature = "test-support")]
    gate: Arc<ReadConcurrencyGate>,
    #[cfg(feature = "test-support")]
    superseded_reads: Arc<AtomicUsize>,
}

const MAX_WORKERS_PER_SOURCE: usize = 4;

struct ReadSessionPool {
    root_path: PathBuf,
    base: Mutex<Option<ReadSession>>,
}

impl ReadSessionPool {
    fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            base: Mutex::new(None),
        }
    }

    fn open(&self) -> Result<ReadSession, SourceError> {
        let mut base = self.base.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(base) = base.as_ref() {
            return base.try_clone();
        }
        let opened = ReadSession::open_existing(&self.root_path)?;
        let worker = opened.try_clone()?;
        *base = Some(opened);
        Ok(worker)
    }
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
        let sessions = Arc::new(ReadSessionPool::new(root_path.to_path_buf()));
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = read_event_channel();
        let worker_count = gate.limit.min(MAX_WORKERS_PER_SOURCE);
        #[cfg(feature = "test-support")]
        let superseded_reads = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(RequestQueue {
            receiver: request_rx,
            pending: PendingRequests {
                #[cfg(feature = "test-support")]
                superseded_reads: Some(Arc::clone(&superseded_reads)),
                ..PendingRequests::default()
            },
        }));
        let events = Arc::new(event_tx);
        let outstanding = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(Mutex::new(RequestRegistry::default()));
        let mut threads = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let sessions = Arc::clone(&sessions);
            let requests = Arc::clone(&requests);
            let events = Arc::clone(&events);
            let gate = Arc::clone(&gate);
            let registry = Arc::clone(&registry);
            threads.push(
                thread::Builder::new()
                    .name(format!("seex-native-reader-{index}"))
                    .spawn(move || worker_loop(sessions, requests, events, gate, registry))?,
            );
        }
        Ok(Self {
            requests: Some(request_tx),
            events: Some(event_rx),
            threads,
            outstanding,
            registry,
            next_token: AtomicU64::new(1),
            #[cfg(feature = "test-support")]
            gate,
            #[cfg(feature = "test-support")]
            superseded_reads,
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
        let ticket = RequestTicket::tracked(Arc::clone(&self.outstanding));
        let token = RequestToken(self.next_token.fetch_add(1, Ordering::Relaxed));
        let tagged = TaggedRequest {
            source_id,
            generation,
            token,
            request,
            _ticket: ticket,
        };
        let interrupts = self
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .mark_latest(&RequestIdentity::new(&tagged));
        if let Some(interrupts) = interrupts {
            for interrupt in interrupts {
                interrupt.interrupt();
            }
        }
        self.requests
            .as_ref()
            .ok_or(WorkerClosed)?
            .send(tagged)
            .map_err(|_| WorkerClosed)
    }

    /// Transfers the event stream to an event-driven integration.
    pub fn take_event_receiver(&mut self) -> Option<ReadEventReceiver> {
        self.events.take()
    }

    #[cfg(feature = "test-support")]
    pub fn resource_snapshot(&self) -> crate::performance::ReadSchedulingSnapshot {
        let concurrent_reads = *self
            .gate
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        crate::performance::ReadSchedulingSnapshot {
            concurrent_reads: concurrent_reads as u64,
            peak_concurrent_reads: self.gate.peak.load(Ordering::Relaxed) as u64,
            superseded_reads: self.superseded_reads.load(Ordering::Relaxed) as u64,
            ..crate::performance::ReadSchedulingSnapshot::default()
        }
    }
}

pub(crate) struct ReadConcurrencyGate {
    active: Mutex<usize>,
    available: Condvar,
    limit: usize,
    #[cfg(feature = "test-support")]
    peak: AtomicUsize,
}

impl ReadConcurrencyGate {
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "read concurrency limit must be positive");
        Self {
            active: Mutex::new(0),
            available: Condvar::new(),
            limit,
            #[cfg(feature = "test-support")]
            peak: AtomicUsize::new(0),
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
        #[cfg(feature = "test-support")]
        self.peak.fetch_max(*active, Ordering::Relaxed);
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
        // Dropping Viewer state must not block the UI thread on any request
        // that was not superseded before the sender closed.
        if self.outstanding.load(Ordering::Acquire) == 0 {
            for thread in self.threads.drain(..) {
                let _ = thread.join();
            }
        } else {
            self.threads.clear();
        }
    }
}

#[derive(Default)]
struct PendingRequests {
    discover: Option<TaggedRequest>,
    overview: Vec<TaggedRequest>,
    detail: Vec<TaggedRequest>,
    inspector: Vec<TaggedRequest>,
    #[cfg(feature = "test-support")]
    superseded_reads: Option<Arc<AtomicUsize>>,
}

impl PendingRequests {
    fn push(&mut self, tagged: TaggedRequest) {
        let _superseded = match &tagged.request {
            ReadRequest::Discover(_) => {
                if self
                    .discover
                    .as_ref()
                    .is_none_or(|pending| tagged.generation >= pending.generation)
                {
                    let superseded = self.discover.is_some();
                    self.discover = Some(tagged);
                    superseded
                } else {
                    false
                }
            }
            ReadRequest::Overview(_) => push_curve_request(&mut self.overview, tagged),
            ReadRequest::Detail(_) => push_curve_request(&mut self.detail, tagged),
            ReadRequest::Inspector(_) => push_curve_request(&mut self.inspector, tagged),
        };
        #[cfg(feature = "test-support")]
        if _superseded && let Some(counter) = &self.superseded_reads {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn take_next(&mut self) -> Option<TaggedRequest> {
        self.discover
            .take()
            .or_else(|| (!self.overview.is_empty()).then(|| self.overview.remove(0)))
            .or_else(|| (!self.detail.is_empty()).then(|| self.detail.remove(0)))
            .or_else(|| (!self.inspector.is_empty()).then(|| self.inspector.remove(0)))
    }

    fn has_newer(&self, identity: &RequestIdentity) -> bool {
        self.discover
            .iter()
            .chain(&self.overview)
            .chain(&self.detail)
            .chain(&self.inspector)
            .any(|candidate| {
                candidate.source_id == identity.key.source_id
                    && candidate.generation > identity.generation
                    && candidate.request.kind() == identity.key.kind
                    && RequestIdentity::new(candidate).key.metric_key == identity.key.metric_key
            })
    }
}

fn push_curve_request(pending: &mut Vec<TaggedRequest>, tagged: TaggedRequest) -> bool {
    let metric_key = match &tagged.request {
        ReadRequest::Overview(request) => &request.selection.metric_key,
        ReadRequest::Detail(request) => &request.selection.metric_key,
        ReadRequest::Inspector(request) => &request.metric_key,
        ReadRequest::Discover(_) => return false,
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
            return true;
        }
        false
    } else {
        pending.push(tagged);
        false
    }
}

struct RequestQueue {
    receiver: Receiver<TaggedRequest>,
    pending: PendingRequests,
}

fn next_request(queue: &Mutex<RequestQueue>) -> Option<TaggedRequest> {
    let mut queue = queue.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        while let Ok(request) = queue.receiver.try_recv() {
            queue.pending.push(request);
        }
        if let Some(request) = queue.pending.take_next() {
            return Some(request);
        }
        let request = queue.receiver.recv().ok()?;
        queue.pending.push(request);
    }
}

fn request_is_superseded(queue: &Mutex<RequestQueue>, identity: &RequestIdentity) -> bool {
    let Ok(mut queue) = queue.try_lock() else {
        return false;
    };
    while let Ok(request) = queue.receiver.try_recv() {
        queue.pending.push(request);
    }
    queue.pending.has_newer(identity)
}

fn request_is_current(registry: &Mutex<RequestRegistry>, identity: &RequestIdentity) -> bool {
    registry
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_current(identity)
}

fn worker_loop(
    sessions: Arc<ReadSessionPool>,
    requests: Arc<Mutex<RequestQueue>>,
    events: Arc<ReadEventSender>,
    gate: Arc<ReadConcurrencyGate>,
    registry: Arc<Mutex<RequestRegistry>>,
) {
    let mut session = None;
    while let Some(request) = next_request(&requests) {
        let identity = RequestIdentity::new(&request);
        let _permit = gate.acquire();
        if request_is_superseded(&requests, &identity) || !request_is_current(&registry, &identity)
        {
            continue;
        }
        let Some(event) = execute(
            &sessions,
            &mut session,
            request,
            &identity,
            &registry,
            || {
                request_is_superseded(&requests, &identity)
                    || !request_is_current(&registry, &identity)
            },
        ) else {
            continue;
        };
        let registry = registry.lock().unwrap_or_else(|error| error.into_inner());
        if !registry.is_current(&identity) {
            continue;
        }
        if !events.send(event) {
            return;
        }
    }
}

fn execute(
    sessions: &ReadSessionPool,
    session: &mut Option<ReadSession>,
    tagged: TaggedRequest,
    identity: &RequestIdentity,
    registry: &Mutex<RequestRegistry>,
    mut is_superseded: impl FnMut() -> bool,
) -> Option<ReadEvent> {
    let TaggedRequest {
        source_id,
        generation,
        token: _,
        request,
        _ticket,
    } = tagged;
    let kind = request.kind();
    let mut registered = false;
    let result = (|| {
        if session.is_none() {
            *session = Some(sessions.open()?);
        }
        let session = session.as_ref().ok_or(WorkerError::SessionUnavailable)?;
        if !registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .begin(identity, session.interrupt_handles().into())
        {
            return Ok(None);
        }
        registered = true;
        session.reader().refresh_diagnostics(generation.0);
        Ok(Some(match request {
            ReadRequest::Discover(request) => ReadSnapshot::Catalog(session.discover(&request)?),
            ReadRequest::Overview(request) => {
                let Some(snapshot) = session.query_overview_until(&request, &mut is_superseded)?
                else {
                    return Ok(None);
                };
                ReadSnapshot::Overview(snapshot)
            }
            ReadRequest::Detail(request) => {
                let Some(snapshot) = session.query_detail_until(&request, &mut is_superseded)?
                else {
                    return Ok(None);
                };
                ReadSnapshot::Detail(snapshot)
            }
            ReadRequest::Inspector(request) => {
                ReadSnapshot::Inspector(session.query_inspector(&request)?)
            }
        }))
    })();
    let current = request_is_current(registry, identity);
    if registered {
        registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish(identity);
    }
    if !current {
        return None;
    }
    let result = match result {
        Ok(Some(result)) => Ok(result),
        Ok(None) => return None,
        Err(error) => Err(error),
    };
    Some(ReadEvent {
        source_id,
        generation,
        kind,
        result,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use seex_model::metric::MetricKey;

    use crate::data::query::{CurveAxis, CurveSelection};

    use super::{
        DataSourceId, DiscoveryRequest, Generation, OverviewRequest, PendingRequests,
        ReadConcurrencyGate, ReadKind, ReadRequest, ReadSessionPool, ReadWorker, RequestIdentity,
        RequestKey, RequestQueue, RequestRegistry, RequestTicket, RequestToken, TaggedRequest,
        execute, next_request, read_event_channel, request_is_superseded,
    };

    fn overview_request(metric: &str) -> ReadRequest {
        ReadRequest::Overview(OverviewRequest {
            selection: CurveSelection {
                source_id: DataSourceId::new("source").expect("test alias should be valid"),
                runs: Vec::new(),
                metric_key: MetricKey::from_string(metric),
                axis: CurveAxis::Step,
            },
            logical_width: 1_000,
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
            threads: vec![thread],
            outstanding: Arc::new(AtomicUsize::new(1)),
            registry: Arc::new(Mutex::new(RequestRegistry::default())),
            next_token: AtomicU64::new(1),
            #[cfg(feature = "test-support")]
            gate: Arc::new(ReadConcurrencyGate::new(1)),
            #[cfg(feature = "test-support")]
            superseded_reads: Arc::new(AtomicUsize::new(0)),
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
                source_id: DataSourceId::new("source").expect("test alias should be valid"),
                generation: Generation(generation),
                token: RequestToken(generation),
                request: ReadRequest::Discover(DiscoveryRequest::default()),
                _ticket: RequestTicket::default(),
            });
        }

        assert_eq!(
            pending.take_next().map(|request| request.generation),
            Some(Generation(2))
        );
    }

    #[test]
    fn pending_curve_requests_coalesce_per_metric_panel() {
        #[cfg(feature = "test-support")]
        let superseded_reads = Arc::new(AtomicUsize::new(0));
        let mut pending = PendingRequests {
            #[cfg(feature = "test-support")]
            superseded_reads: Some(Arc::clone(&superseded_reads)),
            ..PendingRequests::default()
        };
        for (generation, metric) in [(1, "loss"), (2, "accuracy"), (3, "loss")] {
            pending.push(TaggedRequest {
                source_id: DataSourceId::new("source").expect("test alias should be valid"),
                generation: Generation(generation),
                token: RequestToken(generation),
                request: overview_request(metric),
                _ticket: RequestTicket::default(),
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
        #[cfg(feature = "test-support")]
        assert_eq!(
            superseded_reads.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn request_queue_serves_distinct_metrics_to_parallel_consumers() {
        let (request_tx, request_rx) = mpsc::channel();
        for (generation, metric) in [(1, "loss"), (2, "accuracy")] {
            request_tx
                .send(TaggedRequest {
                    source_id: DataSourceId::new("source").expect("test alias should be valid"),
                    generation: Generation(generation),
                    token: RequestToken(generation),
                    request: overview_request(metric),
                    _ticket: RequestTicket::default(),
                })
                .expect("test request queue should remain open");
        }
        let queue = Arc::new(Mutex::new(RequestQueue {
            receiver: request_rx,
            pending: PendingRequests::default(),
        }));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let consumers = (0..2)
            .map(|_| {
                let queue = Arc::clone(&queue);
                let release = Arc::clone(&release);
                let acquired_tx = acquired_tx.clone();
                thread::spawn(move || {
                    let request = next_request(&queue).expect("queued metric should be available");
                    acquired_tx
                        .send(request.generation)
                        .expect("test receiver should remain open");
                    let (released, available) = &*release;
                    let released = released.lock().unwrap_or_else(|error| error.into_inner());
                    drop(
                        available
                            .wait_while(released, |released| !*released)
                            .unwrap_or_else(|error| error.into_inner()),
                    );
                })
            })
            .collect::<Vec<_>>();
        drop(acquired_tx);
        let mut acquired = (0..2)
            .map(|_| acquired_rx.recv_timeout(Duration::from_secs(1)))
            .collect::<Vec<_>>();
        let (released, available) = &*release;
        *released.lock().unwrap_or_else(|error| error.into_inner()) = true;
        available.notify_all();
        for consumer in consumers {
            consumer.join().expect("request consumer should not panic");
        }
        acquired.sort_unstable_by_key(|result| result.as_ref().copied().ok());
        assert_eq!(acquired, [Ok(Generation(1)), Ok(Generation(2))]);
    }

    #[test]
    fn queued_newer_generation_supersedes_before_execution() {
        let (request_tx, request_rx) = mpsc::channel();
        let current = TaggedRequest {
            source_id: DataSourceId::new("source").expect("test alias should be valid"),
            generation: Generation(1),
            token: RequestToken(1),
            request: overview_request("loss"),
            _ticket: RequestTicket::default(),
        };
        let identity = RequestIdentity::new(&current);
        request_tx
            .send(TaggedRequest {
                source_id: DataSourceId::new("source").expect("test alias should be valid"),
                generation: Generation(2),
                token: RequestToken(2),
                request: overview_request("loss"),
                _ticket: RequestTicket::default(),
            })
            .expect("test queue should remain open");
        let queue = Mutex::new(RequestQueue {
            receiver: request_rx,
            pending: PendingRequests::default(),
        });

        assert!(request_is_superseded(&queue, &identity));
        assert_eq!(
            next_request(&queue).map(|request| request.generation),
            Some(Generation(2))
        );
    }

    #[test]
    fn superseded_storage_error_emits_no_event() {
        let root = tempfile::tempdir().expect("test directory should initialize");
        let sessions = ReadSessionPool::new(root.path().to_owned());
        let request = TaggedRequest {
            source_id: DataSourceId::new("source").expect("test alias should be valid"),
            generation: Generation(1),
            token: RequestToken(1),
            request: ReadRequest::Discover(DiscoveryRequest::default()),
            _ticket: RequestTicket::default(),
        };
        let identity = RequestIdentity::new(&request);
        let newer = RequestIdentity {
            key: identity.key.clone(),
            generation: Generation(2),
            token: RequestToken(2),
        };
        let registry = Mutex::new(RequestRegistry::default());
        {
            let mut registry = registry.lock().expect("test registry should lock");
            let _ = registry.mark_latest(&identity);
            let _ = registry.mark_latest(&newer);
        }

        let event = execute(&sessions, &mut None, request, &identity, &registry, || true);

        assert!(event.is_none());
    }

    #[test]
    fn request_ticket_releases_outstanding_work() {
        let outstanding = Arc::new(AtomicUsize::new(0));
        let ticket = RequestTicket::tracked(Arc::clone(&outstanding));
        assert_eq!(outstanding.load(Ordering::Acquire), 1);

        drop(ticket);

        assert_eq!(outstanding.load(Ordering::Acquire), 0);
    }

    #[test]
    fn request_tokens_isolate_keys_and_old_completion() {
        let identity = |metric: &str, generation, token| RequestIdentity {
            key: RequestKey {
                source_id: DataSourceId::new("source").expect("test alias should be valid"),
                kind: ReadKind::Detail,
                metric_key: Some(metric.to_owned()),
            },
            generation: Generation(generation),
            token: RequestToken(token),
        };
        let old = identity("loss", 1, 1);
        let current = identity("loss", 2, 2);
        let unrelated = identity("accuracy", 1, 3);
        let mut registry = RequestRegistry::default();

        assert!(registry.mark_latest(&old).is_none());
        assert!(registry.begin(&old, Vec::new()));
        assert!(registry.mark_latest(&current).is_some());
        assert!(registry.mark_latest(&unrelated).is_none());
        assert!(!registry.is_current(&old));
        assert!(registry.is_current(&current));
        assert!(registry.is_current(&unrelated));
        assert!(registry.begin(&current, Vec::new()));
        registry.finish(&old);
        let active = registry
            .active
            .get(&current.key)
            .expect("new token must remain active");
        assert_eq!(
            (active.generation, active.token),
            (Generation(2), RequestToken(2))
        );
    }

    #[test]
    fn event_receiver_can_only_be_taken_once() {
        let (request_tx, _request_rx) = mpsc::channel();
        let (_event_tx, event_rx) = read_event_channel();
        let mut worker = ReadWorker {
            requests: Some(request_tx),
            events: Some(event_rx),
            threads: Vec::new(),
            outstanding: Arc::new(AtomicUsize::new(0)),
            registry: Arc::new(Mutex::new(RequestRegistry::default())),
            next_token: AtomicU64::new(1),
            #[cfg(feature = "test-support")]
            gate: Arc::new(ReadConcurrencyGate::new(1)),
            #[cfg(feature = "test-support")]
            superseded_reads: Arc::new(AtomicUsize::new(0)),
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
