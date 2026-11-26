//! Code in a line: a span opened by a run of backticks in prose and closed
//! only by a run of exactly as many, so a span opened by two holds one, and a
//! longer or shorter run inside it is code like the rest.
//!
//! What it holds is drawn in the code slot without the runs either side of
//! it, and with one space taken off each end where both ends have one and it
//! is not all spaces, so a span can begin or end with a backtick of its own.
//! No character of it is read as markdown, for the reason none in a fence is:
//! code is full of markers. `*ptr` is a pointer, `_private` is a name and
//! `**kwargs` is how one language spells an argument, every one of them a
//! thing a coding agent writes far more often than it writes emphasis inside
//! a span.
//!
//! Which run closes a span is only known once a run of its length has ended,
//! so what it holds is held until then, as a span of math is: nothing of it
//! reaches the record before the span settles, and the live foot shows it as
//! written. A span ends where its line does and at [`MOST`] bytes; either
//! leaves the run that opened it as the backticks it was, and reads what it
//! held again as the prose it was, where a later run may open a span of its
//! own. A backslash it ended its line on is drawn as written rather than read
//! as a break: the line it was in front of is the span's to say, and the span
//! is what it would have been in.

use super::{HeldMarks, MOST, Markdown};
use crate::color::Slot;
use crate::escape::Escapes;
use crate::row::Row;
use crate::width;

/// A span of code being held.
#[derive(Debug)]
pub(super) struct Span {
    /// How many backticks opened it, and so how many close it.
    opener: usize,
    /// What has arrived since, the backticks that may close it included.
    source: String,
    /// How many backticks `source` ends with.
    closing: usize,
}

impl Span {
    /// A span opened by `opener` backticks that holds nothing yet.
    pub(super) const fn opened(opener: usize) -> Self {
        Self {
            opener,
            source: String::new(),
            closing: 0,
        }
    }

    fn take(&mut self, character: char) {
        self.source.push(character);
        self.closing = if character == '`' {
            self.closing + 1
        } else {
            0
        };
    }
}

impl Markdown {
    /// Settles a run of `count` backticks in prose at `next`, the character
    /// that ended it. Answers whether `next` was taken: it opens a span and is
    /// the first thing the span holds, unless the line ended there and the
    /// run is the backticks it was.
    pub(super) fn opens_code(
        &mut self,
        count: usize,
        next: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        if next == '\n' {
            self.unmark(backticks(count), say);
            return false;
        }
        // Told to the watch on the line already, as the run was.
        let mut span = Span::opened(count);
        span.take(next);
        self.ticked = Some(span);
        self.previous = next;
        true
    }

    /// Reads `character` into the span of code being held. Answers whether
    /// it was taken; `false` leaves it to the rest of the scan, the span
    /// settled.
    pub(super) fn ticks(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut span) = self.ticked.take() else {
            return false;
        };
        if character != '`' && span.closing == span.opener {
            self.coded(&span, say);
            return false;
        }
        if character == '\n' || span.source.len() >= MOST {
            // What it held may open a span of its own, which the caller hands
            // `character` to.
            self.unmark(backticks(span.opener), say);
            self.again(&span.source, room, say);
            // A backslash the span ended its line on stays as written, as it
            // would in a span the next line closed: it breaks no line.
            if character == '\n' && std::mem::take(&mut self.escaped) {
                self.say("\\", say);
                self.previous = '\\';
            }
            return false;
        }
        span.take(character);
        let mut bytes = [0; 4];
        self.watches(character.encode_utf8(&mut bytes));
        self.previous = character;
        self.ticked = Some(span);
        true
    }

    /// The span of code being held, as written and clipped to `columns`, for
    /// the live foot: nothing of it is in the record yet.
    ///
    /// No more of the run that opened it than the foot has columns for, since
    /// the rest is past its edge.
    pub(super) fn ticking(&self, columns: usize) -> Option<Row> {
        let span = self.ticked.as_ref()?;
        let mut escapes = Escapes::default();
        let shown: String = std::iter::repeat_n('`', span.opener.min(columns))
            .chain(span.source.chars())
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

    /// A span a run as long as its opener closed: what it held without that
    /// run, in the code slot.
    fn coded(&mut self, span: &Span, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let held = span.source.len().saturating_sub(span.opener);
        let code = span.source.get(..held).unwrap_or_default();
        let code = match code
            .strip_prefix(' ')
            .and_then(|code| code.strip_suffix(' '))
        {
            Some(inner) if code.bytes().any(|byte| byte != b' ') => inner,
            _ => code,
        };
        let was = std::mem::replace(&mut self.line.code, true);
        self.say(code, say);
        self.line.code = was;
    }
}

/// A run of `count` backticks, to be said back as written.
const fn backticks(count: usize) -> HeldMarks {
    HeldMarks {
        mark: '`',
        count,
        opened: false,
    }
}
