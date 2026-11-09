# ADR 013: One Editor engine; pure libraries allowed in the core

Status: accepted (2026-11-07), amended by ADR 024 (fonts) and ADR 029 (who hosts the engine)

**Context.** An earlier design spread one annotation tool over the domain document, a display list, a render crate, imaging kernels, the UI and a view model, so adding Blur touched five crates. The display list had exactly one consumer.

**Decision.** `laqta-editor` is one deep module: input events and commands in; dirty rects, pixels and a chrome description out. It owns the document, the tools, undo, the effect cache and rasterization. The pure core may depend on pure computation libraries (`tiny-skia`, `cosmic-text`, `swash`, codecs) as long as they do no I/O.

**Fonts (ADR 024).** The engine never loads fonts. `main` builds a font loader (a function that loads the system font database and locale), wraps it in the engine's lazy `Fonts` handle, and passes it to `laqta-ui`, the only host of the engine: the editor window and the region overlay (ADR 029). The Region session and `laqta-app` never receive it. The loader runs on first use. `cosmic-text` is built with `default-features = false`, and the font system is created with `FontSystem::new_with_locale_and_db`.

**Consequences.** A tool's behaviour lives in `laqta-editor`; its label, hint and shortcut sit in `laqta-ui`'s `editor_toolbar` presenter, and a test checks those shortcuts against the engine's key map. Tests are input scripts with image snapshots. The separate render crate is gone.
