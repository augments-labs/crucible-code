# What the transcript draws

The transcript is the conversation as crucible draws it: your prompts, what
each tool did, and the model's answers. An answer is Markdown, and it is drawn
as it arrives, a few characters at a time. Most of it can be: a word is bold
or a link the moment its markers close, and a code fence is highlighted line
by line as it streams.

## Blocks held until they close

A few blocks cannot be drawn until the last of them has arrived, so crucible
holds each one back and draws it whole once it closes:

| Block | Opens with | Closes with |
| --- | --- | --- |
| Table | a line starting with `\|`, followed by a delimiter row such as `\| --- \| ---: \|` | the first line that does not start with `\|` |
| Display math | `$$`, `\[` or `\begin{…}` of an equation environment at the start of a line, or a fence named `math`, `latex`, `tex` or `katex` | the line its closing marker is on |
| Mermaid diagram | a fence named `mermaid` | the fence closing |
| Diff | a fence named `diff` or `patch` | the fence closing |

While one is being held, the foot of the transcript shows one quiet row in
its place and none of its text:

```text
◇ drawing a table · 4 lines so far
```

With the ASCII glyph set the row reads `o drawing a table - 4 lines so far`.
The row names a formula, a mermaid diagram or a diff the same way. It shows in
both [`output.screen`](../configuration/configuration.md#output) modes, in the
full screen while you are reading at the foot of the transcript, and the block
takes its place once the block closes. In the full screen, crucible's own
selection neither highlights nor copies the row's words. In the native screen,
or with <kbd>Shift</kbd> held while you drag, your terminal selects and copies
it as it does any other text.

A rule, a line of `---`, `***` or `___` on its own, is never held. It is drawn
across the window at once.

## Drawn again at every width

A table and a rule are kept as the text the model wrote, beside the rows they
were drawn as. When the window changes width, or you change the glyph set,
each is laid out again from that text for the window it is in now, so a table
drawn in a narrow window and then widened is the table a window that wide
would have drawn. A change of theme repaints the same rows in the new colours.
In native mode this happens when the terminal's scrollback is written again,
once the window has kept its new size.

What the transcript keeps is bounded, and a table counts against that bound by
both its text and its rows. When the oldest of the transcript is dropped, a
table goes with its text. The [session log](sessions.md) still holds every
answer.

## What is shown as written

Some text is drawn exactly as the model wrote it, rather than as a block:

- **A held block that grows past its limit.** A table is held for at most
  8 KiB, display math for 4 KiB, and a mermaid or diff fence for 16 KiB. Past
  that, the block is drawn as it was written, as it would have streamed.
- **A fence or display math the answer never closes.** It is drawn as
  written when the answer ends. A table the answer ends in is still drawn as
  a table: its last row is where it stops.
- **Display math, mermaid diagrams and diffs.** They are held so they arrive
  whole, and are then shown as written.
- **A table without its outer bars.** Lines such as `name | since` that do not
  start with `|` are prose, not a table.
- **Lines with bars and no delimiter row.** They are drawn as written.
- **A `$` in prose.** A price such as `$5`, or `$$` after other words on a
  line, opens no formula.

A table with more columns than the window can hold is still drawn as a table,
with each cell folded into the room its column has, however narrow.
