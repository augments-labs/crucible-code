# Sessions

A session is a conversation bound to a working directory. Every session is
written to a file as it happens, so it survives the terminal it was held in.

## What was worked on here

The screen crucible opens with lists the last few sessions started in the
current directory, newest first, each one showing what it was first asked and
how long ago it began. A directory nobody has worked in says so instead.

The list is short on purpose, and so is the work behind it: crucible reads the
names in its sessions directory to put them in order — a session's name carries
the time it started — and opens only the newest handful of files it finds
there. A machine that has held crucible for a year opens the same number as one
that installed it this morning.

Only sessions that were asked something appear. Starting crucible and leaving
without typing records a file with no turns in it, and there is no row to draw
for one.

## What is left behind when crucible ends

crucible draws on a screen it borrows from the terminal and hands back when it
ends, so what you scroll up to afterwards is the shell you started from. The
transcript is not in the terminal's scrollback and never was.

What crucible writes on its way out is a line naming the file the transcript is
in, and a line saying `crucible --continue` picks the session up. Where the log
stopped being written part-way through, that first line says so beside the
file — a transcript missing its tail is never handed over as a whole one.

Nothing is written where crucible had no screen to borrow, which is every run
whose input or output is not a terminal. Nothing was hidden from you in one, so
everything it drew is already in your own scrollback.

## Moving through the transcript

The wheel moves the transcript a few rows at a time. For a long jump, use
`transcript map →` at the bottom right, directly below the permission mode,
model and effort. The arrow says it opens. Pointing turns the theme's exact
accent into a compact background rectangle and switches the text to contrasting
black or white. Click it and the whole bottom row becomes a map from `first` to
`now`; hollow
marks are prompts, and the filled mark is the place currently on screen.

Drag anywhere along the map for an absolute jump, or click a hollow prompt mark
to land on that prompt. The prompt box and everything standing over it stay where
they are. The wheel still makes precise adjustments while the map is open, moving
the transcript and its mark together. Three seconds after the last drag, click or
wheel turn, the map becomes the bottom-right control again. It takes no keyboard
binding: Escape, Return, Space and the arrows keep their existing meanings.

The open map uses the theme's quiet and accent colours and the terminal's own
background. With colour off, the same shapes carry the distinction.

## Continuing

```bash
crucible --continue
```

This picks up the most recent session **started in the current directory**. Run
it somewhere else and you get that directory's most recent session instead —
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

A log the process was killed part-way through writing costs the line it was on
and nothing more — the turns before it are still a transcript, and `--continue`
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

A log that stops between a tool call and its result is the one case where the
last recorded turn does not come back: an unanswered question is not something
to send a provider, so the replay ends before it and the file is cut to match.

## Switching without restarting

`/resume` shows the same list the opening screen does — this directory's last
nine sessions, newest first, numbered — and picking a number off it changes
which session the crucible you are in is recording to. It reads a log the way
`--continue` reads one, and everything above applies to it: the transcript comes
back, the file is cut to what was replayed before anything new is appended, a
log this build cannot read is refused rather than half-understood, and a session
another crucible has open is not available.

What is different is the session being left. It is finished here rather than
when the process ends, so its log is complete and can be continued from
somewhere else immediately. Its session-long permission answers end with it —
"for the rest of this session" was answered about that session, and the new one
is asked again. The mode carries over, and so do the rules in your configuration
files, which were never held in a session to begin with.

The screen is different too. The session picked up replaces what was on it
rather than following it: the transcript is emptied first, so what you scroll
back through is one conversation, with the welcome card at the top of it —
exactly the screen starting crucible on that session would have drawn. What was
on screen before is not recoverable from inside crucible — the session it
belonged to is still on disk, and picking it back up is how you read it again.

The one session `/resume` will not pick up is the one you are in. It says so,
rather than reporting the claim it would find on that file as another crucible's.

## What comes back looks like what you left

A session put back on the screen is drawn by the code that drew it live, so it
is the same session rather than a rendering of one. A result too long for its
row still says how much it left over, still stands out from the rows with
nothing behind them, and still opens on
[<kbd>Ctrl+O</kbd> or a click](../getting-started/getting-started.md) — the
lines are read back out of the log rather than out of the run that produced
them.

How much of the window is left comes back with it. A log records what each
request carried, so a session picked up says so straight away rather than
waiting for its next answer to measure it — unless it is picked up under
different instructions or a different set of tools, where the reading is about
a request this run would not send and the row waits, as it always did.

One thing does not: a call that changed a file replays as the result it
returned rather than as the lines it moved, because a diff is never written to
a log.

## One at a time

A session is claimed for as long as it is open, so the one crucible is writing
now is not one another crucible can continue:

```
crucible: /home/you/.crucible/sessions/1786713045000-3f9c2a.jsonl is open in another crucible
```

`--continue` says that and stops, having read nothing and changed nothing.
Continuing a session cuts its log back to what was replayed, so without this the
second crucible would delete the turns the first had already written and still
believes are there, and both would append to one file from then on.

Starting a session is refused only if it cannot find itself a free name — eight tries, each a millisecond and twenty-four bits of randomness. Two crucibles in one directory are two
sessions, each recording a log of its own — it is only continuing that has to
pick one, and only continuing that can be told no.

The claim is the operating system's, taken on a `.lock` file beside the log and
released however the process ends, so a crucible that crashed leaves no session
stuck as busy. The file it was taken on stays where it is and is never mistaken
for a session. Some network filesystems have no locks to take at all; there,
`--continue` goes ahead without the check rather than refusing everything.

A `.lock` file that cannot be made at all — something already in its place, a
directory gone read-only — is a third answer and not that one. Nothing was asked
about the log, so nothing is assumed about it: `--continue` stops with
`could not claim the session log …` and the reason the operating system gave,
having read nothing and changed nothing.

## When the window fills

Every model accepts only so much at once, and a long session eventually reaches
it. crucible shows how much is left against the end of the row a turn runs on:

```text
✳ writing (1m 12s · ↓ 4.1k · esc to stop)                     10% window left
```

The percentage measures the part of the window the transcript may still use.
Room reserved for the answer and its tool results is outside it, so `0%` is the
safe compaction boundary — not the model's literal last token. The fixed cost
every request carries — the system instructions and the tool schemas — is
outside it too, so a session that has said nothing begins at `100%`, and
automatic compaction begins at `0%` while that reserve remains available.

Where crucible does not know how much the model accepts, the prompt says `window
unknown` rather than inventing a percentage. While room is being made, the last
reading remains until the compacted transcript replaces it.

When there is no longer room for another exchange, crucible **makes room in the
middle of the turn and the turn carries on**. It asks the model to write down
what is worth keeping in a fixed Goal/Constraints/Progress/Decisions/Next
Steps/Critical Context checkpoint, and that recap stands where the messages it
replaced were. The most recent turns stay word for word — bounded in tokens
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

Nothing is deleted. What is replaced is what the **model** is sent; the session
log keeps every message of it, which is what `--continue` reads and what you
can go back and look at.

`/compact` does the same thing between turns, when you would rather choose the
moment. A session with nothing behind it says so instead of spending a request.

Nothing is frozen while the notes are being written. The box takes what you type
throughout, and a line finished there is sent as the next turn once there is
room. Escape stops the notes, and the session is left exactly as it was:

```text
! stopped
```

Half a recap is not a session's memory, and standing it in place of the messages
it was meant to replace would lose the rest of them for good. So a cancelled,
malformed, filtered, silently ended, or token-truncated recap replaces nothing,
and a turn that was making room for itself ends there rather than asking for the
notes again.

If a provider refuses a request for want of room — because crucible had the
window wrong, or was never told it — the same thing happens and the question
goes back once the session is smaller. A compaction that frees nothing is not
tried twice; the turn stops and says so, and `/clear` or a model with a larger
window is what gets past it.

[Configuration](../configuration/configuration.md#compaction) has the keys.

## Picking up a large one

A session that ran for hours is worth what it cost to build, and carrying all of
it back is what that costs again — on the next request and on every request
after. So picking up a large one asks first:

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

Nothing is decided for you. The one case where carrying it whole is right — you
are about to ask about something said two hours ago — is the case crucible
cannot see from here.

Escape carries it whole, which is the answer that changes nothing — and it means
the same thing once the notes have started, where it stops them and leaves the
session as it was. *Stop asking* writes `compaction.askOnResume` down as `0`;
set it to a number of tokens instead to move the point where the question
appears.

## When recording stops

A write to the log can fail — a full disk, most often — and the turn does not
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
down costs nothing at all — the empty line it leaves is not a message and is not
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
path — a relative one is ignored rather than resolved against wherever you
happened to start crucible, which would scatter a home directory across every
repository you work in. `HOME` is read the same way, and if neither is absolute
crucible says so and stops instead of guessing.

When you set it, everything is under it and nowhere else is consulted — which
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

One more file sits beside them: `prompt.history`, holding the lines the arrow
keys walk back through. It is one file for every directory rather than one per
directory, so it cannot grow with the number of checkouts you work in, and each
line records which directory it was typed in so only that directory's prompts
are offered back. Each directory keeps its
newest hundred prompts and no more, and the file itself is bounded again across
all of them, so it cannot grow either with how long you work or with how many
checkouts you work in. Deleting it is how you forget what you have typed.

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
contents of every file that was read and everything a command printed, so on a
shared machine the usual default would hand all of it to anyone with an account.
The directory is closed for the matching reason from the other side: somewhere
another account can write is somewhere a log can be *planted* for `--continue` to
replay back to the model as though you had typed it.

On Unix that is a mode: `0600` on a log and `0700` on the directory. On Windows
it is an access control list naming the account crucible is running as and
nothing else, with the inheritance from your user profile switched off — so what
that profile hands Administrators and SYSTEM does not reach a transcript merely
by sitting above it. An administrator can still take ownership and read it, which
is the same escape by a longer route and the honest limit of what a file
permission promises on either system.

Both are set on every start and every `--continue`, not only when they are
created, because a directory made by an earlier build or by hand keeps whatever
it was made with — and `--continue` is the run that goes looking in it. A path
that cannot be set stops the run and says so, rather than carrying on and writing
a transcript somewhere the whole machine can read.

## What is in a file

One JSON object per line, in the order things happened. The first line says what
the file is and where it belongs:

```json
{"format":3,"session":"…","workspace":"/home/you/code/my-project"}
```

Then one line per message — what you typed, what the model said and asked to
run, and what the tools returned.

`/clear` writes nothing here. It closes this log and opens another, so the
session it left is complete and replays whole, the same as any other on
`/resume`'s list. A log written by an earlier crucible can hold a line saying
the session forgot what it had said, with everything above that line still in
the file; `--continue` replays such a log from that line rather than from the
top.

The `workspace` in that header is what `--continue` matches against, and
`format` is what makes a file from a build that spelled things differently a
refusal rather than a half-understood replay. No key, credential or environment
value is ever written to one of these files.

## Stability

The session format is unstable for the whole 0.x line. `format` may be
incremented in any 0.x release, and when it is, older files are refused rather
than migrated — a refusal says a session cannot be continued, which is a better
answer than continuing a different one.
