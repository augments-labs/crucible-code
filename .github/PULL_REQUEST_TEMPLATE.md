<!--
Read this through before filling it in, and read AGENTS.md first if you are a
coding agent. Every section wants a specific, true answer. A pull request that
leaves one blank, ships this prompt text in place of an answer, bundles
unrelated changes, or shows no sign that a person read the diff is closed
rather than reviewed.

Delete a heading only when this repository genuinely has no such surface. "n/a"
against a surface the change does touch is not an answer.

Security issues do not belong here. See SECURITY.md.
-->

> **This pull request targets `dev`, not `main`.** `main` carries what has
> shipped, and a tag is cut from it; every other change lands on `dev` first
> through a reviewed pull request. Only release and hotfix branches open
> against `main`, and anything else opened there is asked to retarget `dev`
> before review.

## Who made this change? (required)

<!--
Assume the reader assumes an agent wrote this. Say which one and where it ran,
or say plainly that it was written by hand. How a change was made is part of
how it is weighed: a claim reasoned from documentation is held to a different
bar than one a real session produced. Hiding it is grounds for closing the
pull request rather than reviewing it.
-->

| Field | Value |
| --- | --- |
| Written by | hand / agent |
| Model + exact id | <!-- or n/a if by hand --> |
| Harness + version | <!-- the IDE, CLI or runner, or n/a --> |
| Installed plugins | <!-- name and version of every plugin loaded, or none --> |
| Human who reviewed this diff | <!-- a person, not a role --> |

## Problem

<!--
What is wrong or missing today, for someone who has not been reading along.
The diff shows what changed; this says why anybody should want it changed.
-->

## Change

<!--
One paragraph on the approach and the alternative you rejected. Design
reasoning that outlives this page belongs beside the code or in a design
document, not here.
-->

## Prior work searched

<!--
Which issues and pull requests you looked for before writing this, and what
they said. A change that reverses an earlier decision names it.
-->

## Proof

<!--
The question that decides the review. Name the test, the command or the
session, and give the output. "Tests pass" is not an answer — which test failed
before this change, and what did it say when it failed?
-->

## Surfaces touched

<!--
Delete the lines that do not apply; keep and answer the ones that do.

- Security boundaries: permission verdicts, path reach, process execution,
  sandbox policy, credentials or redaction.
- Durable formats: session lines, checkpoints, cached data, configuration keys,
  or anything an older release also reads.
- Generated files: `schema/` regenerated from its source and committed.
- Platform-specific behaviour: what differs per operating system, and how the
  difference was checked.
- Terminal rendering: snapshot changes reviewed as screens, at the widths and
  themes they were captured for.
- Performance-sensitive paths: startup, rendering, searching, retained session
  data or hot-path allocation.
- Required-case obligations: any `body_sha256` in `scripts/required-cases.json`
  that this change updates, and why the obligation still holds.
-->

## Human review

- [ ] A human has read the **complete** diff before this pull request was
      opened, not a summary of it.

<!--
Leave the box empty if nobody has, and say who is being asked to. A green gate
is evidence about the checks that ran, not about whether the change answers the
right problem, so this is a separate thing a pull request either has or is
still waiting for.
-->

## Checklist

- [ ] Targets `dev` from a task branch, one reason to change per pull request
      (release and hotfix branches target `main`)
- [ ] `scripts/check.sh` passes
- [ ] New behaviour has a test that failed before the change; a fix has a test
      that reproduced the bug
- [ ] No performance budget moved, or the trade is explained above and
      `scripts/bench.sh` output is in Proof
- [ ] New dependencies are `=`-pinned with a comment in `Cargo.toml` saying why
- [ ] `CHANGELOG.md` and affected user documentation updated in this change
