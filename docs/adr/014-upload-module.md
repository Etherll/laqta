# ADR 014: One Upload module behind Destination

Status: accepted (2026-11-07)

**Context.** The first plan split uploading across a "pure" `.sxcu` evaluator in the domain, an engine crate, a hosts crate, an HTTP port and transport ports. The evaluator is not pure: `{select}`, `{inputbox}` and `{outputbox}` open UI, `{random}` needs randomness, `{response}` and `{header:}` need the HTTP response, and `UseNameParser` adds name parsing. The FTP, SFTP, SMTP and HTTP ports each had one adapter.

**Decision.** `laqta-upload` holds everything behind `Destination`, `Shortener` and `Sharer`: the `.sxcu` engine and catalog, native hosts, OAuth and transports. `Prompt`, `Rng` and `SecretStore` are injected. HTTP comes from the `laqta-http` library (ADR 007).

**Consequences.** Tests use a local HTTP server and recorded fixtures at the `Destination` interface. Do not add transport ports unless a second adapter appears.
