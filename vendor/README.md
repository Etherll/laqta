# Vendored crates

- `blocking` 1.7.0 (Apache-2.0 OR MIT), patched in `src/lib.rs` `main_loop`: an idle thread exits
  after its 500 ms timeout even when the pool is under its thread limit. Upstream keeps the last
  thread looping every 500 ms forever, which breaks Laqta's zero-idle-wakeup budget (risk R6).
  Drop this patch when upstream fixes it.
