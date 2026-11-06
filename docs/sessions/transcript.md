# What the transcript draws

The transcript is the conversation as crucible draws it: your prompts, what
each tool did, and the model's answers. An answer is Markdown, and it is drawn
as it arrives, a few characters at a time. Most of it can be: a word is bold
or a link the moment its markers close, and a code fence is highlighted line
by line as it streams, in a frame that names its language.

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

## The source of a table, formula, diagram or diff

A table, a formula, a mermaid diagram and a diff are drawn from the text the
model wrote, and that text can be had whole. In the full screen, a selection
that touches any row of one of them copies the whole of its source, once however
many of its rows or times it is touched, in order with the prose around it.
Two blocks in one selection give both. A rule copies as it is drawn. In the
native screen, or with <kbd>Shift</kbd> held while you drag, your terminal
copies what it shows. In both screens <kbd>Ctrl+O</kbd> lists the source of
each drawn block, newest first, after any cut results: see
[Results cut short](../reference/keys.md#results-cut-short).

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

## Code blocks

A fenced block of code is drawn in a frame. The top edge names the language
written after the opening fence, every line of code stands behind a bar, and
the bottom edge closes it:

```text
┌─ rust
│ fn main() {
│     println!("hello");
│ }
└─
```

A fence that names no language has an unlabelled top edge. Only the first word
after the fence is used, and a name longer than 32 columns is cut short with an
ellipsis. A fence the answer never closes has no bottom edge. With the ASCII
glyph set the corners are `+`, the rule `-` and the bar `|`.

A line of code longer than the window is never folded at a space: it is cut
where the room runs out, and each row after the first starts with `┆` (`:` in
the ASCII glyph set) where the bar would be. In the full screen, crucible's own
selection copies the code without the frame (its bars and both edges), and a
line that wrapped copies as the one line it was written as, with no line break
where it wrapped. In the native screen, or with <kbd>Shift</kbd> held while you
drag, your terminal copies what it shows. When the window changes width the
rows are cut again from the line.

## Lists and quotes that wrap

A bullet, a task or a numbered item that is longer than the window continues
under the first column of its words, not under its mark. A numbered item hangs
after its own number, so `1. ` hangs three columns and `10. ` hangs four, and
an item nested under another keeps the indentation that nested it. A quote
that wraps shows its bar on every row it takes. It is the same in both
[`output.screen`](../configuration/configuration.md#output) modes and in both
glyph sets, and when the window changes width the rows are folded again from
the line, so a narrower window hangs them at its own width.

A dash or a number that opens a line inside a quote is shown as written, not as
an item, so a wrapped one keeps the quote's bar and nothing more.

## What is shown as written

Some text is drawn exactly as the model wrote it, rather than as a block:

- **A held block that grows past its limit.** A table is held for at most
  8 KiB, display math for 4 KiB, and a mermaid or diff fence for 16 KiB. Past
  that, the block is drawn as it was written, as it would have streamed. A
  fence is drawn in its frame, and its top edge says why:
  `┌─ mermaid · longer than this can hold · shown as written`.
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
