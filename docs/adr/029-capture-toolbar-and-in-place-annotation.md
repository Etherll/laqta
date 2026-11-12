# ADR 029: Capture toolbar and in-place annotation

Status: accepted (2026-11-09), amended by ADR 031 (text is read in place: decision 7, and `CopyText` in decision 3) and by ADR 032 (decision 1: `Lock` and `Unlock` are removed). Amends ADR 012 (the Region session no longer renders annotations) and ADR 013 (who receives `Fonts`). Replaces the earlier handoff to `EditorHost` for the Region session.

**Context.**
- ADR 012 had the Region session render annotations through the Editor engine. `laqta-app` never got the engine or `Fonts`, so the overlay only set an `annotate` flag on the selection and the editor opened afterwards in its own window.
- Annotating a region in place, with a toolbar beside the selection, is the usual flow in capture tools. The selection stays adjustable until the user chooses what happens to it.
- OCR, background removal, object erase and image analysis were reachable only as separate dispatcher actions, each starting with its own region pick, and the photo window called the AI ports directly.

**Decision.**
1. **A selected mode in the region state machine.** `RegionConfig::confirm_on_select` (set to `!show_tools_after_select`, a `RegionCaptureOptions` setting that defaults to on) decides whether a drawn or clicked region ends the session. When it is off, the region stays the active, adjustable selection, as in multi-region mode.
   - `RegionKey::Space` shows or hides the tools (`tools_visible`).
   - `RegionCommand::Finish(SelectionAction)` confirms the active region with an action. `Confirm` and `Annotate` keep their drawn-regions-only rule.
   - `RegionCommand::Lock` and `Unlock` freeze the selection once annotations exist, because moving the region would leave them behind. While locked, pointer and arrow-key edits do nothing, Enter finishes with `Default` and Escape cancels.
   - Amended by ADR 032: `Lock` and `Unlock` are gone. The selection stays adjustable with marks on it, and the marks move with the frame (`Command::Reframe`).
2. **`SelectionAction`** (a `laqta-domain` enum): `Default`, `Copy`, `Save`, `Pin`, `Upload`, `Edit`, `CopyText`. `RegionOutcome::Confirmed` and `Selection` carry it.
3. **The dispatcher maps the action to Task flags** (`after_capture_for`, ADR 011: behaviour is Task flags plus a golden trace). Copy sets only `COPY_IMAGE_TO_CLIPBOARD`; Save only `SAVE_IMAGE_TO_FILE`; Pin only `PIN_TO_SCREEN`; Upload `SAVE_IMAGE_TO_FILE | UPLOAD_IMAGE_TO_HOST`; CopyText only `DO_OCR`. Edit adds `ANNOTATE_IMAGE` to the configured steps, and Default leaves them alone. The after-upload steps are never touched. The golden trace `selection_action_copy.trace` covers one action.
4. **The overlay hosts the Editor engine; the app owns the pixels.**
   - When the selection changes, the Region session crops the composed desktop to it and passes the crop in `RegionView::selection_image` (with `selected` and `tools_visible`). It crops on a change of the selection only, not on every pointer move, and since ADR 032 only once a move or resize has ended.
   - The overlay adapter in `laqta-ui` runs the Editor engine on that crop through the same `EditorInteraction` presenter the editor window uses for `EditorHost`, so the overlay and the editor share one tool set. The toolbar's layout and key handling are in a GPUI-free presenter (`presenters/capture_toolbar`).
   - On an action the overlay sends the edited crop as `RegionInput::Annotated`, then `Finish(action)`. If an annotated crop exists when the session ends, `Selection::image` is that crop; otherwise it is the plain crop. A new selection drops any annotated crop.
   - `Fonts`, the engine and the GPU stay in `laqta-ui`. `laqta-app` takes no `laqta-editor` dependency (the edge stays allowed in `deps-policy.toml` and unused).
5. **Tools that act on an area at once use `region::pick`**: recording, scrolling capture, OCR, QR, colour picker, ruler and pin from screen. It runs the same session with `show_tools_after_select` off, so the selection confirms on release and no toolbar appears.
6. **AI use cases are `App` methods** (`laqta-app/src/ai.rs`): `recognize_text`, `remove_background`, `erase` (inpaint under a mask) and `analyze`. Each runs one port call, maps `PortError` to `AppError`, and emits nothing else. The image editor's smart tools, the photo editor and the library call these, never `App::ports()`. The `Ocr`, `BackgroundRemover`, `Inpainter` and `ImageAnalyzer` ports are unchanged.
7. **The OCR result surface is `ToolHost::show_ocr(image, result)`**, which defaults to `show_text`. `laqta-ui` implements it with a text window: the source image, editable text, Copy all and Save as text.
8. **Editor engine additions.** `Command::ReplaceImage(Bitmap)` swaps the document's image (undoable, like a destructive operation). `Tool::ObjectEraser` drags a box and, on release, asks the host to erase it (`EditorRequest::EraseObject { rect }`, in image pixels); the window builds the mask, calls `App::erase`, then issues `ReplaceImage`. The toolbar presenter's shortcuts follow the engine's key map (b blur, p pixelate, s speech balloon), and a test checks they agree.

**Rejected.**
- **The Region session hosting the engine (ADR 012's text).** It needs `Fonts`, the rasterizer and a `laqta-editor` edge inside `laqta-app`, and it would paint no pixels anyway: texture upload and input belong to the view. The pixels pass through the port instead.
- **Handing the crop to `EditorHost` before the session returns.** It keeps a second window between selecting and finishing, and the selection can't be adjusted while annotating.
- **A second tool set for the overlay.** The toolbar lists the engine's tools, so a new tool needs no overlay work.
- **Windows calling the AI ports through `App::ports()`.** Decision 6 routes every AI call through an `App` method instead.

**Consequences.**
- A region capture ends with the user's choice in `Selection::action`. The same flags can be set by a workflow, so a toolbar button and a configured after-capture step run the same Task code.
- Hosting the engine in the overlay makes the overlay adapter heavier. It must stay inside the view rules: the view forwards input and paints, and decisions live in presenters (ADR 017).
- ADR 012's wording ("annotations already rendered through the Editor engine") and ADR 013's `Fonts` clause (the Region session) no longer hold; both carry an "amended by ADR 029" note.
- Still open: `ANNOTATE_IMAGE` as a Task step that can abort, step order, and a blocking photo editor.
