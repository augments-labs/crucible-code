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
arithmetic turning rows into a screen — which band a row lands in, where the
cursor parks, how tall the box may grow — because every other one is handed rows
and never a terminal. `scripts/check.sh` runs it with the rest; on
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

## A pull request has one reason to change

No line ceiling is enforced on it for now. What decides whether a
change is one pull request or several is whether it takes one summary to say
what it does — a change that needs two is two, however short each of them turns
out to be, and a module whose parts do not compile apart is one however long it
runs, since `-D warnings` makes an unreached function a failed build and the
only smaller pull request would be one that lands the code and leaves its tests
for the next.

Nothing checks this, because nothing can. It is the judgement of the person
opening the pull request and the person reading it, and the description is
where it gets made: if saying what the change does takes more than a paragraph,
that is the signal, and a sequence of pull requests that each stand on their
own is the remedy — not one larger one with a note about its size.

CI still counts the diff and prints the number, additions plus deletions with
`Cargo.lock` and `schema/` aside since both are generated. Nothing is sent back
for it: the ceiling it counts against is temporarily off while the project is
this young, because most of what arrives is still a whole module and that is
the shape a line count measures worst. Off rather than gone — the ceiling, and
the one line that enforces it again, are in `.github/workflows/ci.yml`.

The number is there meanwhile so a reader knows what they are opening, and the
`moves-only` and `whole-module` labels are still worth adding when they are
true. They excuse nothing while the ceiling is off, and both say something
about a diff that the diff cannot say about itself: that nothing changed but
where the code lives, and that a module's parts do not compile apart.

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
needs it is looking. The changelog is shorter still: a bold lead and at most
three sentences, written for someone deciding whether to upgrade.

## Performance is a reviewed property

This project exists because the alternatives are slow and heavy, so a change
that regresses a budget is a defect regardless of what it adds. The budgets
live in `scripts/bench.sh` — one probe per budget, each carrying its own limit
and failing when it is over, so the number and the measurement cannot drift
apart — and `README.md` carries the table.

The grep probe pairs each tool run with `rg` over representative workloads. Its
worst paired median owns the budget; p95 and dispersion are diagnostic evidence.
The startup probes cut their runs into nine windows and take the middle window's
tail, so a stretch of wall clock this program did not own is not read as this
program having got slower.

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
