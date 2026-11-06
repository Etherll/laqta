# ADR 009: Undo by commands

Status: accepted (2026-11-07)

**Context.** ShareX's editor keeps up to 50 full mementos.

**Decision.** The Editor engine records reversible commands. Pixel snapshots are stored only for destructive operations (crop, applied effects), under a byte cap.

**Consequences.** Undo memory scales with edits, not with image size times history depth.
