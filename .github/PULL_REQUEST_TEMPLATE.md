<!--
Security issues do not belong here. See SECURITY.md.

Fill every section. Delete a heading only when this repository genuinely has no
such surface — never leave the prompt text in place of an answer, and never
answer a section with "n/a" where the change does touch what it asks about.

Pull requests target `dev`. `main` carries what has shipped, and only release
and hotfix branches open against it; anything else opened against `main` will
be asked to retarget before review.
-->

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

## Authoring environment

<!-- Delete the rows that do not apply. Say so plainly if a model wrote this. -->

| | |
| --- | --- |
| Written by | |
| Model | |
| Harness | |
| Harness version | |
| Human partner who reviewed this diff | |

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
