# Sessions

A session is a conversation bound to a working directory. Every session is
written to a file as it happens, so it survives the terminal it was held in.

## Continuing

```bash
crucible --continue
```

This picks up the most recent session **started in the current directory**. Run
it somewhere else and you get that directory's most recent session instead —
which is the point. Two projects open in two terminals are two sessions.

`--continue` replays the transcript so the model has the earlier turns, and
appends to the same file. It does not restore permissions: anything allowed
with `always` lives as long as the process that made it, and the mode is read
fresh from configuration at every start. See
[Permissions](../permissions/permissions.md).

If nothing was ever recorded for this directory, crucible says so and stops
rather than silently starting a new session.

A log the process was killed part-way through writing costs the line it was on
and nothing more — the turns before it are still a transcript, and `--continue`
hands them back. The half-written line is dropped from the file as the session
is continued, before anything new is appended: the next turn would otherwise be
written onto the end of it, which turns a lost line into a log that cannot be
read at all. Nothing that was handed back is touched.

A log damaged somewhere in the *middle* is refused instead: continuing from a
transcript with a hole in it would read to the model as you contradicting
yourself, and nothing would say why.

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
one is how you forget a session.

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
{"format":1,"session":"…","workspace":"/home/you/code/my-project"}
```

Then one line per message — what you typed, what the model said and asked to
run, and what the tools returned.

The `workspace` in that header is what `--continue` matches against, and
`format` is what makes a file from a build that spelled things differently a
refusal rather than a half-understood replay. No key, credential or environment
value is ever written to one of these files.

## Stability

The session format is unstable for the whole 0.0.x line. `format` may be
incremented in any 0.0.x release, and when it is, older files are refused rather
than migrated — a refusal says a session cannot be continued, which is a better
answer than continuing a different one.
