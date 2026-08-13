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

One command, sixteen sections, no arguments. CI runs this exact script, so a green
run here is a green run there. It is also the whole standard: a rule that cannot
be expressed here, in `Cargo.toml [workspace.lints]`, or in `clippy.toml` is not
enforceable and does not exist.

Every section is a property of the source, which is what lets that promise hold:
the same tree gives the same answer on any machine on any day. A check whose
answer moves on its own does not belong here — see below.

**Every section runs, every time.** A failure does not end the run, so one run
reports everything that is wrong with the tree, and the list at the foot names
the sections that failed. Work up that list rather than re-running after each
fix: the sections are independent, and only the list is the whole answer.

**One section writes.** The `schema` section is the exception to read-only, and
it says so when it fires — see its row below.

## Reading a failure

| Section | What it means when it fails |
| --- | --- |
| `merge conflict markers` | A tracked file still holds what a merge left behind. Every tracked file is read, not just the shipped ones: in a `.rs` file the compiler would catch it, in a changelog or a workflow nothing would. |
| `rustfmt` | `cargo fmt --all` fixes it. Never hand-format around it. |
| `clippy` | Warnings are errors. Fix the code; an `#[allow]` needs a comment saying what the lint got wrong. |
| `tests` | Read the assertion, not the count. |
| `schema` | `schema/crucible-code-schema.json` was stale, and the test that gates it has already rewritten the file — that test rewrites and then fails, so the tree in front of you now differs from the one you ran against. Read the diff, confirm it is what your `shape.rs` change should have produced, and commit it. Never hand-edit that file; it is output. |
| `file length` | Split by responsibility, not by line count. Two halves that must always change together are still one file. |
| `no process memory in shipped files` | A file under `crates/`, `src/`, `docs/`, `schema/`, `README.md` or any `Cargo.toml` names something only this repository can resolve — a decision identifier of any prefix, an assumption label, or one of the four planning and harness directories (`.claude/`, `.agents/`, `.codex/`, `.sdlc-skills/`). Say the thing instead of citing where it was decided. A published standard that happens to share the shape — `UTF-8`, `RFC-2119` — is subtracted by name inside the check; adding a name there is widening a hole, so do it only when the tree really needs it. It also fails when the scan could not finish, which means a directory it was told to read is not there. |
| `documentation links` | A repository-relative link leads nowhere. Follow it: either the target moved and the link needs updating, or the target should exist and does not. External links are deliberately unchecked. |
| `agent rules files` | `AGENTS.md` stopped being a symlink to `CLAUDE.md`. Restore it; never let two copies exist. |
| `agent rules scope` | A file in `.claude/rules/` has no `paths:` frontmatter, or aims at a directory that no longer exists — either way nothing loads it. Or a package has no rule aimed at it: every crate, and `src/`, needs one. |
| `agent skills` | A skill under `.claude/skills/` lost its `.agents/skills/` symlink, or one of those became a real directory. |
| `workspace lints` | A manifest is missing `[lints] workspace = true`, so `Cargo.toml [workspace.lints]` does not apply to it. Every lint denied there is allow-by-default, which means `.unwrap()`, `panic!()` and `unsafe {}` are all legal in that crate and clippy stays green. |
| `dependency pinning` | A crate is not `=`-pinned. Checked in every manifest, in each spelling cargo accepts, wherever the `version` key ends up when an inline table wraps. See the `add-a-dependency` skill. |
| `dependency justification` | A crate has no comment above it saying why it is needed. A comment is spent on the first dependency under it; to put a second crate in the same group, name that crate in the comment — a reason that never mentions it was not written about it, and a crate pasted under an existing one is exactly the case a comment cannot name. A member crate taking `.workspace = true` inherits the reason with the pin, and a `path = "crates/…"` dependency on a workspace member needs none: it is this project depending on itself. |
| `github actions pinning` | An action is referenced by tag. Pin the commit sha, keep the version in a trailing `# vX.Y.Z` comment. |
| `benchmark gate` | You wrote the first bench probe, so `scripts/bench.sh` can now fail for a real reason. Delete `continue-on-error` from the budgets job in `.github/workflows/ci.yml` and let the budget block the merge. |

Both cargo sections pass `--locked`, because CI and every release build do. If one
reports that the lock file needs updating, a manifest was edited without a
`cargo build` after it: run one and commit `Cargo.lock` with the change. A
lockfile that exists only on your machine is a red build on everybody else's.

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

Two branches are easy to forget and are where the failures have actually been:

- **The check that measured nothing.** A glob that matched no files, a `find`
  over a renamed directory, a `grep` whose operand is not there. Delete what the
  check walks and confirm it says so, rather than passing in silence. Every
  section that walks a set counts what it saw for this reason.
- **The correct file the check calls wrong.** A gate that goes red for something
  legal gets worked around, and a worked-around gate is off. Write the awkward
  legal case — a link with a title, an inline table that wrapped, a manifest
  using the other spelling TOML allows — and confirm it stays green.

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
- **Whether the prose is true.** Repository-relative links are followed and
  `README.md` is read for process memory, because both are mechanical. Nothing
  checks that a page still describes the tree, that a YAML workflow does what
  its comment says, or that a sentence is accurate. Read them.
- **Whether the change is right.** Green means it did not break the rules the
  project can express mechanically. It says nothing about whether the behaviour
  is the behaviour someone asked for.
