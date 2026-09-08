# crucible-code

A terminal coding agent in Rust. `AGENTS.md` is the canonical repository guide;
`CLAUDE.md` links here so every coding harness reads the same instructions.

Read the policies in [`.agents/rules/`](.agents/rules/) before working.
They apply to every harness, including those that do not discover rules folders.
Repository skills live in [`.agents/skills/`](.agents/skills/); `.claude/` links
back to those canonical sources. Keep one owner for each instruction.

This guide adds Crucible-specific constraints. Use the installed SDLC skills
for the general development workflow; do not duplicate their planning, debugging,
TDD, review, verification or branch procedures here.

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

The workspace manifests declare the current crate graph;
`scripts/repo-checks.sh` checks it. This map is navigation, not another spec.

## Changing Crucible

Read the module documentation above code you will change; it owns the local
invariants. Update it when the implementation makes a sentence false.

- Open extension sets use traits: adding a provider, tool, sandbox, subagent or
  skill loader must not require naming its implementation in `crucible-core`.
  Closed domain states use enums, matched exhaustively where new cases require
  every consumer to decide.
- `src/` composes concrete implementations into trait objects before passing
  them downward. `crucible-runner` drives domain traits and must not depend on
  concrete providers, tools or other plugin implementations.
- Parse external text once at its format owner: provider wire objects in
  provider modules, tool arguments in tools, configuration in config and
  session lines in session.
- Give distinct domain meanings distinct types. Permission is not a bool or a
  freely constructible verdict: privileged operations require `Approved`, bound
  to the exact call the permission engine settled. Protect new privileged
  plugin routes with tests equivalent to the existing typed-boundary tests.
- Apply secrets without exposing them. Credential-bearing values redact
  `Debug`, stay out of `Display`, errors and session logs, and register exact
  outgoing representations for response redaction.
- Treat model output and checked-out files as hostile input. Validate path
  reach through `Workspace`; process execution is the explicit exception and
  must be classified by what it will run.
- Bound new retained or streamed data before storing bytes, using the relevant
  owner's ceilings. Report truncation as incomplete. The transcript is the
  runner's intentionally growing value; do not create another session-sized copy.
- Only the drawing thread writes terminal output; other threads report events.
  Components fit the current width and available room and join the fit sweep.
  Terminal modes use guards that restore them from `Drop` and do nothing for
  redirected output.
- Prefer one registry or exhaustive match for coupled state. When one source
  cannot own both sides, add an agreement test: configuration/schema,
  provider registry/construction, session shape/format number, component
  signatures/fit sweep and performance budgets/probes are existing examples.

## Dependencies

Use the SDLC scope and reuse checks before choosing a dependency. For this tree,
inspect `[workspace.dependencies]` for an existing fit. A small local solution
is appropriate only when it needs no protocol, parser or platform branching.
A necessary dependency must not become a reason to leave a feature incomplete.

- Declare each third-party crate in root `Cargo.toml` under
  `[workspace.dependencies]`, with an exact `=1.2.3` version and a nearby comment
  explaining what it supplies that `std` does not. The consuming member uses
  `some-crate.workspace = true`; versions and justification live at the root.
- Put the dependency in the narrowest crate that needs it. A dependency in
  `crucible-core` affects every consumer. New internal edges require a deliberate
  update to the graph checked by `scripts/repo-checks.sh`.
- Use crates.io sources. Git dependencies are not an escape from version pins.
- The dependency must support shipped paths without panicking or printing to
  stdout/stderr. `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`,
  `print_stdout` and `print_stderr` are denied. Preserve typed errors that a
  `thiserror` enum can hold with `#[from]`, rather than erasing them.
- Account for compile time, binary size and startup budgets in
  `scripts/bench.sh`. Its budgets change only by an explicit product decision.
- Check direct and transitive licenses against `deny.toml`; changing the allowed
  list requires an explicit rationale in the pull request. The distributed
  binary is MIT-licensed.
- Run `cargo build` to refresh `Cargo.lock` and commit it with the manifest
  changes. Use `chore(deps): add <crate> for <reason>` for an added dependency.
  Dependabot proposes pin updates; blocking CI checks licenses and sources with
  cargo-deny. Advisory checks have their own workflow, including scheduled runs.

## Writing the change

Keep repository prose focused on shipped behavior and why the reader cares:

- Commit: a conventional subject and at most one short paragraph explaining why.
- Changelog: a bold lead and at most three sentences for someone deciding
  whether to upgrade. Release notes reuse that entry.
- Pull request: follow the template, with one short paragraph per section and
  the test that failed before a behavior change. Keep one reason per PR;
  implementation and proof that cannot compile apart remain one change.

Update affected user docs, the first-run README surface, contributor setup and
changelog in the same change. Put long reasoning beside the code or in a focused
design document, not a commit narrative. Do not put internal planning identifiers
or harness paths into shipped comments, docs, schemas or manifests.

## Repository checks

Use [run-the-gate](.agents/skills/run-the-gate/SKILL.md) when changing a check or
preparing to finish. The compatibility gate is:

```bash
scripts/check.sh
```

It aggregates deterministic Rust, repository and Python gates. Run
`scripts/bench.sh` when startup, rendering, searching, retained session data or
hot-path allocation changes; never widen a budget to accommodate a regression.
Platform matrices, dependency policy, advisories, performance and releases have
owners in [`.github/workflows/README.md`](.github/workflows/README.md).
