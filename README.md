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
commands, searches a tree, and streams a model's reasoning inline — in the
scrollback you already have, not in a full-screen buffer that replaces it.

It is provider agnostic. A provider is a wire protocol; how you authenticate is
a separate axis, so an API key today and a subscription login later are two
independent choices rather than one coupled decision.

## Why another one

Because the ones that exist are slower and heavier than a terminal tool should
be. These are budgets, not aspirations — a change that breaks one is a defect,
and the release procedure blocks on them:

| Measure | Budget |
| --- | --- |
| Time to first frame | ≤ 20 ms p95 |
| Time to first input | ≤ 60 ms p95 |
| Peak RSS after a 20-turn session | ≤ 35 MB |
| `grep` tool vs the `rg` binary | ≤ 1.25× |
| Render commits under token burst | ≥ 30/s |

Rendering costs nothing as a transcript grows, because it is inline: scrollback
belongs to the terminal, not to this process. The transcript itself is held in
memory for the life of the session, and is what the peak-RSS figure bounds.

## Building

Rust is pinned in `rust-toolchain.toml`, so rustup fetches the right toolchain
on first build.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build --release
./target/release/crucible --version
```

There are no published binaries yet; when there are, they will be attached to
the GitHub Release for the tag. Anything past this point —
[getting started](docs/getting-started/index.md),
[providers](docs/providers/index.md), [configuration](docs/configuration/index.md),
[permissions](docs/permissions/index.md), [sessions](docs/sessions/index.md) — lives in
`docs/` rather than here, so there is one copy of it to keep true.

## Running it

Set a key, start it in the directory you want it to work in, and type.

```bash
export ANTHROPIC_API_KEY=...
cd ~/code/my-project
crucible
```

`--model` takes a model name, optionally qualified by the provider serving it.
Unqualified names go to Anthropic.

```bash
crucible --model openai/gpt-5.2    # reads OPENAI_API_KEY
crucible --continue                # carry on this directory's last session
```

Reading never asks. Anything that changes a file or starts a process does:

```
? bash wants to run: cargo
  [y]es  [s]ession  [a]lways  [n]o › 
```

`always` writes the rule into `.crucible/config.local.json`, so the next
session starts already knowing.

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
