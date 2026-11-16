// The answers a model writes, kept whole, as the corpus every drawing of an
// answer is compared with.
//
// One answer per construct or group of constructs this program draws, each
// written the way a model writes one rather than as a list of fixtures: prose
// around the table, a sentence before the fence, a closing line that says the
// answer ended. The screen suite streams the same text through the real binary,
// and the pictures beside this module lay it out through `dump.rs`; one source
// for both is what keeps a capture and its picture showing the same answer.
//
// The last line of every answer is [`END`], so a case watching a terminal has
// one thing to wait for that no row already on screen says.
//
// Test-only, and reached from outside the crate through `crucible-tui`'s
// `testing` feature once a drawer crate needs it.

/// The last line of every answer here.
///
/// Short, so it fits a row at the narrowest width a case draws at, and worded
/// so nothing already on the screen — the opening card, the box, a status row —
/// says it. A reader never sees it as a claim about the corpus: it is the line
/// that tells a case the answer has arrived whole.
pub(crate) const END: &str = "The corpus ends here.";

/// Every answer in the corpus, under the name a case asks for it by.
pub(crate) const ANSWERS: &[(&str, &str)] = &[
    ("tables", TABLES),
    ("rules", RULES),
    ("lists", LISTS),
    ("quotes", QUOTES),
    ("code", CODE),
    ("headings", HEADINGS),
    ("alerts", ALERTS),
    ("images", IMAGES),
    ("notes", NOTES),
    ("links", LINKS),
    ("inline", INLINE),
    ("math", MATH),
    ("mermaid_graphs", MERMAID_GRAPHS),
    ("mermaid_timelines", MERMAID_TIMELINES),
    ("diff", DIFF),
    ("hostile", HOSTILE),
    ("tool_edit", TOOL_EDIT),
];

/// The answer kept under `name`, or `None` where the corpus has none.
pub(crate) fn answer(name: &str) -> Option<&'static str> {
    ANSWERS
        .iter()
        .find(|(one, _)| *one == name)
        .map(|(_, text)| *text)
}

/// A table that fits, one written without outer bars, and one far too wide.
pub(crate) const TABLES: &str = r"Here is what the release carries:

| name | what it does | since |
| --- | --- | --- |
| alpha | reads the markdown out of a stream | 0.1 |
| beta | lays a table against the window | 0.2 |
| gamma | keeps the source of a drawn block | 0.3 |

A table may be written without its outer bars:

name | what it does | since
--|--|--
delta | draws a rule across the window | 0.4
epsilon | folds a long cell | 0.5

And one with more columns than a window can hold:

| region | owner | window | rows | scrollback | theme | glyphs | notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| north | the drawing thread | eighty columns of prose that has to wrap somewhere | twenty-four | two rows | dark | unicode | a note long enough to be split across rows again |
| south | the reading thread | one hundred and twenty columns of prose that also wraps | forty | none | light | ascii | another note, just as long, so the split is not a special case |

The corpus ends here.
";

/// Breaks written three ways.
pub(crate) const RULES: &str = r"A rule parts what is above it from what is below.

---

The rule above is three hyphens in the source.

***

The rule above is three stars in the source.

___

And this one is three underscores, which is also a rule.

The corpus ends here.
";

/// Wrapped bullets, numbered items, task items and a nested item.
pub(crate) const LISTS: &str = r"Things still to do:

- a bullet item long enough that it has to wrap onto another row of the window
- [ ] a task nobody has finished yet
- [x] a task somebody has finished

1. a numbered item long enough that it wraps as well
2. two
   - a nested bullet
3. three

The corpus ends here.
";

/// A quote that wraps, and a quote with two paragraphs.
pub(crate) const QUOTES: &str = r"The reply quoted somebody else:

> a quote long enough that it has to wrap onto another row, and then onto one more
>
> and a second paragraph inside the same quote

The corpus ends here.
";

/// A fenced block that names a language, one that names nothing, and a block
/// indented four spaces.
pub(crate) const CODE: &str = r"A fence that names its language:

```rust
fn main() {
    let value = compute_something_long(argument_one, argument_two);
}
```

A fence that names nothing:

```
plain block
```

And a block indented four spaces, after a blank line:

    let x = 1;
    let y = 2;

The corpus ends here.
";

/// ATX headings at every level, and a setext heading.
pub(crate) const HEADINGS: &str = r"# One
## Two
### Three
#### Four
##### Five
###### Six

A heading may also be underlined instead:

Title
=====

Sub
-----

The corpus ends here.
";

/// Every alert kind.
pub(crate) const ALERTS: &str = r"> [!NOTE]
> Useful information.

> [!TIP]
> A shortcut worth taking.

> [!IMPORTANT]
> Something that must not be missed.

> [!WARNING]
> Something that could go wrong.

> [!CAUTION]
> Something that could go wrong badly.

The corpus ends here.
";

/// An image in the checkout, and an image on the network.
pub(crate) const IMAGES: &str = r"A picture from the checkout:

![the build passing](docs/images/build.png)

A picture from the network:

![a diagram of the flow](https://example.com/flow.png)

The corpus ends here.
";

/// Footnotes and the HTML subset, with one tag that is not in it.
pub(crate) const NOTES: &str = r"A claim with a source.[^1] Another claim, defined later.[^2]

<details><summary>More</summary>
hidden
</details>

A line broken<br>inside it, a key <kbd>Ctrl</kbd>, and H<sub>2</sub>O with x<sup>2</sup>.

An <article> tag is not in the subset.

[^1]: the first source
[^2]: the second source, written after it was used

The corpus ends here.
";

/// Reference links defined before, after and never, angle and bare addresses,
/// an ordinary link, and forge addresses.
pub(crate) const LINKS: &str = r"[docs]: https://example.com/docs

A reference to [the documentation][docs] defined before it.

A reference to [the changelog][notes] defined after it.

A reference to [nothing][missing] that is defined never.

An address in brackets, <https://example.com/a>, and one without, www.example.com.

An ordinary link to [the issue list](https://example.com/issues).

Forge references: https://github.com/augments-labs/crucible-code/pull/1324, https://github.com/augments-labs/crucible-code/issues/1288 and https://github.com/augments-labs/crucible-code/commit/0a1b2c3d4e5f60718293a4b5c6d7e8f901234567.

[notes]: https://example.com/changelog

The corpus ends here.
";

/// A backslash break, entities, single and double tildes, and nested emphasis.
pub(crate) const INLINE: &str = r"A line that ends in a backslash\
continues on the next row.

An ampersand written as &amp;, a tag as &lt;tag&gt;, a copy as &copy; and a number as &#169;.

An empty reference &#0; and an override &#x202E; are written as they came.

~one~ is struck under GitHub's rules, and ~~two~~ is struck under both.

A pair may cross a line break: ~the phrase goes
on~ here.

And these strike nothing: edit ~/.config/crucible/config.json then ~/bin/run, between ~/a and ~/b, about ~5 minutes or ~10.

Nested emphasis: ***both***, **bold _italic_ bold**, and *leant **and raised** again*.

The corpus ends here.
";

/// Inline math in both spellings, the dollar lines that are not math, display
/// math in each spelling, and a math fence.
pub(crate) const MATH: &str = r"Inline math may be written \( \alpha_i \) or as $E = mc^2$ in the middle of a sentence.

The dollar fixture lines stay exactly as written:

It costs $5 and $10.

$5/$10 per seat

$20,000-$30,000 a year

Set $HOME before running.

Run $browser then $deploy on it.

echo $PATH and $1

copy it to $HOME/$USER first

set $a,$b and $c

A display formula:

$$
\sum_{i=1}^{n} i = \frac{n(n+1)}{2}
$$

A bracketed display:

\[
\int_0^1 x^2 \, dx = \frac{1}{3}
\]

An aligned one:

\begin{align}
a &= b + c \\
d &= e + f
\end{align}

A math fence:

```math
x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}
```

The corpus ends here.
";

/// A Mermaid fence of each graph kind this program draws as text.
pub(crate) const MERMAID_GRAPHS: &str = r"A flow chart:

```mermaid
graph TD
  A[Start] --> B{Ok?}
  B -->|yes| C[Done]
  B -->|no| A
```

A state diagram:

```mermaid
stateDiagram
  [*] --> Idle
```

And the versioned spelling:

```mermaid
stateDiagram-v2
  [*] --> Idle
  Idle --> Running: start
  Running --> [*]
```

A class diagram:

```mermaid
classDiagram
  class Reader
  Reader : +read(delta)
```

An entity relationship diagram:

```mermaid
erDiagram
  ANSWER ||--o{ BLOCK : draws
```

The corpus ends here.
";

/// A Mermaid fence of each kind that reads as a run of events in time, and one
/// kind this program has no drawing for.
pub(crate) const MERMAID_TIMELINES: &str = r#"A sequence diagram:

```mermaid
sequenceDiagram
  Alice->>Bob: Hello
  Bob-->>Alice: Hi
```

A gantt chart:

```mermaid
gantt
  title Release
  section Tests
  Gate :a1, 2026-11-01, 3d
```

A git graph:

```mermaid
gitGraph
  commit id: "one"
  branch feature
  commit id: "two"
  checkout main
  merge feature
```

A kind with no drawing of its own:

```mermaid
pie
  "drawn" : 1
```

The corpus ends here.
"#;

/// A well-formed diff fence, a patch fence, and a diff fence that is not a
/// unified diff at all.
pub(crate) const DIFF: &str = r"A change written as a diff fence:

```diff
--- a/x.rs
+++ b/x.rs
@@ -1,2 +1,2 @@
-let a = 1;
+let a = 2;
 context
```

The same change written as a patch:

```patch
--- a/y.rs
+++ b/y.rs
@@ -1 +1 @@
-old
+new
```

A fence named diff that is not a diff at all:

```diff
this is not a unified diff
it has no hunks
```

The corpus ends here.
";

/// What a model can write that a terminal reads as an instruction: an image
/// placeholder character, an operating-system command, and a bidi override.
pub(crate) const HOSTILE: &str = "A label a model wrote: \u{10EEEE}\u{0305}\u{0305} and a second \u{10EEEE}.\n\
\n\
A terminal instruction a model wrote: \u{1b}]0;a title it chose\u{7} after it.\n\
\n\
A right-to-left override a model wrote: \u{202E}txet a\u{202C} after it.\n\
\n\
The corpus ends here.\n";

/// What a model says once its call has rewritten a whole file.
pub(crate) const TOOL_EDIT: &str = r"The whole file is rewritten now, every line of it.

Nothing else in the workspace moved, so the change in the call above is the whole of it.

The corpus ends here.
";
