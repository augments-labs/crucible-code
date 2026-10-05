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
from source, or take the archive from the release before it; the version it
holds is the version it says.

The Linux release workflow builds dynamically linked binaries against glibc
2.34. Debian 12, Ubuntu 22.04, RHEL 9 and anything later are fine; older than
that has to build from source. The already-published 0.1.6 Linux artifacts
predate that build and need glibc 2.39; release artifacts cannot be changed in
place. A binary from the current release workflow asks the system for nothing
else: no certificate bundle, no runtime to install. `scripts/sh/smoke.sh` is
what keeps that true, by running each release in a sandbox holding the binary
and its two libraries and nothing besides.

The one thing crucible does look for is a POSIX shell, and only when the `bash`
tool runs a command. Every platform here has one except Windows, where it is
whichever `sh.exe` is on the `PATH`. [Git for
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
`cru` alias in `~/.local/bin`. On Linux and macOS it also installs
`crucible-sandbox-broker` beside `crucible`; confined commands use this native
helper, and it is trusted only when every directory above it belongs to root
or to you and is writable by neither group nor others; the installer points out
a directory that breaks that rule, with the `chmod` that fixes it. It
never asks for `sudo` or edits a shell profile. Use `--version`, `--dir` or
`--dry-run` when the defaults are not the ones you want. The matching
`uninstall.sh` removes only those executables and preserves `~/.crucible`;
deleting configuration, credentials and sessions requires the explicit
`--purge --yes` pair. In a terminal the installer shows each step as it runs,
with a bar while the archive downloads; piped, or under `NO_COLOR` or
`TERM=dumb`, it prints one plain line per step instead. Either way a failure
while it detects the platform, downloads, verifies, unpacks or installs names
that step.

On Windows, in PowerShell 5.1 or 7, run the release installer:

```powershell
irm https://github.com/augments-labs/crucible-code/releases/latest/download/install.ps1 | iex
```

It detects the architecture, verifies the Windows archive against the release's
`SHA256SUMS` before unpacking it, and installs `crucible.exe`,
`crucible-sandbox-broker.exe` and a `cru.exe` copy in
`%LOCALAPPDATA%\Programs\crucible\bin`, without asking for elevation. It
points out that directory, or one above it, when other users can change it,
since they could then replace what it installed. It says
whether that directory is on your `PATH` and how to add it, but changes `PATH`
only when asked to. To pass options, such as `-AddToPath`,
`-Version`, `-Dir` or `-DryRun`, run it as a script block:

```powershell
& ([scriptblock]::Create((irm https://github.com/augments-labs/crucible-code/releases/latest/download/install.ps1))) -AddToPath
```

For a manual Unix install, download the archive and `SHA256SUMS` from the
[releases page](https://github.com/augments-labs/crucible-code/releases). On
Linux use `sha256sum -c`, on macOS use `shasum -a 256 -c`, and on FreeBSD use
`sha256 -c`, with a checksum file narrowed to the downloaded archive. Then
unpack it with `tar xzf` and copy `crucible` into a directory on `PATH`. On
Linux and macOS copy `crucible-sandbox-broker` into the same directory.

Each Windows target also ships the executable on its own, beside the archive and
named the same way (`crucible-<version>-windows-x86_64.exe`), so there is
nothing to unpack. `SHA256SUMS` covers those too; PowerShell verifies it with
`(Get-FileHash .\crucible-<version>-windows-x86_64.exe -Algorithm SHA256).Hash`
before the executable is moved into a directory on `PATH`.

For Windows sandboxing, download the matching archive and keep both
`crucible.exe` and `crucible-sandbox-broker.exe` together. The broker is also
available separately as `crucible-sandbox-broker-<version>-windows-x86_64.exe`
(or `windows-aarch64.exe`); verify its checksum, rename it to
`crucible-sandbox-broker.exe`, and place it beside Crucible. Follow the
[administrator setup instructions](../security/sandboxing.md#windows-setup-maintenance)
before enabling the sandbox. A standalone Crucible executable without its
broker supports sessions with sandboxing disabled.

## Build it

Rust is pinned in `rust-toolchain.toml`, so rustup fetches the right toolchain
on the first build. This is the path for a platform not in the table above, and
for working on crucible itself.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build --release -p crucible-code -p crucible-sandbox-broker
./target/release/crucible --version
```

## Sign in or give it a key

`/login` inside a session can sign in to a ChatGPT or Kimi Code account, or keep
a provider API key in crucible's protected store. An API key can instead come
from the environment. [Providers and models](../providers/providers.md) has the
exact routes and precedence.

```bash
export ANTHROPIC_API_KEY=...
```

Type `/login` to choose how usage is paid for: *Your account with subscription*,
usage included in your paid plan, or *Provide your own API key*, billed by API
usage. The first lists the accounts: OpenAI, and Kimi Code on kimi.ai (accounts
outside mainland China) or on kimi.com (mainland China accounts); and the plans
whose own key you type in, MiniMax on either of its sites and Qwen's Coding Plan
and Token Plan on either of Alibaba Cloud's. The second
lists the providers whose key you may hold, each with the variable it reads
from, `set ANTHROPIC_API_KEY` and so on, for anyone who would rather export a
key than store one. A row that holds your credential says `signed in`. Escape
goes back one screen at a time and cancels from the first.

ChatGPT offers a local browser callback and a device code for remote terminals;
Kimi Code offers a device code. The live panel opens the authorization page,
shows only the safe page and one-time code, stays cancellable with Escape, which
also closes the browser callback so the next `/login` can start, and takes a
masked paste-back fallback for ChatGPT browser login. Anthropic and Google have
no account route.

Words after `/login` narrow the rows: `/login openai` shows OpenAI's two,
`/login kimi.ai` the two on kimi.ai, and words that leave one row open it at
once. `/login anthropic` opens the key box directly, and `/login google` does
once you have answered whether Google may use what is sent on unpaid quota
(see [content use](../providers/content-use.md)). Google also
reads `GEMINI_API_KEY`; for example, select `google/gemini-3.8-flash` in `/model`
after exporting that variable or storing a key. The key goes into its own
labelled box, which takes a paste as readily as typing and draws a dot per
character, never the key. Enter saves it; Enter on an empty box does nothing.
Account tokens and API keys go to `~/.crucible/auth.json`, a file only you can
read. The session asks that provider from the next turn on; there is nothing to
restart. Authentication never chooses a model or effort; both stay explicit
choices, and `/model` is where they are asked together.

Some vendors say they may use what you send to train or improve their models:
a ChatGPT plan, Kimi Code and the Kimi open platform, a Gemini key on unpaid
quota, MiniMax, Qwen's plans on aliyun.com, Z.ai's bigmodel.cn key, and Meta's
two contributor models. Before the first message on one of those goes, crucible asks you once,
with what the vendor's terms say and where; **Use it anyway** sends it
and is remembered, **Go back** keeps the message and sends nothing. [Content
use](../providers/content-use.md) lists each route and what its vendor says.

You do not have to know that command to find it. A run holding no key for any
provider says so under the welcome and names both halves of setting one up:

```
Warning: No models available. Use /login or set an API key environment
variable. Then use /model to select a model.
```

The prompt is there underneath it, the way it is on every other run. crucible
does not stand a panel in front of that screen: the sentence is the whole
answer, and it stays readable while you type at the box under it.

## Run it

Start it in the directory you want it to work in. That directory is the
workspace root, and every path a tool touches is relative to it.

```bash
cd ~/code/my-project
crucible
```

It opens with a card naming the release and the root it is standing on, beside
the last few sessions started in this directory. The card
still opens when a remembered provider has lost its credential; its provider,
model and effort remain inactive until `/login` or `/model` makes them usable.
The card fits itself to the terminal: two columns at eighty and above, one below
that, and under forty-six there is no frame at all, just what it is and where.
Under the card is the box:

```
/home/you/code/my-project

╭──────────────────────────────────────────────────────────────────────────────╮
│ ›                                                                            │
╰──────────────────────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)              anthropic · claude-sonnet-5 · high
```

The box is as wide as the terminal, and a line longer than it wraps onto the
next row rather than scrolling sideways, so the box grows downwards as you
write. It stops at about half the window; past that the line scrolls under the
top edge and what you are writing stays in view.

Text pasted into the box keeps its shape. A tab arrives as the four columns it
stood for, so a snippet written with tabs is still indented, both on screen and
in the prompt that is sent. Anything else a terminal can hide in a paste is left
out, since drawn it would move a cursor this process had already placed.

<kbd>Ctrl+V</kbd> reads an image from the operating-system clipboard. It
imports the PNG into the session's private attachment store and puts a marker
such as `[Image #1]` in the box, numbered in the order the images were pasted;
ordinary text paste still uses the terminal's paste action. The marker can be
written into any later prompt of the same session to attach that image again,
and a clipboard holding a copied image *file* (a path, or a `file://` address
the way a file manager copies one) pastes the picture it points at. A clipboard
image can therefore be sent without a file in the project, and a pasted image
remains available to `--continue`. When nothing readable is on the clipboard,
the reason is written under the box and the next keystroke clears it.

A path can do the same explicitly. A relative image path names a file in the
workspace as before; an absolute image path outside it is imported rather than
making that directory reachable to tools. Put single or double quotes around a
path containing spaces, for example:

```
describe '/home/you/Pictures/Screenshots/Screen Shot.png'
```

The copy is content-addressed under crucible's session directory, so moving or
deleting the original does not change what the transcript sends later.
[Attachments](../sessions/attachments.md) has the rest: what may be attached,
the ceilings, and what a resumed session sends.

What the transcript shows of a sent attachment is its label and its number:
`[Image #1]`, `[Video #1]`, counted per kind in the order they were attached.
Not the path: where the file came from is a detail of getting it here, the copy
that was sent lives somewhere you did not choose anyway, and a row of home
directory is a row of the screen spent on neither.

The row under the box has two ends. At the left is the next key: the permission
mode in force, the key that steps it, and how many commands are still running
behind the box. At the right is what the session is talking to: the provider,
the model, the rung it is being asked on where one has been chosen, and `fast`
after an answer its vendor served fast. It sits beside the box because every
key that changes it is typed into that box.

The three are joined by a dot, `anthropic · claude-sonnet-5 · high`, and read
the same on the `/model` panel and in its answer. What you type keeps its own
form, `/model anthropic/claude-sonnet-5`. The vendor is named because a model
name says which model and never whose. A machine holding keys for two of them is
a machine where that is a real question.

The row is redrawn on every keystroke, and both ends move while a session runs:
<kbd>Shift-Tab</kbd> steps the left, `/model`, `/effort`, `/fast` and `/login`
change the right. Where the window is too narrow for both, the right end gives way whole
rather than being cut, and the mode keeps its place.

The arrows move a character, <kbd>Ctrl</kbd> or <kbd>Alt</kbd> held with one
moves a word (as do <kbd>Alt-B</kbd> and <kbd>Alt-F</kbd>), and <kbd>Home</kbd>
and <kbd>End</kbd> reach the two ends. A word here is a run of anything that is
not a space, so a path is one word.

<kbd>↑</kbd> walks back through the prompts you have already sent from this
directory, newest first, up to a hundred of them, and <kbd>↓</kbd> walks forward
again. The line does not have to be empty: whatever you had typed is set aside
on the first step back, and stepping forward past the newest prompt puts it back
in the box.
The top border of the box says where you are: the prompt's chronological
position within this window, such as `history 80/100` on the first press back
when eighty prompts are retained, counting down toward `history 1/100` for the
oldest. Enter sends whatever is in the box. Moving the cursor along it does not
end the walk. Edit the line, by so much as a <kbd>Backspace</kbd>, and the walk
ends where you edited it: the count goes, the line is yours again, and the one
you had set aside is not coming back.

The prompts are kept between sessions, in one `prompt.history` file beside the
sessions, with the directory each was sent from. A line you sent in one checkout
is never offered under the arrow key in another, and each directory keeps at
most a hundred: the hundred-and-first prompt you send here is what the first one
is spent on, and it goes from the file rather than sitting in it unreachable.
The file holds at most 512 prompts across every directory, so prompts sent
elsewhere can push this directory's oldest out. A prompt longer than 1024 bytes,
usually a paste, is not kept at all.

Where the prompt is more than one line, <kbd>↑</kbd> moves up through it and
only reaches the history from the first line, and <kbd>↓</kbd> only from the
last. A long line that the box has wrapped onto a second row is still one line,
and the arrows go straight to the history from it. While a list is standing over
the box the arrows are its alone, and the history does not answer at all, even
at either end of the list.

The wheel scrolls the transcript, and goes on scrolling it while a list or a
panel stands over it, except where what is standing is itself a window over more
text than fits, which the wheel walks instead. How far one notch goes is
[`CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`](../configuration/configuration.md#crucible_code_mouse_scroll_speed),
six rows unless you say otherwise. Text arriving below while you read back does
not take you away from it.

A click puts the cursor where you point in the box, marks a row of a list, or
opens a result the transcript cut short.

Resting the pointer on one of those results lights it, in your own foreground
rather than the quiet the rest of the transcript is in: every row of that one
result where it took more than one, and no row of any other. What lights is what
a click there opens, so the two always name the same result.

A drag selects, and crucible is what answers it: the rows you cover light up as
you go, and letting go puts them on your clipboard. It runs over the whole
window rather than over the transcript alone (an answer, the row above the box
and the box itself are one drag if that is what you covered), and the first and
last rows are taken from where you pressed and to where you let go, the way a
selection in any other window behaves. A drag that reaches the top or the foot
of the transcript scrolls it a row at a time for as long as the pointer rests
there, and the wheel scrolls it while the button is still down, so a selection
can run past what one window shows: the highlight stays on the words it began
on, and what scrolled off the window is copied with the rest. Resizing lets go
of it, because a new width moves the words out from under the two ends. Holding
<kbd>Shift</kbd> while you drag still hands the pointer back to your terminal,
if its own selection is the one you wanted. A press on the scroll rail, the
transcript's last column, scrolls rather than selects, and no selection takes
the rail.

What a drag over the box takes is the picture: a border down each side, and
blank ground out to the last column. So <kbd>Ctrl+Y</kbd> is still there for the
line itself, exactly as you typed it and with nothing around it, and the row
under the box says it went. It works while a turn is running too, since the line
is yours either way.

Under the box is the mode in force, every time; `ask mode on` is the one nothing
configured gives you. <kbd>Shift-Tab</kbd> steps to the next one while you type,
and the row and the colour of the box both follow it.
[Permissions](../permissions/index.md) is where all three are.

Type a prompt and press enter. The line stays where it was and the answer
streams in under it, with the box and the mode still standing at the bottom of
the screen, so a tool call arriving ten minutes into a turn is read beside the
mode that let it through.

You can go on writing in the box while the answer arrives. <kbd>Enter</kbd>
queues what you wrote, and it is offered to the turn that is running: a turn
still working takes it at its next step and adjusts course, and one that was
already finishing leaves it to be answered as its own turn. Up to 64 finished prompts and 1 MiB of their text can wait; when either
bound is full, <kbd>Enter</kbd> leaves the line in the box and the row beneath
it says why. <kbd>Esc</kbd> asks the turn to stop. <kbd>Shift-Tab</kbd> still
steps the mode, but the running turn keeps the one it started under. The row and
the colour of the box show the mode you stepped to at once, so for the rest of
the turn they name the mode the next turn will run in rather than the one this
turn's tools are asked under. The step is put on the session as this turn
ends, so the row between turns still shows it. <kbd>Ctrl+C</kbd> means what it
means at the prompt: it throws away the line in the box, and against an empty
box it offers to leave, where a second press within two seconds asks the turn
to stop and leaves.

One row stands between the answer and the box for as long as the turn runs, and
a second joins it above while a tool is out:

```
✳ thinking (2m 56s · ↓ 12.8k · esc to interrupt)
```

The mark turns four times a second, and that is the part that says the program
is busy rather than stuck; a screen that has been still for a minute looks the
same either way. The word says what is being waited on: `thinking` for the
model, `writing` while prose is arriving, `running` while a tool has not
answered, `retrying` while a response that went away is being asked for again,
`compacting` while room is being made, and `interrupting` once <kbd>Esc</kbd>
has been pressed and the turn has not stopped yet. The clock counts from the
moment the prompt was sent and never pauses, not even for a permission question,
which is time spent waiting just as much.

`↓` is what the turn has spent so far, counted in the tokens the model has
produced and added up across every response of the turn. It is written the way
it would be said (`840`, `1k`, `1.4k`, `128.4k`), with a tenth only where there
is one to write. It appears once the provider has said and not before, so a turn
shows no count for its first response. A provider that never reports and a model
that has produced nothing are different things, and only one of them is worth a
number. On a window too narrow for all of it the key goes first, the count next
and the clock after that, since all three are recoverable: the key is named
under the box, and the other two will be back next second. The word is the last
thing left.

The prompts waiting behind the turn stand in a panel over the box, framed the
way the box is because they are the same thing a moment apart. They are your own
words, one of them being typed and the rest already sent for:

```
╭─ 4 queued ───────────────────────────────────────────────╮
│ › fix the failing test                                   │
│ › then run the gate                                      │
│ › and write the changelog                                │
│   … +1 more                                              │
╰──────────────────────────────────────────── ctrl+q edit ─╯
```

The bottom edge names the key that opens the queue, for one waiting prompt as
for many. Three are named and the rest are counted, oldest first, which is the
order they will be said in. A line too wide for the window is cut at the right.
On a window too narrow to open a frame the panel is one indented row saying
how many are waiting, since that is the fact that cannot go, and on one too
short for everything standing over the box it gives its rows up before the row
saying a turn is running does: a queued prompt has its own turn coming, and
that row is written nowhere else.

They go together. When the turn ends the whole queue is one turn: the oldest is
its prompt and the rest are handed to the same turn before it asks anything, so
the model reads all of it and then answers all of it. Three lines typed behind a
turn are one thing you wanted said, and answering the first before reading the
third is working to a question you had already added to. Each is still its own
message, in the order you typed it; nothing is joined into a prompt you did not
write. A turn that stopped on a used-up plan is the exception: the queue waits
over the box, where <kbd>Ctrl+Q</kbd> opens it to edit or delete, until you send
a prompt, since sent on its own it would reach a plan that is spent.

<kbd>Ctrl+Q</kbd> stands the whole queue where the box was, with a footer naming
the keys that work. Up and down walk it, <kbd>e</kbd> takes the marked line back
into the box to be edited or sent ahead of the rest, <kbd>d</kbd> deletes it
without taking it back, and <kbd>Esc</kbd>, or <kbd>Ctrl+Q</kbd> again, closes
it. While it stands it has the keyboard, so <kbd>Esc</kbd> there closes the view
rather than interrupting the turn. In a window too short for the whole queue the
view scrolls, so the line the keys act on is always drawn, from its first row.

Nothing leaves the queue while it stands open. The turn above goes on writing,
tools go on running, the answer goes on arriving; what waits is the one moment
those lines would cross into the transcript, and it waits exactly as long as you
hold the view open. A line already in the transcript cannot be taken back, which
is why the ones you are still going over are kept out of it. Closing the view
gives the whole batch up at once, edited and untouched alike, and the turn works
them in at its next pass.

While room is being made, a second line under the word says how far the notes
have got:

```
✳ compacting (18s · esc to interrupt)
  ■■■■■■■■■■□□□□□□□□□□□□□□□□□□  39%
```

The bar measures how far the notes have run rather than how much is left of
them, because nobody knows where they end until the model stops. It appears with
the first of them; until then there is no second line, since the model is still
reading the session it is about to write down. Why room was made is said when
it is done, in [the record that joins the
transcript](../sessions/sessions.md#when-the-window-fills): the window was
full, the model would not take another request this size, you asked, or you
chose notes over carrying a picked-up session whole.

The box under it is a box throughout. What you type reaches it, <kbd>Enter</kbd>
queues the line, and it is sent as the next turn once there is room, against
the session that has just been made smaller, which is why it waits rather than
going first. <kbd>Esc</kbd> stops the notes and nothing is replaced, though old
tool output cleared to make room before they began stays cleared.

Under everything the turn says, and over the box, is the plan, when the agent
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

Seven tasks are shown and the rest are counted: `… +4 more · ctrl+t to expand`.
<kbd>Ctrl+T</kbd> takes that bound off and puts it back, and what it adds
arrives underneath the rows already on screen, so nothing you were reading
moves. On a window with no room for all of this, the panel is measured before
the rows around it: the call line and the queued prompt give way first, since a
call joins the transcript the moment its tool answers and a queued prompt has
its own turn coming, while what the agent is working to is on screen nowhere
else.

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
ask mode on (shift+tab to cycle)
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

Where the model asked for one tool and nothing about it moves, the line is
written the moment the call goes out. `WebSearch` and `WebFetch` are the two
that matter here: they print nothing while they run, no key points at them, and
their row says the same thing at the end that it said at the start. A fetch can
be out for half a minute, and reading it in the transcript is better than
watching the bottom of the screen for it.

The rest wait for their tool. A command that is still printing has its output to
show and `ctrl+b` pointing at it, and a response that asked for four tools at
once has them run one after another in the order asked, so writing all four rows
up front would put each result a row or more away from the call it answers. Those stand above the working
row instead, with the mark pulsing on the beat the mark below it turns on, and
each commits the moment its own tool answers: the same words in the same
columns, with the motion gone. So a call still waiting is told from one that has
finished at a glance, and the result lands under the call it answers with
nothing between the two. On a window with room for one of the two, the call
gives way to the row that says the turn is running at all.
None of it is drawn until the call has been out for three seconds
([`output.pinAfterSeconds`](../configuration/configuration.md#output)): one
that answers sooner goes to the transcript and nowhere else, so a quick command
does not flash a row over the box. The wait is only about drawing: `ctrl+b`
leaves a command running from the moment it starts, before its row is shown.

Where a response asked for several at once, that row counts them instead of
naming one (`8 WebFetch`, or `2 WebFetch and 7 Read`), and the count falls as
each answers and joins the transcript. A model that fetches ten pages in one
breath would otherwise hold the first of the ten over the box until it came
back, which says nothing about the nine still out. A command in the batch is
still named, since `ctrl+b` points at a row rather than at a number.

A tool's output is summarised to its first line and a count of the rest; `read`
numbers lines the way `cat -n` does, which is why the summary starts with a `1`.
It hangs under `└`, one column past the `●` that opened the call, so a result
belongs to the call above it at a glance rather than by being next to it. The
whole row is toned down, corner and words together, because the line above it
already says what was done and this is the detail under it. A call that failed
is marked `✗` there and only there; the call line stands as it was, since a call
that was made is a call that was made whatever came back. That mark is the one
thing on the row left in your terminal's own foreground, so it is where the eye
goes.

A result that opens like a record is squeezed onto that line instead of cut to
it. `gh pr view --json …` comes back pretty-printed over thirty rows whose first
line of content is whichever key sorts first, and `"assignees": []` spends the
reader's only line on nothing; squeezed, the row reads as much of the record as
the window holds, in the order it arrived. Two characters decide it: a brace or
a bracket, and something a record could hold after it. So `[exit status 3]` and
a sentence somebody wrote in brackets keep their spaces and their meaning.

Tool headings and result previews stay on one row, with an ellipsis inside
the closing parenthesis when arguments were clipped: `Bash(cat …)`.
A recent long command stays expandable even when it produced no
output; an individual call with only horizontally clipped text offers
`(ctrl+o to expand)` without a count of extra lines.
What is held in memory for expansion is bounded, and the oldest results are let
go as newer ones arrive. Where the session has a log, a row whose result was let
go of still opens: the result is read back from the log when the view reaches
it. A row with nothing left to open stops offering. Background completion
notices also shorten long commands,
preserving their exit status and output line count.

A result the row had no room for says how much it left over and names the key
that gives it back: `(+128 lines · ctrl+o to expand)`. The key is drawn in the
accent, and what the result said reads as the quiet the rest of the transcript
is in until you point at it. Clicking it opens the same view the key does, for
the one result it belongs to. <kbd>Ctrl+O</kbd> stands every result the rows
offer where the box was, newest first, each under the line of the call it
answers. A result no longer held in memory is read back from the session log
when the view reaches it; one the log cannot give back says
`! this result could not be read back from the session log` in its place.
<kbd>↑</kbd> and <kbd>↓</kbd> move a row up or down; the mouse
wheel uses `CRUCIBLE_CODE_MOUSE_SCROLL_SPEED` (six rows per notch by default).
<kbd>PgUp</kbd> and <kbd>PgDn</kbd> move by the rows the view shows, less one.
<kbd>→</kbd> and <kbd>←</kbd> put the next older or newer result at the top of the view.
Long headings and output lines wrap here so their ends remain readable.
<kbd>Esc</kbd> or <kbd>Ctrl+O</kbd> again closes
it: the box comes back with the line you were typing still in it, and nothing is
written into the transcript on either side of it.

The key works whether or not a turn is running. While one is, the view stands in
the rows the box has and the turn goes on writing above it, so what you are
reading stays where you left it rather than being pushed down the screen by the
next result. A command that has not answered yet stands there too, at the top,
since it is the newest thing there is: the end of what it has printed so far,
which is what the five rows over the box are a sample of. What it holds is what
had been cut when you opened it; a turn that cut more while you were reading is
one press away, since opening it again is what brings the newer results in.

The row names the call you asked about, so what stands is the output of that
call alone. A click anywhere
else, on a row that offered nothing, leaves the screen as it was.

A run of calls that only looked around is one row rather than one row each.
Local file lookups and web research in the same batch are counted together:

```
● Searched for 3 patterns, read 2 files, searched the web 2 times, fetched 4 pages
```

Only the kinds that happened are named, always in this order: searched for
patterns, read files, listed directories, ran commands, searched the web,
fetched pages. Each carries its own
number: a run that read one file says `read 1 file`. Two calls are enough to
fold; a single lookup keeps the row it always had, since a count of one is the
same width as the name it replaced and says less.

The grouped sentence is highlighted on hover and clickable: opening it shows
all its queries, URLs, commands and results. It has no `ctrl+o to expand` suffix.
That hint belongs to individual calls; <kbd>Ctrl+O</kbd> still opens grouped
results too. While a run is active, its status uses present tense: `Searching
for 3 patterns, reading 2 files, searching the web 2 times, fetching 4 pages`.

What folds is successful lookup calls. `grep` counts a pattern, `read` a file,
`glob` a directory, `web_search` a search, and `web_fetch` a page request. A Bash
call counts once, even if it chains several shell commands. Supported reporting
forms include `git status`, `gh pr view`, `ls`, `cat`, a `cd` followed by such
commands, and `sed -n 'N[,M]p' path` with explicit paths for printing a line range.
Wildcard paths and extra options keep a `sed` call individual. This classification
changes only display, never permissions. A failed lookup, a call that writes a
file, or an unrecognized command ends the run and keeps its own result row.
The run also ends wherever the turn does something else worth
reading: a paragraph of the answer, your next prompt, a stop.

And it ends at every round trip: the agent asked for a batch of tools, was
answered, and is going back to the model for more. So a line joins the
transcript each time the agent comes back rather than once the whole turn is
over, and a turn that spends two minutes looking around is two minutes of rows
appearing instead of two minutes of empty screen.

Nothing is folded away. Clicking the line opens every result in the run at once,
in the same view a single result opens in, each under the call it answers, so
the run costs one row and keeps all of it. The line lights up under the pointer
from its mark to its last character, because those cells are the door; the
blank after it on the same row is not.

While the run is still going the counters are in the present
(`Searching for 1 pattern, reading 4 files`), on its own row over the box, above
the call that is out. It wears the same mark as that call and blinks on the same
beat, since the two are one turn doing one thing, and a window with room for
only one of them shows neither. That row does not light under the pointer and
does not open, because nothing behind it has finished yet; it settles into the
transcript, in the past tense and with both, the moment the round trip is
answered.

A call that changed a file is the exception, and says so by offering nothing. It
is shown as the change itself, and a change too long for the block is cut where
the change is built rather than where it is drawn: those lines are gone before
anything is drawn at all, so the count of them is the whole of what is still
true about them.

The answer itself is read as markdown rather than printed with the markers still
in it. A heading loses its hashes and stands out, one `*` or `_` around a phrase
leans on it and two raise its voice, backticks put a run of code (a command, a
path, a name) in the theme's accent so it stands out of the prose, and a fenced
block is toned for its whole length with the fence lines and the language
written on them gone. The tone belongs to the row rather than to the text, so it
costs no column: the answer wraps exactly where the same answer would have
wrapped plain.

A phrase the model leant on or raised may run over more than one line, and it
is read that way: a run opened on one line and closed on the next carries across
the break. It carries no further than the paragraph it was written in (a blank
line, a heading, an item or a quote ends it), so a marker the model opened and
never closed costs that paragraph and nothing after it.

An answer wider than the terminal wraps at the last space before the edge, so a
word arrives whole on one row rather than in halves on two. A word too long for
any row (a path, a hash, a line of code with no spaces in it) is still broken
where the row ends, since there is nowhere else to break it.

A line that wraps and opens with a mark (an item's bullet, a task's box, a
quote's bar, a number and its dot) continues under its own words rather than
back at the edge, so the mark is the only thing in its column and a list still
reads as a list at any width.

An address written on its own is a link without being written as one. `https://`
or `http://` and everything up to the next space is drawn the way a link's words
are, and arrives exactly as it was written: the underscores and stars in a
path are part of it rather than markers. What ends the sentence is not part of
the address, so a full stop after one stays with the prose.

A backslash in front of a marker says the marker was meant as itself: `\*` is a
star and `\|` is a pipe, and the backslash is gone. In front of anything else
it stays exactly where it was, so `C:\Users` and `\d+` arrive whole.

Between two backticks nothing else is a marker. `*ptr`, `_private` and
`**kwargs` arrive on screen as they were written, because the words in a span
are code and code is full of characters that mean something else in prose.

A block is fenced with three backticks or three tildes, and is closed by the
marker that opened it, so a block written with tildes can have backticks inside
it, which is what a model reaches for tildes to do.

A block indented under an item is the item's: the spaces in front of its fence
go with the fence, so the code inside it opens at the item's own column rather
than at one stitched to the front of its first row.

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
brackets have to open the item: `- see [TODO] in the grammar` is a bracket
somebody wrote, and it stays one.

A phrase between two `~~` is one the answer wrote and then took back, and it is
drawn with a line through it and nothing else, struck rather than dimmed,
because a retraction is still being read. Exactly two: `~/Projects` is a path,
`~40` is an approximation, and both are left where they were.

A table is drawn as a table. The bars a model wrote are replaced by one rule
between the columns and one under the header, every column is as wide as the
widest thing drawn in it, and `:--`, `--:` or `:-:` in the row of dashes says
which side a column is drawn against. Where the window cannot hold it, the table
gives up columns from whichever is widest until it fits, and a cell that no
longer fits wraps onto the rows under it, so every row is exactly the width of
the window and the columns stay under each other. A window too narrow for even
one column apiece gets the table as the model wrote it.

A bar is only a table at the start of a line, and only where the line under it
is the row of dashes that makes one. `a | b` in a shell, `Ok(_) | Err(_)` in a
match and a line of bars with nothing under it are all left where they were.

A link is read the same way, and is drawn as its words: underlined in the
accent and carrying the address, so a terminal that opens links opens it from
the words without the address written out after them. A bracket that was not a
link is left exactly as it was written.

A bare `#487`, or `PR #487` and `issue #487` with the word included, is read the
same way, and points at the repository you are in. The
address comes out of the `origin` remote in `.git/config`, so a checkout cloned
from GitHub or GitLab gets a number you can click and every other checkout gets
the four characters it always had; a link to a repository nobody named would be
a link somewhere wrong. `owner/repo#12` is counted against the repository it
names instead. The number goes to the issue page, because prose cannot say
whether a number is an issue or a pull request, and a forge that files both in
one series answers either from there.

Where there is no colour to read it into, the markers are left where the model
put them. Taking a marker out there would drop the emphasis and put nothing in
its place, and `crucible < prompts.txt > answers.md` is a file of markdown worth
keeping.

No flag sets colour. It follows
[`output.color`](../configuration/configuration.md#output) in your configuration:
`auto`, which is what nothing configured gives you, writes colour only when
output is a terminal and `NO_COLOR` is unset or empty, `never` writes none, and
`always` writes it on a terminal even when `NO_COLOR` is set. A terminal that
calls itself `dumb` in `TERM`, or sets no `TERM` at all, is drawn without colour
even under `always`, unless `COLORTERM` says `truecolor` or `24bit`.

A prompt can be more than one line. End a line with a backslash and press
<kbd>Enter</kbd>: the backslash goes, the box grows a row, and you carry on
typing. That is what a backslash has meant at the end of a line for as long as
there have been shells, it is typed rather than signalled, and it works in every
terminal with nothing configured.

<kbd>Shift+Enter</kbd> does the same where your terminal can send it. Some
cannot: the encoding they use has no room for the modifier, so they send the
same bytes for <kbd>Enter</kbd> and <kbd>Shift+Enter</kbd>. crucible asks each
terminal for the newer encoding on the way in. Where it declines,
<kbd>Alt+Enter</kbd> and <kbd>Ctrl+J</kbd> need nothing asked for.

If your terminal keeps all three for itself, swap the two presses over:
`"input": { "send": "altEnter" }` in your
[configuration](../configuration/configuration.md) makes <kbd>Enter</kbd> add a
line and <kbd>Alt+Enter</kbd> send.

Pasting several lines pastes several lines. The breaks arrive as breaks rather
than as a send at the first one, because crucible asks the terminal to mark where
a paste begins and ends. A terminal that cannot do that is one where a pasted
break is indistinguishable from a keypress, and there the paste sends its first
line.

The edits a shell answers to work in the box as well. <kbd>Ctrl+W</kbd> takes the
word behind the cursor, <kbd>Ctrl+U</kbd> the rest of the line behind it and
<kbd>Ctrl+K</kbd> the rest of the line ahead; <kbd>Ctrl+Backspace</kbd> and
<kbd>Alt+Backspace</kbd> take the word too, for fingers that came from an editor
rather than a shell. <kbd>Delete</kbd> takes the character in front of the
cursor, which stays where it is. A word goes on being a word across a break, so
rubbing one out at the start of a line joins it to the line above, the same as
<kbd>Backspace</kbd> does. <kbd>Ctrl+Y</kbd> goes the other way and asks the
terminal to put the whole line on your clipboard. It is asked with the `OSC 52`
sequence, which travels the wire the drawing does, so a copy taken over ssh
lands where you are reading. The row under the box then says `line copied`, and
that means the request went out, not that it was taken: a terminal that does not
implement the sequence drops it without a word, and your next paste is the only
thing that says whether it landed. crucible refuses a line over 64 KiB whole
rather than sending part of it, and says `the line is too long for the terminal
to copy` instead.

<kbd>Ctrl+C</kbd> throws away a line you are part-way through, and does it whether
or not a turn is running. Against an empty box it offers to leave
(`press ctrl+c again to leave`, under the mode), and a second press within two
seconds takes the offer. Any other key first takes it back, so a session is
never ended by one stray keystroke. <kbd>Ctrl+D</kbd> on an empty box leaves at
once.

None of the bindings on this page answers a letter held with <kbd>Shift</kbd> as
well. <kbd>Ctrl+Shift+C</kbd> is a different key from <kbd>Ctrl+C</kbd>, and
where your terminal forwards it rather than taking a copy with it, crucible
leaves it alone.

A run whose input or output is redirected gets no box: `crucible < prompts.txt`
reads whole lines, one prompt each, and the mode is written in front of them
instead.

## Naming a file in the prompt

Write the path to a picture, a PDF or a video in the prompt and it goes with it:

```
what is wrong with this layout? screenshot.png
```

The file is sent as what it is rather than as text. Nothing else changes: the
rest of the line is the prompt, and a path to anything crucible does not send
this way (a source file, a log) is just a word in the sentence, which the `read`
tool opens when the model asks for it. A picture the model asks for that way
comes back as a picture too, rather than as a refusal; see
[`read`](../tools/files.md#a-picture-is-looked-at-rather-than-read).

Pictures go to Anthropic, Google, MoonshotAI and OpenAI. A PDF goes to
Anthropic, OpenAI and Google, whose requests have a shape for a document;
MoonshotAI's have none, and say so rather than sending the file as anything
else. A video, which has to be an `.mp4`, goes to Google and MoonshotAI;
Anthropic's and OpenAI's requests have no shape for one. The seven providers new
in 0.44 are sent text alone in this release, and say so of a file the same
way.

Nothing asks you first. Every other way a file reaches the model goes through a
tool, and a tool is something the agent chose to run, which is the thing a
permission question exists to put in front of you. Here you typed the path
yourself, in the sentence you are sending. There is no second decision to make,
and a question about a file you just named would be asking you to confirm the
prompt you wrote.

The bytes stay on disk. A transcript holds the path, and each request reads the
file again, so what is in front of the model is the file as it is now, and a
session with twenty of them costs twenty paths rather than twenty files. A
request carries at most 4 MB of files; over that, the oldest stop being attached
and the model is told to read them again if it needs them.

When a file cannot go, a line says which half said no:

```
! invoice.pdf is not attached: crucible's moonshot requests have no shape for a pdf. Nothing you type changes that — a later release adds the shape.
```

That one is about the protocol, and nothing you type changes it. The other half
of the same question is the model: where the one you are asking does not read
the kind of file you named, the line names the model instead, and `/model`
picks one that does. A file over 4 MB on its own, which is never read past that,
or a file whose bytes are not what its name claims gets its own line and never
costs a request. A path that leads to something other than a regular file (a
directory, a named pipe) is not refused but left alone: the word stays in the
prompt as a word, nothing is read from it, and nothing is said about it.
[Attachments](../sessions/attachments.md#when-a-file-is-refused) lists every
such line.

## Commands

A line whose first word is one of the names below is a command rather than a
prompt. It is answered here, costs the provider nothing, and is not part of what
the model is told about the session. Where colour is on, a command's name
turns to the accent colour once it is typed in full. A name only part typed
stays plain, and the command list above the box offers the rest.

Any other line is a prompt, even one that opens with a slash: `/etc/hosts is
wrong` and `/tmp is full` are questions about files, and are sent as typed. The
one exception is a word shaped like a command (a slash, a letter from a to z in
either case, then letters and hyphens) typed alone and naming none. That is
taken for a slip: nothing is sent, and the answer names the nearest commands
(`nearest: /model, /mode`) or points at `/help`. The up arrow brings the line
back to correct.

| Command | What it does |
| --- | --- |
| `/help` | Lists these |
| `/release-notes` | Lists the releases to open one, or prints the one you name |
| `/context` | Shows how the model's window is spent by the next request, part by part |
| `/usage` | Shows what the session has used, and how much of each of your plan's limits is gone |
| `/model` | Picks the model to ask from now on and how hard it thinks, or takes the model you name |
| `/effort` | Picks how hard it thinks from now on, or takes the rung you name |
| `/fast` | Asks the model in force for its vendor's [fast form](../providers/fast.md), at its price, or for standard |
| `/login` | Signs in with your provider account |
| `/logout` | Signs out from your provider account |
| `/mode` | The [permission mode](../permissions/modes.md) in force, or the one you name |
| `/sandbox` | Shows the [sandbox](../security/sandboxing.md) in force, and turns it on or off |
| `/theme` | Picks the colours crucible draws with, and the one code is drawn in |
| `/settings` | Changes a [setting](../configuration/configuration.md) without editing JSON, and shows what is in force and what the session has used |
| `/resume` | Stands what was worked on in this directory beside a preview of it, and picks one back up |
| `/cache` | Shows what [prompt caching](../providers/prompt-caching.md) did, or cleans up what it left |
| `/compact` | Replaces what is behind you with the model's own notes on it, making room |
| `/clear` | Starts a new session, leaving this one on `/resume` |
| `/exit` | Ends the session |

`/model` on its own stands a shelf over the whole shell, with the model being
asked now named above it: a search line across the top, the providers this build
serves in one pane beside the models in the other, and the rungs the marked model
takes on a strip underneath. Model rows show exact API IDs, such as `gpt-6-astra`
and `gemini-3.8-flash`; search also accepts display names.
Type to narrow both panes at once: `openai` leaves everything that vendor
serves, `sonnet` leaves the two Sonnet models, and the line does not ask which
kind of name it just got. <kbd>Tab</kbd> crosses between the panes, the up and down
arrows walk whichever one the mark is in, the left and right arrows walk the
rungs, and Enter takes the model and the rung under it together. Taking a row
moves the session to whoever serves that model. Escape leaves it and changes
nothing. `/model <name>` skips the shelf, and is also how to ask for a model the
shelf does not carry: what it holds is a shortcut past the vendor's
documentation, not the limit of what the vendor serves.

Either way the name is written to `~/.crucible/config.json` under the provider
this run is set up for, and the provider beside it, so the next crucible
started anywhere begins with both. See [Providers and
models](../providers/providers.md).

`/release-notes` opens a list of releases, newest first, each with its date and
how many entries it holds; the one you are running is marked `this version`. The
list shows the eight newest and a last row, `all N releases`, that opens the
rest in place. <kbd>Up</kbd> and <kbd>Down</kbd> (or the wheel) move,
<kbd>Enter</kbd> prints that release alone into the transcript, and
<kbd>Esc</kbd> closes the list. `/release-notes 0.41.1`, or `v0.41.1`, prints
that release without the list, and `/release-notes all` prints every release
into the transcript, oldest first: a row each for the older ones, saying how
many entries each group held, then the ten newest in full. Without a keyboard,
or in a window too short to hold the list, `/release-notes` prints that same
output. The notes are the changelog of the build you are running, built into it,
so asking for them needs no network. Where a very narrow window would print more
rows than the transcript keeps, the oldest rows are left out first, and the last
row says how many.

`/context` stands a panel over the prompt box: the model and the size of its
window, one bar across the whole window, and a row for each part of the next
request with its tokens and its share of the window. The parts are the system
prompt, project instructions, the tool schemas advertised, those from MCP
servers, the messages so far (tool results among them), the reserve kept free
for the next answer and the tool results a pass carries back, and what is
free. Project instructions are what
`systemPrompt.append` adds, from whichever configuration file set it. Free is
the figure the line above the box calls `window left`, the share of the room
left before compaction, so the two always agree. A model whose window crucible
does not know shows the tokens alone, with `window not known` in place of the
size and no bar. Escape closes it.

`/usage` stands a panel over the prompt box, headed with the provider and what
pays for it (`API key`, or the sign-in's name, such as `ChatGPT sign-in`), with
what the session has used: its cost, the time its requests were out and the time
since it started, the lines edits added and removed, and its tokens in, out,
read from a cache and written to one. A session with a model crucible has no
price for says `not priced` rather than `$0.00`, as does one where an answer
finished without saying what it used. A cost that reads `at least $0.40`
includes an answer that was stopped before the provider said what it cost: its
tokens are counted as far as it reported them, and the rest of the session is
priced in full, so the session cost that much or more. Under that is the same
context bar `/context` draws, and then the plan limits: one bar for each window
the plan has, with when it starts again in your local time. The windows that
apply to every model come first; a plan that limits a model on its own, as
ChatGPT does for some, has a group of windows under that model's name after
them, and one that limits a family of models, as MiniMax does, has its group
named after the model you are using where the family includes it. A window is
named by its length (`5-hour`, `Daily`, `Weekly`, `Monthly`, `Yearly`, or a
length such as `3-hour`), a limit counted in requests reads `412 of 1,500 used`,
and one the vendor does not limit reads `unlimited`. A reset already behind the
clock reads `since passed`, as the window has started again since it was read.
Where the plan reports more limits than the panel keeps, `more limits not
reported` follows the last one drawn.

The panel opens at once with what crucible already holds, from the last response
or the last answer, and with the ChatGPT sign-in it then asks ChatGPT for every
limit on the account, saying `asking openai…` as the block's last row until the
answer comes and the block is drawn again. A Kimi Code sign-in or key asks Kimi
the same way, saying `asking moonshot…`, for its 5-hour, weekly and monthly
limits, whichever the plan has. A key given on a MiniMax Token Plan row asks
MiniMax the same way, saying `asking minimax…`, for the 5-hour and weekly limits
of each model or family the plan includes; the plan limits are asked for on the
Token Plan sign-in row only. The Usage tab of `/settings` asks the same way when
you turn to it. It asks at most once a minute, never on its own, and not at all
until you have agreed to what is sent to that vendor; a credential the vendor
refuses there is not asked again that session, and any other failure leaves what
was shown. Any other key or sign-in, a key of MoonshotAI's open platform among
them, says `limits not reported`. A window at 100% whose reset is still ahead
ends the next turn before it is sent, with a notice naming the window and its
reset, when the window is one of the plan's own or is in the group of the model
you are using; a model's spent window does not stop another model. The session's
counts are this run's, starting again for a session picked up with `/resume`.
Escape closes it.

### A command typed while a turn runs

Most commands are for the space between turns, but a few can act over a running
one. Typing `/` while a turn runs opens the same command list the prompt opens,
stood above the box, and the arrows walk it as they do there. What happens on
Enter depends on the command:

- **`/theme`, `/help`, `/context` and `/usage`** are screen-only, and run at
  once, panel and all, with the transcript going on behind them. `/context`
  and `/usage` show the figures the running turn last reported, which for
  `/context` include anything it has recorded since its last request; `/usage`
  asks the plan nothing until the turn is over.
- **`/settings`** opens at once too, over the running turn. A value changed
  there is written to your user file as it is between turns, and a row that
  applies at once changes the running session now; the others say `applies at
  next start`.
- **`/model`** cannot reach the runner answering this turn, so it is picked and
  confirmed now but held for the turn that starts after. The rung strip is empty
  there and says so: how hard it thinks is something the running turn has already
  taken, so that half waits for `/effort` between turns.
- **`/fast`** is held the same way: the panel opens now, and the speed taken is
  asked for once the turn ends.
- The rest (`/clear`, `/logout`, `/resume` and the like) move the session
  itself, which a running turn owns, so they are refused and say so on a panel
  rather than act partway through one.
- **`/release-notes`** would stand a list over the answer being written, or
  print every release into it, so it is refused on the same panel; ask for it
  once the turn ends.
- **A word that names no command**, typed alone, stands the same panel with
  the nearest names on it, and is not queued. With words after it the line is
  a prompt, and waits for the turn like any other.

`/theme` stands a list of themes where the prompt box was, with a specimen
beside it drawn in whatever your marks are standing on. Moving a mark redraws
it, so the choice is made by looking rather than by reading a name. Enter takes
it and writes it to `~/.crucible/config.json`; escape puts back what was in
force and changes nothing. `/theme <name>` skips the list.

There are two lists, and the left and right arrows step between them. **interface**
is the one above: borders, marks, the mode in force, the ground a diff takes.
**code** is which theme fenced code is drawn in, and its list holds the names
you already know: Monokai Extended, GitHub, Dracula, Nord, gruvbox and the rest.

The specimen shows both at once, which is why it is a diff. The rows a change
touched carry a ground, and that is the interface theme's; the rows it did not
are free to be read, and that is the code theme's. Move either mark and the half
it decides changes.

The whole transcript changes with it, not only what comes next: crucible holds
what was said rather than what it looked like, so the rows on screen are painted
again from the theme now in force. See
[Configuration](../configuration/configuration.md#output).

`/effort` asks over the rungs the model in force serves (the same ones the shelf
`/model` stands puts on its strip), and is the way to change the rung without
changing the model. The answer is written to the same file beside the model. It
draws a ladder rather than a panel: one track with the rungs under it, `Faster`
at one end and `Smarter` at the other, walked with the left and right arrows.
The mark opens on `high` where nothing has chosen yet, which is a place to start
walking from rather than a rung being asked for: leaving it leaves the session
asking for none, and what applies then is the vendor's own default for that
model. The ladder holds what the model serves rather than all five: the Kimi
and DeepSeek models serve `low`, `high` and `max`, and a model whose vendor serves none is
told so instead of being offered a ladder that cannot be answered. A session
with no model chosen is sent to `/model` first, since a rung is asked of a
model.

`/login <provider>` opens a box for the key, never the command line, which would
put it in your shell's history and in the process listing. The box is labelled
with the provider it is for and takes a paste whole. Escape leaves it without
writing anything and says so: `cancelled, nothing signed in`. A store that
cannot be written is answered with `the key could not be saved` and what to fix
(the permissions, another crucible still writing five seconds later, or a store
that cannot be read and should be moved aside), never with the path or the key.
A window too short for the box says so instead, and asks for a taller one.

`/login` on its own asks how usage is paid for, which is a different question
from which vendor: somebody paying for a ChatGPT plan and somebody holding an
OpenAI console key are two people, and only one of them has a key to type. So
the first panel has two rows, whatever is stored: *Your account with
subscription* and *Provide your own API key*. The accounts connect: ChatGPT
opens a browser authorization, or a device code from a terminal with no browser
to reach, and Kimi Code a device code on the site your account belongs to;
either writes a renewable credential to the same protected store a key goes to.
The key route asks whose key you have before opening the box, and is the route
an Anthropic key takes, Anthropic having no account route.

A provider holds one credential. Choosing a row whose provider holds another
says on the next screen what it replaces, and nothing is replaced until the new
one is stored: a sign-in that fails, is refused or expires says `sign-in did
not complete` and names what is unchanged. A store that is there and cannot be
parsed, is not text, or is past its size limit is said before any row is drawn.

A run with no keyboard to walk that panel (and a window with no room to stand
one in) gets each row as the line to type instead: `/login` and the words that
reach that row alone.

A key that is written lands on the session that took it: the provider is set up
there and then, from the next turn on. Logging in chooses neither a model nor a
rung; `/model` is the explicit next step where nothing has chosen one. A run
started with no key for anything is one command away from a turn, and the row
under the box says which model it will be asking, or sends you to `/model` where
the files name none.

`/logout` is the same panel over what is actually there: the providers a key was
written down for, and nothing else. `/logout <provider>` forgets that one
directly, and a name with no key here says so and lists the ones that have. It
reaches `~/.crucible/auth.json` and only that (a key exported into your shell is
untouched and goes on winning), which is what the line under the answer says.

`/clear` starts a new session with nothing said in it: the next prompt is the
first one the model sees, and the turns before it are neither sent nor paid for
again. The session you were in is finished rather than dropped: its log is
complete and it is on `/resume`'s list, so everything said in it can be picked
up whole. What does not come across is what that session allowed for the rest of
itself, the record of which files it read, or the plan standing over the box,
all three of which belonged to it; a plan that outlived its session would
describe work the agent has no memory of. The panel comes down with it; the mode
does not move, because it is where you are running crucible rather than
something a session decided. The screen empties too, down to the welcome card a
fresh start draws: the one you left is read back with `/resume` rather than by
scrolling into it.

With [`output.screen`](../configuration/configuration.md) set to `native` the
screen is your terminal's, so `/clear` and `/resume` leave the earlier
transcript in its scrollback, above what replaces it.

`/resume` stands this directory's [sessions](../sessions/sessions.md) over the
whole shell: a search line across the top, the sessions in one pane newest
first, and the end of whichever one is marked drawn in the other.

```
╭──────────────────────────────────────────────────────────────────────────────╮
│ Search    a session, or a branch                                             │
╰──────────────────────────────────────────────────────────────────────────────╯
 Resume a session · 3 of 12 · ~/code/my-project

╭──────────────────────────────╮ ╭─────────────────────────────────────────────╮
│ › rename the parser error    │ │ › rename the parser error type              │
│   just now · fix/parser      │ │                                             │
│                              │ │ ● Reading the parser to see what the error  │
│   grep misses hidden files   │ │   type is called now.                       │
│   2 hours ago · main         │ │                                             │
│                              │ │ ● Read(src/parse/error.rs)                  │
│   add --json to the report   │ │   └ 84 lines                                │
│   yesterday · feature/json   │ │                                             │
│                              │ │ ● Renamed ParseFailure to ParseError, and   │
│                              │ │   its three call sites. cargo test passes.  │
│                              │ │ ────────────────────────────────────────────│
│                              │ │ just now · 7 messages · fix/parser          │
│                              │ │ Enter to resume · Esc to cancel             │
╰──────────────────────────────╯ ╰─────────────────────────────────────────────╯

 ctrl+a all projects · ctrl+b this branch · ctrl+w worktrees · esc
```

The keys row names what each key does in full where the window is wide enough;
at eighty columns it names each of the three toggles by what it does next, and
a narrower window gets only the keys. <kbd>Ctrl+A</kbd> shows every project's
sessions, <kbd>Ctrl+B</kbd> keeps this branch's and <kbd>Ctrl+W</kbd> adds this
repository's other worktrees; [Switching without
restarting](../sessions/sessions.md#switching-without-restarting) says what
each shows and what Enter does on a session from another directory.

Type to narrow the list. The line is matched against a session's title, the
branch it was recorded on and the directory its row shows, if any, and does not
ask which it was just given: `parser` and `fix/` both leave the first row above. The up and down arrows walk
what is left, and the preview follows the mark: it is drawn by the code that
draws the live transcript, so the prompts, the calls, the rows results came back
on and the model's prose are what picking that session up would put back on
screen. Under it are the session's age, how many messages are in it, its branch,
and a note where another crucible still has it open. The message count shows
only where the session has ended at least once since counts were kept. The
wheel scrolls whichever pane the pointer is over, so a preview can be read back
past its last rows, and a window too narrow to split folds the preview away and
gives the list every column.

<kbd>Enter</kbd> picks up the marked session. The one you were in is closed (its
log is finished and stays readable), and the one you took becomes the session
this crucible is recording to, with everything already in it back in the
transcript. The plan comes back with it, standing over the box where it stood,
because the call that wrote it is in the transcript being replayed.

<kbd>Ctrl+R</kbd> renames the marked session where its title stands, and Enter
saves it: that is the title the list shows from then on, here and in every later
run. The row becomes a field: it takes the accent the search line takes, the
keys row under the panes changes to `enter to save · esc to cancel`, and a title
longer than the pane scrolls under the cursor as you type rather than stopping
at the edge. A title with nothing in it is refused where it was typed, because a
session without one falls back to its first prompt. <kbd>Escape</kbd> steps back
one layer at a time: out of a rename, then out of a query, then off the screen
having picked up nothing.

`/resume <id>` skips the picker and takes that session directly. The id is the
one the parting message prints and the one `crucible --resume` takes, so a
session named there can be picked up here without looking for it. Anything else
after `/resume` is not read as a search: it says no session here is called that,
and stands the picker so you can go and find it.

A run with no keyboard (input redirected from a file) has nothing to walk, so it
gets a numbered list of the last nine sessions instead, each row carrying the
whole id `/resume` and `--resume` take.

Two things are worth knowing before you switch. The [permission
mode](../permissions/modes.md) comes with you, but what you allowed *for the
rest of that session* does not: the new session is asked about those calls
again, and rules you wrote to a file apply as they always did. And a session
another crucible still has open cannot be picked up: it says so rather than
letting two of them write to one log.

Typing `/` opens the list above the box, filtered to what has been typed so far,
so the box and the mode under it stay where they are. The list closes as soon as
the line becomes something else: a path, a sentence, a command with a word after
it. That is also what keeps `/etc/hosts is wrong` a prompt: a line is only taken
for a command where it could not be anything else.

One row of the open list is marked, and that row is what <kbd>Enter</kbd> runs,
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

Twelve tools, advertised in the order a model tends to reach for them. Nine are
in the list from the start, and eight when nobody is at a keyboard for
`ask_user` to ask. The rest are **held back**: they exist and they work, and the
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
| `bash_output` | Says what a command left running has printed | no |
| `todo_write` | Writes down the plan | no |
| `ask_user` | Puts a question to you | no |
| `web_search` | Searches the web | yes |
| `web_fetch` | Reads one web page | yes |
| `tool_search` | Finds a tool that is not in the list | no |

`web_search` and `web_fetch` exist only where the provider serves them.
Anthropic, Google, MoonshotAI and OpenAI serve both; Meta and xAI serve
`web_search` alone; DeepSeek, MiMo, MiniMax, Qwen and Z.ai serve neither, and a
session there has ten tools or fewer.

Reads inside the workspace never ask; one that leads outside it puts the path
to you first. Anything that changes a file or starts a process asks, until
you configure rules or a mode that answer for you; see
[Permissions](../permissions/index.md). [Tools](../tools/index.md) is what each
one takes, what bounds its answer, and what it says when it hits that bound.

`write` puts down a whole file, so it refuses to overwrite one this session has
not read or written itself, and says so rather than ending the turn. The model
reads the file and writes it again. That covers the case a permission prompt
cannot: a `write` you approve is one you agreed to, and neither of you can see
that the file holds work nobody looked at.

`bash` runs its command through a POSIX shell in the workspace root, and starts
it with a short list of variables (`PATH`, `HOME`, the locale) rather than the
environment crucible is running in. Your provider key is not on that list, so a
command that prints the environment prints no key. Anything else a command needs
is named in [`env`](../configuration/configuration.md#env).

The shell and its descendants are one command scope. When the command exits,
times out, is cancelled, or cannot be collected, crucible stops that scope and
waits only for a bounded interval: a background process does not keep an output
reader or a turn alive. OS confinement is off by default; enable it in your
configuration with `{"sandbox":{"enabled":true}}`. On Linux this requires a
verified Bubblewrap boundary with closed networking and only the declared
filesystem view. An unavailable enforcing backend refuses execution. See
[Operating-system confinement](../security/sandboxing.md) for the exact
capability matrix and behavior with confinement disabled.

`todo_write` reaches nothing outside crucible. It puts down the plan the agent
is working to: a list of at most 64 tasks, each of them a line, each one of
`open`, `doing` and `done`. You read it as a panel above the box. Every call
replaces the whole plan, so what the model thinks the plan is and what you are
looking at are one thing rather than two. [Writing down the
plan](../tools/planning.md) is the rest of it.
