# Laqta architecture

Laqta (from Arabic لقطة, "a snapshot") is a Rust replacement for ShareX with a GPUI front end. Domain terms are defined in
[GLOSSARY.md](GLOSSARY.md), the feature checklist is [FEATURES.md](FEATURES.md), decisions are in
[`adr/`](adr/), each milestone's build steps are in [`plan/`](plan/), and the chosen libraries are in
[research/STACK.md](research/STACK.md).

Two rules shape the design:

1. **Dependencies point inward.** Rules about capturing, uploading, editing and recording never
   name GPUI, Win32, Wayland, FFmpeg, SQLite or HTTP.
2. **Few deep modules.** Each concept has one module with a small interface and most of the
   behaviour inside. A seam (a trait) exists only where the seam rule in §2.3 allows it.

---

## 1. Problem, goals and non-goals

### 1.1 Problem

The capture tools with this feature set run only on Windows. Linux users have no tool with this combination of workflows per
hotkey, after-capture and after-upload actions, custom uploaders, annotation editor and screen
recorder. Laqta provides the same feature set on Windows and Linux in one native binary that
costs almost nothing while it waits in the tray.

### 1.2 Goals and budgets

| Goal | Budget | How it is checked |
| - | - | - |
| Idle in tray | 0% CPU and zero wakeups while no lazy resource is held; RSS under 40 MB with no windows open | `cargo xtask measure-idle` on Linux and Windows: run by hand in M0 (the R6 gate, with GPUI running and no windows), then in the `bench.yml` CI workflow from M1. Wakeups are counted on Linux only |
| Hotkey to overlay visible | under 120 ms on a 4K screen, on Windows, X11 and Wayland compositors with layer-shell | trace spans from hotkey to first overlay frame (bench, M1) |
| Capture peak memory | region capture on two 4K monitors raises RSS by under 150 MB, released within 2 s of the task finishing | bench, M1 |
| Fullscreen capture → PNG on disk | under 250 ms for one 4K monitor on the reference machine | bench in `laqta-imaging`, M0 |
| Recording overhead | 1080p30 with a hardware encoder under 5% CPU on the reference machine | manual gate per release, M4 |
| Parity | 99% of non-dropped rows in FEATURES.md marked `done` | FEATURES.md status column |

**Reference machine:** Intel Core i5-1235U laptop with Iris Xe graphics, 16 GB RAM, a 1080p
internal panel and one 4K external monitor.

### 1.3 Non-goals

- Porting hosts that have shut down (ADR 010).
- macOS before M7 is complete (ADR 001).
- GTK, WebView or any embedded browser engine.
- Plugins or scripting. Custom uploaders and effect presets are data files, not code.
- Mobile platforms.

### 1.4 Resource rules

- **Windows are short-lived.** The main window, settings, history and editor are created on
  demand and dropped on close.
- **Tray, hotkeys and IPC do not need GPUI.** They run on their own threads (§2.5), so the tray
  keeps working with no window open, and the UI could move to a separate process if R6 requires it.
- **Lazy resources.** These start on first use: the HTTP runtime, the SQLite connection, the
  compute pool, the onnxruntime library, the OCR and background-removal models, the FFmpeg
  process, and the font database. Models, onnxruntime and FFmpeg are downloaded on first use
  through `laqta-http`'s verified fetch, with progress shown.
- **Release timers.** Each owner of a lazy resource arms one one-shot release timer while it
  holds the resource (60 s after the last use), using the `Clock` port. With nothing held, no
  timer exists and the process has no wakeups.
- **No CPU-side duplication in process.** A captured frame is an `Arc`-shared buffer. Crops are
  views until encoding. Copies happen only where a boundary requires one: uploading a texture to
  the GPU, or writing frames to the FFmpeg pipe.
- **Undo stores commands, not bitmaps** (ADR 009).

---

## 2. Modules

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ Outer: delivery and drivers                                                   │
│   laqta-ui (views + presenters) · laqta-platform-{windows,linux} ·          │
│   laqta-upload · laqta-cloud · laqta-ml · laqta-media · laqta-library ·  │
│   laqta-import · laqta-ipc · laqta-codecs-c · laqta-update ·             │
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
| `geometry` | Physical-pixel rects, points and monitor layouts (DPI conversion happens in platform code), snapping, aspect constraints |
| `capture` | Capture modes and options (delay, cursor, transparency, HDR) |
| `region` | The region-selection state machine: hover target, drag, resize handles, multi-region, keyboard nudge, magnifier sample rect, info text. Used only by the Region session |
| `workflow` | Workflows, task settings and their sections, the after-capture and after-upload flags (listed in FEATURES.md), destination choices, and the merge of a workflow's overrides onto defaults (ADR 008) |
| `action` | The `Action` enum, one variant per ShareX hotkey action. Its CLI and IPC names live in `laqta-ipc`, not here |
| `naming` | Name patterns and upload-info templates (token lists in FEATURES.md) |
| `effects` | Effects as data and presets. The `.sxie` package format is read and written by `laqta-import` |
| `history` | History items, tags, query filters |
| `settings` | The `Settings` value the application receives from `main` |
| `hotkey` | Platform-neutral key chords, the binding table, conflict detection |

**`laqta-imaging`**: pure pixel work. Effect kernels, resize, combine, split, scrolling-capture
stitching, hashing, QR encode and decode, and the image codecs (PNG, JPEG, WebP, AVIF, GIF, BMP,
TIFF, JPEG XL). Codecs are pure computation, so they are a library here, not a port.
- **Codec set.** `laqta-imaging` defines a codec-extension trait and a `Codecs` value listing
  the codecs available. `main` builds that value, adding the optional C codecs from
  `laqta-codecs-c` (lossy WebP encode, AVIF decode, HEIC) when their cargo feature is on. The
  value is passed to whoever encodes or decodes. There is no global registry.
- **Parallel work.** Kernels use `rayon` parallel iterators but never create a pool. They run
  inside the compute pool that `main` builds and hands to callers through
  `Spawner::run_compute`, which calls `pool.install`. Each public parallel function starts with
  `debug_assert!(rayon::current_thread_index().is_some())`, so a call outside the pool (which
  would quietly create rayon's global pool) fails in tests.

**`laqta-editor`**: the **Editor engine**, the single module behind the annotation editor,
annotating in the overlay, and the beautifier.

- Interface: input events and commands go in; dirty rects, rendered pixels and a chrome
  description come out. Export is the same renderer at full resolution.
- Inside: the document (base image plus annotations), the 21 tools listed in FEATURES.md and the
  canvas operations as state machines that turn pointer input into commands, undo and redo, the
  per-annotation effect cache, and rendering on `tiny-skia` with text through `cosmic-text` and
  `swash` (ADR 003).
- Fonts are injected. `main` builds a font loader (a function that loads the system font
  database and locale) and wraps it in the engine's lazy `Fonts` handle. The same handle goes to
  the Region session and to `laqta-ui`, and the loader runs on first use. `cosmic-text` is built with `default-features = false`, and the engine creates its
  font system with `FontSystem::new_with_locale_and_db`, so it never scans system fonts itself.
- Adding a tool edits this crate only.

**`laqta-photo`** (created in M7): the photo editor (layers, masks, blend modes, selections), a
second pure-core module beside the Editor engine. Its interface is decided in the M7 plan.

### 2.2 Application (`laqta-app`)

Four deep modules and a dispatcher. Nothing here depends on GPUI, Tokio or OS crates. It depends
on `laqta-ports` for every interface it calls, and on the pure core.

File access goes through one private `files` module (ADR 024). It uses `std::fs`, because the
file system is substitutable locally and tests use a temporary directory, so no port is needed.
Every `files` function is async: a private helper runs the work through `Spawner::run_blocking`
and returns the result over a oneshot channel. The single async worker therefore never blocks on
a slow disk. `laqta-app`'s `clippy.toml` bans the `std::fs` items, `Path::exists` and
`Path::metadata` everywhere else in the crate.

**Action dispatcher.** The single entry point for every delivery mechanism: hotkeys, the tray,
the main window, the CLI, single-instance IPC, browser native messaging and watch folders all
call `dispatch(action, workflow)`. It merges settings and routes the action to one of:

- Task, the Region session, the Recording session or Watch folders;
- a **pure tool**: a tool that only decodes, runs `laqta-imaging` and encodes (hash, resize,
  convert, split, combine, thumbnail, strip metadata, QR);
- a **windowed tool**: a tool with its own window (colour pickers, ruler, pin to screen, OCR,
  video tools, window tools, viewers). These open through the `ToolHost` port. Their rules
  (colour sampling and formats, ruler measurements, pin-window state and the other tool state
  machines) live in `laqta-app`'s pure `tools` module and in `laqta-imaging`. Their windows live in `laqta-ui`, render the tool's
  state, and reach ports and pixel work only through methods on the application handle (for
  example OCR, transcoding or sampling), so the work stays testable with fakes.

**Task.** Owns a task from input to result, because the steps
share ordering and cleanup rules (ADR 011).

- Interface: submit a task job (capture result, file, text, URL, folder, or URL to download and
  reupload) with its settings; receive a stream of task events (status, progress, result,
  errors); stop one task or all.
- Inside, private: the step plan and order, encoding once (only when save, save-as, OCR, upload or
  analyze needs it), thumbnails from the saved path, the large-file warning, the before-upload
  prompt, upload filters, bounded retries (risk R7), after-upload steps (run only when a URL was
  expected and returned), branching by job kind, cleanup that spans both phases (delete file,
  clear an early-copied URL on failure), the status machine, and the queue with its concurrency
  limit.
- The implementation is split into private sub-modules (queue, plan, steps, retry, cleanup).
  Their unit tests are internal; the golden traces test the interface.
- Each job runs inside `catch_unwind`. A panic becomes a `Failed` event instead of ending the
  process (ADR 020).
- Tests: golden scenarios ported from `WorkerTask.cs`, run against fake ports, asserting the
  event trace and side effects.

**Region session.** Owns selecting a region on screen, for capture and recording (ADR 012).

- Interface: given frozen frames and options, return a selection (rects, the target window's
  title and process, and the final image with any annotations already rendered) or `Cancelled`.
- Inside: shows the overlay through the `Overlay` port before hover targets load, loads hover
  targets in the background from `HoverSource`, runs the `domain::region` state machine and hit
  tests, and hosts the Editor engine for annotating in the overlay. When the user confirms, it
  renders the annotations into the cropped image through the Editor engine, so Task receives
  finished pixels.

**Recording session.** Owns a recording from start to file.

- Interface: start with a mode (region, window, custom, last region) and a record spec; pause,
  resume, stop or abort; receive recording events. The finished file goes to Task.
- A **record spec** holds the region, container, codec, frame rate, quality (CRF or bitrate),
  cursor on or off, and the audio device IDs to record.
- Inside: region selection through the Region session, the countdown, the fixed-duration stop,
  opening a `ScreenStream`, driving the `Media` port, and GIF encoding after stop.

**Watch folders.** Watches configured folders through `FileWatcher` and submits Task jobs with
each folder's workflow.

### 2.3 Ports and the seam rule

All ports live in **`laqta-ports`**, with the plain types they carry (`TaskEvent`, `AppError`,
`PortError`, `BoxFuture`, …). Data shared with the pure core, such as `Bitmap` and `Frame`, stays
in `laqta-domain`. That crate holds traits and data only. Adapters depend on
`laqta-ports` and `laqta-domain`, never on `laqta-app`, so a change inside Task or a session
doesn't touch any adapter (ADR 024).

Port types use Laqta's vocabulary, never an adapter's. FFmpeg arguments, Win32 handles,
Wayland objects and file formats stay in the adapter that maps them. One coupling is accepted
and contained (ADR 016): a recording `Target` carries an `OsScreenRef` (a Windows output and
adapter, or an X11 display and screen) and audio devices carry the OS's device name. The
application passes both through unread; only the platform adapter creates them and only
`laqta-media` reads them.

**Seam rule.** A port (trait) exists only if it has either:

- two production adapters (different platforms count), or
- one production adapter plus a fake that a deep module's tests need.

Async ports return boxed futures so they work behind `Arc<dyn Port>`. A port that cannot do
something on the current platform returns `Unsupported`, and callers handle that one error
instead of checking flags (ADR 018).

| Port | Production adapters | Fake needed by | Used by |
| - | - | - | - |
| `ScreenSource` (stills; always one frame per monitor, adapters split a pre-composed image) | Windows, Wayland, X11 | Task, Region session tests | Task, Region session |
| `ScreenStream`: video is `Frames` (a frame source) or `Target` (a screen target the recorder may capture itself, carrying a sealed `OsScreenRef` that only the platform adapter fills in and only `laqta-media` reads); audio inputs are `Device` (an audio device ID and name) or `Pcm` (a PCM source, such as Windows system audio) | WGC frame pool, PipeWire frames, screen targets for Windows and X11 | Recording session tests | Recording session |
| `HoverSource` | Win32 + UI Automation, X11 + AT-SPI, one Wayland compositor | Region session tests | Region session |
| `Overlay` | GPUI | Region session tests | Region session |
| `Media` (record a stream, transcode a file) | FFmpeg (native Windows recorder in M7) | Recording session tests | Recording session, windowed tools |
| `Destination`, `Shortener`, `Sharer` | one per host in `laqta-upload` | Task tests | Task |
| `Prompt` (ask for text, choose from a list, show text, confirm) | GPUI | Task tests | Task, `laqta-upload` |
| `ToolHost` | GPUI | dispatcher tests | dispatcher |
| `Clipboard` | Windows, Wayland, X11 | Task tests | Task, tools |
| `Notifier` | GPUI toasts, native toasts | Task tests | Task |
| `Shell` (open, reveal, print) | Windows, Linux | Task tests | Task, `laqta-upload` (opens the browser for OAuth) |
| `HotkeyRegistrar` | `RegisterHotKey`, GlobalShortcuts portal, `XGrabKey` | — | main |
| `Tray`, `Autostart` | Windows, Linux | — | main |
| `SecretStore` | Credential Manager, Secret Service | upload tests | `laqta-upload` |
| `Ocr` | `laqta-ml` (PP-OCR family and others in the catalog), Windows.Media.Ocr | Task tests | Task, tools |
| `BackgroundRemover` | `laqta-ml` (BiRefNet, BEN2 and the other families in the catalog), `laqta-cloud` (remove.bg) | tool tests | tools |
| `ImageAnalyzer` | `laqta-cloud`: one OpenAI-compatible client; provider, base URL and model name are configuration (OpenAI, OpenRouter, Gemini, Ollama, LM Studio) | Task tests | Task, tools |
| `FileWatcher` | `notify` | Watch folders tests | Watch folders |
| `Clock` (`now`, `sleep`) | system time and `async-io` timers | every deep module's tests (a manual clock) | everything with time: retries, countdown, fixed-length recording, release timers |
| `Rng` | system | tests with fixed seeds | Task, `laqta-upload` |
| `Spawner` (`spawn`, `run_blocking`, `run_compute`) | `async-executor`, `blocking`, the compute pool | every deep module's tests | everything |

`HotkeyRegistrar`, `Tray` and `Autostart` are used only by `main`. They are still ports, because
each has one adapter per platform and `main` must not contain platform code.

Local models are chosen per task from a catalog, or added by the user as custom models of a
supported model family (ADR 023). `laqta-ml` owns all of this behind `Ocr` and
`BackgroundRemover`.

**Capabilities** exist only as information for the settings UI and onboarding (for example,
"window detection is unavailable on this compositor"). No rule branches on them.

**Settings and history are not ports.** The application receives settings as a value from `main`,
and a new value whenever the user changes something. History is written by a task-event
subscriber in `laqta-library`. `laqta-ui` reads it through `laqta-library`'s query functions,
which return `laqta-domain` types only. Presenters receive those values and may not import
`laqta-library`.

**Platform state** that an adapter must keep between runs, such as the ScreenCast restore token,
is passed in by `main` when the adapter is built, and handed back through a callback that `main`
saves with `laqta-library`. Platform crates never touch settings storage.

### 2.4 Outer modules

| Crate | Role |
| - | - |
| `laqta-upload` | The **Upload module**: every host behind `Destination`, `Shortener` and `Sharer` (ADR 014). It holds the custom-uploader (`.sxcu`) engine and the bundled catalog (ADR 004). It also holds the hosts written in Rust: OAuth2 and OAuth1, S3 SigV4, Azure, B2, MEGA, FTP, FTPS, SFTP, SMTP and shared folder. It owns the OAuth loopback listener with PKCE and token refresh, and the remote-storage browser. `Prompt`, `Rng`, `Clock`, `Shell` and `SecretStore` are injected. Each host is one file behind its own cargo feature |
| `laqta-http` | A library, not a port. It runs a Tokio runtime on one thread, started on the first request, with one shared `reqwest` + rustls client and the SFTP client (ADR 007). It also holds **verified fetch**: download to a file with a pinned SHA-256, progress and cancellation, used for FFmpeg, onnxruntime, models and updates. Used by `laqta-upload`, `laqta-cloud`, `laqta-ml`, `laqta-media` and `laqta-update` |
| `laqta-cloud` | Cloud ML adapters: `ImageAnalyzer` (one OpenAI-compatible client) and the remove.bg `BackgroundRemover`. API keys come from `SecretStore` |
| `laqta-update` (M6) | Update checks and installs: GitHub Releases through `laqta-http`'s verified fetch, applied by Velopack on Windows; a notice only on Linux |
| `laqta-media` | The `Media` adapter: FFmpeg with hardware or software encoders, GIF through FFmpeg's `palettegen` / `paletteuse`, audio mux (piping Windows loopback PCM to FFmpeg as a second input), cursor overlay. It consumes a `ScreenStream` and never calls platform code. It alone maps Laqta's screen targets and audio inputs to FFmpeg inputs (`ddagrab`, `x11grab`, `dshow`, `pulse`, raw frames and PCM on pipes). It records into a container that survives a crash (fragmented MP4 or MKV), then remuxes on stop |
| `laqta-library` | Settings (versioned JSON, migrations, atomic writes, rotating and weekly backups, export and import archives, portable mode) and history (SQLite with WAL, thumbnail cache, task-event subscriber) |
| `laqta-import` | Reads ShareX config, history, `.sxcu`, `.sxie` (it owns the package format) and backup archives into Laqta values. Secrets encrypted with DPAPI decrypt only on Windows; elsewhere the user re-enters them |
| `laqta-ui` | GPUI views: main window, settings, history, editor window (paints Editor engine output), overlay (`Overlay`), prompts (`Prompt`), tool windows (`ToolHost`), recording bar, toasts, pin windows. Its `presenters` module holds GPUI-free logic (task-list rows, history paging, settings shown as differences from defaults, the tray menu model, and the editor and photo **Editor interaction**: input, render gating and snapshot renders, with compute injected as a function so the R1 A2 facade can replace it). The CI guard keeps that module free of `gpui` and `laqta_library` imports (ADR 017). It receives the `Fonts` handle from `main` |
| `laqta-ml` | The **model runtime** (ADR 023):<br>• the model catalog (`catalog/models.toml`) and the user's custom models;<br>• downloads through `laqta-http`'s verified fetch;<br>• `ort` sessions (loaded lazily, at most one per task, released by the release timer) and the execution-provider setting (Auto, CPU, GPU);<br>• the model families, one file each (a parametric family for models that differ only in numbers, plus dedicated families);<br>• validation of custom models.<br>Internal modules (catalog, runtime and sessions, families, validation) each have their own tests. Implements `Ocr` and `BackgroundRemover` |
| `laqta-codecs-c` | Optional codecs that need C libraries (`webp` for lossy WebP, `dav1d` for AVIF decode, WIC / `libheif-rs` for HEIC), behind cargo features |
| `laqta-ipc` | Single-instance lock and forwarding of commands between instances, over a named pipe (Windows) or a Unix socket (Linux). It owns the text names of actions used by the CLI and the IPC protocol |
| `laqta-platform-windows` | Capture via Windows.Graphics.Capture and DXGI duplication (HDR via DXGI 1.6). DWM capture of transparent windows. Hover targets from `EnumWindows` + UI Automation. Hotkeys and the tray on a dedicated message-loop thread. WinRT toasts, Credential Manager, the Startup task, shell verbs and Send To |
| `laqta-platform-linux` | Capture, overlay and hotkey backends with their Wayland fallbacks (ADR 019). Also: SNI tray with a DBusMenu menu (`ksni`), freedesktop notifications, Secret Service, XDG autostart, AT-SPI |

### 2.5 Composition root (`apps/laqta`)

`main.rs` is the only place that names every concrete type. It:

1. Parses CLI arguments. If another instance is running, it forwards them over `laqta-ipc` and exits.
2. Picks the platform module with `cfg(target_os)` and, on Linux, Wayland or X11 at runtime.
3. Loads settings from `laqta-library`, builds the port adapters, the `Codecs` value and the
   `Spawner`, and constructs `laqta-app`. The `Spawner` uses `async-executor` on one worker
   thread, the `blocking` pool for blocking I/O, and a `rayon` compute pool built on first use
   and released by its timer. None of these depend on GPUI (ADR 020).
4. Subscribes `laqta-library`'s history writer to task events.
5. Starts the tray, hotkeys and IPC listener on their own threads, all calling `dispatch`.
6. Runs GPUI's application loop on the main thread with no windows open.

`apps/laqta-nmh` is a second binary: the browser native-messaging host. It forwards messages to
the running instance over IPC.

---

## 3. Dependency graph

Every workspace dependency edge. The CI guard checks this list (§ "CI guard" below).

```text
apps/laqta        ──▶ every crate below (composition root)
apps/laqta-nmh    ──▶ laqta-ipc

laqta-ui          ──▶ laqta-app (dispatch, application handle), laqta-ports, laqta-domain,
                       laqta-editor (paints its output), laqta-library (reads, domain types only),
                       laqta-imaging (thumbnails and the overlay magnifier; added in M1, D9),
                       laqta-photo (the photo editor window; M7)
laqta-platform-*  ──▶ laqta-ports, laqta-domain
laqta-upload      ──▶ laqta-ports, laqta-domain, laqta-http
laqta-cloud       ──▶ laqta-ports, laqta-domain, laqta-http
laqta-ml          ──▶ laqta-ports, laqta-domain, laqta-imaging, laqta-http
laqta-media       ──▶ laqta-ports, laqta-domain, laqta-http
laqta-library     ──▶ laqta-ports (task events), laqta-domain
laqta-update      ──▶ laqta-domain, laqta-http
laqta-import      ──▶ laqta-domain
laqta-codecs-c    ──▶ laqta-imaging            (implements the codec-extension trait)
laqta-ipc         ──▶ laqta-domain
laqta-http        ──▶ (no workspace crates)

laqta-app         ──▶ laqta-ports, laqta-editor, laqta-imaging, laqta-domain
laqta-ports       ──▶ laqta-domain
laqta-editor      ──▶ laqta-imaging, laqta-domain
laqta-photo       ──▶ laqta-editor, laqta-imaging, laqta-domain   (M7, ADR 027)
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
  - **Allow-lists:** each pure-core crate (`laqta-domain`, `laqta-imaging`, `laqta-editor`)
    and `laqta-ports` may depend directly only on the crates on its allow-list.
  - **Transitive deny-list:** for those crates and `laqta-app`, no transitive dependency on a
    crate that does I/O or drives the OS (gpui, tokio, reqwest, rusqlite, libsqlite3-sys, ort,
    pipewire, libwayshot, wayland-*, x11rb, windows, zbus, ashpd, notify, mio, keyring-core,
    arboard, ksni, interprocess, …). `windows-sys` is allowed, because Cargo's feature
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
  The full lists are in the M0 plan. The `check` CI job runs clippy with `-D warnings`.

---

## 4. Key flows

### 4.1 Region capture

```text
platform hotkey ─▶ dispatch(Action::RectangleRegion, workflow)
  ├─ ScreenSource.capture_all()            frozen frames, one Arc buffer per monitor
  ├─ Region session.select(frames, options)
  │    ├─ Overlay.show(...)                shown before hover targets load (120 ms budget)
  │    ├─ HoverSource loads targets in the background
  │    └─ returns Selection { rects, target window, final image } | Cancelled
  └─ Task.submit(capture job, settings) ─▶ task events
```

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
```

### 4.4 Recording

```text
dispatch(Action::ScreenRecorder) ─▶ Recording session
  ├─ Region session.select(...)
  ├─ countdown
  ├─ ScreenStream.open(selection, audio inputs)    Frames (WGC, PipeWire) or Target (Windows, X11)
  ├─ Media.record(stream, record spec)             pause, resume, stop, abort
  └─ finished file ─▶ Task.submit(file job)
```

FFmpeg is found on `PATH`, at a configured path, or downloaded on first use and checked against
a pinned hash (§6). If FFmpeg exits partway, the session emits a failure event and keeps the
partial file, because the container survives a crash.

---

## 5. UI (GPUI)

- **Design system:** one theme token set (colour, radius, spacing, type scale, motion) with light,
  dark and system modes, applied through `gpui-component` theming. Mica / Acrylic on Windows 11
  and background blur on Wayland compositors that support it. The opaque fallback applies when the
  GPU adapter is a software renderer (WARP, llvmpipe), when the OS asks for reduced transparency,
  or when the user turns it off.
- **Surfaces:** main window (task list, quick capture bar, destinations); settings (searchable,
  one page per section, per-workflow overrides shown as differences from defaults); history (list
  and a virtualized image grid backed by the thumbnail cache); editor; effects editor with live
  preview; region overlay; recording bar; toasts; pin-to-screen windows; tool windows. The tray
  menu is a native menu owned by the platform `Tray` adapter, built from the presenters' tray
  menu model.
- **Rule:** a view may read a presenter or settings, call `dispatch`, paint Editor engine output,
  or implement `Overlay`, `Prompt` or `ToolHost`. It may not compute business results.
- **GPUI version:** `gpui-pre` pinned exactly to the snapshot `gpui-component` is built against
  (ADR 002). That snapshot includes the Wayland `LayerShell` window kind the overlay uses.

---

## 6. Cross-cutting concerns

| Concern | Decision |
| - | - |
| Executors | Built by `main` and reached through `Spawner`: an `async-executor` worker for the application, the `blocking` pool for blocking I/O, and a `rayon` compute pool for CPU-heavy work, built on first use. GPUI's executors run UI work only. Tokio runs only inside `laqta-http` (ADRs 007, 020, 024) |
| Panics | Unwinding stays on. Each Task job and each tool run is wrapped in `catch_unwind`; a panic hook writes the panic to the log (ADR 020) |
| Errors | One application error type with user-presentable kinds; signatures are in the M0 plan |
| Logging | `tracing` with a rolling file appender, plus an in-app log viewer (as in ShareX) |
| Secrets | Never in JSON. Settings hold a reference that resolves through `SecretStore`. With no OS keyring, `SecretStore` returns `Unsupported`, and credentials are asked for once per session and kept in memory |
| Supply chain | Every download (FFmpeg, onnxruntime, models, updates) goes through `laqta-http`'s verified fetch, with a SHA-256 pinned in the binary or in the model catalog. A mismatch deletes the file and fails the step |
| Other security | `.sxcu` and `.sxie` imports are data, never code. FFmpeg arguments are built as argv, never through a shell. OAuth uses PKCE with a loopback redirect |
| Localization | Fluent (`.ftl`) bundles in `locales/`; `scripts/resx-to-ftl` converts ShareX translations once to seed about 30 languages |
| Updates | `laqta-update`: GitHub Releases on Windows, downloaded by verified fetch and applied by Velopack; on Linux a notice only, since packages handle updates |
| Testing | Each deep module is tested at its interface:<br>• Domain: unit and property tests<br>• Editor engine: input script in, image snapshot out<br>• Task: golden event traces<br>• Region session: scripted input with a fake `HoverSource`<br>• Upload module: local HTTP server and recorded fixtures<br>• Library: temporary directory and in-memory SQLite<br>• Platform modules: a contract test per port<br>• UI: GPUI `TestAppContext` smoke tests |

---

## 7. Workspace layout

Every path below is created by the milestone named in its comment. M0 creates only the paths
marked M0.

```text
LICENSE                             M0  GPL-3.0 text (ADR 022)
Cargo.toml                          M0  workspace, [workspace.lints], release profile
deny.toml                           M0  licences, advisories, bans
.gitignore                          M0
ci/sway.conf                        M0  headless sway for the Wayland CI job
rust-toolchain.toml                 M0
deps-policy.toml                    M0  CI dependency guard config
xtask/                              M0  check-deps, bench runner
.github/workflows/ci.yml            M0  test + guard matrix
.github/workflows/bench.yml         M1  budgets from §1.2
crates/
  laqta-domain/                    M0  (clippy.toml)
  laqta-imaging/                   M0  (clippy.toml, examples/png_bench.rs)
  laqta-ports/                     M0
  laqta-editor/                    M3  (clippy.toml, tests/snapshots/)
  laqta-photo/                     M7
  laqta-app/                       M0  (clippy.toml, src/files.rs, tests/golden/ for Task traces)
  laqta-upload/                    M2  (catalog/*.sxcu, hosts/<host>.rs with feature host-<host>, tests/fixtures/)
  laqta-http/                      M2
  laqta-cloud/                     M7
  laqta-update/                    M6
  laqta-media/                     M4
  laqta-library/                   M1
  laqta-codecs-c/                  M1
  laqta-ml/                        M5  (catalog/models.toml, families/<family>.rs, tests/golden/)
  laqta-import/                    M6
  laqta-ui/                        M0  (src/presenters/)
  laqta-ipc/                       M0
  laqta-platform-windows/          M0
  laqta-platform-linux/            M0
apps/
  laqta/                           M0  composition root
  laqta-nmh/                       M6  native-messaging host
locales/                            M6  Fluent bundles
scripts/resx-to-ftl/                M6
docs/                               ARCHITECTURE.md · FEATURES.md · GLOSSARY.md · adr/ · plan/ · research/ · proposals/
```

---

## 8. Roadmap

Each milestone gets its own plan in `docs/plan/Mn.md`, written and reviewed before work starts.
The M0 plan is [plan/M0.md](plan/M0.md). A milestone is done when its exit checks pass in CI on
Windows and Linux, and its manual checks pass on Windows 11, a wlroots compositor, KDE Plasma,
GNOME and X11. Feature lists are the FEATURES.md rows tagged with that milestone.

| # | Milestone | Scope | Exit checks |
| - | - | - | - |
| M0 | Skeleton | Workspace, domain types, M0 ports and fakes, dispatcher, minimal Task (capture → save → copy), single-instance IPC, tray, one hotkey, fullscreen capture | Task golden traces for the M0 job; dispatcher routing test; platform contract tests; CI guard; PNG budget; idle RSS measured (R6) |
| M1 | Capture core | Region session and overlay, every capture mode in FEATURES.md "Capture" except scrolling, auto and text capture; after-capture steps that don't upload; toasts; history; naming; settings with workflows and the hotkey editor | Region session scripted-input tests; Task traces for every after-capture flag in M1's scope (the steps that don't upload and don't need the editor, OCR, pinning or AI); overlay latency and capture-memory budgets |
| M2 | Upload core | Upload module with the M2 hosts in FEATURES.md; after-upload steps; quick task menu; actions toolbar; after-capture and before-upload windows; Actions; URL sharing; clipboard, drag-and-drop, text, URL and folder upload | Upload fixtures for each M2 host; `.sxcu` syntax tests covering all 13 functions; Task traces for upload failure, retry and cleanup |
| M3 | Editor | Editor engine with the 21 tools, undo and redo, canvas operations, the 60 classic image effects, presets and `.sxie` import, beautifier, annotating in the overlay | Editor snapshot tests per tool and effect; export matches view |
| M4 | Recording | Recording session; FFmpeg video and GIF; audio; pause, stop and abort; countdown; recording bar; fixed duration | Recording session tests with a fake stream; recording CPU budget; crash-survival test (kill FFmpeg midway) |
| M5 | Tools | Every row in FEATURES.md "Tools" tagged M5, plus the mouse highlighter | Pure-tool tests; windowed-tool smoke tests |
| M6 | Integration | Scrolling capture, auto capture, Watch folders, the ShareX command-line arguments, native messaging, ShareX importer, sandbox mode, localization, autostart, shell integration (Explorer context menu and Send To; Linux `.desktop` actions), auto-update | Importer fixtures from real ShareX configs; CLI routing tests |
| M7 | Remaining features | Photo editor; the newer effects beyond the 60; background removal; AI image analysis; camera overlay; remote storage browser; native Windows recorder; the M7 hosts in FEATURES.md | Per the M7 plan |

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
| [008](adr/008-option-patches.md) | Workflow overrides as `Option` patches | ShareX's `UseDefault*` booleans |
| [009](adr/009-command-undo.md) | Undo by commands | full-image mementos |
| [010](adr/010-dead-services.md) | Dead services are not ported | porting dead endpoints |
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

---

## 10. Risks

| # | Risk | Mitigation |
| - | - | - |
| R1 | GNOME (Mutter) has no layer-shell, so the overlay cannot use `LayerShell` there | Fullscreen window per monitor; latency measured but not budgeted on GNOME (ADR 019) |
| R2 | The GlobalShortcuts portal is missing (for example on `xdg-desktop-portal-wlr`), so hotkeys don't work | `laqta --action <name>` over IPC, bound in compositor settings; settings page shows the command (ADR 019) |
| R3 | The GPUI snapshot pin, `LayerShell` and the overlay latency budget depend on one external project | Pin exactly; upgrade only together with `gpui-component`; overlay latency bench in CI from M1 |
| R4 | FFmpeg is missing, crashes or is tampered with | Pinned-hash download; crash-surviving container; failure event keeps the partial file |
| R5 | A panic in a decoder, kernel or host could end the resident process | `catch_unwind` per job and tool run, with panics logged (ADR 020) |
| R6 | GPUI may hold a GPU device and fonts with no windows, pushing idle RSS over 40 MB | Measured in M0. If over budget, an ADR chooses between raising the budget and running the UI as a separate on-demand process. Tray, hotkeys and IPC already run without GPUI |
| R7 | Retrying an upload duplicates it on hosts that don't deduplicate | Retry only on connection errors or timeouts before any response, at most twice, with backoff |
| R8 | On GNOME Wayland, setting the clipboard may fail without a focused window | M0 manual check. If it fails, the toast offers a Copy button, and clicking it gives the app focus |
| R9 | Layering drift: GPUI types leak into the core, views grow rules, or Task gains a public step interface | CI guard (§3); region rules stay in `domain::region`, tool rules in `laqta-app`'s `tools` module, and annotation rules in the Editor engine; new after-capture behaviour is a flag inside Task plus a golden scenario |

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
