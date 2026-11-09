# Glossary

Domain terms used in Laqta's code and docs. Architecture terms (module, interface, depth, seam,
adapter, leverage, locality) follow the codebase-design vocabulary and are not repeated here.

| Term | Meaning |
| - | - |
| **Action** | One thing the user can trigger: a variant of the `Action` enum, one per ShareX hotkey action plus a few of Laqta's own (about 80 in all). Hotkeys, the tray, the CLI and watch folders all produce Actions |
| **Action dispatcher** | The single entry point that turns an Action plus an optional Workflow into work for Task, the Region session, the Recording session, Watch folders, a pure tool or a windowed tool. A mechanism that already holds the action's input (a dropped file, a URL, a watched file) adds an `ActionInput` through `dispatch_with` |
| **Pure core** | The crates that do no I/O, start no threads of their own and make no OS calls: `laqta-domain`, `laqta-imaging`, `laqta-editor` and `laqta-photo` |
| **Pure tool** | A tool that only decodes, runs `laqta-imaging` and encodes (hash, resize, convert, QR, …). The dispatcher calls it directly |
| **Windowed tool** | A tool with its own window (colour picker, ruler, pin to screen, OCR, video tools, …). It opens through the `ToolHost` port; its rules live in `laqta-app`'s `tools` module |
| **OsScreenRef** | A reference to an OS screen (a Windows output, or an X11 display and screen) carried by a recording `Target`. Created by the platform adapter, passed through the application unread, and read by `laqta-media` and the Windows native recorder |
| **Lazy resource** | A heavy dependency started on first use and released by its owner's one-shot release timer: the HTTP runtime, the compute pool, the SQLite connection (opened at start, then released when idle), onnxruntime and models, the FFmpeg process, the font database. The HTTP runtime, the compute pool and the SQLite connection release today; the font database, onnxruntime and models do not yet |
| **Workflow** | A named Action plus a `TaskSettingsPatch`. ShareX calls these hotkey task settings |
| **Effective settings** | Default `TaskSettings` with a Workflow's patch applied |
| **Task** | One unit of work from input to result: after-capture steps, upload, after-upload steps and cleanup. Also the module that owns this lifecycle. (`laqta_ml::catalog::Task` is an unrelated enum of model tasks) |
| **TaskJob** | What a Task starts from: `Capture` (a captured image) or `Upload` of an `UploadSource` (text, a file, a URL to download and reupload, a URL to shorten, or a file already saved). A folder has no job of its own: the dispatcher submits one job per file |
| **TaskEvent** | A status, progress, result or error update published by Task. The tray, task list, toasts and history writer subscribe to them |
| **After-capture flags** | `AfterCaptureTasks` and `AfterUploadTasks`: the bit sets in a Workflow's task settings that say what Task does with a capture and with its upload. New after-capture behaviour is a flag handled inside Task |
| **Step** | One after-capture or after-upload operation (save, copy, upload, shorten, …). Private to Task. (`presenters::editor_interaction::Step` is an unrelated UI type) |
| **Frozen frame** | The screen image captured before the overlay opens: a `Frame` per monitor, composed into one desktop image. Selection and cropping work on it, so nothing is captured twice |
| **Bitmap**, **PhysRect** | `Bitmap` is an image (straight-alpha RGBA or BGRA pixels shared through `Arc`); `PhysRect` is a rectangle in physical desktop pixels. Both are in `laqta-domain` |
| **Region session** | The module that turns frozen frames into a Selection, shared by capture and recording. With the capture toolbar it keeps the region selected until the user picks an action |
| **Editor interaction** | The GPUI-free presenter between an editor window and its engine (`EditorInteraction`, `PhotoInteraction`): it applies input or queues edits, decides when the canvas must re-render, and coalesces renders through a **Render gate**. Renders run on a snapshot of the engine (`RenderJob`; `PhotoRenderJob` for the photo editor), never under its lock. The region overlay uses the same presenter on the selection. Not a "session": it has no lifecycle of its own |
| **Selection** | What a Region session returns: rects, the target window (title, process), the image (the plain crop, or the crop the overlay annotated), the `annotate` flag from the Annotate button, and the **SelectionAction** the user chose |
| **SelectionAction** | The toolbar button that ended a region session: `Default`, `Copy`, `Save`, `Pin`, `Upload`, `Edit` or `CopyText`. The dispatcher turns it into after-capture flags |
| **Capture toolbar** | The bar beside a selected region on the overlay: the editor engine's drawing tools, undo and redo, copy text, and the output actions. Space shows or hides it. Controlled by the `show_tools_after_select` setting |
| **Hover target** | A window or UI element the overlay can snap to |
| **HoverSource** | The port that supplies hover targets, loaded in the background |
| **Overlay** | The fullscreen surface that shows frozen frames and the selection UI. It forwards input and paints state |
| **Watch folders** | The module that watches configured folders and submits a Task for each new file, using that folder's workflow |
| **Recording session** | The module that owns a recording from region selection to finished file |
| **ScreenStream** | The port a recording reads from; it opens a **RecordingInput**: video is `Frames` (a frame source Laqta pumps) or `Target` (a screen target the recorder may capture itself), and audio is a list of `Device` (an audio device ID and name) or `Pcm` (a PCM source, such as Windows system audio). It uses no FFmpeg vocabulary except the user's custom arguments; `laqta-media` maps it to FFmpeg inputs |
| **Record spec** | The settings for one recording (`RecordSpec`): area, codec (the container follows from it), frame rate, quality (CRF or bitrate), cursor, audio devices, custom encoder arguments and an optional camera overlay |
| **Media port** | The port that records a ScreenStream to a file and transcodes files. Its first adapter is FFmpeg |
| **Editor engine** | The module behind every annotation surface: document, tools, undo, effect cache and rendering. The image editor window and the region overlay both host it |
| **AI use cases** | `App::recognize_text`, `remove_background`, `erase` and `analyze`: one port call each, with the error mapped to `AppError`. The editors and the library call them instead of the ports |
| **Chrome description** | What the Editor engine reports for the UI to paint around the image: handles, selection outline, active tool, toolbar state (`Chrome` in code). Unrelated to the `chrome` module in `laqta-ui`, which draws the window titlebar |
| **Destination** | Anywhere a Task can upload to: an image, text or file host. URL shorteners and sharing services are `Shortener` and `Sharer` |
| **Upload module** | The module that implements every Destination, Shortener and Sharer, including the custom-uploader engine |
| **Custom uploader** | A `.sxcu` file describing an HTTP request and how to read its response |
| **Catalog** | The `.sxcu` files bundled with Laqta for simple hosts |
| **Name pattern** | A file-name template with `%` tokens, such as `%y-%mo-%d_%rn` |
| **Upload-info template** | An output template with `$` tokens, such as `$url` or `$deletionurl` |
| **Preset** | A named list of image effects; `.sxie` is its package format |
| **Model family** | Laqta's implementation of one model architecture: input preparation, which outputs to read, and post-processing. Each lives in `laqta-ml`; `families/o1.rs` holds the two PP-OCR families (detection and recognition) |
| **Model catalog** | The list of concrete models that ships with each release: id, task, family and parameters, download URL, SHA-256, size, weights licence |
| **Custom model** | An ONNX file the user adds and assigns to a supported family. It is validated before use |
| **Execution provider** | Where ONNX Runtime runs a model: CPU, or a GPU provider such as DirectML. Auto falls back to CPU. No GPU provider is built in yet, so every setting runs on the CPU |
| **Ports crate** | `laqta-ports`: every port trait and the plain types it carries. Adapters depend on it instead of on `laqta-app` |
| **Ports** | The bundle of port adapters that `main` hands to `App::new` (`laqta_app::Ports`). Not the **Ports crate** |
| **Compute pool** | The `rayon` pool built by `main` on first use. Pure-core kernels run inside it through `Spawner::run_compute`; they never create a pool |
| **Codec set** | The `Codecs` value built by `main`: the pure-Rust codecs, plus any extension codecs (none exist yet). Passed to whoever encodes or decodes |
| **Verified fetch** | `laqta-http`'s download function: pinned SHA-256, progress, cancellation. Used for FFmpeg, onnxruntime, models and updates. An update's expected hash comes from its own release's `SHA256SUMS`, and the update download passes no progress and a cancel flag nobody sets |
| **Seam rule** | A port exists only with two production adapters (platforms count), or one plus a fake that a deep module's tests need (ADR 021) |
| **Unsupported** | The error a port returns when the current platform cannot do something |
