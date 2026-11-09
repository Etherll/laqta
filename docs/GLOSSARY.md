# Glossary

Domain terms used in Laqta's code and docs. Architecture terms (module, interface, depth, seam,
adapter, leverage, locality) follow the codebase-design vocabulary and are not repeated here.

| Term | Meaning |
| - | - |
| **Action** | One thing the user can trigger: a variant of the `Action` enum, one per ShareX hotkey action (about 80). Hotkeys, the tray, the CLI and watch folders all produce Actions |
| **Action dispatcher** | The single entry point that turns an Action plus an optional Workflow into work for Task, the Region session, the Recording session, Watch folders, a pure tool or a windowed tool |
| **Pure core** | The crates that do no I/O, start no threads of their own and make no OS calls: `laqta-domain`, `laqta-imaging`, `laqta-editor` (and `laqta-photo` in M7) |
| **Pure tool** | A tool that only decodes, runs `laqta-imaging` and encodes (hash, resize, convert, QR, …). The dispatcher calls it directly |
| **Windowed tool** | A tool with its own window (colour picker, ruler, pin to screen, OCR, video tools, …). It opens through the `ToolHost` port; its rules live in `laqta-app`'s `tools` module |
| **OsScreenRef** | A sealed reference to an OS screen (a Windows output, or an X11 display and screen) carried by a recording `Target`. Created by the platform adapter, read only by `laqta-media` |
| **Lazy resource** | A heavy dependency started on first use and released by its owner's one-shot release timer: the HTTP runtime, the SQLite connection, the compute pool, onnxruntime and models, the FFmpeg process, the font database |
| **Workflow** | A named Action plus a `TaskSettingsPatch`. ShareX calls these hotkey task settings |
| **Effective settings** | Default `TaskSettings` with a Workflow's patch applied |
| **Task** | One unit of work from input to result: after-capture steps, upload, after-upload steps and cleanup. Also the module that owns this lifecycle |
| **TaskJob** | What a Task starts from: a capture result, file, text, URL, folder, or URL to download and reupload |
| **TaskEvent** | A status, progress, result or error update published by Task. The tray, task list, toasts and history writer subscribe to them |
| **Step** | One after-capture or after-upload operation (save, copy, upload, shorten, …). Private to Task |
| **Frozen frame** | The screen image captured before the overlay opens. Selection and cropping work on it, so nothing is captured twice |
| **Region session** | The module that turns frozen frames into a Selection, shared by capture and recording |
| **Editor interaction** | The GPUI-free presenter between an editor window and its engine (`EditorInteraction`, `PhotoInteraction`): it applies input or queues edits, decides when the canvas must re-render, and coalesces renders through a **Render gate**. Renders run on a snapshot of the engine (`RenderJob`), never under its lock. Not a "session": it has no lifecycle of its own |
| **Selection** | What a Region session returns: rects, the target window (title, process), and the final image with any overlay annotations rendered into it |
| **Hover target** | A window or UI element the overlay can snap to |
| **HoverSource** | The port that supplies hover targets, loaded in the background |
| **Overlay** | The fullscreen surface that shows frozen frames and the selection UI. It forwards input and paints state |
| **Watch folders** | The module that watches configured folders and submits a Task for each new file, using that folder's workflow |
| **Recording session** | The module that owns a recording from region selection to finished file |
| **ScreenStream** | What a recording reads. Video is `Frames` (a frame source Laqta pumps) or `Target` (a screen target the recorder may capture itself). Audio inputs are `Device` (an audio device ID and name) or `Pcm` (a PCM source, such as Windows system audio). It uses no FFmpeg vocabulary; `laqta-media` maps it to FFmpeg inputs |
| **Record spec** | The settings for one recording: region, container, codec, frame rate, quality (CRF or bitrate), cursor, audio device IDs |
| **Media port** | The port that records a ScreenStream to a file and transcodes files. Its first adapter is FFmpeg |
| **Editor engine** | The module behind every annotation surface: document, tools, undo, effect cache and rendering |
| **Chrome description** | What the Editor engine reports for the UI to paint around the image: handles, selection outline, active tool, toolbar state |
| **Destination** | Anywhere a Task can upload to: an image, text or file host. URL shorteners and sharing services are `Shortener` and `Sharer` |
| **Upload module** | The module that implements every Destination, Shortener and Sharer, including the custom-uploader engine |
| **Custom uploader** | A `.sxcu` file describing an HTTP request and how to read its response |
| **Catalog** | The `.sxcu` files bundled with Laqta for simple hosts |
| **Name pattern** | A file-name template with `%` tokens, such as `%y-%mo-%d_%rn` |
| **Upload-info template** | An output template with `$` tokens, such as `$url` or `$deletionurl` |
| **Preset** | A named list of image effects; `.sxie` is its package format |
| **Model family** | Laqta's implementation of one model architecture: input preparation, which outputs to read, and post-processing. One file per family in `laqta-ml` |
| **Model catalog** | The list of concrete models that ships with each release: id, task, family and parameters, download URL, SHA-256, size, weights licence |
| **Custom model** | An ONNX file the user adds and assigns to a supported family. It is validated before use |
| **Execution provider** | Where ONNX Runtime runs a model: CPU, or a GPU provider such as DirectML. Auto falls back to CPU |
| **Ports crate** | `laqta-ports`: every port trait and the plain types it carries. Adapters depend on it instead of on `laqta-app` |
| **Compute pool** | The `rayon` pool built by `main` on first use. Pure-core kernels run inside it through `Spawner::run_compute`; they never create a pool |
| **Codec set** | The `Codecs` value built by `main`: the pure-Rust codecs plus optional C codecs from `laqta-codecs-c`. Passed to whoever encodes or decodes |
| **Verified fetch** | `laqta-http`'s download function: pinned SHA-256, progress, cancellation. Used for FFmpeg, onnxruntime, models and updates |
| **Seam rule** | A port exists only with two production adapters (platforms count), or one plus a fake that a deep module's tests need (ADR 021) |
| **Unsupported** | The error a port returns when the current platform cannot do something |
