# crucible-code

A terminal coding agent in Rust. This file holds the rules a reviewer will
hold a change to that neither the compiler nor the gate can tell you.
Repository skills are in [`.agents/skills/`](.agents/skills/).

## Finding the owner

Every crate's `src/lib.rs` opens by saying what the crate is for, so
`head -n3 crates/*/src/lib.rs` is the map. Before changing code, read the
module documentation above it: it states the local invariants, and when your
change makes one of its sentences false, update it in the same change.

`python3 scripts/python/crate-edges.py Cargo.toml Cargo.toml crates/*/Cargo.toml`
prints which crate depends on which. The edges allowed, each with its reason,
are the list in the crate-layering section of `scripts/sh/repo-checks.sh`; a
new edge is a deliberate edit to that list. New code names the crate that owns
what it uses.

What the tree does not show:

- `crucible-app` is the composition root. It builds the concrete providers,
  tools, sandboxes and storage and hands them downward as trait objects, and
  `src/` adapts a terminal to what it hands back. `crucible-runner` drives
  domain traits and depends on no concrete provider, tool or other plugin.
- Open sets are traits. Adding a provider, tool, sandbox, subagent or skill
  loader never names its implementation in the crate that owns its contract,
  and a new vendor is a dialect of a wire that names no vendor. Closed domain
  states are enums, matched exhaustively wherever a new case must make every
  consumer decide.
- External text is parsed once, by its format's owner: provider wire objects
  in provider modules, tool arguments in the tool, configuration in config and
  session lines in session.

## Invariants

- **Permission is a type.** A privileged operation takes `Approved`, bound to
  the exact call the permission engine settled; never a bool or a verdict
  anyone can construct. Give every distinct domain meaning its own type. A new
  privileged plugin route gets tests equivalent to the existing typed-boundary
  tests.
- **Front ends are clients.** The terminal, and a consumer with no terminal,
  reach a conversation through `crucible_app::client` with a
  `crucible-client-api` request, and neither can do what the other cannot.
  `crucible-app` translates runner events and errors into the contract's
  bounded values with a match written out by hand; never serialize them as
  they stand. `crucible-client-api` depends on `crucible-types` alone. A
  client's decision names a pending action and is held against it: it is never
  a permission and cannot construct `Approved`.
- **Secrets are applied, never shown.** A credential-bearing value redacts its
  `Debug` and stays out of `Display`, errors and session logs, and the exact
  representation sent out is registered so responses can be redacted. A stored
  credential's name is its `/login` row (the bare provider name for a row
  0.43.3 knows, else `provider@site`) and never changes once shipped.
- **Nothing goes to a vendor before a yes.** A vendor that uses what it is sent
  is sent nothing until the user agrees: every client that can reach a vendor
  takes `crucible_app::content_use`'s hold. `scripts/sh/repo-checks.sh` lists
  each file that builds a client, and why; a new one goes on that list.
- **Input is hostile.** Model output and checked-out files are hostile input.
  Every path is checked for reach through `Workspace`. Running a process is the
  one exception, and it is classified by what it will run.
- **Everything kept is bounded.** Bound retained or streamed data before
  storing a byte, with the ceilings of the owner involved, and report
  truncation as incomplete. The transcript is the one value meant to grow with
  the session; build no second session-sized copy of it.
- **One thread draws.** Only the drawing thread writes terminal output; other
  threads report events to it. A component fits the current width and the room
  available, and joins the fit sweep. A terminal mode is set by a guard that
  restores it from `Drop` and does nothing when output is redirected.
- **Coupled state has one source.** Keep both sides in one registry or one
  exhaustive match. When one source cannot own both, add an agreement test, as
  configuration and schema, provider registry and construction, session shape
  and format number, component signatures and the fit sweep, and performance
  budgets and probes already have.

## Keeping the surfaces whole

A change that adds or changes what a user can see or set looks at each
surface below, and updates every one it reaches in the same change.

- The user docs under `docs/`, the README's first-run section, contributor
  setup in `CONTRIBUTING.md`, and `CHANGELOG.md`.
- A boolean environment variable accepts exactly `true`, `false`, `1` and `0`,
  and refuses any other value with a message that names those four.
- Content added to a request gets a category in `/context`.
- A new kind of usage or limit a provider reports is shown in `/usage`.
- A new configuration key gets a `/settings` row or an exclusion that says why;
  `every_declared_key_has_a_settings_row_or_a_reason_it_has_none` fails until
  it has one.
- A refusal that means "come back later" is classified by the provider module
  that owns the wire, and is never retried as if it were about now.

## Dependencies

Before adding a crate, look in `[workspace.dependencies]` for one that already
fits, and check whether `std` covers the need. Write it yourself only when it
needs no protocol, parser or platform branching; otherwise add the crate:

- Declare it in the root `Cargo.toml` under `[workspace.dependencies]`, pinned
  exactly (`=1.2.3`), with a comment beside it saying what it supplies that
  `std` does not. The member that uses it writes `some-crate.workspace = true`.
- Put it in the narrowest crate that needs it; one in `crucible-types` reaches
  every crate.
- Take it from crates.io. A git dependency is not a way around the pin.
- It must not panic or print on a shipped path: `unwrap_used`, `expect_used`,
  `panic`, `indexing_slicing`, `print_stdout` and `print_stderr` are denied.
  Keep its typed errors in a `thiserror` enum with `#[from]` rather than
  erasing them.
- Weigh its build time, binary size and startup cost, and run the benchmarks
  below for a runtime change.
- Its licenses, direct and transitive, must be allowed by `deny.toml`. The
  binary ships under MIT; widening the allowed list needs its reason stated in
  the pull request.
- Run `cargo build` and commit the refreshed `Cargo.lock` with the manifest,
  as `chore(deps): add <crate> for <reason>`.

## Writing the change

Work lands on `dev`; `main` holds what shipped. Open a pull request against
`dev`: one against `main` is retargeted before review, because merging it would
put an unreleased change into the branch a tag is cut from. Only a release
branch and a hotfix target `main`, and [`RELEASING.md`](RELEASING.md) owns both.

Write about shipped behavior and why the reader cares:

- A commit has a conventional subject and at most one short paragraph saying
  why. Long reasoning goes beside the code or in a focused design document.
- A changelog entry is a bold lead and at most three sentences, for someone
  deciding whether to upgrade. A version section opens with a summary written
  the same way, and the release notes are that summary, the comparison link
  and a link to the changelog.
- Shipped comments, docs, schemas and manifests carry no internal planning
  identifiers and no harness paths.

## What a pull request must satisfy

Read the whole of
[`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md) before
filling any of it in, then answer every section with one short paragraph. A
section left blank, answered with the prompt text, or answered with a summary
of the diff instead of what it asked is worse than no pull request: it spends a
reviewer's attention and returns nothing, and the person whose name is on it
pays for that.

- **One reason.** Unrelated changes are split, and a batch opened by pointing
  an agent at a list is closed on sight. Implementation and proof that cannot
  compile apart stay one change.
- **A named failure.** A panic, an assertion, a wrong screen, a measurement;
  for a behavior change, the test that failed before it. "It could break" and
  "a review tool flagged it" are not problems, and a change with no observed
  failure behind it gives Proof nothing to hold.
- **Prior art accounted for.** Search open **and** closed pull requests and
  issues for the same problem or area first. If it was tried, say what is
  different now; if it is a duplicate, say so instead of opening another.
- **Provenance disclosed.** Say what produced the change: a person's hand, or
  an agent, with its exact model id, the harness and its version, and every
  plugin loaded. It is weighed, not policed, since a claim reasoned out of
  documentation reads differently from one a real session produced; hiding it
  is what closes a pull request.
- **A person answering for it.** The complete diff reaches the person whose
  name is on it. Tick the human-review box, naming them in the table, only once
  they have said they read the diff or authorized the pull request; otherwise
  leave it empty and name who is being asked. A tick nobody gave is a false
  statement about a person.

Finishing, review and publication follow [`CONTRIBUTING.md`](CONTRIBUTING.md)
and [`RELEASING.md`](RELEASING.md).

## Repository checks

Before calling a change done, run the gate:

```bash
scripts/sh/check.sh
```

It runs the Rust, repository and Python gates. Use
[run-the-gate](.agents/skills/run-the-gate/SKILL.md) to read a failure, and
whenever you add or change a check. Run `scripts/sh/bench.sh` when startup,
rendering, searching, retained session data or hot-path allocation changes. A
budget moves only by an explicit product decision; never widen one to make a
change pass.

Whatever a run writes goes under `generated/`, which git ignores whole: a
single document under the directory for its format, a tree under a directory of
its own. Nothing there is committed. Platform matrices, dependency policy,
advisories, performance and releases have their owners in
[`.github/workflows/README.md`](.github/workflows/README.md).
