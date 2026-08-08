---
name: run-the-gate
description: >-
  Run scripts/check.sh and act on what it reports. Use before every commit, and
  before claiming any change is done. Also covers adding a new gate — a gate
  nobody has watched fail is not a gate.
---

# Run the gate

```bash
scripts/check.sh
```

One command, ten sections, no arguments. CI runs this exact script, so a green
run here is a green run there. It is also the whole standard: a rule that cannot
be expressed here, in `Cargo.toml [workspace.lints]`, or in `clippy.toml` is not
enforceable and does not exist.

Every section is a property of the source, which is what lets that promise hold:
the same tree gives the same answer on any machine on any day. A check whose
answer moves on its own does not belong here — see below.

## Reading a failure

| Section | What it means when it fails |
| --- | --- |
| `rustfmt` | `cargo fmt --all` fixes it. Never hand-format around it. |
| `clippy` | Warnings are errors. Fix the code; an `#[allow]` needs a comment saying what the lint got wrong. |
| `tests` | Read the assertion, not the count. |
| `file length` | Split by responsibility, not by line count. Two halves that must always change together are still one file. |
| `agent rules files` | `AGENTS.md` stopped being a symlink to `CLAUDE.md`. Restore it; never let two copies exist. |
| `agent skills` | A skill under `.claude/skills/` lost its `.agents/skills/` symlink, or one of those became a real directory. |
| `dependency pinning` | A crate in `[workspace.dependencies]` is not `=`-pinned. See the `add-a-dependency` skill. |
| `dependency justification` | A crate has no comment above it saying why it is needed. One comment covers the group beneath it; a blank line starts a new group. |
| `github actions pinning` | An action is referenced by tag. Pin the commit sha, keep the version in a trailing `# vX.Y.Z` comment. |
| `benchmark gate` | You wrote the first bench probe, so `scripts/bench.sh` can now fail for a real reason. Delete `continue-on-error` from the budgets job in `.github/workflows/ci.yml` and let the budget block the merge. |

## Adding a gate

A new check is not finished when it passes. It is finished when you have seen it
fail for the reason it exists:

1. Write the check.
2. Break the tree deliberately, in the exact way the check is meant to catch.
3. Run `scripts/check.sh` and read the `FAIL` line. If it did not fire, or fired
   with a message that would not tell someone what to do, fix the check.
4. Restore the tree — verify by hash, not by memory.
5. Run the gate again and see it green.

Do that for every branch the check has. The actions-pinning check has two (a tag
instead of a sha; a sha with no version comment), so it was falsified twice. The
benchmark gate has three, and the third is the one that mattered: probe present
with the escape hatch still there fires, probe absent does not, and probe present
with only the *comment* mentioning `continue-on-error` must not fire either —
that last branch is what turned a `grep` for a word into a `grep` for the key.

## What the gate does not cover

- **The performance budgets.** Those are `scripts/bench.sh`, and `RELEASING.md`
  blocks a tag on them. All this gate checks is that CI has stopped excusing
  them once there is something to measure.
- **Advisories, licences and sources.** Those are `cargo deny check` against
  `deny.toml`, run by `.github/workflows/audit.yml` weekly and on any change to
  the dependency set. Deliberately not here: an advisory appears when somebody
  else publishes one, so the same tree would pass today and fail tomorrow —
  which is precisely the promise this script makes and must not break. Run it
  by hand when you touch a dependency.
- **Markdown, YAML and the README.** Nothing checks them; read them.
- **Whether the change is right.** Green means it did not break the rules the
  project can express mechanically. It says nothing about whether the behaviour
  is the behaviour someone asked for.
