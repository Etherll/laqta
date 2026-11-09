# Vendored crates

- `blocking` 1.7.0 (Apache-2.0 OR MIT), patched in `src/lib.rs` `main_loop`: an idle thread exits
  after its 500 ms timeout even when the pool is under its thread limit. Upstream keeps the last
  thread looping every 500 ms forever, which breaks Laqta's zero-idle-wakeup budget (risk R6).
  Drop this patch when upstream fixes it.
- `gpui-component` 0.9.0 (Apache-2.0), patched in `src/window_border.rs` and `src/root.rs`: the
  client-side window frame has 10px rounded corners on Linux (square on tiled edges), the root
  surface rounds itself to match, and `window_corner_radii` is exported so views round the
  backgrounds they paint into a corner. Upstream keeps the frame square because content masks are
  rectangular. `set_window_blank` makes a window draw nothing, frame and shadow included, so the
  main window leaves no trace in a capture while the compositor animates its minimize. Upgrade it together with `gpui-pre` (ADR 002) and re-apply the patch; search for
  "Laqta patch".
