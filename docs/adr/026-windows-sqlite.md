# ADR 026: SQLite from vcpkg on Windows

Status: accepted (2026-11-07), amends ADR 006 for the Windows build (`rusqlite` was `bundled`).

**Context.** `rusqlite`'s `bundled` feature compiles SQLite's C source in a build script. The required
Windows check (`cargo check --workspace --target x86_64-pc-windows-msvc`) runs on Linux without the
MSVC C runtime headers, so that build script fails there (the same problem as ADR 025).

**Decision.**
- Linux: `rusqlite` with `bundled` (FTS5 included), as before.
- Windows: `rusqlite` without `bundled`; `libsqlite3-sys` finds a static SQLite through vcpkg
  (`sqlite3[fts5]:x64-windows-static-md`, `VCPKGRS_TRIPLET=x64-windows-static-md`). The Windows CI job
  installs it first. No release workflow exists yet; it must do the same. Nothing is downloaded at run time, and the binary has no
  extra DLL.

**Consequences.** Windows builds need vcpkg on the build machine. The SQLite version on Windows follows
the vcpkg baseline; `laqta-library`'s schema uses only features both builds have (WAL, FTS5).
