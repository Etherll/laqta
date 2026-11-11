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
  <img alt="macOS" src="https://img.shields.io/badge/macOS-14%2B%20(preview)-3D8BFD?style=flat-square">
  <img alt="Rust" src="https://img.shields.io/badge/built%20with-Rust-1E2A78?style=flat-square">
  <img alt="License" src="https://img.shields.io/badge/license-GPL--3.0-FF6B5A?style=flat-square">
</p>

<p align="center">
  <img alt="Laqta's main window: capture modes, quick actions and the capture library" src="assets/readme/cover-light.webp" width="900">
</p>

**Laqta** (Arabic لقطة, *"a snapshot"*) takes screenshots and screen recordings, lets you mark
them up, and uploads them wherever you want, starting from one shortcut. It's written in Rust with
a GPU-drawn interface, sits in the tray until you need it, and works on Windows, on Linux
(X11 and Wayland) and on macOS 14 or later.

Coming from **ShareX**? Laqta is built to replace it, and it brings your setup along: settings,
hotkeys, workflows, custom uploaders (`.sxcu`), image effects (`.sxie`), backups (`.sxb`) and
history. The command-line options you already use work too.

## ⚡ Get started

Download the latest build for your system:

<table>
  <tr><td><b>Platform</b></td><td><b>Download</b></td></tr>
  <tr><td><b>Windows installer (x64)</b></td><td><a href="../../releases/latest/download/laqta-windows-x64-setup.exe">laqta-windows-x64-setup.exe</a></td></tr>
  <tr><td><b>Windows portable (x64)</b></td><td><a href="../../releases/latest/download/laqta-windows-x64-portable.exe">laqta-windows-x64-portable.exe</a></td></tr>
  <tr><td><b>Linux AppImage (x86_64)</b></td><td><a href="../../releases/latest/download/laqta-linux-x86_64.AppImage">laqta-linux-x86_64.AppImage</a></td></tr>
  <tr><td><b>Debian / Ubuntu (amd64)</b></td><td><a href="../../releases/latest/download/laqta-linux-amd64.deb">laqta-linux-amd64.deb</a></td></tr>
  <tr><td><b>Linux archive (x86_64)</b></td><td><a href="../../releases/latest/download/laqta-linux-x86_64.tar.zst">laqta-linux-x86_64.tar.zst</a></td></tr>
  <tr><td><b>macOS 14+ (Apple silicon and Intel), preview</b></td><td><a href="../../releases/latest/download/laqta-macos-universal.dmg">laqta-macos-universal.dmg</a></td></tr>
</table>

Every file has a `.sha256` next to it on the [Releases](../../releases) page.

- **Windows:** the installer sets Laqta up for your account, adds Start menu and desktop
  shortcuts, and keeps it updated. The portable `.exe` runs from any folder, a USB stick
  included, and keeps its settings in a `LaqtaData` folder beside it.
- **Linux:** make the AppImage executable (`chmod +x laqta-linux-x86_64.AppImage`) and run it. On
  Debian or Ubuntu you can instead install the package with
  `sudo apt install ./laqta-linux-amd64.deb`. The archive holds the plain binaries, or you can
  [build from source](#%EF%B8%8F-build-from-source).
- **macOS (preview):** open the `.dmg` and drag Laqta into Applications. The build isn't
  notarized by Apple yet, so the first time you open it macOS blocks it: go to
  **System Settings → Privacy & Security** and click **Open Anyway**. Laqta then asks for the
  Screen Recording and Accessibility permissions. Recording uses FFmpeg from Homebrew
  (`brew install ffmpeg`). The macOS build passes CI but hasn't been tested on a real Mac yet,
  so please [report](../../issues) anything that doesn't work.
- **NixOS / Nix:** try it with `nix run github:Etherll/laqta`, or install it from the flake
  (below). The Nix package brings its own FFmpeg and ONNX Runtime from Nixpkgs instead of
  downloading them on first use. Text recognition and the local models need ONNX Runtime (about 240 MiB); leave it out
  with `laqta.override { withOnnxruntime = false; }`.

  ```nix
  # flake.nix of your system: add the input, then the package
  inputs.laqta.url = "github:Etherll/laqta";
  # NixOS:        environment.systemPackages = [ inputs.laqta.packages.${pkgs.stdenv.hostPlatform.system}.default ];
  # Home Manager: home.packages = [ inputs.laqta.packages.${pkgs.stdenv.hostPlatform.system}.default ];

  # NixOS: download the build from the binary cache instead of compiling it (about 35 minutes)
  nix.settings = {
    substituters = [ "https://laqta.cachix.org" ];
    trusted-public-keys = [ "laqta.cachix.org-1:ZTaWCeegQ7a7vo/SbzIcA/CCGIVl+Syry34569o9Rz4=" ];
  };
  ```

  The cache holds builds made with the flake's own Nixpkgs. Adding
  `inputs.laqta.inputs.nixpkgs.follows = "nixpkgs";` builds Laqta with your Nixpkgs instead, which
  avoids a second copy of the libraries it shares with your system but compiles it on your machine.

  On Wayland, screen capture and global shortcuts go through the desktop portal, so keep
  `xdg.portal.enable = true` with your desktop's portal (GNOME and KDE set this up themselves;
  on Hyprland or Sway add `xdg-desktop-portal-hyprland` or `xdg-desktop-portal-wlr`).

Laqta lives in the tray (the menu bar on macOS). Press <kbd>Ctrl</kbd>+<kbd>PrtSc</kbd> to
capture a region, <kbd>PrtSc</kbd> for the whole screen, or set your own shortcuts in
**Settings → Shortcuts**. Macs have no Print Screen key and keep
<kbd>⌘</kbd>+<kbd>⇧</kbd>+<kbd>3</kbd>/<kbd>4</kbd>/<kbd>5</kbd> for their own screenshots, so
there the defaults are <kbd>⌃</kbd>+<kbd>⌥</kbd>+<kbd>2</kbd> for a region and
<kbd>⌃</kbd>+<kbd>⌥</kbd>+<kbd>1</kbd> for the whole screen.

## ⭐ Features

### 📸 Capture
* **Every mode:** region, window, monitor, full screen, last region, scrolling capture, and auto
  capture on a timer.
* **Smart selection:** the overlay snaps to windows *and to the parts inside them* (sidebars,
  panels, text boxes), found from the screenshot itself and from the accessibility tree. Scroll to
  grow the selection to the enclosing part.
* **Act on the selection:** a toolbar next to it copies, saves, pins, uploads, edits, or reads the
  text out of it. You can also draw on the selection before you capture it.
* **Text right where it is:** the words in your selection light up on the screenshot itself. Drag
  over the ones you want and copy them, with no extra window.
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
* **Text recognition (OCR)** for a region or an image. It runs offline, and on a region it
  highlights the words in place so you can pick the ones to copy.
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
  <tr>
    <td width="50%"><img alt="Text read in place: every word in the selection is highlighted on the screenshot, and a bar says the text was copied" src="assets/readme/text-read.webp"></td>
    <td width="50%"><img alt="Four words picked with a drag, shown in blue, with a Copy button under the selection" src="assets/readme/text-select.webp"></td>
  </tr>
  <tr>
    <td align="center"><b>Text in place</b>: every word lights up where it is</td>
    <td align="center"><b>Pick and copy</b>: drag over the words you want</td>
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

On NixOS, `nix build` builds the package into `./result`, and `nix develop` opens a shell with
the toolchain and libraries for `cargo build` and `cargo run`.

#### Windows
With the MSVC build tools and [vcpkg](https://vcpkg.io):

```powershell
vcpkg install "sqlite3[fts5]:x64-windows-static-md"
$env:VCPKG_ROOT = "<your vcpkg folder>"
cargo build --release -p laqta
.\target\release\laqta.exe
```

#### macOS
With the Xcode command line tools (`xcode-select --install`). macOS grants screen and
accessibility permissions to an app bundle, so build the bundle and open it:

```bash
cargo build --release -p laqta -p laqta-nmh
scripts/macos-bundle.sh target/release/laqta target/release/laqta-nmh 0.0.0 target/mac
open target/mac/Laqta.app
```

On Windows and Linux, Laqta downloads FFmpeg the first time you record; on macOS it uses
Homebrew's. You can also point it at your own copy in Settings. The optional local models download from the model manager when you first need them.

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
| Adapters | `laqta-platform-linux`, `laqta-platform-windows`, `laqta-platform-macos`, `laqta-media`, `laqta-upload`, `laqta-cloud`, `laqta-library`, `laqta-ml`, … |
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
