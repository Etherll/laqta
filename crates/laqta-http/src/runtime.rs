//! The shared HTTP runtime (ADR 007).
//!
//! One current-thread Tokio runtime runs on a thread named `laqta-http`. The first job starts it,
//! and it stops once no job has been in flight for `RELEASE_AFTER`; the next job starts a new one.
//! Callers never touch Tokio: a job's result comes back over a `futures-channel` oneshot.

use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use reqwest::Client;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::HttpError;

/// How long the runtime stays up with no job in flight.
pub(crate) static RELEASE_AFTER: Mutex<Duration> = Mutex::new(Duration::from_secs(60));

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;
/// A job gets the runtime's client and returns the task that runs it on the runtime.
type Job = Box<dyn FnOnce(Client) -> Task + Send>;

/// The running runtime, if any. Starting, submitting to and releasing it all take this lock, so a
/// job is never queued on a runtime that has already decided to stop.
static HANDLE: Mutex<Option<Handle>> = Mutex::new(None);

struct Handle {
    jobs: UnboundedSender<Envelope>,
    shared: Arc<Shared>,
}

/// State that the caller side and the runtime thread both read.
struct Shared {
    in_flight: AtomicUsize,
    /// When the last job ended, or when the runtime started. The release timer counts from here.
    idle_since: Mutex<Instant>,
}

struct Envelope {
    job: Job,
    in_flight: InFlight,
}

/// Counts one job from submission until it is dropped. Dropping it also restarts the idle clock.
/// A job that never runs (the runtime stopped first) releases its count the same way.
struct InFlight(Arc<Shared>);

impl InFlight {
    fn begin(shared: &Arc<Shared>) -> Self {
        shared.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(Arc::clone(shared))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
        *locked(&self.0.idle_since) = Instant::now();
    }
}

/// Queues `job` on the runtime, starting the runtime thread if it is not running.
pub(crate) fn submit(job: Job) -> Result<(), HttpError> {
    let mut handle = locked(&HANDLE);
    if handle.as_ref().is_none_or(|live| live.jobs.is_closed()) {
        *handle = Some(start()?);
    }
    let Some(live) = handle.as_ref() else {
        return Err(stopped());
    };
    let envelope = Envelope {
        job,
        in_flight: InFlight::begin(&live.shared),
    };
    if live.jobs.send(envelope).is_ok() {
        return Ok(());
    }
    // The thread died without clearing its handle. Forget it, so the next call starts a new one.
    *handle = None;
    Err(stopped())
}

/// Runs `make` on the runtime and returns a receiver for its output. The task stops early when the
/// receiver is dropped, so a caller that gives up also aborts the work.
pub(crate) fn dispatch<O, F, Fut>(make: F) -> Result<oneshot::Receiver<O>, HttpError>
where
    F: FnOnce(Client) -> Fut + Send + 'static,
    Fut: Future<Output = O> + Send + 'static,
    O: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let job: Job = Box::new(move |client| -> Task { Box::pin(deliver(make(client), tx)) });
    submit(job)?;
    Ok(rx)
}

/// Waits for a job's result. A job that ended without one (its task panicked or the runtime
/// stopped) is a network failure, so the caller can retry.
pub(crate) async fn finish<T>(rx: oneshot::Receiver<Result<T, HttpError>>) -> Result<T, HttpError> {
    rx.await.unwrap_or_else(|_| Err(stopped()))
}

/// Whether the runtime thread is running.
pub(crate) fn alive() -> bool {
    locked(&HANDLE)
        .as_ref()
        .is_some_and(|live| !live.jobs.is_closed())
}

pub(crate) fn set_release_after(after: Duration) {
    *locked(&RELEASE_AFTER) = after;
}

/// Runs `future` to completion and sends its output, unless the receiver is dropped first.
async fn deliver<O: Send>(future: impl Future<Output = O> + Send, mut tx: oneshot::Sender<O>) {
    let mut future = pin!(future);
    let output = poll_fn(|cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        if tx.poll_canceled(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await;
    if let Some(output) = output {
        let _ = tx.send(output);
    }
}

fn start() -> Result<Handle, HttpError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| HttpError::Io(err.to_string()))?;
    let (jobs, queue) = tokio::sync::mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        in_flight: AtomicUsize::new(0),
        idle_since: Mutex::new(Instant::now()),
    });
    let thread_shared = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("laqta-http".to_owned())
        .spawn(move || runtime.block_on(serve(thread_shared, queue)))
        .map_err(|err| HttpError::Io(err.to_string()))?;
    tracing::debug!("laqta-http runtime started");
    Ok(Handle { jobs, shared })
}

/// The runtime thread's main future: run queued jobs, and stop once idle for long enough.
async fn serve(shared: Arc<Shared>, mut queue: UnboundedReceiver<Envelope>) {
    let client = match build_client() {
        Ok(client) => client,
        Err(err) => {
            tracing::error!(error = %err, "laqta-http cannot build its HTTP client");
            return;
        }
    };
    loop {
        match tokio::time::timeout(idle_wait(&shared), queue.recv()).await {
            Ok(Some(Envelope { job, in_flight })) => {
                let task = job(client.clone());
                drop(tokio::spawn(async move {
                    let _in_flight = in_flight;
                    task.await;
                }));
            }
            Ok(None) => break,
            Err(_elapsed) => {
                if release_if_idle(&shared) {
                    break;
                }
            }
        }
    }
    tracing::debug!("laqta-http runtime stopped");
}

/// How long to wait for the next job before checking the release deadline again.
fn idle_wait(shared: &Shared) -> Duration {
    let release = *locked(&RELEASE_AFTER);
    if shared.in_flight.load(Ordering::SeqCst) > 0 {
        return release;
    }
    release.saturating_sub(locked(&shared.idle_since).elapsed())
}

/// Stops the runtime if it has been idle for `RELEASE_AFTER`. Returns true when it should exit.
fn release_if_idle(shared: &Arc<Shared>) -> bool {
    let mut handle = locked(&HANDLE);
    let release = *locked(&RELEASE_AFTER);
    let idle = shared.in_flight.load(Ordering::SeqCst) == 0
        && locked(&shared.idle_since).elapsed() >= release;
    if idle
        && handle
            .as_ref()
            .is_some_and(|live| Arc::ptr_eq(&live.shared, shared))
    {
        *handle = None;
    }
    idle
}

fn build_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .user_agent(concat!("Laqta/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(30))
        .build()
}

fn stopped() -> HttpError {
    HttpError::Network("the HTTP task stopped before it finished".to_owned())
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
