# ADR 006: JSON settings, SQLite history

Status: accepted (2026-11-07)

**Decision.** Settings are versioned JSON with ordered migrations, atomic writes and rotating backups. History is SQLite with WAL and a disk thumbnail cache. Both live in `laqta-library`.

**As built.** There is no disk thumbnail cache yet: `History::set_thumbnail` exists but nothing calls it, and `laqta-ui` decodes thumbnails into an in-memory cache.

**Backup import is staged.** The settings window never unpacks a backup into the data folder while the app runs: the history database is open (Windows refuses to replace it) and the running copy would overwrite `settings.json` on its next save. `laqta-library` checks the archive and keeps a copy as `import-pending.zip` (`stage_archive`); `apps/laqta` applies it at the next start, before the settings are read and the history database opens (`apply_staged_import`). It removes the copy once applied, and also when the archive itself is bad (damaged, or with an unsafe entry name), so a broken archive cannot fail every start; an I/O error (a locked file, a full disk) keeps it, so the next start tries again. The settings it replaces are kept as `backups/settings-before-import.json`.

**Saving.** The save path refuses a new issue `laqta_domain::settings::validate` reports (a screenshots folder that is not an absolute path, an auto-capture interval outside its bounds) before it writes or applies anything. The settings read at start are repaired first (`repair_loaded`), so an issue already in the file never blocks a save the user did not make about it. The shortcuts switch that the tray and the "disable shortcuts" key flip goes through the same save path, so it lasts across a restart.

**Consequences.** ShareX's JSON configs map closely, which helps the importer. Rejected: TOML (worse fit for the ShareX config structures that the importer reads) and embedded key-value stores (no ad-hoc querying).
