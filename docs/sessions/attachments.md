# Attachments

A file named in a prompt goes to the model with it. Type the path of a
picture, a PDF or a video, or paste a picture from the clipboard, and the next
request carries the file's bytes beside your words. Nothing asks first: you
typed the name, in the sentence you are sending, and that is the choice.

The session log never holds the bytes. The transcript records the path, and
each request reads the file again, so a log with twenty pictures in it holds
twenty paths rather than twenty pictures. A file that has changed since you
attached it is left out, and the model is told why, as
[What the model is told](#what-the-model-is-told) shows.

## Naming a file

Any word in the prompt that ends in one of the extensions under [What may be
attached](#what-may-be-attached), and names a file that is there, is attached:

```
what is wrong with the layout in shot.png
```

A relative path names a file in the workspace, and an absolute path under a
directory that [`permissions.extraDirectories`](../permissions/directories.md)
adds counts as one too. A leading `~` is not expanded, so write the path out.
An absolute path outside all of them is copied into the session's own store
first, rather than making that directory reachable to tools; [Where the bytes
are kept](#where-the-bytes-are-kept) says where. Put single or double quotes
around a path with spaces in it:

```
describe '/home/you/Pictures/Screenshots/Screen Shot.png'
```

The extension is read from the text after the last dot, whatever its case, so
`IMG_0001.JPG` is a picture. The word has to stand on its own: `shot.png,` with
a comma against it ends in `png,` and is a word like any other. A name that
does not lead to a file crucible can open (nothing there, a directory, a pipe)
is left as a word too, and nothing is said about it. The same file named twice
in one prompt, or named and pasted, goes once.

## Pasting a picture

<kbd>Ctrl+V</kbd> reads a picture off the operating-system clipboard, writes
it into the session's store as a PNG and puts a marker in the box:

```
[Image #1]
```

Markers count up in the order you paste, and a later prompt in the same
session can say `[Image #1]` again to send the same picture again. A marker
has to be spelled exactly that way; prose about images sends nothing. Ordinary
text still pastes the way your terminal pastes it. A clipboard that holds a
copied picture *file* rather than pixels (an absolute path, or the `file://`
address a file manager copies) pastes the picture it points at, by that path,
the same as typing the path would.

When the clipboard holds no picture, or the picture cannot be kept, the reason
is written under the box and the next key clears it:

```
the clipboard does not hold a readable image: <what the clipboard said>
the clipboard image is larger than the 4 MB one attachment may be
the clipboard image could not be imported: <what the file system said>
the clipboard could not be opened: <what the clipboard said>
this session has nowhere durable to keep a clipboard image
```

The last comes from a session that keeps no log, which has no store to copy
into.

The numbering starts again at 1 with each run of crucible, and after `/clear`
and `/resume`. A marker whose number names nothing stays in the prompt as
words.

## What may be attached

| Extension | Sent as | Label in the transcript |
| --- | --- | --- |
| `.png` | `image/png` | `[Image #N]` |
| `.jpg`, `.jpeg` | `image/jpeg` | `[Image #N]` |
| `.gif` | `image/gif` | `[Image #N]` |
| `.webp` | `image/webp` | `[Image #N]` |
| `.pdf` | `application/pdf` | `[PDF #N]` |
| `.mp4` | `video/mp4` | `[Video #N]` |

Anything else is a word like any other. The model can open a text file named
that way with the [`read`](../tools/files.md#read) tool. A Word document, a
spreadsheet, a slide deck or an e-book is refused there with a way to
convert it, as [A document is read by converting
it first](../tools/files.md#a-document-is-read-by-converting-it-first)
shows, and a file whose name says nothing about what it is gets a plain
refusal. No audio format is on the list, so no audio file is attached,
whatever the model reads.

The bytes are checked against the name once the file is read. A file called
`.png` whose bytes are not a PNG is refused with a line saying so, rather than
sent under a label it does not fit.

## Which models take one

Two halves have to agree. The protocol crucible speaks to the provider must
have a shape for the kind of file, and the model must read it; [What a model
can read](../providers/reading.md) is the table for the model's half. The
protocol's half, with each provider under the name a refusal line uses:

| Provider | Its requests carry |
| --- | --- |
| `anthropic` | pictures, PDFs |
| `google` | pictures, PDFs, video, audio |
| `moonshot` | pictures, video |
| `openai` | pictures, PDFs |

The narrower of the two decides, and it is decided for every request rather
than once: [switch model](../providers/reading.md#changing-model-in-a-session)
and the files already in the transcript go, or stay behind, by what the new
model reads. A model this build has no entry for gets neither answer: a file
named under one is not attached, and `/model` names a model it knows.

## How large

One file may be at most 4 MB. A larger one is refused when you name it, before
any of it is read, and a smaller copy would go.

One request carries at most 4 MB of files in all, by bytes rather than by
count. When the transcript holds more than that, the newest files that fit are
sent, and each older one keeps its place in the request as a sentence telling
the model to read it again if it needs it. A picture the model asked for
itself through `read` counts against the same request.

Files take room in the model's window as well as in the request. crucible
counts 750 tokens for each one until the provider's answer says what it really
cost, and [When the window fills](sessions.md#when-the-window-fills) says what
happens when that room runs out.

## What you see

Under the prompt you sent, one row per file that went with it, labelled by
kind and counted per kind in the order they were attached:

```
› compare shot.png with the mockup in design.pdf
  ⎿ [Image #1]
  ⎿ [PDF #1]
```

Not the path: the row says what went, and the prompt above it says which.
Files named by path come first, in the order the prompt names them, then the
pasted ones in marker order. The number on a row counts that kind within that
prompt, so a prompt that sends only `[Image #3]` shows `[Image #1]` under it.
The rows are drawn again whenever the session is replayed.

Where the answer arrives, a row for each file the request went out without:

```
  ⎿ shot.png not sent, can be read again
  ⎿ design.pdf not sent, this model does not read it
```

The first row is a file the request had no room for, or one that changed or
could not be read since it was attached; the model was told to read it again,
and naming it in a prompt attaches it afresh. The second is a file the model
being asked does not read, and `/model` is what changes that. The name is
relative to the workspace where the file is under it, and whole where it is
not.

## When a file is refused

A file that will not go is said before the turn starts, one line per file,
wrapped rather than clipped and left on the screen. The prompt still goes,
without that file. Each line names the file the way you typed it (a pasted
picture by the path of its copy) and says which half said no. The protocol:

```
! invoice.pdf is not attached: crucible's moonshot requests have no shape for a pdf.
```

That line goes on to say that nothing you type changes it, and that a later
release adds the shape. The model:

```
! clip.mp4 is not attached: <model> does not read a video. /model picks one that does.
```

```
! shot.png is not attached: this build has no entry for <model>, so it does not know what that model reads. That is not a refusal. /model names one it knows.
```

Then the file itself:

```
! shot.png is larger than the 4 MB one attachment may be, so it is not attached. A smaller copy of it would be.
```

```
! shot.png is not attached: it is named .png and its bytes are not a png. Rename it to what it is.
```

And, for a file outside the workspace, the copy that could not be made, or a
session that keeps no log and so has nowhere to copy it:

```
! /home/you/Pictures/shot.png could not be imported for this session: <what the file system said>
! /home/you/Pictures/shot.png is outside the workspace and this session has nowhere durable to import it.
```

## What the model is told

The files that go are written into the request the way the protocol spells
them; [What crucible puts in a
request](../providers/reading.md#what-crucible-puts-in-a-request) has each
one. The prompt itself goes as you typed it, path words and markers included.

A file that does not go keeps its place with one sentence, naming the file by
its full path:

```
/home/you/project/shot.png is not attached to this request, to keep the request within its size limit: read it again if you need it.
```

The middle of that sentence is one of three: `to keep the request within its
size limit`, `because it changed after it was attached` or `because it could
not be read`. A file the model does not read gets a sentence with no next move
in it, because reading the file again would produce the same kind:

```
/home/you/project/clip.mp4 is not attached to this request: it is video, which the model being asked does not read.
```

## Where the bytes are kept

A workspace file, or one under an extra directory, stays where it is, and
nothing is copied. The session log
records the path, the kind, the media type and the SHA-256 of the bytes as
they were when the file was attached, and never the bytes:

```json
{"attached":[{"hash":"…","media_type":"image/png","modality":"image","path":"/home/you/project/shot.png"}],"user":"what is wrong with the layout in shot.png"}
```

The hash is what says, on a later request, whether the file is still the one
that was attached.

A file outside the workspace, and every picture pasted as pixels from the
clipboard, is copied under the session directory, beside the logs:

```
~/.crucible/sessions/attachments/<session id>/<sha256>.<extension>
```

The name is the hash of the bytes, so the same picture pasted twice is one
copy, and the transcript records the copy's path, so moving or deleting the
original changes nothing. `CRUCIBLE_CODE_HOME` moves the copies with
everything else ([Where they are kept](sessions.md#where-they-are-kept)), and
the directory and each copy in it are created readable by you alone, the same
as a log ([Who can read them](sessions.md#who-can-read-them)). Nothing removes
a copy: deleting a session's log leaves its copies, and deleting the directory
named for that session is how you forget them.

## Picking a session up again

`--continue`, `--resume` and `/resume` replay the transcript with the
attachment rows under each prompt, and every request after that reads every
file in the transcript again, from the path the log recorded. What can have
changed by then:

- A file whose bytes are no longer the ones attached is not sent. The model
  is told it changed, and the answer arrives with `not sent, can be read
  again` beside the name. Naming the file again attaches it as it is now.
- A file that is gone, or cannot be opened, gets the same row, with the model
  told it could not be read.
- A model switched to one that reads less, or to one this build has no entry
  for, leaves the files it does not read behind with `not sent, this model
  does not read it`. Switching back carries them again.
- The markers are new. `[Image #1]` in a prompt of a continued session names
  nothing until a picture is pasted in that session. To send a pasted picture
  again, paste it again, or type the path of its copy: the log holds it, and
  a copy that already exists with the same bytes is used as it is.

`/clear` starts a new session, so pictures pasted after it are copied into
that session's own directory, and the ones before stay with the log they
belong to.

## When the model asks for one itself

`read` on a picture hands the file back attached rather than as text, under
the same ceilings; [A picture is looked at rather than
read](../tools/files.md#a-picture-is-looked-at-rather-than-read) has the
sentences it answers with. A PDF or a video is not handed back that way even
on a model that reads one: [A document is read by converting it
first](../tools/files.md#a-document-is-read-by-converting-it-first) and [A
video is read as the frames pulled out of
it](../tools/files.md#a-video-is-read-as-the-frames-pulled-out-of-it) say what
the model gets instead.
