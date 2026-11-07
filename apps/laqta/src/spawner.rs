//! The runtime handed to `laqta-app`: one async worker, blocking threads, and a lazily built
//! compute pool released 60 s after its last use (ARCHITECTURE §1.4, ADR 020).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use laqta_ports::{BoxFuture, Clock, Spawner};

const POOL_RELEASE: Duration = Duration::from_secs(60);

/// Local time from `jiff`, sleeps from `async-io` timers.
#[derive(Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> jiff::civil::DateTime {
        jiff::Zoned::now().datetime()
    }
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            async_io::Timer::after(duration).await;
        })
    }
}

#[derive(Default)]
struct Pool {
    pool: Mutex<Option<Arc<rayon::ThreadPool>>>,
    uses: AtomicU64,
    in_flight: AtomicUsize,
    timer_armed: Mutex<bool>,
}

pub struct AppSpawner {
    executor: Arc<async_executor::Executor<'static>>,
    compute: Arc<Pool>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AppSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSpawner").finish_non_exhaustive()
    }
}

impl AppSpawner {
    /// Starts the single async worker thread, `laqta-app`.
    pub fn start(clock: Arc<dyn Clock>) -> std::io::Result<Arc<AppSpawner>> {
        let executor = Arc::new(async_executor::Executor::new());
        let ex = executor.clone();
        std::thread::Builder::new().name("laqta-app".into()).spawn(move || {
            async_io::block_on(ex.run(futures_lite::future::pending::<()>()));
        })?;
        Ok(Arc::new(AppSpawner { executor, compute: Arc::new(Pool::default()), clock }))
    }

    fn pool(&self) -> Result<Arc<rayon::ThreadPool>, String> {
        let mut guard = self.compute.pool.lock().map_err(|_| "compute pool lock poisoned".to_string())?;
        if let Some(p) = guard.as_ref() {
            return Ok(p.clone());
        }
        // Decision (M0): one thread per available hardware thread; std exposes no physical-core
        // count, and the pool only exists while work runs.
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("laqta-compute-{i}"))
            .build()
            .map_err(|e| e.to_string())?;
        let pool = Arc::new(pool);
        *guard = Some(pool.clone());
        tracing::debug!("compute pool started with {threads} threads");
        Ok(pool)
    }

    /// Arms the single release timer if none is armed. It drops the pool once 60 s pass with no
    /// new use and nothing in flight, then disarms, so an idle process has no timer.
    fn arm_release_timer(&self) {
        {
            let Ok(mut armed) = self.compute.timer_armed.lock() else { return };
            if *armed {
                return;
            }
            *armed = true;
        }
        let compute = self.compute.clone();
        let clock = self.clock.clone();
        self.executor
            .spawn(async move {
                loop {
                    let seen = compute.uses.load(Ordering::SeqCst);
                    clock.sleep(POOL_RELEASE).await;
                    if compute.uses.load(Ordering::SeqCst) == seen && compute.in_flight.load(Ordering::SeqCst) == 0 {
                        if let Ok(mut p) = compute.pool.lock() {
                            *p = None;
                        }
                        if let Ok(mut armed) = compute.timer_armed.lock() {
                            *armed = false;
                        }
                        tracing::debug!("compute pool released");
                        return;
                    }
                }
            })
            .detach();
    }
}

impl Spawner for AppSpawner {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        self.executor.spawn(task).detach();
    }

    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        crate::blocking_pool::unblock(work)
    }

    fn run_compute(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        let pool = self.pool();
        self.compute.uses.fetch_add(1, Ordering::SeqCst);
        self.compute.in_flight.fetch_add(1, Ordering::SeqCst);
        self.arm_release_timer();
        let compute = self.compute.clone();
        Box::pin(async move {
            match pool {
                Ok(pool) => crate::blocking_pool::unblock(move || pool.install(work)).await,
                Err(e) => tracing::error!("compute pool unavailable: {e}"),
            }
            compute.uses.fetch_add(1, Ordering::SeqCst);
            compute.in_flight.fetch_sub(1, Ordering::SeqCst);
        })
    }
}
