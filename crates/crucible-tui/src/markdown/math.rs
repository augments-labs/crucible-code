//! Math in an answer, found from exactly what was written and handed to the
//! drawers whole: a span in a line, and a display block on lines of its own.
//!
//! A span opens at a `$` or a `\(` in prose and is held, never read as
//! markdown, until what closes it arrives, so `$a*b*c$` is one formula rather
//! than an emphasised `b`. Whether a dollar opened one at all is only known
//! once the span ends, and a span that turns out to be none is read again as
//! the prose it was. What decides it:
//!
//! - A dollar that a space, a tab, a comma or the end of the line follows
//!   opens nothing, and neither do two dollars together in a line.
//! - A dollar closes a span where no backslash stands before it, no space or
//!   tab stands just inside it, and no letter, digit or `_` follows it. One
//!   that does not close a span is the dollar it was, and may open the next.
//!   So `$5 and $10`, `$HOME/$USER`, `$n$th` and `set $a,$b` stay as written.
//! - `\)` closes a span `\(` opened, where the backslash is not itself
//!   escaped.
//! - A span ends where its line does, at a backtick, at `://`, and at
//!   [`SPAN_CAP`] bytes; each of those leaves it as written and reads what it
//!   held again.
//! - `\$` is a dollar, written: the backslash says it opens nothing.
//!
//! Only in prose: code, a key, a block drawn as written and the words a
//! link or an address holds never see a dollar as math.
//!
//! A span that closes is drawn by the first drawer whose inline answer draws
//! it, in the mode the reader was given; under [`Mode::Written`], with no
//! drawer, or where every drawer declines, it is shown exactly as written,
//! markers and all, with no character of it read. Until a picture can be shown
//! [`Mode::Image`] draws a span as [`Mode::Text`] does. Nothing held reaches
//! the record before the span settles; the live foot shows it as written.
//!
//! A display block (`$$`, `\[` or a named environment) is gathered by the
//! reader as a block, and [`display`] says what the math drawer is handed of
//! it. One no drawer draws is read again as a formula: every marker,
//! backslash and dollar in it shown as written, and a link, an address and a
//! number read as they are in prose. One a drawer refused for a reason worth
//! saying is shown as a math fence it refused would be: framed, the reason on
//! the frame, and every character inside it as written.

use super::Markdown;
use crate::color::Slot;
use crate::drawing::{Drawers, Mode, Room};
use crate::escape::Escapes;
use crate::row::Row;
use crate::width;

/// The most source one span holds before it is shown as written instead.
///
/// Four thousand bytes is far past any formula written in a line and short of
/// anything that matters to the budget.
pub(super) const SPAN_CAP: usize = 4096;

/// What the reader draws math with, and the span being held.
#[derive(Debug)]
pub(super) struct Math {
    /// The span opened and not yet settled.
    span: Option<Span>,
    /// What a span that closes is offered to, in order.
    drawers: Drawers,
    /// How a span that closes is drawn.
    mode: Mode,
    /// Whether what is read is a formula no drawer drew, shown as written:
    /// no marker, backslash or dollar in it means anything, and a link, an
    /// address and a number in it are read as in prose.
    pub(super) formula: bool,
}

impl Default for Math {
    /// No drawers, so every span is shown as written; the mode a setting
    /// leaves alone draws math as text where a drawer can.
    fn default() -> Self {
        Self {
            span: None,
            drawers: Drawers::default(),
            mode: Mode::Image,
            formula: false,
        }
    }
}

/// A span being held.
#[derive(Debug)]
struct Span {
    delimiter: Delimiter,
    /// What was written between the markers, as written.
    source: String,
    state: State,
    /// Whether the last character held is a backslash that nothing paired.
    slashed: bool,
}

/// What opened a span, which is what closes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    /// `$`, closed by `$`.
    Dollar,
    /// `\(`, closed by `\)`.
    Paren,
}

impl Delimiter {
    /// The markers it is written between.
    const fn marks(self) -> (&'static str, &'static str) {
        match self {
            Self::Dollar => ("$", "$"),
            Self::Paren => ("\\(", "\\)"),
        }
    }
}

/// Where a span being held stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// A dollar arrived, and nothing after it yet.
    Opened,
    /// Characters of the formula are arriving.
    Inside,
    /// A dollar that may close it arrived, and the character after it says.
    Closing,
}

impl Markdown {
    /// The same reader, drawing a span of math by the first of `drawers`
    /// that draws it, in `mode`.
    #[must_use]
    pub fn drawing(mut self, drawers: &Drawers, mode: Mode) -> Self {
        self.math.drawers = drawers.clone();
        self.math.mode = mode;
        self
    }

    /// The same reader, reading a formula no drawer drew where `formula`.
    #[must_use]
    pub(super) fn formula(mut self, formula: bool) -> Self {
        self.math.formula = formula;
        self
    }

    /// Whether a span of math is held, not yet known to be one.
    pub(crate) fn holds_a_span(&self) -> bool {
        self.math.span.is_some()
    }

    /// The span of math being held, as written and clipped to `columns`, for
    /// the live foot: nothing of it is in the record yet.
    pub(crate) fn spanning(&self, columns: usize) -> Option<Row> {
        let span = self.math.span.as_ref()?;
        let (open, close) = span.delimiter.marks();
        let closer = if span.state == State::Closing {
            close
        } else {
            ""
        };
        let mut escapes = Escapes::default();
        let shown: String = open
            .chars()
            .chain(span.source.chars())
            .chain(closer.chars())
            .filter(|character| {
                !escapes.holds(*character) && !width::unshown(*character) && !character.is_control()
            })
            .collect();
        Some(
            Row::new()
                .then_structural(Slot::Plain, shown)
                .clipped(columns),
        )
    }

    /// Opens a span at `character`, `escaped` where a backslash stands before
    /// it. Answers whether it did.
    pub(super) fn opens_math(&mut self, character: char, escaped: bool) -> bool {
        let delimiter = match (character, escaped) {
            ('$', false) => Delimiter::Dollar,
            ('(', true) => Delimiter::Paren,
            _ => return false,
        };
        let prose = self.inside == super::Inside::Prose
            && !self.line.code
            && !self.line.key
            && !self.unescaped
            && !self.math.formula
            && self.passing.is_none();
        if !prose {
            return false;
        }
        self.math.span = Some(Span {
            delimiter,
            source: String::new(),
            state: match delimiter {
                Delimiter::Dollar => State::Opened,
                Delimiter::Paren => State::Inside,
            },
            slashed: false,
        });
        self.started = true;
        self.previous = character;
        true
    }

    /// Reads `character` into the span being held. Answers whether it was
    /// taken; `false` leaves it to the rest of the scan, the span settled.
    pub(super) fn spans(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut span) = self.math.span.take() else {
            return false;
        };
        match span.state {
            State::Opened if matches!(character, ' ' | '\t' | ',' | '\n') => {
                self.say("$", say);
                return false;
            }
            State::Opened if character == '$' => {
                // Two dollars in a line open nothing.
                self.say("$$", say);
                self.taken(character);
                return true;
            }
            State::Opened | State::Inside => {}
            State::Closing => {
                let word = character.is_alphanumeric() || character == '_';
                if !word && !span.source.ends_with([' ', '\t']) {
                    self.spoken(&span, room, say);
                    return false;
                }
                // No span: the dollar that opened it is a dollar, and the one
                // that would have closed it may open the next.
                span.source.push('$');
                self.unspanned(&span, room, say);
                return self.spans(character, room, say);
            }
        }

        if character == '/' && span.source.ends_with(":/") {
            // An address, read again whole with the slash that made it one.
            span.source.push(character);
            self.unspanned(&span, room, say);
            return true;
        }
        if matches!(character, '\n' | '`') || span.source.len() >= SPAN_CAP {
            self.unspanned(&span, room, say);
            return self.spans(character, room, say);
        }

        self.taken(character);
        match span.delimiter {
            Delimiter::Dollar if character == '$' && !span.slashed => {
                span.state = State::Closing;
            }
            Delimiter::Paren if character == ')' && span.slashed => {
                span.source.pop();
                self.spoken(&span, room, say);
                return true;
            }
            Delimiter::Dollar | Delimiter::Paren => {
                span.state = State::Inside;
                span.slashed = character == '\\' && !span.slashed;
                span.source.push(character);
            }
        }
        self.math.span = Some(span);
        true
    }

    /// Settles a span the message or the line ended in, as the end of a line
    /// would have.
    pub(super) fn unspan(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.math.span.is_some() {
            self.spans('\n', room, say);
        }
    }

    /// Tells the watch on the line a character the span took, as the scan
    /// would have, since the span keeps it from reaching it.
    fn taken(&mut self, character: char) {
        let mut bytes = [0; 4];
        self.watches(character.encode_utf8(&mut bytes));
        self.previous = character;
    }

    /// A span that closed, drawn by the drawers or shown as written.
    fn spoken(&mut self, span: &Span, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let room = Room {
            columns: room,
            glyphs: self.glyphs,
        };
        let drawn = match self.math.mode {
            Mode::Written => None,
            // Until a picture can be shown in a line, a span is drawn as text.
            Mode::Text | Mode::Image => self.math.drawers.inline(&span.source, &room),
        };
        let Some(runs) = drawn else {
            let (open, close) = span.delimiter.marks();
            self.say(&format!("{open}{}{close}", span.source), say);
            return;
        };
        for run in runs {
            // Emphasis is the one slot a run changes; the rest of what the
            // line wears is the line's.
            let leant = self.line.emphasis.leant;
            self.line.emphasis.leant |= run.slot == Slot::Emphasis;
            self.say(&run.text, say);
            self.line.emphasis.leant = leant;
        }
    }

    /// A span that turned out to be none: its opener as written, and what it
    /// held read again as the prose it was.
    fn unspanned(
        &mut self,
        span: &Span,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.say(span.delimiter.marks().0, say);
        self.again(&span.source, room, say);
    }
}

/// What of a display block written `source` the math drawer is handed: the
/// formula between `$$` and `$$` or `\[` and `\]`, or a named environment from
/// `\begin` through its `\end`. `None` where anything follows what closes it,
/// so the block is shown as written.
pub(super) fn display(source: &str) -> Option<&str> {
    let source = source.trim_start_matches([' ', '\t']);
    let source = source.strip_suffix('\n').unwrap_or(source);
    let (body, after) = if let Some(rest) = source.strip_prefix("$$") {
        rest.split_once("$$")?
    } else if let Some(rest) = source.strip_prefix("\\[") {
        rest.split_once("\\]")?
    } else {
        let rest = source.strip_prefix("\\begin{")?;
        let (name, _) = rest.split_once('}')?;
        let end = format!("\\end{{{name}}}");
        let at = source.find(&end)?.saturating_add(end.len());
        (source.get(..at)?, source.get(at..)?)
    };
    let body = body.trim();
    (after.trim().is_empty() && !body.is_empty()).then_some(body)
}

/// A display block written `source` as a math fence holding every character
/// of it, markers and all: a fence of more backticks than any run in it, so
/// nothing in it closes the fence.
pub(super) fn framed(source: &str) -> String {
    let longest = source
        .split(|character| character != '`')
        .map(str::len)
        .max();
    let fence = "`".repeat(longest.unwrap_or(0).saturating_add(1).max(3));
    let body = source.strip_suffix('\n').unwrap_or(source);
    format!("{fence}math\n{body}\n{fence}\n")
}

/// Whether a display math block written `source` is a fence, rather than
/// math between markers of its own.
pub(super) fn fenced(source: &str) -> bool {
    source
        .trim_start_matches([' ', '\t'])
        .starts_with(['`', '~'])
}
