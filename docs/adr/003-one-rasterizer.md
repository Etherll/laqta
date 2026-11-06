# ADR 003: One CPU rasterizer

Status: accepted (2026-11-07). See also ADR 013 (the Editor engine owns rendering) and ADR 024 (fonts are injected)

**Context.** Drawing annotations with GPUI primitives for the live view and with a separate renderer for export would make the two drift apart.

**Decision.** `tiny-skia` (with `cosmic-text` + `swash` for text) renders both the live view and the export, inside the Editor engine (ADR 013). GPUI only shows the resulting texture and paints the chrome. Live editing re-renders dirty rects only.

**Rejected.**
- **A GPU vector renderer (vello):** it needs a wgpu device separate from GPUI's renderer, which costs memory on low-end machines, and output can differ between GPU drivers.
- **GPUI paths for the view with `tiny-skia` for export:** the two renderers would not match.

**Consequences.** What the user sees is what gets saved, and output is identical on every machine. Very large canvases depend on dirty-rect rendering staying fast. This is measured by the M3 snapshot benches.
