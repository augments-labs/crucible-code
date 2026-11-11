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
| Table | a line starting with `\|`, followed by a delimiter row such as `\| --- \| ---: \|`; or a line of two or more cells separated by `\|` with no outer bars, followed by a delimiter row such as `--\|--` with as many cells | the first line that does not start with `\|`; or, for one without outer bars, the first blank line |
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
across the window at once. A heading does not wait for its end either: its
words are drawn as they arrive. See [Headings](#headings) for the rule under
them.

## Headings

A heading is its words on a row of their own, with no `#` and no underline
drawn. A heading of the first level has a heavy rule on the row under it
(`━`, or `=` in the ASCII glyph set), and one of the second level a light rule
(`─`, or `-`). Each rule is as wide as the last row the words are drawn on, in
columns, so a heading that wraps is ruled as wide as its last row, and the
length of a row of `=` or `-` you wrote under it makes no difference. A heading
of the third level has its row and no rule, and so do the fourth, fifth and
sixth, which look like the third.

A heading is written with `#` marks (`# One`, `## Two`, `### Three`), or as a
line over a row of `=` (the first level) or `-` (the second):

```text
Title
=====
```

The row is one mark or more, up to three spaces in front of it, and spaces or
tabs after it, and nothing else. Only the last line of a paragraph is the
heading; the lines above it stay as they were. The two ways of writing a
heading are drawn alike. A row of `-` under a list item, a quote, a heading, a
code block or a table is a rule across the window, as it is after a blank
line, and a row of `=` there is text; so is a row of `=` or `-` of more than
128 bytes, and so is a title of more than 128 bytes. The row under a line of
two or more cells that has as many cells is the delimiter row of a table: see
[Blocks held until they close](#blocks-held-until-they-close). Any other line
over a row of dashes is a heading of the second level, so `one cell` over `---`
is one.

The words of a line that may be a title are drawn as they arrive. Only the
line break after it, and the start of the line under it, wait; when the row
turns out to be an underline the line is drawn again as the heading it is, and
when it turns out to be anything else nothing changes. In the native screen
the line stays out of the terminal's scrollback until that is known, and
nothing already written to it is taken back. Output that is not a terminal
keeps the markdown as it was written, the row of `=` or `-` included. Heading
rules are laid out again when the window changes width or you change the glyph
set, in both screens, from the heading as it was written.

## The source of a table, formula, diagram or diff

A table, a formula, a mermaid diagram and a diff are drawn from the text the
model wrote, and that text can be had whole. In the full screen, a selection
that touches any row of one of them copies the whole of its source, once however
many of its rows or times it is touched, in order with the prose around it.
Two blocks in one selection give both. A rule, and a heading with its rule,
copy as they are drawn, the words and then the rule. In the native screen, or
with <kbd>Shift</kbd> held while you drag, your terminal copies what it shows.
In both screens <kbd>Ctrl+O</kbd> lists the source of each drawn block, newest
first, after any cut results: see
[Results cut short](../reference/keys.md#results-cut-short).

## Drawn again at every width

A table, a rule and the rule under a heading are kept as the text the model
wrote, beside the rows they were drawn as. When the window changes width, or
you change the glyph set, each is laid out again from that text for the window
it is in now, so a table drawn in a narrow window and then widened is the table
a window that wide would have drawn. A change of theme repaints the same rows
in the new colours. In native mode this happens when the terminal's scrollback
is written again, once the window has kept its new size.

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

A block indented four spaces after a blank line is code too, and is drawn in
the same frame with an unlabelled top edge. The four spaces are not drawn, and
any past them are the code's own. The block ends at the first line that is not
blank and is indented less, and the blank lines before that line are drawn
after the bottom edge; the end of the answer closes it as well. A line
indented four spaces inside a list item, in a quote, or straight under a
paragraph line is not code, and is drawn as text as before.

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

## Alerts

A quote whose first line is nothing but `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`,
`[!WARNING]` or `[!CAUTION]` is drawn as an alert, whatever the letter case of
the kind. Each row of the quote, a `> ` that opens the line or follows spaces,
has a quiet bar down its left, and the first holds the kind's mark and its word
in bold, in place of the marker:

| Kind | Unicode | ASCII |
| --- | --- | --- |
| Note | `◆ Note` | `* Note` |
| Tip | `✓ Tip` | `+ Tip` |
| Important | `■ Important` | `# Important` |
| Warning | `▲ Warning` | `! Warning` |
| Caution | `✗ Caution` | `x Caution` |

The mark is drawn in the ink of its kind: a colour for Note, Important and
Caution, bold for Warning and a struck grey for Tip. The bar is drawn in the
quiet grey and the word in bold, and neither takes a kind's colour; the body
keeps whatever colour the answer gave its code spans and links. The bar is `▎`
in the unicode glyph set and `|` in the ASCII one, and a line of an alert that
wraps shows it on every row it takes, in both
[`output.screen`](../configuration/configuration.md#output) modes. Nothing of an
alert is drawn until the end of its first line shows it was one.

An alert ends where its quote does: at a blank line or at any line that is not a
row of it, a bare `>` among them, and so does a table whose first line opens
with `|`, a heading written with an underline or a display formula. A quote row
after any of them starts a quote of its own, except one straight after a table
whose first line does not open with `|`, such as `a | b` over `- | -` or over
`| - | - |`: that table takes it as one more of its rows.

A quote naming any other kind, with anything after the marker on its line, or
with the marker on a later line, is an ordinary quote and shows the marker as
written. So does a marker that is not in a quote, and one on the bullet's own
line of a list item, as in `- > [!NOTE]`.

## Footnotes

A footnote reference such as `[^1]` or `[^note]` is drawn as a number: in
superscript, `¹`, in the unicode glyph set, and as `[1]` in the quiet grey in
the ASCII one. Numbers count up from 1 in the order the answer first writes each
label, whatever the label says, and a label is the same label in any letter
case: `[^Note]` and `[^note]` are one footnote. A line that opens with the
brackets and a colon, after nothing but spaces, as in `[^1]: the source`, is
that footnote's note, and is drawn quiet behind its number and one space,
however many spaces follow the colon: `¹ the source`. A note written
before or after the reference that uses it takes the same number. In a quote
or a list item, the brackets are a reference even where a colon follows them.

## Tags

Six HTML tags are read instead of drawn, in any letter case and with or
without attributes, so long as those hold `<` or `>` only inside a quoted
value, as in `<kbd title="a>b">`:

| Written | Unicode | ASCII |
| --- | --- | --- |
| `<kbd>Ctrl</kbd>` | `Ctrl` | `[Ctrl]` |
| `H<sub>2</sub>O`, `x<sup>2</sup>` | `H₂O`, `x²` | `H₂O`, `x²` |
| `a<br>b`, `<br/>`, `<br />` | `b` on the next row | the same |
| `<summary>Why</summary>` | `▸ Why` | `Details: Why` |
| `<details>` … `</details>` | the prose between, two columns in on every row | the same |

A key is drawn in the colour of code, and what it holds is drawn as written:
`<kbd>&amp;</kbd>` shows `&amp;`. A `<sub>` or `<sup>` is drawn in the quiet
grey with Unicode's subscript or superscript characters, which exist for the
digits, `+ - = ( )` and a few letters, in both glyph sets. When any character
between the tags has none, as in `x<sup>2a</sup>`, the text is drawn plain and
the tags are dropped; so it is when the text runs past 64 characters or its
closing tag is not on its line. A summary's words are bold behind the mark. A
paragraph inside `<details>` is drawn two columns in, on every row it wraps
onto, until the first `</details>` or the end of the answer; a list, quote,
heading, table, code block or display formula inside it is drawn where it
would be without it. A line of nothing but `</details>` ends a table written
without outer bars inside the block, and is never one of its rows.

A line written at the left edge that holds nothing but `<details>` and closing
tags, such as `</details>` or `</summary>`, draws no row. A line
that `<br>`, `<details>` or `<summary>` is read on is never the first line of a
table written without outer bars, nor the title of a heading underlined with
`===` or `---`.

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
- **A table without its outer bars.** A line such as `name | since` over a
  delimiter row such as `--|--` with the same number of cells is a table. Its
  rows are the lines after them up to the first blank line, or inside
  `<details>` a line of nothing but `</details>`, so a table written this way
  needs a blank line before whatever follows it. A delimiter row with
  another number of cells, a lone `---`, or a line that only looks like a
  delimiter row in the middle of prose leaves both lines as prose. Nothing
  marks the first line as a header until the delimiter row has arrived, so
  every line is shown as it arrives, as the prose it may be, and is replaced by
  the table once the delimiter row has. Only the line break after a line, and
  the start of the line under it, wait for what follows; a header longer than
  128 bytes is prose. Output that is not a terminal keeps
  every table, like the rest of the model's markdown, as it was written.
- **Lines with bars and no delimiter row.** They are drawn as written.
- **A `$` in prose.** A price such as `$5`, or `$$` after other words on a
  line, opens no formula.
- **Footnotes and tags where they are not read.** In code, and from a `[` to
  the next `]` or the end of its line, as in a link's words, footnotes and the
  six tags are shown as written. In a heading, a
  table's cell and a block drawn as written, such as display math, a footnote,
  `<details>` and `<summary>` are shown as written and `<br>` is a space.
- **A footnote crucible will not number.** An empty label, `[^]`; one holding
  a space or a control character; one longer than 32 characters; a second note
  for a label that already has one; and a label first written after 64 others.
- **Any other tag.** A tag that is not one of the six, such as `<article>` or
  `<b>`, is shown as written, and so is one of the six whose `>` does not come
  on its line or within 64 characters of its `<`, as when a quoted value is
  left open.

A table with more columns than the window can hold is still drawn as a table,
with each cell folded into the room its column has, however narrow.

## Pictures and image files

The transcript does not yet put a picture on the screen as pixels, whatever
`output.images` says; which protocol a session would draw one with, and on
which terminals, is under [Where pictures are
drawn](../configuration/configuration.md#where-pictures-are-drawn).

A picture the transcript turns into pixels, whether an SVG diagram or a PNG,
JPEG, GIF or WebP file, is bounded before it is read, and a picture past any
bound is refused rather than shrunk or cut:

- **The file or the SVG.** At most 1 MiB. Past that it is refused unread.
- **The pixels.** At most 2 MiB of them, at four bytes a pixel, so a picture of
  about 724 by 724 pixels, or 1 024 by 512. The size a file or an SVG says it
  has is judged before any pixel is made, so a small file that claims a huge
  picture is refused at once.
- **What drawing an SVG holds at once.** The same 2 MiB bounds the sum of
  every pixmap the drawing would hold together: the picture itself, each
  group drawn apart from its parent (for opacity, a filter, a clip or a
  blend), the result of each filter step, and each clip.
  The sum is worked out from the SVG before anything is drawn, so a small file
  whose filters ask for far more room than the picture is refused at once.
- **What drawing an SVG computes.** The pixels each filter step's region
  holds, times what the step does with each pixel (a turbulence per octave, a
  morphology per cell of its window, a convolution per cell of its matrix, a
  merge per input it draws), and for each fill or stroke, a shape inside a
  clip among them, the largest of: twenty steps, however small it is or far
  off the picture; one step for every sixteen pixels it covers inside the
  picture, or for a dashed stroke at least eight for each dash it is cut
  into, however short and thin; the time its outline takes to build and to
  sweep down the rows it crosses, longer for a stroke than for a fill; for a
  fill whose edges crowd together along a row, how far along the row each
  can walk; and for a gradient, its pixels times its stops, with every stop
  set up again for each shape it paints. Added up over the picture, these
  are at most 140 000 steps. That is about what a drawing can finish in a
  fiftieth of a second. A picture past it is refused before it is drawn, and
  so is one with a stroke reaching more than a million pixels from the
  corner of the picture, which takes far longer to outline than its size
  says.
- **What the copies in an SVG take.** A `<use>` copies what it names, and a
  copy can hold uses of its own, so a few hundred bytes can ask for millions
  of copies. A marker's content is copied at every vertex it is drawn on, a
  clip or a filter sized to the shape it is on is made again for each shape,
  a gradient sized to the shape it paints has its stops copied for each
  shape, a text copied where it is not drawn still has its letters laid out,
  and each colour the picture is drawn in is matched against every copy.
  Every shape drawn, once or in each copy, has its outline built: a circle,
  an ellipse, a rounded rectangle or an arc is cut into more curves the
  larger it is, and a stroked shape has the outline of its stroke worked out,
  which takes longer the larger its curves are. A stroked shape that is
  turned or skewed, by its own transform or by what holds, copies or marks
  it, has that outline worked out again at its size on the page. The time
  making them all would take, by the elements, attributes, letters and
  outlines made, is counted before the picture is built, and copies that
  would take more than 16 ms are refused.
- **A picture sent to the terminal.** Sent as PNG it is at most 1 MiB, and
  sent as Sixel it is at most 600 pixels wide.
- **One at a time.** Only one picture is read or drawn at a time, wherever it
  was asked for. One that waits longer than ten seconds for its turn is
  refused as late.

An SVG is drawn from its own bytes and nothing else, and only with the
elements a picture is drawn with: `svg`, `g`, `defs`, `symbol` and `use`;
`path`, `rect`, `circle`, `ellipse`, `line`, `polyline` and `polygon`; `text`
and `tspan`; `marker`; `linearGradient`, `radialGradient` and `stop`;
`clipPath`; `filter` and its steps; and `title`, `desc` and `metadata`. A
picture holding any other element, such as a `<style>` sheet, a `<mask>`, a
`<pattern>`, an `<image>`, a `<foreignObject>`, a `<switch>`, a link, a
`<tref>` or a `<textPath>`, or an element in a namespace other than SVG's, is
refused. So is a reference anywhere but on a `<use>` naming an element of the
picture or on a filter's image step naming something outside it: a gradient
that takes its stops from another, or an image step that draws an element,
is refused. So is an id given to two elements, and a number of the picture's
geometry, such as a length, a coordinate, a radius or a factor of a
transform, larger than 100 000, and a `width`, `height`, `x` or `y` given in
`%`, `em` or `ex` on an `svg` inside the picture, a `symbol` or a `use`: each
sizes the view inside it from the one around it, so a chain of them grows
without bound. The outermost `svg` may still give its size in any unit, and
a percentage inside the picture is taken of the largest view that size, a
view box, or a `use` drawing a `symbol` gives. A
file, a URL or a font it names is not opened, followed or fetched: a filter's
image step naming one is drawn as if the reference were not there. Only
one font is available, DejaVu Sans, so text in a script it has no letters for
is drawn with gaps. A filter's image step holding a
raster image, such as a PNG in a `data:` URL, is drawn as nothing; one
holding an SVG refuses the picture. A picture whose references lead round to
themselves, such as three clips that each use the next, is refused, as is a
marker drawn inside a marker, a clip or a filter inside a clip, and one the
drawing cannot finish.
