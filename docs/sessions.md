# Sessions

A session is a conversation bound to a working directory. Every session is
written to a file as it happens, so a conversation survives the terminal it was
held in.

## Continuing

```bash
crucible --continue
```

This picks up the most recent session **started in the current directory**. Run
it somewhere else and you get that directory's most recent session instead —
which is the point. Two projects open in two terminals are two conversations.

`--continue` replays the transcript so the model has the earlier turns, and
appends to the same file. It does not restore permissions: a grant lives as long
as the process that made it, so a continued session asks again the first time it
wants to change a file or run something. See [Permission](permission.md).

If nothing was ever recorded for this directory, crucible says so and stops
rather than silently starting a new conversation.

## Where they are kept

One file per session, under the data directory:

```
$XDG_DATA_HOME/crucible/sessions/
$HOME/.local/share/crucible/sessions/     # when XDG_DATA_HOME is not usable
```

`XDG_DATA_HOME` is used only when it is set to an absolute path; a relative one
is ignored rather than resolved against wherever you happened to start crucible,
which would scatter sessions across the directories you work in. `HOME` is read
the same way, and if neither is absolute crucible says so and stops instead of
guessing.

Each file is named for its session and ends in `.jsonl`. They are yours: reading
one with `cat`, `jq` or a text editor is a supported thing to do, and deleting
one is how you forget a conversation.

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
