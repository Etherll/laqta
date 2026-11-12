# The capture selection is a frame over its marks

## Problem

Region capture shows a toolbar beside the selection, and you can draw on the selection in place. Today the first drawing tool you pick locks the selection: its handles disappear, and dragging, arrow keys and right-click do nothing. Once a mark exists, the only way to adjust the selection again is to undo every mark. Nothing on screen says why.

The lock exists because the annotation engine runs on a crop of the selection. Moving the crop would leave every mark at the old spot, so the overlay froze the selection rather than move the marks.

The overlay also had smaller faults around the selection:

- A stray click outside a held selection replaced it with the window under the pointer, with no outline to warn of it.
- While the selection was moved or resized, its size badge and toolbar vanished, and the window under the press stayed outlined.
- A drawing tool took presses on the selection's handles, so it drew instead of resizing.
- Esc, or the ✕ button, threw the marks away without asking.
- A picked mark showed only a thin outline, with no handles.
- The handles were 8 px targets, and the edges between them could not be grabbed.
- The selection border covered 2 px of the captured pixels.
- No colour or size could be set for a tool.

## Solution

Marks live in desktop space, and the selection is a frame over them. When the frame moves or resizes, the session crops again, and the editor shifts every mark by the old corner minus the new one, so each mark stays over the same pixels. With nothing to protect, the lock goes.

Who gets a press, which pointer shape shows, and what Esc does next become GPUI-free presenters (ADR 017), so they are unit tests rather than handler code. The decision is recorded in [[69a1dcc:docs/adr/032-selection-is-a-frame.md]].

```mermaid [Moving the selection after drawing]
sequenceDiagram
    participant O as Overlay
    participant S as Region session
    participant R as RegionState
    participant E as Editor engine
    O->>S: PointerDown / Move / Up on the frame
    S->>R: handle(input)
    R-->>S: selected() moves; in_gesture() until release
    S->>S: crop once the move settles
    S->>O: RegionView { selected, selection_image }
    O->>E: Command::Reframe { image, dx, dy }
    E-->>O: marks shifted by (dx, dy), canvas re-rendered
    %% ref node:O [[69a1dcc:crates/laqta-ui/src/overlay.rs#L642-L665]]
    %% ref node:S [[69a1dcc:crates/laqta-app/src/region/mod.rs#L289-L322]]
    %% ref node:R [[69a1dcc:crates/laqta-domain/src/region.rs#L316-L320]]
    %% ref node:E [[213cdab:crates/laqta-editor/src/lib.rs#L610-L629]]
```

**Goals**

- The selection stays adjustable after drawing, and marks stay over the pixels they were drawn on.
- No press, key or button throws away marks without a warning.
- The size stays visible while the selection is moved or resized.
- The pointer shows what a press will do.
- Every routing decision can be tested without a window.

**Non-goals**

- An editor over the whole desktop: two 4K monitors would break the 150 MB capture budget in ARCHITECTURE §1.2.
- A smaller `RegionView`, or one key table for the overlay. Both came up in the architecture review and are left for later.

## Phases

### ✅ Phase 1: Marks follow a moved frame

**Today**

The editor works on one image and has no notion of the image moving under its marks. The only way to swap the image is `Command::ReplaceImage`, which flattens the marks into the pixels.

**Proposed**

`Command::Reframe { image, dx, dy }` swaps the base image and moves every annotation by `(dx, dy)`. The undo and redo entries move too, and so does the clipboard. Undo after a reframe therefore acts on the moved marks. A reframe is not an undo step itself, since moving the frame changes the view, not the document.

```callstack [Reframe]
 editor.execute(Command::Reframe { image, dx, dy }) [[213cdab:crates/laqta-editor/src/lib.rs#L158-L162]]
 └── reframe(image, dx, dy) [[213cdab:crates/laqta-editor/src/lib.rs#L610-L629]]
     ├── if history.has_destructive(): return [[213cdab:crates/laqta-editor/src/history.rs#L104-L106]]
     ├── finish_editing()  # typed text lands in history first
     ├── base = image
     ├── annotation.kind.translate(dx, dy)  # every mark
     ├── entry.translate(dx, dy) [[213cdab:crates/laqta-editor/src/history.rs#L52-L62]] # every undo and redo entry
     ├── clipboard.translate(dx, dy)
     └── pending, gesture reset; generation += 1; cache reset
```

A destructive entry (crop, flip, applied effect) holds a pixel snapshot that cannot move, so a reframe is refused while one is in the history. The capture toolbar offers no destructive tool, so the overlay never meets the refusal. A mark moved fully outside the canvas is kept, and comes back when the frame covers it again. Export clips what lies outside.

`Editor::annotation_at(x, y)` tells the overlay whether a press lands on a mark [[213cdab:crates/laqta-editor/src/lib.rs#L320-L322]].

Tests cover moving and resizing, undo and redo after a reframe, a mark off the canvas and back, the export size, the refusal after a destructive step, and the hit test.

```changes
213cdab
crates/laqta-editor/src/lib.rs
crates/laqta-editor/src/history.rs
```

### ✅ Phase 2: The selection stays adjustable

The domain, session and overlay change together: removing the lock from `RegionView` leaves the overlay without its old routing, so the two halves land in one commit.

#### Selection rules

**Today**

`RegionCommand::Lock` and `Unlock` freeze the selection. `selected()` is `None` during any gesture, so the session re-crops on every step and the overlay loses the toolbar. Hover stays on while a selection is held, so a click outside snaps to a window. The grab area is the drawn 8 px dot.

**Proposed**

- **No lock.** The `Lock`/`Unlock` commands, the `locked` field and every check for it are gone from the domain, the port and the session.
- **`selected()` during move and resize.** It keeps returning the frame while it is moved or resized, but not while a new rectangle is drawn [[69a1dcc:crates/laqta-domain/src/region.rs#L316-L320]].
- **Hover only while nothing is held.** There is no hover while a selection is held or a gesture runs [[69a1dcc:crates/laqta-domain/src/region.rs#L591-L604]]. A click outside a held selection therefore changes nothing.
- **A wider grab area.** `grab_at(rect, at, grab)` first checks squares at the corners and edge midpoints, then strips along each edge [[69a1dcc:crates/laqta-domain/src/region.rs#L232-L263]]. `grab_size` is twice the drawn handle size. `RegionView::handle_at` uses it too, so the session, the cursor and text picking agree [[69a1dcc:crates/laqta-ports/src/overlay.rs#L140-L146]].
- **Crop once a change settles.** The session crops when the selection changed and no gesture is under way. The text read in place hides during a gesture, and the crop, marks and text are only cleared once a change settles, so a stray click keeps them [[69a1dcc:crates/laqta-app/src/region/mod.rs#L289-L322]].

```rust [Crop once a change settles]
let selected = state.selected();
if selected != cropped && !state.in_gesture() {
    cropped = selected;
    annotated = None; // the edited crop belongs to the old frame
    text_gone = text.take().is_some();
    // ... crop `selected` and send it with the next view
}
// [!note L2] A move, resize or new draw crops once, on release.
// [!note L4-L5] A click that ends where it began never gets here, so its marks and text stay.
```

#### Overlay presses, cursors and Esc

**Today**

The mouse and key handlers in `overlay.rs` decide routing inline: a drawing tool takes every press, and presses outside the canvas are dropped silently. Esc puts the tool away, then cancels the capture.

**Proposed**

Each press asks `capture_pointer::route`, and the handlers carry out the answer [[69a1dcc:crates/laqta-ui/src/presenters/capture_pointer.rs#L63-L81]]. The model remembers the route, so the press's moves and release go to the same place on any monitor's window.

```callstack [Who gets a press]
 route = capture_pointer::route(scene, at, button) [[69a1dcc:crates/laqta-ui/src/presenters/capture_pointer.rs#L63-L81]]
 ├── right or middle: Ignore with marks, else Session  # it could cancel the capture
 ├── over the text read in place: Text
 ├── no frame, or a gesture running: Session
 ├── on a handle or edge (any tool): Session  # resize
 ├── inside the frame
 │   ├── on a mark, or a tool out: Editor
 │   └── region tool: Session  # move the frame
 └── outside: Session with the region tool and no marks, else Ignore
```

`pointer_scene` gathers what the pointer is over [[69a1dcc:crates/laqta-ui/src/overlay_tools.rs#L159-L182]]. It counts the picked mark's handles as part of the mark, and treats a multi-region session's active region as its frame. `capture_pointer::pointer` gives the matching shape: resize arrows, an open hand (closed while dragging), a crosshair to draw, an I-beam over text, and not-allowed where a press does nothing [[69a1dcc:crates/laqta-ui/src/overlay.rs#L711-L724]].

Esc steps back one layer at a time [[69a1dcc:crates/laqta-ui/src/presenters/capture_escape.rs#L39-L49]]:

1. stop typing;
2. let go of the picked mark;
3. put the tool away;
4. with marks, warn: "Press Esc again to discard N annotations";
5. cancel the capture.

The ✕ buttons give the same warning. It lapses after 3 s, or when the number of marks changes [[69a1dcc:crates/laqta-ui/src/overlay_tools.rs#L334-L355]].

`sync_annot` reframes the existing engine on a new crop, and builds one only for the first crop [[69a1dcc:crates/laqta-ui/src/overlay_tools.rs#L118-L154]]. The engine survives a gesture, so a click that comes to nothing keeps it. The canvas is clipped to the frame, and until the new render lands the old picture stays where it was drawn [[69a1dcc:crates/laqta-ui/src/overlay_tools.rs#L398-L424]].

Several more changes close the gaps a review found:

- A picked mark takes Delete and the arrow keys under any tool.
- Delete never reaches the session while marks exist, since it would remove the selection with every mark on it [[69a1dcc:crates/laqta-ui/src/overlay.rs#L744-L810]].
- Actions wait while the frame is dragged, or until its new crop has reached the engine, so they never save a crop of the old frame [[69a1dcc:crates/laqta-ui/src/overlay_tools.rs#L367-L392]].

Further changes in the view:

- **Border:** the selection border is drawn 2 px outside the selection.
- **Size badge:** it stays during a drag and shows the position while moving [[69a1dcc:crates/laqta-ui/src/presenters/capture_toolbar.rs#L279-L285]]. The toolbar waits for the release.
- **Picked mark:** it gets the editor window's handles, through a shared `mark_handle`, and a 2 px outline.
- **Style row:** colour swatches and three sizes appear under the toolbar when a tool is out [[69a1dcc:crates/laqta-ui/src/presenters/capture_toolbar.rs#L218-L228]] [[69a1dcc:crates/laqta-ui/src/overlay_tools.rs#L533-L601]]. The picked swatch and size carry a ring, so their state does not rest on colour alone.

No screenshots: the overlay draws over a live screen capture, which the browser tools here cannot drive. Checks run:

- `cargo test --workspace`: 1852 passed;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all -- --check`;
- `cargo xtask check-deps`.

The headless overlay tests cover:

- moving the selection after drawing;
- each Esc step;
- the close button;
- Delete and actions with marks;
- the wider grab area under text.

Nobody has tried it by hand yet.

```changes
69a1dcc
docs/adr/032-selection-is-a-frame.md
crates/laqta-domain/src/region.rs
crates/laqta-ports/src/overlay.rs
crates/laqta-app/src/region/mod.rs
crates/laqta-ui/src/presenters/capture_pointer.rs
crates/laqta-ui/src/presenters/capture_escape.rs
crates/laqta-ui/src/presenters/capture_toolbar.rs
crates/laqta-ui/src/overlay_tools.rs
crates/laqta-ui/src/overlay.rs

ignore:
crates/laqta-app/src/region/tests.rs
crates/laqta-ui/src/headless_tests/overlay.rs
crates/laqta-ui/src/presenters/mod.rs
crates/laqta-ui/src/overlay_text.rs
crates/laqta-ui/src/editor_window.rs
docs/GLOSSARY.md
docs/adr/029-capture-toolbar-and-in-place-annotation.md
docs/ARCHITECTURE.md
```

### ✅ Phase 3: Models leave memory after 10 s

**Today**

A loaded model (OCR, background removal, inpainting) stays in memory for 60 to 120 s after its last use.

**Proposed**

`ModelReleaser` drops a session after 10 s unused, and checks every second, so a model leaves 10 to 11 s after its last use [[0966819:apps/laqta/src/models.rs#L55-L58]]. A check is a lock and a few timestamp comparisons. The timer runs only while a model is loaded, so the idle budget of zero wakeups still holds. A model used again after it was dropped loads from disk again.

```changes
0966819
apps/laqta/src/models.rs

ignore:
docs/ARCHITECTURE.md
```

## References

- ADR 032, the decision this spec carries out: [[69a1dcc:docs/adr/032-selection-is-a-frame.md]]
- ADR 029, now amended by ADR 032: [[docs/adr/029-capture-toolbar-and-in-place-annotation.md]]
- ADR 017, presenters inside `laqta-ui`: [[docs/adr/017-presenters-in-ui.md]]
- The plan the work followed: `PLAN.md` (untracked)
