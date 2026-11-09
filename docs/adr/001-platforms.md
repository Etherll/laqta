# ADR 001: Target platforms

Status: accepted (2026-11-07)

**Context.** GPUI supports Windows, macOS and Linux. Each platform needs its own capture, hotkey, tray and hover-target code.

**Decision.** Windows 10+ and Linux (Wayland and X11) are supported from M0. macOS work starts after M7 is complete and has no milestone yet.

**Rejected.** Supporting all three platforms from M0. It triples platform work before the core features are in place, and none of the first users asked for macOS.

**Consequences.** Every platform port has a Windows and a Linux adapter from the start. Wayland limits are handled by ADR 019 and by ports returning `Unsupported` (ADR 018). `laqta-platform-macos` is not created until macOS work starts.
