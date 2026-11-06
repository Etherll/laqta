use std::time::Duration;

use crate::BoxFuture;

pub trait Clock: Send + Sync {
    /// Local time.
    fn now(&self) -> jiff::civil::DateTime;
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

pub trait Spawner: Send + Sync {
    fn spawn(&self, task: BoxFuture<'static, ()>);
    /// Blocking I/O, off the async worker.
    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()>;
    /// CPU-heavy work, inside the compute pool (`pool.install`). The pool is built on first use.
    fn run_compute(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()>;
}
