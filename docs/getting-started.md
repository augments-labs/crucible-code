# Getting started

## What it runs on

Linux x86-64. The released binary is dynamically linked against glibc and needs
**2.34 or newer** — Debian 12, Ubuntu 22.04, RHEL 9 and anything later are fine;
older than that has to build from source. It asks the system for nothing else:
no certificate bundle, no shell, no runtime to install. `scripts/smoke.sh` is
what keeps that true, by running each release in a sandbox holding the binary
and its two libraries and nothing besides.

## Build it

Rust is pinned in `rust-toolchain.toml`, so rustup fetches the right toolchain
on the first build.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build --release
./target/release/crucible --version
```

## Give it a key

crucible reads a key from the environment and never stores one. Which variable
is read depends on the provider serving the model you ask for — see
[Providers and models](providers.md).

```bash
export ANTHROPIC_API_KEY=...
```

## Run it

Start it in the directory you want it to work in. That directory is the
workspace root, and every path a tool touches is relative to it.

```bash
cd ~/code/my-project
crucible
```

It opens with the version, the model, and the root it is standing on:

```
crucible 0.0.1 · claude-sonnet-5
/home/you/code/my-project

› 
```

Type a prompt and press enter. The answer streams in as it is produced. Tool
calls appear as they run:

```
› what does the runner do when a tool fails?

· read {"path":"crates/crucible-runner/src/runner.rs"}
       1	//! The turn loop. (+238 lines)

A failed tool is not a failed turn: the failure goes back to the model as the
result of that call, and the model decides what to do about it.

› 
```

A tool's output is summarised to its first line and a count of the rest; `read`
numbers lines the way `cat -n` does, which is why the summary starts with a `1`.
A call that failed is marked `✗`.

Press <kbd>Ctrl-D</kbd> on an empty prompt to leave.

## When an answer stops early

An answer can end for a reason other than the model having finished. When it
does, a line says so under the turn:

```
! unfinished: the answer reached the token ceiling
```

```
! unfinished: the provider's filter cut the answer short
```

The two are named apart because the remedy is opposite. The first means the
answer ran out of room, and a narrower question gets a complete one. The second
means the provider stopped the answer on its own, and asking for less buys
nothing. Without the line, both look exactly like an answer that finished.

A turn that ended normally says nothing at all. There is a third line,
`! stopped`, for a turn that was cancelled — but nothing in 0.0.1 can cancel
one, so you will not see it yet. See <kbd>Ctrl-C</kbd> below.

<kbd>Ctrl-C</kbd> ends the process rather than the turn. In 0.0.1 input is left
in the terminal's cooked mode and no signal is caught, so there is no way to stop
a single answer and keep the session. The session log is written as the turn
goes, so `crucible --continue` picks the session up from wherever it
stopped — see [Sessions](sessions.md).

## What it can do

Six tools, advertised in the order a model tends to reach for them:

| Tool | What it does | Asks first |
| --- | --- | --- |
| `read` | Reads a file | no |
| `grep` | Searches file contents | no |
| `glob` | Finds files by pattern | no |
| `edit` | Replaces text in a file | yes |
| `write` | Creates or overwrites a file | yes |
| `bash` | Runs a command | yes |

Reads never ask. Anything that changes a file or starts a process does — see
[Permission](permission.md).
