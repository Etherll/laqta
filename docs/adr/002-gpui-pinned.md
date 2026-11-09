# ADR 002: GPUI pinned to gpui-component's snapshot

Status: accepted (2026-11-07)

**Context.**
- crates.io `gpui` is 0.2.2 and lacks `WindowKind::LayerShell`, which the Wayland overlay needs.
- `gpui-component` 0.9.0 depends on `gpui-pre =0.3.11`, a crates.io snapshot of Zed commit `f816e1f` (2026-11-01). That commit has `WindowKind::LayerShell`.
- `gpui-component`'s own Cargo.toml warns that any snapshot may change GPUI's API, so it pins the snapshot exactly.

**Decision.**
- Depend on `gpui = { package = "gpui-pre", version = "=0.3.11" }` and `gpui_platform = { package = "gpui-pre-platform", version = "=0.3.11" }`.
- Depend on `gpui-component = "=0.9.0"` and `gpui-kit-assets = "=0.9.0"` (ADR 028). Both are part of GPUI Kit and are released together. `gpui-fps`, GPUI Kit's debug HUD, was to be an optional dependency behind a `debug-hud` feature; it is not adopted (no HUD exists), and it joins the upgrade set if one is built.
- Upgrade GPUI only when GPUI Kit moves to a new snapshot, and move `gpui-pre`, `gpui-component` and `gpui-kit-assets` in one commit.
- Only `laqta-ui` and `apps/laqta` depend on either crate.

**Rejected.**
- **egui:** immediate-mode, so a frame is redrawn on every input event. Its widgets don't fit the native-looking settings and history screens. It has no layer-shell support.
- **Slint:** its declarative markup language would hold a second copy of UI logic outside Rust. The editor canvas and overlay need custom GPU drawing, which is harder there.
- **iced:** layer-shell exists only in third-party forks, and there is no widget set comparable to `gpui-component`.
- **A raw Zed git revision:** `gpui-component` would not compile against an arbitrary revision.
- **Hand-built widgets:** months of work.

**Consequences.** Builds use crates.io only, with no git dependencies. The GPUI pin is risk R3 in ARCHITECTURE.md.
