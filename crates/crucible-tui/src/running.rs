//! What is still running, listed where the box was.
//!
//! **Rows, not a screen.** Like every component here this hands back rows and
//! draws nothing; how tall the window is and what happens to these rows
//! afterwards are the caller's, and for this one what happens is that they are
//! taken back rather than written down. A command still running is a thing that
//! is happening, and a thing that is happening cannot also be a line in the
//! record of what has happened.
//!
//! **The chrome is the chrome that stands a result whole**, because a reader
//! reaching either has reached for the same thing: everything about one item, in
//! the room the box was using, closed with the same key. The rows themselves are
//! [`crate::Menu`]'s, so the row a key acts on is marked *and* coloured — a
//! terminal drawing no colour still says which one.
//!
//! **The footer names only what it can do.** Stopping a command is offered
//! because that is why this is reachable at all; showing one is offered because
//! five rows of a sample never was the whole of it. Neither is named where there
//! is nothing to act on, which cannot happen — with nothing running there is no
//! count on the row below to have opened this.

use std::time::Duration;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::menu::{Listed, Menu};
use crate::row::Row;
use crate::width::clip;

/// What the list keeps in front of a name and between the two columns.
///
/// The figures [`Menu`] lays a chosen list out with — the mark and its space, and
/// the gap before the second column. Named here because this is where a name is
/// cut to leave the second column its room, and cutting it against the wrong
/// figures would push the counts off the edge, which is the one thing on the row a
/// reader cannot find anywhere else.
const AROUND: usize = 5;

/// Rows spent on everything that is not a command: the rule, the blanks around
/// the heading, and the blank and the footer at the foot.
const CHROME: usize = 6;

/// What the heading says.
const TITLE: &str = "Still running";

/// The keys, in the order somebody reaches for them.
const KEYS: &str = "esc to close · enter shows it · x stops it";

/// And the same where the window has no room for all three.
const CLOSE: &str = "esc to close";

/// One command the list names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command<'a> {
    /// The number its call was answered with, which is what the model calls it.
    pub number: usize,
    /// The command, in the words the call sent.
    pub called: &'a str,
    /// How long it has been running.
    pub running: Duration,
    /// How many lines it has printed.
    pub lines: usize,
    /// How many bytes it has printed.
    pub bytes: usize,
}

/// Every command still running, with one of them marked.
#[derive(Debug, Clone, Copy)]
pub struct Running<'a> {
    /// What to list, in the order they were started.
    pub shown: &'a [Command<'a>],
    /// Which row a key would act on.
    pub at: usize,
}

impl Running<'_> {
    /// How many rows this needs to draw `shown` whole.
    #[must_use]
    pub fn height(&self) -> usize {
        if self.shown.is_empty() {
            return 0;
        }

        self.shown.len().saturating_add(CHROME)
    }

    /// The list, in `room` rows, and empty where nothing fits.
    ///
    /// Empty rather than as-much-as-fits, for the reason every component here
    /// answers that way: a panel taller than the window is a region the renderer
    /// cannot rewind over, and half a list of processes is worse than the count
    /// that was already on screen.
    #[must_use]
    pub fn rows(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        if self.shown.is_empty() || self.height() > room {
            return Vec::new();
        }

        // The counts first, because they decide how much of a name there is room
        // for. A command line can be any length and it is already in the
        // transcript above; how long one has been running and how much it has
        // printed is on this row and nowhere else, so the name is what gives way.
        let counts: Vec<String> = self.shown.iter().map(|one| one.says(glyphs)).collect();
        let widest = counts
            .iter()
            .map(|says| crate::width::columns(says))
            .max()
            .unwrap_or_default();
        let room = columns.saturating_sub(widest).saturating_sub(AROUND).max(1);

        let named: Vec<(String, String)> = self
            .shown
            .iter()
            .zip(counts)
            .map(|(one, says)| (clip(&one.name(), room).to_owned(), says))
            .collect();
        let listed: Vec<Listed<'_>> = named
            .iter()
            .map(|(name, says)| Listed { name, says })
            .collect();

        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
            Row::new().then(Slot::Strong, clip(TITLE, columns).to_owned()),
            Row::new(),
        ];

        rows.extend(
            Menu {
                shown: &listed,
                chosen: Some(self.at.min(self.shown.len().saturating_sub(1))),
            }
            .rows(columns, glyphs),
        );

        rows.push(Row::new());
        rows.push(Row::new().then(Slot::Quiet, footer(columns)));

        rows
    }
}

impl Command<'_> {
    /// What the first column says: the number a key acts on, and the command.
    fn name(&self) -> String {
        format!("{}. {}", self.number, self.called)
    }

    /// And the second: how long, how much.
    fn says(&self, glyphs: Glyphs) -> String {
        let dot = glyphs.dot();

        format!(
            "{} {dot} {} {dot} {}",
            elapsed(self.running),
            counted(self.lines, "line"),
            sized(self.bytes)
        )
    }
}

/// The row under the list, saying only what fits.
fn footer(columns: usize) -> String {
    if crate::width::columns(KEYS) > columns {
        return clip(CLOSE, columns).to_owned();
    }

    KEYS.to_owned()
}

/// How long a command has been running, in the units somebody reads it in.
///
/// The same shape the row above the box uses for a turn, because they are the
/// same fact about two different things and a pair written two ways reads as two
/// kinds of fact.
fn elapsed(running: Duration) -> String {
    let seconds = running.as_secs();

    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds}s"),
        (minutes, seconds) => format!("{minutes}m {seconds:02}s"),
    }
}

/// `count` of `what`, pluralised.
fn counted(count: usize, what: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };

    format!("{count} {what}{plural}")
}

/// A count of bytes, in the units somebody reads it in.
fn sized(bytes: usize) -> String {
    for (unit, over) in [("MB", 1_000_000), ("kB", 1_000)] {
        if bytes >= over {
            let (whole, tenth) = (bytes / over, (bytes % over) * 10 / over);

            return match tenth {
                0 => format!("{whole} {unit}"),
                _ => format!("{whole}.{tenth} {unit}"),
            };
        }
    }

    format!("{bytes} B")
}

#[cfg(test)]
mod tests;
