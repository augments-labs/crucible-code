# crucible-code

A terminal coding agent in Rust.

`AGENTS.md` points here so coding harnesses share one repository entrypoint.
This file routes work; it does not repeat rules, skills, module documentation or
checks.

## Before changing code

- Read the module documentation in the files you will change.
- Use the `change-crucible` skill for implementation and architecture work.
- Use `add-a-dependency` when adding or widening a third-party crate.
- Use `write-the-change` for documentation, changelog, commit and pull-request
  writing.
- Use `run-the-gate` when adding a check or preparing to finish.

`.claude/rules/` contains the always-loaded policy, mirrored through
`.agents/rules`. Skills are written under `.claude/skills/` and mirrored through
`.agents/skills/`. Do not restate either here: one policy has one owner.

## Repository map

```text
src/                     binary composition and CLI
crates/crucible-core/    domain types and extension traits
crates/crucible-auth/    credentials and account authorization
crates/crucible-config/  configuration documents and settings
crates/crucible-extension/ runs somebody else's program and talks to it
crates/crucible-mcp/      speaks the Model Context Protocol to such a program
crates/crucible-privacy/ protected local-file primitives
crates/crucible-provider/ provider wire protocols
crates/crucible-runner/  turn execution over traits
crates/crucible-sandbox-broker/ frozen child-status protocol and PID 1 broker
crates/crucible-session/ append-only session storage and replay
crates/crucible-tools/   built-in tool implementations
crates/crucible-tui/     terminal rendering and interaction
schema/                  generated configuration schema
scripts/                 local gates, benchmarks and release helpers
docs/                    published user documentation
```

The current crate graph is declared in the workspace manifests and checked by
`scripts/repo-checks.sh`; this map is navigation, not a second specification.

## Finishing

Run the compatibility gate before handing work over:

```bash
scripts/check.sh
```

It aggregates the deterministic local Rust and repository gates. Performance,
platform matrices, dependency policy, advisories and releases have dedicated
workflow owners documented in [`.github/workflows/README.md`](.github/workflows/README.md).
