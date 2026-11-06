# ADR 005: FFmpeg as a subprocess

Status: accepted (2026-11-07)

**Context.** ShareX records through an FFmpeg process by default, and also has a native Windows path (Windows.Graphics.Capture + Media Foundation).

**Decision.** The `Media` port's first adapter runs FFmpeg as a subprocess. A native Windows recorder is a second adapter, added in M7.

**Rejected.**
- **Linking libav* (via `ffmpeg-next`):** it ties the build to system FFmpeg headers or a vendored build and brings LGPL/GPL linking obligations. An FFmpeg crash would also take down the resident process.
- **GStreamer:** a large runtime dependency, not commonly installed on Windows, and it adds a second media stack beside FFmpeg.
- **Native-only from the start:** it needs a separate encoder for each OS before recording works anywhere.

**Consequences.** FFmpeg is found on `PATH`, configured by path, or downloaded on first use. The download is a BtbN static build (GPL variant, ADR 022) pinned to a dated `autobuild-*` release tag, not `latest`, which is rewritten daily. Its `checksums.sha256` hash is pinned in the binary, and the download goes through `laqta-http`'s verified fetch (ADR 024). Frames written to the FFmpeg pipe are a copy (ARCHITECTURE.md §1.4).
