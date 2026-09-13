# crucible-code

A terminal coding agent in Rust.

Repository skills live in [`.agents/skills/`](.agents/skills/).

## Repository map

| Directory | Purpose |
| --- | --- |
| `src/` | CLI and application wiring |
| `crates/crucible-types/` | Shared validated values every crate exchanges |
| `crates/crucible-registry/` | Bounded, source-aware registries |
| `crates/crucible-credentials/` | Credential contracts and outgoing redaction |
| `crates/crucible-storage/` | History, checkpoint and cache contracts |
| `crates/crucible-workspace/` | The directories crucible reaches, and path proofs |
| `crates/crucible-attachments/` | What may be attached, and the one read it comes through |
| `crates/crucible-runtime/` | The controls a turn is steered and stopped by, and how work is owned |
| `crates/crucible-sandbox/` | What a confined process may observe or change |
| `crates/crucible-sandbox-local/` | The confinement this machine can enforce, and the processes it runs |
| `crates/crucible-tools/` | What a tool is, what may run one, and the proof that it may |
| `crates/crucible-core/` | Domain types and extension traits |
| `crates/crucible-auth/` | Credentials and account authorization |
| `crates/crucible-builtins/` | Built-in tools |
| `crates/crucible-config/` | Configuration and settings |
| `crates/crucible-extension/` | External program integration |
| `crates/crucible-mcp/` | Model Context Protocol client |
| `crates/crucible-privacy/` | Protected local files |
| `crates/crucible-provider/` | Provider wire protocols |
| `crates/crucible-runner/` | Agent turn execution |
| `crates/crucible-sandbox-broker/` | Isolated child execution and status |
| `crates/crucible-session/` | Session storage and replay |
| `crates/crucible-tui/` | Terminal rendering and interaction |
| `schema/` | Generated configuration schema |
| `scripts/` | Checks, benchmarks and release helpers, under `sh/` and `python/` |
| `docs/` | User documentation |

Workspace manifests declare crate dependencies; `scripts/sh/repo-checks.sh`
enforces their allowed directions. `crucible-core` re-exports the names it no
longer defines, so a consumer keeps one import path while ownership moves out of
it; new code names the owning crate.

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

Before choosing a dependency, inspect `[workspace.dependencies]` for an existing
fit and check whether `std` covers the need. A small local solution is appropriate
only when it needs no protocol, parser or platform branching. Add a crate when
the required behavior justifies it; keep the declaration and checks below.

- Declare each third-party crate in root `Cargo.toml` under
  `[workspace.dependencies]`, with an exact `=1.2.3` version and a nearby comment
  explaining what it supplies that `std` does not. The consuming member uses
  `some-crate.workspace = true`; versions and justification live at the root.
- Put the dependency in the narrowest crate that needs it. A dependency in
  `crucible-core` affects every consumer. New internal edges require a deliberate
  update to the graph checked by `scripts/sh/repo-checks.sh`.
- Use crates.io sources. Git dependencies are not an escape from version pins.
- The dependency must support shipped paths without panicking or printing to
  stdout/stderr. `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`,
  `print_stdout` and `print_stderr` are denied. Preserve typed errors that a
  `thiserror` enum can hold with `#[from]`, rather than erasing them.
- Assess build time, binary size and startup impact. Follow the performance
  checks below for runtime changes.
- Check direct and transitive licenses against `deny.toml`; changing the allowed
  list requires an explicit rationale in the pull request. The distributed
  binary is MIT-licensed.
- Run `cargo build` to refresh `Cargo.lock` and commit it with the manifest
  changes. Use `chore(deps): add <crate> for <reason>` for an added dependency.
  Dependabot proposes pin updates; blocking CI checks licenses and sources with
  cargo-deny. Advisory checks have their own workflow, including scheduled runs.

## Writing the change

Work lands on `dev`; `main` holds what shipped. A pull request targets `dev` —
one targeting `main` is retargeted before review, because merging it would put
an unreleased change into the branch a tag is cut from. Only a release branch
and a hotfix target `main`, both owned by [`RELEASING.md`](RELEASING.md).

Keep repository prose focused on shipped behavior and why the reader cares:

- Commit: a conventional subject and at most one short paragraph explaining why.
- Changelog: a bold lead and at most three sentences for someone deciding
  whether to upgrade. Release notes reuse that entry.
- Pull request: follow the template, answering every section with one short
  paragraph and naming the test that failed before a behavior change.

Update affected user docs, the first-run README surface, contributor setup and
changelog in the same change. Put long reasoning beside the code or in a focused
design document, not a commit narrative. Do not put internal planning identifiers
or harness paths into shipped comments, docs, schemas or manifests.

## What a pull request must satisfy

Read the whole of
[`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md) before
filling any of it in. A section left blank, answered with the prompt text, or
answered with a summary of the diff instead of the thing it asked for is worse
than no pull request: it spends a reviewer's attention and returns nothing, and
it is the person whose name is on it who pays for that.

- **One reason.** Unrelated changes travelling together get split, and a batch
  opened by pointing an agent at a list is closed on sight. Implementation and
  proof that cannot compile apart remain one change.
- **A named failure.** A panic, an assertion, a wrong screen, a measurement.
  "It could break" and "a review tool flagged it" are not problems, and a change
  with no observed failure behind it has nothing for Proof to hold.
- **Prior art accounted for.** Open **and** closed pull requests and issues for
  the same problem or area are searched first. If it has been tried, say what is
  different here; if it is a duplicate, say so rather than opening another.
- **Provenance disclosed.** What produced the change: hand or agent, and for an
  agent the exact model id, the harness and its version, and every plugin
  loaded. This is weighed, not policed — a claim reasoned out of documentation
  is read differently from one a real session produced — and hiding it is what
  closes a pull request.
- **A person answering for it.** The complete diff reaches the person whose name
  is on it. The human-review box is theirs: ticked, with them named in the
  table, once they have said they read the diff or authorized the pull request;
  left empty otherwise, naming who is being asked. A tick nobody gave is a false
  statement about a person.

How a branch is finished, reviewed and published is owned by the skills in
[`.agents/skills/`](.agents/skills/); this file states what the result has to be.

## Repository checks

Use [run-the-gate](.agents/skills/run-the-gate/SKILL.md) when changing a check or
preparing to finish. The compatibility gate is:

```bash
scripts/sh/check.sh
```

It aggregates deterministic Rust, repository and Python gates. Run
`scripts/sh/bench.sh` when startup, rendering, searching, retained session data or
hot-path allocation changes. Budget changes require an explicit product decision;
do not widen them merely to make a failing change pass. What a run writes goes
under `generated/`, which git ignores whole: a document under the directory for
its format, a tree under a directory of its own. Nothing there is committed.
Platform matrices, dependency policy, advisories, performance and releases have
owners in [`.github/workflows/README.md`](.github/workflows/README.md).
