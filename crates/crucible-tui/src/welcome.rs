//! The welcome component: what crucible draws before it asks for anything.
//!
//! Three assemblies over one list of rows. At eighty columns and above the
//! identity and what is beside it stand in two columns; from forty-six to
//! seventy-nine they stack, because a thirty-two column identity cannot get
//! narrower without the wordmark breaking; below forty-six the frame costs more
//! columns than it earns and goes, leaving the three lines that say what this
//! is, what it is asking, and where.
//!
//! It returns [`Row`]s rather than writing anything. That is what lets every
//! width be asserted with no terminal attached — and it is why a renderer that
//! is not a terminal would reimplement [`Row::paint`] rather than this file.

use crate::row::Row;
use crate::width;

mod card;
mod fit;
mod glyphs;
mod parts;

pub use glyphs::Glyphs;

/// The narrowest terminal that gets a frame at all.
const FRAMED_AT: usize = 46;

/// The narrowest terminal that gets two columns.
const PAIRED_AT: usize = 80;

/// The two-column form's left column. Fixed rather than shared out, because it
/// holds a wordmark that is the width it is.
const IDENTITY: usize = 32;

/// What the left column's content is moved right by, to keep the wordmark off
/// the edge beside it.
const INDENT: &str = " ";

/// One session, as it is drawn.
///
/// Both already spelled: what a session is called and how long ago it was are
/// decided where the sessions are read, and this crate names no domain type.
#[derive(Debug, Clone, Copy)]
pub struct Recent<'a> {
    /// What the session was about, in its own words.
    pub title: &'a str,
    /// How long ago it was.
    pub when: &'a str,
}

/// What the welcome component says.
#[derive(Debug, Clone, Copy)]
pub struct Welcome<'a> {
    /// The release, as it is drawn beside the name.
    pub version: &'a str,
    /// The model this session will ask.
    pub model: &'a str,
    /// How hard that model is being asked to think, where that is a thing it
    /// takes. Nothing is drawn in its place when it is not.
    pub effort: Option<&'a str>,
    /// The directory crucible is working in.
    pub root: &'a str,
    /// What happened in that directory before, newest first. More of them than
    /// fit is what puts the way to reach the rest on screen.
    pub sessions: &'a [Recent<'a>],
}

impl Welcome<'_> {
    /// The component, drawn for a terminal `columns` wide.
    ///
    /// Every row is exactly that wide wherever there is a frame to hold, and
    /// never wider anywhere: a row past the last column is one the terminal
    /// wraps itself, which puts the cursor a row below where the next frame
    /// expects it.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        if columns < FRAMED_AT {
            return self.bare(columns, glyphs);
        }
        if columns < PAIRED_AT {
            return self.stacked(columns, glyphs);
        }
        self.paired(columns, glyphs)
    }

    /// Two columns: the identity, and the sessions and tips beside it.
    fn paired(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let aside = columns - card::PAIRED - IDENTITY;
        let inset = IDENTITY - width::columns(INDENT);

        let mut left = vec![Row::new()];
        left.extend(parts::wordmark(inset, glyphs));
        left.push(Row::new());
        left.push(parts::model(self, inset, glyphs));
        left.push(Row::new());
        left.push(parts::root(self, inset, glyphs));

        let right = parts::aside(self, aside, glyphs);

        let height = left.len().max(right.len());
        let mut rows = Vec::with_capacity(height + 2);
        rows.push(card::top(self.version, columns, glyphs));

        // The shorter column is padded out with rows that hold nothing rather
        // than the card being cut to it: the frame is a rectangle, and a column
        // that ran out of things to say still owes its half of one.
        let mut left = left.into_iter();
        let mut right = right.into_iter();
        for _ in 0..height {
            rows.push(card::pair(
                Row::plain(INDENT).join(left.next().unwrap_or_default()),
                right.next().unwrap_or_default(),
                IDENTITY,
                aside,
                glyphs,
            ));
        }

        rows.push(card::bottom(columns, glyphs));
        rows
    }

    /// One column: the same rows, stacked, still inside the frame.
    fn stacked(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let inner = columns - card::STACKED;

        let mut body = vec![Row::new(), parts::name(inner, glyphs)];
        body.push(parts::model(self, inner, glyphs));
        body.push(parts::root(self, inner, glyphs));
        body.push(Row::new());
        body.extend(parts::aside(self, inner, glyphs));

        let mut rows = Vec::with_capacity(body.len() + 2);
        rows.push(card::top(self.version, columns, glyphs));
        rows.extend(body.into_iter().map(|row| card::framed(row, inner, glyphs)));
        rows.push(card::bottom(columns, glyphs));
        rows
    }

    /// No frame: what this is, what it is asking, and where.
    ///
    /// A terminal this narrow has no columns to spend on a border, and none to
    /// spare for a tip that would elide to nothing. What is left is the part
    /// that is a fact rather than an offer.
    fn bare(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        vec![
            parts::name(columns, glyphs),
            parts::model(self, columns, glyphs),
            parts::root(self, columns, glyphs),
        ]
    }
}

#[cfg(test)]
mod tests;
