# ADR 007: Tokio only inside laqta-http

Status: accepted (2026-11-07)

**Context.** `reqwest` needs Tokio. A runtime in every crate costs idle memory and ties the application to one executor.

**Decision.**
- `laqta-http` starts a single-thread Tokio runtime on its own thread at the first request, and bridges results to callers through channels.
- `reqwest` is used with `default-features = false` and the `rustls`, `http2`, `stream` and `multipart` features. Version 0.13 otherwise defaults to OpenSSL on Linux.
- `laqta-http` also hosts the SFTP client (`russh` + `russh-sftp`), which needs Tokio, and exposes it to `laqta-upload` through the same channel bridge.
- The application uses its own executor (ADR 020).

**Rejected.**
- **`ureq` on a blocking pool:** cancellation and upload progress need a reader wrapper, and HTTP/2 is not supported.
- **smol-based HTTP clients (`isahc`, `surf`):** they are less maintained and depend on libcurl or have weaker TLS options.
- **A Tokio runtime started at launch:** it costs memory while idle.
- **`ssh2` (libssh2) for SFTP on the blocking pool:** it avoids Tokio but adds a C library and OpenSSL.

**Consequences.** No Tokio memory is used until the first network call. `laqta-upload`, the AI analyzer, the update checker and the FFmpeg downloader share one runtime, released by the lazy-resource timer.
