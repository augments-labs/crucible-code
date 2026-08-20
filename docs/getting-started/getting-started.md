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

The FreeBSD archive is the one that may be absent from a release. There is no
FreeBSD machine to build it on, so it is built in a virtual one, and when that
does not come up the release goes out without it rather than not at all. Build
from source, or take the archive from the release before it — the version it
holds is the version it says.

The Linux release workflow builds dynamically linked binaries against glibc
2.34 — Debian 12, Ubuntu 22.04, RHEL 9 and anything later are fine; older than
that has to build from source. The already-published 0.1.6 Linux artifacts
predate that build and need glibc 2.39; release artifacts cannot be changed in
place. A binary from the current release workflow asks the system for nothing
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

On Linux, macOS and FreeBSD, download and run the release installer:

```bash
curl --proto '=https' --tlsv1.2 -fsSLO \
  https://github.com/augments-labs/crucible-code/releases/latest/download/install.sh
bash install.sh
```

The script requires Bash. Linux and macOS installations normally have it;
FreeBSD keeps it in the `bash` package rather than the base system. Without
Bash, use the manual path below. The script detects the platform, verifies
exactly the archive it downloads
against the release's `SHA256SUMS`, and atomically installs `crucible` plus a
`cru` alias in `~/.local/bin`. It never asks for `sudo` or edits a shell
profile. Use `--version`, `--dir` or `--dry-run` when the defaults are not the
ones you want. The matching `uninstall.sh` removes only those executables and
preserves `~/.crucible`; deleting configuration, credentials and sessions
requires the explicit `--purge --yes` pair.

For a manual Unix install, download the archive and `SHA256SUMS` from the
[releases page](https://github.com/augments-labs/crucible-code/releases). On
Linux use `sha256sum -c`, on macOS use `shasum -a 256 -c`, and on FreeBSD use
`sha256 -c`, with a checksum file narrowed to the downloaded archive. Then
unpack it with `tar xzf` and copy `crucible` into a directory on `PATH`.

Each Windows target also ships the executable on its own, beside the archive and
named the same way — `crucible-<version>-windows-x86_64.exe` — so there is
nothing to unpack. `SHA256SUMS` covers those too; PowerShell verifies it with
`(Get-FileHash .\crucible-<version>-windows-x86_64.exe -Algorithm SHA256).Hash`
before the executable is moved into a directory on `PATH`.

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

## Sign in or give it a key

`/login` inside a session can authorize a ChatGPT or Kimi Code account, or keep
a provider API key in crucible's protected store. An API key can instead come
from the environment. [Providers and models](../providers/providers.md) has the
exact routes and precedence.

```bash
export ANTHROPIC_API_KEY=...
```

Type `/login` to choose an account or console-key route. ChatGPT offers a local
browser callback and a device code for remote terminals; Kimi Code offers a
device code. The live panel opens the authorization page, shows only the safe
page and one-time code, stays cancellable with Escape, and takes a masked
paste-back fallback for ChatGPT browser login. Anthropic has no account route;
choose Console account and enter an Anthropic API key.

`/login anthropic` is the direct API-key shortcut. The box draws a dot per
character and never the key. Account tokens and API keys go to
`~/.crucible/auth.json`, a file only you can read. The session asks that
provider from the next turn on — there is nothing to restart. Authentication
never chooses a model or effort; `/model` and `/effort` stay separate,
explicit choices.

You do not have to know that command to find it. A run holding no key for any
provider says so under the welcome and names both halves of setting one up:

```
Warning: No models available. Use /login or set an API key environment
variable. Then use /model to select a model.
```

The prompt is there underneath it, the way it is on every other run. crucible
does not stand a panel in front of that screen — the sentence is the whole
answer, and it stays readable while you type at the box under it.

## Run it

Start it in the directory you want it to work in. That directory is the
workspace root, and every path a tool touches is relative to it.

```bash
cd ~/code/my-project
crucible
```

It opens with a card naming the release and the root it is standing on, beside
the last few sessions started in this directory. The live row under the prompt
names the selected provider, model and effort when there is one. A separate row
under the card names the active non-secret
authentication source, such as a stored account or `OPENAI_API_KEY`. The card
still opens when a remembered provider has lost that credential; its provider,
model and effort remain inactive until `/login` or `/model` makes them usable.
The card fits itself to the terminal: two columns at eighty and above, one below
that, and under forty-six there is no frame at all — just what it is and where.
Under the card is the box:

```
╭──────────────────────────────────────────────────────────────────────────────╮
│ ›                                                                            │
╰──────────────────────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)                anthropic/claude-sonnet-5 · high
```

The box is as wide as the terminal, and a line longer than it wraps onto the
next row rather than scrolling sideways — so the box grows downwards as you
write. It stops at about half the window; past that the line scrolls under the
top edge and what you are writing stays in view.

The row under the box has two ends. On the left is the permission mode and the
key that steps it; on the right is which model the next turn goes to, whose it
is, and how hard it is being asked to think — written the way `--model` takes it
back, so what the row says is what you would type to ask for it again. A vendor
is named there because a model name says which model and never whose, and a
machine holding keys for two of them is a machine where that is a real question.
Both ends change while a session runs, and that
row is redrawn on every keystroke — which the card above it is not, since what
crucible has already written belongs to the terminal's scrollback.

The arrows move a character, <kbd>Ctrl</kbd> or <kbd>Alt</kbd> held with one
moves a word — as do <kbd>Alt-B</kbd> and <kbd>Alt-F</kbd> — and <kbd>Home</kbd>
and <kbd>End</kbd> reach the two ends. A word here is a run of anything that is
not a space, so a path is one word.

The mouse belongs to the terminal: the wheel scrolls, dragging selects, the
middle button pastes. Set `output.mouse` to `click` and crucible takes the
buttons instead, for the whole session, so clicking in the box between turns
puts the cursor where you clicked, and clicking a result that was cut short
stands it whole — at the price of the wheel, because the wheel is a button and
the scrollback it scrolls is where crucible's transcript lives. An inline
renderer cannot have both, which is why it is a choice rather than a default.

Under the box is the mode in force, every time — `ask mode on` is the one
nothing configured gives you. <kbd>Shift-Tab</kbd> steps to the next one while
you type, and the row and the colour of the box both follow it.
[Permissions](../permissions/index.md) is where all three are.

Type a prompt and press enter. The line stays where it was and the answer
streams in under it, with the box and the mode still standing at the bottom of
the screen — so a tool call arriving ten minutes into a turn is read beside the
mode that let it through.

You can go on writing in the box while the answer arrives. <kbd>Enter</kbd>
queues what you wrote as the next prompt, and it is run the moment the turn
ends. Up to 64 finished prompts and 1 MiB of their text can wait; when either
bound is full, <kbd>Enter</kbd> leaves the line in the box and the row beneath
it says why. <kbd>Esc</kbd> asks the turn to stop. The key that steps the mode is
not offered there, because the mode is away with the turn until it finishes, and
neither is <kbd>Ctrl+C</kbd>, which means the same thing there as at the prompt.

One row stands between the answer and the box for as long as the turn runs, and
a second joins it above while a tool is out:

```
✳ thinking (2m 56s · ↓ 12.8k · esc to interrupt)
```

The mark turns four times a second, and that is the part that says the program
is busy rather than stuck — a screen that has been still for a minute looks the
same either way. The word says what is being waited on: `thinking` for the
model, `writing` while prose is arriving, `running` while a tool has not
answered, `retrying` while a response that went away is being asked for again,
`compacting` while room is being made, and `interrupting` once <kbd>Esc</kbd>
has been pressed and the turn has not stopped yet. The clock counts from the
moment the prompt was sent and never pauses — not for a permission question,
which is time spent waiting just as much.

`↓` is what the turn has spent so far, counted in the tokens the model has
produced and added up across every response of the turn. It is written the way
it would be said — `840`, `1k`, `1.4k`, `128.4k` — with a tenth only where there
is one to write. It appears once the provider has said and not before, so a turn
shows no count for its first response — a provider that never reports and a
model that has produced nothing are different things, and only one of them is
worth a number. On a window too narrow for all of it the key goes first, the
count next and the clock after that, since all three are recoverable — the key
is named under the box, and the other two will be back next second; the word is
the last thing left.

A prompt queued while the turn runs is named directly under that row, so a line
that left the box has somewhere it can be read back:

```
✳ thinking (2m 56s · ↓ 12.8k · esc to interrupt)
  Next: fix the failing test
```

It is the one the next turn takes — the oldest of the queue rather than the last
you typed — and it starts in the column the word above it starts in, with no
mark of its own, since nothing about it has begun. A prompt too wide for the
window is cut at the right. Nothing parts it from the row above, because it is a
second line of that row rather than a second thing beside it. On a window too
short for everything standing over the box, this row goes before the one above
it does: the prompt is still in the queue and its own turn will say it, and the
row saying a turn is running is written nowhere else.

While room is being made, a second line under the word says why it is happening
and how far the notes have got:

```
✳ compacting (18s · esc to interrupt)
  ■■■■■■■■■■■□□□□□□□□□□□□□□□□□  39%  the window was full
```

The reason is one of three — the window filled, the model would not take another
request this size, or you asked. The bar measures how far the notes have run
rather than how much is left of them, because nobody knows where they end until
the model stops, and it appears with the first of them: until then the line is
the reason alone, since the model is still reading the session it is about to
write down.

The box under it is a box throughout. What you type reaches it, <kbd>Enter</kbd>
queues the line, and it is sent as the next turn once there is room — against
the session that has just been made smaller, which is why it waits rather than
going first. <kbd>Esc</kbd> stops the notes and leaves the session exactly as it
was.

Under everything the turn says, and over the box, is the plan — when the agent
has written one:

```
────────────────────────────────────────────────────────────────

3 tasks (1 done · 1 doing · 1 open)
■ Run the validation spikes
□ Design the architecture
✓ Build the gate script
```

The agent puts it there with a tool and rewrites it as the work moves, so the
panel is what it is working to rather than what it said it would do. `■` is the
task under way and it is the one warm mark on the screen; `□` is one nobody has
started; `✓` is one that is finished, struck through and toned down. The task
under way is drawn first whatever order the plan was written in, then what is
open, then what is finished with the most recently ticked off at the top of it.

Seven tasks are shown and the rest are counted — `… +4 more · ctrl+t to expand`.
<kbd>Ctrl+T</kbd> takes that bound off and puts it back, and what it adds arrives
underneath the rows already on screen, so nothing you were reading moves. On a
window with no room for all of this, the panel is measured before the rows around
it: the call line and the queued prompt give way first, since a call reaches your
scrollback the moment its tool answers and a queued prompt has its own turn
coming, while what the agent is working to is on screen nowhere else.

The panel does not come down when the turn does. What the agent was working to
is what the next prompt is typed against, so it stands over the box between
turns as well, and `/clear` is what puts it away.

Tool calls appear as they answer:

```
› what does the runner do when a tool fails?

● Read(crates/crucible-runner/src/runner.rs)
  └      1	//! The turn loop. (+238 lines · ctrl+o to expand)

A failed tool is not a failed turn: the failure goes back to the model as the
result of that call, and the model decides what to do about it.

╭──────────────────────────────────────────────────────────────────────────────╮
│ ›                                                                            │
╰──────────────────────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)                anthropic/claude-sonnet-5 · high
```

A call is the tool's name and, in brackets, the one thing the call is about: the
path for `read`, `write` and `edit`, the pattern for `grep` and `glob`, the
command line for `bash`, how many tasks are being written down for `todo_write`.
Each tool answers that for itself, because each knows
which of its arguments a person would recognise the call by. A call whose
arguments could not be read is just the name, and the tool says why next.

The mark and the name are in crucible's own colour and the brackets beside them
are toned down, so a screenful of calls reads as the tools that ran with what
each was given rather than as a paragraph. The colour is the row's rather than
the text's, so it costs no column, and it is there whether the line is still
waiting above the box or has already been written out.

That line is written when the tool answers rather than when the model asks for
it, so it and the result under it reach your scrollback one after the other and
nothing the turn did in between comes to stand between the two. While the tool
is out it stands above the working row instead, with its mark pulsing on the
beat the mark below it turns on, and it commits the moment the tool answers —
the same words in the same columns, with the motion gone. So a call still
waiting is told from one that has finished at a glance, and what reaches your
scrollback is the still line. On a window with room for one of the two, the call
gives way to the row that says the turn is running at all.

A tool's output is summarised to its first line and a count of the rest; `read`
numbers lines the way `cat -n` does, which is why the summary starts with a `1`.
It hangs under `└`, one column past the `●` that opened the call, so a result
belongs to the call above it at a glance rather than by being next to it. The
whole row is toned down, corner and words together, because the line above it
already says what was done and this is the detail under it. A call that failed
is marked `✗` there and only there — the call line stands as it was, since a
call that was made is a call that was made whatever came back — and that mark is
the one thing on the row left in your terminal's own foreground, so it is where
the eye goes.

A result the row had no room for says how much it left over and names the key
that gives it back: `(+128 lines · ctrl+o to expand)`. <kbd>Ctrl+O</kbd> stands
every result that was cut this way where the box was, newest first, each under
the line of the call it answers. The arrow keys walk it where there is more of
it than the window holds, and <kbd>Esc</kbd> or <kbd>Ctrl+O</kbd> again closes
it — the box comes back with the line you were typing still in it, and nothing
is written into your scrollback on either side of it.

The key works whether or not a turn is running. While one is, the view stands in
the rows the box has and the turn goes on writing above it, so what you are
reading stays where you left it rather than being pushed down the screen by the
next result. A command that has not answered yet stands there too, at the top,
since it is the newest thing there is — the end of what it has printed so far,
which is what the five rows over the box are a sample of. What it holds is what had been cut when you opened it; a turn that
cut more while you were reading is one press away, since opening it again is
what brings the newer results in.

With `output.mouse` set to `click`, clicking one of those rows stands that one
result rather than all of them — the row names the call you asked about, so the
answer is the output of that call alone. It reads the same as the key
otherwise, and works while a turn runs for the same reason. A click anywhere
else, on a row that offered nothing, leaves the screen as it was.

A call that changed a file is the exception, and says so by offering nothing. It
is shown as the change itself, and a change too long for the block is cut where
the change is built rather than where it is drawn — those lines are gone before
anything is drawn at all, so the count of them is the whole of what is still
true about them.

The answer itself is read as markdown rather than printed with the markers still
in it. A heading loses its hashes and stands out, one `*` or `_` around a phrase
leans on it and two raise its voice, backticks leave a run of code toned down,
and a fenced
block is toned for its whole length with the fence lines and the language
written on them gone. The tone belongs to the row rather than to the text, so it
costs no column: the answer wraps exactly where the same answer would have
wrapped plain.

An answer wider than the terminal wraps at the last space before the edge, so a
word arrives whole on one row rather than in halves on two. A word too long for
any row -- a path, a hash, a line of code with no spaces in it -- is still broken
where the row ends, since there is nowhere else to break it.

A line that wraps and opens with a mark -- an item's bullet, a task's box, a
quote's bar, a number and its dot -- continues under its own words rather than
back at the edge, so the mark is the only thing in its column and a list still
reads as a list at any width.

A list is read too. Whichever of `-`, `*` or `+` the model reached for, every
item opens with the same small mark, and the spaces that nest one list inside
another are kept exactly as they were written. A line that opens with `>` gets a
bar down its left and goes quiet for its length, because the words are somebody
else's. Both marks come out of the same set as every border on screen, so
`"output": { "glyphs": "ascii" }` changes them along with everything else.

A dash is only a bullet at the start of a line with a space after it. `5 - 3`,
`--colour never` and `a -> b` are left exactly where they were.

A line that is nothing but three or more `-`, `*` or `_` is a rule between the
blocks either side of it, and is drawn as one across the window rather than as
the three characters it was written with. Two are not enough, and `a --- b` is
left where it was.

An item that opens with `[ ]` or `[x]` is a task, and its box takes the bullet's
place rather than following it: an unfinished one gets a hollow mark, a finished
one gets a tick and its words go behind you, subdued and struck through. The
brackets have to open the item — `- see [TODO] in the grammar` is a bracket
somebody wrote, and it stays one.

A phrase between two `~~` is one the answer wrote and then took back, and it is
drawn with a line through it and nothing else — struck rather than dimmed,
because a retraction is still being read. Exactly two: `~/Projects` is a path,
`~40` is an approximation, and both are left where they were.

A table is drawn as a table. The bars a model wrote are replaced by one rule
between the columns and one under the header, every column is as wide as the
widest thing drawn in it, and `:--`, `--:` or `:-:` in the row of dashes says
which side a column is drawn against. Where the window cannot hold it, the table
gives up columns from whichever is widest until it fits, and a cell that no
longer fits ends in an ellipsis — so every row is exactly the width of the
window and the columns stay under each other. A window too narrow for even one
column apiece gets the table as the model wrote it.

A bar is only a table at the start of a line, and only where the line under it
is the row of dashes that makes one. `a | b` in a shell, `Ok(_) | Err(_)` in a
match and a line of bars with nothing under it are all left where they were.

A link is read the same way, and keeps both halves of itself: the words are
underlined and the address follows them in brackets, quietly, so it can be
copied — or clicked, in a terminal that finds its own links. A bracket that was
not a link is left exactly as it was written.

Where there is no colour to read it into, the markers are left where the model
put them. That covers a redirected run, `NO_COLOR`, and `--color never` — taking
a marker out there would drop the emphasis and put nothing in its place, and
`crucible < prompts.txt > answers.md` is a file of markdown worth keeping.

<kbd>Ctrl+C</kbd> throws away a line you are part-way through, and does it whether
or not a turn is running. Against an empty box it offers to leave — `press ctrl+c
again to leave`, under the mode — and a second press within two seconds takes the
offer. Any other key first takes it back, so a session is never ended by one
stray keystroke. <kbd>Ctrl+D</kbd> on an empty box leaves at once.

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
| `/login` | Signs in with your provider account |
| `/logout` | Signs out from your provider account |
| `/mode` | The [permission mode](../permissions/modes.md) in force, or the one you name |
| `/theme` | Picks the colours crucible draws with, and the one code is drawn in |
| `/resume` | Lists what was worked on in this directory, and picks one back up |
| `/compact` | Replaces what is behind you with the model's own notes on it, making room |
| `/clear` | Starts a new session, leaving this one on `/resume` |
| `/exit` | Ends the session |

`/model` on its own stands a panel where the prompt box was, holding every
provider this build serves beside a few of the models each offers, with the one
being asked now named above them. Taking a row off another provider's half moves
the session to that provider. Escape leaves it and changes nothing. `/model
<name>` skips the panel, and is also how to ask for a model the panel does not
carry: the list is a shortcut past the vendor's documentation, not the limit of
what the vendor serves.

Either way the name is written to `~/.crucible/config.json` under the provider
this run is set up for, and the provider beside it, so the next crucible
started anywhere begins with both. See [Providers and
models](../providers/providers.md).

`/theme` stands a list of themes where the prompt box was, with a specimen
beside it drawn in whatever your marks are standing on. Moving a mark redraws
it, so the choice is made by looking rather than by reading a name. Enter takes
it and writes it to `~/.crucible/config.json`; escape puts back what was in
force and changes nothing. `/theme <name>` skips the list.

There are two lists, and the left and right arrows step between them. **interface**
is the one above — borders, marks, the mode in force, the ground a diff takes.
**code** is which theme fenced code is drawn in, and its list holds the names you
already know: Monokai Extended, GitHub, Dracula, Nord, gruvbox and the rest.

The specimen shows both at once, which is why it is a diff. The rows a change
touched carry a ground, and that is the interface theme's; the rows it did not
are free to be read, and that is the code theme's. Move either mark and the half
it decides changes.

What is already in your scrollback keeps the colours it was drawn in. crucible
draws into the terminal's own buffer and never goes back over what it has
written, so a theme changes what comes next. See
[Configuration](../configuration/configuration.md#output).

`/effort` asks the same question over the rungs the model in force serves, and
the answer is written to the same file beside the model. It draws a ladder
rather than a panel: one track with the rungs under it, `Faster` at one end and
`Smarter` at the other, walked with the left and right arrows. The mark opens on
`high` where nothing has chosen yet, which is a place to start walking from
rather than a rung being asked for: leaving it leaves the session asking for
none, and what applies then is the vendor's own default for that model. The
ladder holds what the model serves rather than all five — the Kimi models serve
`low`, `high` and `max`, and a model whose vendor serves none is told so instead
of being offered a ladder that cannot be answered. A session with no model
chosen is sent to `/model` first, since a rung is asked of a model.

`/login <provider>` opens a box for the key — never the command line, which
would put it in your shell's history, in the process listing and in the
scrollback. Escape leaves it without writing anything.

`/login` on its own asks how crucible should sign its requests, which is a
different question from which vendor: somebody paying for a ChatGPT plan and
somebody holding an OpenAI console key are two people, and only one of them has a
key to type. So the panel offers three ways — OpenAI's ChatGPT Plus, Pro,
Business and Enterprise plans; MoonshotAI's Kimi Code; and a console account
billed by API usage. The two plans connect: ChatGPT opens a browser
authorization, or a device code from a terminal with no browser to reach, and
Kimi Code a device code — either writes a renewable credential to the same
protected store a key goes to. The console account asks whose console before
opening the box, and is the route an Anthropic key takes, Anthropic having no
account route.

A run with no keyboard to walk that panel — and a window with no room to stand
one in — gets the provider names as rows instead, with the variable each reads
from.

A key that is written lands on the session that took it: the provider is set up
there and then, from the next turn on. Logging in chooses neither a model nor a
rung — `/model` is the explicit next step where nothing has chosen one. A run
started with no key for anything is one command away from a turn, and the line
under the box says which model it will be asking — or sends you to `/model`
where the files name none.

`/logout` is the same panel over what is actually there: the providers a key was
written down for, and nothing else. `/logout <provider>` forgets that one
directly, and a name with no key here says so and lists the ones that have. It
reaches `~/.crucible/auth.json` and only that — a key exported into your shell is
untouched and goes on winning — which is what the line under the answer says.

`/clear` starts a new session with nothing said in it: the next prompt is the
first one the model sees, and the turns before it are neither sent nor paid for
again. The session you were in is finished rather than dropped — its log is
complete and it is on `/resume`'s list, so everything said in it can be picked
up whole. What does not come across is what that session allowed for the rest of
itself, the record of which files it read, or the plan standing over the box,
all three of which belonged to it — a plan that outlived its session would
describe work the agent has no memory of. The panel comes down with it; the
mode does not move, because it is where you are running crucible rather than
something a session decided. The screen is left alone, because what is above the
box is the terminal's scrollback rather than crucible's.

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
The plan comes back with it, standing over the box where it stood, because the
call that wrote it is in the transcript being replayed. The number is the row on
the list you were just shown, so read it again if something else has been
recorded here since.

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

<kbd>Esc</kbd> during a turn asks that turn to stop, and leaves the session
where it was. Nothing is killed: the provider stops between reads and a command
stops between the steps it takes, so a file a tool was writing is either
untouched or finished. What was on screen stays on screen, what you had typed
stays in the box, and the next prompt carries on the same session.

## What it can do

Ten tools, advertised in the order a model tends to reach for them. Six are
always in the list. The rest are **held back**: they exist and they work, and the
agent does not see them until it looks them up with `tool_search`. A schema the
agent can see is one it pays for on every request of every turn, and most
sessions never write a plan or ask a question about the world.

| Tool | What it does | Asks first |
| --- | --- | --- |
| `read` | Reads a file | no |
| `grep` | Searches file contents | no |
| `glob` | Finds files by pattern | no |
| `edit` | Replaces text in a file | yes |
| `write` | Creates or overwrites a file | yes |
| `bash` | Runs a command | yes |
| `todo_write` | Writes down the plan | no |
| `web_search` | Searches the web | yes |
| `web_fetch` | Reads one web page | yes |
| `tool_search` | Finds a tool that is not in the list | no |

Reads never ask. Anything that changes a file or starts a process does, until
you configure rules or a mode that answer for you — see
[Permissions](../permissions/index.md). [Tools](../tools/index.md) is what each
one takes, what bounds its answer, and what it says when it hits that bound.

`write` puts down a whole file, so it refuses to overwrite one this session has
not read or written itself, and says so rather than ending the turn. The model
reads the file and writes it again. That covers the case a permission prompt
cannot: a `write` you approve is one you agreed to, and neither of you can see
that the file holds work nobody looked at.

`bash` runs its command through a POSIX shell in the workspace root, and starts
it with a short list of variables — `PATH`, `HOME`, the locale — rather than the
environment crucible is running in. Your provider key is not on that list, so a
command that prints the environment prints no key. Anything else a command needs
is named in [`env`](../configuration/configuration.md#env).

The shell and its descendants are one command scope. When the command exits,
times out, is cancelled, or cannot be collected, crucible stops that scope and
waits only for a bounded interval: a background process does not keep an output
reader or a turn alive. Windows uses a kill-on-close job and Unix a process
group. A Unix program that deliberately creates a new session can leave that
group; this is resource cleanup, not a sandbox around an allowed command.

`todo_write` is the one that reaches nothing at all. It puts down the plan the
agent is working to — a list of at most 64 tasks, each of them a line, each one
of `open`, `doing` and `done` — and you read it as a panel above the box. Every
call replaces the whole plan, so what the model thinks the plan is and what you
are looking at are one thing rather than two. [Writing down the
plan](../tools/planning.md) is the rest of it.
