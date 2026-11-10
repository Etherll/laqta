# ADR 030: macOS support

Status: accepted (2026-11-10)

**Context.** ADR 001 left macOS until after M7. Windows and Linux are at parity (W1), and GPUI already runs on macOS. No Mac is available on the development machine, so the work has to be provable from Linux and from a CI runner.

**Decision.**

- **One adapter crate, `laqta-platform-macos`.** It implements the existing ports and depends only on `laqta-ports` and `laqta-domain`. No port is added: `main` gets the same bundle of ports as on Windows.
- **Minimum macOS 14.** Screenshots use `SCScreenshotManager`, which needs 14.
- **Bindings are the `objc2` crates.** They are pure Rust with no build script, so `cargo check -p laqta-platform-macos --target aarch64-apple-darwin` type-checks the adapter on Linux. Carbon's hotkey calls have no binding crate and are declared in the adapter.
- **Capture and recording use ScreenCaptureKit.** Its content filter leaves Laqta's own app out of every screenshot and stream, so no window is hidden before a capture (`laqta-ui`'s `vanish` does nothing on macOS). One `SCStream` gives the frames and the system audio, so no loopback driver is needed. FFmpeg encodes; there is no native recorder.
- **Recording passes frames, not an OS screen reference.** `OsScreen` (ADR 016) gets no macOS variant.
- **Physical pixels keep each display's own size.** macOS has one global space of points. A display's pixel size is its point size times its own scale, and its pixel origin is its point origin times the largest scale among the displays, so the pixel rects of displays with different scales never overlap. A rect takes the scale of the display under its centre.
- **A recording covers one display.** One `SCStream` captures one display, so an area that crosses displays is cut to the display under its centre. Screenshots are not limited this way.
- **Global hotkeys use Carbon `RegisterEventHotKey`.** It needs no permission and is event-driven. Mac keyboards have no Print Screen key, so the default workflows on macOS are Control+Option+1 to 5 (`Settings::default_for_os`).
- **Permissions are asked for when a feature first needs one**, never at startup. A missing permission fails with a message that names it and where to turn it on.
- **AppKit work runs on the main queue**, sent there from inside the adapter. The adapter does not use GPUI.
- **Distribution is one universal `.app` in a `.dmg`.** It is signed with a Developer ID and notarized when the release workflow has the signing secrets, and ad-hoc signed otherwise. Updates show the download page, as on Linux.
- **FFmpeg is found on `PATH` or in Homebrew.** There is no pinned FFmpeg download for macOS yet.

**Rejected.**

- *A Finder Sync extension for right-click entries.* It is a second signed target built against Apple's extension API. "Open With" and dropping a file on the Dock icon upload the file instead.
- *Supporting macOS 13.* It needs a second screenshot path through `CGDisplayCreateImage`, which Apple has removed from the current SDK.
- *The `cocoa`/`core-graphics` crates or a Swift helper.* The first are unmaintained in favour of `objc2`; the second adds a toolchain and cannot be type-checked from Linux.
- *VideoToolbox codecs in this phase.* They add codec variants to the domain and Settings; the software encoders already work.

**Consequences.** `Unsupported` on macOS (ADR 018): changing another app's window (borderless, always on top), Finder menu entries, printing. HDR tone-mapping is not needed because ScreenCaptureKit returns SDR. A CI job on a macOS runner lints, tests and runs the app with a screenshot per step; behaviour that needs a person (permission prompts, audio, a camera, Retina and multiple displays) is listed in `docs/plan/W2.md` T19.
