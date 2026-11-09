# ADR 022: Licence and FFmpeg build

Status: accepted (2026-11-07, owner decision)

**Context.** Laqta's licence decides which libraries and binaries it may ship. Three of them matter here:
- the GPL FFmpeg build, which includes x264 and x265;
- reusing logic from ShareX, which is GPL-3.0;
- free code signing through SignPath Foundation, which requires an open-source licence.

**Decision.**
- Laqta is licensed `GPL-3.0-or-later`.
- Recording downloads the GPL FFmpeg build on first use (ADR 005). Low-end machines without a hardware encoder get x264 as a fallback.
- Models are downloaded on first use. OCR is about 21 MB (the PP-OCR detection and recognition models). Background removal is about 123 MB on Linux: the fp16 BiRefNet lite model (114.5 MB) plus onnxruntime (8.3 MB; the Windows build of onnxruntime is 78 MB). The prompt asks before a download and the progress shows as toasts; the model manager's own Download shows a percentage, and adding a custom model shows none.
- MEGA was deferred to M7 and dropped there (no maintained Rust client).

**Rejected.**
- **MIT or Apache-2.0:** rules out reusing ShareX logic, and GPL pieces could only be separate downloads.
- **Closed source:** paid signing, and GPL and AGPL pieces would have to stay out.
- **The LGPL FFmpeg build:** no software H.264 fallback.

**Consequences.**
- GPL-3.0 and AGPL-3.0 crates become licence-compatible: `gifski`, `imagequant`, `dssim-core`, and the `hyprland` and `niri-ipc` IPC crates. No stack choice depends on them, but later work may use them.
- `cargo-deny` enforces the licence allowlist. `cargo-about` generates the third-party notices.
- Code signing uses SignPath Foundation.
