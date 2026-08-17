# Reading and changing a file

Three tools name one file and work on it: `read` looks, `edit` replaces part of
it, `write` puts down the whole thing. All three take a path relative to the
workspace root and refuse one that leads outside it.

## `read`

| Argument | What it is |
| --- | --- |
| `path` | The file to read. Required. |
| `offset` | The first line to return, counting from 1. Defaults to the start. |
| `limit` | How many lines. Defaults to 2000, never more than 10000. |

The answer is the file's lines with their numbers in front, the way `cat -n`
writes them:

```
     1	//! The client and its retry policy.
     2	
     3	use std::time::Duration;
```

The numbers are the file's own, so an `offset` of 500 answers with line 500
numbered 500 — which is what lets the model quote a line to `edit` and talk
about it to you.

Three things end an answer early, and each says so. A `limit` reached leaves
`[more follows: call read again with offset 501]`. The 30000-byte bound does the
same, at whatever line filled it. And a single line longer than 2000 characters
is cut there and marked `[line cut at 2000 characters]` — one minified bundle on
one line would otherwise be the whole answer.

An `offset` past the end of the file is not a failure: the answer is
`one.txt has no line 900`, which tells the model it walked off the end rather
than that something is wrong. A file that is not text says so too, rather than
putting its bytes in the transcript.

Reading is never asked about. It is allowed, or refused by a `deny`
[rule](../permissions/rules.md), and the question is answered without being put
to you.

## `edit`

| Argument | What it is |
| --- | --- |
| `path` | The file to change. Required. |
| `find` | The exact text to replace, indentation included. Required unless `edits` is sent. |
| `replace` | What to put in its place. Empty deletes. Required unless `edits` is sent. |
| `all` | Replace every occurrence instead of requiring exactly one. |
| `edits` | Several changes — `find`, `replace` and `all` each — instead of one. |

Exact text in, exact text out — no patch format and no line numbers. A model
that has just read a file can quote from it, and quoting is the one thing it can
do without counting.

A call carries either one change or a list of them, and sending both shapes is
refused rather than half-read. `edits` is what turns ten changes to one file
from ten turns into one: they are made in order, each looking at what the one
before it left, so a change can find text an earlier one wrote. If any of them
cannot be made the file is left exactly as it was, and the answer says which:
`edit 2 of 4 could not be made, so nothing was changed: that text does not
appear in src/main.rs`. A file holding half of what was asked for is a state
nobody chose, and the model cannot see which half it got without reading the
file back.

`find` must appear exactly once unless `all` is true. Where it appears more
often, the call comes back as
`that text appears 3 times in src/main.rs: include more of the surrounding
lines, or pass all`, and the model picks one of those two. Where it appears not
at all, `that text does not appear in src/main.rs`. Neither ends the turn.

The file and its result are each capped at 1000000 bytes: an exact whole-file
transformation needs both in memory at once, and something larger wants a
different tool rather than a bigger number.

The replacement is prepared beside the file, flushed, and only then renamed over
it. Nothing ever observes the half-written interval that changing a file in
place creates, and a failure part-way through leaves the original whole.

## `write`

| Argument | What it is |
| --- | --- |
| `path` | The file to write. Required. |
| `content` | The complete new contents. Required. |

A path whose parent directories are missing gets them, one checked level at a
time, on Linux, macOS and FreeBSD. On Windows the parent directory has to be
there already.

The answer is `created src/main.rs, 41 lines` or `replaced src/main.rs, 41
lines`. The file is put down the same way `edit` puts one down: written beside
its destination and renamed over it, so it is either the old file or the new one.

### It refuses to replace a file nobody has looked at

`write` names a file and discards whatever is in it, which makes it the one tool
here that can destroy work. So it will only replace a file this run has read or
written itself. Anything else comes back as

```
notes.md has not been read, so replacing it would discard what is in it: read it first
```

and the turn continues — the model reads the file and writes it again, which
costs one call and is the whole remedy.

This is the case a permission question cannot cover. A `write` you approve is
one you agreed to; what neither of you can see at that moment is that the file
holds a paragraph somebody wrote an hour ago, or a change another program made
while the turn was running. The refusal is about what is already there rather
than about the write, which is why it is decided before anything is opened.

What counts as having looked:

- A `read` call that showed at least one line of that file. A call that
  answered `has no line 900` showed nothing, so it is not one — otherwise a
  single offset past the end would be a way past this.
- A `write` call that created or replaced it. What the agent just put down it
  has by definition seen, so correcting it does not cost a round trip spent
  learning what the same turn wrote.

Files are remembered by their resolved path, so `./notes.md` and `notes.md` are
one file rather than two. The last 1024 of them are kept and reading one again
moves it back to the front, which is enough for any real session; past that the
least recently read is forgotten and a `write` to it asks for a read first.

The record belongs to the session rather than to the transcript. `/clear` and
`/resume` both leave the session those files were read in, so both empty it, and
a `write` in the session that follows asks for the read again. Leaving crucible
drops it for the same reason, so a fresh run refuses a file the last one read.
Nothing about it is written to disk.

## What a change leaves on screen

The answer `edit` and `write` send the model says what was done. What is drawn
under the call is the change itself:

```
● Edit(.github/workflows/release.yml)
  └ Added 2 lines, removed 1 line
      303            scripts/smoke.sh --no-provider
      304
      305 -  # Shared-runner numbers are trend data.
      305 +  # What stops a tag whose build got slower
      306 +  # is the budget below.
      306    budgets:
      307      name: release budgets
```

A line that went is marked `-` and one that came is marked `+`, each on ground
saying which way; the lines around them did not move and are there to be read
against. Every one carries the number it has in the file, and both sides of a
replacement start at the same one, since one line stands where the other did.

At most 64 lines are drawn, and a change longer than that is still counted whole
on the row above, which then says how much of it is not below:
`Added 300 lines, removed 12 lines (248 of them not shown)`.

`write` is the one that can have nothing to show. It is handed the new file and
has to read the one it is about to discard, so a file over 1000000 bytes or one
that is not text is replaced with the block left out rather than guessed at. The
write itself happens either way.
