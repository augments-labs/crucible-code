# Contributing

Thanks for looking. This file covers the workflow. The rules that govern the
code itself live in [`CLAUDE.md`](CLAUDE.md) — read that before your first
change; it is short, and it is what review checks against. (`AGENTS.md` is a
symlink to it, so whichever name your tools look for, there is one source.)

Each crate also has a rules file under `.claude/rules/` naming the obligations
that only apply inside it — what a new provider has to touch, what a new tool
has to declare. Read the one for the crate you are changing. Agent tooling loads
them automatically; a human should skim them, since they are shorter than the
crate and say what review will ask about.

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

What your machine needs besides rustup is one C compiler, and
[Building](docs/building/building.md) names the package that carries it on
Linux, macOS and Windows. [Building for another
platform](docs/building/cross-compiling.md) covers the rest: crucible ships
seven targets, and six of them can be compiled from one machine.

## Before you commit

```bash
scripts/check.sh
```

That is the whole gate, and CI runs exactly the same script — a green run here
is a green run there. It covers formatting, clippy with `-D warnings`, tests,
the 1000-line-per-file cap, leftover merge conflict markers, and dependency
pinning and justification.

One check deliberately sits outside it. The advisory scan runs on a schedule in
CI rather than here, because its answer changes when somebody publishes an
advisory rather than when you edit — putting it in this script would break the
sentence above, and turn your pull request red for something you never touched.
See [New dependencies](#new-dependencies).

If the file-length check fails, split the file by responsibility rather than by
line count. Two halves that must always change together are still one file, and
the ceiling is set high enough that hitting it means the subject went missing
rather than that the file got long. Splitting to be under a number produces the
failure no number can see: a directory of files too small to have a subject,
where learning what one of them does means opening all of them.

## Making a change

1. Branch from `main`. Name it for the change: `feat/session-resume`,
   `fix/grep-symlink-loop`.
2. Tests lead. New behaviour starts with a failing test; a bug fix starts with a
   test that reproduces it.
3. Keep the commit focused. One reason to change per commit.
4. Run `scripts/check.sh`.
5. Open a pull request. The template asks what the change does and how you
   verified it — the second question is the one that matters.

## Pull requests are 400 changed lines or fewer

Additions plus deletions across the diff, `Cargo.lock` and `schema/` aside since
both are generated. CI measures it and a larger one is sent back — past that
size a review turns into agreement, where the reader is checking that a change
looks plausible rather than that it is right.

Where it is going decides whether that blocks. Into `main`, `dev` or a release
branch it does: those are read by somebody other than the author, and a release
is cut from them. Into a branch of your own that is collecting sub-branches it
does not — CI prints the number and passes, because that branch is measured in
turn when it asks for `main`, over everything it collected. Nothing reaches
`main` unmeasured.

A change that does not fit is a sequence of pull requests that each stand on
their own, not one larger one with a note about its size. The one diff this
measures wrongly is code that only moves, and no diff can prove that about
itself: ask for the `moves-only` label, which grants the exception and leaves it
visible on the pull request afterwards.

## Commit messages

```
feat(tools): add glob tool
fix(runner): cancel in-flight stream on Esc
chore(ci): pin actions to commit sha
docs(readme): document the api key env vars
perf(tui): reuse the render buffer between frames
```

Scope is the crate or area. The body explains *why*, since the diff already
shows what — a short paragraph, not an essay. Reasoning that needs more than
that belongs in a comment beside the code it explains, where the person who
needs it is looking. The changelog is shorter still: a bold lead and a sentence
or two, written for someone deciding whether to upgrade.

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
it is needed; `scripts/check.sh` fails without both. One comment covers the
group beneath it, since the four ripgrep crates are one decision rather than
four. Prefer the standard library, then a few lines of your own, then a
dependency. A crate added for one function is a permanent cost for a temporary
convenience.

Adding one also has to pass `deny.toml`: no open advisory, a licence on the
permissive list, and crates.io as the source. Run it before you open the pull
request, or let the `audit` workflow tell you — it fires on any change to
`Cargo.toml`, `Cargo.lock` or `deny.toml`, and weekly regardless.

```bash
cargo install cargo-deny --locked   # once
cargo deny check
```

A licence that is not on the list is a decision, not an oversight — say why it
belongs in the pull request rather than adding the line quietly. The same goes
for an advisory you believe cannot be reached through crucible: it gets an entry
in `deny.toml` with the reasoning, so the next person knows it was considered.

## Reporting things

- **Bug or feature request:** open an issue; the templates ask for what is
  needed to act on it.
- **Security issue:** do not open a public issue. See [`SECURITY.md`](SECURITY.md).

## Scope

The 0.0.x line is deliberately small, and a good idea that is out of scope is
still out of scope for it. If a pull request adds a capability that is not
on the current milestone, expect a request to split it out rather than a
rejection of the idea.
