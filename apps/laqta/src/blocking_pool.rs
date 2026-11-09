//! Blocking-I/O threads that exit after 10 s idle, so an idle process has no wakeups.
//!
//! The `blocking` crate 1.7.0 never retires its last idle thread, which
//! then wakes every 500 ms forever. Laqta's own blocking work runs here instead.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use laqta_ports::BoxFuture;

type Job = Box<dyn FnOnce() + Send>;

const IDLE_EXIT: Duration = Duration::from_secs(10);
const MAX_THREADS: usize = 64;

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    idle: usize,
    threads: usize,
}

struct Pool {
    state: Mutex<State>,
    cvar: Condvar,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool { state: Mutex::new(State::default()), cvar: Condvar::new() })
}

fn submit(job: Job) {
    let p = pool();
    let Ok(mut s) = p.state.lock() else { return };
    s.queue.push_back(job);
    if s.idle > 0 {
        p.cvar.notify_one();
        return;
    }
    if s.threads < MAX_THREADS {
        s.threads += 1;
        let spawned = std::thread::Builder::new().name("laqta-blocking".into()).spawn(worker);
        if spawned.is_err() {
            s.threads -= 1;
        }
    }
}

fn worker() {
    let p = pool();
    let Ok(mut s) = p.state.lock() else { return };
    loop {
        if let Some(job) = s.queue.pop_front() {
            drop(s);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            s = match p.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            continue;
        }
        s.idle += 1;
        let (guard, timeout) = match p.cvar.wait_timeout(s, IDLE_EXIT) {
            Ok(r) => r,
            Err(_) => return,
        };
        s = guard;
        s.idle -= 1;
        if timeout.timed_out() && s.queue.is_empty() {
            s.threads -= 1;
            return;
        }
    }
}

/// Runs `work` on a pool thread and resolves with its result. A panic in `work` resumes on await.
pub fn unblock<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> BoxFuture<'static, T> {
    let (tx, rx) = futures_channel::oneshot::channel();
    submit(Box::new(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        let _ = tx.send(r);
    }));
    Box::pin(async move {
        match rx.await {
            Ok(Ok(v)) => v,
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(_) => panic!("blocking pool dropped a job"),
        }
    })
}
