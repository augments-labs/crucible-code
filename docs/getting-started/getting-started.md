# Getting started

## What it runs on

Seven builds, one per release:

| Platform | Artifact |
| --- | --- |
| Linux x86-64 | `crucible-<version>-linux-x86_64.tar.gz` |
| Linux ARM64 | `crucible-<version>-linux-aarch64.tar.gz` |
| macOS Apple silicon | `crucible-<version>-macos-aarch64.tar.gz` |
| macOS Intel | `crucible-<version>-macos-x86_64.tar.gz` |
| Windows x86-64 | `crucible-<version>-windows-x86_64.tar.gz`, `.exe` |
| Windows ARM64 | `crucible-<version>-windows-aarch64.tar.gz`, `.exe` |
| FreeBSD x86-64 | `crucible-<version>-freebsd-x86_64.tar.gz` |

`SHA256SUMS` beside them covers all of it. Anything else builds from source.

The Linux binaries are dynamically linked against glibc and need **2.34 or
newer** — Debian 12, Ubuntu 22.04, RHEL 9 and anything later are fine; older
than that has to build from source. Every build asks the system for nothing
else: no certificate bundle, no runtime to install. `scripts/smoke.sh` is what
keeps that true, by running each release in a sandbox holding the binary and its
two libraries and nothing besides.

The one thing crucible does look for is a POSIX shell, and only when the `bash`
tool runs a command. Every platform here has one except Windows, where it is
whichever `sh.exe` is on the `PATH` — [Git for
Windows](https://git-scm.com/download/win) carries one, and crucible finds that
one where it is normally installed even when it is not on the `PATH`. Without
one, everything except the `bash` tool works and that tool says what is missing.

## Install it

The archives are attached to the
[releases page](https://github.com/augments-labs/crucible-code/releases), with
`SHA256SUMS` beside them. Take the one for your platform and that file, check
one against the other, unpack it, and put the binary somewhere on your `PATH`:

```bash
sha256sum --ignore-missing -c SHA256SUMS
tar xzf crucible-<version>-linux-x86_64.tar.gz
install crucible-<version>-linux-x86_64/crucible ~/.local/bin/
```

`--ignore-missing` is what lets one checksum file covering the whole release
check the one archive you took. Each archive unpacks into a directory named
after it, holding the binary, the README and the licence.

Each Windows target also ships the executable on its own, beside the archive and
named the same way — `crucible-<version>-windows-x86_64.exe` — so there is
nothing to unpack. `SHA256SUMS` covers those too.

## Build it

Rust is pinned in `rust-toolchain.toml`, so rustup fetches the right toolchain
on the first build. This is the path for a platform not in the table above, and
for working on crucible itself.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build --release
./target/release/crucible --version
```

## Give it a key

crucible reads a key from the environment and never stores one. Which variable
is read depends on the provider serving the model you ask for — see
[Providers and models](../providers/providers.md).

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

It opens with a card naming the release, the model it is asking, and the root it
is standing on, beside the last few sessions started in this directory. The card
fits itself to the terminal: two columns at eighty and above, one below that,
and under forty-six there is no frame at all — just what it is, what it is
asking, and where. Under the card is the box:

```
╭────────────────────────────────────────────────────────────╮
│ ›                                                          │
╰────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)
```

The box is as wide as the terminal, and a line longer than it scrolls inside it
rather than wrapping. Under it is the mode in force, every time — `ask mode on`
is the one nothing configured gives you. <kbd>Shift-Tab</kbd> steps to the next
one while you type, and stepping into full access is agreed to first, because
nothing is asked after it. [Permissions](../permissions/index.md) is where all
three are.

Type a prompt and press enter. The box goes, the line stays where it was, and
the answer streams in under it. Tool calls appear as they run:

```
› what does the runner do when a tool fails?

· read {"path":"crates/crucible-runner/src/runner.rs"}
       1	//! The turn loop. (+238 lines)

A failed tool is not a failed turn: the failure goes back to the model as the
result of that call, and the model decides what to do about it.

╭────────────────────────────────────────────────────────────╮
│ ›                                                          │
╰────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)
```

A tool's output is summarised to its first line and a count of the rest; `read`
numbers lines the way `cat -n` does, which is why the summary starts with a `1`.
A call that failed is marked `✗`.

<kbd>Ctrl-C</kbd> throws away a line you are part-way through. Against an empty box
it offers to leave — `press ctrl-c again to leave`, under the mode — and a second
press within two seconds takes the offer. Any other key first takes it back, so a
session is never ended by one stray keystroke. <kbd>Ctrl-D</kbd> on an empty box
leaves at once.

A run whose input or output is redirected gets no box: `crucible < prompts.txt`
reads whole lines, one prompt each, and the mode is written in front of them
instead.

## Commands

A line starting with `/` is a command rather than a prompt. It is answered here,
costs the provider nothing, and is not part of what the model is told about the
session.

| Command | What it does |
| --- | --- |
| `/help` | Lists these |
| `/model` | The model this session is asking |
| `/mode` | The [permission mode](../permissions/modes.md) in force, or the one you name |
| `/resume` | Lists what was worked on in this directory, and picks one back up |
| `/clear` | Forgets what has been said, keeping the session |
| `/exit` | Ends the session |

`/clear` empties the transcript: the next prompt is the first one the model
sees, and the turns before it are neither sent nor paid for again. It is the
same session either way — the same log, the same permission answers, the same
mode — and the screen is left alone, because what is above the box is the
terminal's scrollback rather than crucible's. Continuing that session later
picks it up from where it started again.

`/resume` lists this directory's last nine [sessions](../sessions/sessions.md),
newest first, each numbered and shown with when it started and what it was first
asked:

```
1  just now      rename the parser's error type
2  2 hours ago   why does the grep tool miss hidden files
3  yesterday     add a --json flag to the report command
```

`/resume 2` picks that one up. The session you were in is closed — its log is
finished and stays readable — and the one you named becomes the session this
crucible is recording to, with everything already in it back in the transcript.
The number is the row on the list you were just shown, so read it again if
something else has been recorded here since.

Two things are worth knowing before you switch. The [permission
mode](../permissions/modes.md) comes with you, but what you allowed *for the
rest of that session* does not — the new session is asked about those calls
again, and rules you wrote to a file apply as they always did. And a session
another crucible still has open cannot be picked up: it says so rather than
letting two of them write to one log.

Typing `/` opens the list above the box, filtered to what has been typed so far,
so the box and the mode under it stay where they are. The list closes as soon as
the line becomes something else — a path, a sentence, a command with a word
after it. That is also what keeps `/etc/hosts is wrong` a prompt: a line is only
taken for a command where it could not be anything else.

## When an answer stops early

An answer can end for a reason other than the model having finished. When it
does, a line says so under the turn:

```
! unfinished: the answer reached the token ceiling
```

```
! unfinished: the provider's filter cut the answer short
```

```
! unfinished: the provider paused this turn; ask it to go on
```

The three are named apart because the remedy differs. The first means the answer
ran out of room, and a narrower question gets a complete one. The second means
the provider stopped the answer on its own, and asking for less buys nothing.
The third means the answer is not over: the same prompt again carries on from a
transcript that already holds this much. Without the line, all three look
exactly like an answer that finished.

A turn that ended normally says nothing at all. There is a fourth line,
`! stopped`, for a turn that was cancelled — but nothing in 0.0.x can cancel
one, so you will not see it yet. See <kbd>Ctrl-C</kbd> below.

<kbd>Ctrl-C</kbd> ends the process rather than the turn. In 0.0.x input is left
in the terminal's cooked mode and no signal is caught, so there is no way to stop
a single answer and keep the session. The session log is written as the turn
goes, so `crucible --continue` picks the session up from wherever it
stopped — see [Sessions](../sessions/sessions.md).

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

Reads never ask. Anything that changes a file or starts a process does, until
you configure rules or a mode that answer for you — see
[Permissions](../permissions/index.md).
