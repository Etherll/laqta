# ADR 020: Panic policy and executors

Status: accepted (2026-11-07)

**Context.** Laqta is a resident process. With `panic = "abort"`, a panic in a decoder handling a hostile image, an effect kernel or an upload host would end the tray and every queued task. CPU-heavy work on GPUI's background executor could also slow the UI.

**Decision.**
- Panics unwind, so the release profile does not set `panic = "abort"`.
- Task wraps each job, and the dispatcher wraps each tool run, in `catch_unwind`. A panic becomes a `Failed` event.
- A panic hook writes the panic message and backtrace to the log.
- Application async work runs on `async-executor` with one worker thread. Blocking I/O runs on a pool of `main`'s own (`apps/laqta/src/blocking_pool.rs`: threads exit after 10 s idle, at most 64), because the `blocking` crate 1.7.0 never retires its last thread. CPU-heavy work runs on a `rayon` compute pool that `main` builds on first use and releases with its timer; kernels reach it through `Spawner::run_compute` and never create a pool (ADR 024). GPUI's executors run UI work only.
- None of these executors depends on GPUI, so the UI could move to a separate process if the GPUI idle-memory risk (ARCHITECTURE.md §10) requires it.

**Rejected.**
- **`panic = "abort"`:** about 5–10% smaller binary, at the cost of losing the process on any bug.
- **Running each job in a worker subprocess:** process start-up cost and IPC for every task, which is premature.

**Consequences.** Code inside `catch_unwind` must not leave shared state half-updated. Task state is owned per job, and shared state (the queue) is only updated outside the guarded section.
