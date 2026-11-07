# ADR 025: SChannel TLS and system OpenSSH SFTP on Windows

Status: accepted (2026-11-07), amends ADR 007 and STACK.md §9.

**Context.**
- ADR 007 picks `reqwest` with rustls, and `russh` for SFTP. Both crypto providers rustls and
  russh accept (`aws-lc-rs`, `ring`) compile C code in their build scripts.
- The required check `cargo check --workspace --target x86_64-pc-windows-msvc` runs on Linux,
  where no MSVC C runtime headers exist, so those build scripts fail. Downloading the Windows SDK
  headers (`cargo-xwin`) needs accepting Microsoft's licence on the build machine.
- Windows already ships TLS (SChannel, using the OS trust store) and an OpenSSH client.

**Decision.**
- **HTTP TLS:** `laqta-http` enables `reqwest`'s `rustls` feature on Linux and its `native-tls`
  feature (SChannel through `schannel`; no OpenSSL) on Windows, as target-specific dependencies.
- **SFTP:** on Linux, `russh` + `russh-sftp` with `ring` on `laqta-http`'s runtime. On Windows,
  the SFTP host runs the system `sftp.exe` in batch mode (`-b`), with argv built by Laqta, never a
  shell. Key and agent authentication work; password authentication returns `Unsupported` on
  Windows and asks the user to set up a key.
- The port vocabulary does not change: `laqta-upload`'s SFTP host hides the backend.

**Rejected.**
- `cargo-xwin` or a vendored Windows SDK: licence acceptance on every build machine.
- Checking Windows only with `x86_64-pc-windows-gnu`: the goal requires the MSVC target.
- Pure-Rust TLS providers (`rustls-rustcrypto`): not production-ready.

**Consequences.**
- Two TLS stacks to keep in mind; both use the OS trust store on Windows (SChannel) and the
  platform verifier on Linux.
- Windows SFTP needs `sftp.exe` (present on Windows 10 1809+); its absence returns `Unsupported`.
- `cargo deny` keeps banning `openssl`; `native-tls` reaches it only on non-Windows targets, where
  the feature is off.
