//! Markdown, read as an answer streams.
//!
//! A model answers in markdown whether or not anything asked it to, so the
//! choice is between showing the markers and reading them. This reads them: a
//! marker is recognised, dropped, and the run it covered is handed on wearing a
//! [`Slot`] instead. What it does *not* do is turn the model's text into escape
//! sequences — the run and its slot arrive separately and the slot is applied as
//! the row is drawn, which is what keeps the rule that no byte from a model is
//! ever written through as an instruction.
//!
//! The scan is one character at a time and its state outlives a delta, because a
//! delta is a piece of the wire rather than a piece of the answer: a marker
//! arrives split across two of them as often as not. Nothing looks ahead past
//! the single character that ends a run of markers — an answer that stopped
//! mid-sentence has already been drawn up to where it stopped.
//!
//! Two runs are held back, and both are bounded and both are given up on. A
//! line inside a fence is held because a highlighter reads whole lines, and a
//! link's words are held because nothing says they were a link's words until
//! the `](` after them. Either one that runs past its bound, or past the line
//! it is on, is handed out as the text it turned out to be.
//!
//! A table written without outer bars, and a heading written as a line over a
//! row of `=` or `-`, are prose until the line under them arrives, and prose
//! that is neither is not made to wait for it. Every paragraph line is read
//! as it arrives, as the prose it is so far, while a copy of it is watched,
//! from its first character, for the shape of a header or a title. Its break
//! is held back once it is a whole line, together with the start of the next
//! line for as long as that could be a delimiter row or an underline; both are
//! handed out as they were written the moment they are not. A delimiter row
//! that is one takes the line back ([`Said::Retract`]), and the table is handed
//! on as a block like any other. An underline takes the line back the same
//! way, and says it again as the heading it is, with the rule under it as a
//! block; the underline itself is read and never said. A heading written with
//! `#` is never taken back, since its first characters say what it is: it
//! streams, and its rule follows once its line is whole.
//!
//! Whole blocks are held too, and handed on as a [`Block`] rather than as
//! runs, in order with the runs around them: a table, display math, a mermaid
//! fence and a diff fence ([`Held`]) are gathered until they close, and a rule
//! is one the moment its line ends. A block keeps the source it was written
//! as, so whoever keeps it can lay it out again at another width or in
//! another glyph set with `lay`, where runs once written stay as they were
//! folded. Each hold has a cap of its own, and a block past it is handed on as
//! the runs it would have been had nothing held it. A reader made
//! [`Markdown::as_written`] holds no block at all, which is what lays one out.
//!
//! A fence is drawn in a frame of [`chrome`]'s, and the reader says it as it
//! streams: the fence's opening line becomes the top edge naming its language,
//! every line of code starts with the gutter as a run of its own, and the
//! closing line becomes the bottom edge. A fence a message ends inside is
//! never given a bottom. The name is the first word of the info string, cleaned
//! here once because this is where a model's text is read, and a held fence
//! past its cap says so in its top edge. `framed` reads such a row back by its
//! gutter, and breaks it where the room runs out rather than at a space.
//!
//! A block indented four spaces after a blank line is code as well, outside an
//! item of a list, and is drawn in the same frame with no name in its top
//! edge. The spaces a line that could be one of its lines opens with are
//! counted rather than read, until the first thing that is not a space says
//! whether they were: four or more open or carry on the block, the rest past
//! the fourth being the code's own, and fewer read again as the line they
//! began. A blank line inside the block is counted too, since only the line
//! after it says whether it was the block's or stood after it; the first line
//! that is not blank and is indented less shuts the block with its bottom
//! edge, and the end of the message shuts it as well.
//!
//! A quote's bar and an item's mark are drawn in slots of their own, and
//! `hang` reads them back off a line to say what a row that line wraps onto
//! stands in front of its words with. The reader decides where a line ends and
//! nothing about where it wraps; the record that folds it asks.
//!
//! A quote whose first line is nothing but `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`,
//! `[!WARNING]` or `[!CAUTION]`, in any letter case, is an alert: each row of
//! it, a `> ` that opens the line or follows spaces, is drawn behind a bar of
//! its own, and the first holds the kind's mark and its word in place of the
//! marker. It ends at the first line that is not such a row, a bare `>` among
//! them, and at a table whose first line opens with `|`, a heading's underline
//! or a display formula, which take their lines whole; a quote row after any of
//! them is a quote of its own, but one straight after a table whose first line
//! does not open with `|`, such as `a | b` over `- | -` or over `| - | - |`, is
//! one more row of that table. What a first line is cannot be told until it
//! ends, so the bar of a quote's first line is owed like a bullet's mark and
//! nothing of a marker is drawn before its break.
//! Every other quote, including one naming a kind that is not one of the five,
//! is drawn as it always was.
//!
//! A link's brackets holding a caret and a label, `[^1]` or `[^note]`, are a
//! footnote, numbered in the order the answer first writes each label, in any
//! letter case: a reference is drawn as its number in superscript, or under
//! ascii as a quiet `[1]`. Where the brackets are the first thing a line says
//! and a colon follows them, they are the note's definition, and the line is
//! drawn quiet behind the number and one space, however many followed the
//! colon. A label is remembered until the message ends, so a definition takes
//! its reference's number wherever either stands. The
//! brackets are shown as written in a heading, a table's cell, code, from a
//! `[` to the next `]` or the line's end, and a block laid out again; for a
//! label that is empty, longer than its cap, or holds whitespace or a control
//! character; for a second definition of one label; and for a label first
//! written past the cap.
//!
//! A line that is a label in brackets, a colon and one address, a scheme and
//! what follows it and nothing else but blanks, defines that label. It is
//! drawn as the address, a link, and a reference written after it, `[words]`,
//! `[words][label]` or `[words][]`, is drawn as its words, a link to that
//! address; a label compares in any letter case, and the first definition of
//! one is the one kept. A definition written after its reference, or none, ties
//! nothing up: the brackets are shown as written, as `[TODO]` and `arr[0]` are.
//! The definition line is held to its end, as a link's words are, and is read
//! as the line it is where it turns out to be none, where it has a title, an
//! address in angle brackets or one written without a scheme, where it runs
//! past `MOST`, and where it is the one past `DEFINITION_CAP` definitions
//! or `DEFINITION_BYTES` bytes of them. Definitions and references are read
//! where footnotes are, and shown as written everywhere those are.
//!
//! An address in angle brackets, `<https://…>`, `<http://…>` or `<name@host>`,
//! is drawn without its brackets as a link to the address without them, an
//! e-mail address as a `mailto:` one; one that runs past `MOST`, holds a
//! blank or a control character, or stands in code, a key or sub or
//! superscript, is shown as written. A word written as `www.` and a host, after
//! a space, an opening bracket or a mark of emphasis, is a link to the `https://`
//! address that begins with it.
//!
//! Six tags, `<details>`, `<summary>`, `<br>`, `<kbd>`, `<sub>` and `<sup>`, in
//! any letter case and with attributes holding `<` or `>` only inside a quoted
//! value, are read where prose is, and never in code or from a `[` to the next
//! `]` or the line's end; any other `<`, an address in angle brackets aside, is
//! shown as written, and so is one of
//! the six that runs past its cap or its line, a quote left open included. A
//! paragraph's line inside a `<details>` block is drawn two columns in, on
//! every row it wraps onto, until `</details>` or the message's end, and a
//! `<summary>` is drawn bold behind a mark; both are shown as written in a
//! heading, a table's cell and a block laid out again.
//! `<br>` breaks the line, and is a space in a heading, a table's cell and a
//! block laid out again. A line that a `<br>`, `<details>` or `<summary>` is
//! read on is neither the header of a table written without outer bars nor
//! the title of a heading. What `<kbd>` holds is a key, in the code slot and
//! under ascii in brackets, with nothing in it read; a `<sub>` or `<sup>` run
//! is drawn quiet in sub or superscript characters where every one of its
//! characters has one, and as its plain text where one does not or the run
//! passes its cap or its line. A tag is read away, and a line written at the
//! margin that holds nothing but `<details>` and closing tags draws no row.
//! Inside a `<details>` block, a line written at the margin that holds nothing
//! but `</details>` ends a table written without outer bars and is no row of
//! it.
//!
//! On the way out, each settled run is told by the kind of thing it is, which
//! is what a design colours: a span of code that names a file is a path, and a
//! word of prose can be a version, a hash, a figure or a list's number. That
//! holds a span until it ends and a word with a digit in it until the word
//! does, both bounded, and changes a slot and never a character.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use crate::chrome::{self, Frame, Label, Undrawn};
use crate::color::Slot;
use crate::escape::Escapes;
use crate::files::Files;
use crate::forge::Forge;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

mod escapes;
mod kind;
mod strikes;
mod table;

use crate::syntax::Syntax;
use table::{Sight, Table};

/// The longest run of one marker character read as a marker.
///
/// Six because that is the deepest heading. Past it the run is capped rather
/// than rejected, so `#######` is a heading — the alternative is spending a
/// counter on a line nobody writes.
const RUN: usize = 6;

/// What a run is written back as, where it turned out not to be a marker.
///
/// Static, so putting one back allocates nothing and does not borrow the delta
/// it arrived in — which matters, since a run can start in one delta and be
/// settled by the next.
const HASHES: &str = "######";
const STARS: &str = "******";
const SCORES: &str = "______";
const TICKS: &str = "``````";
const DASHES: &str = "------";
const PLUSES: &str = "++++++";
const ANGLES: &str = ">>>>>>";
const TILDES: &str = "~~~~~~";

/// The most of a table held before it is written out as itself.
///
/// A table is drawn only once the last of it has arrived, so this is the reader
/// with nothing to show — enough for any table an answer sensibly contains, and
/// an end to a wall of bars that is never going to stop.
pub(crate) const TABLE_CAP: usize = 8 * 1024;

/// The most of a line watched in case it is the header or the delimiter row of
/// a table written without outer bars, the title of a heading or the row of
/// `=` or `-` under it.
///
/// Only a copy is kept: the line itself goes out as it arrives. A line past it
/// is prose, and is not watched any further. Far longer than any header or
/// title an answer sensibly contains.
pub(crate) const HEADER_CAP: usize = 128;

/// The most of a block of display math held before it is shown as written.
///
/// Smaller than the others because a formula is a line or two, and one that
/// runs on for pages is prose with dollars in it.
pub(crate) const MATH_CAP: usize = 4 * 1024;

/// The most source bytes held after a single tilde, waiting for the one that
/// closes it: a closer not among them strikes nothing.
pub(crate) const STRIKE_CAP: usize = 4 * 1024;

/// The most of a mermaid fence held before it is shown as written.
pub(crate) const MERMAID_CAP: usize = 16 * 1024;

/// The most of a diff fence held before it is shown as written.
pub(crate) const DIFF_CAP: usize = 16 * 1024;

/// The most footnotes one answer numbers.
///
/// A label is remembered from the first time it is written, as a reference or
/// as a definition, in any letter case, so a definition written after its
/// reference is tied to the number the reference was drawn with. Past this
/// many labels, a reference and a definition with a label not yet numbered
/// are shown as written.
pub(crate) const FOOTNOTE_CAP: usize = 64;

/// The most characters a footnote's label is written with, after its `^`.
///
/// Every label is remembered for the rest of the answer, so each is bounded as
/// well as how many there are. A longer one is no footnote and is shown as
/// written.
pub(crate) const FOOTNOTE_LABEL_CAP: usize = 32;

/// The most reference definitions one answer keeps.
///
/// A definition is kept from the line that wrote it, so a reference after it
/// reads as its words. The first definition of a label is the one kept, and
/// each label counts once, in any letter case. Past this many, a definition is
/// shown as written and links nothing. An answer defines a handful of links;
/// this is as many as it numbers footnotes.
pub(crate) const DEFINITION_CAP: usize = 64;

/// The most bytes of labels and addresses one answer's definitions keep, so
/// that a few long addresses are bounded as well as how many there are.
///
/// Sixty-four definitions of a label and a long address of a couple of
/// hundred bytes each fit; a definition that would pass it is shown as
/// written and links nothing.
pub(crate) const DEFINITION_BYTES: usize = 16 * 1024;

/// The most characters of a tag held while the ones that say whether it is one
/// of the six read here arrive, `<` and `>` included.
///
/// Far longer than `<details open>`; a tag that runs past it is shown as
/// written.
pub(crate) const TAG_CAP: usize = 64;

/// The most bytes of a `<sub>` or `<sup>` run's text held until its closing
/// tag says whether it can be drawn with sub or superscript characters.
///
/// Every character that has such a form is one byte, so for a run that can be
/// drawn so it is as many characters. A run past it is drawn as its plain
/// text, as is one that has a character with no such form.
pub(crate) const SCRIPT_CAP: usize = 64;

/// A kind of block the reader holds back until it closes.
///
/// Everything else streams: an ordinary fence is drawn a line at a time, and a
/// rule is a block of one line that is never held at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Lines that open with a bar, until one that does not; or a header and a
    /// delimiter row written without outer bars, until the first blank line.
    Table,
    /// `$$`, `\[` or `\begin{…}` of an equation environment, until the
    /// marker that closes it, or a fence named for math.
    DisplayMath,
    /// A fence named `mermaid`.
    Mermaid,
    /// A fence named `diff` or `patch`.
    Diff,
}

impl Held {
    /// The block it turns into once it closes.
    #[must_use]
    pub fn kind(self) -> Kind {
        match self {
            Self::Table => Kind::Table,
            Self::DisplayMath => Kind::DisplayMath,
            Self::Mermaid => Kind::Mermaid,
            Self::Diff => Kind::Diff,
        }
    }

    /// What the reader is told is being drawn while it waits.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::DisplayMath => "formula",
            Self::Mermaid => "mermaid diagram",
            Self::Diff => "diff",
        }
    }

    /// The most of it held before it is shown as written.
    fn cap(self) -> usize {
        match self {
            Self::Table => TABLE_CAP,
            Self::DisplayMath => MATH_CAP,
            Self::Mermaid => MERMAID_CAP,
            Self::Diff => DIFF_CAP,
        }
    }

    /// The held kind a fence's language names, where it names one.
    fn fenced(language: &str) -> Option<Self> {
        let named = language.split_whitespace().next()?.to_ascii_lowercase();
        match named.as_str() {
            "mermaid" => Some(Self::Mermaid),
            "diff" | "patch" => Some(Self::Diff),
            "math" | "latex" | "tex" | "katex" => Some(Self::DisplayMath),
            _ => None,
        }
    }
}

/// A held block still arriving, as the live foot tells of it.
///
/// One quiet row and never the block itself: what is held is not drawn until
/// it closes, and a reader watching nothing arrive is owed a word about why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holding {
    /// What is being held.
    pub held: Held,
    /// How many of its lines have begun, the one arriving included.
    pub lines: usize,
}

impl Holding {
    /// The row, drawn for a window `columns` wide and never wider.
    ///
    /// Structural, as a component's marks are: it says what the reader is
    /// waiting for and is none of the answer, so a drag over it lights none of
    /// it and copies none of it.
    #[must_use]
    pub fn row(&self, columns: usize, glyphs: Glyphs) -> Row {
        let lines = match self.lines {
            1 => "1 line".to_owned(),
            lines => format!("{lines} lines"),
        };
        let said = format!(
            "{} drawing a {} {} {lines} so far",
            glyphs.summed(),
            self.held.word(),
            glyphs.dot(),
        );
        Row::new()
            .then_structural(Slot::Quiet, said)
            .clipped(columns)
    }
}

/// Every kind of block the reader hands on whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A table, laid out against the width it is given.
    Table,
    /// A rule across the width it is given.
    Rule,
    /// The rule under a heading of the first or second level, as wide as the
    /// words above it. The words themselves are runs: they stream before the
    /// heading's end is known, and only the rule needs the room there is.
    Heading(Level),
    /// Display math, shown as written.
    DisplayMath,
    /// A mermaid fence, shown as written.
    Mermaid,
    /// A diff fence, shown as written.
    Diff,
    /// An image. Nothing hands one on yet.
    Image,
}

/// How loud a heading is, where it is drawn with a rule under it.
///
/// Two, because a rule is what tells the first two levels apart from the
/// rest: the third and every deeper level are drawn alike, on a row of their
/// own and with no rule, and so have no block to say which they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// A heading `#` or a row of `=` makes, with a heavy rule.
    One,
    /// A heading `##` or a row of `-` makes, with a light rule.
    Two,
}

impl Kind {
    /// The word the reader knows this block by, where it has a source worth
    /// reading whole: a rule, a heading's rule and an image have none.
    #[must_use]
    pub fn named(self) -> Option<&'static str> {
        match self {
            Self::Table => Some("table"),
            Self::DisplayMath => Some("math"),
            Self::Mermaid => Some("mermaid diagram"),
            Self::Diff => Some("diff"),
            Self::Rule | Self::Heading(_) | Self::Image => None,
        }
    }
}

/// A block, with the source it was written as.
///
/// The source rather than what it was drawn as, because what it is drawn as
/// depends on the width and the glyph set, and both can change after it has
/// arrived. Laid out by `lay`.
#[derive(Clone, PartialEq, Eq)]
pub struct Block {
    /// What kind of block it is.
    pub kind: Kind,
    /// The lines it was written as, markers and breaks included.
    pub source: String,
    /// Whether it closed before the message ended. A fence or display math
    /// that never closed is shown as written; a table the message ended in is
    /// still drawn as a table.
    pub closed: bool,
}

impl std::fmt::Debug for Block {
    /// The kind and the size, not the source: an answer's text has no place
    /// in a log line.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Block")
            .field("kind", &self.kind)
            .field("source", &self.source.len())
            .field("closed", &self.closed)
            .finish()
    }
}

/// What the reader hands on: a run of text under its slot, or a whole block.
#[derive(Debug)]
pub enum Said<'a> {
    /// Text wearing `slot`, going to `link` when clicked where it has one.
    Run {
        /// What the text is drawn as.
        slot: Slot,
        /// The text, with its markers gone.
        text: &'a str,
        /// Where a click on it goes.
        link: Option<&'a str>,
    },
    /// A block, handed on once it is whole.
    Block(Block),
    /// The line being written is taken back: it was the header of a table
    /// written without outer bars, which comes next as a [`Block`], or the
    /// title of a heading, which comes next as the same words and then its
    /// rule as a [`Block`].
    ///
    /// Said once the line under it has settled it, so a line that might have
    /// been one is on screen as prose until then. Everything before it was
    /// said is the line's own, and nothing else is taken.
    Retract,
}

/// What a reader links its text to: the repository a bare number counts
/// against and the checkout a path to a file is read against.
///
/// Given to every reader of an answer, and kept by the record as well, since a
/// block is read again each time it is laid out and has to link what the same
/// text linked as it streamed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Links {
    /// The repository bare numbers count against; `None` leaves a number the
    /// plain text it was written as.
    pub(crate) forge: Option<Forge>,
    /// The checkout a link to a path opens in; `None` hands the path on as it
    /// was written.
    pub(crate) files: Option<Files>,
}

/// `block` laid out as the lines it draws `room` columns wide, in `glyphs`.
///
/// What the record keeps a block as, and what it lays it out with again at
/// every width and glyph set it is asked for.
///
/// Lines rather than rows: one put back as it was written can be wider than
/// the room, and is folded where it is drawn, as streamed text is.
#[must_use]
pub(crate) fn lay(block: &Block, room: usize, glyphs: Glyphs, links: &Links) -> Vec<Row> {
    let mut rows = vec![Row::new()];
    tell(block, room, glyphs, links, &mut |slot, text, link| {
        let mut pieces = text.split('\n');
        if let Some(piece) = pieces.next() {
            push(&mut rows, slot, piece, link);
        }
        for piece in pieces {
            rows.push(Row::new());
            push(&mut rows, slot, piece, link);
        }
    });

    // The break that ends the last line ends the block, and is no row of it.
    if rows.last().is_some_and(Row::is_empty) {
        rows.pop();
    }
    rows
}

/// `block` as the runs [`lay`] makes its lines of, each with where it links,
/// handed to `put` in order, a line's end as a break in the text.
///
/// A table is drawn against the room, or put back as it was written where it
/// cannot be; a rule is drawn across the room; anything else is read again by
/// a reader that holds nothing and links by `links`, so it is shown the way it
/// would have streamed. Runs rather than lines for a page printed once, which
/// keeps each address beside its words as a run streamed past it would.
pub(crate) fn tell(
    block: &Block,
    room: usize,
    glyphs: Glyphs,
    links: &Links,
    put: &mut dyn FnMut(Slot, &str, Option<&str>),
) {
    match block.kind {
        Kind::Table => {
            let mut kinds = kind::Kinds::default();
            Table::of(&block.source).laid(glyphs, room, &mut |slot, text, link| {
                kinds.tell(slot, text, link, &mut *put);
            });
            kinds.flush(&mut *put);
        }
        // A character at a time, as the rule streamed before it was a block.
        Kind::Rule => {
            for _ in 0..room {
                put(Slot::Quiet, glyphs.horizontal(), None);
            }
        }
        // As wide as the last row the words come to at this room, which is
        // the row the record draws them in: the same reader, the same fold.
        Kind::Heading(level) => {
            let mark = match level {
                Level::One => glyphs.heavy(),
                Level::Two => glyphs.horizontal(),
            };
            for _ in 0..headed(block, room, glyphs, links) {
                put(Slot::Quiet, mark, None);
            }
        }
        Kind::DisplayMath | Kind::Mermaid | Kind::Diff | Kind::Image => {
            let mut markdown = Markdown::new(glyphs)
                .linking(links)
                .as_written()
                .unescaped();
            let mut said = |said: Said<'_>| {
                if let Said::Run { slot, text, link } = said {
                    put(slot, text, link);
                }
            };
            markdown.read(&block.source, room, &mut said);
            markdown.finish(room, &mut said);
        }
    }
}

/// The title a heading block was written with: the line a row of `=` or `-`
/// stood under, or the heading line itself, `#` marks and all.
///
/// A line that is a title is read again as the heading it is, which is the one
/// reading the record draws it with.
fn title(source: &str) -> Cow<'_, str> {
    match source.split_once('\n') {
        Some((title, _)) => Cow::Owned(format!("# {}", title.trim())),
        None => Cow::Borrowed(source),
    }
}

/// How many columns the last row of `block`'s words comes to, folded to `room`.
fn headed(block: &Block, room: usize, glyphs: Glyphs, links: &Links) -> usize {
    let mut row = Row::new();
    let mut markdown = Markdown::new(glyphs).linking(links).as_written();
    let mut said = |said: Said<'_>| {
        if let Said::Run { slot, text, link } = said {
            match link {
                Some(link) => row.push_linked(slot, text, link),
                None => row.push(slot, text),
            }
        }
    };
    markdown.read(&title(&block.source), room, &mut said);
    markdown.finish(room, &mut said);
    row.fold(room).last().map_or(0, Row::columns)
}

/// One run onto the last of `rows`.
fn push(rows: &mut [Row], slot: Slot, text: &str, link: Option<&str>) {
    if text.is_empty() {
        return;
    }
    if let Some(row) = rows.last_mut() {
        match link {
            Some(link) => row.push_linked(slot, text, link),
            None => row.push(slot, text),
        }
    }
}

/// What a line of an answer stands in front of its words with, and so what
/// every row it wraps onto stands in front of theirs.
///
/// Read off the line as it was drawn rather than kept beside it: the marks are
/// drawn in the slots this reader gives them, and the line is the one place a
/// record that folds it again at another width has them all. See [`hang`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hang {
    /// How many bytes of the line's text stand in front of its words.
    pub(crate) lead: usize,
    /// What stands in front of the words of each row after the first: the bar
    /// of a quote again, and the blanks that leave the words where they began.
    pub(crate) prefix: Row,
    /// The columns of `prefix`, which are the columns the first row spends on
    /// what stands in front of its words.
    pub(crate) columns: usize,
}

/// What a line of a `<details>` block's body stands in front of its words
/// with: two quiet columns, so it is told apart from prose that is indented.
const BODY_LEAD: &str = "  ";

/// How the line `row` hangs when it wraps, where it opens with a quote's bar,
/// an item's mark or the lead of a `<details>` block's body.
///
/// The bar of a quote or of an alert, the mark of a bullet or a task and the
/// number of a numbered item are what this reader draws in front of an item's
/// words, each followed by the space that parts it from them. Rows after the
/// first stand the bar again where there was one and leave the columns of the
/// rest blank, so a number of two digits hangs two columns further than one of
/// one, and the indentation that nested the item is kept with it. The two
/// columns a `<details>` block's body is set in by are left blank the same way.
///
/// `None` for a line that opens with none of them: it wraps flush, as prose
/// does. A mark with no space after it is not one.
#[must_use]
pub(crate) fn hang(row: &Row) -> Option<Hang> {
    let (mut lead, mut columns, mut hung) = (0, 0, false);
    let mut prefix = Row::new();
    let mut spans = row.spans().peekable();

    while let Some(&(slot, text)) = spans.peek() {
        if slot == Slot::Quiet && text == BODY_LEAD {
            prefix.push(Slot::Quiet, text);
            lead += text.len();
            columns += text.len();
            hung = true;
        } else if text.bytes().all(|byte| byte == b' ') {
            prefix.push(Slot::Plain, text);
            lead += text.len();
            columns += text.len();
        } else if slot == Slot::Quiet
            && [Glyphs::Unicode, Glyphs::Ascii]
                .into_iter()
                .flat_map(|glyphs| [glyphs.vertical(), glyphs.alerted()])
                .any(|bar| bar == text)
        {
            spans.next();
            if spans.next_if(|&(_, next)| next == " ").is_none() {
                break;
            }
            prefix.push(Slot::Quiet, text);
            prefix.push(Slot::Quiet, " ");
            lead += text.len() + 1;
            columns += crate::width::columns(text) + 1;
            hung = true;
            continue;
        } else {
            let marks = match slot {
                Slot::Bullet | Slot::DoneMark | Slot::Ordinal => true,
                Slot::Quiet => [Glyphs::Unicode, Glyphs::Ascii]
                    .map(Glyphs::open)
                    .contains(&text),
                _ => false,
            };
            spans.next();
            if marks && spans.next_if(|&(_, next)| next.starts_with(' ')).is_some() {
                let width = crate::width::columns(text) + 1;
                prefix.push(Slot::Plain, " ".repeat(width));
                lead += text.len() + 1;
                columns += width;
                hung = true;
            }
            break;
        }
        spans.next();
    }

    hung.then_some(Hang {
        lead,
        prefix,
        columns,
    })
}

/// What stands in front of a row of code that carries on the one above it,
/// where a line of code wrapped: a mark and a space, as wide as the gutter.
#[must_use]
pub(crate) const fn continuation(glyphs: Glyphs) -> &'static str {
    match glyphs {
        Glyphs::Unicode => "┆ ",
        Glyphs::Ascii => ": ",
    }
}

/// The set a row of code in its frame was drawn in, which its gutter says.
fn gutters(row: &Row) -> Option<(Glyphs, usize)> {
    let (slot, text) = row.spans().next()?;
    if slot != Slot::Quiet {
        return None;
    }
    [Glyphs::Unicode, Glyphs::Ascii]
        .into_iter()
        .find(|glyphs| chrome::gutter(*glyphs) == text)
        .map(|glyphs| (glyphs, text.len()))
}

/// A frame's edge, as the structure it is: drawn, never copied.
///
/// Only a row of nothing but quiet words is read as one, which is how the
/// reader says an edge and no running text is said.
fn edged(row: &Row) -> bool {
    row.spans().all(|(slot, _)| slot == Slot::Quiet) && chrome::is_edge(&row.text())
}

/// Where `code` breaks to fit `room` columns, as byte ranges that partition it.
///
/// A line of code is broken wherever the room runs out, never at a space: it
/// is cut as written and nothing is dropped at a break, because what a reader
/// copies off a wrapped line has to be the line. Whitespace that would end a
/// row begins the next one instead, so that the rows read back off the screen
/// (which has no way to tell a space from the blank after it) join into the
/// line. A row of nothing but whitespace keeps it, and may lose it on a copy.
///
/// The one thing that is dropped: a character wider than `room` itself, which
/// no row could draw (a double-width character where one column is left), is
/// left out with the marks on it, so a copy of that line lacks it.
fn cuts(code: &str, room: usize) -> Vec<Range<usize>> {
    let mut parts = Vec::new();
    let mut base = 0;

    while base < code.len() {
        let rest = code.get(base..).unwrap_or_default();
        let Some(over) = width::cut(rest, room) else {
            parts.push(base..code.len());
            break;
        };

        // A character wider than the room can be drawn nowhere in it, so it is
        // left out, with the marks that ride on it.
        if over == 0 {
            let mut left = rest.chars();
            let first = left.next().map_or(rest.len(), char::len_utf8);
            let marks: usize = left
                .take_while(|mark| width::advance(*mark) == Some(0))
                .map(char::len_utf8)
                .sum();
            base += first + marks;
            continue;
        }

        let row = rest.get(..over).unwrap_or_default();
        let kept = row.trim_end_matches(char::is_whitespace).len();
        let at = if kept == 0 { over } else { kept };
        parts.push(base..base + at);
        base += at;
    }

    parts
}

/// Where a row of code in its frame breaks at `columns`: the set it was drawn
/// in, the bytes its gutter takes, and the ranges of its code after that.
///
/// `None` for a row that is not one, and for a window with no room for code
/// beside the gutter.
fn broken(row: &Row, columns: usize) -> Option<(Glyphs, usize, Vec<Range<usize>>)> {
    let (glyphs, lead) = gutters(row)?;
    let room = columns
        .checked_sub(chrome::GUTTER)
        .filter(|room| *room > 0)?;
    let code = row.between(lead, usize::MAX).text();
    let mut parts = cuts(&code, room);
    if parts.is_empty() {
        parts.push(0..0);
    }
    Some((glyphs, lead, parts))
}

/// The display rows a row of code in its frame comes to at `columns`, where
/// `row` is one.
///
/// The gutter is the frame's, and structural, so a drag takes the code and not
/// the bar. A line that wraps carries on under a mark of its own where the
/// gutter stood, also structural, and breaks wherever the room runs out. What
/// the code says comes back whole and in order, which is what a copy of the
/// wrapped line is made from. The frame's two edges are structural too, and
/// are clipped to the window as the frame is, never folded, so that every row
/// of one is known as an edge and a drag over a block takes its code alone.
#[must_use]
pub(crate) fn framed(row: &Row, columns: usize) -> Option<Vec<Row>> {
    if edged(row) {
        let edge = Row::new().then_structural(Slot::Quiet, row.text());
        return Some(vec![edge.clipped(columns)]);
    }
    let (glyphs, lead, parts) = broken(row, columns)?;
    let code = row.between(lead, usize::MAX);

    Some(
        parts
            .into_iter()
            .enumerate()
            .map(|(at, part)| {
                let front = if at == 0 {
                    chrome::gutter(glyphs)
                } else {
                    continuation(glyphs)
                };
                Row::new()
                    .then_structural(Slot::Quiet, front)
                    .join(code.between(part.start, part.end))
            })
            .collect(),
    )
}

/// How many display rows [`framed`] gives `row`, without building them.
#[must_use]
pub(crate) fn framed_height(row: &Row, columns: usize) -> Option<usize> {
    if edged(row) {
        return Some(1);
    }
    broken(row, columns).map(|(_, _, parts)| parts.len())
}

/// What a fence's info string names, as its frame's label is made from it.
///
/// Cleaned first and split after: an escape sequence has spaces in it that
/// are not the end of the name, and a control character in the middle of a
/// word is one word with the control taken out of it. Cleaned here, once, and
/// not by whatever draws the label, because this is where the text from a
/// model is read -- what is dropped is what a terminal would act on or a
/// reader would be misled by, and the placeholder an image is placed with
/// along with the marks that give it a row and a column.
fn fence_name(info: &str) -> String {
    let mut escapes = Escapes::default();
    let mut placed = false;
    let cleaned: String = info
        .chars()
        .filter(|character| {
            if escapes.holds(*character) {
                return false;
            }
            if *character == PLACEHOLDER {
                placed = true;
                return false;
            }
            let advance = width::advance(*character);
            let mark = placed && advance == Some(0);
            placed = placed && mark;
            advance.is_some() && !mark
        })
        .collect();
    cleaned
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The character an image is placed with, which no text from a model may use.
const PLACEHOLDER: char = '\u{10EEEE}';

/// A held block being gathered.
#[derive(Debug)]
struct Gathering {
    held: Held,
    /// Everything written so far, the opening marker included.
    source: String,
    /// What ends it.
    closer: Closer,
    /// Where the line a closer is looked for in begins.
    line: usize,
}

/// What ends a held block other than a table.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Closer {
    /// A line opening, after its spaces, with three or more of this mark.
    Fence(char),
    /// A line holding this, anywhere on it.
    Within(String),
}

impl Closer {
    /// Whether `line` is the one that ends the block.
    fn closes(&self, line: &str) -> bool {
        match self {
            Self::Fence(mark) => {
                line.trim_start_matches(' ')
                    .chars()
                    .take_while(|character| character == mark)
                    .count()
                    >= 3
            }
            Self::Within(closer) => line.contains(closer.as_str()),
        }
    }
}

/// Display math being shown as written past its cap, until the line that
/// holds its closer ends.
///
/// Watched so the closer is not read as a block opening: `$$` closes a block
/// and opens one, and which it does depends on what came before it.
#[derive(Debug)]
struct Passing {
    closer: String,
    /// The end of the line being read, as long as the closer at most.
    tail: String,
    /// Whether this line holds the closer.
    seen: bool,
}

impl Passing {
    /// Takes `character`; `true` once the line holding the closer has ended.
    fn passes(&mut self, character: char) -> bool {
        if character == '\n' {
            if self.seen {
                return true;
            }
            self.tail.clear();
            return false;
        }
        self.tail.push(character);
        while self.tail.len() > self.closer.len() {
            self.tail.remove(0);
        }
        self.seen |= self.tail.ends_with(self.closer.as_str());
        false
    }
}

/// Whether a line opening with display math is one, so far.
#[derive(Debug, Default, PartialEq, Eq)]
enum Probe {
    /// Nothing looked at yet on this line.
    #[default]
    Ready,
    /// What the line opened with, held while it could still be an opening.
    Probing(String),
    /// Looked at and given up on for the rest of the line.
    Done,
}

/// What the start of a line spells, as display math.
#[derive(Debug, PartialEq, Eq)]
enum Opener {
    /// Could still be the opening of a block.
    Partly,
    /// The opening of a block, ended by this.
    Whole(String),
    /// Not the opening of one.
    Not,
}

/// The environments a `\begin` opens display math with, starred or not.
const ENVIRONMENTS: [&str; 12] = [
    "equation",
    "align",
    "gather",
    "multline",
    "cases",
    "matrix",
    "pmatrix",
    "bmatrix",
    "Bmatrix",
    "vmatrix",
    "Vmatrix",
    "smallmatrix",
];

/// What `written`, at the start of a line, spells as display math.
fn opener(written: &str) -> Opener {
    const BEGIN: &str = "\\begin{";
    let written = written.trim_start_matches(' ');
    match written {
        "$$" => return Opener::Whole("$$".to_owned()),
        "\\[" => return Opener::Whole("\\]".to_owned()),
        _ => {}
    }
    if "$$".starts_with(written) || "\\[".starts_with(written) || BEGIN.starts_with(written) {
        return Opener::Partly;
    }
    let Some(named) = written.strip_prefix(BEGIN) else {
        return Opener::Not;
    };
    match named.strip_suffix('}') {
        Some(name) if ENVIRONMENTS.contains(&name.strip_suffix('*').unwrap_or(name)) => {
            Opener::Whole(format!("\\end{{{name}}}"))
        }
        None if named.len() <= "smallmatrix*".len()
            && named
                .chars()
                .all(|character| character.is_ascii_alphabetic() || character == '*') =>
        {
            Opener::Partly
        }
        _ => Opener::Not,
    }
}

/// How many lines of `source` have begun, the one still arriving included.
fn begun(source: &str) -> usize {
    let breaks = source.matches('\n').count();
    breaks + usize::from(!source.is_empty() && !source.ends_with('\n'))
}

/// A run of one marker character, waiting for the character that says what it
/// was.
#[derive(Debug, Clone, Copy)]
struct HeldMarks {
    mark: char,
    count: usize,
    /// Whether the run began before anything else on the line.
    ///
    /// Kept on the run rather than read when it settles, because settling is
    /// what makes the line started: by then every run looks like one in the
    /// middle of a line. It is what tells `* item` from `a *word*`, and it is
    /// the whole of what makes a bullet a bullet.
    opened: bool,
}

/// A bulleted item's mark, through the life of the item it belongs to.
///
/// A mark is not drawn where its bullet settles, because what the mark *is*
/// depends on what comes after it: `- [ ] a thing` is a task and a task's mark
/// is a box. A scan reading one character at a time cannot know that yet, so
/// the mark is owed until the first thing that is not a box arrives, and paid
/// immediately before it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Marked {
    /// Nothing on this line is a bulleted item.
    #[default]
    Nothing,
    /// A bullet whose mark is owed.
    Owed,
    /// A mark already drawn: a plain bullet, or a task nobody has finished.
    Drawn,
    /// A task somebody has finished.
    ///
    /// Its words wear [`Slot::Done`] whatever else is written into them: a
    /// finished task is behind you, and the emphasis inside one is something
    /// leant on while it was still ahead.
    Done,
}

/// What a line that opened with a quote mark is part of.
///
/// The bar of a quote's first line is owed, like a bullet's mark, because what
/// the quote is cannot be told until its first line is over: `> [!NOTE]` with
/// nothing after it is the header of an alert, and the bar down its left is
/// not the bar of a plain quote. Every later line of a quote knows what it is
/// part of and draws its bar at once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Quoted {
    /// The line opened with no quote mark.
    #[default]
    Not,
    /// The first line of a quote, whose bar has not been drawn.
    Owed,
    /// A line of a quote, bar drawn or not owed.
    Plain,
    /// A line of an alert, which wears its bar on every row.
    Alert,
}

/// The kinds of alert an answer can write, as `> [!KIND]` on the first line of
/// a quote with nothing after it.
///
/// A closed set, as GitHub's own is: a quote naming anything else is a quote
/// and shows what was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Alert {
    /// The kind a marker names, in any case, or `None` for words that are not
    /// the whole of one: `[!NOTES]` is a bracket somebody wrote.
    fn of(marker: &str) -> Option<Self> {
        let kind = marker.strip_prefix('!')?;
        [
            Self::Note,
            Self::Tip,
            Self::Important,
            Self::Warning,
            Self::Caution,
        ]
        .into_iter()
        .find(|alert| kind.eq_ignore_ascii_case(alert.word()))
    }

    /// The word the alert is shown under.
    fn word(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    /// The slot the alert's mark wears, which is what tells the kinds apart by
    /// ink: the bar, the space and the word are the same for every kind.
    fn slot(self) -> Slot {
        match self {
            Self::Note => Slot::Link,
            Self::Tip => Slot::Done,
            Self::Important => Slot::Accent,
            Self::Warning => Slot::Doing,
            Self::Caution => Slot::Trouble,
        }
    }

    /// The mark in front of the word, out of the set being drawn with.
    fn mark(self, glyphs: Glyphs) -> &'static str {
        match self {
            Self::Note => glyphs.noted(),
            Self::Tip => glyphs.advised(),
            Self::Important => glyphs.urged(),
            Self::Warning => glyphs.warned(),
            Self::Caution => glyphs.cautioned(),
        }
    }
}

/// What is open on the line being read.
///
/// Most of it ends where the line does. Emphasis is the exception, because a
/// model writes a bold phrase and lets it wrap, and a run closed on the line
/// after the one that opened it is the case that actually happens -- read
/// per-line, the opening marker is eaten and the closing one is printed into
/// the prose, which is worse than either answer.
///
/// So emphasis crosses a line break and nothing else does, and it crosses only
/// where the paragraph does: a blank line ends it, a block mark opening the
/// next line ends it, and a fence ends it. An unclosed marker then costs the
/// paragraph it was written in rather than the rest of the answer.
// Each bool is a fact about the line that holds whatever the others are, a
// key inside a summary on a note's line among them; one state for them all
// would have to name every combination.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, Clone, Copy)]
struct Line {
    heading: bool,
    code: bool,
    /// What the line's bullet is marked with, where it has one.
    marked: Marked,
    /// Whether the line opened with a quote mark, so the rest of it is
    /// somebody else's words, and what kind of quote it is part of.
    quoted: Quoted,
    emphasis: Emphasis,
    /// Whether the gutter of a line of code has gone out.
    ///
    /// Owed until the line has something to put it in front of, or ends: a
    /// blank line of code still stands in its frame.
    gutter: Gutter,
    /// Whether the line is a footnote's definition, from its number on, so
    /// the rest of it is a note and quiet.
    noted: bool,
    /// Whether the one space between a note's number and its text is still
    /// owed: whatever spaces were written after the colon are not drawn, and
    /// this one is, before the first thing that is.
    gap: bool,
    /// Whether a `<kbd>` tag is open, so what follows is a key, drawn in the
    /// code slot as written until its closing tag or the end of the line.
    key: bool,
    /// Whether a `<summary>` tag is open, so what follows is the summary of a
    /// `<details>` block, drawn bold.
    summary: bool,
    /// Whether one of the tags read here has been read on the line, so a line
    /// that held nothing else draws no row at all.
    tagged: bool,
    /// Whether anything has gone out on the line, the spaces in front of its
    /// first word included: those go out as they arrive where nothing holds
    /// them, before the line has started.
    spoke: bool,
    /// Whether a `<br>` is owed: the row it breaks to stands only once
    /// something is drawn on it, since the line's own end breaks it as well.
    broken: bool,
    /// Whether the line has been set two columns in as part of the body of a
    /// `<details>` block, or that has been decided against.
    fold: Fold,
}

/// Whether a line inside a `<details>` block has been set in from the margin.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Fold {
    /// Not decided until the first thing on the line reaches the terminal.
    #[default]
    Owed,
    /// Two spaces went out in front of the line.
    Drawn,
    /// The line is no line of a body, or is blank, and stays where it is.
    Not,
}

/// Whether a line of code has put its gutter out yet.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Gutter {
    #[default]
    Owed,
    Put,
}

/// The emphasis open on the line being read.
///
/// Two of them, because markdown spells both with the same character and tells
/// them apart by how many there are: one marker leans on a phrase and two raise
/// it, and a phrase written with three is under both at once.
#[derive(Debug, Default, Clone, Copy)]
struct Emphasis {
    /// One marker around the phrase.
    leant: bool,
    /// Two or more.
    raised: bool,
    /// Two tildes around it: written, and then taken back.
    struck: bool,
}

/// Whether a line may still be watched as the header of a table written
/// without outer bars or the title of a heading. A line that ran past its cap
/// or showed it was neither is written out as it was, and the rest of its line
/// is `Declined`: it is not watched a second time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Header {
    #[default]
    Open,
    Declined,
}

/// Whether the line being said is to be taken back.
///
/// What a header of a table written without outer bars or the title of a
/// heading needs: it is said as prose until the line under it settles what it
/// is. Whoever is told what is said keeps the line apart until then, if it can
/// put it out for good: see [`Markdown::in_doubt`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Retract {
    /// It may, and nothing is owed.
    #[default]
    Allowed,
    /// The line is to be taken back before anything else is said.
    Owed,
}

/// What the scan is in the middle of, across lines.
///
/// A block of code is the one thing a line break does not end — that is what a
/// fence is for, and a block of code is the one place where running past its
/// end is less wrong than stopping at the first line of it. So one the model
/// never closes does run to the end of the message; the message boundary is
/// where it is cleared.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Inside {
    #[default]
    Prose,
    /// The rest of an opening fence's line, which names a language.
    Opening,
    /// A fenced block, until a fence at the start of a line closes it.
    Fence,
    /// The rest of a closing fence's line.
    Closing,
    /// A block indented four spaces after a blank line, until a line that is
    /// not blank and is indented less than that ends it.
    Indented,
}

/// How the line being read opened, as far as that has arrived.
///
/// Read once per line, at its first characters, because two things are
/// decided there and nowhere else: whether the line is a line of an indented
/// block of code, which its spaces say, and whether it opens an item numbered
/// `1.` or `1)`, after which an indented line is the item's and not code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lead {
    /// Only spaces so far, this many of them.
    Margin(usize),
    /// Digits after the spaces, this many of them: the number of an item, if
    /// a stop and a space follow.
    Number(usize),
    /// The stop after an item's number, waiting for the space that makes it
    /// one.
    Stopped,
    /// The line has said what it opened with.
    Read,
}

impl Default for Lead {
    /// A line nothing of has arrived.
    fn default() -> Self {
        Self::Margin(0)
    }
}

/// What the line before this one was, as far as a block indented four spaces
/// cares: one may open only after a blank line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Above {
    /// Something was written on it, or there was none.
    #[default]
    Written,
    Blank,
}

/// Whether an item of a list is open, across the lines under it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Item {
    #[default]
    Shut,
    Open,
}

/// The most digits an item's number is written with, as markdown allows: a
/// longer run of digits opens no item.
const NUMBERED: usize = 9;

/// How many spaces in front of a line make it a line of code.
const CODE: usize = 4;

/// Which part of a link's shape is arriving.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Between `[` and the `]` that ends it.
    #[default]
    Label,
    /// Just past the `]`, where only a `(` carries on being a link.
    Between,
    /// Between `(` and the `)` that ends it.
    Target,
    /// Between the `[` that follows the `]` and the `]` that ends it: the
    /// label of a reference, held in the target's place.
    Second,
}

/// A link's words and its address, while the shape that decides them is still
/// arriving.
///
/// The one thing here that cannot be handed on as it is read. Nothing says the
/// words were a link's words until the `](` after them, so they are held until
/// it arrives -- and put back exactly as they were written where it does not,
/// which is what leaves `[TODO] fix this` alone.
#[derive(Debug, Default, Clone)]
struct Link {
    part: Part,
    label: String,
    target: String,
    /// Whether the bracket opened the line, where a footnote's label is its
    /// definition rather than a reference to it.
    first: bool,
}

impl Link {
    /// Whether there is room for another character.
    ///
    /// Bounded for the reason the line of code beside it is: a buffer that
    /// grows with the answer is the one thing this crate may not have. Past the
    /// bound the shape stops being a link and is put back as the text it is.
    fn holds(&self) -> bool {
        self.label.len().saturating_add(self.target.len()) < MOST
    }
}

/// A number the answer wrote, while the characters that decide it arrive.
///
/// Held for the reason an address is: `#487` is one word to the reader and a
/// marker followed by digits to a scan reading characters, and whether it is a
/// reference at all is not known until something that is not a digit turns up.
#[derive(Debug, Clone, Default)]
struct Reference {
    /// The word the answer put in front of the number to say what kind of
    /// thing it counts -- `PR `, `issue ` -- where it put one. Peeled off the
    /// run for the reason the repository is: it is part of what the reader
    /// takes the reference to be, so it is part of the words they click.
    lead: Option<Box<str>>,
    /// The repository the reference named in front of its hash, where it named
    /// one. Peeled off the run rather than read here, because it arrived
    /// before the hash that said it was a repository at all.
    slug: Option<Box<str>>,
    /// The digits after the hash, as far as they have arrived.
    number: String,
}

/// How many digits a number is read as, at most.
///
/// A forge counts in the thousands and this is room for a great deal more than
/// that. The bound is here for the reason every other one in this file is:
/// what is held comes off a wire, and a run of digits nobody meant to end is
/// not a reason to grow a string without limit.
const DIGITS: usize = 12;

/// What a repository is named with, either side of the slash.
fn named(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '_' | '-')
}

/// The words an answer puts in front of a number to say what it counts.
///
/// Matched without regard to case, and only as whole words: `PR #12` and
/// `issue #12` are read the way they are written, as one thing, where `prepr
/// #12` names nothing. Short, on purpose -- a word here goes into the link's
/// words, so each is one a reader would expect to be part of the reference.
const LEADS: [&str; 3] = ["pull request", "issue", "pr"];

/// How many bytes at the end of `pending` are a lead word and the one space
/// after it, where they are.
///
/// Zero where there is none. Whatever is in front of the word has to be a word
/// boundary -- the start of the run, or a character no word is spelled with --
/// so the end of a longer word is not taken for the lead it happens to end in.
fn lead(pending: &str) -> usize {
    let Some(words) = pending.strip_suffix(' ') else {
        return 0;
    };
    let lower = words.to_ascii_lowercase();

    LEADS
        .into_iter()
        .filter(|one| lower.ends_with(one))
        .map(|one| one.len() + 1)
        .find(|back| {
            words
                .get(..words.len() + 1 - back)
                .and_then(|before| before.chars().next_back())
                .is_none_or(|before| !before.is_alphanumeric())
        })
        .unwrap_or(0)
}

/// How many bytes at the end of `pending` name a repository, where they do.
///
/// `owner/repo` and nothing else: one slash, a name either side of it, and
/// nothing but a word boundary in front. Two slashes is a path somebody wrote
/// and a path is not a repository, which is what keeps `src/cli/draw.rs#L20`
/// out of this.
fn repository(pending: &str) -> Option<usize> {
    let back = pending.len() - pending.trim_end_matches(|c| named(c) || c == '/').len();
    let slug = pending.get(pending.len() - back..)?;
    let (owner, repo) = slug.split_once('/')?;

    let spelled = |part: &str| !part.is_empty() && part.chars().all(named);
    (spelled(owner) && spelled(repo)).then_some(back)
}

/// The schemes an address is recognised by.
///
/// Two, and both of them the web's: an answer about code is full of words with
/// a colon in them, and the ones a reader can act on in a terminal are these.
const SCHEMES: [&str; 2] = ["https://", "http://"];

/// What an address written without a scheme starts with.
///
/// Read as an address of its own in prose, and linked to the `https` address
/// it names, so that following it never leaves the encrypted web.
const WWW: &str = "www.";

/// How much of a scheme a run of characters has spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelled {
    /// Not a scheme and not on the way to one.
    Nothing,
    /// A scheme so far, with more of it to come.
    Partly,
    /// A scheme, and everything after it belongs to the address.
    Whole,
}

/// How much of a scheme `text` has spelled.
fn spelled(text: &str) -> Spelled {
    if SCHEMES.iter().any(|scheme| text.starts_with(scheme)) || text.starts_with(WWW) {
        Spelled::Whole
    } else if SCHEMES
        .iter()
        .chain([&WWW])
        .any(|scheme| scheme.starts_with(text))
    {
        Spelled::Partly
    } else {
        Spelled::Nothing
    }
}

/// A footnote's label, in lower case, and whether its definition has been
/// read.
#[derive(Debug, Clone)]
struct Note {
    label: String,
    defined: bool,
}

/// A reference definition kept for the rest of the answer: its label in lower
/// case, and the address it names.
#[derive(Debug, Clone)]
struct Definition {
    label: String,
    address: String,
}

/// A definition line being read, from the colon after its label on: the rest of
/// the line, held until the line says whether it is one address and nothing
/// else. Bounded by [`MOST`]; ends with the line.
#[derive(Debug, Clone)]
struct Defining {
    label: String,
    rest: String,
}

/// Whether what a link held is a footnote's label: a caret, then one to
/// [`FOOTNOTE_LABEL_CAP`] characters that are neither whitespace nor control
/// characters.
fn footnote(label: &str) -> bool {
    let Some(name) = label.strip_prefix('^') else {
        return false;
    };
    let length = name.chars().count();
    (1..=FOOTNOTE_LABEL_CAP).contains(&length)
        && name
            .chars()
            .all(|character| !character.is_whitespace() && !character.is_control())
}

/// The text of a `<sub>` or `<sup>` run, until its closing tag.
#[derive(Debug, Clone)]
struct Script {
    /// `<sup>` rather than `<sub>`.
    raised: bool,
    text: String,
}

/// The tags read here, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Name {
    Details,
    Summary,
    Br,
    Kbd,
    Sub,
    Sup,
}

impl Name {
    /// Every name, as it is written in lower case.
    const ALL: [(&'static str, Self); 6] = [
        ("details", Self::Details),
        ("summary", Self::Summary),
        ("br", Self::Br),
        ("kbd", Self::Kbd),
        ("sub", Self::Sub),
        ("sup", Self::Sup),
    ];
}

/// One of the six tags, whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag {
    name: Name,
    /// Written `</name>`.
    closing: bool,
}

/// How much of a tag the characters held from a `<` have spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// No tag read here, and none on the way to one.
    Not,
    /// One of them so far, with more of it to come.
    Partly,
    Whole(Tag),
}

/// How much of a tag `written`, from its `<` on, has spelled.
///
/// A name in any letter case, then `>` straight after it or after whitespace
/// and attributes; and `<br/>` or `<br />`. A closing tag is the same with a
/// `/` after the `<`. A value quoted after an `=` is passed over to its
/// closing quote, so a `<` or `>` in it neither spoils nor ends the tag, and
/// one left open leaves the tag never whole.
fn shape(written: &str) -> Shape {
    let Some(rest) = written.strip_prefix('<') else {
        return Shape::Not;
    };
    let (closing, rest) = match rest.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let named = rest.len()
        - rest
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .len();
    let (name, after) = rest.split_at(named);
    let name = name.to_ascii_lowercase();

    let Some(after) = after.chars().next().map(|first| (first, after)) else {
        let partly = Name::ALL.iter().any(|(whole, _)| whole.starts_with(&name));
        return if partly { Shape::Partly } else { Shape::Not };
    };
    let Some(&(_, name)) = Name::ALL.iter().find(|(whole, _)| *whole == name) else {
        return Shape::Not;
    };
    let whole = Shape::Whole(Tag { name, closing });
    match after {
        ('>', ">") => whole,
        ('/', "/") if name == Name::Br && !closing => Shape::Partly,
        ('/', "/>") if name == Name::Br && !closing => whole,
        (first, attributes) if first.is_whitespace() => match ended(attributes) {
            Some(true) => whole,
            Some(false) => Shape::Partly,
            None => Shape::Not,
        },
        _ => Shape::Not,
    }
}

/// The tag that closes a `<details>` block.
const CLOSES_DETAILS: Tag = Tag {
    name: Name::Details,
    closing: true,
};

/// Whether `character` can be part of an e-mail address written between angle
/// brackets, on either side of its `@`.
fn email(character: char) -> bool {
    character.is_ascii_alphanumeric() || ".!#$%&'*+=?^_`{|}~-".contains(character)
}

/// Whether `body`, what stands after a `<`, could still be an address in angle
/// brackets: a scheme and then no whitespace, control character or `<`, or an
/// e-mail address's characters.
fn angles(body: &str) -> bool {
    match spelled(body) {
        Spelled::Nothing => body
            .chars()
            .all(|character| email(character) || character == '@'),
        _ => body.chars().all(|character| {
            !character.is_whitespace() && !character.is_control() && !matches!(character, '<' | '>')
        }),
    }
}

/// Where the address `body`, written between angle brackets, points: itself
/// where it is a web address, and `mailto:` and itself where it is an e-mail
/// address. `None` where it is neither.
fn angle_target(body: &str) -> Option<String> {
    if spelled(body) == Spelled::Whole && !body.starts_with(WWW) {
        return (!SCHEMES.contains(&body)).then(|| body.to_owned());
    }
    let (local, host) = body.split_once('@')?;
    let local = !local.is_empty() && local.chars().all(email);
    let host = host.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    });
    (local && host).then(|| format!("mailto:{body}"))
}

/// Whether `line` is nothing but the tag that closes a `<details>` block,
/// written at the margin and no longer than [`TAG_CAP`], with only blanks
/// after it.
fn closes_details(line: &str) -> bool {
    let line = line.trim_end_matches([' ', '\t']);
    line.chars().count() <= TAG_CAP && shape(line) == Shape::Whole(CLOSES_DETAILS)
}

/// Whether a tag's `attributes` end it: `Some(true)` where its `>` is the last
/// of them, `Some(false)` where more may follow, and `None` where a `<` or a
/// `>` before the last spoils the tag. Neither counts inside a quoted value.
fn ended(attributes: &str) -> Option<bool> {
    let (mut quote, mut valued) = (None, false);
    let mut characters = attributes.chars().peekable();
    while let Some(character) = characters.next() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' if valued => quote = Some(character),
            '>' => return characters.peek().is_none().then_some(true),
            '<' => return None,
            _ => {}
        }
        if !character.is_whitespace() {
            valued = character == '=';
        }
    }
    Some(false)
}

/// The character `character` is written with in a `<sub>` run, or in a
/// `<sup>` run where `raised`, where Unicode has one.
fn scripted(character: char, raised: bool) -> Option<char> {
    const LOWERED: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    const RAISED: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    if let Some(digit) = character.to_digit(10) {
        let digits = if raised { &RAISED } else { &LOWERED };
        return digits.get(usize::try_from(digit).ok()?).copied();
    }
    match (raised, character) {
        (false, '+') => Some('₊'),
        (false, '-') => Some('₋'),
        (false, '=') => Some('₌'),
        (false, '(') => Some('₍'),
        (false, ')') => Some('₎'),
        (false, 'a') => Some('ₐ'),
        (false, 'e') => Some('ₑ'),
        (false, 'o') => Some('ₒ'),
        (false, 'x') => Some('ₓ'),
        (false, 'h') => Some('ₕ'),
        (false, 'k') => Some('ₖ'),
        (false, 'l') => Some('ₗ'),
        (false, 'm') => Some('ₘ'),
        (false, 'n') => Some('ₙ'),
        (false, 'p') => Some('ₚ'),
        (false, 's') => Some('ₛ'),
        (false, 't') => Some('ₜ'),
        (true, '+') => Some('⁺'),
        (true, '-') => Some('⁻'),
        (true, '=') => Some('⁼'),
        (true, '(') => Some('⁽'),
        (true, ')') => Some('⁾'),
        (true, 'n') => Some('ⁿ'),
        (true, 'i') => Some('ⁱ'),
        _ => None,
    }
}

/// Reads markdown out of a stream of deltas.
// Each bool is a fact the others say nothing of: a reader holding no block
// can be inside a `<details>` block with a backslash owed on a started line.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub struct Markdown {
    /// A run of markers whose meaning the next character decides.
    held: Option<HeldMarks>,
    /// Something other than a space has arrived on this line.
    ///
    /// A heading is only one at the start of a line; anywhere else it is a hash.
    started: bool,
    /// The character before the run being held, for the one rule that needs it.
    previous: char,
    /// A bare address being read, from the first letter of its scheme on.
    ///
    /// Held for the same reason a link's words are: an address is one word to
    /// the reader and a run of markers to a scan reading characters. Held from
    /// the first letter rather than from the scheme, because by the time the
    /// scheme is spelled out the letters that spelled it have gone -- so a word
    /// that merely starts like one is held for as long as it could still turn
    /// into one, which is never more than a few characters.
    address: Option<String>,
    /// A number being read, from the hash that opened it on.
    reference: Option<Reference>,
    /// The repository a bare number is counted against, where the session
    /// knows one.
    ///
    /// Optional because a checkout with no forge behind it is an ordinary
    /// place to work, and because a number pointing at the wrong repository is
    /// worse than a number pointing nowhere. Held rather than passed with each
    /// delta, because unlike the room this cannot change while a message is
    /// arriving: the checkout is the one it was when the session opened.
    forge: Option<Forge>,
    /// The checkout a link to a path is read against, where the session knows
    /// one.
    ///
    /// Held for the reason the forge is: the checkout is the one the session
    /// opened in. Without it a path is handed on as it was written.
    files: Option<Files>,
    /// A backslash is standing, waiting to see what it was put in front of.
    ///
    /// Held rather than written, because what it does is decided by the next
    /// character: a marker after it is drawn as itself and the backslash is
    /// gone, and anything else gives the backslash back. It outlives a delta
    /// for the reason everything here does -- a delta is a piece of the wire,
    /// and the pair arrives split as often as not.
    escaped: bool,
    /// A backslash and the break after it are held, with the spaces that have
    /// followed, until the line under it says whether it was blank.
    slashed: Option<escapes::Slashed>,
    /// An `&` and what has followed it, held until it is a whole reference or
    /// is none.
    entity: Option<String>,
    /// The spaces this line opened with, held for whatever follows them.
    ///
    /// Whether they were indentation at all is the first marker's to say: the
    /// ones in front of a fence are part of the fence and go with its line,
    /// and every other marker keeps them, which is what nests one list inside
    /// another. Counted rather than kept, since a space is a space.
    indent: usize,
    line: Line,
    inside: Inside,
    /// The marker the open block was fenced with.
    ///
    /// A block is closed by the marker that opened it and by no other, so a
    /// row of backticks inside a block fenced with tildes is a row of
    /// backticks. Read only while a block is open, which is the only time it
    /// has been written.
    fenced: char,
    /// What the opening fence said the block is written in, while that fence's
    /// own line is still being read.
    language: String,
    /// The reader for this block, where the language was one this build knows.
    ///
    /// `None` for a fence that named nothing, named something unrecognised, or
    /// is not open at all — and then a block is drawn exactly as it was before
    /// any of this existed, quiet and whole.
    syntax: Option<Syntax>,
    /// A link whose shape is still arriving, and the text held back for it.
    link: Option<Link>,
    /// The block of bars being gathered, where one is.
    ///
    /// The only thing here that outlives a line without being an [`Inside`]:
    /// what a table is waiting for is not a character that ends it but a line
    /// that is not one of its own, which is a question asked at the start of
    /// every line rather than settled once.
    table: Option<Table>,
    /// Whether this line may still be watched, as the header of a table
    /// without outer bars or as the title of a setext heading.
    header: Header,
    /// Whether the line being said is to be taken back.
    retract: Retract,
    /// The title of the setext heading the line taken back turned out to be,
    /// to be said again as the heading it is straight after that line.
    retitle: Option<String>,
    /// The heading line being read, as it was written, from its marks on.
    ///
    /// Kept for the rule under a heading of the first or second level, which
    /// is a block and is laid out from this. Bounded by [`MOST`].
    atx: Option<(Level, String)>,
    /// The line of code collected so far, waiting for the break that completes
    /// it.
    ///
    /// A highlighter reads whole lines, and a delta is a piece of the wire — so
    /// the two are joined here, which is the only place that knows where a line
    /// ends. Bounded: past [`MOST`] the line is handed on as it stands and the
    /// rest of it follows plain, because a buffer that grows with the answer is
    /// the one thing this crate may not have.
    code: String,
    /// Which characters a bullet and a quote bar are drawn with.
    ///
    /// The one thing here that is a setting rather than a piece of the scan,
    /// and it is here because a marker that is dropped has to be replaced by
    /// something a font actually has. Carried across the reset at the end of
    /// every line, since it is a fact about the terminal rather than about the
    /// answer.
    glyphs: Glyphs,
    /// What each settled run is a kind of, and what is held while that is
    /// still being decided.
    ///
    /// Outside the scan rather than part of it: it reads what the scan says,
    /// and the scan's state is reset at every line while a word of prose held
    /// at the end of a delta belongs to no line yet.
    kinds: kind::Kinds,
    /// Whether this reader draws every block the way it streams, holding none.
    ///
    /// What lays a block out, and what reads a table's cell: both are reading
    /// something already held, and holding it again would never end. Such a
    /// reader numbers no footnote and reads no `<details>` or `<summary>`,
    /// and a `<br>` is a space to it.
    unheld: bool,
    /// Whether this reader reads neither a backslash before a break nor a
    /// reference.
    ///
    /// What lays out a block shown as written, display math among them: its
    /// words are source, and source is what was written.
    unescaped: bool,
    /// Why the fence being read was not drawn, where it is a held fence that
    /// ran past its cap, so the frame it opens says so.
    ///
    /// Only until that fence's opening line has been read.
    refusal: Option<Undrawn>,
    /// A block that has just closed, waiting to be handed on in its place.
    ready: Option<Block>,
    /// A held block other than a table, being gathered.
    gathering: Option<Gathering>,
    /// Display math past its cap, being shown as written until it ends.
    passing: Option<Passing>,
    /// Whether this line opens with display math, as far as it has arrived.
    probe: Probe,
    /// Whether the fence on this line opened it, and how many marks it took,
    /// for the end of the line to rebuild it from where the block is held.
    fence: Option<(bool, usize)>,
    /// How this line opened, as far as that has arrived.
    ///
    /// Where the line could be one of an indented block — inside one, or
    /// after a blank line where a block may open — its spaces are held here
    /// rather than read, until the first thing that is not a space says how
    /// many of them were the block's.
    lead: Lead,
    /// Whether the line before this one was blank, so a block indented four
    /// spaces may open on this one. Carried across a line, since it is a fact
    /// about the line before.
    above: Above,
    /// Whether an item of a list is open, so a line indented under it is the
    /// item's and not code.
    ///
    /// Opened by a bullet or a number at the start of a line and shut by a
    /// line after a blank one that starts at the margin; carried across lines
    /// until then.
    item: Item,
    /// What the line before this one was part of, so a line that opens with a
    /// quote mark knows whether it starts a quote or goes on with one.
    ///
    /// Carried across a line like [`Markdown::above`]: a line that is no row
    /// of the quote, whatever it is written with, ends it, and an alert with
    /// it.
    quoting: Quoted,
    /// The blank lines inside an indented block that have not been drawn,
    /// because the line after them decides whether they were the block's or
    /// stood after it. Counted rather than kept: a blank line is a blank line.
    gap: usize,
    /// The footnote labels the answer has written, in the order they first
    /// appeared, which is the order they are numbered in.
    ///
    /// Carried across lines and cleared with the message, since a definition
    /// is tied to its reference by its label wherever either stands. Bounded
    /// by [`FOOTNOTE_CAP`] labels, each kept in lower case: at most
    /// [`FOOTNOTE_LABEL_CAP`] characters as written, and at most twice that
    /// once lowered, since no character lowers to more than two.
    notes: Vec<Note>,
    /// The reference definitions the answer has written so far, a label once.
    ///
    /// Carried across lines and cleared with the message, since a reference is
    /// tied to a definition written before it, wherever either stands. Bounded
    /// by [`DEFINITION_CAP`] labels and [`DEFINITION_BYTES`] bytes.
    definitions: Vec<Definition>,
    /// A line that opened with a label and a colon, held until its end says
    /// whether it is a definition.
    defining: Option<Defining>,
    /// A `<` and what has followed it, once it can no longer be one of the six
    /// tags and could still be an address in angle brackets. Bounded by
    /// [`MOST`]; ends with the line.
    angle: Option<String>,
    /// Whether the text being read is text read again, which the heading line
    /// and the watch on the line have been told already as it arrived.
    told: bool,
    /// How many bytes of `angle` arrived as a possible tag, and so have been
    /// seen by the watch on the line already.
    angle_seen: usize,
    /// A `<` and what has followed it, while the characters that say whether
    /// it is one of the tags read here arrive. Bounded by [`TAG_CAP`]; ends
    /// with the line.
    tag: Option<String>,
    /// A `<sub>` or `<sup>` run, held until its closing tag says whether it
    /// can be drawn in sub or superscript characters. Bounded by
    /// [`SCRIPT_CAP`]; ends with the line.
    script: Option<Script>,
    /// Whether a `<details>` block is open, so a paragraph line is drawn two
    /// columns in. Carried across lines until `</details>` or the end of the
    /// message; one closing tag ends any nesting.
    folded: bool,
    /// What is held after single tildes that may open a phrase. See
    /// [`strikes`]; carried across lines, and made on the first read.
    strikes: Option<Rc<RefCell<strikes::Strikes>>>,
    /// Whether the scan about to run reads source as it arrives, rather than
    /// text read again, so it is what the hold of a phrase counts.
    fresh: bool,
}

/// The most of one line of code that is held back to be read.
///
/// Four thousand columns is far past any line anybody wrote and far short of
/// anything that matters to the budget. A line longer than this is drawn plain
/// rather than dropped or held.
const MOST: usize = 4096;

/// The spaces a held indentation is handed on from, taken as a slice rather
/// than built: an indent is a handful of columns and this is the render path.
const SPACES: &str = "                                ";

impl Markdown {
    /// A reader drawing its bullets and quote bars with `glyphs`.
    #[must_use]
    pub fn new(glyphs: Glyphs) -> Self {
        Self {
            glyphs,
            ..Self::default()
        }
    }

    /// Draws what comes next with `glyphs`, leaving whatever the scan has
    /// open where it is: a set can change between two deltas of one answer.
    pub fn draws(&mut self, glyphs: Glyphs) {
        self.glyphs = glyphs;
    }

    /// The same reader, counting bare numbers against `forge`.
    #[must_use]
    pub fn counting(mut self, forge: Option<Forge>) -> Self {
        self.forge = forge;
        self
    }

    /// The same reader, pointing a link to a path at the file in `files`.
    #[must_use]
    pub fn opening(mut self, files: Option<Files>) -> Self {
        self.files = files;
        self
    }

    /// The same reader, counting and opening by `links`.
    #[must_use]
    pub(crate) fn linking(self, links: &Links) -> Self {
        self.counting(links.forge.clone())
            .opening(links.files.clone())
    }

    /// The same reader, holding no block: every one is drawn the way it
    /// streams, as runs.
    ///
    /// What `lay` reads a block's source with, and what a table reads its
    /// cells with.
    #[must_use]
    pub fn as_written(mut self) -> Self {
        self.unheld = true;
        self
    }

    /// This reader, reading neither a backslash before a line break nor a
    /// reference: for a block whose words are shown exactly as they were
    /// written, where `x\` and `a &lt; b` are what they say.
    #[must_use]
    pub(crate) fn unescaped(mut self) -> Self {
        self.unescaped = true;
        self
    }

    /// Whether the line being said may yet be taken back with a
    /// [`Said::Retract`]: it may be the header of a table written without
    /// outer bars or the title of a heading, and the line under it has not
    /// arrived to say so.
    ///
    /// What a caller that puts what it is told out for good, and cannot take
    /// it back, asks after each delta, and holds the line until it is false.
    /// It is true for a paragraph line from its first character, whatever it
    /// opens with, while it is no more than 128 bytes (the header cap), and
    /// after its break until the start of the next line rules out a row of `=`
    /// or `-`: any other character, a mix of the two, a blank line, the end of
    /// the message, or a row past the same cap. A line that opens another
    /// block, a rule or a heading of its own is never in doubt.
    #[must_use]
    pub fn in_doubt(&self) -> bool {
        self.table.as_ref().is_some_and(Table::pending) || matches!(self.probe, Probe::Probing(_))
    }

    /// The held block being gathered, and how many of its lines have begun.
    ///
    /// What the live foot says while one is: the block is not drawn until it
    /// closes, and a reader waiting on nothing visible is owed a word about
    /// why.
    #[must_use]
    pub fn holding(&self) -> Option<Holding> {
        if let Some(table) = self
            .table
            .as_ref()
            .filter(|table| self.holds() && !table.pending())
        {
            return Some(Holding {
                held: Held::Table,
                lines: table.begun(),
            });
        }
        self.gathering.as_ref().map(|gathering| Holding {
            held: gathering.held,
            lines: begun(&gathering.source),
        })
    }

    /// Reads `delta`, handing each run of text to `say` under its slot, and
    /// each block whole once it closes.
    ///
    /// Runs rather than characters: the text between two markers is one call,
    /// so a delta with no markers in it is one call for the whole delta.
    /// Markers themselves are never handed on.
    ///
    /// `room` is how many columns the caller has to draw into. An argument
    /// rather than something this holds, because it is the one fact here that
    /// changes without a delta arriving: a window resized between two of them
    /// would leave a field stale, and the caller knows the size at every call
    /// anyway.
    pub fn read(&mut self, delta: &str, room: usize, say: &mut dyn FnMut(Said<'_>)) {
        let mut kinds = std::mem::take(&mut self.kinds);
        let strikes = self.shared_strikes();
        let mut rest = delta;
        loop {
            self.fresh = true;
            let resume = self.scan(rest, room, &mut |slot, text, link| {
                if !strikes.borrow_mut().keep(slot, text, link) {
                    kinds.tell(slot, text, link, &mut |slot, text, link| {
                        say(Said::Run { slot, text, link });
                    });
                }
            });
            self.hand_on(room, &mut kinds, say);
            match resume {
                Some(at) => rest = rest.get(at..).unwrap_or_default(),
                None => break,
            }
        }
        self.kinds = kinds;
    }

    /// Hands on the block that has just closed, after every run before it, and
    /// the line it takes back before either.
    fn hand_on(&mut self, room: usize, kinds: &mut kind::Kinds, say: &mut dyn FnMut(Said<'_>)) {
        // A phrase is not held across a line taken back or a block.
        if self.retract == Retract::Owed || self.retitle.is_some() || self.ready.is_some() {
            self.unstrike(&mut |slot, text, link| {
                kinds.tell(slot, text, link, &mut |slot, text, link| {
                    say(Said::Run { slot, text, link });
                });
            });
        }
        if self.retract == Retract::Owed {
            self.retract = Retract::Allowed;
            kinds.flush(&mut |slot, text, link| say(Said::Run { slot, text, link }));
            say(Said::Retract);
        }
        // The line taken back was a title: it is said again as a heading,
        // read by a reader that holds nothing, in the words the heading line
        // streams in.
        if let Some(title) = self.retitle.take() {
            let mut heading = Markdown::new(self.glyphs)
                .counting(self.forge.clone())
                .opening(self.files.clone())
                .as_written();
            heading.read(&format!("# {}\n", title.trim()), room, say);
            heading.finish(room, say);
        }
        if let Some(block) = self.ready.take() {
            kinds.flush(&mut |slot, text, link| say(Said::Run { slot, text, link }));
            say(Said::Block(block));
        }
    }

    /// Whether a block arriving now is held rather than drawn as it streams.
    fn holds(&self) -> bool {
        !self.unheld && self.passing.is_none()
    }

    /// Reads `delta` into runs, each under the slot its markers gave it; what
    /// kind of thing a run of prose or code is gets told on the way out.
    ///
    /// Stops where a block has closed, having said every run before it, and
    /// answers where in `delta` to go on from once the block is handed on.
    fn scan(
        &mut self,
        delta: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> Option<usize> {
        // Where the text not yet handed on begins.
        let mut run = 0;
        let fresh = std::mem::take(&mut self.fresh);

        for (at, character) in delta.char_indices() {
            let next = at.saturating_add(character.len_utf8());

            // A block closed on the character before this one, and goes out
            // before anything after it does; so does the line it takes back.
            if self.ready.is_some() || self.retract == Retract::Owed {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                return Some(at);
            }

            if fresh && self.holds_a_phrase() {
                self.feeds(character, say);
            }

            // What a heading is written with is kept for the rule under it.
            if let Some((_, line)) = self.atx.as_mut()
                && character != '\n'
                && !self.told
                && line.len() < MOST
            {
                line.push(character);
            }

            // A block of bars takes every character in it, so nothing below
            // runs while one is open and the run stays empty until it closes.
            if self.table.as_ref().is_some_and(|table| !table.mirrors()) {
                if self.tabled(character, room, say) {
                    run = next;
                    continue;
                }
                if self.ready.is_some() {
                    return Some(at);
                }
                run = at;
            }

            // So does any other block being held.
            if self.gathering.is_some() {
                self.gathers(character, room, say);
                run = next;
                continue;
            }

            if let Some(passing) = self.passing.as_mut()
                && passing.passes(character)
            {
                self.passing = None;
            }

            // A backslash that ends a line, held with its break.
            if self.slashes(character, room, say) {
                run = next;
                continue;
            }

            // How the line opened: the spaces in front of what may be a line
            // of an indented block are held until it is or it is not, and
            // whatever opened the line is read before anything below sees it.
            if self.leads(character, room, say) {
                run = next;
                continue;
            }

            // The start of a line that could be display math is held until
            // it is or it is not.
            if self.probes(character) {
                let pending = delta.get(run..at).unwrap_or_default();
                if self.probed(pending, character, room, say) {
                    run = next;
                    continue;
                }
                run = at;
            }

            // Held text, not a run, for the reason a link's words are: the rest
            // of a line that may be a definition, and a `<` that may be an
            // address. Before the title watch, which would hold the break that
            // ends them, and before the links and addresses below, because what
            // they hand back is read again as if it were arriving, and may
            // leave one of those open for this character to carry on. The run
            // is empty while either is held, so none of it is left behind.
            if self.holds_text(character, room, say) {
                run = next;
                continue;
            }

            // A paragraph line is watched from its first character, and read
            // like any other: it is prose until the line under it says it was
            // the header of a table written without outer bars, or the title of
            // a heading. Its break is the one character held, with the start of
            // the line under it, until that says what it was. After the probe,
            // so that a line that opens like display math and is not is watched
            // from the character it opened with.
            if self.watched(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                run = next;
                continue;
            }

            match self.held {
                // More of the run being held. Not text, and not decided yet.
                Some(held) if held.mark == character => {
                    self.held = Some(HeldMarks {
                        count: held.count.saturating_add(1).min(RUN),
                        ..held
                    });
                    run = next;
                    continue;
                }
                // The run is over, and this character says what it was.
                Some(held) => {
                    if self.settle(held, character, room, say) {
                        run = next;
                        continue;
                    }
                    run = at;
                }
                None => {}
            }

            // Held text, not a run: a link's words are the one thing here that
            // cannot be handed on where they stand, so while one is arriving
            // the run is empty and every character is decided below.
            if self.link.is_some() {
                if self.links(character, room, say) {
                    run = next;
                    continue;
                }
                // Not a link after all. What was held has gone back into the
                // stream and this character is ordinary, so it starts the run.
                run = at;
            }

            // A backslash is standing and this is the character it was put in
            // front of.
            // Held text, not a run, for the reason a link's words are.
            if self.address.is_some() {
                if self.addressed(character, say) {
                    run = next;
                    continue;
                }
                // Not an address after all, or one that has just ended. What
                // was held has been drawn or put back, and this character is
                // ordinary, so it starts the run.
                run = at;
            }

            // Held text, not a run, for the reason an address is: a number
            // after a hash, a tag, and a reference. When what was held is
            // settled it has been drawn or put back, and this character starts
            // the run.
            if self.holds_word(character, room, say) {
                run = next;
                continue;
            }

            // A backslash in front of a `<` keeps it from opening a tag, and
            // stays itself.
            let mut kept = false;
            if std::mem::take(&mut self.escaped) {
                if Self::escapes(character) {
                    self.say(delta.get(at..next).unwrap_or_default(), say);
                    self.started = true;
                    self.previous = character;
                    run = next;
                    continue;
                }

                // In front of nothing this scan would have acted on, so it was
                // a character of the answer rather than a word about the next
                // one. The run starts at this character, which is where the
                // backslash left it.
                self.say("\\", say);
                self.started = true;
                self.previous = '\\';
                kept = true;
            }

            if character == '\n' {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.end_line(room, say);
                run = next;
            } else if self.script.is_some() && character != '<' && !self.opens_entity(character) {
                run = if self.raises(character, say) {
                    next
                } else {
                    at
                };
            } else if self.line.key && character != '<' {
                // What a key holds is written as it is: no marker, bracket or
                // backslash inside one means anything.
                self.previous = character;
            } else if !kept && self.opens_tag(character) {
                // Spaces in front of a tag that opens the line are its
                // indentation, as they are a marker's.
                if self.started {
                    self.say(delta.get(run..at).unwrap_or_default(), say);
                } else {
                    self.indent += delta.get(run..at).map_or(0, str::len);
                }
                self.tag = Some(String::from(character));
                run = next;
            } else if !kept && self.opens_entity(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.entity = Some(String::from(character));
                self.started = true;
                run = next;
            } else if self.opens_link(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.link = Some(Link {
                    first: !self.started,
                    ..Link::default()
                });
                self.started = true;
                run = next;
            } else if self.opens_table(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.leaves_quote(character);
                self.table = Some(Table::opening());
                self.tabled(character, room, say);
                run = next;
            } else if self.opens_address(character, delta.get(at..).unwrap_or_default()) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.address = Some(String::from(character));
                run = next;
            } else if let Some(back) =
                self.opens_reference(character, delta.get(run..at).unwrap_or_default())
            {
                // What the repository was named with is the reference's, not
                // the prose's: it goes into the hold rather than out, so the
                // words the reader clicks are the words the answer wrote.
                let pending = delta.get(run..at).unwrap_or_default();
                let (text, slug) = pending.split_at(pending.len() - back);
                // The word saying what the number counts is the reference's
                // too, where the answer wrote one, and for the same reason.
                let (text, lead) = text.split_at(text.len() - lead(text));
                self.say(text, say);
                self.reference = Some(Reference {
                    lead: (!lead.is_empty()).then(|| lead.into()),
                    slug: (!slug.is_empty()).then(|| slug.into()),
                    number: String::new(),
                });
                self.started = true;
                run = next;
            } else if self.escaping(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.escaped = true;
                run = next;
            } else if self.marks(character) {
                // Nothing has been written on the line yet, so what stands in
                // front of this marker is the whitespace that indented it, and
                // it is held rather than said until the marker says what it was.
                if self.started {
                    self.say(delta.get(run..at).unwrap_or_default(), say);
                } else {
                    self.indent += delta.get(run..at).map_or(0, str::len);
                }
                self.held = Some(HeldMarks {
                    mark: character,
                    count: 1,
                    opened: !self.started,
                });
                run = next;
            } else if matches!(self.inside, Inside::Opening | Inside::Closing) {
                // What is left of a fence's own line names the language it is
                // written in, or nothing at all. Either way it is not the
                // answer, so it goes the way the fence went — but the opening
                // one is kept, because it is what decides how the block is read.
                if self.inside == Inside::Opening && self.language.len() < MOST {
                    self.language.push(character);
                }
                run = next;
            } else {
                self.started |= character != ' ';
                self.previous = character;
            }
        }

        self.say(delta.get(run..).unwrap_or_default(), say);
        self.ready.is_some().then_some(delta.len())
    }

    /// Whether `character` may open display math, or carry on one that may.
    fn probes(&self, character: char) -> bool {
        match self.probe {
            Probe::Probing(_) => true,
            Probe::Done => false,
            Probe::Ready => {
                matches!(character, '$' | '\\')
                    && self.holds()
                    && self.inside == Inside::Prose
                    && !self.started
                    && !self.line.code
                    && self.indent == 0
                    && self.held.is_none()
                    && self.link.is_none()
                    && self.address.is_none()
                    && self.reference.is_none()
                    && self.tag.is_none()
                    && !self.escaped
            }
        }
    }

    /// Reads `character` into what the line opened with, `pending` being the
    /// spaces in front of it not yet said. Answers whether it was taken.
    ///
    /// `false` says the line is not display math after all: what was held
    /// has been read again as it would have been, and `character` is the
    /// caller's.
    fn probed(
        &mut self,
        pending: &str,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let mut written = match std::mem::take(&mut self.probe) {
            Probe::Probing(written) => written,
            Probe::Ready | Probe::Done => pending.to_owned(),
        };
        let held = written.len();
        written.push(character);
        match opener(&written) {
            Opener::Partly => {
                self.probe = Probe::Probing(written);
                true
            }
            Opener::Whole(closer) => {
                self.probe = Probe::Done;
                // Display math is no paragraph line, whatever stood in front
                // of it.
                self.leaves_quote(character);
                self.table = None;
                self.gathering = Some(Gathering {
                    held: Held::DisplayMath,
                    line: written.len(),
                    source: written,
                    closer: Closer::Within(closer),
                });
                true
            }
            Opener::Not => {
                self.probe = Probe::Done;
                written.truncate(held);
                // What stood in front of the line is read again with the rest
                // of it, and watched again.
                if self.table.take_if(|table| table.mirrors()).is_some() {
                    self.header = Header::Open;
                }
                self.scan(&written, room, say);
                false
            }
        }
    }

    /// Reads `character` into the held block being gathered.
    fn gathers(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let Some(mut gathering) = self.gathering.take() else {
            return;
        };
        gathering.source.push(character);

        // Held longer than the block is worth waiting for. What was gathered
        // is read again as it would have been had nothing held it, and the
        // rest of it follows the same way.
        if gathering.source.len() > gathering.held.cap() {
            let unheld = std::mem::replace(&mut self.unheld, true);
            let refusal = std::mem::replace(
                &mut self.refusal,
                matches!(gathering.closer, Closer::Fence(_)).then_some(Undrawn::PastCap),
            );
            self.scan(&gathering.source, room, say);
            self.unheld = unheld;
            self.refusal = refusal;

            // A fence's own state says where it ends. Display math has none,
            // so its closer is watched for until the line holding it ends.
            if let Closer::Within(closer) = gathering.closer {
                let mut passing = Passing {
                    closer,
                    tail: String::new(),
                    seen: false,
                };
                let line = gathering.source.get(gathering.line..).unwrap_or_default();
                // Every character is fed, since what it has seen is kept.
                let mut ended = false;
                for character in line.chars() {
                    ended |= passing.passes(character);
                }
                if !ended {
                    self.passing = Some(passing);
                }
            }
            return;
        }

        if character == '\n' {
            let line = gathering.source.get(gathering.line..).unwrap_or_default();
            if gathering.closer.closes(line) {
                self.ready = Some(Block {
                    kind: gathering.held.kind(),
                    source: gathering.source,
                    closed: true,
                });
                return;
            }
            gathering.line = gathering.source.len();
        }
        self.gathering = Some(gathering);
    }

    /// Whether a line opening here may be a line of an indented block, so the
    /// spaces in front of it are held until it is or it is not.
    ///
    /// Inside one, always. Otherwise only in prose after a blank line, since a
    /// paragraph's next line is the paragraph's however far in it starts, and
    /// not under an item of a list, whose lines are indented to be its own.
    fn indents(&self) -> bool {
        self.inside == Inside::Indented
            || (self.inside == Inside::Prose
                && self.above == Above::Blank
                && self.item == Item::Shut
                && self.passing.is_none())
    }

    /// Reads `character` into how the line opened. Answers whether it was
    /// taken: held, or read already with the spaces held in front of it.
    fn leads(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        match self.lead {
            Lead::Margin(spaces) if character == ' ' => {
                self.lead = Lead::Margin(spaces.saturating_add(1));
                self.indents()
            }
            Lead::Margin(spaces) => {
                self.lead = Lead::Read;
                self.margin(spaces, character, room, say)
            }
            Lead::Number(digits) => {
                self.lead = match character {
                    '0'..='9' if digits < NUMBERED => Lead::Number(digits.saturating_add(1)),
                    '.' | ')' => Lead::Stopped,
                    _ => Lead::Read,
                };
                false
            }
            Lead::Stopped => {
                if character == ' ' {
                    // A numbered item is a paragraph of its own, as a bullet
                    // is, so a phrase opened by a single tilde above it ends.
                    self.unstrike(say);
                    self.item = Item::Open;
                }
                self.lead = Lead::Read;
                false
            }
            Lead::Read => false,
        }
    }

    /// Reads the first character of a line that is not a space, with `spaces`
    /// in front of it. Answers whether it was taken.
    ///
    /// Four spaces or more inside an indented block carry it on, and what is
    /// past the fourth is the code's own; fewer end it on any line but a blank
    /// one. After a blank line, four or more open one, its top edge said here
    /// before anything on the line.
    fn margin(
        &mut self,
        spaces: usize,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        // Worked out before anything below changes what it asks.
        let held = spaces > 0 && self.indents();

        // Where a line indented by spaces says whether it is a row of the
        // quote carried: a quote mark here leaves that to the mark reader, and
        // anything else is no row of it.
        if spaces > 0 {
            self.leaves_quote(character);
        }

        if self.inside == Inside::Indented {
            // Whether a blank line is the block's is the next line's to say.
            if character == '\n' {
                self.gap = self.gap.saturating_add(1);
                self.lead = Lead::Margin(0);
                return true;
            }
            if spaces >= CODE {
                for _ in 0..std::mem::take(&mut self.gap) {
                    say(Slot::Quiet, chrome::gutter(self.glyphs), None);
                    say(Slot::Quiet, "\n", None);
                }
                self.indent = spaces - CODE;
                return false;
            }
            self.shuts(say);
        }
        if self.inside != Inside::Prose {
            return false;
        }

        if held && spaces >= CODE && character != '\n' {
            let frame = Frame {
                label: Label::new("", self.glyphs),
                refusal: None,
            };
            say(Slot::Quiet, &frame.top(), None);
            say(Slot::Quiet, "\n", None);
            self.inside = Inside::Indented;
            self.above = Above::Written;
            self.indent = spaces - CODE;
            return false;
        }

        if character != '\n' {
            // A line at the margin after a blank one is no item's.
            if self.above == Above::Blank && spaces == 0 {
                self.item = Item::Shut;
            }
            self.above = Above::Written;
        }
        let taken = held && self.replays(spaces, character, room, say);
        if character.is_ascii_digit() {
            self.lead = Lead::Number(1);
        }
        taken
    }

    /// Reads `spaces` held in front of `character` again, as they would have
    /// been read had nothing held them. Answers whether `character` was read
    /// with them.
    ///
    /// A break is not: the spaces in front of one are a blank line's, which
    /// nothing claims. Anything else is, because it decides what they were --
    /// an item's indentation, or part of a rule's or a fence's line.
    fn replays(
        &mut self,
        spaces: usize,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        if character == '\n' {
            self.blanks(spaces, room, say);
            return false;
        }
        // Fewer than the four that would have opened a block.
        let mut written = String::from(SPACES.get(..spaces).unwrap_or_default());
        written.push(character);
        self.scan(&written, room, say);
        true
    }

    /// Reads `spaces` held at the start of a line again, with nothing after
    /// them.
    fn blanks(
        &mut self,
        spaces: usize,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let mut left = spaces;
        while left > 0 {
            let some = left.min(SPACES.len());
            self.scan(SPACES.get(..some).unwrap_or_default(), room, say);
            left -= some;
        }
    }

    /// Ends the indented block being read, on a line that is not its own: its
    /// bottom edge, then the blank lines that stood between it and this line.
    fn shuts(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.inside = Inside::Prose;
        say(Slot::Quiet, chrome::bottom(self.glyphs), None);
        say(Slot::Quiet, "\n", None);
        let gap = std::mem::take(&mut self.gap);
        for _ in 0..gap {
            say(Slot::Plain, "\n", None);
        }
        self.above = if gap > 0 {
            Above::Blank
        } else {
            Above::Written
        };
    }

    /// Lets go of the spaces a line the message ended on opened with, and of
    /// the indented block it ended in.
    fn unindent(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let Lead::Margin(spaces) = self.lead else {
            return;
        };
        let held = spaces > 0 && self.indents();
        self.lead = Lead::Read;
        if self.inside == Inside::Indented {
            self.shuts(say);
        }
        if held {
            self.blanks(spaces, room, say);
        }
    }

    /// Whether `character` starts a run of markers where the scan stands.
    fn marks(&self, character: char) -> bool {
        match self.inside {
            // Inside a fence the only marker left is the fence that closes it,
            // and only at the start of a line. Code is full of the others.
            Inside::Fence => character == self.fenced && !self.started,
            // A fence's own line is dropped whole; nothing on it marks anything.
            // An indented block has no fence to close it, and is code like any
            // other.
            Inside::Opening | Inside::Closing | Inside::Indented => false,
            Inside::Prose => match character {
                '`' => true,
                // Inside a span the backtick above is the only marker left,
                // for the reason a fence is: the words in there are code, and
                // code is full of the others. `*ptr` is a pointer, `_private`
                // is a name, and `**kwargs` is how one language spells an
                // argument -- every one of them a thing a coding agent writes
                // far more often than it writes emphasis inside a span.
                _ if self.line.code => false,
                '*' | '_' | '~' => true,
                // Only where nothing else has been written on the line. A
                // hash is a heading there and a comment everywhere else; a
                // dash is a bullet there and a minus sign everywhere else.
                '#' | '-' | '+' | '>' => !self.started,
                _ => false,
            },
        }
    }

    /// Whether `character` is a backslash standing in front of something.
    ///
    /// Prose only. In code a backslash is a character like any other, and a
    /// span is where a pattern goes to be written down exactly.
    fn escaping(&self, character: char) -> bool {
        character == '\\' && self.inside == Inside::Prose && !self.line.code
    }

    /// Whether a standing backslash was put in front of this to say it means
    /// nothing.
    ///
    /// Only what this scan would otherwise have acted on, rather than every
    /// piece of punctuation a stricter reading escapes: `\d` and `\.` are a
    /// pattern far more often than they are prose that meant a `d` or a stop,
    /// and dropping the backslash there would change what somebody copies off
    /// the screen into something that no longer matches.
    fn escapes(character: char) -> bool {
        matches!(
            character,
            '\\' | '*' | '_' | '`' | '~' | '#' | '-' | '+' | '>' | '|' | '['
        )
    }

    /// Decides what a finished run of markers was, given the character that
    /// ended it. Answers whether that character was part of the marker.
    fn settle(
        &mut self,
        held: HeldMarks,
        next: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        self.held = None;
        self.started = true;

        match held.mark {
            // Three or more of one marker with nothing else on the line: a
            // rule between the blocks either side of it. Drawn across the room
            // the caller has rather than as the three characters it was
            // written with, because what the model meant by them is a
            // separator and a separator that stops after three columns reads
            // as a stray. This stands above every arm below it: a line that is
            // nothing but markers cannot also be emphasis or a bullet. Under a
            // paragraph line it is not reached at all: the watch of that line
            // has taken `-` and `=` for the underline of a heading.
            //
            // Handed on as a block where blocks are held, so whoever keeps it
            // can draw it across the room there is later rather than the room
            // there was.
            '-' | '*' | '_' if held.opened && held.count >= 3 && next == '\n' => {
                self.indent = 0;
                if self.holds() {
                    self.ready = Some(Block {
                        kind: Kind::Rule,
                        source: written(held).to_owned(),
                        closed: true,
                    });
                    return false;
                }
                for _ in 0..room {
                    say(Slot::Quiet, self.glyphs.horizontal(), None);
                }
                false
            }
            // The hashes and the one space after them are the marker, and what
            // is left of the line is the heading itself.
            '#' if next == ' ' => {
                self.opens_block(say);
                self.line.heading = true;
                let level = match held.count {
                    1 => Some(Level::One),
                    2 => Some(Level::Two),
                    _ => None,
                };
                if let (true, Some(level)) = (self.holds(), level) {
                    // Without the blanks in front of it: the rule is as wide
                    // as the words, as a setext heading's is.
                    let mut line = String::from(written(held));
                    line.push(' ');
                    self.atx = Some((level, line));
                }
                true
            }
            // Three or more open a block that outlives the line it is on, or
            // close the one already open.
            // Either marker, because a model reaches for tildes exactly when
            // the block is full of backticks -- and a block read as prose is
            // code with its markers taken out of it, which is the worst thing
            // this reader can do to an answer.
            '`' | '~' if held.count >= 3 && self.fences(held.mark) => {
                // A fence's own line goes whole, and the spaces in front of it
                // are part of that line: an item's block would otherwise open
                // with the item's indentation stitched to its first row.
                self.indent = 0;
                self.inside = if self.inside == Inside::Fence {
                    Inside::Closing
                } else {
                    self.fenced = held.mark;
                    self.fence = Some((held.opened, held.count));
                    Inside::Opening
                };
                false
            }
            '`' if self.inside == Inside::Prose => {
                self.line.code = !self.line.code;
                false
            }
            // A bullet, and the space after it, are the marker — so what is
            // drawn in their place is a mark and a space of this crate's own,
            // out of the set the rest of the interface is drawn from. The
            // indentation before it has already gone out, so a nested item
            // stays nested. Owed rather than drawn here: see [`Line::owed`].
            '-' | '+' | '*' if held.opened && held.count == 1 && next == ' ' => {
                self.opens_block(say);
                self.line.marked = Marked::Owed;
                self.item = Item::Open;
                true
            }
            // A quote is a bar down the left and the words beside it, which is
            // what a reader already knows a quote looks like. The bar is
            // quiet, and the words are a quote's, which is quiet too unless
            // the design says otherwise: the point of a quote is that the
            // words are somebody else's.
            //
            // The first line of a quote owes its bar until its line says what
            // the quote is: an alert's header has a bar of its own. A line
            // that goes on with a quote knows already.
            '>' if held.opened && held.count == 1 && next == ' ' => {
                self.opens_block(say);
                self.line.quoted = match self.quoting {
                    Quoted::Plain => Quoted::Plain,
                    Quoted::Alert => Quoted::Alert,
                    Quoted::Not | Quoted::Owed => Quoted::Owed,
                };
                self.spend(say);
                if self.line.quoted != Quoted::Owed {
                    self.bar(say);
                }
                true
            }
            // Exactly two, because that is the only run markdown has ever
            // meant a retraction by -- which is also what keeps `~/Projects` a
            // path: one tilde falls through and is written back as itself,
            // and three are the fence handled above.
            '~' if held.count == 2 && self.strikes(next) => {
                self.line.emphasis.struck = !self.line.emphasis.struck;
                false
            }
            // One, in prose that is not shown as written: see [`strikes`].
            '~' if held.count == 1 && !self.unescaped && self.inside == Inside::Prose => {
                self.tilde(held, next, say);
                false
            }
            // One marker is emphasis and two are weight, which is what markdown
            // has always meant by them and what a model writes expecting to be
            // read that way. Three or more are both, and shown as both.
            '*' | '_' if self.emphasises(held, next) => {
                let emphasis = &mut self.line.emphasis;
                match held.count {
                    1 => emphasis.leant = !emphasis.leant,
                    2 => emphasis.raised = !emphasis.raised,
                    // Closes whichever of the two is open, or opens both.
                    _ if emphasis.raised != emphasis.leant => {
                        *emphasis = Emphasis {
                            struck: emphasis.struck,
                            ..Emphasis::default()
                        }
                    }
                    _ => {
                        emphasis.raised = !emphasis.raised;
                        emphasis.leant = !emphasis.leant;
                    }
                }
                false
            }
            _ => {
                self.previous = held.mark;
                self.say(written(held), say);
                false
            }
        }
    }

    /// Drops the emphasis carried out of the line before this one.
    ///
    /// A block mark opens something of its own, and what it opens is not the
    /// paragraph an unclosed marker was left open in, nor the paragraph a
    /// phrase is held in. See [`Line`].
    fn opens_block(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.line.emphasis = Emphasis::default();
        self.unstrike(say);
    }

    /// Whether a run of three or more `mark` is this line's fence.
    ///
    /// One opens a block wherever a block is not already open, and closes only
    /// the block its own marker opened. See [`Markdown::fenced`].
    fn fences(&self, mark: char) -> bool {
        self.inside != Inside::Fence || self.fenced == mark
    }

    /// Whether `character` opens a block of bars where the scan stands.
    ///
    /// Only where nothing else has been written on the line, and not inside a
    /// span of code: a bar in the middle of a line is `a | b` in a shell and
    /// `Ok(_) | Err(_)` in a match, and neither is a table.
    fn opens_table(&self, character: char) -> bool {
        character == '|' && self.inside == Inside::Prose && !self.started && !self.line.code
    }

    /// Whether `character` starts a line that could be the header of a table
    /// written without outer bars.
    ///
    /// Only the first character of a line nothing else has claimed: after
    /// spaces, a marker or a bullet it is `a | b` in a list item or a quote,
    /// which is the prose it was. Not one a marker or a bar would have opened,
    /// since those are another reading of the same line.
    fn opens_bare(&self, character: char) -> bool {
        self.holds()
            && self.header == Header::Open
            && self.inside == Inside::Prose
            && !self.started
            && self.previous == '\0'
            && self.indent == 0
            && !self.line.code
            && self.held.is_none()
            && self.link.is_none()
            && self.address.is_none()
            && self.reference.is_none()
            && self.tag.is_none()
            && !self.escaped
            && !character.is_whitespace()
            && !matches!(
                character,
                '`' | '*' | '_' | '~' | '#' | '-' | '+' | '>' | '[' | '\\' | '|' | '$'
            )
    }

    /// Whether `character` starts a paragraph line, which is watched as the
    /// header of a table written without outer bars and as the title of a
    /// heading.
    ///
    /// Of the line and of what is open, what [`Markdown::opens_bare`] asks;
    /// of the character, only that it is not a bar, which opens a block of
    /// bars, and not a break, which opens nothing. A marker, a space and a
    /// dollar open a title as any other does, since what a line is written
    /// with does not say what it was a title of.
    fn opens_title(&self, character: char) -> bool {
        self.table.is_none() && !matches!(character, '\n' | '|') && self.opens_bare('a')
    }

    /// Reads `character` into the block of bars being held. Answers whether it
    /// belonged to the block.
    ///
    /// `false` says the block is over: what it turned out to be has already
    /// gone out, and `character` is the caller's again — the first character of
    /// a line that is not part of any table, which is why the line state goes
    /// back to what it is at the start of one.
    fn tabled(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut table) = self.table.take() else {
            return false;
        };

        if table.is_bare() {
            return self.bared(table, character, room, say);
        }

        // A line that does not open with a bar is not the table's, and the
        // table ended above it.
        if table.fresh() && character != '|' {
            self.lays(&table, room, say);
            self.line = Line::default();
            self.started = false;
            self.lead = Lead::default();
            return false;
        }

        // Held longer than a table is worth waiting for. Out it goes as the
        // model wrote it, and the rest of the block is read as the prose it
        // now is.
        if !table.takes(character) {
            table.spilt(say);
            return true;
        }

        // The delimiter row is the second line or there is no table here.
        if !table.possible() {
            table.spilt(say);
            return true;
        }

        self.table = Some(table);
        true
    }

    /// Watches `character`, which the scan reads all the same, as part of the
    /// header of a table written without outer bars. Answers whether it is the
    /// header's break, which is held back rather than read.
    ///
    /// A line too long to be a header or a title is prose, and nothing is
    /// watched past it. A line with fewer than two cells is no table header,
    /// but it is still watched as the title of a heading, so only the table
    /// side of the watch stops there.
    fn mirrored(&mut self, character: char) -> bool {
        let Some(mut table) = self.table.take() else {
            return false;
        };
        if table.sees(character) != Sight::Keep {
            // Not watched a second time on the same line.
            self.header = Header::Declined;
            return false;
        }

        self.table = Some(table);
        character == '\n'
    }

    /// [`Markdown::tabled`] for a block that opened without a bar, once its
    /// header is whole. Answers whether `character` belonged to the block.
    ///
    /// Until the delimiter row is whole, what the second line has spelled is
    /// held; a line that turns out not to be one is read as the prose it is,
    /// from the header's break on, and `character` is the caller's. The
    /// delimiter row takes the header back from whoever was told it as prose.
    fn bared(
        &mut self,
        mut table: Table,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        if character == '\n'
            && let Some((rows, line)) = self.closed_under(&table)
        {
            self.lays(&Table::of(&rows), room, say);
            self.line = Line::default();
            self.started = false;
            self.reads(CLOSES_DETAILS, &line, say);
            return false;
        }
        let lines = table.lined();
        match table.sees(character) {
            Sight::Keep => {
                if lines == 1 && table.lined() == 2 {
                    self.retract = Retract::Owed;
                    self.forgets_line();
                }
                self.table = Some(table);
                true
            }
            Sight::Over => {
                self.lays(&table, room, say);
                self.line = Line::default();
                self.started = false;
                false
            }
            Sight::Under(level) => {
                self.titled(&table, level);
                true
            }
            Sight::Spill => {
                table.spilt(say);
                self.header = Header::Declined;
                true
            }
            Sight::Release => {
                self.release(&table, room, say);
                // What the second line began may have opened a block of its
                // own, which then has the character: a header to watch, or a
                // block of bars.
                if self.table.as_ref().is_some_and(Table::mirrors) {
                    self.mirrored(character)
                } else {
                    self.table.is_some() && self.tabled(character, room, say)
                }
            }
        }
    }

    /// What a bare table's body holds before the line it is filling, and that
    /// line, where the line is nothing but the tag that closes the
    /// `<details>` block the table stands in: it ends the table, and is no row
    /// of it.
    fn closed_under(&self, table: &Table) -> Option<(String, String)> {
        if !self.folded || table.lined() < 2 {
            return None;
        }
        let source = table.source();
        let (rows, line) = source.rsplit_once('\n')?;
        closes_details(line).then(|| (format!("{rows}\n"), line.to_owned()))
    }

    /// Takes the line a row of `=` or `-` stood under back, to be said again as
    /// the heading it is, with the rule under it handed on as a block.
    ///
    /// The row itself was only ever watched: it has not been said, and is not.
    fn titled(&mut self, table: &Table, level: Level) {
        self.retract = Retract::Owed;
        self.retitle = table.title().map(str::to_owned);
        self.ready = Some(Block {
            kind: Kind::Heading(level),
            source: table.source(),
            closed: true,
        });
        self.forgets_line();
    }

    /// Reads what a header's table held back as the prose it always was: the
    /// header's break, and the start of the line under it.
    fn release(
        &mut self,
        table: &Table,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        if let Some(held) = table.held() {
            self.scan(&held, room, say);
        }
    }

    /// Forgets what the line being read had begun, without saying it.
    ///
    /// For a line that is no longer prose being read as such: it is the header
    /// of a table or the title of a heading, which keep their source and are
    /// laid out from that.
    fn forgets_line(&mut self) {
        self.held = None;
        self.link = None;
        self.address = None;
        self.reference = None;
        self.entity = None;
        self.defining = None;
        self.angle = None;
        self.tag = None;
        self.script = None;
        self.escaped = false;
        self.indent = 0;
        self.previous = '\0';
        self.started = false;
        self.line = Line::default();
        self.lead = Lead::default();
        self.probe = Probe::default();
        self.atx = None;
    }

    /// Lets go of a table that has ended: as a block where blocks are held
    /// and it has the delimiter row that makes it one, and drawn as it stands
    /// otherwise.
    fn lays(&mut self, table: &Table, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.holds() && table.delimited() {
            self.ready = Some(Block {
                kind: Kind::Table,
                source: table.source(),
                closed: true,
            });
        } else {
            table.laid(self.glyphs, room, say);
        }
    }

    /// Draws the bullet's own mark, where one is owed, and the space a note's
    /// number owes its text.
    ///
    /// Called immediately before the first thing on the item reaches the
    /// terminal, whichever path that is — a run of prose, a link that arrived
    /// whole, a bracket that turned out not to be one, or the break of an item
    /// with nothing after its marker at all.
    fn pay(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.broke(say);
        self.pay_bar(say);
        self.fold(say);
        if std::mem::take(&mut self.line.gap) {
            say(self.slot(), " ", None);
        }
        if self.line.marked != Marked::Owed {
            return;
        }

        self.line.marked = Marked::Drawn;
        say(Slot::Bullet, self.glyphs.bullet(), None);
        say(Slot::Quiet, " ", None);
    }

    /// Draws the bar of a quote's first line, where it is owed.
    ///
    /// Paid wherever a bullet's mark is, and before it: the bar is the
    /// outermost thing on the row.
    fn pay_bar(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.line.quoted == Quoted::Owed {
            self.line.quoted = Quoted::Plain;
            self.bar(say);
        }
    }

    /// Ends the quote the line before was part of, and an alert with it, when
    /// `first` shows the line to be no row of one.
    ///
    /// `first` is what each caller has when it knows what the line is: the
    /// first character of the line that is not a space for a header, a table
    /// of bars and a line indented by spaces, and for display math the
    /// character that completes its opener, the second `$` of `$$`, the `[` of
    /// `\[` or the `}` of `\begin{…}`. Said by each place that opens a block
    /// taking its lines whole: those lines never reach [`Markdown::end_line`],
    /// which is where a line says what it was part of, so a quote row after the
    /// block would go on with a quote that the block has already ended. A quote
    /// mark is left alone: the mark reader decides whether the line is a row of
    /// the quote still carried.
    fn leaves_quote(&mut self, first: char) {
        if first != '>' && self.line.quoted == Quoted::Not {
            self.quoting = Quoted::Not;
        }
    }

    /// Draws the bar down the left of a line of a quote, and the space that
    /// parts it from the words.
    fn bar(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let bar = match self.line.quoted {
            Quoted::Alert => self.glyphs.alerted(),
            _ => self.glyphs.vertical(),
        };
        say(Slot::Quiet, bar, None);
        say(Slot::Quiet, " ", None);
    }

    /// Whether what is held at a line's end is the whole of an alert's marker,
    /// on the first line of a quote.
    fn alerts(&self, link: &Link) -> Option<Alert> {
        (self.line.quoted == Quoted::Owed && link.part == Part::Between)
            .then(|| Alert::of(&link.label))
            .flatten()
    }

    /// Draws the header row of an alert: its bar, its mark and its word.
    ///
    /// The marker is not shown; the row says what it meant.
    fn headline(&mut self, alert: Alert, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.line.quoted = Quoted::Alert;
        self.bar(say);
        say(alert.slot(), alert.mark(self.glyphs), None);
        say(Slot::Plain, " ", None);
        say(Slot::Bold, alert.word(), None);
    }

    /// Whether what a bullet is holding is a task's box rather than a link.
    ///
    /// Only where the mark is still owed, so `- [ ] a thing` is a task and
    /// `- see [ ] in the grammar` is a bracket somebody wrote. `character` is
    /// the space that follows the box, which is part of the marker and goes
    /// with it.
    fn boxed(&self, link: &Link, character: char) -> bool {
        self.line.marked == Marked::Owed
            && link.part == Part::Between
            && character == ' '
            && matches!(link.label.as_str(), "" | " " | "x" | "X")
    }

    /// Draws a task's box in place of the bullet's mark, and says whether the
    /// task is one somebody has finished.
    fn tick(&mut self, label: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.pay_bar(say);
        if matches!(label, "x" | "X") {
            self.line.marked = Marked::Done;
            say(Slot::DoneMark, self.glyphs.done(), None);
        } else {
            self.line.marked = Marked::Drawn;
            say(Slot::Quiet, self.glyphs.open(), None);
        }

        say(Slot::Quiet, " ", None);
    }

    /// Whether `character` opens a link where the scan stands.
    ///
    /// Prose only, and not inside a span of code: a bracket in `` `arr[0]` ``
    /// is an index somebody wrote, and reading it as a link would take the
    /// index off the screen.
    fn opens_link(&self, character: char) -> bool {
        character == '[' && self.inside == Inside::Prose && !self.line.code
    }

    /// Reads `character` into the link being held. Answers whether it belonged
    /// to the link.
    ///
    /// `false` says the shape was not a link: what was held has already gone
    /// back out as the text it turned out to be, and `character` is the
    /// caller's again.
    fn links(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut link) = self.link.take() else {
            return false;
        };

        // A link does not cross a line break. Markdown allows one inside the
        // brackets; a model that opened a bracket and never closed it is the
        // case that actually happens, and it costs one line here rather than
        // the rest of the answer. Past the bound, the same.
        let carries = character != '\n' && link.holds();

        match (link.part, character) {
            (Part::Label, ']') if carries => link.part = Part::Between,
            (Part::Between, '(') if carries => link.part = Part::Target,
            // A second label only where there is a definition to name: with
            // none, the bracket after `]` is as ordinary as it ever was.
            (Part::Between, '[') if carries && self.reads_definitions() => {
                link.part = Part::Second;
            }
            (Part::Between, ':') if carries && self.opens_definition(&link) => {
                self.defining = Some(Defining {
                    label: link.label,
                    rest: String::new(),
                });
                return true;
            }
            (Part::Target, ')') if carries => {
                self.wrote_link(&link, say);
                return true;
            }
            (Part::Second, ']') if carries => {
                self.wrote_reference_link(&link, room, say);
                return true;
            }
            (Part::Label, held) if carries => link.label.push(held),
            (Part::Target | Part::Second, held) if carries => link.target.push(held),
            // A second label that is not one: it is read again, below.
            (Part::Second, _) => {
                self.retaken(&link, character, room, say);
                return true;
            }
            // The marker of an alert is the whole of its line. The break
            // is the character that says so, and is the caller's again: the
            // header is drawn in place of the marker and the line ends as any
            // other does.
            (Part::Between, '\n') if self.alerts(&link).is_some() => {
                if let Some(alert) = self.alerts(&link) {
                    self.headline(alert, say);
                }
                return false;
            }
            // A box takes the bullet's mark rather than following it, which is
            // why this stands above the arm that puts a bracket back: the
            // space after `]` is the marker's own and goes with it.
            _ if self.boxed(&link, character) => {
                self.tick(&link.label, say);
                return true;
            }
            // `[TODO] fix this`, and every other bracket somebody meant,
            // where it is no footnote.
            _ => {
                if let Some(taken) = self.cites(&link, character, say) {
                    return taken;
                }
                if self.ties(&link, say) {
                    return false;
                }
                self.pay(say);
                spill(&link, say);
                return false;
            }
        }

        self.link = Some(link);
        true
    }

    /// Whether an address could start here.
    ///
    /// At the start of a word only: `shttps://` is not an address, and neither
    /// is the second half of one a link already named. `www.` too, but where
    /// the word before it ended, since it is found in more places.
    ///
    /// `ahead` is what this delta has from `character` on. A `w` that this
    /// delta already shows to be the start of some other word is not held,
    /// which would split the word into two runs for nothing.
    fn opens_address(&self, character: char, ahead: &str) -> bool {
        let starts = if character == 'w' {
            // Narrower than a scheme's: a `www.` is read after a space, an
            // opening bracket or the mark of emphasis, and a `.www.` or a
            // `-www.` is part of a longer word.
            (self.previous == '\0'
                || self.previous.is_whitespace()
                || matches!(self.previous, '(' | '*' | '_' | '~'))
                && (ahead.starts_with(WWW) || WWW.starts_with(ahead))
        } else {
            SCHEMES.iter().any(|scheme| scheme.starts_with(character))
                && !self.previous.is_alphanumeric()
        };
        starts && self.inside == Inside::Prose && !self.line.code
    }

    /// Reads one character into the address being held. Answers whether that
    /// character was part of it.
    fn addressed(
        &mut self,
        character: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut address) = self.address.take() else {
            return false;
        };

        // An address ends where the word does. Bounded like everything else
        // held here: past the bound it stops being an address and is put back
        // as the text it is.
        let carries = !character.is_whitespace() && address.len() < MOST;
        if carries && spelled(&address) == Spelled::Whole {
            address.push(character);
            self.address = Some(address);
            return true;
        }

        if carries {
            address.push(character);
            if spelled(&address) != Spelled::Nothing {
                self.address = Some(address);
                return true;
            }
            // A word that started like a scheme and turned out to be a word.
            // The character that decided it is part of that word, and goes
            // back with it rather than to the caller.
            self.say(&address, say);
            self.started = true;
            self.previous = character;
            return true;
        }

        self.wrote_address(&address, say);
        false
    }

    /// Hands on an address the answer wrote bare.
    ///
    /// Drawn as the link it is, so what can be acted on in a terminal looks
    /// like it can. What ends a sentence is not part of it: a full stop after
    /// an address belongs to the prose, and a reader who copies the row gets
    /// an address that still resolves.
    fn wrote_address(&mut self, address: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let ends = address.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"']);

        // A scheme and nothing after it is a word somebody wrote, not somewhere
        // anybody can go, and `www.` needs a host to start with a letter or a
        // digit.
        let target = if let Some(host) = ends.strip_prefix(WWW) {
            host.starts_with(|character: char| character.is_ascii_alphanumeric())
                .then(|| format!("https://{ends}"))
        } else {
            (spelled(ends) == Spelled::Whole && !SCHEMES.contains(&ends)).then(|| ends.to_owned())
        };
        let Some(target) = target else {
            self.say(address, say);
            self.started = true;
            self.previous = address.chars().next_back().unwrap_or('/');
            return;
        };

        self.pay(say);
        say(Slot::Link, ends, Some(&target));
        self.started = true;
        self.previous = ends.chars().next_back().unwrap_or('/');
        self.say(&address[ends.len()..], say);
    }

    /// How many bytes in front of `character` belong to a reference opening
    /// here, or `None` where none is.
    ///
    /// Prose only, and not inside a span of code, for the reason a link is:
    /// `#[derive(Debug)]` is an attribute and `#define` is a directive, and
    /// both are things a coding agent writes far more often than it writes a
    /// number about a repository. Not where a line has yet to start either --
    /// there a hash is a heading, and headings were spoken for first.
    ///
    /// Nothing at all without a repository in hand, which is the check that
    /// leaves every other terminal drawing exactly what it drew before.
    fn opens_reference(&self, character: char, pending: &str) -> Option<usize> {
        if character != '#'
            || self.inside != Inside::Prose
            || self.line.code
            || !self.started
            || self.forge.is_none()
        {
            return None;
        }

        // `owner/repo#12` is a number counted somewhere else, and the name in
        // front of the hash is part of the reference. Where there is no name,
        // the hash has to start a word: `abc#def` is an identifier, a fragment
        // or an anchor, and none of those is a number anybody can open.
        repository(pending).or_else(|| (!self.previous.is_alphanumeric()).then_some(0))
    }

    /// Reads one character into the reference being held. Answers whether that
    /// character was part of it.
    fn referenced(
        &mut self,
        character: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut reference) = self.reference.take() else {
            return false;
        };

        if character.is_ascii_digit() && reference.number.len() < DIGITS {
            reference.number.push(character);
            self.reference = Some(reference);
            return true;
        }

        self.wrote_reference(&reference, say);
        false
    }

    /// Hands on a number the answer wrote, as the link it is.
    ///
    /// What ends a sentence is not part of it and never was: the number ended
    /// at the first character that was not a digit, so the stop after `#487`
    /// is prose that has not been read yet rather than something to trim.
    ///
    /// A hash with no digits behind it goes back as the characters it was
    /// written with. So does one the session has nowhere to point -- the same
    /// answer drawn on a checkout with no forge behind it, which is why this
    /// asks again rather than trusting the hold to have been opened wisely.
    fn wrote_reference(
        &mut self,
        reference: &Reference,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let lead = reference.lead.as_deref().unwrap_or_default();
        let slug = reference.slug.as_deref().unwrap_or_default();
        let words = format!("{lead}{slug}#{}", reference.number);
        self.previous = words.chars().next_back().unwrap_or('#');
        self.started = true;

        let address = self
            .forge
            .as_ref()
            .filter(|_| !reference.number.is_empty())
            .map(|forge| forge.address(reference.slug.as_deref(), &reference.number));

        let Some(address) = address else {
            self.say(&words, say);
            return;
        };

        // The mark the line owes, then the words -- the order every other run
        // on a line reaches the terminal in. The indentation is not owed here:
        // a reference only opens once something has been written on the line,
        // and whatever that was spent it.
        self.pay(say);
        say(Slot::Link, &words, Some(&address));
    }

    /// Hands on a link that arrived whole.
    ///
    /// The words wear the link's own slot and carry the address, which is not
    /// written out beside them: the sentence the answer wrote goes on reading
    /// as a sentence, and the address is the row's to hand to a terminal that
    /// opens links -- which is how a reader reaches it, the same way they
    /// reach one on any page. A link with no words is its address, since that
    /// is the one thing there is to show. A link to a path is handed on as the
    /// file it names in the checkout, which is the one form of it a terminal
    /// can open; its words stay what the answer wrote. Nothing here is ever
    /// written as anything but text: an address is bytes a model chose, and
    /// the rule this file opens with is that none of them leaves as an
    /// instruction.
    fn wrote_link(&mut self, link: &Link, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.pay(say);

        let target = target(&link.target);
        let label = escapes::decoded(&link.label);
        let words = if label.is_empty() { target } else { &label };

        let file = self.files.as_ref().and_then(|files| files.address(target));
        let address = file.as_deref().unwrap_or(target);
        say(Slot::Link, words, (!address.is_empty()).then_some(address));
        self.previous = words.chars().next_back().unwrap_or(')');
    }

    /// Whether a reference can be tied to a definition where the scan stands:
    /// one has been written, and this is where footnotes are read.
    fn reads_definitions(&self) -> bool {
        !self.definitions.is_empty() && self.notes()
    }

    /// Whether the colon after `link`'s `]` opens a definition: the brackets
    /// are the first thing the line says, hold a label that is no footnote's,
    /// and this is where footnotes are read.
    fn opens_definition(&self, link: &Link) -> bool {
        link.first && !link.label.is_empty() && !footnote(&link.label) && self.notes()
    }

    /// Reads `character` into the line held as a definition or the angle
    /// address held, whichever is open. Answers whether it was taken.
    ///
    /// `false` says what was held is settled: drawn as what it was, or read as
    /// it would have been had it not been held, and `character` is the
    /// caller's again.
    fn holds_text(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        (self.defining.is_some() && self.defines(character, room, say))
            || (self.angle.is_some() && self.angled(character, room, say))
    }

    /// Reads `character` into the number, tag or reference held, whichever is
    /// open. Answers whether it was taken, and `false` says what was held is
    /// settled and `character` is the caller's again.
    fn holds_word(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        (self.reference.is_some() && self.referenced(character, say))
            || (self.tag.is_some() && self.tagged(character, say))
            || (self.entity.is_some() && self.entitied(character, room, say))
    }

    /// Shows the watch on the line, for a title or the header of a table,
    /// `text` that was held while it arrived and has now been drawn.
    ///
    /// The block such a line turns out to be is laid out from what the watch
    /// saw. A held text that is drawn as a link never goes back through the
    /// scan, so it is told here, in the order it was written; one that is read
    /// again is seen by the watch as that reading goes. Text read again was
    /// told to the watch before it was, so while one is read nothing is told.
    fn watches(&mut self, text: &str) {
        if self.told {
            return;
        }
        for character in text.chars() {
            if !self.table.as_ref().is_some_and(Table::mirrors) {
                return;
            }
            self.mirrored(character);
        }
    }

    /// Whether the watch on the line holds `character`, the start of a title
    /// or a header being opened first. Text read again was told to the watch as
    /// it arrived, so it is not told twice.
    fn watched(&mut self, character: char) -> bool {
        if self.told {
            return false;
        }
        if self.opens_title(character) {
            // Spaces in front of a line do not say whether it leaves the
            // quote. The first character after them says that: `margin`
            // hands it to `leaves_quote`, or, when it is a quote mark, the
            // mark reader decides.
            if character != ' ' {
                self.leaves_quote(character);
            }
            self.table = Some(Table::bare());
        }
        self.table.as_ref().is_some_and(Table::mirrors) && self.mirrored(character)
    }

    /// Reads `text` again as if it were arriving, where the heading line and
    /// the watch on the line have been told it already.
    fn again(&mut self, text: &str, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let told = std::mem::replace(&mut self.told, true);
        self.scan(text, room, say);
        self.told = told;
    }

    /// Reads `character` into the line being held as a definition. Answers
    /// whether it was taken.
    ///
    /// `false` says the line is over, or past its bound: what was held has been
    /// drawn as the definition it was, or read as the line it turned out to be,
    /// and `character` is the caller's again.
    fn defines(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut held) = self.defining.take() else {
            return false;
        };
        if character != '\n' && held.rest.len() < MOST {
            held.rest.push(character);
            self.defining = Some(held);
            return true;
        }
        self.defined(held, character == '\n', room, say);
        false
    }

    /// Settles a line that opened with a label and a colon, `whole` saying
    /// whether it ended at a break rather than at its bound.
    ///
    /// A definition is the label, a colon, and then one address and nothing
    /// else but blanks. It is drawn as that address, a link, and kept for the
    /// references after it: the first of a label is the one kept, and one past
    /// [`DEFINITION_CAP`] or [`DEFINITION_BYTES`] is no definition. Whatever
    /// else the line says is shown as written, and read as any other line
    /// would have been.
    fn defined(
        &mut self,
        held: Defining,
        whole: bool,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let address = held.rest.trim_matches([' ', '\t']);
        let spelled_out = whole
            && spelled(address) == Spelled::Whole
            && !address.starts_with(WWW)
            && !SCHEMES.contains(&address)
            && address
                .chars()
                .all(|character| !character.is_whitespace() && !character.is_control());

        if spelled_out && self.keeps(&held.label, address) {
            self.pay(say);
            say(Slot::Link, address, Some(address));
            self.previous = address.chars().next_back().unwrap_or(':');
            self.watches(&held.rest);
            return;
        }

        // The brackets, and then the rest of the line as it arrived, read
        // again: its markup and its addresses are what they would have been
        // had nothing been held.
        self.pay(say);
        spill(
            &Link {
                part: Part::Between,
                label: held.label,
                ..Link::default()
            },
            say,
        );
        say(Slot::Plain, ":", None);
        self.previous = ':';
        self.watches(&held.rest);
        self.again(&held.rest, room, say);
    }

    /// Whether `address` can be drawn as the definition of `label`: it is
    /// kept, or the label was defined before, and the first is the one kept.
    fn keeps(&mut self, label: &str, address: &str) -> bool {
        let label: String = label.chars().flat_map(char::to_lowercase).collect();
        if self.definitions.iter().any(|kept| kept.label == label) {
            return true;
        }
        let bytes: usize = self
            .definitions
            .iter()
            .map(|kept| kept.label.len() + kept.address.len())
            .sum();
        if self.definitions.len() >= DEFINITION_CAP
            || bytes + label.len() + address.len() > DEFINITION_BYTES
        {
            return false;
        }
        self.definitions.push(Definition {
            label,
            address: address.to_owned(),
        });
        true
    }

    /// The address a definition written before this point gave `label`, in any
    /// letter case, where references are read.
    fn defined_as(&self, label: &str) -> Option<&str> {
        if !self.reads_definitions() {
            return None;
        }
        let label: String = label.chars().flat_map(char::to_lowercase).collect();
        self.definitions
            .iter()
            .find(|kept| kept.label == label)
            .map(|kept| kept.address.as_str())
    }

    /// Draws `link` as a reference to a definition, where its label has one
    /// and nothing but the label follows it. Answers whether it did.
    fn ties(&mut self, link: &Link, say: &mut dyn FnMut(Slot, &str, Option<&str>)) -> bool {
        if link.part != Part::Between {
            return false;
        }
        let Some(address) = self.defined_as(&link.label).map(str::to_owned) else {
            return false;
        };
        self.wrote_reference_to(&link.label, &address, say);
        true
    }

    /// Draws a reference written with a second label, `[words][label]` or
    /// `[words][]`, as its words where the label has a definition, and as it
    /// was written where it has none.
    fn wrote_reference_link(
        &mut self,
        link: &Link,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let label = if link.target.is_empty() {
            &link.label
        } else {
            &link.target
        };
        if let Some(address) = self.defined_as(label).map(str::to_owned) {
            self.wrote_reference_to(&link.label, &address, say);
        } else {
            self.retaken(link, ']', room, say);
        }
    }

    /// Settles a second label that names no definition, `ended` being the
    /// character that closed it or that it could not carry on past.
    ///
    /// The first brackets are shown as written. The second, and `ended`, are
    /// read again as if they were arriving, which is what they would have been
    /// had nothing been looked for: the opening of a link with its address
    /// after it, or of a footnote's reference, as it is where no definition
    /// has been written.
    fn retaken(
        &mut self,
        link: &Link,
        ended: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.pay(say);
        spill(
            &Link {
                part: Part::Between,
                label: link.label.clone(),
                ..Link::default()
            },
            say,
        );
        self.previous = ']';
        let mut again = String::from('[');
        again.push_str(&link.target);
        again.push(ended);
        self.again(&again, room, say);
    }

    /// Hands on a reference's `words`, carrying the `address` its definition
    /// gave, the way a link written out is.
    fn wrote_reference_to(
        &mut self,
        words: &str,
        address: &str,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.pay(say);
        let words = escapes::decoded(words);
        let words = if words.is_empty() { address } else { &words };
        say(Slot::Link, words, Some(address));
        self.previous = words.chars().next_back().unwrap_or(']');
    }

    /// Reads `character` into the angle address being held. Answers whether it
    /// was taken.
    ///
    /// `false` says it was no address: what was held has been read as it would
    /// have been had no address been looked for, and `character` is the
    /// caller's again. The `>` that ends one is taken with it, and is no part
    /// of the words or of where they point.
    fn angled(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut written) = self.angle.take() else {
            return false;
        };
        if character == '>' {
            // Inside a key or a run of sub or superscript a link has no place,
            // so there the address is shown as written.
            let address = written.get(1..).and_then(angle_target);
            if let (Some(target), None, false) = (address, &self.script, self.line.key) {
                self.pay(say);
                say(
                    Slot::Link,
                    written.get(1..).unwrap_or_default(),
                    Some(&target),
                );
                self.started = true;
                self.previous = '>';
                self.watches(written.get(self.angle_seen..).unwrap_or_default());
                self.watches(">");
                return true;
            }
        } else if written.len() < MOST {
            written.push(character);
            let body = written.get(1..).unwrap_or_default();
            let carries = match spelled(body) {
                Spelled::Nothing => email(character) || character == '@',
                _ => !character.is_whitespace() && !character.is_control() && character != '<',
            };
            if carries {
                self.angle = Some(written);
                return true;
            }
            written.pop();
        }
        self.unangle(&written, room, say);
        false
    }

    /// Reads what an angle address was held as, as it would have been read had
    /// none been looked for: the `<` is text, and what followed it arrives
    /// again.
    fn unangle(
        &mut self,
        written: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.untag("<", say);
        self.watches(written.get(self.angle_seen..).unwrap_or_default());
        self.again(written.get(1..).unwrap_or_default(), room, say);
    }

    /// Whether footnotes are read where the scan stands.
    ///
    /// Not in a heading, and not by a reader that lays out a block or reads a
    /// table's cell: there a label is shown as written, and the numbers stay
    /// the answer's own.
    fn notes(&self) -> bool {
        !self.unheld && !self.line.heading && self.passing.is_none()
    }

    /// The number of the footnote `label`, numbering it if it is new, or
    /// `None` where it is to be shown as written: a second definition of one
    /// label, or a new label past [`FOOTNOTE_CAP`].
    ///
    /// A label is the same label in any letter case, so it is kept in lower
    /// case and each counts once against the cap.
    fn number(&mut self, label: &str, defining: bool) -> Option<usize> {
        let label: String = label.chars().flat_map(char::to_lowercase).collect();
        if let Some(at) = self.notes.iter().position(|note| note.label == label) {
            let note = self.notes.get_mut(at)?;
            if defining {
                if note.defined {
                    return None;
                }
                note.defined = true;
            }
            return Some(at + 1);
        }
        if self.notes.len() >= FOOTNOTE_CAP {
            return None;
        }
        self.notes.push(Note {
            label,
            defined: defining,
        });
        Some(self.notes.len())
    }

    /// Reads what `link` held as a footnote, given the character after its
    /// `]`. Answers `None` where it is not one, and otherwise whether
    /// `character` was taken.
    ///
    /// A label that opened its line and has a colon after it is the
    /// definition: its number stands in for the label and the colon, and the
    /// rest of the line is the note, quiet. Anywhere else the label is a
    /// reference, drawn as its number in superscript, or under ascii in
    /// brackets and quiet, and `character` is the caller's.
    fn cites(
        &mut self,
        link: &Link,
        character: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> Option<bool> {
        if link.part != Part::Between || !self.notes() || !footnote(&link.label) {
            return None;
        }
        let defining = character == ':' && link.first;
        let cited = self.glyphs.cited(self.number(&link.label, defining)?);
        self.started = true;
        self.previous = ']';
        if defining {
            self.line.noted = true;
            self.say(&cited, say);
            self.line.gap = true;
            return Some(true);
        }
        match self.glyphs {
            Glyphs::Unicode => self.say(&cited, say),
            Glyphs::Ascii => {
                self.spend(say);
                self.pay(say);
                say(Slot::Quiet, &cited, None);
            }
        }
        Some(false)
    }

    /// Whether `character` opens a tag where the scan stands.
    ///
    /// Prose only, and not inside a span of code or display math shown as
    /// written, for the reason a link is.
    fn opens_tag(&self, character: char) -> bool {
        character == '<'
            && self.inside == Inside::Prose
            && !self.line.code
            && self.passing.is_none()
    }

    /// Reads `character` into the tag being held. Answers whether it was
    /// taken.
    ///
    /// `false` says the tag was none of the six, or ran past [`TAG_CAP`] or
    /// the line: what was held has gone out as written, and `character` is
    /// the caller's again. What is no tag but starts like an address in angle
    /// brackets is handed to the hold that reads those.
    fn tagged(&mut self, character: char, say: &mut dyn FnMut(Slot, &str, Option<&str>)) -> bool {
        let Some(mut written) = self.tag.take() else {
            return false;
        };
        if character == '\n' || written.chars().count() >= TAG_CAP {
            self.untag(&written, say);
            return false;
        }
        written.push(character);
        match shape(&written) {
            Shape::Partly => {
                self.tag = Some(written);
                true
            }
            Shape::Not => {
                // It may yet be an address in angle brackets, which is read
                // from here on by its own hold.
                if written.get(1..).is_some_and(angles) {
                    self.angle_seen = written.len();
                    self.angle = Some(written);
                    return true;
                }
                written.pop();
                self.untag(&written, say);
                false
            }
            Shape::Whole(tag) => {
                self.reads(tag, &written, say);
                true
            }
        }
    }

    /// Holds `character` in the open `<sub>` or `<sup>` run. Answers whether
    /// it was held.
    ///
    /// Past [`SCRIPT_CAP`] the run is plain text, and so is the rest of it:
    /// what was held goes out as written, and `character` is the caller's.
    fn raises(&mut self, character: char, say: &mut dyn FnMut(Slot, &str, Option<&str>)) -> bool {
        let Some(script) = self.script.as_mut() else {
            return false;
        };
        if script.text.len() + character.len_utf8() <= SCRIPT_CAP {
            script.text.push(character);
            return true;
        }
        let text = std::mem::take(&mut script.text);
        self.script = None;
        self.say(&text, say);
        self.previous = character;
        false
    }

    /// Hands on held text that turned out to be no tag, as written: into the
    /// run of a `<sub>` or `<sup>` open around it, or out.
    fn untag(&mut self, written: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.started = true;
        self.previous = written.chars().next_back().unwrap_or('<');
        if let Some(script) = self.script.as_mut() {
            if script.text.len() + written.len() <= SCRIPT_CAP {
                script.text.push_str(written);
                return;
            }
            let text = std::mem::take(&mut script.text);
            self.script = None;
            self.say(&text, say);
        }
        self.say(written, say);
    }

    /// Reads a whole tag, `written` as it was.
    fn reads(&mut self, tag: Tag, written: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        // Inside a key only its closing tag is read, and any other is one
        // more thing the key holds.
        if self.line.key
            && tag
                != (Tag {
                    name: Name::Kbd,
                    closing: true,
                })
        {
            self.untag(written, say);
            return;
        }

        // A run of sub or superscript ends at its own closing tag, and at any
        // other tag as the plain text it was.
        if let Some(script) = self.script.take() {
            let own = if script.raised { Name::Sup } else { Name::Sub };
            if tag
                == (Tag {
                    name: own,
                    closing: true,
                })
            {
                self.line.tagged = true;
                self.scripts(&script, say);
                return;
            }
            self.say(&script.text, say);
        }

        // A `<details>` block and its summary belong to the answer, so they
        // are read only where the answer is: a heading, and a reader laying
        // out a block or a cell, show them as written.
        let lays = self.unheld || self.line.heading;
        if lays && matches!(tag.name, Name::Details | Name::Summary) {
            self.untag(written, say);
            return;
        }

        self.line.tagged = true;
        // Neither a break nor a block of its own is the header of a table or
        // the title of a heading, and the line is not watched for either.
        if matches!(tag.name, Name::Details | Name::Summary | Name::Br)
            && self.table.take_if(|table| table.mirrors()).is_some()
        {
            self.header = Header::Declined;
        }
        match (tag.name, tag.closing) {
            (Name::Details, closing) => self.folded = !closing,
            (Name::Summary, false) => {
                self.line.summary = true;
                self.spend(say);
                self.pay(say);
                say(Slot::Quiet, self.glyphs.folded(), None);
                say(Slot::Plain, " ", None);
                self.started = true;
                self.previous = '>';
            }
            (Name::Summary, true) => {
                self.line.summary = false;
                self.previous = '>';
            }
            // A break with no line to break, in a heading or a cell, is the
            // space between the words either side of it.
            (Name::Br, false) if lays => {
                self.say(" ", say);
                self.started = true;
                self.previous = ' ';
            }
            (Name::Br, false) => self.breaks(say),
            (Name::Kbd, false) => {
                self.line.key = true;
                self.started = true;
                self.previous = '>';
                let (open, _) = self.glyphs.keyed();
                self.say(open, say);
            }
            (Name::Kbd, true) => {
                self.previous = '>';
                if self.line.key {
                    let (_, close) = self.glyphs.keyed();
                    self.say(close, say);
                    self.line.key = false;
                }
            }
            (Name::Sub | Name::Sup, false) => {
                self.script = Some(Script {
                    raised: tag.name == Name::Sup,
                    text: String::new(),
                });
                self.started = true;
                self.previous = '>';
            }
            // A closing tag with nothing open is read away.
            (Name::Br | Name::Sub | Name::Sup, true) => self.previous = '>',
        }
    }

    /// Hands on a run of sub or superscript that has closed: in those
    /// characters, quiet, where every character of it has one, and as its
    /// plain text where one does not.
    fn scripts(&mut self, script: &Script, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.previous = '>';
        let drawn: Option<String> = script
            .text
            .chars()
            .map(|character| scripted(character, script.raised))
            .collect();
        match drawn {
            Some(drawn) if !drawn.is_empty() => {
                self.spend(say);
                self.pay(say);
                say(Slot::Quiet, &drawn, None);
            }
            Some(_) => {}
            None => self.say(&script.text, say),
        }
    }

    /// Owes a break where a `<br>` stands, after whatever the line owed
    /// before it.
    fn breaks(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.spend(say);
        self.pay(say);
        self.line.broken = true;
        self.started = true;
        self.previous = '>';
    }

    /// Breaks the line where a `<br>` owed it, now something is drawn on the
    /// row after, and stands that row where the line's own stood: behind a
    /// quote's bar, and two columns in inside a `<details>` block.
    fn broke(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if !std::mem::take(&mut self.line.broken) {
            return;
        }
        say(self.slot(), "\n", None);
        if matches!(self.line.quoted, Quoted::Plain | Quoted::Alert) {
            self.bar(say);
        }
        if self.line.fold == Fold::Drawn {
            say(Slot::Quiet, BODY_LEAD, None);
        }
    }

    /// Lets go of a tag, a run of sub or superscript and a key, which a line's
    /// end does as well as a message's: a tag and a run as written, and a key
    /// with the bracket that closes it.
    fn untags(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if let Some(written) = self.tag.take() {
            self.untag(&written, say);
        }
        if let Some(script) = self.script.take() {
            self.say(&script.text, say);
        }
        if self.line.key {
            let (_, close) = self.glyphs.keyed();
            self.say(close, say);
            self.line.key = false;
        }
    }

    /// Sets a line of a `<details>` block's body two columns in, once, before
    /// the first thing on it, with a lead every row it wraps onto hangs under.
    ///
    /// A paragraph's line only: a quote, an item of a list, a heading, a
    /// summary, code and every block are drawn where they always are.
    fn fold(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.line.fold != Fold::Owed {
            return;
        }
        let folds = self.folded
            && self.inside == Inside::Prose
            && self.line.quoted == Quoted::Not
            && self.line.marked == Marked::Nothing
            && self.item == Item::Shut
            && !self.line.heading
            && !self.line.summary;
        self.line.fold = if folds {
            say(Slot::Quiet, BODY_LEAD, None);
            Fold::Drawn
        } else {
            Fold::Not
        };
    }

    /// Lets go of whatever was still being held when the message ended.
    ///
    /// A link goes back as the characters it was written with; a table is drawn
    /// as the table it turned out to be. `room` is what it is drawn against, and
    /// is the caller's for the reason it is on [`Markdown::read`].
    ///
    /// The last moment there is: the reader is dropped between messages, and
    /// text still held when that happens is text the reader never sees.
    pub fn finish(&mut self, room: usize, say: &mut dyn FnMut(Said<'_>)) {
        let mut kinds = std::mem::take(&mut self.kinds);
        let strikes = self.shared_strikes();
        let mut tell = |slot: Slot, text: &str, link: Option<&str>| {
            if !strikes.borrow_mut().keep(slot, text, link) {
                kinds.tell(slot, text, link, &mut |slot, text, link| {
                    say(Said::Run { slot, text, link });
                });
            }
        };
        // Before what the rest of the line held, since the spaces came first.
        self.unindent(room, &mut tell);
        self.close(room, &mut tell);
        // The end of the message ends the paragraph a phrase was held in.
        self.unstrike(&mut tell);

        // A fence's own line that the message ended in the middle of is a line
        // that ended all the same, so its edge is drawn: a frame is shut by the
        // line that closes it whether or not a break follows.
        if matches!(self.inside, Inside::Opening | Inside::Closing) {
            self.end_line(room, &mut tell);
        }

        // So is a line of an indented block, and the block is shut after it,
        // since nothing but a line of its own could have carried it on.
        if self.inside == Inside::Indented {
            self.end_line(room, &mut tell);
            self.shuts(&mut tell);
        }

        // The heading the message ended in the middle of has its rule all the
        // same, under a line that ends as a break would have ended it.
        if let Some(block) = self.atx.take().and_then(ruled) {
            tell(Slot::Plain, "\n", None);
            self.ready = Some(block);
        }

        // A block that never closed is handed on as it stands: the end of the
        // message is the last thing that could have closed it. A table is
        // drawn from what arrived; anything else is shown as written.
        if let Some(gathering) = self.gathering.take() {
            self.hand_on(room, &mut kinds, say);
            self.ready = Some(Block {
                kind: gathering.held.kind(),
                source: gathering.source,
                closed: false,
            });
        }
        self.passing = None;
        // What the answer numbered and left open ends with it.
        self.notes.clear();
        self.definitions.clear();
        self.folded = false;
        self.hand_on(room, &mut kinds, say);
        kinds.flush(&mut |slot, text, link| say(Said::Run { slot, text, link }));
        self.kinds = kinds;
    }

    /// Reads a line that is still in doubt ([`Markdown::in_doubt`]) as the
    /// prose it was said to be, because whoever is told is about to end the
    /// line itself and the delimiter row or underline that would have taken it
    /// back can no longer reach it.
    ///
    /// What the watch held back is read as written, and nothing is watched for
    /// after. The rest of a message is read as it would have been had the line
    /// ended there, so a table that straddles it is not one, and a row of `=`
    /// or `-` that follows is no underline. A phrase held after a single tilde
    /// is drawn as written, since its closer can no longer reach it either.
    pub fn undoubted(&mut self, room: usize, say: &mut dyn FnMut(Said<'_>)) {
        let mut kinds = std::mem::take(&mut self.kinds);
        let strikes = self.shared_strikes();
        let mut tell = |slot: Slot, text: &str, link: Option<&str>| {
            if !strikes.borrow_mut().keep(slot, text, link) {
                kinds.tell(slot, text, link, &mut |slot, text, link| {
                    say(Said::Run { slot, text, link });
                });
            }
        };
        while let Some(table) = self.table.take_if(|table| table.pending()) {
            self.release(&table, room, &mut tell);
        }
        self.unstrike(&mut tell);
        self.hand_on(room, &mut kinds, say);
        kinds.flush(&mut |slot, text, link| say(Said::Run { slot, text, link }));
        self.kinds = kinds;
    }

    /// Lets go of whatever the scan was still holding, which a line's end does
    /// as well as a message's.
    fn close(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        // The line the message ended in is as complete as a break would have
        // made it, and the `>` that would have ended an address never came.
        // First, since what they hand back is read again and may leave an
        // address, a link or a run of markers for what follows to let go of.
        if let Some(held) = self.defining.take() {
            self.defined(held, true, room, say);
        }
        if let Some(written) = self.angle.take() {
            self.unangle(&written, room, say);
        }
        self.unslash_at_end(room, say);

        // A header still waiting for its delimiter row is the prose it was
        // read as until then, and what was held back with it is read before
        // anything else is let go of, since the line it begins is what the
        // rest of this stands in. The end of the message is a break, so a row
        // of `=` or `-` complete at it underlines the line above it as one
        // followed by a break does.
        while let Some(table) = self.table.take_if(|table| table.pending()) {
            match table.underlined() {
                Some(level) => self.titled(&table, level),
                None => self.release(&table, room, say),
            }
        }

        // A reference the message ended in the middle of, after what a header
        // held back has been read, which is where its break and the start of
        // the line under it were: they follow it, and it is no more of them.
        if let Some(written) = self.entity.take() {
            self.unentity(&written, room, say);
        }

        // A run of markers is held for the character that says what it was,
        // and the end of a message is that character never arriving. Settled
        // against a line break, because the end of a message ends everything a
        // line break ends and one more thing besides -- so a marker that meant
        // nothing comes back as itself, exactly as it would have one character
        // later, and one that opened something is consumed exactly as it would
        // have been there too.
        if let Some(held) = self.held.take() {
            self.settle(held, '\n', room, say);
        }

        // The word that would have ended the address never arrived, and the end
        // of the message ends it exactly as a space would have.
        if let Some(address) = self.address.take() {
            self.wrote_address(&address, say);
        }

        // The digit that would have carried the number on never arrived, and
        // the end of the message ends it exactly as a space would have.
        if let Some(reference) = self.reference.take() {
            self.wrote_reference(&reference, say);
        }

        // The character that would have said what it was in front of never
        // arrived, so it was in front of nothing.
        if std::mem::take(&mut self.escaped) {
            self.say("\\", say);
        }

        // The `>` that would have ended a tag never arrived, nor the closing
        // tag of a run or a key.
        self.untags(say);

        // The end of the message is the break the marker of an alert was
        // waiting for, and the header is drawn before the bar it owes is.
        if let Some(alert) = self.link.as_ref().and_then(|link| self.alerts(link)) {
            self.link = None;
            self.headline(alert, say);
        }

        // A line that has said nothing by now is blank, or a tag's alone,
        // and is not set in; what is still held is said below, and is.
        // A break with nothing after it on its line is the line's own, and a
        // note with no text owes no space after its number.
        if self.link.is_none() && !matches!(self.probe, Probe::Probing(_)) {
            self.line.broken = false;
            self.line.gap = false;
            if self.line.fold == Fold::Owed {
                self.line.fold = Fold::Not;
            }
        }

        // After the settle above, so a fence at the very end of a message has
        // had its chance to take the spaces in front of it with it.
        self.spend(say);
        self.pay(say);

        // A footnote's label the end of the message came straight after is a
        // reference, as one a break came after is.
        if let Some(link) = self.link.take()
            && self.cites(&link, '\n', say).is_none()
            && !self.ties(&link, say)
        {
            spill(&link, say);
        }
        if let Some(table) = self.table.take() {
            match self.closed_under(&table) {
                // The tag closes the block, which the message's end does as
                // well, so only the rows above it are drawn.
                Some((rows, _)) => self.lays(&Table::of(&rows), room, say),
                None => self.lays(&table, room, say),
            }
        }

        // A line that could have opened display math and never said more.
        if let Probe::Probing(written) = std::mem::take(&mut self.probe) {
            self.probe = Probe::Done;
            self.scan(&written, room, say);
        }
    }

    /// Ends the line, and with it everything markdown ends at one.
    fn end_line(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let ended = self.inside;
        let heading = self.atx.take();
        // A line of prose written at the margin that held nothing but tags read
        // away draws no row. One with spaces in front stands: those may have
        // gone out already, as they arrived, and the row is theirs.
        let vanishes = ended == Inside::Prose
            && !self.started
            && self.line.tagged
            && self.indent == 0
            && !self.line.spoke;
        // Belt and braces: the line break puts a link back before it gets here,
        // and the reset below would drop what one was holding without a sound.
        self.close(room, say);
        let gone = vanishes && !self.started;

        // The line of code is complete, so this is the moment it can be read.
        // A blank one has said nothing yet and still stands in its frame.
        if ended == Inside::Fence {
            self.gutter(say);
        }
        self.read_code(say);

        let inside = match ended {
            Inside::Opening => Inside::Fence,
            Inside::Closing => Inside::Prose,
            inside => inside,
        };

        // A block is read in whatever its opening fence named, and the reader
        // is made here because this is where that name is finished. A block
        // that closes drops its reader with it: the next one names its own.
        // Carried only out of a line of prose that had something on it: a
        // blank line is the end of the paragraph, and a fence has no emphasis
        // to speak of. Put back below rather than here -- see the line break.
        let emphasis = if ended == Inside::Prose && self.started {
            self.line.emphasis
        } else {
            // Nor is a phrase held: what is held goes out as written, before
            // the line ends.
            self.unstrike(say);
            Emphasis::default()
        };

        // A fence named for a block that is held, opened at the start of its
        // line, is gathered from here rather than read: what opened it is
        // rebuilt as the first line of what is kept, so the block can be read
        // again from its source alone.
        let held = match (ended, self.fence) {
            (Inside::Opening, Some((true, count))) if self.holds() => Held::fenced(&self.language)
                .map(|held| {
                    let mut source: String = std::iter::repeat_n(self.fenced, count).collect();
                    source.push_str(&self.language);
                    source.push('\n');
                    Gathering {
                        held,
                        line: source.len(),
                        source,
                        closer: Closer::Fence(self.fenced),
                    }
                }),
            _ => None,
        };
        let inside = if held.is_some() {
            Inside::Prose
        } else {
            inside
        };

        let syntax = match ended {
            _ if held.is_some() => None,
            Inside::Opening => Syntax::of(&self.language),
            Inside::Closing => None,
            _ => self.syntax.take(),
        };

        // A fence's own line is where its frame begins and ends. Worked out
        // before the reset, which forgets the language, and said after it, so
        // the edge carries no slot into the row that follows it. A fence that
        // is held is drawn whole when it closes and has no frame of its own.
        let edge = match ended {
            Inside::Opening if held.is_none() => {
                let name = fence_name(&self.language);
                let frame = Frame {
                    label: Label::new(&name, self.glyphs),
                    refusal: self.refusal,
                };
                Some(Cow::Owned(frame.top()))
            }
            Inside::Closing => Some(Cow::Borrowed(chrome::bottom(self.glyphs))),
            _ => None,
        };

        // A block that closed on this line stands in for the line, break and
        // all.
        let ready = self.ready.take();
        let blocked = ready.is_some();

        *self = Self {
            // A block of code is the one thing carried over, and a fence's own
            // line is where its effect begins or ends. The glyphs are not carried
            // over so much as never given up: they are what the terminal can
            // draw, which no line of an answer changes. Nor is whether blocks
            // are held, or a block that has just closed and not yet gone out,
            // or a line taken back that has not been told, or display math past
            // its cap.
            inside,
            fenced: self.fenced,
            syntax,
            glyphs: self.glyphs,
            unheld: self.unheld,
            unescaped: self.unescaped,
            retract: self.retract,
            retitle: self.retitle.take(),
            refusal: self.refusal.filter(|_| ended != Inside::Opening),
            ready,
            gathering: held,
            passing: self.passing.take(),
            // Nor what the line said about the next one: whether it was blank,
            // and whether an item it was under is still open.
            above: if ended == Inside::Prose && !self.started && !gone {
                Above::Blank
            } else {
                Above::Written
            },
            item: self.item,
            quoting: self.line.quoted,
            // Nor what the answer has said about the lines after it: the
            // footnotes it has numbered, and a `<details>` block still open.
            notes: std::mem::take(&mut self.notes),
            definitions: std::mem::take(&mut self.definitions),
            folded: self.folded,
            strikes: self.strikes.take(),
            ..Self::default()
        };

        // A fence's own line is a marker, and no marker is drawn: what stands
        // in its place is the frame's edge, which is a row of its own. After
        // the reset otherwise, so the row that ends carries no slot into the
        // row that follows it.
        if let Some(edge) = edge {
            say(Slot::Quiet, &edge, None);
            say(Slot::Quiet, "\n", None);
        } else if !matches!(ended, Inside::Opening | Inside::Closing) && !blocked && !gone {
            say(self.slot(), "\n", None);
        }

        // After the break, so the slot the row ends under is the one it would
        // have worn before emphasis learnt to cross a line at all.
        self.line.emphasis = emphasis;

        // The heading's words have gone out as they arrived, and the rule
        // under them follows as the block it is.
        if let Some(block) = heading.and_then(ruled) {
            self.ready = Some(block);
        }
    }

    /// Hands on a run of text, if there is any.
    ///
    /// Inside a block being read, the run is held instead: a highlighter reads
    /// whole lines and this is a piece of one. Everywhere else it goes straight
    /// out, which is every run in every answer that is not code.
    fn say(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if text.is_empty() {
            return;
        }

        // A note's text stands one space after its number, however many
        // were written after the colon, and the space goes out with it.
        if self.line.gap {
            let text = text.trim_start_matches([' ', '\t']);
            if text.is_empty() {
                return;
            }
            self.line.gap = false;
            self.say(&format!(" {text}"), say);
            return;
        }

        // The line's own order: what indented it, then the mark it owes, then
        // the words themselves.
        self.spend(say);
        self.pay(say);
        self.wears(text, say);
    }

    /// Hands on the indentation the line opened with, where it kept any.
    ///
    /// Called wherever the first thing on a line reaches the terminal, which
    /// is the moment the marker that could have claimed those spaces has
    /// either claimed them or gone. See [`Markdown::indent`].
    fn spend(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let mut indent = std::mem::take(&mut self.indent);
        while indent > 0 {
            let spaces = indent.min(SPACES.len());
            self.wears(&SPACES[..spaces], say);
            indent -= spaces;
        }
    }

    /// Hands on the gutter of the line of code being read, once.
    ///
    /// One run, so that whatever folds the row afterwards can tell it from
    /// the rest by what it says: see [`framed`].
    fn gutter(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if matches!(self.inside, Inside::Fence | Inside::Indented)
            && self.line.gutter == Gutter::Owed
        {
            self.line.gutter = Gutter::Put;
            say(Slot::Quiet, chrome::gutter(self.glyphs), None);
        }
    }

    /// Hands on a run under the slot the line is wearing, or into the block
    /// being read where one is open.
    fn wears(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.line.spoke = true;
        self.gutter(say);
        if self.syntax.is_some() && self.inside == Inside::Fence && self.code.len() < MOST {
            self.code.push_str(text);
            return;
        }

        let slot = self.slot();
        say(slot, text, None);
    }

    /// Hands on the line of code collected so far, read.
    ///
    /// Every byte comes back exactly once and in order, which is the property
    /// the whole thing rests on — a byte dropped is code that quietly changed
    /// meaning on screen, and a byte doubled is a row wider than it measured.
    fn read_code(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.code.is_empty() {
            return;
        }

        let mut code = std::mem::take(&mut self.code);
        // The reader wants the break as part of the line: a great many rules in
        // a syntax definition are written against the end of one.
        code.push('\n');

        match self.syntax.as_mut() {
            Some(syntax) => syntax.read(&code, &mut |slot, text| {
                // The break is the caller's to write, once, below.
                let text = text.strip_suffix('\n').unwrap_or(text);
                if !text.is_empty() {
                    say(slot, text, None);
                }
            }),
            None => say(Slot::Quiet, code.trim_end_matches('\n'), None),
        }
    }
}

/// Writes a link back as the characters it was written with.
///
/// Exactly them, in order, so a bracket that was never a link costs the reader
/// nothing at all.
fn spill(link: &Link, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
    say(Slot::Plain, "[", None);
    if !link.label.is_empty() {
        say(Slot::Plain, &link.label, None);
    }
    if link.part != Part::Label {
        say(Slot::Plain, "]", None);
    }
    let opening = match link.part {
        Part::Label | Part::Between => return,
        Part::Target => "(",
        Part::Second => "[",
    };
    say(Slot::Plain, opening, None);
    if !link.target.is_empty() {
        say(Slot::Plain, &link.target, None);
    }
}

/// The address out of what stood between the brackets.
///
/// Markdown lets a title follow the address, and lets the address itself be
/// wrapped in angle brackets. Neither is the destination, and the destination
/// is the whole of what is worth a reader's columns.
fn target(between: &str) -> &str {
    between
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|character| character == '<' || character == '>')
}

/// The rule under a heading `line` was read from, where it has words to rule
/// under.
fn ruled((level, line): (Level, String)) -> Option<Block> {
    let words = line.trim_start().trim_start_matches('#');
    let words = !words.trim().is_empty();
    words.then_some(Block {
        kind: Kind::Heading(level),
        source: line,
        closed: true,
    })
}

/// The text a run is put back as, where it meant nothing.
fn written(held: HeldMarks) -> &'static str {
    let whole = match held.mark {
        '#' => HASHES,
        '*' => STARS,
        '`' => TICKS,
        '-' => DASHES,
        '+' => PLUSES,
        '>' => ANGLES,
        '~' => TILDES,
        _ => SCORES,
    };

    whole.get(..held.count).unwrap_or(whole)
}

/// The answers every drawing of an answer is compared with, and the line a
/// case watching them waits for.
#[cfg(test)]
pub(crate) mod corpus;
#[cfg(test)]
pub(crate) mod corpus_pictures;
#[cfg(test)]
mod tests;
