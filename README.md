<p align="center">
  <img src="assets/icon/laqta-256.png" width="128" height="128" alt="Laqta icon">
</p>

<h1 align="center">Laqta</h1>

<p align="center">
  Fast, modern screen capture, recording, annotation and sharing for Windows and Linux.
</p>

---

Laqta (Arabic لقطة, "a snapshot") is a desktop tool for taking screenshots and screen
recordings, marking them up, and sending them where they need to go, all in one shortcut. It is
written in Rust with a GPU-accelerated [GPUI](https://www.gpui.rs) interface, starts in a blink,
and works the same on Windows and Linux, X11 and Wayland included.

If you are coming from ShareX, Laqta is built to replace it: it imports your ShareX settings,
hotkeys, workflows, custom uploaders (`.sxcu`), image effects (`.sxie`), backups (`.sxb`) and
history, and keeps the command-line options you already use.

## Features

**Capture**
- Region, window, monitor and full-screen capture, with the last or a custom region on a hotkey
- A selection overlay that snaps to windows and to the parts inside them (sidebars, panels, text
  boxes), found from the screenshot itself and from the accessibility tree; scroll to grow the
  selection to the enclosing part
- A toolbar beside the selection to copy, save, pin, upload, edit or read the text, and an
  optional mode bar for screenshot or video, region or full screen, colour picker and text capture
- Annotate right on the selection before it is captured
- Scrolling capture, auto capture on an interval, capture delay, cursor, magnifier and crosshair

**Recording**
- Video and GIF recording of a region, window or screen, with system audio and microphone
- FFmpeg encoders (x264, x265, VP9, AV1, NVENC, AMF, QSV, VAAPI) and GIF, WebP and APNG output
- Pause, timer, fixed duration, camera overlay and a mouse highlighter
- Native capture on Wayland through PipeWire, including KDE window recording

**Editor and effects**
- A full annotation editor: shapes, arrows, text, callouts, numbered steps, highlighter, blur,
  pixelate, magnifier, images and crop, with undo and redo
- Around 300 image effects, presets and an image beautifier
- A layered photo editor with styles and blend modes

**Upload and share**
- Imgur, Dropbox, Google Drive, OneDrive, Amazon S3 and S3-compatible storage, Azure Blob,
  Google Cloud Storage, Backblaze B2, FTP, FTPS, SFTP and dozens more
- Custom uploaders with the full `.sxcu` format, URL shorteners and sharing services
- A remote storage browser for S3, FTP and SFTP

**Smart tools**
- Text recognition (OCR) on a region or an image, offline
- Background removal and object eraser with local ONNX models, on the GPU where available
- Image description and analysis through any OpenAI-compatible provider, local or hosted

**Everything else**
- Workflows: chain after-capture and after-upload steps per hotkey
- Pin to screen, colour picker, ruler, QR codes, image and video converters, hash checker and more
- Searchable history with tags, about 30 languages, portable and sandbox modes, auto-update

## Install

Prebuilt packages will be published on the
[Releases](../../releases) page. Until then, build from source.

### Build from source

You need the Rust toolchain from [rustup](https://rustup.rs); the repository pins the version in
`rust-toolchain.toml`.

**Linux** (Debian/Ubuntu package names):

```sh
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
  libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev libdbus-1-dev \
  libpipewire-0.3-dev libclang-dev pkg-config
cargo build --release -p laqta
./target/release/laqta
```

**Windows** (with the MSVC build tools and [vcpkg](https://vcpkg.io)):

```powershell
vcpkg install "sqlite3[fts5]:x64-windows-static-md"
$env:VCPKG_ROOT = "<your vcpkg folder>"
cargo build --release -p laqta
.\target\release\laqta.exe
```

FFmpeg is downloaded on first use (or point Laqta at your own copy in Settings), and the optional
local models are fetched from the model manager when you first need them.

### Usage

Laqta runs in the tray. Press <kbd>Print Screen</kbd> for a region capture, or set your own
shortcuts in Settings → Shortcuts. A second launch forwards its arguments to the running instance,
so you can also script it:

```sh
laqta --action print-screen
```

## Project layout

Laqta follows a clean architecture: a pure core (`laqta-domain`, `laqta-imaging`,
`laqta-editor`, `laqta-photo`) with no I/O, an application layer (`laqta-app`), ports
(`laqta-ports`) and platform and service adapters around them. `apps/laqta` wires everything
together.

- [Architecture](docs/ARCHITECTURE.md): crates, ports and the rules between them
- [Features](docs/FEATURES.md): the full feature list
- [Glossary](docs/GLOSSARY.md): the names used in the code
- [Decisions](docs/adr): architecture decision records

## Contributing

Bug reports and pull requests are welcome. Before opening a pull request, run:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo xtask check-deps
```

## Acknowledgements

- [ShareX](https://github.com/ShareX/ShareX), for setting the bar for what a capture tool can
  do. Laqta's translations are converted from ShareX's translations (GPL-3.0), and its import
  feature reads ShareX's file formats.
- [Unsloth Studio](https://github.com/unslothai/unsloth), whose interface inspired Laqta's
  look and feel.
- [Zed's GPUI](https://www.gpui.rs) and [GPUI Component](https://github.com/longbridge/gpui-component)
  (included under `vendor/`, Apache-2.0), which the interface is built on.
- [FFmpeg](https://ffmpeg.org), [ONNX Runtime](https://onnxruntime.ai) and the many open-source
  crates listed in `Cargo.lock`.

## License

Laqta is free software, licensed under the [GNU General Public License v3.0 or later](LICENSE).
