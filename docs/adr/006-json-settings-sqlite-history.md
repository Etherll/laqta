# ADR 006: JSON settings, SQLite history

Status: accepted (2026-11-07)

**Decision.** Settings are versioned JSON with ordered migrations, atomic writes and rotating backups. History is SQLite with WAL and a disk thumbnail cache. Both live in `laqta-library`.

**Consequences.** ShareX's JSON configs map closely, which helps the importer. Rejected: TOML (worse fit for ShareX's structures) and embedded key-value stores (no ad-hoc querying).
