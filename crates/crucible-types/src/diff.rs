//! What a call changed, as the reader sees it.
//!
//! For the reader and for nobody else, which is the whole of why this is a type
//! rather than the two versions of a file it was made from. A tool that changed
//! a file is the only thing that ever holds both, and it holds them for the
//! length of one call; anything downstream would have to work the change out
//! again from bytes it no longer has. So the tool says it once, here, on its way
//! out. An installed agent runs its own tools, and its report of a call is the
//! one place the change is said, so that report is read into one here too.
//!
//! Preview lines never enter provider requests. The live result a tool returns
//! drops them on its way into the record kept for the transcript, which keeps
//! [`crate::Changed`] as display metadata. Provider projections send result text and attachments;
//! neither the preview lines nor these counts change the request bytes.
//!
//! Bounded previews may be retained in the protected session's display history
//! and drawn again on resume. They can contain sensitive file text, so they
//! never reach ordinary logs, errors or panic payloads. [`Debug`] is written
//! by hand and redacts, like other values carrying file contents.
//!
//! A diff may name the file it changed, as the tool that made it was asked for
//! it. The name is the model's, so it is hostile text: whatever draws it makes
//! it safe to draw, and [`Debug`] redacts it with the lines, since a file's
//! name can say what is in it. A diff kept before it could name its file has
//! none, and is drawn without one.
//!
//! Every bound is taken here rather than trusted from above: a diff crosses a
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
    path: Option<Box<str>>,
    lines: Box<[Line]>,
    added: usize,
    removed: usize,
    dropped: usize,
}

impl fmt::Debug for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Diff")
            .field("path", &self.path.as_ref().map(|_| "[redacted]"))
            .field("lines", &"[redacted]")
            .field("added", &self.added)
            .field("removed", &self.removed)
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl Diff {
    /// Restores a bounded display preview without inventing omitted lines.
    ///
    /// Returns `None` when its totals cannot describe the retained lines or
    /// when its retained line shape could not come from a live bounded diff.
    #[must_use]
    pub fn restored(
        lines: Vec<Line>,
        added: usize,
        removed: usize,
        dropped: usize,
    ) -> Option<Self> {
        if lines.len() > Self::LINES || (dropped > 0 && lines.len() != Self::LINES) {
            return None;
        }
        let visible_added = lines
            .iter()
            .filter(|line| line.change() == Change::Added)
            .count();
        let visible_removed = lines
            .iter()
            .filter(|line| line.change() == Change::Removed)
            .count();
        if added < visible_added
            || removed < visible_removed
            || added
                .checked_sub(visible_added)?
                .checked_add(removed.checked_sub(visible_removed)?)?
                > dropped
        {
            return None;
        }
        Some(Self {
            path: None,
            lines: lines.into_boxed_slice(),
            added,
            removed,
            dropped,
        })
    }

    /// The most lines a diff carries.
    ///
    /// Rather more than a screen, because a reader scrolls back to what a tool
    /// did and that is where the rows above the fold are read. What it is not is
    /// a whole file: a change to every line of one is a change the reader takes
    /// on trust from the count, and this type is the detail under that count
    /// rather than the file.
    pub const LINES: usize = 64;

    /// The most of a path a diff carries, in characters.
    ///
    /// Longer than any path a reader would want spelled out, which whatever
    /// draws it shortens to its window anyway; the cut here is about what
    /// crosses a thread and waits to be drawn.
    pub const PATH: usize = 1024;

    /// Names the file this changed, cut to its last [`Diff::PATH`] characters.
    ///
    /// The end is kept because it is the file's own name, which is what a
    /// reader looks for. An empty name is no name.
    #[must_use]
    pub fn in_file(mut self, path: impl Into<Box<str>>) -> Self {
        let path = path.into();
        let skip = path.chars().count().saturating_sub(Self::PATH);
        let path = match path.char_indices().nth(skip) {
            Some((at, _)) if skip > 0 => path.get(at..).unwrap_or_default().into(),
            _ => path,
        };
        self.path = (!path.is_empty()).then_some(path);
        self
    }

    /// The file this changed, when the tool that made it named one.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

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
            path: None,
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
    /// The line text and the path are the variable part; numbers, tags and the
    /// boxed slice are fixed per bounded line and charged conservatively beside
    /// it. Used by the renderer's retained-record ceiling, never as a
    /// serialized size.
    #[must_use]
    pub fn retained(&self) -> usize {
        let named = self.path.as_deref().map_or(0, str::len);
        self.lines.iter().fold(named, |held, line| {
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

mod between;

#[cfg(test)]
mod tests;
