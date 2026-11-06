# ADR 018: Ports return Unsupported; settings and history are not ports

Status: accepted (2026-11-07), revised after the EGM review

**Context.**
- A capabilities struct that every caller checks spreads platform branching everywhere.
- `HistoryRepo` and `SettingsRepo` each had one real adapter, and a temporary directory and in-memory SQLite already serve as test stand-ins.
- An encrypted-file secret store had no safe place to keep its key.

**Decision.**
- A port that cannot do something returns `Unsupported`, and callers handle that one error. Capabilities are shown only in the settings UI and onboarding.
- The application receives settings as a value. History is written by a task-event subscriber in `laqta-library` and read directly by the UI.
- Tray and autostart are platform ports. All ports live in `laqta-ports` (ADR 024).
- `SecretStore` has OS keyring adapters only. With no keyring, it returns `Unsupported`, and credentials are asked for once per session and kept in memory.

**Rejected.**
- **Capability flags checked by callers:** the branching spreads everywhere (see Context).
- **Repository ports for settings and history:** each had one real adapter.
- **An encrypted file with a key stored next to it:** this gives no protection.

**Consequences.** The application has no storage interface, so it stays storage-independent without a seam that has one adapter.
