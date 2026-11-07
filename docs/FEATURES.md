# Features

Status values: `planned` · `wip` · `done` · `dropped` (with a reason).
The milestone column refers to [ARCHITECTURE.md §8](ARCHITECTURE.md#8-roadmap).

**How parity is counted.** Each table row is one unit. Parity is the number of rows marked `done` divided by the number of rows not marked `dropped`. The target is 99% (ARCHITECTURE.md §1.2). A row is `dropped` only when its service has shut down (ADR 010) or the user decides it is out of scope.

Areas the survey did not cover in depth need a second pass:
- **Before M3:** the per-annotation property lists, the parameters of each ImageEditor effect, and
  the full editor keyboard map.
- **Before M6:** the full CLI argument list, the native-messaging message format, and Steam
  integration.

## Capture

| Feature | Milestone | Status |
| - | - | - |
| Fullscreen, monitor, active monitor | M0–M1 | done |
| Window, active window, custom window | M1 | done |
| Region, custom region, last region | M1 | done |
| Capture delay, cursor, transparent window (DWM), HDR | M1 | done |
| Overlay: window and UI-element detection, multi-region, dim, magnifier, crosshairs, info text, toolbar, mouse-button actions, keyboard nudge | M1 | done |
| Scrolling capture (wheel, arrow, page down, scroll message; stitching) | M6 | done |
| Auto capture (start / stop / interval) | M6 | done |
| Text capture (OCR) | M5 | done |

## Recording

| Feature | Milestone | Status |
| - | - | - |
| Video: region, active window, custom region, last region | M4 | done |
| GIF: same start modes | M4 | done |
| Codecs: x264, x265, VP8, VP9, Xvid, NVENC, AMF, QSV, GIF, WebP, APNG (+ VAAPI, AV1 new) | M4 | done |
| Audio: system loopback, microphone, gain, mix | M4 | done |
| Pause, stop, abort, start delay, fixed duration, timer, recording bar, border | M4 | done |
| Custom FFmpeg args, custom command line, FFmpeg path override and download | M4 | done |
| Camera overlay (shape, position, size) | M7 | done |
| Native GPU path (WGC + Media Foundation) | M7 | done |
| Mouse highlighter (while recording and as a tool) | M5 | done |

## Workflow and tasks

| Feature | Milestone | Status |
| - | - | - |
| 23 after-capture tasks (listed below) | M1–M7: each flag ships with the feature it needs (M1 local steps, M2 upload and windows, M3 editor and effects, M5 OCR, QR and pin, M7 AI and photo editor) | done |
| 6 after-upload tasks (listed below) | M2 | done |
| Per-workflow overrides of every settings section | M1 | done |
| Destinations per type (image, text, file, shortener, sharing; FTP and custom-uploader index) | M2 | done |
| Task queue, status machine, progress, stop, stop all | M1–M2 | done |
| Quick task menu, actions toolbar, after-capture window, before-upload window | M2 | done |
| Actions (run external program with arguments) | M2 | done |
| Image format options (PNG / JPEG / GIF / WebP / BMP, quality, auto JPEG by size, file-exists action, thumbnail) | M1 | done |
| Toast options (duration, fade, placement, size, click actions), per-event sounds | M1 | done |
| Name patterns (37 tokens) and upload-info templates (13 tokens; HTML, forum, wiki, Markdown), listed below | M1 / M2 | done |
| Watch folders | M6 | done |
| Clipboard, drag-and-drop, text, URL (download then reupload), folder upload | M2 | done |

**After-capture tasks (23):** ShowQuickTaskMenu, ShowAfterCaptureWindow, BeautifyImage, AddImageEffects, AnnotateImage, OpenInPhotoEditor, CopyImageToClipboard, PinToScreen, SendImageToPrinter, SaveImageToFile, SaveImageToFileWithDialog, SaveThumbnailImageToFile, PerformActions, CopyFileToClipboard, CopyFilePathToClipboard, CopyFolderPathToClipboard, ShowInExplorer, AnalyzeImage, ScanQRCode, DoOCR, ShowBeforeUploadWindow, UploadImageToHost, DeleteFile.

**After-upload tasks (6):** ShowAfterUploadWindow, UseURLShortener, ShareURL, CopyURLToClipboard, OpenURL, ShowQRCode.

**Name-pattern tokens (37, prefix `%`):**
- Window: `t`, `pn`.
- Date and time: `y`, `yy`, `mo`, `mon`, `mon2`, `w`, `w2`, `wy`, `d`, `h`, `mi`, `s`, `ms`, `pm`, `unix`.
- Counters: `i`, `ia`, `iAa`, `ib`, `ix`.
- Random: `rn`, `ra`, `rna`, `rx`, `guid`, `radjective`, `ranimal`, `remoji`, `rf`.
- Image: `width`, `height`.
- Computer: `un`, `uln`, `cn`.
- Other: `n` (newline).

These tokens are compatible with ShareX's name patterns.

**Upload-info tokens (13):** `$url`, `$shorturl`, `$thumbnailurl`, `$deletionurl`, `$result`, `$filename`, `$filenamenoext`, `$filepath`, `$folderpath`, `$foldername`, `$thumbnailfilename`, `$thumbnailfilenamenoext`, `$uploadtime`.

**Region info-text tokens:** `$x`, `$y`, `$r`, `$g`, `$b`, `$hex`, `$HEX`, `$n`.

## Upload destinations

Hosts in the catalog are `.sxcu` files (ADR 004); hosts written in Rust are marked *native*.

| Service | Approach | Milestone | Status |
| - | - | - | - |
| **M2 native:** Imgur, Dropbox, Google Drive, OneDrive, Amazon S3 and S3-compatible, FTP, FTPS, SFTP, shared folder, Bitly | native | M2 | done |
| **M2 catalog, image and text:** Chevereto, vgy.me, ImageBin, Img1, Pastebin, Paste.ee, Hastebin, OneTimeSecret, Pastie, uPaste, Slexy, Paste2 | `.sxcu` | M2 | done |
| **M2 catalog, file:** Pomf clones, Uguu, transfer.sh, FileBin, Plik, Lambda, s-ul, img.fish, Puush, Immich, Seafile, Streamable | `.sxcu` | M2 | done |
| **M2 catalog, shorteners:** YOURLS, Polr, Kutt, Zero Width, is.gd, v.gd, TinyURL, TURL, 2.gp, qr.net, vurl, nlcm | `.sxcu` | M2 | done |
| **M2 URL sharing:** Email, Pushbullet, Facebook, Reddit, Pinterest, Tumblr, LinkedIn, VK, Google Lens, Bing Visual Search | URL templates (email via the user's mail client until SMTP lands in M7) | M2 | done |
| Custom uploaders: `.sxcu` with all body types and all 13 syntax functions, old `$…$` syntax migration | `laqta-upload` | M2 | done |
| **M7 native:** Flickr (OAuth1), Box, YouTube, Google Cloud Storage, Azure Blob, Backblaze B2, MEGA (deferred: kept only if a maintained option exists when the M7 plan is written), GitHub Gist, PrivateBin (client-side crypto), SMTP email, Pushbullet (file), Nextcloud / ownCloud, SendSpace, MediaFire | native | M7 | done |
| Remote storage browser: S3, FTP, SFTP (list, upload, download, rename, delete) | native | M7 | done |
| Dropped: Copy, Drop.io, FileSonic, Photobucket, ImageShack, Hostr, StumbleUpon, Delicious, Firebase Dynamic Links, Immio | services shut down; re-checked when M7 starts | — | pending (owner): services shut down; re-add if one returns |

## Editor and effects

| Feature | Milestone | Status |
| - | - | - |
| 21 annotation tools (Select, Rectangle, Ellipse, Line, Arrow, Freehand, Text, SpeechBalloon, Step, Image, Clone, Emoji, Cursor, Highlight, SmartEraser, Blur, Pixelate, Magnify, Spotlight, Crop, CutOut) | M3 | done |
| Undo / redo, clipboard, nudge, toolbar customization and hotkeys | M3 | done |
| Annotate inside the region overlay | M3 | done |
| The 60 classic image effects (15 adjustments, 8 drawings, 27 filters, 10 manipulations) | M3 | done |
| About 240 newer effects (adjustments, drawings, filters, borders, film, glitch) | M7 | done |
| Effect presets, `.sxie` packages, random effect | M3 | done |
| Image beautifier | M3 | done |
| Photo editor: layers (raster, shape, text, adjustment, fill, frame, group, smart object), styles, blend modes, masks, selection tools, brushes, retouch, inpainting | M7 | done |

## Tools

| Tool | Milestone | Status |
| - | - | - |
| Color picker, screen color picker, ruler | M5 | done |
| Pin to screen (from screen, clipboard, file; close all) | M5 | done |
| Image viewer, image comparer, icon converter | M5 | done |
| Image combiner, splitter, resizer, converter, watermark, thumbnailer | M5 | done |
| Video converter, trimmer, thumbnailer; animated GIF maker and trimmer | M5 | done |
| OCR, QR generate, QR decode from screen, QR scan region | M5 | done |
| Hash checker, directory indexer (HTML, JSON, text, XML), clipboard viewer | M5 | done |
| Metadata viewer and stripper (EXIF, ISO BMFF, Matroska, ASF, RIFF) | M5 | done |
| Borderless window, active window borderless / topmost, inspect window | M5 (Windows and X11; Wayland where the compositor allows) | done |
| Monitor test, network monitor | M5 | done |
| Background remover: local model families (BiRefNet, BEN2 and others, ADR 023), custom models, and a remove.bg cloud option | M7 | done |
| Model manager: per-task model choice, downloads with progress, custom ONNX models of a supported family, CPU execution | M5 (OCR) / M7 (background removal) | done |
| GPU execution for local models (DirectML on Windows, CUDA on Linux): GPU ONNX Runtime builds and the `ort` provider features | M7 | pending (owner): needs the GPU runtime packages chosen and verified on the owner's GPUs |
| Analyze image (OpenAI, Gemini, OpenRouter) | M7 | done |

## Application

| Feature | Milestone | Status |
| - | - | - |
| About 80 hotkey actions, defaults on PrintScreen combos, disable hotkeys | M1 | done |
| Tray icon and menu, main window, toggle tray menu | M0–M1 | done |
| History (list, image grid, stats, tags), legacy History.json migration | M1 | done |
| Settings files, rotating and weekly backups, export / import archive, portable mode | M1 | done |
| Sandbox mode | M6 | done |
| Import from an existing ShareX install | M6 | done |
| CLI (`task`, `workflow`, custom uploader, image effect, native-message input; `-silent`, `-portable`, `-multi`, `-NoHotkeys`) | M6 | done |
| Single instance and argument forwarding | M0 | done |
| Browser extension native messaging (Chrome, Firefox) | M6 | done |
| Run at startup, shell context menu, Send To, file-type registration | M6 | done |
| About 30 languages | M6 | done |
| Auto-update | M6 | done |
| Steam build | dropped unless requested | pending (owner): dropped unless the owner asks for a Steam build |
