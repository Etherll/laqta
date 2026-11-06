# ADR 008: Workflow overrides as Option patches

Status: accepted (2026-11-07)

**Context.** ShareX pairs every task-settings section with a `UseDefault*` boolean.

**Decision.** A workflow holds a `TaskSettingsPatch` whose fields are `Option`s. `TaskSettings::effective(defaults, patch)` merges them.

**Consequences.** The importer maps `UseDefault* = true` to `None`. The settings UI shows overrides as differences from defaults.
