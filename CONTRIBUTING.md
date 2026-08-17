# Contributing

Thanks for looking. This file covers the workflow. The rules that govern the
code itself live in [`CLAUDE.md`](CLAUDE.md) — read that before your first
change; it is short, and it is what review checks against. (`AGENTS.md` is a
symlink to it, so whichever name your tools look for, there is one source.)

Each crate also has a rules file under `.agents/rules/` naming the obligations
that only apply inside it — what a new provider has to touch, what a new tool
has to declare. Agent tooling matches those to the files you touched and loads
them on its own. A human has no such thing, so here is the same map; read the
row for what you are changing, since it is shorter than the crate and says what
review will ask about.

| What you are changing | Read |
| --- | --- |
| `src/` | [`binary-wiring.md`](.agents/rules/binary-wiring.md) — the only place concrete types meet, and the only place an error becomes an exit code |
| `crates/crucible-core/` | [`core-types.md`](.agents/rules/core-types.md) — what earns a place in a crate every other crate compiles, and which side of the open/closed line a new type is on |
| `crates/crucible-privacy/` | [`privacy-files.md`](.agents/rules/privacy-files.md) — owner-only creation, identity-bound opens, replacement and platform-specific invariants |
| `crates/crucible-auth/` | [`auth-store.md`](.agents/rules/auth-store.md) — the credential store, account login flows, and what may never leave as a string |
| `crates/crucible-config/`, `schema/` | [`config-document.md`](.agents/rules/config-document.md) — adding a setting, how layers merge, and what an error is allowed to say |
| `crates/crucible-provider/` | [`provider-wire.md`](.agents/rules/provider-wire.md) — adding a provider, credentials, and where a vendor's `chunk` stops |
| `crates/crucible-tools/` | [`tools-permission.md`](.agents/rules/tools-permission.md) — adding a tool, grants, and why a failed tool is a result rather than an error |
| `crates/crucible-runner/` | [`runner-loop.md`](.agents/rules/runner-loop.md) — the loop over traits, ending a turn once, and the transcript being the only thing that grows |
| `crates/crucible-session/` | [`session-log.md`](.agents/rules/session-log.md) — the append-only log, its format version, and the owner-only tree |
| `crates/crucible-tui/` | [`tui-render.md`](.agents/rules/tui-render.md) — the render budget, wrapping, and restoring the terminal you changed |

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
the 2000-line-per-file cap, leftover merge conflict markers, and dependency
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

## The whole-screen test

`tests/whole_screen` starts the real binary on a real pseudo terminal, sends
keystrokes and asserts on the screen it drew. It is the only test that sees the
arithmetic turning rows into a screen — how far a frame rewinds, where the
cursor parks, how tall the live region may be — because every other one is
handed rows and never a terminal. `scripts/check.sh` runs it with the rest; on
its own it is

```bash
cargo test --test whole_screen
```

Linux only, and `setsid` from util-linux has to be on the path. The child needs
the pty as its *controlling* terminal or it reads the size of your window
instead of the one the case asked for, and claiming one without `unsafe` means
handing that job to `setsid --ctty`.

The screens are `insta` snapshots, so anything that moves the layout fails them
— including a release, which changes the version on the welcome. Read the diff
as a screen, and accept it once it is the screen you meant:

```bash
cargo insta accept                                    # with cargo-insta
INSTA_UPDATE=always cargo test --test whole_screen    # without it
```

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

Wherever it is going. A branch of your own that collects sub-branches used to be
exempt, on the argument that it gets measured in turn when it asks for `main` —
but that measures the collection and never the pieces, and the piece is what
somebody sat down to read. The ceiling is about the reader in front of the diff,
and every pull request has one.

A change that does not fit is a sequence of pull requests that each stand on
their own, not one larger one with a note about its size.

Two diffs are measured wrongly, and no diff can prove either about itself, so
each is a label — somebody saying so, left visible on the pull request
afterwards:

- `moves-only`, for code that only moves. Nothing changed but where it lives.
- `whole-module`, for a module whose parts do not compile apart. `-D warnings`
  makes an unreached function a failed build, so a new provider, tool or
  renderer arrives exported and working or it does not build at all. Where that
  floor is already over the ceiling, the only smaller pull request is one that
  lands the code and leaves its tests for the next — which is a worse thing to
  ask a reviewer to approve than a long diff.

The second is the narrower of the two and is meant to stay that way. It is not
for a change that is merely large, or awkward to split, or all one topic: the
question is whether an intermediate pull request would *compile*, and the answer
has to be no. Say in the pull request where the floor is and what you measured
it at, so the next reader can check the claim rather than take it.

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
| Worst paired median, `grep` tool / `rg` binary | ≤ 1.25× |
| Render commits under token burst | ≥ 30/s |

The grep probe pairs each tool run with `rg` over representative workloads. Its
worst paired median owns the budget; p95 and dispersion are diagnostic evidence.

If your change moves one of these, say so in the pull request and explain the
trade. A budget change is a decision, not a side effect.

## New dependencies

Every dependency is `=`-pinned and carries a comment in `Cargo.toml` saying why
it is needed; `scripts/check.sh` fails without both. One comment covers a group
— the four ripgrep crates are one decision rather than four — but it has to name
every crate it covers, because otherwise it is spent on the first dependency
beneath it and the next one inherits a justification nobody wrote for it.
Prefer the standard library, then a few lines of your own, then a
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

The 0.x line is deliberately small, and a good idea that is out of scope is
still out of scope for it. If a pull request adds a capability that is not
on the current milestone, expect a request to split it out rather than a
rejection of the idea.
