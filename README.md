<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/logo-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/readme/logo-light.png">
    <img alt="Laqta" src="assets/readme/logo-light.png" height="72">
  </picture>
</h1>

<h3 align="center">
  Capture, record, mark up and share your screen. Free and open source.
</h3>

<p align="center">
  <a href="#-get-started">Get started</a> •
  <a href="#-features">Features</a> •
  <a href="#-screenshots">Screenshots</a> •
  <a href="#%EF%B8%8F-build-from-source">Build</a> •
  <a href="docs/ARCHITECTURE.md">Architecture</a>
</p>

<p align="center">
  <img alt="Windows" src="https://img.shields.io/badge/Windows-10%20%7C%2011-3D8BFD?style=flat-square">
  <img alt="Linux" src="https://img.shields.io/badge/Linux-X11%20%7C%20Wayland-3D8BFD?style=flat-square">
  <img alt="Rust" src="https://img.shields.io/badge/built%20with-Rust-1E2A78?style=flat-square">
  <img alt="License" src="https://img.shields.io/badge/license-GPL--3.0-FF6B5A?style=flat-square">
</p>

<p align="center">
  <img alt="Laqta's main window: capture modes, quick actions and the capture library" src="assets/readme/cover-light.webp" width="900">
</p>

**Laqta** (Arabic لقطة, *"a snapshot"*) takes screenshots and screen recordings, lets you mark
them up, and uploads them wherever you want, starting from one shortcut. It's written in Rust with
a GPU-drawn interface, sits in the tray until you need it, and works on Windows and on Linux
(X11 and Wayland).

Coming from **ShareX**? Laqta is built to replace it, and it brings your setup along: settings,
hotkeys, workflows, custom uploaders (`.sxcu`), image effects (`.sxie`), backups (`.sxb`) and
history. The command-line options you already use work too.

## ⚡ Get started

Download the latest build for your system:

<table>
  <tr><td><b>Platform</b></td><td><b>Download</b></td></tr>
  <tr><td><b>Windows installer (x64)</b></td><td><a href="../../releases/latest/download/laqta-windows-x64-setup.exe">laqta-windows-x64-setup.exe</a></td></tr>
  <tr><td><b>Windows portable (x64)</b></td><td><a href="../../releases/latest/download/laqta-windows-x64-portable.zip">laqta-windows-x64-portable.zip</a></td></tr>
  <tr><td><b>Linux AppImage (x86_64)</b></td><td><a href="../../releases/latest/download/laqta-linux-x86_64.AppImage">laqta-linux-x86_64.AppImage</a></td></tr>
  <tr><td><b>Debian / Ubuntu (amd64)</b></td><td><a href="../../releases/latest/download/laqta-linux-amd64.deb">laqta-linux-amd64.deb</a></td></tr>
  <tr><td><b>Linux archive (x86_64)</b></td><td><a href="../../releases/latest/download/laqta-linux-x86_64.tar.gz">laqta-linux-x86_64.tar.gz</a></td></tr>
</table>

Every file has a `.sha256` next to it on the [Releases](../../releases) page.

- **Windows:** the installer sets Laqta up for your account, adds Start menu and desktop
  shortcuts, and keeps it updated. The portable zip runs from any folder and keeps its settings
  next to `laqta.exe`.
- **Linux:** make the AppImage executable (`chmod +x laqta-linux-x86_64.AppImage`) and run it. On
  Debian or Ubuntu you can instead install the package with
  `sudo apt install ./laqta-linux-amd64.deb`. The archive holds the plain binaries, or you can
  [build from source](#%EF%B8%8F-build-from-source).

Laqta lives in the tray. Press <kbd>Ctrl</kbd>+<kbd>PrtSc</kbd> to capture a region,
<kbd>PrtSc</kbd> for the whole screen, or set your own shortcuts in **Settings → Shortcuts**.

## ⭐ Features

### 📸 Capture
* **Every mode:** region, window, monitor, full screen, last region, scrolling capture, and auto
  capture on a timer.
* **Smart selection:** the overlay snaps to windows *and to the parts inside them* (sidebars,
  panels, text boxes), found from the screenshot itself and from the accessibility tree. Scroll to
  grow the selection to the enclosing part.
* **Act on the selection:** a toolbar next to it copies, saves, pins, uploads, edits, or reads the
  text out of it. You can also draw on the selection before you capture it.
* Magnifier, crosshair, pixel info and an optional mode bar for screenshot or video, colour
  picker and text capture.

### 🎥 Record
* Video or GIF of a region, a window or the whole screen, with system audio and microphone.
* FFmpeg encoders: x264, x265, VP9, AV1, NVENC, AMF, QSV and VAAPI, plus GIF, WebP and APNG.
* Pause, timer, fixed length, camera overlay and a mouse highlighter.
* Native screen capture on Wayland through PipeWire, including KDE window recording.

### 🖌️ Edit
* An annotation editor with shapes, arrows, text, callouts, numbered steps, highlighter, blur,
  pixelate, magnifier, emoji and crop, with full undo and redo.
* About **300 image effects**, effect presets and an image beautifier.
* A layered photo editor with styles, masks and blend modes.

### ✨ AI tools
* **Text recognition (OCR)** for a region or an image. It runs offline.
* **Background removal** and an **object eraser** with local ONNX models, on the GPU where
  available (DirectML on Windows, CUDA on Linux).
* **Image analysis** through any OpenAI-compatible provider, such as OpenAI, Gemini or
  OpenRouter, or a local Ollama or LM Studio server.

### ☁️ Share
* Imgur, Dropbox, Google Drive, OneDrive, Amazon S3 and S3-compatible storage, Azure Blob, Google
  Cloud Storage, Backblaze B2, FTP, FTPS, SFTP and dozens more.
* Custom uploaders with the complete `.sxcu` format, URL shorteners and sharing services.
* A remote storage browser for S3, FTP and SFTP.

### ⚙️ Everything else
* **Workflows:** chain after-capture and after-upload steps, per hotkey.
* Pin to screen, colour picker, ruler, QR codes, image and video converters, hash checker,
  metadata stripper and more.
* Searchable history with tags, about 30 languages, light and dark themes, portable and sandbox
  modes, and auto-update.

## 🖼️ Screenshots

<table>
  <tr>
    <td width="50%"><img alt="A region selected in the capture overlay, with its size and the capture toolbar below it" src="assets/readme/capture-toolbar.webp"></td>
    <td width="50%"><img alt="Annotating the selection in place: a rectangle, an arrow and numbered steps" src="assets/readme/annotate-in-place.webp"></td>
  </tr>
  <tr>
    <td align="center"><b>Select</b>: snap, adjust, then pick what happens next</td>
    <td align="center"><b>Annotate in place</b>: mark it up before you capture</td>
  </tr>
  <tr>
    <td width="50%"><img alt="The image editor with the annotation tools, colour palette and AI tools menu" src="assets/readme/editor.webp"></td>
    <td width="50%">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="assets/readme/shortcuts-dark.webp">
        <img alt="Settings: the Shortcuts page with global shortcuts and their keys" src="assets/readme/shortcuts-light.webp">
      </picture>
    </td>
  </tr>
  <tr>
    <td align="center"><b>Editor</b>: annotations stay editable until you export</td>
    <td align="center"><b>Shortcuts</b>: global keys that work in every app</td>
  </tr>
  <tr>
    <td width="50%"><img alt="Region detection: hovering a card inside a window highlights just that card, with its size" src="assets/readme/region-detection.webp"></td>
    <td width="50%"><img alt="The colour picker: the picked colour, its formats, and tints and shades" src="assets/readme/color-picker.webp"></td>
  </tr>
  <tr>
    <td align="center"><b>Region detection</b>: hover any part of a window, scroll to grow</td>
    <td align="center"><b>Colour picker</b>: pick from screen, copy in any format</td>
  </tr>
</table>

## 🛠️ Build from source

You need the Rust toolchain from [rustup](https://rustup.rs); the version is pinned in
`rust-toolchain.toml`.

#### Linux
Debian and Ubuntu package names:

```bash
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
  libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev libdbus-1-dev \
  libpipewire-0.3-dev libclang-dev pkg-config
cargo build --release -p laqta
./target/release/laqta
```

#### Windows
With the MSVC build tools and [vcpkg](https://vcpkg.io):

```powershell
vcpkg install "sqlite3[fts5]:x64-windows-static-md"
$env:VCPKG_ROOT = "<your vcpkg folder>"
cargo build --release -p laqta
.\target\release\laqta.exe
```

Laqta downloads FFmpeg the first time you record, or you can point it at your own copy in
Settings. The optional local models download from the model manager when you first need them.

#### Scripting
Running `laqta` again passes its arguments to the copy that's already running, so you can drive
it from scripts:

```bash
laqta --action rectangle-region      # region capture
laqta --action screen-recorder       # start recording
laqta screenshot.png                 # upload a file
```

## 🧭 Project layout

The rules for capturing, editing, recording and uploading live in a core that does no I/O.
Platform and service code plugs into it through small interfaces (ports).

| Part | Crates |
| --- | --- |
| Pure core | `laqta-domain`, `laqta-imaging`, `laqta-editor`, `laqta-photo` |
| Application | `laqta-app` (use cases, the task pipeline, the region session) and `laqta-ports` |
| Adapters | `laqta-platform-linux`, `laqta-platform-windows`, `laqta-media`, `laqta-upload`, `laqta-cloud`, `laqta-library`, `laqta-ml`, … |
| Interface | `laqta-ui` (GPUI) and `apps/laqta`, which wires it all together |

Read more in [Architecture](docs/ARCHITECTURE.md), the [feature list](docs/FEATURES.md), the
[glossary](docs/GLOSSARY.md) and the [decision records](docs/adr).

## 🤝 Contributing

Issues and pull requests are welcome. Before opening a pull request, run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo xtask check-deps
```

## 💙 Acknowledgements

* [**ShareX**](https://github.com/ShareX/ShareX), which set the bar for what a capture tool can do.
  Laqta's translations are converted from ShareX's (GPL-3.0), and its importer reads ShareX's
  file formats.
* [**Unsloth Studio**](https://github.com/unslothai/unsloth), whose interface inspired Laqta's look
  and feel.
* [**GPUI**](https://www.gpui.rs) from Zed and [**GPUI Component**](https://github.com/longbridge/gpui-component)
  (included under `vendor/`, Apache-2.0), which the interface is built on.
* [**ort**](https://github.com/pykeio/ort) by pyke, the Rust bindings for
  [ONNX Runtime](https://onnxruntime.ai) that run Laqta's local models.
* [FFmpeg](https://ffmpeg.org) and the many open-source crates in `Cargo.lock`.

## 📄 License

Laqta is free software under the [GNU General Public License v3.0 or later](LICENSE).
