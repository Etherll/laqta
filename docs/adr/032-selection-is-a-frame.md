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
3. **Hover only while no selection is held.** A held selection shows no region or window hover target (the outline of a mark under the Pointer is decision 5's), and a click outside it with no drag changes nothing.
4. **The grab area is separate from the drawn dot.** `grab_size` is the side of each corner's grab square and the width of each edge's grab strip, in physical pixels, and is at least `handle_size`. `grab_at` tests the corners before the edges. `handle_rects` still draws the dots.
5. **One Pointer works on the selection and its marks.** The Pointer (V or M, and the tool a settled selection starts with) needs no switching: the first match under a press wins. A handle of the picked mark resizes the mark; the selection's handles and edges resize the selection; a mark's stroke, or the inside of the picked mark's box, picks and drags the mark; empty space inside moves the selection, and a click there lets the picked mark go. Alt at the press moves the frame alone even over a mark, so a mark covering the selection never pins it. A mark under the Pointer is outlined, so the click's target shows first. With a drawing tool out a press draws, even over a mark, and the picked mark's handles still win; holding Space lends the Pointer until it comes up (after the drag under way); Space going down mid-drag changes nothing. An earlier version split this into Move and Select tools; switching between them for every adjustment cost more than the mistakes it prevented.
   - The image editor has no selection to move, so its Select tool stays, but it shares the hover outline, Space-hold (Select stands in) and the pointer shapes. The shared rules are presenters: `mark_pointer` (handle reach, grips, the editor canvas's cursor) and `space_lend` (Space-hold); `mark_chrome` draws the handle dot and maps a pointer to a cursor for both windows.
   - A move carries the marks along: `Reframe` gets (0, 0), so the marks keep their place in the crop. Holding Alt at the press moves the frame alone, and the marks stay over the same desktop pixels. A resize always keeps them over their pixels.
   - While a move carries the marks, the canvas steps aside until release, since its pixels are the old crop's.
6. **Presenters decide the pointer and the Esc ladder** (ADR 017). `presenters/capture_pointer` decides who gets a press and which pointer shape shows, on top of `presenters/mark_pointer`. `presenters/capture_escape` decides what Esc does. The route chosen on a press receives that press's moves and release.
   - With marks on screen, a press outside the frame does nothing. A right or middle press does nothing either, so it cannot cancel the capture. Handles and edges always resize, whatever tool is out.
   - Esc steps back one layer at a time: typing, then a selected mark, then the tool. With marks left, the first press shows "Press Esc again to discard N annotations". A second press within `DISARM_AFTER` (3 s) cancels the capture. The warning shows beside the toolbar. The ✕ cancels at once: a click on it is a choice, where Esc is also the key that steps back.

**Rejected.**
- **An editor over the whole desktop.** The editor would hold the full desktop as its base image and render all of it. ARCHITECTURE §1.2 caps the peak memory of region capture on two 4K monitors at under 150 MB.
- **An origin stored on the document.** The document would subtract its origin in every render, hit test and effect. The result is the same as moving the marks once in `Reframe`, but every one of those paths would change.
- **Keeping the lock.** The selection could not be adjusted after the first mark, which is the problem this ADR removes.

**Consequences.**
- Marks partly outside the frame are clipped on export. A plain move cannot put them there; an Alt move or a resize can.
- A mark fully outside the frame survives in the editor and returns when the frame moves back over it.
- `Lock`, `Unlock` and `RegionView::locked` are removed. ADR 029 decision 1 keeps the old lock text as history, with the amendment above.
