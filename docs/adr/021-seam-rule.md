# ADR 021: The seam rule

Status: accepted (2026-11-07), amended 2026-11-07 after the trade-offs review (file access)

**Context.** The first rule counted any test fake as a second adapter. Every trait has a fake, so the rule excluded nothing, and it contradicted ADRs 014 and 018.

**Decision.** A port exists only if it has either:
- two production adapters (adapters for different platforms count), or
- one production adapter plus a fake that a deep module's tests need.

Checking the port table against this rule led to these changes:
- **`Transcoder`** merged into **`Media`** (record and transcode). Its fake is needed by the Recording session tests, and a native Windows recorder is planned.
- **`BackgroundRemover`** removed at first, because it had one adapter. ADR 023 restores it with two production adapters: local model families in `laqta-ml`, and a cloud API.
- **`ImageCodec`** removed earlier. Codecs are pure and live in `laqta-imaging`.
- No file-system port. `laqta-app` uses `std::fs` only inside its private async `files` module (ADR 024), and tests use a temporary directory.
- **`ToolHost`** added. It is the single port for opening windowed tools, and its fake is needed by the dispatcher tests.

**Rejected.** Counting any fake: see Context.

**Consequences.** The port table in ARCHITECTURE.md §2.3 lists each port's production adapters and the tests that need its fake, so the rule can be checked during review.
