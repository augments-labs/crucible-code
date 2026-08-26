# Reading and changing a file

Three tools name one file and work on it: `read` looks, `edit` replaces part of
it, `write` puts down the whole thing. All three take a path relative to the
workspace root. `edit` and `write` refuse one that leads outside it; `read`
puts it to you and follows your answer.

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
putting its bytes in the transcript — and where its name says what it is, the
refusal says what would turn it into something readable.

### A picture is looked at rather than read

A `.png`, `.jpg`, `.gif` or `.webp` has no lines to number, so `read` hands the
file back to be looked at instead:

```
shot.png is attached as an image rather than read as text.
```

The picture goes with the answer and the model sees it. Nothing is asked first:
`read` is a read, and the file is one inside the workspace.

Two things still stop it. A picture larger than the 4 MB one request carries is
refused, because that is more than a request could carry whatever else it holds.
And a model that does not read pictures gets the answer without the file and a
row naming what stayed behind — [what a model can
read](../providers/reading.md) is the table that decides.

A file named `.png` whose bytes are not a PNG is read as text, and gets whatever
the text reader makes of it. The name is what somebody meant by it; the bytes
are what is there, and where they disagree the bytes win.

A PDF is the exception, and deliberately: it is not handed back this way even on
a model that reads one. What it gets is the refusal below, naming a converter —
an answer that works on every model crucible offers, where an attachment works
on two of the three.

### A document is read by converting it first

`read` answers with text, and a Word document, a spreadsheet, a slide deck, an
e-book or a PDF is not text. Where the name says which of those a file is, the
refusal names something that would turn it into text:

```
report.docx is not a text file. It is a Word document — convert it and read
what comes out, and its pictures come out beside it into converted-media/ where
each one can be attached to a prompt and looked at, for example: pandoc
report.docx --extract-media=converted-media -o converted.md
```

The command named is one that is **on your machine**. crucible looks for each
converter it knows about, best first, and names the first one you have:

| The file | Tried, in this order |
| --- | --- |
| `.docx` `.odt` `.rtf` | `pandoc`, then `textutil` on macOS, then `soffice` |
| `.epub` | `pandoc` |
| `.xlsx` `.xls` `.ods` | `soffice`, then `xlsx2csv` |
| `.pptx` `.odp` | `soffice` |
| `.pdf` | `pdftotext`, then `soffice` |

The order is what survives, not what is likeliest. `pandoc` writes Markdown and
keeps headings, lists and tables; the others flatten a document into prose — a
heading becomes a line like any other, and a table becomes tab-separated rows.

`--extract-media` is there because a picture is the one thing no converter turns
into text. `pandoc` without it writes the *reference* and not the file, leaving
a link to something that was never saved; with it the pictures come out beside
the Markdown as ordinary files — and an ordinary file is one `read` hands back
to be looked at, or that you can
[name in the prompt](../getting-started/getting-started.md#naming-a-file-in-the-prompt).
That is the second half of reading a document: the words come out of the
converter, and the diagram goes back in as a picture.

Only `pandoc` is described that way, because only `pandoc` does it. The rest
keep a picture's description, where it has one, and nothing else — so their
refusals promise nothing about files that will not be there.

LibreOffice's command line is `soffice`, and its installer puts it on the `PATH`
on Linux only — so on macOS and Windows crucible also looks where the installer
writes, and names the absolute path it found. Where you have none of the
converters for a file, the answer says so, and which to install:

```
budget.xlsx is not a text file. It is a spreadsheet, and nothing installed
here converts one — soffice or xlsx2csv would.
```

Nothing is converted for you and nothing is installed for you. The command is a
suggestion, run — if you allow it — by [`bash`](commands.md) like any other. A
file whose name says nothing about it gets the plain refusal, because a
suggestion that does not fit is worse than none.

What comes back is the text of the document, and only the text. Cell formatting
and layout do not survive a conversion, and neither does a picture — except
under `pandoc`, where it survives as a file beside the text rather than as part
of it. A slide deck or a spreadsheet whose content *is* a diagram still converts
to very little.

### A video is read as the frames pulled out of it

Nothing crucible speaks to reads a video, and everything it speaks to reads a
picture. So a video is refused in the same shape as a document — turn it into
something a model takes in, then hand that over — with a different second half:

```
clip.mp4 is not a text file. It is a video — nothing here reads one and
everything here reads a picture, so pull frames out and attach those. ffprobe
-v error -show_entries format=duration -of csv=p=0 clip.mp4 says how long it is,
and ffmpeg -i clip.mp4 -vf fps=1 -frames:v 20 frame-%03d.jpg takes one a second
and stops at twenty, which is about as many as one request carries. Sample a
long recording sparsely rather than sampling the start of it.
```

`.mp4`, `.mov`, `.mkv`, `.webm`, `.avi` and `.m4v`, all by `ffmpeg`. `ffprobe`
is named beside it because a rate chosen without a duration is chosen blind:
one frame a second is thirty frames of a clip and seven thousand of a two-hour
recording. It ships with `ffmpeg`, so there is one thing to install — which is
why the answer for a machine that has neither names only `ffmpeg`.

Neither is looked for on the `PATH` alone. `ffmpeg` publishes an archive rather
than an installer on macOS and Windows, so it usually arrives through a package
manager, and crucible also looks in the directories those managers document
their shims into — Homebrew's and MacPorts' prefixes, and winget's,
Chocolatey's and Scoop's. It is the same lookup that finds LibreOffice where
its installer puts it, held to the same line: a directory is looked in because
an installer is known to write there, never because a program is often found
there. A folder you unpacked an archive into yourself is not looked in, because
guessing at one is how an answer sounds right and is wrong.

Three things are lost, and knowing which is what makes the frames readable:

- **The soundtrack.** No model crucible offers accepts audio, so a video whose
  content is speech becomes a video with no content.
- **Everything between two sampled frames.** Good for *what is on this screen*,
  wrong for *what happened between these two moments*.
- **Most of a long recording.** One request carries 4 MB of files, and twenty
  full-size frames is already about that. That is why the suggested command
  carries a rate and a cap rather than extracting everything: a minute of
  thirty-frames-a-second video is 1 800 files, and a suggestion that fills a
  directory is worse than none. Smaller pictures buy more of them —
  `-vf fps=1,scale=1280:-1` is the same command with the frames scaled down.

crucible extracts nothing and chooses no frames. The commands are suggestions,
run — if you allow it — by [`bash`](commands.md), and the pictures that come out
are read like any other.

Reading inside the workspace is not asked about: it is allowed, or refused by
a `deny` [rule](../permissions/rules.md), and the question is answered without
being put to you. A path that leads outside the workspace is the exception —
that read is put to you the way a command is, and runs only on your yes.

## `edit`

| Argument | What it is |
| --- | --- |
| `path` | The file to change. Required. |
| `find` | The exact text to replace, indentation included. Required unless `edits` is sent. |
| `replace` | What to put in its place. Empty deletes. Required unless `edits` is sent. |
| `all` | Replace every occurrence instead of requiring exactly one. |
| `edits` | Several changes — `find`, `replace` and `all` each — instead of one. |
| `description` | One line saying what the call is for, shown to you on the [question](../permissions/permissions.md#the-question). Optional, and the tool never reads it. |
| `explanation` | The long form of the same thing: a list of strings, one per paragraph, shown on the question when you press `ctrl+e`. Optional, and the tool never reads it. |

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
| `description` | One line saying what the call is for, shown to you on the [question](../permissions/permissions.md#the-question). Optional, and the tool never reads it. |
| `explanation` | The long form of the same thing: a list of strings, one per paragraph, shown on the question when you press `ctrl+e`. Optional, and the tool never reads it. |

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
