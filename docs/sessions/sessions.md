# Sessions

A session is a conversation bound to a working directory. Every session is
written to a file as it happens, so it survives the terminal it was held in.

## What was worked on here

The screen crucible opens with lists the last few sessions started in the
current directory, newest first, each one showing what it was first asked and
how long ago it began. A directory nobody has worked in says so instead.

The list is short on purpose, and so is the work behind it: the names come from
a small index kept beside the logs, already in the order the sessions were
started, and only the newest handful of the files it names are opened. A machine
that has held crucible for a year opens the same number as one that installed it
this morning. `/resume` looks further, once you ask it to; see
[Switching without restarting](#switching-without-restarting).

Only sessions that were asked something appear. Starting crucible and leaving
without typing records a file with no turns in it, and there is no row to draw
for one.

## What is left behind when crucible ends

crucible draws on a screen it borrows from the terminal and hands back when it
ends, so what you scroll up to afterwards is the shell you started from. The
transcript is not in the terminal's scrollback and never was.

What crucible writes on its way out is two lines: how to come back, and the
`crucible --resume` command naming this exact session. That command still works
after another session has been started in the same directory, and the id in it
is the one thing about the session the screen never said. Everything else it
drew is in the scrollback the shell has just been handed back. Where the log
stopped being written part-way through, a line above them says so and names the
file, because that was reported on a screen which no longer exists and a
transcript missing its tail should not be opened later as a whole one.

With [`output.screen`](../configuration/configuration.md) set to `native`,
crucible borrows no screen: each finished line is written once into your
terminal's scrollback as the session goes, and only the part still changing at
the foot is drawn again. When the window changes size, crucible waits for it to
stop changing and then writes the whole conversation again at the new width,
clearing the terminal's scrollback first. The transcript is still above your
shell prompt when crucible ends, so the same two lines are all it writes,
whether you leave with `/exit` or with <kbd>Ctrl+C</kbd> pressed twice; a log
that stopped part-way through was reported at the time, and that report is in
the scrollback too. `/clear` and `/resume`, which on the full screen empty it
and draw the opening card again, cannot take back what the terminal already
holds here, so the earlier transcript stays in its scrollback and the new one
starts under a single divider row, `new session` or `session resumed`, with no
second card. The earlier transcript stays there until the window is resized,
when only the new one is written again. A `/clear` before anything has been said
writes no divider, only `nothing had been said`, since no session ended there.

Nothing is written in a run whose input or output is not a terminal. Nothing
was hidden from you in one, and nobody was at the keys to be told.

## Moving through the transcript

The wheel moves the transcript a few rows at a time. For a long jump, use the
scroll rail: the transcript's last column, from its top row to its foot. The
thumb, drawn heavier in the theme's accent, is the part of the whole transcript
on screen, as long as that share of it and never shorter than a row; it sits at
the bottom while you are at the newest line. A mark on the quiet track is a
prompt you sent, at the place it falls in the whole transcript, and prompts too
close to tell apart share one mark. The prompt you are reading under has its
mark drawn larger, `●`, quiet on the track and in the accent on the thumb: the
one you last landed on by clicking its mark, while it starts on screen and
until you send another, and otherwise the latest prompt that starts at or above
the screen's last row. With the pointer on the rail, the track and marks take
the accent too, and the mark under the pointer is drawn larger, so you can see
which prompt a click there lands on; moving off the rail puts it back.

Click the rail off the thumb and the transcript moves so the thumb is centred
there, or as near as the rail's ends allow; click a mark and you land on that
prompt. A click on the thumb takes hold of it without moving it, and a mark the
thumb covers is drawn as thumb, unless it is the prompt you are reading under.
Keep the button down and drag to scroll with the pointer, holding the thumb
where you took it, and drag it to the foot to follow the newest line again. The
prompt box and everything standing over it stay where they are. The rail takes
no keyboard binding, and a drag that selects text never takes it.

While the whole transcript fits on screen there is nowhere else to go, so the
column stands blank, and a click there opens nothing beside it. The
transcript's text wraps one column narrower to leave it room, and a window
narrower than 24 columns, where the prompt box also drops its frame, does not
draw it. Set [`output.scrollRail`](../configuration/configuration.md) to
`false` to give the column back to the text. With colour off, the thumb, track
and marks still differ by shape, and with `output.glyphs` set to `ascii` the
larger mark is `*`.

With `output.screen` set to `native` there is no rail and the wheel is your
terminal's: the transcript is in its scrollback, and you move through it there.

## Continuing

```bash
crucible --continue
```

This picks up the most recent session **started in the current directory**. Run
it somewhere else and you get that directory's most recent session instead,
which is the point. Two projects open in two terminals are two sessions.

`--continue` replays the transcript so the model has the earlier turns, and
appends to the same file. It does not restore permissions: a session-long allow
lives as long as the process that made it, and the mode is read fresh from
configuration at every start. A durable `allow` rule you deliberately wrote in
`~/.crucible/config.json` is read again, but crucible does not turn an answer at
a permission question into one. See
[Permissions](../permissions/permissions.md).

If nothing was ever recorded for this directory, crucible says so and stops
rather than silently starting a new session.

A log name in the sessions directory that is a symbolic link or a pipe rather
than a file is refused as a log that cannot be read is: `--continue` stops with
`could not read the session log …` and the reason, having read nothing through
the link and waited on no pipe, rather than continuing an older session in its
place. `--resume` and the `/resume` preview refuse a link the same way, and
answer a pipe as an id nothing was recorded under. A log with a second hard
name, as a backup made with hard links leaves, is read as any other.

A running session reads its own log back the same way, when it draws the
conversation again or opens a result too old to still be held. If the log's
name is replaced by a link or a pipe while the session runs, that read is
refused as a log that cannot be read, rather than showing what the link leads
to or waiting on the pipe.

Closing the terminal window, or sending crucible a `kill`, while an answer is
arriving does not lose it. On Linux, macOS and FreeBSD the hang-up or
termination stops the turn first, the way Escape would: what the model had said
so far is written to the log, the terminal is handed back, and then the process
ends by that signal. `--continue` picks the session up with that much of the
answer in it. Between turns there is nothing in flight. While crucible waits
for a key there, at the prompt or in a panel such as the `/login` key box, the
signal is noticed within a quarter of a second, the terminal is handed back
with what is typed showing again, and then the process ends by that signal.
While a command is being carried out, or an account sign-in waits on the
browser, the signal ends crucible at once, as it does while a permission
question is waiting for a key.
A `kill -9` cannot be caught by anything, and on Windows a closing console
window is not caught either; both end the process where it stands, which is the
case the next paragraph is about.

A log the process was killed part-way through writing costs the line it was on
and nothing more: the turns before it are still a transcript, and `--continue`
hands them back. The half-written line is dropped from the file as the session
is continued, before anything new is appended: the next turn would otherwise be
written onto the end of it, which turns a lost line into a log that cannot be
read at all. Nothing that was handed back is touched.

What comes back is always the start of a transcript, never one with a hole in
it. What a line this build cannot read as a message costs is decided by where it
sits. At the *end* of a log it is where the log stops, and it costs that line
alone: the turns before it are handed back, and the file is cut there before
anything new is appended, exactly as a torn line is.

With more of the log after it, the same line stops the run instead. A transcript
missing its middle would be replayed with nothing to say so, and the cut
that follows a replay would take every turn recorded after the damage off the
disk as well. crucible says which file it is and continues nothing, so the file
is still there, whole, to look at.

A log that stops between a tool call and its result is answered from what the
calls recorded. A call that recorded how it ended, whether it succeeded, failed
or was stopped, is answered with that result, so the model is told what it did
instead of being free to run it again. A call that was started and recorded no
result may have done what it was asked before the process ended, so it is
answered as a failure that says so: `interrupted: this call started and no
result was recorded; it may have taken effect. Check before running it again.`
Any other call in the same pass recorded no start, and is answered as
interrupted. These answers are written to the log before anything new is, so a
later `--continue` reads them back as it reads any other result. Where no call
in the pass recorded a start, the last recorded turn does not come back: an
unanswered question is not something to send a provider, so the replay ends
before it and the file is cut to match.

## Conversation, journal and checkpoints

New logs use format 13 and remain readable alongside formats 3–12. Format 12
adds bounded provider continuation beside completed agent messages: signed
thinking, encrypted reasoning and native ordering needed for subsequent tool
passes. This private state is not rendered or given to the recap model, and a
failed, incomplete or cancelled stream cannot commit it. Format 13 adds a line
saying that tool results were cleared because the vendor that produced them
restricts where they may be sent, and the sentence left in their place; a
session that never left such a vendor never carries one. Older Crucible builds
cannot resume format 12 or 13 logs. A search result may also record which vendor's
search answered it and, where that vendor restricts where it may be sent, the sentence
to leave in its place. The field is optional within format 13, so builds that read
format 13 without knowing it, such as 0.41.0, ignore it and still resume these logs.

Continuation is bound to its producing protocol, model compatibility, credential
and recipient. A changed key, endpoint or incompatible provider receives visible
history and descriptive settled tool results, not another scope's native state.
Switches among the four bundled Gemini models preserve their native steps;
other Gemini names use visible history without those private steps. Fable drops older
thinking when compaction or pruning rewrites its bound prefix. `/clear` starts a
new log and carries no old continuation.

The conversation a provider receives is deliberately a closed sequence of
context, user, agent and tool-result messages. Framework facts live beside that
conversation rather than becoming new provider messages. A format 11 session
therefore follows a conversation line with versioned `run_item` metadata where
there is ancestry or lifecycle state to retain. The current journal envelope is
version 2: sandbox plans record boolean `enabled` and `disabled_reason` replaces
the obsolete degradation field. Message metadata does not copy
the prompt text. Prompt-cache records keep normalized per-attempt decisions,
usage and cost; invocation records keep stable prepared, started and finished
states. Conversation replay skips these records, except where a log stops
before a tool pass's result line: a finished invocation record then answers its
call, and a started one with nothing finished after it answers that the call
may have taken effect, as described under [Continuing](#continuing).

Namespaced custom entries use the same framework journal and carry their own
schema version, source and entry identity. They are not sent to a model unless
code explicitly projects one into an ordinary closed message. This keeps an
extension's private state from quietly becoming future model context.

An execution checkpoint is different again. It is a bounded, owner-only,
versioned document replaced atomically under a stable checkpoint identity, not
a conversation message or a session log. It may hold the exact pending call or
human question needed to continue unfinished work, but ordinary diagnostics
redact that content. Resume checks expiry and the current endpoint, model,
credential and authority fingerprints, along with prompt-cache policy,
capability and pricing versions and a freshly derived cache scope and prefix. A
saved prefix fingerprint is never treated as proof of a cache hit.

Pending approvals, external tool work and human input each have a stable action
identity and execution ancestry. Repeating the same resolution is idempotent;
once resumed, a different resolution cannot replace it. Approved and external
tool actions also retain a stable invocation identity. A crash before execution
may retry, a started read-only or keyed-idempotent invocation follows its
declared retry policy, an ambiguous non-idempotent effect requires
reconciliation, and a durably finished invocation reuses its recorded result.
Only settled calls are projected back into a provider transcript.

## Picking one by name

```bash
crucible --resume 019854c2-9a1e-73f1-b0d6-2f1c4e7a58d1
```

Every session has an id, and that id is the whole of how a session is named: the
parting message prints it, `/resume` takes it, and this flag takes it. Where
`--continue` asks for whichever session is newest here, this asks for one exact
session, which is what makes it survive somebody starting another one in the
same directory in the meantime.

The id carries the millisecond the session began, so a directory of logs is
already in the order it was worked in. Sessions recorded before this shape of id
was minted are named differently and are still listed, previewed and picked up
exactly as these are: a listing orders them by the time inside the name rather
than by the name, so a directory holding both kinds is still one timeline, and
nothing has to be renamed to stay reachable.

An id nothing here was recorded under is refused by name, rather than being
taken for the nearest thing to it. Everything `--continue` does with the log it
reads, this does with the log it names.

## Switching without restarting

`/resume` stands this directory's sessions over the shell (a search line, the
sessions beside a preview of the marked one), and taking one changes which
session the crucible you are in is recording to. `/resume <id>` takes that
session without standing anything. Either way it reads a log the way
`--continue` reads one, and everything above applies to it: the transcript comes
back, the file is cut to what was replayed before anything new is appended, a
log this build cannot read is refused rather than half-understood, and a session
another crucible has open is not available. The picker says so on the marked
session's own line rather than waiting for Enter to find out.

The picker looks through every session the index names, not the handful the
opening screen reads, and lists up to 100 of them. It opens on this
directory's, and three keys change what it shows, each pressed again to undo
it. It opens even where this directory has none of its own, saying
`no earlier session for this workspace` on its empty list, as long as another
directory has one for these keys to reach; where none has, `/resume` says that
line and stands nothing.

- <kbd>Ctrl+A</kbd> shows every project's sessions.
- <kbd>Ctrl+B</kbd> keeps only those recorded on the branch checked out here.
  With no branch checked out it does nothing, and the keys row leaves it out.
- <kbd>Ctrl+W</kbd> adds the sessions of this repository's other worktrees,
  found by reading its `.git` directory; git itself is not run.

The heading says what is shown, as `Resume a session · 3 of 12 · ~/code/app`,
`all projects` or `this repository's worktrees`, with the branch ahead of it
while <kbd>Ctrl+B</kbd> holds: `Resume a session · 3 of 12 · main · ~/code/app`.
At eighty columns the keys row names each of the three by what it does next,
as `ctrl+a all projects · ctrl+b this branch · ctrl+w worktrees · esc`; a
narrower window gets the keys alone. A session from another directory shows
that directory after its branch, and a search matches it too. Where the row is
too narrow for the whole directory its front gives way, marked `…`, so the end
that names the project is what you see. The sessions were read once when the
picker opened, and again after a rename, so a key only filters them again.

A session recorded in another directory cannot be resumed from this one, since
it belongs to that directory's files, and the foot of its preview says
`Enter to see how to resume`. Enter on one leaves the picker open on it and
says, under the list, how to pick it up there:

```text
cd ~/code/website && crucible --resume 019854c2-9a1e-73f1-b0d6-2f1c4e7a58d1
```

Where the window is too narrow for that on one row, it is broken after the `&&`,
where a shell reads on to the next line, so pasting both rows runs it. In any
window at least 56 columns wide the id is whole on its row; a directory too long
for its row loses its front, marked `…`.

On Windows no one line runs the same in cmd and in PowerShell: Windows
PowerShell 5.1 refuses `&&`, cmd's `cd` does not change drive, and PowerShell's
`cd` reads `[` and `]` in a name as a wildcard. So each shell gets its own
command, and the resume is a command of its own that you run next in either.
Each label is a row above its command, so a command row copies whole with
nothing else on it:

```text
cmd
pushd D:\code\website
PowerShell
Set-Location -LiteralPath D:\code\website
then
crucible --resume 019854c2-9a1e-73f1-b0d6-2f1c4e7a58d1
```

The directory is written whole rather than under `~`, and quoted where it needs
it: in double quotes for cmd and single quotes for PowerShell, so nothing in its
name is expanded. cmd expands `%NAME%` even inside quotes, so a directory whose
name holds `%` gets only the PowerShell rows.

The preview reads a bounded message tail and uses the live transcript's message
renderer. It omits supplemental diff bodies and compaction notices; selecting
the session restores those details from the full history. A session too long
for the preview opens on a mark saying so, so its first words are not mistaken
for the first words of the session.

What is different is the session being left. It is finished here rather than
when the process ends, so its log is complete and can be continued from
somewhere else immediately. Its session-long permission answers end with it:
"for the rest of this session" was answered about that session, and the new one
is asked again. The mode carries over, and so do the rules in your configuration
files, which were never held in a session to begin with.

The screen is different too. The session picked up replaces what was on it
rather than following it: the transcript is emptied first, so what you scroll
back through is one conversation, with the welcome card at the top of it,
exactly the screen starting crucible on that session would have drawn. What was
on screen before is not recoverable from inside crucible. The session it
belonged to is still on disk, and picking it back up is how you read it again.

The one session `/resume` will not pick up is the one you are in. It says so,
rather than reporting the claim it would find on that file as another crucible's.

Renaming a session with <kbd>Ctrl+R</kbd> saves the title beside the logs rather
than in one, so it survives the picker being closed and this crucible ending. It
changes what the session is called and nothing about what is in it: the
transcript, the id and the log file are untouched. An empty title is refused
with `a title cannot be empty` under the row, since a session with no title is
already called by its first prompt.

## What comes back looks like what you left

A session put back on the screen is drawn by the code that drew it live, so it
is the same session rather than a rendering of one. A result too long for its
row still says how much it left over, still stands out from the rows with
nothing behind them, and still opens on
[<kbd>Ctrl+O</kbd> or a click](../getting-started/first-session.md). The
lines are read back out of the log rather than out of the run that produced
them, and a result too old to still be held is read back from the log when the
view reaches it.

How much of the window is left comes back with it. A log records what each
request carried, so a session picked up says so straight away rather than
waiting for its next answer to measure it, unless it is picked up under
different instructions or a different set of tools, or picking it up took out
results the provider now in use may not be sent, where the reading is about a
request this run would not send and the row waits, as it always did.

The visible conversation comes from the original log, independently of the
compacted context sent to the model. Earlier prompts, answers and tool results
remain in scrollback, with a marker where each completed compaction happened.
New records preserve the live marker’s reason and measurements; older records
show the compaction facts they contain.

Newly recorded file changes restore their bounded diff previews: up to 64 lines,
with up to 1024 characters per line, plus the original added, removed and omitted
counts. These previews are private display data and are never sent to the model.
Older logs without preview bodies still show their recorded change counts;
resume does not reread files or rerun tools to invent missing history.

## One at a time

A session is claimed for as long as it is open, so the one crucible is writing
now is not one another crucible can continue:

```
crucible: /home/you/.crucible/sessions/01890a5d-ac96-774b-bcce-b302099a8057.jsonl is open in another crucible
```

`--continue` says that and stops, having read nothing and changed nothing.
Continuing a session cuts its log back to what was replayed, so without this the
second crucible would delete the turns the first had already written and still
believes are there, and both would append to one file from then on.

Starting a session is refused only if it cannot find itself a free name: eight
tries, each a millisecond and seventy-four bits of randomness. Two crucibles in
one directory are two sessions, each recording a log of its own. It is only
continuing that has to pick one, and only continuing that can be told no.

The claim is the operating system's, taken on a `.lock` file beside the log and
released however the process ends, so a crucible that crashed leaves no session
stuck as busy. The file it was taken on stays where it is and is never mistaken
for a session. Some network filesystems have no locks to take at all; there,
`--continue` goes ahead without the check rather than refusing everything.

A `.lock` file that cannot be made at all (something already in its place, a
directory gone read-only) is a third answer and not that one. Nothing was asked
about the log, so nothing is assumed about it: `--continue` stops with
`could not claim the session log …` and the reason the operating system gave,
having read nothing and changed nothing.

## When the window fills

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/compaction-dark.svg">
  <img alt="Before compaction, a long history (older turns, recent turns and the newest turn) is shown filling the model's window up to the room reserved for the answer and its tool results. When the window fills mid-turn, when you run /compact between turns, or when you choose Carry on from summary while picking up a large session, old tool output is cleared and the same model writes notes to stand in for the older turns, so the next request carries those notes, the recent turns word for word and the newest turn whole, with room to spare, and the session log keeps every message." src="../assets/compaction-light.svg" width="720">
</picture>

Every model accepts only so much at once, and a long session eventually reaches
it. crucible shows how much is left against the end of the row a turn runs on:

```text
✳ writing (1m 12s · ↓ 4.1k · esc to stop)                     10% window left
```

The percentage measures the part of the window the transcript may still use.
Room reserved for the answer and its tool results is outside it, so `0%` is the
safe compaction boundary, not the model's literal last token. The fixed cost
every request carries (the system instructions and the tool schemas) is
outside it too, so a session that has said nothing begins at `100%`, and
automatic compaction begins at `0%` while that reserve remains available.

Where crucible does not know how much the model accepts, the prompt shows no
reading rather than inventing a percentage. While room is being made, the last
reading remains until the compacted transcript replaces it.

When there is no longer room for another exchange, crucible **makes room in the
middle of the turn and the turn carries on**. It asks the model to write down
what is worth keeping under fixed headings: Goal, Constraints & Preferences,
Progress (as Done, In Progress and Blocked), Decisions, Next Steps and Critical
Context. crucible itself adds the files the replaced messages read or modified,
so that list survives a second compaction. That recap stands where the messages
it replaced were. The most recent turns stay word for word, bounded in tokens
rather than counted in turns, so a turn that is mostly tool output cannot carry
the tail past the window on its own. Old bulky tool output is pruned first; if a
completed active tool pass still cannot fit, automatic recovery may recap that
complete pass at the safe boundary before the next request.

The result is reported in place:

```text
────────────────────────────────────────────────────────────────────────────────
 compacted · the window was full
 41 messages became a recap · 156k → 18k carried · 4 turns kept whole
────────────────────────────────────────────────────────────────────────────────
```

Where there was no middle to recap and clearing old tool output freed some
room, the second line says `old tool output was cleared` instead of counting
messages.

Nothing is deleted. What is replaced is what the **model** is sent; the session
log keeps every message of it, which is what `--continue` reads and what you
can go back and look at.

`/compact` does the same thing between turns, when you would rather choose the
moment. A session with nothing behind it says so instead of spending a request.

Nothing is frozen while the notes are being written. The box takes what you type
throughout, and a line finished there is sent as the next turn once there is
room. Escape stops the notes and nothing is replaced:

```text
! stopped
```

Old tool output cleared to make room before the notes began stays cleared from
what the model is sent; the session log still has it.

Half a recap is not a session's memory, and standing it in place of the messages
it was meant to replace would lose the rest of them for good. So a cancelled,
malformed, filtered, silently ended, or token-truncated recap replaces nothing,
and a turn that was making room for itself ends there rather than asking for the
notes again.

If a provider refuses a request for want of room, because crucible had the
window wrong or was never told it, the same thing happens and the question goes
back once the session is smaller. A compaction that frees nothing gets one more
go; when that frees nothing either, the turn stops with `there is no room left
in the model's window, and compacting it freed none`. Where the provider
refused the request outright rather than cutting its answer short, it stops
with the provider's own `<provider>: the request did not fit the model's
window` instead. `/clear` or a model with a larger window is what gets past it.

[Configuration](../configuration/configuration.md#compaction) has the keys.

## Picking up a large one

A session that ran for hours is worth what it cost to build, and carrying all of
it back is what that costs again, on the next request and on every request
after. The panel appears only in an interactive session whose carried context
reaches `compaction.askOnResume` (60000 tokens by default), with that setting
enabled. Smaller sessions resume directly. A large one asks:

```text
This session is large
340k carried, from a session started 3 hours ago. Carrying it whole spends that
again on every turn.

  1  Carry on from summary
     one request now, and every request after it is smaller
  2  Carry all of it
     all of it goes back to the model, on every turn from here
  3  Stop asking
     written down; sessions are carried whole from now on

enter to choose · esc to carry it whole
```

These choices control model context, including any compaction already applied.
They do not remove the original conversation or compaction markers from visible
history. “Carry all of it” keeps the current context whole; it does not undo an
earlier compaction.

Nothing is decided for you. The one case where carrying it whole is right (you
are about to ask about something said two hours ago) is the case crucible
cannot see from here.

Escape carries it whole, which is the answer that changes nothing. Pressed once
the notes have started, it stops them and nothing is replaced, though old tool
output cleared before the notes began stays cleared. *Stop asking* writes
`compaction.askOnResume` down as `0`; set it to a number of tokens instead to
move the point where the question appears.

## When recording stops

A write to the log can fail (a full disk, most often), and the turn does not
stop with it. One line says so, after the turn it happened in:

```
! this session has stopped being recorded: No space left on device (os error 28)
```

It is said once rather than under every turn from then on, which would bury the
turns it is warning you about. What reached the disk before it is still there,
and recording is not abandoned: a later write that succeeds still lands, on a
line of its own so that a write which stopped part-way cannot have the next one
welded onto it.

What that leaves is read back under the rules above. An attempt that got nothing
down costs nothing at all: the empty line it leaves is not a message and is not
damage, and the replay reads past it. One that stopped in the middle of a line
is damage where it sits.

## Where they are kept

One file per session, in crucible's home directory:

```
~/.crucible/sessions/
```

That directory holds your configuration file too, so everything crucible keeps
for you is in one place you can back up, inspect or delete as a unit.

`CRUCIBLE_CODE_HOME` moves the whole directory. It is taken as the home itself,
not as somewhere to put a `.crucible` inside, and only when it is an absolute
path. A relative one is ignored rather than resolved against wherever you
happened to start crucible, which would scatter a home directory across every
repository you work in. `HOME` is read the same way, and if neither is absolute
crucible says so and stops instead of guessing.

When you set it, everything is under it and nowhere else is consulted, which
is what makes it usable for a container or a throwaway run that must not write
into your real home directory.

Because it is read to find the configuration file, `CRUCIBLE_CODE_HOME` is the
one setting of crucible's own that a configuration file cannot set. Writing it
in an `env` block is refused rather than ignored, so it cannot look applied and
do nothing.

Each file is named for its session and ends in `.jsonl`. They are yours: reading
one with `cat`, `jq` or a text editor is a supported thing to do, and deleting
one is how you forget a session. A `.jsonl.lock` beside one is where the claim
above is taken; it holds nothing, and an empty one left by a crash is only a
file.

`recent.sessions` sits beside them too: a small index of the newest sessions,
which is what the welcome reads and where a title from <kbd>Ctrl+R</kbd> is
kept. Its `recent.sessions.lock` is the claim taken while it is rewritten, and
like a log's holds nothing.

One more file sits beside them: `prompt.history`, holding the lines the arrow
keys walk back through. It is one file for every directory rather than one per
directory, so it cannot grow with the number of checkouts you work in, and each
line records which directory it was typed in so only that directory's prompts
are offered back. Each directory keeps its
newest hundred prompts and no more, and the file itself is bounded again across
all of them, so it cannot grow either with how long you work or with how many
checkouts you work in. Deleting it is how you forget what you have typed.

Like a log, none of these is read through a symbolic link or waited on as a
pipe. A link or a pipe named `recent.sessions` is an index that cannot be read:
starting a session, `--continue` and `crucible sessions list` stop with
`could not use the session index …` and the reason, and the welcome lists no
sessions, until you remove it. One named `prompt.history` offers nothing back
and is left as it is, so nothing you type is added to it. One among a session's
saved call results makes that session's log refused as a log that cannot be
read is.

Nor is anything written, locked or narrowed to your account through one, so
nothing outside the sessions directory is changed by a link planted in it. A
link or a pipe named for a session's `.jsonl.lock` is a claim that cannot be
made: `--continue` stops with `could not claim the session log …`, as above.
One named `recent.sessions.lock` stops starting a session and `--continue` with
`could not use the session index …` until you remove it. The index also keeps a
small mark beside it saying this build put it in order; under a link or a pipe
that mark is simply not left, and each start scans the directory again.

### If you used crucible 0.0.2 or earlier

Sessions used to live under `$XDG_DATA_HOME/crucible/sessions`, falling back to
`$HOME/.local/share/crucible/sessions`. If a directory is already there and
`~/.crucible/sessions` is not, crucible keeps using the one you have. Nothing is
copied, moved or deleted, and `--continue` goes on finding the session you were
in the middle of. Setting `CRUCIBLE_CODE_HOME` turns this off: an explicit home
is used as given.

To move to the new place, move the directory yourself:

```sh
mkdir -p ~/.crucible
mv ~/.local/share/crucible/sessions ~/.crucible/sessions
```

The new location wins as soon as it exists, so that is the whole migration.

## Who can read them

Yours alone. A transcript holds what you typed, what the model said, the
retained file and command output, and bounded previews of edited file contents, so on a
shared machine the usual default would hand all of it to anyone with an account.
The directory is closed for the matching reason from the other side: somewhere
another account can write is somewhere a log can be *planted* for `--continue` to
replay back to the model as though you had typed it.

On Unix that is a mode: `0600` on a log and `0700` on the directory. On Windows
it is an access control list naming the account crucible is running as and
nothing else, with the inheritance from your user profile switched off, so what
that profile hands Administrators and SYSTEM does not reach a transcript merely
by sitting above it. An administrator can still take ownership and read it, which
is the same escape by a longer route and the honest limit of what a file
permission promises on either system.

Both are set on every start and every `--continue`, not only when they are
created, because a directory made by an earlier build or by hand keeps whatever
it was made with, and `--continue` is the run that goes looking in it. A path
that cannot be set stops the run and says so, rather than carrying on and writing
a transcript somewhere the whole machine can read.

## What is in a file

One JSON object per line, in the order things happened. The first line says what
the file is and where it belongs:

```json
{"branch":"main","format":13,"session":"…","workspace":"/home/you/code/my-project"}
```

Then one line per message: what you typed, what the model said and asked to
run, and what the tools returned. Supplemental journal records retain bounded
private diff previews and completed compaction notices for display replay.
Compaction changes model context without deleting earlier conversation records.

`/clear` writes nothing here. It closes this log and opens another, so the
session it left is complete and replays whole, the same as any other on
`/resume`'s list. A log written by an earlier crucible can hold a line saying
the session forgot what it had said, with everything above that line still in
the file; `--continue` replays such a log from that line rather than from the
top.

The `workspace` in that header is what `--continue` matches against, and
`format` is what makes a file from a build that spelled things differently a
refusal rather than a half-understood replay. Crucible does not serialize its
credential store or process environment into these files. Conversation content,
tool output and diff previews can nevertheless contain secrets from files or
commands, so treat a session log as sensitive when sharing or backing it up.

## Stability

The session format is unstable for the whole 0.x line. `format` may be
incremented in any 0.x release. A build goes on reading the older formats whose
lines still mean what they meant, so your history keeps replaying across an
upgrade; a file from a *newer* build is refused rather than half-understood, and
so is one whose format changed the meaning of a line. A refusal says a session
cannot be continued, which is a better answer than continuing a different one.
