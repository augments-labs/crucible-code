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
| Table | a line starting with `\|`, followed by a delimiter row such as `\| --- \| ---: \|`; or a line of two or more cells separated by `\|` with no outer bars, followed by a delimiter row such as `--\|--` with as many cells | the first line that does not start with `\|`; or, for one without outer bars, the first blank line, or inside `<details>` a line of nothing but `</details>` written at the left edge |
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
first, after any cut results, and then every
[address drawn shorter](#the-address-after-a-links-words): see
[Results cut short](../reference/keys.md#results-cut-short).

## Drawn again at every width

A table, a diff, a rule and the rule under a heading are kept as the text the
model wrote, beside the rows they were drawn as. When the window changes width,
or you change the glyph set, each is laid out again from that text for the
window it is in now, so a table drawn in a narrow window and then widened is
the table a window that wide would have drawn, and a diff's grounds reach the
new edge. A change of theme repaints the same rows
in the new colours. In native mode this happens when the terminal's scrollback
is written again, once the window has kept its new size.

What the transcript keeps is bounded, and a table or a diff counts against that
bound by both its text and its rows. When the oldest of the transcript is
dropped, a table or a diff goes with its text. The [session log](sessions.md) still holds every
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

## Diffs

A fence named `diff` or `patch` that holds a unified diff is drawn as the
change it describes, in the box a tool's edit is shown in:

```text
╭─ src/parse.rs · Added 1 line, removed 1 line
│   40 -     tree
│   40 +     tree.finish()
├─ README.md · Added 1 line
│    1   # parse
│    2 + A small parser.
╰─
```

Each file of the diff opens with a row naming it and counting its lines, and
everything said of the box under
[What a change leaves on screen](../tools/files.md#what-a-change-leaves-on-screen)
holds here: the grounds, the raised words, the colours of the file's language
and the fold. A file of more than 32 rows is folded around a row such as
`⋯  36 lines folded · ctrl+o to expand  ⋯`, and <kbd>Ctrl+O</kbd> lists the
diff as the model wrote it, every line of it: see
[The source of a table, formula, diagram or diff](#the-source-of-a-table-formula-diagram-or-diff).
Where no key is read, in output sent to a file or a pipe and in the `/resume`
preview of a session, the row keeps its count and names no key:
`⋯  36 lines folded  ⋯`.

A fence under either name that is not a unified diff is shown as written, in
the frame of a [code block](#code-blocks). So is a diff past what a change
holds, one that names more than 8 files or runs to 2048 lines or more, and its
top edge says why: `┌─ diff · longer than this can hold · shown as written`.
A fence is [held](#what-is-shown-as-written) for 16 KiB at most, so a diff
longer than that is shown the same way.

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

## Reference links and addresses

A line that is a label in brackets, a colon and one address, such as
`[docs]: https://example.com/docs`, defines that label, and is drawn as the
address, a link. A reference written after it, `[the docs][docs]`,
`[the docs][]` or `[docs]`, is drawn as its words, a link to that address. A
label is the same label in any letter case, as a footnote's is: `[Docs]` and
`[docs]` are one. A label defined twice keeps its first address.

Only a label defined above the reference ties up. A definition written after
the reference, or none, leaves the brackets as written, so `[TODO]` and `arr[0]`
stay as they are. An answer keeps 64 definitions, and 16 KiB of their labels
and addresses; the 65th definition, or the one that would pass the 16 KiB,
links nothing and is shown as written. That is far more than an answer defines,
and a line is held for at most 4 KiB while it may be one. A definition with a
title after its address, an address in angle brackets, or an address that is not
spelled with a scheme such as `https://` is not read: the line is drawn as
written. References and definitions are read where footnotes are, and shown as
written in a heading, a table's cell and a block drawn as written.

An address in angle brackets, `<https://example.com/a>`, `<http://example.com/a>`
or `<name@example.com>`, is drawn without its brackets, a link to the address
without them; an e-mail address links to `mailto:name@example.com`. One that
holds a space or a control character, runs past 4 KiB, or stands in code, a key
or a `<sub>` or `<sup>` run is shown as written. A word that begins `www.`, at
the start of a line or after a space, an opening bracket or the mark of
emphasis, is a link to the `https://` address that begins with it:
`www.example.com` points to `https://www.example.com`. The address of every
link is written as safe characters only, whatever a model put in it.

## Pull requests, issues and commits

An `https://` address on `github.com` or `gitlab.com`, or on the host of the
repository you are in, that names one pull request, merge request, issue or
commit is drawn as the reference people call it by, still a link to the whole
address:

| Address | In the repository you are in | Anywhere else |
| --- | --- | --- |
| `…/owner/repo/pull/1324` | `#1324` | `owner/repo#1324` |
| `…/owner/repo/issues/1324` | `#1324` | `owner/repo#1324` |
| `…/group/repo/-/merge_requests/12` | `!12` | `group/repo!12` |
| `…/owner/repo/commit/58ed227aa01` | `58ed227` | `owner/repo@58ed227` |

A number is at most 12 digits and a hash is 7 to 40 hexadecimal digits, of
which the first 7 are drawn. On GitHub the repository is an owner and a name;
on GitLab it may sit in subgroups, as in `group/sub/repo!3`. One trailing `/`
is allowed. An address with anything after the item, such as a tab of the
page, a query or an anchor, any other shape, or another host is an ordinary
link. The same repository on another host is another repository, and is
named in full.

## The address after a link's words

A link whose words are not its address is followed by a space and its quiet
address in brackets, in the quiet grey: `[the guide](https://example.com/docs)`
is drawn `the guide (example.com/docs)`. The address loses its scheme, or
`mailto:`. One with a host, a `www.` address among them, keeps its host and
first segment, and where anything follows them, a further segment, a query or
an anchor, it ends in an ellipsis, `…`, or `...` in the ASCII glyph set:
`[the guide](https://example.com/docs/guide)` is drawn
`the guide (example.com/docs/…)`, and a query straight after the host is drawn
`(example.com…)`. A path or an e-mail address is cut once it is wider than 24
columns and keeps what fits. Nothing before the ellipsis is wider than 24
columns, and a wide character that would cross that bound is left out. Words
that link to a pull request, an issue or a commit are followed by its
reference instead, as in `the fix (#1324)`, and words that are its reference
already, such as `#1324`, have nothing after them. The quiet address is not a
link, and a drag in the full screen copies the words without it.

An image, `![the build passing](https://example.com/build.png)`, is drawn as
the link `[image: the build passing]`, or `[image]` with no words, followed by
its quiet address; nothing is fetched. A `!` before any other bracket is drawn
as itself, and so is one after a backslash, which stays in front of it:
`\![x](https://example.com/y.png)` is drawn `\!x (example.com/y.png)`. An
address written as its own words, bare or between angle brackets, has nothing
after it. In a table's cell a link
is drawn as it is in prose, an address as its reference included, but with
nothing after its words: the whole of it is in the table's source, and
<kbd>Ctrl+O</kbd> lists it whenever it would list the same link in prose.

A link in code, a key, a fence or a mermaid diagram is drawn as the model wrote
it, one in a line of a diff is that line's text and links nothing, and one in
display math is drawn as its words alone, as before. Every
link and image in output with no colour, including output that is not a
terminal, is written as the model wrote it.

<kbd>Ctrl+O</kbd> lists the whole of every address drawn shorter than it was
written, a reference or a quiet address cut with an ellipsis, newest first, each
under the word `address`, after the cut results and the drawn blocks. A link in
a table's cell is listed whenever the same link in prose would be. It
keeps the newest 64; an older one is still in the
[session log](sessions.md) as the answer wrote it. A bare number such as `#7`
is not among them, since it was written that short.

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
would be without it. A line of nothing but `</details>`, written at the left
edge, ends a table written without outer bars inside the block, and is never
one of its rows.

A line written at the left edge that holds nothing but `<details>` and closing
tags, such as `</details>` or `</summary>`, draws no row. A line
that `<br>`, `<details>` or `<summary>` is read on is never the first line of a
table written without outer bars, nor the title of a heading underlined with
`===` or `---`.

## Line breaks and entities

- **A backslash at the end of a line.** In a paragraph, a list item or a quote,
  a `\` just before the line break breaks the line and is not drawn, once more
  of the same paragraph follows. When the message ends there, a blank line comes
  next, or the next line opens a block, the backslash is drawn as written and
  the block opens as it would have. The lines that open a block are those led by
  a heading mark, a bullet (`-`, `+` or `*`) or a quote mark and then a space; a
  number with a `.` or `)` and then a space, a tab or the line's end; a fence; a
  rule of three or more marks; `$$` or `\[` display math; `<details>` or
  `<summary>`; and a row of bars with a delimiter row under it, which is a
  table. A `-` or `=` under a line, and a delimiter row under a header that its
  line break ends, make that line a heading or a table header, with the
  backslash in it. A line that only starts like one is more of the paragraph,
  and the backslash breaks it: a marker after a tab or with no space after it, a
  number such as `1.5`, a row of bars with no delimiter row under it, a
  delimiter row under a header when the answer ends there, which opens no table,
  and a `-` under a list item. In code, a code span, a heading, display math and
  a block drawn as written it is an ordinary character.
- **Entities.** Outside code, `&name;` for the names below and every numeric
  reference, `&#169;` or `&#xA9;` with up to seven digits, is drawn as the
  character it names. What it names is text, never read again: `&lt;details&gt;`
  opens nothing and `&#42;` leans on nothing. The names are `amp`, `lt`, `gt`,
  `quot`, `apos`, `nbsp`, `copy`, `reg`, `trade`, `hellip`, `mdash`, `ndash`,
  `lsquo`, `rsquo`, `ldquo`, `rdquo`, `laquo`, `raquo`, `bull`, `middot`, `deg`,
  `plusmn`, `times`, `divide`, `micro`, `para`, `sect`, `euro`, `pound`, `yen`,
  `cent`, `larr`, `rarr`, `uarr`, `darr`, `harr`, `rArr`, `lArr`, `hArr`, `ne`,
  `le`, `ge`, `infin` and `check`, and case matters. Any other name, an entity
  without its `;`, and one inside code, a key, display math or a block drawn as
  written are drawn as written. Inside `<sub>` and `<sup>` it is decoded and
  then drawn in the script, as far as the script has the character.
- **An entity for a character a screen does not draw as itself.** A control
  character, an escape, a bidirectional override or other format character,
  zero, a surrogate and a value past the last character are drawn as written,
  `&#27;[31m` and `&#x202E;` among them. No entity puts an escape on the
  terminal.

## Struck phrases and weight with emphasis

- **One tilde on each side.** A phrase between two single tildes is struck
  through, as one between two pairs is. A tilde opens a phrase when the
  character after it is not a space, and is either a letter or a digit or comes
  after a space, a punctuation mark or the start of the line. It closes the
  phrase opened last and still open when the character before it is not a
  space, and is either a letter or a digit or comes before a space, a
  punctuation mark or the end of the line. Inside a word counts too. So `~one~`
  strikes `one`, `takes 2~3 seconds, then 5~10` strikes `3 seconds, then 5`,
  and `see ~/a~ here` strikes `/a`. A phrase may cross a line break inside one
  paragraph. Two tildes and three or more read as they did before.
- **A tilde with no closer is drawn as written.** `~/Projects`, `between ~/a
  and ~/b` and `about ~5 minutes or ~10` keep every tilde. So does a tilde
  whose only closer comes after the paragraph ends, or once 4 KiB of the answer
  as written (the reader's `STRIKE_CAP`) has followed it, counted in bytes: a
  phrase strikes only when what is written between its tildes is shorter than
  that. When more than one tilde waits, the count runs from the oldest. A
  paragraph ends at a blank line, a line that opens a block of its own, a
  numbered item, a table, a block held until it closes, a line that turns out
  to be a table's header or a heading's title, and the end of the message.
- **Held until it settles.** What follows an opener is not drawn until its
  closer arrives or one of the limits above is reached. Meanwhile the foot of
  the transcript shows the last line of what is held, as it was written and
  cut to the window's width, with any control character left out. It shows in
  both [`output.screen`](../configuration/configuration.md#output) modes, is
  never kept in the transcript or your terminal's scrollback, and gives way to
  the phrase, struck or as written, once it settles.
- **Where a tilde is never a marker.** In code, a key, a subscript or
  superscript, a link's words and address, a bare address, after a backslash
  (`\~`), and in a block drawn as written, a single tilde is a character.
- **What a struck phrase looks like.** The strike is drawn over weight,
  emphasis and a heading's look, and code, a link, a quote and a done task keep
  theirs.
- **Weight with emphasis.** A run that is both bold and italic, as in
  `***both***`, the italic words of `**bold _italic_ bold**` or the bold words
  of `*leant **and raised** again*`, is drawn bold and italic together, in
  every theme and with either glyph set. Code and a strike are still drawn over
  it.

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
- **Display math and mermaid diagrams.** They are held so they arrive whole,
  and are then shown as written.
- **A diff crucible will not draw as a change.** A fence named `diff` or
  `patch` that is not a unified diff, and one past what a change holds: see
  [Diffs](#diffs).
- **A table without its outer bars.** A line such as `name | since` over a
  delimiter row such as `--|--` with the same number of cells is a table. Its
  rows are the lines after them up to the first blank line, or inside
  `<details>` a line of nothing but `</details>` written at the left edge, so
  a table written this way needs a blank line before whatever follows it. A
  delimiter row with another number of cells, a lone `---`, or a line that
  only looks like a delimiter row in the middle of prose leaves both lines as
  prose. Nothing
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
- **A reference or definition crucible will not tie up.** A label with no
  definition above it, a definition past the 64th or past 16 KiB, and a
  definition line that holds more than its address.
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
