# ADR 012: One Region session

Status: accepted (2026-11-07), amended by ADR 029 (annotation)

**Context.** An earlier design had the capture use case snapshot windows, then call the overlay, while the overlay separately queried UI elements live. Showing the overlay first and loading windows in the background keeps it instant. Recording needs the same selection, and naming tokens such as `%pn` need the target window.

**Decision.** One Region session module in `laqta-app`, used by capture and recording. It shows the overlay through the `Overlay` port before hover targets load, loads hover targets from one `HoverSource` port in the background, runs the `domain::region` state machine, and returns a selection with rects, the target window, the image and the action the user chose. Annotation is the overlay's: it hosts the Editor engine on the selection's crop and sends the edited crop back, and the session returns that crop as the image (ADR 029). The session itself holds no engine.

**Consequences.** `WindowCatalog` and `ElementLocator` merge into `HoverSource`. The overlay view forwards input and paints state. Tests use scripted input and a fake `HoverSource`.
