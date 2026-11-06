# ADR 017: Presenters are a GPUI-free module inside laqta-ui

Status: accepted (2026-11-07), revised after the EGM review

**Context.**
- A separate `laqta-presenters` crate produced contradictions. It was listed as an outer crate, but the CI guard treated it as inner.
- It needed history from `laqta-library`, which pulls in `rusqlite`.
- It held only four presenters.

**Decision.**
- Presenters live in `crates/laqta-ui/src/presenters/`, and only where they compute something: task-list rows, history paging, settings shown as differences from defaults, and the tray menu model.
- The CI guard rejects any `gpui` or `laqta_library` import in that folder. Views read history through `laqta-library`'s query functions, which return `laqta-domain` types, and pass those values to presenters.
- Simple forms bind to settings directly and are tested with GPUI's `TestAppContext`.

**Rejected.**
- **A separate crate:** see Context.
- **A view model for every screen:** a seam with one adapter, and every UI change would touch two places.

**Consequences.** `laqta-ui` reads history from `laqta-library` (an allowed outer edge, ARCHITECTURE.md §3).
