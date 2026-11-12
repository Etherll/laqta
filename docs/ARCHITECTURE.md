# Laqta architecture

Laqta (from Arabic لقطة, "a snapshot") is a fast, modern screen capture and sharing tool for
Windows and Linux, built in Rust with a GPUI front end, and a replacement for ShareX. Domain terms
are defined in [GLOSSARY.md](GLOSSARY.md), the feature checklist is [FEATURES.md](FEATURES.md), decisions are in
[`adr/`](adr/), and the pinned library versions are in `Cargo.toml`.

Two rules shape the design:

1. **Dependencies point inward.** Rules about capturing, uploading, editing and recording never
   name GPUI, Win32, Wayland, FFmpeg, SQLite or HTTP.
2. **Few deep modules.** Each concept has one module with a small interface and most of the
   behaviour inside. A seam (a trait) exists only where the seam rule in §2.3 allows it.

---

## 1. Problem, goals and non-goals

### 1.1 Problem

The capture tools that combine workflows per hotkey, after-capture and after-upload actions,
custom uploaders, an annotation editor and a screen recorder run only on Windows. Linux users have
no tool that combines these features. Laqta brings that combination to Windows and Linux in one
native binary that costs almost nothing while it waits in the tray.

### 1.2 Goals and budgets

| Goal | Budget | How it is checked |
| - | - | - |
| Idle in tray | 0% CPU and zero wakeups while no lazy resource is held; RSS under 40 MB with no windows open | `cargo xtask measure-idle`. On Linux it reads RSS, CPU and wakeups; on Windows it only says to read the working set in Task Manager. Run by hand, with GPUI running and no windows. The CI job is not built yet (pending, owner) |
| Hotkey to overlay visible | under 120 ms on a 4K screen, on Windows, X11 and Wayland compositors with layer-shell | trace spans from hotkey to first overlay frame (the `laqta::perf` log target), read by hand. No bench exists (pending, owner) |
| Capture peak memory | region capture on two 4K monitors raises RSS by under 150 MB, released within 2 s of the task finishing | measured by hand. No bench exists (pending, owner) |
| Fullscreen capture → PNG on disk | under 250 ms for one 4K monitor on the reference machine | `cargo xtask bench-png`, an example in `laqta-imaging`. CI does not run it |
| Recording overhead | 1080p30 with a hardware encoder under 5% CPU on the reference machine | manual gate per release |
| Feature coverage | 99% of non-dropped rows in FEATURES.md marked `done` | FEATURES.md status column |

**Reference machine:** Intel Core i5-1235U laptop with Iris Xe graphics, 16 GB RAM, a 1080p
internal panel and one 4K external monitor.

### 1.3 Non-goals

- Support for hosts that have shut down (ADR 010).
- macOS until the roadmap in §8 reaches it (ADR 001).
- GTK, WebView or any embedded browser engine.
- Plugins or scripting. Custom uploaders and effect presets are data files, not code.
- Mobile platforms.

### 1.4 Resource rules

- **Windows are short-lived.** The main window, settings, history and editor are created on
  demand and dropped on close.
- **Tray, hotkeys and IPC do not need GPUI.** They run on their own threads (§2.5), so the tray
  keeps working with no window open, and the UI could move to a separate process if that becomes necessary.
- **Lazy resources.** These start on first use: the HTTP runtime, the compute pool, the
  onnxruntime library, the OCR and background-removal models, the FFmpeg process, and the font
  database. The SQLite connection is the exception: it opens at start, so a bad history file
  fails early, and the next call after a release opens it again. Models, onnxruntime and FFmpeg
  are downloaded on first use through `laqta-http`'s verified fetch. The first-use model and
  onnxruntime downloads report their progress as toasts.
- **Release timers.** Each owner of a lazy resource arms one one-shot release timer while it
  holds the resource (60 s after the last use). With nothing held, no timer exists and the
  process has no wakeups. Built so far:
  - the compute pool and the SQLite connection (`History::release_when_idle`) time themselves
    with the `Clock` port;
  - the HTTP runtime stops itself after 60 s with no job, on its own thread's timer, because
    `laqta-http` has no `Clock`;
  - the loaded models: `main`'s `ModelReleaser` (`apps/laqta/src/models.rs`) drops the ONNX
    Runtime sessions of OCR, background removal and inpainting, with their threads, after 60 s
    unused, then calls glibc's `malloc_trim` on Linux.

  Not built yet: nothing releases the font database or the onnxruntime library (its environment
  is process-wide and cannot be freed).
- **No CPU-side duplication in process.** A captured frame is an `Arc`-shared buffer. Copies
  happen where a boundary needs one: cropping a region out of the composed desktop
  (`laqta_imaging::crop` copies the pixels), uploading a texture to the GPU, or writing frames to
  the FFmpeg pipe.
- **Undo stores commands, not bitmaps** (ADR 009).

---

## 2. Modules

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ Outer: delivery and drivers                                                   │
│   laqta-ui (views + presenters) · laqta-platform-{windows,linux,macos} ·    │
│   laqta-upload · laqta-cloud · laqta-ml · laqta-media · laqta-library ·  │
│   laqta-import · laqta-ipc · laqta-update ·                              │
│   laqta-http (library)                                                      │
│ ┌──────────────────────────────────────────────────────────────────────────┐ │
│ │ Application: laqta-app                                                  │ │
│ │   Action dispatcher · Task · Region session · Recording session ·        │ │
│ │   Watch folders                                                          │ │
│ │ ┌──────────────────────────────────────────────────────────────────────┐ │ │
│ │ │ Ports: laqta-ports (traits and the plain types they carry)          │ │ │
│ │ │ ┌──────────────────────────────────────────────────────────────────┐ │ │ │
│ │ │ │ Pure core: no I/O, no threads of its own, no OS calls            │ │ │ │
│ │ │ │   laqta-domain · laqta-imaging · laqta-editor · laqta-photo  │ │ │ │
│ │ │ └──────────────────────────────────────────────────────────────────┘ │ │ │
│ │ └──────────────────────────────────────────────────────────────────────┘ │ │
│ └──────────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────┘
                     every arrow points inward
```

The **pure core** may use pure computation libraries (`tiny-skia`, `cosmic-text`, `png`, `rxing`)
but nothing that does I/O, starts threads, reads the environment or calls the OS. Anything of
that kind (a thread pool, a font database, a codec set, the current time) is built in `main` or
an outer crate and passed in. The CI guard enforces this with allow-lists (§3, ADR 024).

### 2.1 Pure core

**`laqta-domain`**: data and rules that need no machine.

| Module | Contents |
| - | - |
| `image` | `Bitmap` (straight-alpha RGBA or BGRA pixels shared through `Arc`) and `Frame` (one monitor's captured image) |
| `geometry` | Physical-pixel rects (`PhysRect`, with their union) and monitors (`MonitorId`, `Monitor`). DPI conversion happens in platform code |
| `capture` | Capture modes and options (delay, cursor, transparency, HDR) |
| `region` | The region-selection state machine: hover target, drag, resize handles, multi-region, keyboard nudge, the selected state behind the capture toolbar (tools shown or hidden, the selection as a frame over its marks, `Finish(SelectionAction)`; ADR 029, ADR 032), magnifier sample rect, info text. `RegionInput` also carries what the overlay asks of the app and the state ignores: the annotated crop, and `TextCommand` for text read in place (ADR 031). Used only by the Region session |
| `workflow` | Workflows, task settings and their sections, the after-capture and after-upload flags (listed in FEATURES.md), destination choices, and the merge of a workflow's overrides onto defaults (ADR 008) |
| `action` | The `Action` enum: one variant per hotkey action, plus a few Laqta-specific ones (`RectangleLight`, `GifMaker`, `OpenSettings`, `RemoveBackground`). Its CLI and IPC names live in `laqta-ipc`, not here |
| `naming` | Name patterns and upload-info templates (token lists in FEATURES.md) |
| `effects` | Effects as data and presets. The `.sxie` package format is read and written by `laqta-import` |
| `history` | History items, tags, query filters |
| `settings` | The `Settings` value the application receives from `main` |
| `hotkey` | Platform-neutral key chords and conflict detection. The binding table is built by `Settings::hotkey_bindings` |
| `color` | Colours as data (`Rgba`) |
| `hosts` | Native upload host accounts as data (kinds and their fields). `main` turns them into `laqta-upload` hosts |
| `record` | Recording as data: codecs, quality, audio devices, camera overlay and the `RecordSpec` |

**`laqta-imaging`**: pure pixel work. Effect kernels, resize, combine, split, scrolling-capture
stitching, the beautifier, QR encode and decode, and the image codecs (PNG, JPEG, GIF, BMP, TIFF
and lossless WebP). Codecs are pure computation, so they are a library here, not a port. Hashing
is not here: it lives in `laqta-app`'s `tools`.
- **Codec set.** `laqta-imaging` defines a codec-extension trait and a `Codecs` value listing
  the codecs available. `main` builds that value and passes it to whoever encodes or decodes.
  There is no global registry. The extension trait has no production implementation: the optional
  C codecs (lossy WebP encode, AVIF decode, HEIC) that a `laqta-codecs-c` crate was to add were
  never built.
- **Parallel work.** Kernels use `rayon` parallel iterators but never create a pool. They run
  inside the compute pool that `main` builds and hands to callers through
  `Spawner::run_compute`, which calls `pool.install`. Each public parallel function starts with
  `debug_assert!(rayon::current_thread_index().is_some())`, so a call outside the pool (which
  would quietly create rayon's global pool) fails in tests.

**`laqta-editor`**: the **Editor engine**, the single module behind every annotation surface:
the image editor window and annotating on the region overlay (ADR 029). The beautifier is not
here; it is `laqta-imaging`'s `beautify`, run by Task.

- Interface: input events and commands in; dirty rects, rendered pixels and a chrome
  description out, plus requests for the host (save, continue, erase an object). Export is the
  same renderer at full resolution.
- Inside: the document (base image plus annotations), the 22 tools (the 21 listed in
  FEATURES.md and the object eraser, which asks the host to erase a box) and the canvas
  operations as state machines that turn pointer input into commands, undo and redo (including
  replacing the whole image, which the AI tools use), the per-annotation effect cache, and
  rendering on `tiny-skia` with text through `cosmic-text` and `swash` (ADR 003).
- Fonts are injected. `main` builds a font loader (a function that loads the system font
  database and locale) and wraps it in the engine's lazy `Fonts` handle. The handle goes to
  `laqta-ui`, the only host of the engine (the editor window and the region overlay), and the
  loader runs on first use. The Region session and `laqta-app` never see it. `cosmic-text` is
  built with `default-features = false`, and the engine creates its font system with
  `FontSystem::new_with_locale_and_db`, so it never scans system fonts itself.
- Adding a tool edits this crate, plus the tool's label, hint and shortcut in `laqta-ui`'s
  `editor_toolbar` presenter; a presenter test keeps those shortcuts in step with the engine's
  key map.

**`laqta-photo`**: the photo editor (layers, masks, blend modes, selections), a second pure-core
module beside the Editor engine.

### 2.2 Application (`laqta-app`)

An Action dispatcher and four deep modules (Task, Region session, Recording session, Watch
folders), with smaller modules beside them: auto capture and scrolling capture (both driven from
the dispatcher), the tool routes and `tools`, the AI use cases and history edits that open
windows call (`ai`, `history`), and image-file helpers (`images`). Nothing here depends on GPUI,
Tokio or OS crates. It depends on `laqta-ports` for every interface it calls, and on the pure core.

File access goes through one private `files` module (ADR 024). It uses `std::fs`, because the
file system is substitutable locally and tests use a temporary directory, so no port is needed.
Every `files` function is async: a private helper runs the work through `Spawner::run_blocking`
and returns the result over a oneshot channel. The single async worker therefore never blocks on
a slow disk. `laqta-app`'s `clippy.toml` bans the `std::fs` items, `Path::exists` and
`Path::metadata` everywhere else in the crate.

**Action dispatcher.** The single entry point for every delivery mechanism: hotkeys, the tray,
the main window, the CLI, single-instance IPC, browser native messaging and watch folders all
call `dispatch(action, workflow)`. A mechanism that already holds the action's input (a dropped
file, a URL, a watched folder's new file) calls `dispatch_with(action, ActionInput, workflow)`
instead (ADR 015). It merges settings and routes the action to one of:

- Task, the Region session, the Recording session or Watch folders;
- a **pure tool**: a tool that only decodes, runs `laqta-imaging` and encodes (hash, resize,
  convert, split, combine, thumbnail, strip metadata, QR);
- a **windowed tool**: a tool with its own window (colour pickers, ruler, pin to screen, OCR,
  video tools, window tools, viewers). These open through the `ToolHost` port. Their rules
  live in `laqta-app`'s `tools` module and in `laqta-imaging`. The module mixes plain values
  (colour formats, ruler measurements) with functions that take the `Ports` they use, so file
  access, pixel work and platform calls still go through `files`, the compute pool and the
  ports. Pin-window state lives in the GPUI view. The windows live in `laqta-ui`, render the
  tool's state, and reach ports and pixel work only through methods on the application handle
  (for example OCR, transcoding or sampling), so the work stays testable with fakes.

**Use cases for open windows.** Entry points go through `dispatch`. Work done inside an open
window calls methods on `App`, which take plain data and return a future.
- `ai.rs` has the four AI use cases: `recognize_text`, `remove_background`, `erase` (fill the
  pixels under a mask) and `analyze`. Each runs one port call (`Ocr`, `BackgroundRemover`,
  `Inpainter`, `ImageAnalyzer`), maps `PortError` to `AppError`, and emits nothing else, so the
  image editor, the photo editor and the library can all use them and the caller decides what to
  do with the answer (ADR 029).
- `history.rs` has `history_set_tags`, `history_delete` and `attach_history`. A window edits a
  history entry through the narrow `HistoryEdit` port, which runs on the blocking pool (ADR 018).

**Task.** Owns a task from input to result, because the steps share ordering and cleanup rules
(ADR 011).

- Interface: submit a task job (a capture result, or an upload of text, a file, a URL to
  download and reupload, a URL to shorten, or an already saved file; the dispatcher turns a
  folder into one job per file) with its settings; receive a stream of task events (status,
  progress, result, errors); stop one task or all.
- Inside, private: the step plan and order, encoding once (only when save, save-as, OCR, upload or
  analyze needs it), thumbnails from the saved path, the large-file warning, the before-upload
  prompt, bounded retries, after-upload steps (run only when a URL was expected and returned),
  branching by job kind, cleanup that spans both phases (`DELETE_FILE` removes the file whatever
  the outcome; ADR 011), the status machine, and the queue (two capture slots and one save slot,
  fixed in `task/mod.rs`).
- Not built yet: upload filters, and copying the URL before the task ends (with clearing it when
  the task fails).
- The implementation is split into private sub-modules (`encode`, `run`, `save`, `status`,
  `upload`). Their unit tests are internal; the golden traces test the interface.
- Each job runs inside `catch_unwind`. A panic becomes a `Failed` event instead of ending the
  process (ADR 020).
- Tests: golden scenarios, run against fake ports, asserting the event trace and side effects.

**Region session.** Owns selecting a region on screen, for capture and recording (ADR 012,
ADR 029).

- Interface: given frozen frames and options, return a selection (rects, the target window's
  title and process, the image, and the action the user chose) or `Cancelled`.
- Inside: shows the overlay through the `Overlay` port before hover targets load, loads hover
  targets in the background from `HoverSource`, and runs the `domain::region` state machine and
  hit tests. With `show_tools_after_select` on (the default), a drawn or clicked region stays
  selected and adjustable, and the overlay shows the capture toolbar beside it. The session crops
  the composed desktop to the selection whenever a change settles and passes the crop to the overlay
  (`RegionView::selection_image`). The overlay hosts the Editor engine on that crop and sends the
  annotated crop back (`RegionInput::Annotated`); the selection's image is then that crop.
  `Selection::action` is the toolbar button that ended the session.
- The session holds no Editor engine and no `Fonts`. Tools that act on an area at once
  (recording, scrolling capture, OCR, QR, colour picker, ruler, pin from screen) go through
  `region::pick`, which confirms on release with no toolbar.

**Recording session.** Owns a recording from start to file.

- Interface: start with a mode (region, window, custom, last region) and a record spec; pause,
  resume, stop or abort; receive recording events. The finished file goes to Task as an upload
  job of the saved file.
- A **record spec** holds the area, codec (the container follows from it), frame rate, quality
  (CRF or bitrate), cursor on or off, the audio devices to record, custom encoder arguments and
  an optional camera overlay.
- Inside: region selection through the Region session (for the region area only; the other
  areas use the active window, the custom region or the last area), the countdown, the
  fixed-duration stop, opening a `ScreenStream` on one rect (several selected rects are
  unioned), and driving the `Media` port. GIF encoding happens in the `Media` adapter, not in
  the session.

**Watch folders.** Watches configured folders through `FileWatcher` and, for each new file,
dispatches a `FileUpload` with that folder's workflow (`dispatch_with`).

### 2.3 Ports and the seam rule

All ports live in **`laqta-ports`**, with the plain types they carry (`TaskEvent`, `AppError`,
`PortError`, `BoxFuture`, …). Data shared with the pure core, such as `Bitmap` and `Frame`, stays
in `laqta-domain`. The crate holds traits and data, plus working fakes for tests behind its
`fakes` feature. Adapters depend on `laqta-ports` and `laqta-domain`, never on `laqta-app`, so a
change inside Task or a session doesn't touch any adapter (ADR 024).

Port types use Laqta's vocabulary, never an adapter's. FFmpeg arguments, Win32 handles,
Wayland objects and file formats stay in the adapter that maps them. A few couplings are accepted
and contained (ADR 016): a recording `Target` carries an `OsScreenRef` (a Windows output and
adapter, or an X11 display and screen), audio devices carry the OS's device name, and a record
spec carries the user's custom encoder arguments as opaque text. The application passes all of
these through unread. The platform adapter creates the first two; `laqta-media` reads all three,
and the Windows native recorder reads the first.

**Seam rule.** A port (trait) exists only if it has either:

- two production adapters (different platforms count), or
- one production adapter plus a fake that a deep module's tests need.

Async ports return boxed futures so they work behind `Arc<dyn Port>`. A port that cannot do
something on the current platform returns `Unsupported`, and callers handle that one error
instead of checking flags (ADR 018).

| Port | Production adapters | Fake needed by | Used by |
| - | - | - | - |
| `ScreenSource` (stills; always one frame per monitor, adapters split a pre-composed image) | Windows (DXGI duplication, GDI fallback), Wayland, X11 | Task, Region session tests | Task, Region session |
| `ScreenStream`: video is `Frames` (a frame source) or `Target` (a screen target the recorder may capture itself, carrying an `OsScreenRef`); audio inputs are `Device` (an audio device ID and name) or `Pcm` (a PCM source, such as Windows system audio). Its sub-traits are `FrameSource`, `PcmSource` and `RecordingHandle` | PipeWire frames (Wayland); screen targets (Windows, X11) | Recording session tests | Recording session |
| `HoverSource` | Win32 + UI Automation (Windows); X11; sway and KWin (Wayland). AT-SPI elements wrap every Linux backend | Region session tests | Region session |
| `Overlay` | GPUI | Region session tests | Region session |
| `EditorHost` (opens the annotation editor after capture) | GPUI | Task tests | Task |
| `Media` (record a stream, transcode a file) | FFmpeg (`laqta-media`), the native Windows recorder (Windows.Graphics.Capture, in `laqta-platform-windows`) | Recording session tests | Recording session, windowed tools |
| `Destination`, `Shortener`, `Sharer` | one per host in `laqta-upload` | Task tests (`Sharer` has none yet) | Task |
| `Uploaders` (look up hosts by id, download a URL) | `laqta-upload` | Task tests | Task, dispatcher |
| `Prompt` (ask for text, choose from a list, show text, confirm; also the file and folder pickers, the after-capture and before-upload windows, and the save dialog) | GPUI | Task tests | Task, `laqta-upload` |
| `ToolHost` (opens windowed tools, shows text and OCR results) | GPUI | dispatcher tests | dispatcher |
| `Clipboard` | Windows; Linux (one `arboard` adapter for Wayland and X11) | Task tests | Task, tools |
| `Notifier` | GPUI toasts (`UiHandle`, and `AppNotifier` in `main`) | Task tests | Task |
| `Shell` (open a file or URL, reveal, print, run a program) | Windows, Linux | Task tests | Task, `laqta-upload` (opens the browser for OAuth) |
| `HotkeyRegistrar` | `RegisterHotKey`, GlobalShortcuts portal, `XGrabKey` | none (a fake exists) | main |
| `Tray`, `Autostart` | Windows, Linux | none (`Tray` has a fake) | main |
| `SecretStore` | Credential Manager, Secret Service. `main` wraps either in `SessionSecrets`, which keeps what a keyring refuses in memory for the session (`MemorySecrets` under `--sandbox`) | upload tests | `laqta-upload` |
| `HistoryEdit` (replace tags, delete an entry) | `laqta-library`'s `History` | dispatcher tests | `App::history_set_tags`, `App::history_delete` |
| `Ocr` | `laqta-ml` (the PP-OCR families), Windows.Media.Ocr | Task tests | Task, tools, `App::recognize_text` |
| `BackgroundRemover` | `laqta-ml` (the matting family: BiRefNet, BEN2 and U²-Net-p entries), `laqta-cloud` (remove.bg) | tool tests | tools, `App::remove_background` |
| `ImageAnalyzer` | `laqta-cloud`: one OpenAI-compatible client; provider, base URL and model name are configuration (OpenAI, OpenRouter, Gemini, Ollama, LM Studio) | Task tests | Task, tools, `App::analyze` |
| `Inpainter` | `laqta-ml` (the LaMa family). One production adapter so far | none needed yet | photo editor, `App::erase` |
| `WindowOps` (borderless, top-most, window properties) | Windows, Linux | tool tests | tools |
| `InputSynth` (scroll a window) | Windows, Linux | scrolling capture tests | scrolling capture |
| `PointerHighlighter` | Windows, Linux | tool tests | tools |
| `ShellIntegration` (Explorer menu and Send To; `.desktop` actions) | Windows, Linux | none | main |
| `RemoteStorage` | `laqta-upload` (S3, FTP, SFTP) | remote browser tests | remote browser window |
| `FileWatcher` | `notify`, in `laqta-library` | Watch folders tests | Watch folders |
| `Clock` (`now`, `sleep`) | system time and `async-io` timers | every deep module's tests (a manual clock) | everything with time: retries, countdown, fixed-length recording, release timers |
| `Rng` | system | tests with fixed seeds | Task, `laqta-upload` |
| `Spawner` (`spawn`, `run_blocking`, `run_compute`) | `async-executor`, `main`'s blocking pool (`blocking_pool.rs`), the compute pool | every deep module's tests | everything |

`HotkeyRegistrar`, `Tray` and `Autostart` are used only by `main`. They are still ports, because
each has one adapter per platform and `main` must not contain platform code.

Local models are chosen per task from a catalog, or added by the user as custom models of a
supported model family (ADR 023). `laqta-ml` owns all of this behind `Ocr`, `BackgroundRemover`
and `Inpainter`.

There is no capabilities type. If the settings UI or onboarding needs to say that something is
unavailable (for example, "window detection is unavailable on this compositor"), that is
information only, and no rule branches on it.

**Settings and history are not ports.** The application receives settings as a value from `main`,
and a new value whenever the user changes something. New history entries are written by a
task-event subscriber in `laqta-library`. A window edits an existing entry (tags, delete) through
`App`, which runs the narrow `HistoryEdit` port on the blocking pool; the application never reads
history. `laqta-ui` reads history through `laqta-library`'s query functions, which return
`laqta-domain` types only. Presenters receive those values and may not import `laqta-library`.

**Platform state** that an adapter must keep between runs, such as the ScreenCast restore token,
is passed in by `main` when the adapter is built, and handed back through a callback that `main`
saves (the token goes to `<data>/screencast-token`). Platform crates never touch settings storage.

### 2.4 Outer modules

| Crate | Role |
| - | - |
| `laqta-upload` | The **Upload module**: every host behind `Destination`, `Shortener` and `Sharer` (ADR 014). It holds the custom-uploader (`.sxcu`) engine and the bundled catalog (ADR 004). It also holds the hosts written in Rust (see `src/hosts/`: OAuth2 and OAuth1, S3 SigV4 and GCS, Azure, B2, FTP, FTPS, SFTP, SMTP, shared folder and others; MEGA was dropped, ADR 010), the OAuth flow with PKCE and token refresh, and the remote-storage browser. `Prompt`, `Rng`, `Shell` and `SecretStore` are injected; OAuth takes the current time as a closure, not a `Clock`. Nine hosts sit behind a `host-*` cargo feature (Imgur, Dropbox, Google Drive, OneDrive, S3, FTP, SFTP, shared folder, Bitly); the others compile unconditionally |
| `laqta-http` | A library, not a port. It runs a Tokio runtime on one thread, started on the first request and stopped after 60 s with no job, with one shared `reqwest` client (rustls on Linux, SChannel through `native-tls` on Windows; ADR 025), the OAuth loopback listener and the SFTP client (ADR 007). It also holds **verified fetch**: download to a file with a pinned SHA-256, progress and cancellation, used for FFmpeg, onnxruntime, models and updates. Used by `laqta-upload`, `laqta-cloud`, `laqta-ml`, `laqta-media`, `laqta-update` and `apps/laqta` |
| `laqta-cloud` | Cloud ML adapters: `ImageAnalyzer` (one OpenAI-compatible client) and the remove.bg `BackgroundRemover`. API keys come from `SecretStore` |
| `laqta-update` | Update checks and installs: GitHub Releases through `laqta-http`'s verified fetch (the expected hash comes from the release's own `SHA256SUMS` asset), applied by Velopack on Windows; a notice only on Linux |
| `laqta-media` | The `Media` adapter: FFmpeg with hardware or software encoders, GIF through FFmpeg's `palettegen` / `paletteuse`, audio mux (piping Windows loopback PCM to FFmpeg as a second input), cursor overlay. It consumes a `ScreenStream` and never calls platform code. It alone maps Laqta's screen targets and audio inputs to FFmpeg inputs (`ddagrab`, `x11grab`, `dshow`, `pulse`, raw frames and PCM on pipes). It records into a container that survives a crash (fragmented MP4 or MKV), then remuxes on stop |
| `laqta-library` | Settings (versioned JSON, migrations, atomic writes, rotating and weekly backups, export and import archives, portable mode) and history (SQLite with WAL, the task-event subscriber that records saved files, the `HistoryEdit` adapter, and release of the connection after 60 s idle). It also holds the `notify` adapter for `FileWatcher` and the reader of ShareX's history files (`legacy.rs`). It has no thumbnail cache: `laqta-ui` keeps decoded thumbnails in memory |
| `laqta-import` | Reads ShareX config, history, `.sxcu`, `.sxie` (it owns the package format) and backup archives into Laqta values. Secrets encrypted with DPAPI decrypt only on Windows; elsewhere the user re-enters them |
| `laqta-ui` | GPUI views: main window, settings, history, editor window (paints Editor engine output), overlay (`Overlay`, with the capture toolbar and the Editor engine hosted on the selection, ADR 029, and the selection's text read in place, ADR 031), prompts (`Prompt`), tool windows (`ToolHost`, including the text window for analysis results and for OCR text the user wants to edit), recording bar, toasts, pin windows, a GPUI tray menu. Its `presenters` module holds GPUI-free logic (task-list rows, history paging, settings shown as differences from defaults, the tray menu model, the capture toolbar's layout and keys, the words, selection and motion of text read in place, the editor toolbar, and the editor and photo **Editor interaction**: input, render gating and snapshot renders, with compute injected as a function). The CI guard keeps that module free of `gpui` and `laqta_library` imports (ADR 017). It receives the `Fonts` handle from `main` |
| `laqta-ml` | The **model runtime** (ADR 023):<br>• the model catalog (`catalog/models.toml`) and the user's custom models;<br>• downloads through `laqta-http`'s verified fetch;<br>• `ort` sessions (loaded lazily, at most one per task) and the execution-provider setting (Auto, CPU, GPU; no GPU provider is built in yet);<br>• the model families, one file each (a parametric matting family for models that differ only in numbers, the two PP-OCR families together in `o1.rs`, and LaMa for inpainting);<br>• validation of custom models.<br>Internal modules (catalog, runtime and sessions, families, validation) each have their own tests. Implements `Ocr`, `BackgroundRemover` and `Inpainter` |
| `laqta-ipc` | Single-instance lock and forwarding of commands between instances, over a named pipe (Windows) or a Unix socket (Linux). It owns the text names of actions used by the CLI and the IPC protocol |
| `laqta-platform-windows` | Stills through DXGI duplication with a GDI fallback (HDR via DXGI 1.6). Transparent windows are captured twice, over white and over black pop-ups, and the alpha comes from the difference. Hover targets from `EnumWindows` + UI Automation. Hotkeys and the tray on a dedicated message-loop thread. Credential Manager, the Run-key autostart, shell verbs and Send To. The native recorder (a `Media` adapter on Windows.Graphics.Capture, with WASAPI loopback). It has no toast code: notifications are GPUI toasts |
| `laqta-platform-linux` | Capture, hover and hotkey backends with their Wayland fallbacks (ADR 019). Also: SNI tray with a DBusMenu menu (`ksni`), Secret Service, XDG autostart, AT-SPI. The overlay is not here: its layer-shell backend is in `laqta-ui`. There are no freedesktop notifications; toasts are GPUI's |
| `laqta-platform-macos` | Stills and recording frames through ScreenCaptureKit, which leaves Laqta's own app out, with system audio from the same stream. Hover targets from the window list and the Accessibility API. Carbon hotkeys, an `NSStatusItem` menu, the pasteboard, Keychain, `SMAppService` autostart, Vision OCR. Privacy permissions are read and asked for by the onboarding window at start (ADR 030). AppKit work runs on the main queue. Points become physical pixels in `displays` (ADR 030) |

### 2.5 Composition root (`apps/laqta`)

`apps/laqta` is the composition root. `main.rs` is short and hands off to `wiring::run`; the
concrete adapters are named in `wiring.rs`, `cloud.rs`, `media.rs`, `local_models.rs`,
`notifier.rs` and `stubs.rs`. It:

1. Parses CLI arguments. If another instance is running, it forwards them over `laqta-ipc` and exits.
2. Picks the platform module with `cfg(target_os)` and, on Linux, Wayland or X11 at runtime.
3. Loads settings from `laqta-library`, builds the port adapters, the `Codecs` value and the
   `Spawner`, and constructs `laqta-app`. The `Spawner` uses `async-executor` on one worker
   thread, a blocking pool of its own for blocking I/O (`blocking_pool.rs`; threads exit after
   10 s idle), and a `rayon` compute pool built on first use and released by its timer. None of
   these depend on GPUI (ADR 020).
4. Subscribes `laqta-library`'s history writer to task events, lets the history release its
   connection when idle, and attaches it to the application for tag and delete edits.
5. Starts the tray, hotkeys and IPC listener on their own threads, all calling `dispatch`.
6. Runs GPUI's application loop on the main thread. A start without `-silent` or `-autoclose`
   opens the main window.

`apps/laqta-nmh` is a second binary: the browser native-messaging host. It forwards messages to
the running instance over IPC.

---

## 3. Dependency graph

Every workspace dependency edge. The CI guard checks this list (§ "CI guard" below).

```text
apps/laqta        ──▶ every crate below except laqta-photo (composition root)
apps/laqta-nmh    ──▶ laqta-ipc

laqta-ui          ──▶ laqta-app (dispatch, application handle), laqta-ports, laqta-domain,
                       laqta-editor (hosts the engine: editor window and region overlay),
                       laqta-library (reads, domain types only),
                       laqta-imaging (thumbnails and the overlay magnifier),
                       laqta-photo (the photo editor window)
laqta-platform-*  ──▶ laqta-ports, laqta-domain
laqta-upload      ──▶ laqta-ports, laqta-domain, laqta-http
laqta-cloud       ──▶ laqta-ports, laqta-domain, laqta-http
laqta-ml          ──▶ laqta-ports, laqta-domain, laqta-imaging, laqta-http
laqta-media       ──▶ laqta-ports, laqta-domain, laqta-http
laqta-library     ──▶ laqta-ports (task events), laqta-domain
laqta-update      ──▶ laqta-http
laqta-import      ──▶ laqta-domain
laqta-ipc         ──▶ laqta-domain
laqta-http        ──▶ (no workspace crates)

laqta-app         ──▶ laqta-ports, laqta-imaging, laqta-domain  (laqta-editor is allowed but unused: the engine runs in laqta-ui, ADR 029)
laqta-ports       ──▶ laqta-domain
laqta-editor      ──▶ laqta-imaging, laqta-domain
laqta-photo       ──▶ laqta-editor, laqta-imaging, laqta-domain   (ADR 027)
laqta-imaging     ──▶ laqta-domain
laqta-domain      ──▶ (no workspace crates)
```

- **Edges between outer crates:** only `laqta-ui` → `laqta-library` (reads) and the uses of
  the `laqta-http` library. Outer crates depending on the pure core or on `laqta-ports` are
  ordinary inward edges.
- **No cycles:** Cargo rejects them.
- **Stability:** `laqta-domain` and `laqta-ports` are the most depended-on crates and hold only
  data and traits (stable and abstract). `laqta-app` is concrete, and only `laqta-ui` and
  `main` depend on it. `apps/laqta` depends on everything, and nothing depends on it.
- **CI guard** (ADR 024): `cargo xtask check-deps`, configured in `deps-policy.toml`, fails the
  build when any of these breaks:
  - **Workspace edges:** each crate's workspace dependencies must be on the list above.
  - **Allow-lists:** each pure-core crate (`laqta-domain`, `laqta-imaging`, `laqta-editor`,
    `laqta-photo`) and `laqta-ports` may depend directly only on the crates on its allow-list.
  - **Transitive deny-list:** for those crates and `laqta-app`, no transitive dependency on a
    crate that does I/O or drives the OS (gpui, tokio, reqwest, rusqlite, libsqlite3-sys, ort,
    pipewire, libwayshot, wayland-client, wayland-protocols, wayland-protocols-wlr, x11rb,
    windows, zbus, ashpd, notify, mio, keyring-core, arboard, ksni, interprocess, …; the list
    names exact crates, with no wildcards). `windows-sys` is allowed, because Cargo's feature
    unification lets `jiff` reach it; the clippy rules below stop the core from calling OS APIs.
  - **Framework placement:** `gpui-pre`, `gpui-pre-platform` and `gpui-component` may be direct
    dependencies only of `laqta-ui` and `apps/laqta`; `tokio` and `reqwest` only of
    `laqta-http`. This is checked for every workspace crate.
  - **Presenter imports:** no file under `crates/laqta-ui/src/presenters/` imports `gpui` or
    `laqta_library`.
- **Clippy:** each pure-core crate has a `clippy.toml` whose `disallowed-methods` and
  `disallowed-types` list each banned item by path, because clippy can't ban whole modules. The
  list covers file, thread, process, network and environment APIs; clock reads
  (`SystemTime::now`, `Instant::now`, `jiff::Zoned::now`, `jiff::Timestamp::now`);
  `rayon::ThreadPoolBuilder`; and `cosmic_text::FontSystem::new`. `laqta-app` has its own
  `clippy.toml` that bans only the `std::fs` items, `Path::exists` and `Path::metadata`. Its
  `src/files.rs` alone carries `#![allow(clippy::disallowed_methods, clippy::disallowed_types)]`.
  The full lists are in each crate's `clippy.toml`. The `check` CI job runs clippy with `-D warnings`.

---

## 4. Key flows

### 4.1 Region capture

```text
platform hotkey ─▶ dispatch(Action::RectangleRegion, workflow)
  ├─ ScreenSource.capture()                after the configured delay: frozen frames, one Arc buffer per monitor
  ├─ Region session.select(frames, options)
  │    ├─ Overlay.show(...)                shown before hover targets load (120 ms budget)
  │    ├─ HoverSource loads targets in the background
  │    ├─ a drawn or clicked region stays selected; the app crops it and the overlay shows the
  │    │  capture toolbar beside it, with the Editor engine on the crop (RegionView.selection_image)
  │    ├─ the first annotation locks the selection; the overlay sends the edited crop (RegionInput::Annotated)
  │    └─ returns Selection { rects, target window, image, action } | Cancelled
  ├─ after_capture_for(settings, selection.action)   the toolbar button picks the after-capture flags
  └─ Task.submit(capture job, settings) ─▶ task events
```

With `show_tools_after_select` off, or for a tool that acts on an area at once (`region::pick`),
the region confirms on release and no toolbar appears.

### 4.2 Task

```text
Task.submit(job, settings)
  ├─ after-capture steps (private plan, encode once, save, copy, …)
  ├─ upload through Destination, with bounded retries
  ├─ after-upload steps, only if a URL was expected and returned
  └─ cleanup across both phases ─▶ task events ─▶ tray, task list, toasts, history writer
```

### 4.3 Annotation editor

```text
editor window ── input events ──▶ Editor engine ──▶ dirty rects + pixels + chrome description
editor window ◀── paints texture and chrome ───────┘
export ─▶ Editor engine renders at full resolution (same renderer)
region overlay ── the same loop on the selection's crop ──▶ annotated crop back to the Region session
region overlay ── TextCommand::Read ──▶ Region session ── Ocr on the crop ──▶ RegionView::text
region overlay ◀── words lit on the frozen screen; a drag picks, TextCommand::Copy copies (ADR 031)
editor "Smart tools" ─▶ App::recognize_text | remove_background | erase | analyze
                         ├─ an image comes back ─▶ Command::ReplaceImage (undoable)
                         └─ text comes back ─▶ the text window (ToolHost::show_ocr for OCR)
```

### 4.4 Recording

```text
dispatch(Action::ScreenRecorder) ─▶ Recording session
  ├─ Region session.pick(...)                      only for the region area; the other areas need no selection
  ├─ countdown
  ├─ ScreenStream.open(one rect, audio inputs)     Frames (PipeWire) or Target (Windows, X11)
  ├─ Media.start(input, record spec, output)       pause, resume, stop, abort
  └─ finished file ─▶ Task.submit(upload job of the saved file)
```

FFmpeg is found on `PATH`, at a configured path, or downloaded on first use and checked against
a pinned hash (§6). If FFmpeg exits partway, the session emits a failure event and keeps the
partial file, because the container survives a crash.

---

## 5. UI (GPUI)

- **Design system:** one theme token set (colour, radius, spacing, type scale) with light, dark
  and system modes, applied through `gpui-component` theming (ADR 028). There are no motion
  tokens. Mica / Acrylic on Windows 11 and background blur on Wayland compositors, with an opaque
  fallback for a software renderer, reduced transparency or a user toggle, are not built:
  windows are opaque or transparent only.
- **Surfaces:** main window (task list, quick capture bar, destinations); settings (one page per
  section, per-workflow overrides shown as differences from defaults; only the language list is
  searchable); history (list and a paged image grid; thumbnails are decoded into an in-memory
  cache); editor, with its smart tools (text recognition, background removal, object erase,
  analyze); effects editor with live preview; region overlay with the capture toolbar and the
  selection's text read in place; recording bar; toasts; pin-to-screen windows; the macOS permissions onboarding; tool windows,
  including the text window for analysis results and editable OCR text. The platform `Tray` adapter owns the tray icon and its native menu, built from
  the presenters' tray menu model; `laqta-ui` also draws a GPUI pop-up tray menu
  (`Action::ToggleTrayMenu`).
- **Rule:** a view may read a presenter or settings, call `dispatch`, paint Editor engine output,
  or implement `Overlay`, `Prompt` or `ToolHost`. It may not compute business results. Work done
  inside an open window calls methods on `App` (the AI use cases, history edits), not ports.
- **GPUI version:** `gpui-pre` pinned exactly to the snapshot `gpui-component` is built against
  (ADR 002). That snapshot includes the Wayland `LayerShell` window kind the overlay uses.

---

## 6. Cross-cutting concerns

| Concern | Decision |
| - | - |
| Executors | Built by `main` and reached through `Spawner`: an `async-executor` worker for the application, a blocking pool of `main`'s own for blocking I/O (`blocking_pool.rs`: threads exit after 10 s idle, at most 64, because the `blocking` crate never retires its last thread), and a `rayon` compute pool for CPU-heavy work, built on first use. GPUI's executors run UI work only. Tokio runs only inside `laqta-http` (ADRs 007, 020, 024) |
| Panics | Unwinding stays on. Each Task job and each tool run is wrapped in `catch_unwind`; a panic hook writes the panic to the log (ADR 020) |
| Errors | One application error type with user-presentable kinds (`AppError`, in `laqta-ports`) |
| Logging | `tracing` with a daily rolling file appender and a panic hook. There is no in-app log viewer yet |
| Secrets | Never in JSON. Settings hold a reference that resolves through `SecretStore`. With no OS keyring, `SecretStore` returns `Unsupported`, and `main`'s `SessionSecrets` keeps the credentials in memory for the session, so they are asked for again on the next launch |
| Supply chain | Every download (FFmpeg, onnxruntime, models, updates) goes through `laqta-http`'s verified fetch, with a SHA-256 pinned in the binary or in the model catalog; an update's expected hash comes from the `SHA256SUMS` asset of its own release, because a binary can't pin a later release's hash. A mismatch deletes the file and fails the step |
| Other security | `.sxcu` and `.sxie` imports are data, never code. FFmpeg arguments are built as argv, never through a shell. OAuth uses PKCE with a loopback redirect |
| Localization | Fluent (`.ftl`) bundles in `locales/`; `scripts/resx-to-ftl.py` converts `.resx` string tables to Fluent once, to seed about 30 languages |
| Updates | `laqta-update`: GitHub Releases on Windows, downloaded by verified fetch and applied by Velopack; on Linux a notice only, since packages handle updates |
| Testing | Each deep module is tested at its interface:<br>• Domain: unit and property tests<br>• Editor engine: input script in, image snapshot out<br>• Task: golden event traces<br>• Region session: scripted input with a fake `HoverSource`<br>• Upload module: local HTTP server and recorded fixtures<br>• Library: temporary directory and in-memory SQLite<br>• Platform modules: a contract test per port<br>• UI: GPUI `TestAppContext` smoke tests |

---

## 7. Workspace layout

The tree shows the repository's crates, apps and support files. A planned path that was never
built (a `bench.yml` workflow, `laqta-codecs-c`) is not listed.

```text
LICENSE  GPL-3.0 text (ADR 022)
Cargo.toml  workspace, [workspace.lints], release profile; patches `blocking` from vendor/
deny.toml  licences, advisories, bans
.gitignore  rustfmt.toml  ci/sway.conf  headless sway for the Wayland CI job
rust-toolchain.toml  deps-policy.toml  CI dependency guard config
xtask/  check-deps [--self-test], bench-png, measure-idle
.github/workflows/ci.yml  test + guard matrix
crates/
  laqta-domain/  (clippy.toml)
  laqta-imaging/  (clippy.toml, examples/png_bench.rs)
  laqta-ports/  laqta-editor/  (clippy.toml, tests/snapshots/)
  laqta-photo/  (clippy.toml)
  laqta-app/  (clippy.toml, src/files.rs, tests/golden/ for Task traces)
  laqta-upload/  (catalog/*.sxcu, hosts/<host>.rs; nine hosts have a host-<host> feature)
  laqta-http/  laqta-cloud/  laqta-update/  laqta-media/  laqta-library/  laqta-ml/  (catalog/models.toml, families/<family>.rs)
  laqta-import/  laqta-ui/  (src/presenters/)
  laqta-ipc/  laqta-platform-windows/  laqta-platform-linux/  laqta-platform-macos/  apps/
  laqta/  composition root
  laqta-nmh/  native-messaging host
vendor/  patched `blocking` and `gpui-component` (vendor/README.md)
locales/  Fluent bundles
scripts/  resx-to-ftl.py, bench-analyze.py, bench-sway.sh
docs/                               ARCHITECTURE.md · FEATURES.md · GLOSSARY.md · adr/
```

---

## 8. Roadmap

Each milestone lists its scope and exit checks below. A milestone is done when its exit checks pass in CI on
Windows and Linux, and its manual checks pass on Windows 11, a wlroots compositor, KDE Plasma,
GNOME and X11. Feature lists are the FEATURES.md rows tagged with that milestone.

| # | Milestone | Scope | Exit checks |
| - | - | - | - |
| M0 | Skeleton | Workspace, domain types, M0 ports and fakes, dispatcher, minimal Task (capture → save → copy), single-instance IPC, tray, one hotkey, fullscreen capture | Task golden traces for the M0 job; dispatcher routing test; platform contract tests; CI guard; PNG budget; idle RSS measured |
| M1 | Capture core | Region session and overlay, every capture mode in FEATURES.md "Capture" except scrolling, auto and text capture; after-capture steps that don't upload; toasts; history; naming; settings with workflows and the hotkey editor | Region session scripted-input tests; Task traces for every after-capture flag in M1's scope (the steps that don't upload and don't need the editor, OCR, pinning or AI); overlay latency and capture-memory budgets (measured by hand; pending, owner) |
| M2 | Upload core | Upload module with the M2 hosts in FEATURES.md; after-upload steps; quick task menu; actions toolbar; after-capture and before-upload windows; Actions; URL sharing; clipboard, drag-and-drop, text, URL and folder upload | Upload fixtures for each M2 host; `.sxcu` syntax tests covering all 13 functions; Task traces for upload failure, retry and cleanup |
| M3 | Editor | Editor engine with the 21 tools, undo and redo, canvas operations, the 60 classic image effects, presets and `.sxie` import, beautifier, annotating in the overlay | Editor snapshot tests per tool and effect; export matches view |
| M4 | Recording | Recording session; FFmpeg video and GIF; audio; pause, stop and abort; countdown; recording bar; fixed duration | Recording session tests with a fake stream; recording CPU budget; crash-survival test (kill FFmpeg midway) |
| M5 | Tools | Every row in FEATURES.md "Tools" tagged M5, plus the mouse highlighter | Pure-tool tests; windowed-tool smoke tests |
| M6 | Integration | Scrolling capture, auto capture, Watch folders, the ShareX command-line arguments, native messaging, ShareX importer, sandbox mode, localization, autostart, shell integration (Explorer context menu and Send To; Linux `.desktop` actions), auto-update | Importer fixtures from real ShareX configs; CLI routing tests |
| M7 | Remaining features | Photo editor; the newer effects beyond the 60; background removal; AI image analysis; camera overlay; remote storage browser; native Windows recorder; the M7 hosts in FEATURES.md | Exit checks are set when M7 starts |

macOS work starts after M7 (ADR 001).

---

## 9. Decisions

Full records are in [`adr/`](adr/).

| ADR | Decision | Main option rejected |
| - | - | - |
| [001](adr/001-platforms.md) | Windows 10+ and Linux (Wayland and X11) first; macOS after M7 | all three platforms at once |
| [002](adr/002-gpui-pinned.md) | `gpui-pre` pinned to `gpui-component`'s snapshot | egui, Slint, iced |
| [003](adr/003-one-rasterizer.md) | One CPU rasterizer (`tiny-skia`) for view and export | GPU renderer (vello) |
| [004](adr/004-simple-hosts-as-sxcu.md) | Simple hosts ship as bundled `.sxcu` data | one Rust module per host |
| [005](adr/005-ffmpeg-subprocess.md) | FFmpeg subprocess; native Windows recorder later | linking libav, GStreamer |
| [006](adr/006-json-settings-sqlite-history.md) | Versioned JSON settings; SQLite history | TOML, key-value stores |
| [007](adr/007-tokio-only-in-http.md) | Tokio only inside `laqta-http` | `ureq` on a blocking pool, smol-based HTTP |
| [008](adr/008-option-patches.md) | Workflow overrides as `Option` patches | per-field `UseDefault*` booleans |
| [009](adr/009-command-undo.md) | Undo by commands | full-image mementos |
| [010](adr/010-dead-services.md) | Services that have shut down are not supported | supporting dead endpoints |
| [011](adr/011-task-module.md) | One Task module owns the task lifecycle | public step objects plus several interactors |
| [012](adr/012-region-session.md) | One Region session; overlay first, hover targets in the background | snapshot windows, then open the overlay |
| [013](adr/013-editor-engine.md) | One Editor engine crate; pure libraries allowed in the core | tool logic split across five crates |
| [014](adr/014-upload-module.md) | One Upload module behind `Destination` | split engine, hosts and transport ports |
| [015](adr/015-action-dispatcher.md) | One Action dispatcher | one interactor per operation |
| [016](adr/016-screen-stream.md) | Recording consumes a `ScreenStream` that also names audio devices | media calling platform capture code |
| [017](adr/017-presenters-in-ui.md) | Presenters are a GPUI-free module inside `laqta-ui` | a separate presenters crate |
| [018](adr/018-unsupported-not-capabilities.md) | Ports return `Unsupported`; settings and history are not ports | capability flags checked by callers |
| [019](adr/019-wayland-degradation.md) | Wayland backends and fallbacks for capture, overlay and hotkeys | requiring layer-shell and GlobalShortcuts |
| [020](adr/020-panics-and-executors.md) | Unwinding with `catch_unwind` per job; GPUI-independent executors | `panic = "abort"`; GPUI's executor for app work |
| [021](adr/021-seam-rule.md) | The seam rule in §2.3, and the port changes it caused (amended 2026-11-07: file access through `files`) | counting any fake as a second adapter |
| [022](adr/022-licence.md) | Laqta is GPL-3.0; recording uses the GPL FFmpeg build | permissive licence; LGPL FFmpeg |
| [023](adr/023-model-families.md) | Local models through a catalog and model families; users pick a model or add their own of a supported family | one fixed model per task; arbitrary files with no family |
| [024](adr/024-ports-crate-and-core-purity.md) | A `laqta-ports` crate; injected pool, fonts and codecs; neutral port vocabulary; CI allow-lists; `laqta-app` file access confined to `files`; `cosmic-text` on a caret requirement | ports inside `laqta-app`; deny-list only |
| [025](adr/025-windows-tls-and-sftp.md) | Windows uses SChannel for HTTP TLS and the system OpenSSH `sftp.exe` for SFTP; Linux keeps rustls and `russh` | `cargo-xwin` with the Windows SDK; GNU-only Windows checks |
| [026](adr/026-windows-sqlite.md) | Linux bundles SQLite; Windows links a static vcpkg SQLite with FTS5 | bundling SQLite on Windows (needs the MSVC C runtime headers on the checking machine) |
| [027](adr/027-photo-uses-editor.md) | `laqta-photo` depends on `laqta-editor` for fonts and text layout (its rasterizer is private, so shapes and brushes use tiny-skia directly) | a copy of those parts in `laqta-photo`; a shared third crate |
| [028](adr/028-ui-design-system.md) | One design system in `laqta-ui`: theme tokens, shared components, bundled Inter and Space Grotesk, `gpui-kit-assets` icons, a custom titlebar | inline styling per window; a design-system crate; the platform UI font; the system titlebar |
| [029](adr/029-capture-toolbar-and-in-place-annotation.md) | A selected region stays adjustable under a capture toolbar; the overlay hosts the Editor engine on the selection while the app owns the pixels; the chosen action becomes Task flags; AI work is `App` use cases | the Region session hosting the engine; an editor window for every annotation; windows calling AI ports |
| [030](adr/030-macos.md) | macOS through one adapter crate on `objc2`, ScreenCaptureKit and Carbon hotkeys, proven in CI | a Finder Sync extension; macOS 13; a Swift helper |
| [031](adr/031-text-read-in-place.md) | The Region session reads the selection's text and the overlay shows it on the frozen screen: words light up, a drag picks them, a bar copies them; the text window stays for editing | the overlay calling the recogniser or the clipboard; a window over a live screen; locking the selection; selecting by character |

---

## 10. Risks

| Risk | Mitigation |
| - | - |
| GNOME (Mutter) has no layer-shell, so the overlay cannot use `LayerShell` there | When the layer surface is refused, the overlay opens a pop-up window sized to each monitor (in `laqta-ui`'s `overlay.rs`; not yet run on GNOME). Latency is measured but not budgeted on GNOME (ADR 019) |
| The GlobalShortcuts portal is missing (for example on `xdg-desktop-portal-wlr`), so hotkeys don't work | `laqta --action <name>` over IPC, bound in compositor settings; the settings page should show the command (ADR 019) but does not yet |
| The GPUI snapshot pin, `LayerShell` and the overlay latency budget depend on one external project | Pin exactly; upgrade only together with `gpui-component`; overlay latency is read from trace spans by hand (no CI bench yet) |
| FFmpeg is missing, crashes or is tampered with | Pinned-hash download; crash-surviving container; failure event keeps the partial file |
| A panic in a decoder, kernel or host could end the resident process | `catch_unwind` per job and tool run, with panics logged (ADR 020) |
| GPUI may hold a GPU device and fonts with no windows, pushing idle RSS over 40 MB | Measured in M0. If over budget, an ADR chooses between raising the budget and running the UI as a separate on-demand process. Tray, hotkeys and IPC already run without GPUI. Idle mitigations built: the vendored `blocking` crate with its patch (idle threads exit; `vendor/README.md`) and `main`'s own blocking pool (threads exit after 10 s idle) |
| Retrying an upload duplicates it on hosts that don't deduplicate | Retry only a connection error, a timeout, an HTTP 5xx or a 429, at most `settings.upload.retries` times (default 2, at most 10 in the settings window), with a 1 s then 3 s backoff |
| On GNOME Wayland, setting the clipboard may fail without a focused window | M0 manual check. If it fails, the toast should offer a Copy button, and clicking it gives the app focus. The toast has no Copy button yet: a failed copy shows only in the task list |
| Layering violations: GPUI types leak into the core, views grow rules, or Task gains a public step interface | CI guard (§3); region rules stay in `domain::region`, tool rules in `laqta-app`'s `tools` module, and annotation rules in the Editor engine; new after-capture behaviour is a flag inside Task plus a golden scenario |

---

## 11. Design-intent scorecard

These are properties of the design. Rows marked "CI" are enforced by a check from M0; the rest are
upheld by review.

| Property | Enforced by |
| - | - |
| Business rules testable without UI, DB, network or OS | review; fakes for every port |
| Source dependencies point inward | CI (`xtask check-deps`: workspace edge list, allow-lists, transitive deny-list) |
| The pure core does no I/O, starts no threads and reads no environment | CI (allow-lists, clippy `disallowed-methods`) |
| `laqta-app` touches the file system only through its async `files` module | CI (clippy, `laqta-app/clippy.toml`) |
| Port types use Laqta's vocabulary, never an adapter's | review (§2.3) |
| Adapters depend on `laqta-ports`, not `laqta-app` | CI (workspace edge list) |
| The application never sees storage | review (§2.3) |
| Every delivery mechanism goes through the Action dispatcher | review; dispatcher routing test |
| GPUI appears only in `laqta-ui` and `main`; Tokio only in `laqta-http` | CI (`xtask check-deps` framework-placement rule) |
| No dependency cycles | CI (Cargo) |
| Only `main` builds concrete adapters, the compute pool, the codec set and the font loader | review; CI for the pure core (clippy, `debug_assert` on pool use) |
| Every port meets the seam rule | review (§2.3 table) |
| One deep module per concept: Task, Region session, Recording session, Watch folders, Editor engine, Upload module | review |
