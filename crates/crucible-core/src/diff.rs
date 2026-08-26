//! What a call changed, as the reader sees it.
//!
//! For the reader and for nobody else, which is the whole of why this is a type
//! rather than the two versions of a file it was made from. A tool that changed
//! a file is the only thing that ever holds both, and it holds them for the
//! length of one call; anything downstream would have to work the change out
//! again from bytes it no longer has. So the tool says it once, here, on its way
//! out.
//!
//! The model is never sent it. [`crate::ToolOutput::forget_diff`] is where it
//! comes off, at the one point the reader's copy and the model's part company —
//! a diff is drawn once, and a transcript is replayed every turn for the rest of
//! the session, so one that kept a diff per edit would grow with what had been
//! *shown* where what bounds it is what was *said*.
//!
//! And it reaches no log, error or panic payload either. A diff of a file
//! holding a key is a key, so [`Debug`] is written by hand and redacts, the same
//! as everything else in this crate that carries what a file said.
//!
//! Both bounds are taken here rather than trusted from above: a diff crosses a
//! thread and is held until it is drawn, and a producer that forgot to cut one
//! would be a screenful of rows and a megabyte of them.

use core::fmt;

/// What a change did to one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Left alone. Drawn anyway, so the lines that did move have something
    /// around them to be read against.
    Kept,
    /// Taken out.
    Removed,
    /// Put in.
    Added,
}

/// One line of a diff.
#[derive(Clone, PartialEq, Eq)]
pub struct Line {
    number: usize,
    text: Box<str>,
    change: Change,
}

impl fmt::Debug for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Line")
            .field("number", &self.number)
            .field("text", &"[redacted]")
            .field("change", &self.change)
            .finish()
    }
}

impl Line {
    /// The most of one line a diff carries.
    ///
    /// Well past any terminal a person reads a diff on, and well short of what
    /// one line of a minified file is. Whatever draws this clips to its own
    /// window; the cut here is about what crosses a thread and waits to be
    /// drawn, not about what fits.
    pub const TEXT: usize = 1024;

    /// Takes one line, at `number` in the file, cut to [`Line::TEXT`].
    ///
    /// The number is the file's own, counted from one, so the gutter beside a
    /// drawn row is the number the reader would find that line at.
    #[must_use]
    pub fn new(number: usize, change: Change, text: impl Into<Box<str>>) -> Self {
        let text = text.into();
        let text = match text.char_indices().nth(Self::TEXT) {
            Some((at, _)) => text.get(..at).unwrap_or_default().into(),
            None => text,
        };

        Self {
            number,
            text,
            change,
        }
    }

    /// Which line of the file this is.
    #[must_use]
    pub fn number(&self) -> usize {
        self.number
    }

    /// What the line says, with its ending taken off.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Which way it went.
    #[must_use]
    pub fn change(&self) -> Change {
        self.change
    }
}

/// The lines a call changed, and how many of them there were.
#[derive(Clone, PartialEq, Eq)]
pub struct Diff {
    lines: Box<[Line]>,
    added: usize,
    removed: usize,
    dropped: usize,
}

impl fmt::Debug for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Diff")
            .field("lines", &"[redacted]")
            .field("added", &self.added)
            .field("removed", &self.removed)
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl Diff {
    /// The most lines a diff carries.
    ///
    /// Rather more than a screen, because a reader scrolls back to what a tool
    /// did and that is where the rows above the fold are read. What it is not is
    /// a whole file: a change to every line of one is a change the reader takes
    /// on trust from the count, and this type is the detail under that count
    /// rather than the file.
    pub const LINES: usize = 64;

    /// Takes the lines a tool worked out, cut to [`Diff::LINES`].
    ///
    /// The counts are of everything that moved and not of what survived the
    /// cut, because they are what the row above the block says: a reader told
    /// that nine lines went in has been told the truth about the call whether or
    /// not all nine are under it. What the cut took off is [`Diff::dropped`],
    /// and it is said in the same place, since a block that stopped without
    /// saying so reads as the whole of what happened.
    #[must_use]
    pub fn new(lines: impl IntoIterator<Item = Line>) -> Self {
        let (mut added, mut removed) = (0_usize, 0_usize);
        let mut kept = Vec::new();
        let mut dropped = 0_usize;

        for line in lines {
            match line.change() {
                Change::Added => added = added.saturating_add(1),
                Change::Removed => removed = removed.saturating_add(1),
                Change::Kept => {}
            }

            if kept.len() < Self::LINES {
                kept.push(line);
            } else {
                dropped = dropped.saturating_add(1);
            }
        }

        Self {
            lines: kept.into_boxed_slice(),
            added,
            removed,
            dropped,
        }
    }

    /// The lines, in the order they are drawn.
    #[must_use]
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// How many lines the change put in.
    #[must_use]
    pub fn added(&self) -> usize {
        self.added
    }

    /// How many it took out.
    #[must_use]
    pub fn removed(&self) -> usize {
        self.removed
    }

    /// How many lines the cut took off the end.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Bytes retained for drawing this change again.
    ///
    /// The line text is the variable part; numbers, tags and the boxed slice are
    /// fixed per bounded line and charged conservatively beside it. Used by the
    /// renderer's retained-record ceiling, never as a serialized size.
    #[must_use]
    pub fn retained(&self) -> usize {
        self.lines.iter().fold(0_usize, |held, line| {
            held.saturating_add(line.text.len())
                .saturating_add(std::mem::size_of::<Line>())
        })
    }

    /// Whether the call left the file exactly as it was.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

#[cfg(test)]
mod tests;
