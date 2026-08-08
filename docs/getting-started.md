# Getting started

## Build it

There are no published binaries yet. Rust is pinned in `rust-toolchain.toml`, so
rustup fetches the right toolchain on the first build.

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
  //! The turn loop. (+312 lines)

A failed tool is not a failed turn: the failure goes back to the model as the
result of that call, and the model decides what to do about it.

› 
```

Press <kbd>Ctrl-D</kbd> on an empty prompt to leave.

<kbd>Ctrl-C</kbd> ends the process rather than the turn. In 0.0.1 input is left
in the terminal's cooked mode and no signal is caught, so there is no way to stop
a single answer and keep the session. The session log is written as the turn
goes, so `crucible --continue` picks the conversation up from wherever it
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
