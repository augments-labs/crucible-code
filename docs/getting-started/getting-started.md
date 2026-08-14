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

crucible reads a key from the environment, or from a file you can write it into
instead. Which variable is read depends on the provider serving the model you
ask for — `/login` inside a session says which, and
[Providers and models](../providers/providers.md) has the rest.

```bash
export ANTHROPIC_API_KEY=...
```

Or type `/login anthropic` inside a session and let crucible keep the key
instead of your shell profile. The box that opens draws a dot per character and
never the key, and what it takes goes to `~/.crucible/auth.json`, a file only
you can read. The session asks that provider from the next turn on — there is
nothing to restart. An exported variable still wins over it.

You do not have to know that command to find it. A run holding no key for any
provider opens on the panel `/login` stands, before it reads anything you type:
choose one and the box for the key opens on the same screen. Escape skips it and
leaves the session where every other run starts, with the offer still standing
under the name of the command that opens it again. A run with nothing at the
keyboard — one reading a prompt down a pipe — never meets it, and gets the
warning alone.

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

The box is as wide as the terminal, and a line longer than it wraps onto the
next row rather than scrolling sideways — so the box grows downwards as you
write. It stops at about half the window; past that the line scrolls under the
top edge and what you are writing stays in view.

The arrows move a character, <kbd>Ctrl</kbd> or <kbd>Alt</kbd> held with one
moves a word — as do <kbd>Alt-B</kbd> and <kbd>Alt-F</kbd> — and <kbd>Home</kbd>
and <kbd>End</kbd> reach the two ends. A word here is a run of anything that is
not a space, so a path is one word.

The mouse belongs to the terminal: the wheel scrolls, dragging selects, the
middle button pastes. Set `output.mouse` to `click` and crucible takes the
buttons instead, for the whole session, so clicking in the box between turns
puts the cursor where you clicked — at the price of the wheel, because the wheel
is a button and the scrollback it scrolls is where crucible's transcript lives.
An inline renderer cannot have both, which is why it is a choice rather than a
default.

Under the box is the mode in force, every time — `ask mode on` is the one
nothing configured gives you. <kbd>Shift-Tab</kbd> steps to the next one while
you type, and the row and the colour of the box both follow it.
[Permissions](../permissions/index.md) is where all three are.

Type a prompt and press enter. The line stays where it was and the answer
streams in under it, with the box and the mode still standing at the bottom of
the screen — so a tool call arriving ten minutes into a turn is read beside the
mode that let it through, and the screen looks the same whether or not crucible
happens to be working.

You can go on writing in the box while the answer arrives. <kbd>Enter</kbd>
queues what you wrote as the next prompt, and it is run the moment the turn
ends. <kbd>Ctrl-C</kbd> asks the turn to stop. The key that steps the mode is
not offered there, because the mode is away with the turn until it finishes.

Tool calls appear as they run:

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

The answer itself is read as markdown rather than printed with the markers still
in it. A heading loses its hashes and stands out, `**` or `_` around a phrase
leaves it emphasised, backticks leave a run of code toned down, and a fenced
block is toned for its whole length with the fence lines and the language
written on them gone. The tone belongs to the row rather than to the text, so it
costs no column: the answer wraps exactly where the same answer would have
wrapped plain.

Where there is no colour to read it into, the markers are left where the model
put them. That covers a redirected run, `NO_COLOR`, and `--color never` — taking
a marker out there would drop the emphasis and put nothing in its place, and
`crucible < prompts.txt > answers.md` is a file of markdown worth keeping.

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
| `/model` | Picks the model to ask from now on, or takes the one you name |
| `/effort` | Picks how hard it thinks from now on, or takes the rung you name |
| `/login` | Takes a key for a provider and writes it down |
| `/logout` | Forgets a key crucible wrote down |
| `/mode` | The [permission mode](../permissions/modes.md) in force, or the one you name |
| `/resume` | Lists what was worked on in this directory, and picks one back up |
| `/clear` | Forgets what has been said, keeping the session |
| `/exit` | Ends the session |

`/model` on its own stands a panel where the prompt box was, holding a few of
the models your provider serves with the one being asked now named above them.
Escape leaves it and changes nothing. `/model <name>` skips the panel, and is
also how to ask for a model the panel does not carry: the list is a shortcut past
the vendor's documentation, not the limit of what the vendor serves.

Either way the name is written to `~/.crucible/config.json` under the provider
this run is set up for, so the next crucible started anywhere begins with it.
See [Providers and models](../providers/providers.md).

`/effort` is the same shape over the five rungs — `low`, `medium`, `high`,
`xhigh`, `max` — and is written to the same file beside the model. The panel
opens on `high` where nothing has chosen yet, which is a place to start walking
from rather than a rung being asked for: leaving it leaves the session asking for
none, and what applies then is the vendor's own default for that model. All five
are offered wherever you are, because which of them a model serves is the
vendor's answer and differs between models of one vendor.

`/login <provider>` opens a box for the key — never the command line, which
would put it in your shell's history, in the process listing and in the
scrollback. Escape leaves it without writing anything. `/login` on its own stands
a panel where the prompt box was: ↑ and ↓ walk the providers this build serves,
Enter opens the box for the one marked. A run with no keyboard to walk it — and a
window with no room to stand a panel in — gets those names as rows instead, with
the variable each reads from. That same panel is what a run holding no key for
anything opens on, unasked, saying so above the list and offering escape as a
skip rather than a cancel.

A key that is written lands on the session that took it: the provider is set up
there and then, and the model and rung your configuration names for it come with
it wherever nothing has chosen one yet. A run started with no key for anything is
one command away from a turn, and the line under the box says which model it will
be asking — or sends you to `/model` where the files name none.

`/logout` is the same panel over what is actually there: the providers a key was
written down for, and nothing else. `/logout <provider>` forgets that one
directly, and a name with no key here says so and lists the ones that have. It
reaches `~/.crucible/auth.json` and only that — a key exported into your shell is
untouched and goes on winning — which is what the line under the answer says.

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

One row of the open list is marked, and that row is what <kbd>Enter</kbd> runs —
so a command runs from the letters that name it, without the rest being typed.
The mark starts on the first row the filter left, or on the command whose name
you have typed in full where that is one of them, and <kbd>↑</kbd> and
<kbd>↓</kbd> move it. It stops at either end rather than running round.

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
`! stopped`, for a turn you stopped yourself:

```
! stopped
```

<kbd>Ctrl-C</kbd> during a turn asks that turn to stop, and leaves the session
where it was. Nothing is killed: the provider stops between reads and a command
stops between the steps it takes, so a file a tool was writing is either
untouched or finished. What was on screen stays on screen, what you had typed
stays in the box, and the next prompt carries on the same session.

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

`bash` runs its command through a POSIX shell in the workspace root, and starts
it with a short list of variables — `PATH`, `HOME`, the locale — rather than the
environment crucible is running in. Your provider key is not on that list, so a
command that prints the environment prints no key. Anything else a command needs
is named in [`env`](../configuration/configuration.md#env).
