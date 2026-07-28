use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use seex_app::data::worker::{ReadEvent, ReadEventReceiver};

#[derive(Debug)]
pub enum ReceiveEventError {
    Disconnected,
    Timeout,
}

impl Display for ReceiveEventError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => formatter.write_str("worker event stream disconnected"),
            Self::Timeout => formatter.write_str("worker event did not arrive before the deadline"),
        }
    }
}

impl Error for ReceiveEventError {}

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

pub fn receive_event(
    receiver: &ReadEventReceiver,
    timeout: Duration,
) -> Result<ReadEvent, ReceiveEventError> {
    let deadline = Instant::now() + timeout;
    let mut future = std::pin::pin!(receiver.recv());
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(Some(event)) => return Ok(event),
            Poll::Ready(None) => return Err(ReceiveEventError::Disconnected),
            Poll::Pending => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ReceiveEventError::Timeout);
        }
        std::thread::park_timeout(remaining);
    }
}
