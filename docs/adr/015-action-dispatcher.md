# ADR 015: One Action dispatcher

Status: accepted (2026-11-07)

**Context.** The first plan listed about 50 interactors; most were one-call pass-throughs (pause recording, hash a file, resize images). ShareX routes every hotkey action through one `ExecuteJob`.

**Decision.** `laqta-app` exposes `dispatch(Action, Option<WorkflowId>)`. Hotkeys, the tray, windows, the CLI, IPC, native messaging and watch folders all call it. It routes to Task, Region session, Recording session, Watch folders, a pure tool (decode, `laqta-imaging`, encode), or a windowed tool opened through the `ToolHost` port.

**As built (UX1 D1).** `dispatch_with(Action, ActionInput, Option<WorkflowId>)` is the same entry for a delivery mechanism that already holds the action's input. `ActionInput` is `None`, `Files`, `Text` or `Url`; the upload actions use it in place of asking the user. Watch folders dispatch `FileUpload` with the folder's workflow this way.

**Consequences.** Delivery mechanisms share one interface. A routing table test covers every `Action`.
