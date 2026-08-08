# Contributing

Thanks for looking. This file covers the workflow. The rules that govern the
code itself live in [`CLAUDE.md`](CLAUDE.md) — read that before your first
change; it is short, and it is what review checks against. (`AGENTS.md` is a
symlink to it, so whichever name your tools look for, there is one source.)

Participation is covered by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Setup

The toolchain is pinned in `rust-toolchain.toml`, so rustup installs the right
one automatically the first time you build.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build
cargo run -- --help
```

## Before you commit

```bash
scripts/check.sh
```

That is the whole gate, and CI runs exactly the same script — a green run here
is a green run there. It covers formatting, clippy with `-D warnings`, tests,
the 400-line-per-file cap, and dependency pinning.

If the file-length check fails, split the file by responsibility rather than by
line count. Two halves that must always change together are still one file.

## Making a change

1. Branch from `main`. Name it for the change: `feat/session-resume`,
   `fix/grep-symlink-loop`.
2. Tests lead. New behaviour starts with a failing test; a bug fix starts with a
   test that reproduces it.
3. Keep the commit focused. One reason to change per commit.
4. Run `scripts/check.sh`.
5. Open a pull request. The template asks what the change does and how you
   verified it — the second question is the one that matters.

## Commit messages

```
feat(tools): add glob tool
fix(runner): cancel in-flight stream on Esc
chore(ci): pin actions to commit sha
docs(readme): document the api key env vars
perf(tui): reuse the render buffer between frames
```

Scope is the crate or area. The body explains *why*, since the diff already
shows what.

## Performance is a reviewed property

This project exists because the alternatives are slow and heavy, so a change
that regresses the budget is a defect regardless of what it adds:

| Measure | Budget |
| --- | --- |
| Time to first frame | ≤ 20 ms p95 |
| Time to first input | ≤ 60 ms p95 |
| Peak RSS after a 20-turn session | ≤ 35 MB |
| `grep` tool vs the `rg` binary | ≤ 1.25× |
| Render commits under token burst | ≥ 30/s |

If your change moves one of these, say so in the pull request and explain the
trade. A budget change is a decision, not a side effect.

## New dependencies

Every dependency is `=`-pinned and carries a comment in `Cargo.toml` saying why
it is needed. Prefer the standard library, then a few lines of your own, then a
dependency. A crate added for one function is a permanent cost for a temporary
convenience.

## Reporting things

- **Bug or feature request:** open an issue; the templates ask for what is
  needed to act on it.
- **Security issue:** do not open a public issue. See [`SECURITY.md`](SECURITY.md).

## Scope

`v0.0.1` is deliberately small, and a good idea that is out of scope is still
out of scope for this release. If a pull request adds a capability that is not
on the current milestone, expect a request to split it out rather than a
rejection of the idea.
