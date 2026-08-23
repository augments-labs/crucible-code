<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
  <img alt="crucible" src="assets/logo-light.svg" width="50%">
</picture>

**The harness where agents are forged.**

A terminal coding agent in Rust — fast to start, light on memory, and yours to run.

</div>

---

## What it is

A coding agent you drive from a terminal. It reads and edits files, runs
commands, searches a tree, and streams a model's reasoning onto a screen of its
own — one you scroll, select and click in, and that is handed back whole when
you leave.

It is provider agnostic. A provider is a wire protocol; how you authenticate is
a separate axis, so the two are independent choices rather than one coupled
decision.

## Why another one

Because the ones that exist are slower and heavier than a terminal tool should
be. These are budgets, not aspirations — a change that breaks one is a defect,
and the release procedure blocks on them:

| Measure | Budget |
| --- | --- |
| Time to first frame | ≤ 20 ms p95 |
| Time to first input | ≤ 60 ms p95 |
| Peak RSS after a 20-turn session | ≤ 35 MB |
| Worst paired median, `grep` tool / `rg` binary | ≤ 1.25× |
| Render commits under token burst | ≥ 30/s |

The grep probe pairs each tool run with `rg` over representative workloads. Its
worst paired median owns the budget; p95 and dispersion are diagnostic evidence.

Rendering costs nothing as a transcript grows: a frame folds and paints only
the rows the window covers, and writes only the ones whose text is not already
there. The transcript itself is held in memory for the life of the session and
lent to each provider request rather than cloned. Provider request bodies are written directly into their outbound
allocation; the transcript is what the peak-RSS figure bounds.

## Installing

Every release attaches a binary for Linux, macOS and Windows on x86-64 and
ARM64, usually with FreeBSD on x86-64 beside them, and one `SHA256SUMS`
covering all of it, on the
[releases page](https://github.com/augments-labs/crucible-code/releases).
Getting started says why FreeBSD is the one that may be missing.

Take the archive for your platform and that file, check one against the other,
and put the binary on your `PATH`:

```bash
sha256sum --ignore-missing -c SHA256SUMS
tar xzf crucible-<version>-linux-x86_64.tar.gz
install crucible-<version>-linux-x86_64/crucible ~/.local/bin/
```

## Building

Rust is pinned in `rust-toolchain.toml`, so rustup fetches the right toolchain
on first build.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build --release
./target/release/crucible --version
```

Anything past this point —
[getting started](docs/getting-started/index.md),
[tools](docs/tools/index.md), [providers](docs/providers/index.md),
[configuration](docs/configuration/index.md),
[permissions](docs/permissions/index.md), [sessions](docs/sessions/index.md) — lives in
`docs/` rather than here, so there is one copy of it to keep true.

## Running it

Set a key, start it in the directory you want it to work in, and type.

```bash
export ANTHROPIC_API_KEY=...
cd ~/code/my-project
crucible
```

`/login` inside the session is the other way in — a ChatGPT or Kimi Code
account, or a key kept in crucible's protected store rather than exported.

`--model` takes a model name, optionally qualified by the provider serving it.
Unqualified, the provider is whichever holds a usable credential — a key in one
of `ANTHROPIC_API_KEY`, `MOONSHOT_API_KEY` and `OPENAI_API_KEY`, or one stored
by `/login`, whether an API key or an account login. Hold more than one and
nothing chooses between them, so qualify the name or set `provider` for one of
them. There is no model built in: name one,
configure one, or run `/model` in the session and take one off the shelf, which
searches by model or provider, takes the effort in the same visit, and writes
both down.

`--effort` says how hard to think — `low`, `medium`, `high`, `xhigh` or `max` —
on every turn of the session, and `providers.<name>.effort` says it once. Left
off, crucible asks for no rung and the vendor's own default for that model is
what applies. Not every model serves all five, and some serve none at all;
`/effort` offers the ones the model in force does, as does the shelf `/model`
stands.

```bash
crucible --model openai/gpt-5.6-terra   # reads OPENAI_API_KEY
crucible --model moonshot/k3            # reads MOONSHOT_API_KEY
crucible --effort max                   # think as hard as this model does
crucible --continue                     # carry on this directory's last session
```

Reading never asks. Anything that changes a file or starts a process does:

```
╭──────────────────────────────────────────────────────────────────────────────╮
│  Bash command                                                                │
│                                                                              │
│    cargo test                                                                │
│                                                                              │
│  This command needs your verdict.                                            │
│                                                                              │
│  Do you want to proceed?                                                     │
│  › 1. Yes, once                                                              │
│    2. Yes, and don't ask again this session                                  │
│    3. No, and end the turn                                                   │
╰──────────────────────────────────────────────────────────────────────────────╯
  esc to cancel
```

`↑` and `↓` move the mark, `enter` takes it, and `1`, `2` and `3` answer
directly. The second answer remembers calls like this one until this process
exits. Durable rules are
written deliberately in the user configuration outside a checkout.

Full documentation is in [`docs/`](docs/index.md).

## Contributing

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md) for the workflow and
[`CLAUDE.md`](CLAUDE.md) for the rules the code is held to. One command covers
every gate, and it is what CI runs:

```bash
scripts/check.sh
```

Participation is covered by the [Code of Conduct](CODE_OF_CONDUCT.md). Security
issues go through [`SECURITY.md`](SECURITY.md), never a public issue.

## License

MIT — see [`LICENSE`](LICENSE).
