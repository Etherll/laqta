# Features

This page lists what Laqta does, the roadmap phase that delivers each feature, and its status.
Status values are `planned`, `wip`, `done` and `dropped` (with a reason). The milestone column
refers to the phases in [ARCHITECTURE.md §8](ARCHITECTURE.md#8-roadmap).

**How coverage is counted.** Each table row is one unit. Coverage is the number of rows marked
`done` divided by the number of rows not marked `dropped`. The target is 99% (ARCHITECTURE.md
§1.2). A row is `dropped` only when its service has shut down (ADR 010) or the feature is out of
scope.

## Capture

| Feature | Milestone | Status |
| - | - | - |
| Fullscreen, monitor, active monitor | Skeleton to Capture core | done |
| Window, active window, custom window | Capture core | done |
| Region, custom region, last region | Capture core | done |
| Capture delay, cursor, transparent window (DWM), HDR | Capture core | done |
| Overlay: window and UI-element detection, multi-region, dim, magnifier, crosshairs, info text, toolbar, mouse-button actions, keyboard nudge | Capture core | done |
| Scrolling capture (wheel, arrow, page down, scroll message; stitching) | Integration | done |
| Auto capture (start / stop / interval) | Integration | done |
| Text capture (OCR): the text is read in place on the frozen screen, words are picked with a drag and copied (ADR 031) | Tools | done |

## Recording

| Feature | Milestone | Status |
| - | - | - |
| Video: region, active window, custom region, last region | Recording | done |
| GIF: same start modes | Recording | done |
| Codecs: x264, x265, VP8, VP9, Xvid, NVENC, AMF, QSV, GIF, WebP, APNG, plus VAAPI and AV1 | Recording | done |
| Audio: system loopback, microphone, gain, mix | Recording | done |
| Pause, stop, abort, start delay, fixed duration, timer, recording bar, border | Recording | done |
| Custom FFmpeg args, custom command line, FFmpeg path override and download | Recording | done |
| Camera overlay (shape, position, size) | Remaining features | done |
| Native GPU path (WGC + Media Foundation) | Remaining features | done |
| Mouse highlighter (while recording and as a tool) | Tools | done |

## Workflow and tasks

| Feature | Milestone | Status |
| - | - | - |
| 23 after-capture tasks (listed below) | Capture core to Remaining features: each flag ships with the feature it needs (Capture core: local steps; Upload core: upload and windows; Editor: editor and effects; Tools: OCR, QR and pin; Remaining features: AI and photo editor) | done |
| 6 after-upload tasks (listed below) | Upload core | done |
| Per-workflow overrides of every settings section | Capture core | done |
| Destinations per type (image, text, file, shortener, sharing; FTP and custom-uploader index) | Upload core | done |
| Task queue, status machine, progress, stop, stop all | Capture core to Upload core | done |
| Quick task menu, actions toolbar, after-capture window, before-upload window | Upload core | done |
| Actions (run external program with arguments) | Upload core | done |
| Image format options (PNG / JPEG / GIF / WebP / BMP, quality, auto JPEG by size, file-exists action, thumbnail) | Capture core | done |
| Toast options (duration, fade, placement, size, click actions), per-event sounds | Capture core | done |
| Name patterns (37 tokens) and upload-info templates (13 tokens; HTML, forum, wiki, Markdown), listed below | Capture core / Upload core | done |
| Watch folders | Integration | done |
| Clipboard, drag-and-drop, text, URL (download then reupload), folder upload | Upload core | done |

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

**Upload-info tokens (13):** `$url`, `$shorturl`, `$thumbnailurl`, `$deletionurl`, `$result`, `$filename`, `$filenamenoext`, `$filepath`, `$folderpath`, `$foldername`, `$thumbnailfilename`, `$thumbnailfilenamenoext`, `$uploadtime`.

**Region info-text tokens:** `$x`, `$y`, `$r`, `$g`, `$b`, `$hex`, `$HEX`, `$n`.

## Upload destinations

Hosts in the catalog are `.sxcu` files (ADR 004). Hosts written in Rust are marked *native*.

| Service | Approach | Milestone | Status |
| - | - | - | - |
| **Native:** Imgur, Dropbox, Google Drive, OneDrive, Amazon S3 and S3-compatible, FTP, FTPS, SFTP, shared folder, Bitly | native | Upload core | done |
| **Catalog, image and text:** Chevereto, vgy.me, ImageBin, Img1, Pastebin, Paste.ee, Hastebin, OneTimeSecret, Pastie, uPaste, Slexy, Paste2 | `.sxcu` | Upload core | done |
| **Catalog, file:** Pomf clones, Uguu, transfer.sh, FileBin, Plik, Lambda, s-ul, img.fish, Puush, Immich, Seafile, Streamable | `.sxcu` | Upload core | done |
| **Catalog, shorteners:** YOURLS, Polr, Kutt, Zero Width, is.gd, v.gd, TinyURL, TURL, 2.gp, qr.net, vurl, nlcm | `.sxcu` | Upload core | done |
| **URL sharing:** Email, Pushbullet, Facebook, Reddit, Pinterest, Tumblr, LinkedIn, VK, Google Lens, Bing Visual Search | URL templates (email opens the user's mail client; SMTP email is a native host below) | Upload core | done |
| Custom uploaders: `.sxcu` with all body types and all 13 syntax functions, old `$…$` syntax migration | `laqta-upload` | Upload core | done |
| **Native:** Flickr (OAuth1), Box, YouTube, Google Cloud Storage, Azure Blob, Backblaze B2, GitHub Gist, PrivateBin (client-side crypto), SMTP email, Pushbullet (file), Nextcloud / ownCloud, SendSpace, MediaFire | native | Remaining features | done |
| Remote storage browser: S3, FTP, SFTP (list, upload, download, rename, delete) | native | Remaining features | done |
| **Dropped:** Copy, Drop.io, FileSonic, Photobucket, ImageShack, Hostr, StumbleUpon, Delicious, Firebase Dynamic Links, Immio | — | — | dropped: the services have shut down (ADR 010) |
| **MEGA** | native | — | dropped: no maintained client to build on (ADR 010) |

## Editor and effects

| Feature | Milestone | Status |
| - | - | - |
| 21 annotation tools (Select, Rectangle, Ellipse, Line, Arrow, Freehand, Text, SpeechBalloon, Step, Image, Clone, Emoji, Cursor, Highlight, SmartEraser, Blur, Pixelate, Magnify, Spotlight, Crop, CutOut) | Editor | done |
| Undo / redo, clipboard, nudge, toolbar customization and hotkeys | Editor | done |
| Annotate inside the region overlay | Editor | done |
| The 60 image effects (15 adjustments, 8 drawings, 27 filters, 10 manipulations) | Editor | done |
| About 240 more image effects (adjustments, drawings, filters, borders, film, glitch) | Remaining features | done |
| Effect presets, `.sxie` packages, random effect | Editor | done |
| Image beautifier | Editor | done |
| Photo editor: layers (raster, shape, text, adjustment, fill, frame, group, smart object), styles, blend modes, masks, selection tools, brushes, retouch, inpainting | Remaining features | done |

## Tools

| Tool | Milestone | Status |
| - | - | - |
| Color picker, screen color picker, ruler | Tools | done |
| Pin to screen (from screen, clipboard, file; close all) | Tools | done |
| Image viewer, image comparer, icon converter | Tools | done |
| Image combiner, splitter, resizer, converter, watermark, thumbnailer | Tools | done |
| Video converter, trimmer, thumbnailer; animated GIF maker and trimmer | Tools | done |
| OCR, QR generate, QR decode from screen, QR scan region | Tools | done |
| Hash checker, directory indexer (HTML, JSON, text, XML), clipboard viewer | Tools | done |
| Metadata viewer and stripper (EXIF, ISO BMFF, Matroska, ASF, RIFF) | Tools | done |
| Borderless window, active window borderless / topmost, inspect window | Tools (Windows and X11; Wayland where the compositor allows) | done |
| Monitor test, network monitor | Tools | done |
| Background remover: local model families (BiRefNet, BEN2 and others, ADR 023), custom models, and a remove.bg cloud option | Remaining features | done |
| Model manager: per-task model choice, downloads with progress, custom ONNX models of a supported family, CPU execution | Tools (OCR) / Remaining features (background removal) | done |
| GPU execution for local models (DirectML on Windows, CUDA on Linux): GPU ONNX Runtime builds and the `ort` provider features | Remaining features | planned: the GPU runtime packages are not yet chosen or verified |
| Analyze image (OpenAI, Gemini, OpenRouter) | Remaining features | done |

## Application

| Feature | Milestone | Status |
| - | - | - |
| About 80 hotkey actions, defaults on PrintScreen combos, disable hotkeys | Capture core | done |
| Tray icon and menu, main window, toggle tray menu | Skeleton to Capture core | done |
| History (list, image grid, stats, tags) and import of ShareX's History.json | Capture core | done |
| Settings files, rotating and weekly backups, export / import archive, portable mode | Capture core | done |
| Sandbox mode | Integration | done |
| Import from an existing ShareX install | Integration | done |
| CLI (`task`, `workflow`, custom uploader, image effect, native-message input). Accepts the ShareX-style flags `-silent`, `-portable`, `-multi` and `-NoHotkeys` | Integration | done |
| Single instance and argument forwarding | Skeleton | done |
| Browser extension native messaging (Chrome, Firefox) | Integration | done |
| Run at startup (on by default for a new install), shell context menu, Send To, file-type registration | Integration | done |
| About 30 languages | Integration | done |
| Auto-update | Integration | done |
| Steam build | — | dropped: no Steam build is planned |
