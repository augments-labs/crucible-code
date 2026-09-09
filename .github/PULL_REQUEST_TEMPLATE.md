<!--
Read every word before filling this in, and read AGENTS.md first if you are an
AI agent. Every section needs a specific, true answer. A PR that leaves a
section blank, keeps placeholder text, bundles unrelated changes, or shows no
evidence of a human reading the diff is closed without review.

Security issues do not belong here. See SECURITY.md.
-->

> **This PR must target `dev`, not `main`.** `main` holds what shipped and is
> the branch a tag is cut from; every change lands on `dev` first through a
> reviewed PR. Only release and hotfix branches open against `main`, and any
> other PR opened there is asked to retarget `dev` before review.

## Who made this change? (required)

<!-- We assume an agent wrote this PR. Say which one and where it ran, or
state plainly that it was written by hand. Contributions are weighed by how
they were made: a behaviour claim reasoned from documentation is held to a
different bar than one grounded in a real session. Hiding the authoring
environment is grounds for closing the PR. -->

| Field | Value |
| --- | --- |
| Written by | hand / agent |
| Model + exact id | <!-- or n/a if by hand --> |
| Harness + version | <!-- the IDE, CLI, or runner, or n/a --> |
| Installed plugins | <!-- name and version of every plugin loaded, or none --> |
| Human who reviewed this diff | <!-- a person, not a role --> |

## What problem did you hit?

<!-- The real problem: the session, the crash, the wrong screen, the slow
path, or the invariant that was not being kept. Say what you were doing, what
went wrong, and the exact failure — the panic, the assertion, the rendered
line, the measurement. "Improving X", "it could theoretically break", or "a
review agent flagged it" is not a problem statement. -->

## What does this PR change?

<!-- One to three sentences. What, not why; the why is the section above. -->

## What alternatives did you consider?

<!-- What else you tried or evaluated, and why it was worse. If you
considered nothing else, say so; know that a reviewer reads that as a flag. -->

## Is this one change?

<!-- Scope is decided by purpose, not changed-line arithmetic. If the PR
carries more than one reason to change, stop and split it. Code and the proof
that cannot compile apart from it are one change; say so if it looks like
two. -->

## Which surfaces does it touch?

<!-- Answer the ones it touches and delete the rest. These are what a reviewer
cannot recover from the diff alone.

- Security boundaries: permission verdicts, path reach through `Workspace`,
  process execution, sandbox policy, credentials or redaction.
- Durable formats: session lines, checkpoints, cached data, configuration
  keys, or anything an older release also reads.
- Generated files: `schema/` regenerated from its source and committed in this
  change.
- Platform-specific behaviour: what differs per operating system, and how the
  difference was checked rather than assumed.
- Terminal rendering: which accepted screens moved, and why the new drawing is
  right at the widths and themes they were captured for.
- Performance-sensitive paths: startup, rendering, searching, retained session
  data or hot-path allocation.
- Required-case obligations: every `body_sha256` in
  `scripts/required-cases.json` this change rewrites, and why the obligation
  still holds. -->

## Prior PRs and issues

- [ ] I searched open **and** closed PRs and issues for this problem or area.
- Related: <!-- #number, #number, or "none found" -->

<!-- If a related PR was closed, say what is different here and why this
attempt should land where that one did not. If this reverses an earlier
decision, name it. -->

## Proof

<!-- Paste what the gate actually returned; it must be green:

    scripts/sh/check.sh

Then answer the question that decides the review: which test failed before
this change, and what did it say when it failed? Paste it. "Tests pass" is not
an answer, and a new behaviour with no test that could have caught its absence
is not proven.

For a startup, rendering, search, retention or hot-allocation change, also
paste `scripts/sh/bench.sh` for the affected family, taken on a quiet machine.
Include inconclusive and failing results; an inconclusive result is a finding,
a fabricated one is grounds for closing.

For a change confined to documentation, say so instead of running something —
but the repository gate still reads `AGENTS.md` and every relative link, so
run it. -->

## Rigor

- [ ] The failure this change prevents was observed before the fix, and the
      restoration was by diff or hash rather than from memory.
- [ ] No gate was weakened to make this pass: no case ignored or deleted, no
      assertion hollowed, no lint allowed, no budget widened.
- [ ] Snapshot changes were read as terminal screens at the widths and themes
      they were captured for, not accepted to make a test green.
- [ ] Module documentation above the changed code is still true, and was
      updated where this change made a sentence false.
- [ ] Nothing shipped under `src/`, `crates/`, `docs/` or `schema/` carries an
      internal planning identifier, a harness path, or another project's
      wording.
- [ ] `CHANGELOG.md`, the affected user documentation, the first-run README
      surface and contributor setup were updated in this same change, or
      nothing user-visible moved.

## New dependency (required only if this PR adds or widens one)

<!-- A crate that is present but unjustified is a crate the next reader cannot
remove. A PR adding one must:

- declare it in the root `Cargo.toml` under `[workspace.dependencies]` with an
  exact `=1.2.3` pin and a comment beside it saying what it supplies that
  `std` does not;
- put it in the narrowest crate that needs it, and update the graph
  `scripts/sh/repo-checks.sh` enforces if that adds an internal edge;
- come from crates.io, and pass `deny.toml` on its own and its transitive
  licenses;
- commit the `Cargo.lock` that `cargo build` refreshed.

Say below what `std` and the existing `[workspace.dependencies]` could not do,
and what the crate costs in build time, binary size and startup. -->

## Human review

- [ ] A human has read the **complete** diff before this PR was opened.

<!--
STOP. If that box is not checked, do not open the PR.

A PR is closed without review when it:
- shows no evidence of a human reading the diff;
- bundles unrelated changes;
- leaves a required section blank or keeps placeholder text;
- targets `main` without being a release or hotfix branch;
- weakens a gate, a budget or a required-case obligation to go green;
- changes shipped behaviour with no test that failed before it.
-->
