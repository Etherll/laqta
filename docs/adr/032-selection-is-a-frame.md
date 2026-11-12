# ADR 032: Selection is a frame

Status: accepted (2026-11-12). Amends ADR 029: decision 1 (`Lock` and `Unlock` are removed, so the selection stays adjustable once it has marks).

**Context.**
- ADR 029 froze the selection once annotations existed (`RegionCommand::Lock`), because moving the region would leave them behind. After the first mark the user could not move or resize the selection to correct it.
- The marks lived in the crop's own pixels, and nothing outside the editor knew an offset, so a move had no way to carry them along.
- The grab area was the drawn handle dot (`handle_at` tests only `handle_rects`), so an edge was easy to miss. Hover was still tracked under a held selection, and a press outside it began a new drawing gesture.
- Pointer routing, cursor shape and the Esc ladder were in the overlay view, which is GPUI code. ADR 017 keeps decisions like these in presenters.

**Decision.**
1. **Annotations live in desktop space, and the selection is a frame over them.** When the frame moves or resizes, the overlay sends `Command::Reframe { image, dx, dy }`. The editor takes the new crop as its base image and moves every mark by (dx, dy), so each stays over the same pixels. The undo and redo entries move with the marks.
   - `Reframe` is not undoable and not recorded in history, like zoom.
   - It is refused while the history holds a destructive entry, whose pixel snapshot cannot move. The capture toolbar has no tool that makes one, so the overlay does not meet the refusal.
   - `Lock` and `Unlock` are removed. `selected()` stays `Some` while the frame moves or resizes.
2. **The session crops once per settled change.** The desktop is cropped when a move or resize settles, not on every pointer move, so a drag costs one crop at release.
3. **Hover only while no selection is held.** A held selection shows no hover target, and a click outside it with no drag changes nothing.
4. **The grab area is separate from the drawn dot.** `grab_size` is the side of each corner's grab square and the width of each edge's grab strip, in physical pixels, and is at least `handle_size`. `grab_at` tests the corners before the edges. `handle_rects` still draws the dots.
5. **Presenters decide the pointer and the Esc ladder** (ADR 017). `presenters/capture_pointer` decides who gets a press and which pointer shape shows. `presenters/capture_escape` decides what Esc and the ✕ do. The route chosen on a press receives that press's moves and release.
   - With marks on screen, a press outside the frame does nothing. A right or middle press does nothing either, so it cannot cancel the capture. Handles and edges always resize, whatever tool is out.
   - Esc steps back one layer at a time: typing, then a selected mark, then the tool. With marks left, the first press shows "Press Esc again to discard N annotations". A second press within `DISARM_AFTER` (3 s) cancels the capture. The ✕ follows the same rule.

**Rejected.**
- **An editor over the whole desktop.** The editor would hold the full desktop as its base image and render all of it. ARCHITECTURE §1.2 caps the peak memory of region capture on two 4K monitors at under 150 MB.
- **An origin stored on the document.** The document would subtract its origin in every render, hit test and effect. The result is the same as moving the marks once in `Reframe`, but every one of those paths would change.
- **Keeping the lock.** The selection could not be adjusted after the first mark, which is the problem this ADR removes.

**Consequences.**
- Marks partly outside the frame are clipped on export.
- A mark fully outside the frame survives in the editor and returns when the frame moves back over it.
- `Lock`, `Unlock` and `RegionView::locked` are removed. ADR 029 decision 1 keeps the old lock text as history, with the amendment above.
