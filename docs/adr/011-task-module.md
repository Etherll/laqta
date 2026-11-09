# ADR 011: One Task module owns the task lifecycle

Status: accepted (2026-11-07)

**Context.** Some rules span several steps. The image is encoded once, and only when save, OCR, upload or analyze needs it. Thumbnails use the path the save step produced. Cleanup runs across both the capture and upload phases: it deletes the file and clears an early-copied URL. After-upload steps run only when a URL was expected and returned. The job kind changes the flow. An earlier design split this into a step plan, step objects, a shared mutable context, two after-* interactors, a queue and five upload interactors, which left those rules with no owner.

**Decision.** One Task module in `laqta-app`. Interface: submit a `TaskJob` with effective settings; receive `TaskEvent`s; stop one or all. The step plan, the steps, retries, cleanup, the status machine and the queue are private.

**As built.** `DELETE_FILE` runs when the job ends whatever the outcome: a failed or stopped task still deletes the file it wrote (golden traces `delete_file_after_failed_upload` and `delete_file_after_stop`). Copying the URL early, and clearing it when the task fails, is not built, and nor are upload filters.

**Consequences.** Tests are golden event traces ported from `WorkerTask.cs` scenarios, run against fake ports. New behaviour is a new flag handled inside Task plus a new scenario, not a new public step type. Do not re-split steps into a public plug-in API without a second consumer. Code outside Task that needs a step's behaviour (for example, a tool that wants "save and copy") submits a Task job. The trade-offs review (2026-11-07) re-confirmed this ADR as is.
