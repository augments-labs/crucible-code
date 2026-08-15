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

The one session `/resume` will not pick up is the one you are in. It says so,
rather than reporting the claim it would find on that file as another crucible's.

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
{"format":2,"session":"…","workspace":"/home/you/code/my-project"}
```

Then one line per message — what you typed, what the model said and asked to
run, and what the tools returned.

`/clear` is written down rather than cut out: it appends a line saying the
session forgot what it had said, and everything above that line stays in the
file. `--continue` replays from there, so a cleared session comes back as what
was said after it and nothing before.

The `workspace` in that header is what `--continue` matches against, and
`format` is what makes a file from a build that spelled things differently a
refusal rather than a half-understood replay. No key, credential or environment
value is ever written to one of these files.

## Stability

The session format is unstable for the whole 0.x line. `format` may be
incremented in any 0.x release, and when it is, older files are refused rather
than migrated — a refusal says a session cannot be continued, which is a better
answer than continuing a different one.
