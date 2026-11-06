# ADR 022: Licence and FFmpeg build

Status: accepted (2026-11-07, owner decision)

**Context.** Laqta's licence decides which libraries and binaries it may ship. Three of them matter here:
- the GPL FFmpeg build, which includes x264 and x265;
- reusing logic from ShareX, which is GPL-3.0;
- free code signing through SignPath Foundation, which requires an open-source licence.

**Decision.**
- Laqta is licensed `GPL-3.0-or-later`.
- Recording downloads the GPL FFmpeg build on first use (ADR 005). Low-end machines without a hardware encoder get x264 as a fallback.
- Models (OCR about 20 MB, background removal about 135 MB with onnxruntime) are downloaded on first use, with progress shown.
- MEGA is deferred to the M7 plan.

**Rejected.**
- **MIT or Apache-2.0:** rules out reusing ShareX logic, and GPL pieces could only be separate downloads.
- **Closed source:** paid signing, and GPL and AGPL pieces would have to stay out.
- **The LGPL FFmpeg build:** no software H.264 fallback.

**Consequences.**
- GPL-3.0 and AGPL-3.0 crates become licence-compatible: `gifski`, `imagequant`, `dssim-core`, and the `hyprland` and `niri-ipc` IPC crates. The stack choices in `research/STACK.md` don't depend on them, but later plans may use them.
- `cargo-deny` enforces the licence allowlist. `cargo-about` generates the third-party notices.
- Code signing uses SignPath Foundation.
