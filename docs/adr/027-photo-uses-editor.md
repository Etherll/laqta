# ADR 027: The photo editor reuses the Editor engine's text and rasterizer

Status: accepted (2026-11-07, M7 plan D1).

**Context.** ARCHITECTURE §3 gave `laqta-photo` edges to `laqta-imaging` and `laqta-domain` only.
The photo editor needs the same text shaping (the injected `Fonts` handle, ADR 024) and the same
`tiny-skia` brush and shape rasterizing that `laqta-editor` already has, with snapshot tests.

**Decision.** `laqta-photo` depends on `laqta-editor`. Both are pure core, so the edge points inward
and the purity rules (no I/O, threads, environment or clock) still hold for both. `laqta-ui` gains an
edge to `laqta-photo` for the photo editor window.

**Rejected.**
- A copy of the text and rasterizer code in `laqta-photo`: two copies drift, and both need snapshots.
- A third shared crate: more surface for two users. It can be split out later if a third user appears.
