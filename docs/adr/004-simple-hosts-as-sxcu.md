# ADR 004: Simple hosts as bundled .sxcu data

Status: accepted (2026-11-07)

**Context.** About 40 ShareX hosts are a single HTTP request with a simple response.

**Decision.** Those hosts ship as `.sxcu` files in `laqta-upload/catalog/`. Only hosts that need OAuth, request signing, custom crypto or a non-HTTP protocol get Rust code.

**Consequences.** Adding or fixing a simple host is a data change. The `.sxcu` engine must support every ShareX syntax function.
