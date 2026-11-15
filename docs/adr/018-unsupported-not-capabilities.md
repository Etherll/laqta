# ADR 018: Ports return Unsupported; settings and history are not ports

Status: accepted (2026-11-07)

**Context.**
- A capabilities struct that every caller checks spreads platform branching everywhere.
- `HistoryRepo` and `SettingsRepo` each had one real adapter, and a temporary directory and in-memory SQLite already serve as test stand-ins.
- An encrypted-file secret store had no safe place to keep its key.

**Decision.**
- A port that cannot do something returns `Unsupported`, and callers handle that one error. Capabilities are shown only in the settings UI and onboarding.
- The application receives settings as a value. History is written by a task-event subscriber in `laqta-library` and read directly by the UI. Edits to existing entries (replace tags, delete) go through `App::history_set_tags` and `App::history_delete` to a narrow `HistoryEdit` port, which `laqta-library`'s `History` implements; `main` attaches it with `App::attach_history`. It meets the seam rule (one production adapter and a fake the dispatcher tests need), and it is not a repository: it has no reads and no inserts.
- Tray and autostart are platform ports. All ports live in `laqta-ports` (ADR 024). `ModelService`, the settings window's model manager, is not a port: it is a seam defined in `laqta-ui` with a `main` adapter and a test fake.
- `SecretStore` has OS keyring adapters only. With no keyring, it returns `Unsupported`. `main` wraps the platform store in `SessionSecrets`, which keeps what the keyring refuses in memory for the session, so password and key hosts, the SMTP login and the native-messaging setup work until the process exits and are asked for again on the next launch. A working keyring keeps the only copy. OAuth tokens also have their own session cache in `laqta-upload`. Under `--sandbox` the store is an in-memory one.

**Rejected.**
- **Capability flags checked by callers:** the branching spreads everywhere (see Context).
- **Repository ports for settings and history:** each had one real adapter.
- **An encrypted file with a key stored next to it:** this gives no protection.

**Consequences.** The application has no storage interface beyond `HistoryEdit`'s two writes, so it stays storage-independent without a seam that has one adapter and no test need.

**As built (Windows window capture).** A transparent window capture that fails for any reason other than `Unsupported` (the window resized, a minimized or elevated window, a backdrop failure) is mapped by the Windows adapter to `PortError::Unsupported` and logged as a warning. The dispatcher then falls back to a plain crop of the window out of the screen frames; the adapter makes no fallback choice itself.
