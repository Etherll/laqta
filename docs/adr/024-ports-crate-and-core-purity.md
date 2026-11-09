# ADR 024: Ports crate, core purity and its enforcement

Status: accepted (2026-11-07, after the Clean Architecture validation), amended 2026-11-07 after the trade-offs review (`egm-review/tradeoffs/`): file access in `laqta-app`, and the `cosmic-text` requirement; amended 2026-11-09 by ADR 029 (the `Fonts` handle goes to `laqta-ui`, not the Region session)

**Context.** The validation scored the plan 7/10. Five of seven checks passed. Two failed:

- **Outside vocabulary in inner types.** `ScreenStream` was described in FFmpeg and WASAPI terms.
  The `.sxie` package format and the CLI action names sat in `laqta-domain`.
- **Construction inside the core.** `laqta-imaging::pool()` created a thread pool. The C codecs
  were registered globally. `cosmic-text`'s `FontSystem::new()` reads the locale and scans system
  fonts, which was confirmed in the cosmic-text 0.19.0 source.

It also found these problems:
- About eight adapter crates depended on the concrete `laqta-app`.
- The CI guard was a deny-list that crates like `ort`, `pipewire` and `notify` would pass.
- There was no timer port.
- Tool rules were placed in the UI.
- The ScreenCast restore token had no allowed path to storage.
- Hash-checked downloads were planned in three places.

**Decision.**

1. **`laqta-ports`.** Every port trait and the plain types it carries (`Frame`, `TaskEvent`,
   `PortError`, `BoxFuture`, …) move to `laqta-ports`, which holds traits and data only.
   Adapters depend on `laqta-ports` and `laqta-domain`. Only `laqta-ui` and `main` depend on
   `laqta-app`.
2. **Neutral port vocabulary.** Port types use Laqta's terms. `ScreenStream` is
   `Frames | Target` for video and `Device | Pcm` for audio (ADR 016). `ScreenSource` always
   returns one frame per monitor, and an adapter that gets a composed image splits it.
   `Prompt` asks for text, offers a choice, shows text or confirms. CLI and IPC action names
   live in `laqta-ipc`, and `.sxie` lives in `laqta-import`.
3. **The core builds nothing that touches the OS.** `main` builds these things and passes them in:
   - the `rayon` compute pool, reached through `Spawner::run_compute`;
   - the `Codecs` value;
   - the font loader. `main` wraps a function that loads the system font database and locale in
     the Editor engine's lazy `Fonts` handle, and gives the handle to `laqta-ui`, which hosts
     the engine (ADR 029). `cosmic-text` uses `default-features = false` and
     `FontSystem::new_with_locale_and_db`.

   Public parallel kernels `debug_assert!` that they run inside a pool, so an accidental call
   outside `run_compute` fails in tests instead of creating rayon's global pool.

   `jiff` in `laqta-domain` uses `default-features = false`. Reading the local time zone happens
   in the `Clock` adapter.
4. **A timer.** `Clock` gains `sleep(Duration)`. Tests use a manual clock, so golden traces don't
   depend on real time.
5. **Tool rules in the application.** A pure `tools` module in `laqta-app` holds the windowed
   tools' rules. Tool windows only render that state. It is not in `laqta-domain`, which would
   make the most-depended-on crate change for tool-UI reasons.
6. **Platform state through `main`.** Values an adapter must keep between runs, such as the
   restore token, are passed in when the adapter is built and handed back through a callback that
   `main` saves.
7. **Verified fetch.** `laqta-http` provides one download function with a pinned SHA-256,
   progress and cancellation. FFmpeg, onnxruntime, models and updates all use it.
8. **Enforcement.**
   - The CI guard checks the workspace edge list (ARCHITECTURE.md §3).
   - Each pure-core crate and `laqta-ports` may depend directly only on the crates in its
     allow-list.
   - The pure-core crates, `laqta-ports` and `laqta-app` keep a transitive deny-list.
   - A framework-placement rule limits `gpui-pre`, `gpui-pre-platform` and `gpui-component` to
     `laqta-ui` and `apps/laqta`, and `tokio` and `reqwest` to `laqta-http`, for every crate.
   - The presenter import rule covers both `gpui` and `laqta_library`.
   - Each pure-core crate has a `clippy.toml` that lists banned items one by one: file, thread,
     process, network, environment and clock APIs, `rayon::ThreadPoolBuilder` and
     `cosmic_text::FontSystem::new`. Clippy can't ban whole modules.
   - `laqta-app` has a `clippy.toml` that bans the `std::fs` items, `Path::exists` and
     `Path::metadata`. Only `src/files.rs` allows them.
9. **New crates.**
   - `laqta-cloud` holds `ImageAnalyzer` and the remove.bg `BackgroundRemover`.
   - `laqta-update` checks for and applies updates.

**Rejected.**
- **Keeping ports in `laqta-app`:** every adapter would depend on a concrete, frequently changing
  crate, which breaks the Stable Abstractions Principle.
- **A deny-list only:** it passes any I/O crate nobody listed, and it cannot see `std::fs` or
  `std::thread`.
- **A global codec registry:** hidden mutable state in the core.
- **Splitting `laqta-ml` into runtime and families crates now:** internal modules with their own
  tests are enough until compile times say otherwise.

**`cosmic-text` requirement (amended).** The earlier text pinned `cosmic-text` to GPUI's version.
The trade-offs review replaced that:
- **Evidence.** In `gpui-pre` 0.3.11, `cosmic-text` comes in only through `gpui-pre-wgpu`, on
  Linux. GPUI's font system is private, so sharing a version never saved memory. Re-check this
  on each GPUI upgrade.
- **Requirement.** `cosmic-text = "0.19"`, a caret requirement. Cargo merges it with GPUI's
  `^0.19` into one copy, and `Cargo.lock` decides the exact version. An exact `=0.19.0` pin
  would fail to resolve as soon as GPUI needed 0.19.1.
- **Review rule.** Any `Cargo.lock` change that moves `cosmic-text` requires reviewing the
  Editor engine's snapshot tests, because text in exported images may shift.

**Re-check (2026-11-07).** All seven checks pass (9/10). The follow-up fixes are recorded here:
- the compute-pool `debug_assert`;
- fonts through `main`'s loader;
- the sealed `OsScreenRef` (ADR 016);
- `#[non_exhaustive]` on `TaskEvent`, `AppError` and `PortError`, so Task's output can grow without breaking adapters;
- clippy bans listed item by item, including clock reads;
- tool rules in `laqta-app`.

**Consequences.**
- The validation's two failed checks are fixed, so all seven checks pass on paper.
- `laqta-app` keeps `std::fs`, confined to its private `files` module. Every `files` function is
  async: a private helper runs the work through `Spawner::run_blocking` and returns the result
  over a `futures-channel` oneshot, so the single async worker never blocks on a slow disk. The
  `Spawner` signature is unchanged. The file system is substitutable locally, and tests use a
  temporary directory.
- The allow-lists need updating whenever a pure-core crate adds a dependency, which makes each
  new core dependency a reviewed change.
