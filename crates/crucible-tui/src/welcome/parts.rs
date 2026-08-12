//! The rows the welcome component is made of, before anything frames them.
//!
//! Every width draws from this one list. What differs between them is the
//! assembly — two columns, one column, or none — and never the content, so a
//! tip added here appears at every width or at none of them.

use crate::color::Slot;
use crate::row::Row;
use crate::width;

use super::glyphs::Glyphs;
use super::{Recent, Welcome, fit};

/// The name, as letters, for every form that is not drawing it as art.
const WORDMARK: &str = "CRUCIBLE";

/// The heading over what happened in this directory before.
const SESSIONS: &str = "Recent sessions";

/// What that heading has under it in a directory nobody has worked in. The
/// heading itself stays: its absence would be a different thing than its
/// emptiness, and only one of the two is worth reading.
const NONE_YET: &str = "No recent sessions";

/// How to reach the sessions that did not fit.
const MORE: &str = "/resume for more";

/// How many sessions a column holds. More than this and the list stops being
/// something the eye takes in at once, which is the only job it has.
pub(super) const SHOWN: usize = 3;

/// The heading over what the prompt itself reacts to.
const TIPS: &str = "Tips";

/// Three characters that do something the moment they are typed at the prompt.
///
/// Not flags and not files: those are in `--help`, and this is the category
/// `--help` cannot hold, because there is nothing to look up until you already
/// know the character exists.
const AFFORDANCES: [(&str, &str); 3] = [
    ("/", "opens the command list"),
    ("!", "runs a shell command here"),
    ("@", "points crucible at a file"),
];

/// At least this much space between a session's title and when it happened, so
/// that the two never read as one string.
const GAP: usize = 1;

/// The name, as letters.
///
/// Shortened like anything else when the terminal is narrower than the word.
/// Eight columns is not much to ask for, but a terminal that will not give them
/// up gets a name it can hold rather than one it wraps.
pub(super) fn name(columns: usize, glyphs: Glyphs) -> Row {
    Row::new().then(Slot::Strong, fit::elide(WORDMARK, columns, glyphs))
}

/// The name, as art where the font has it and as letters otherwise.
///
/// Only the form with rows to spend asks for this; every narrower one takes
/// [`name`] directly.
pub(super) fn wordmark(columns: usize, glyphs: Glyphs) -> Vec<Row> {
    match glyphs.art() {
        Some(art) => art
            .iter()
            .map(|row| Row::new().then(Slot::Strong, *row))
            .collect(),
        None => vec![name(columns, glyphs)],
    }
}

/// Which model is being asked, and how hard it is being asked to think.
///
/// Nothing about the provider, which the model implies, and nothing about the
/// permission mode, which lives under the prompt where it changes.
pub(super) fn model(welcome: &Welcome<'_>, columns: usize, glyphs: Glyphs) -> Row {
    if let Some(effort) = welcome.effort {
        let row = Row::plain(welcome.model)
            .then(Slot::Quiet, format!(" {} ", glyphs.dot()))
            .then(Slot::Plain, format!("{effort} effort"));

        if row.columns() <= columns {
            return row;
        }
    }

    // No room for both. The effort goes whole rather than being shortened: half
    // a model name still says which model, and half a word for how hard it is
    // thinking says nothing at all.
    Row::plain(fit::elide(welcome.model, columns, glyphs))
}

/// Where crucible is working.
///
/// Drawn because every tool path is relative to it, and somebody who started
/// crucible in the wrong directory should find that out here rather than after
/// the first tool call.
pub(super) fn root(welcome: &Welcome<'_>, columns: usize, glyphs: Glyphs) -> Row {
    Row::new().then(Slot::Quiet, fit::shorten(welcome.root, columns, glyphs))
}

/// What sits beside the identity in the wide form and under it in the narrow
/// one: the sessions, a rule, and the tips.
///
/// The rule is what keeps them two lists. Sessions are a fact about this
/// directory and tips are an instruction about the prompt, and run together
/// they read as one list of six unrelated things.
pub(super) fn aside(welcome: &Welcome<'_>, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut rows = sessions(welcome, columns, glyphs);

    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(columns)));
    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Strong, TIPS));
    rows.push(Row::new());
    rows.extend(tips(glyphs));
    rows
}

/// The heading, then what happened in this directory before.
fn sessions(welcome: &Welcome<'_>, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut rows = vec![Row::new().then(Slot::Strong, SESSIONS), Row::new()];

    if welcome.sessions.is_empty() {
        rows.push(Row::new().then(Slot::Quiet, NONE_YET));
        return rows;
    }

    for (index, session) in welcome.sessions.iter().take(SHOWN).enumerate() {
        rows.push(recent(index + 1, session, columns, glyphs));
    }

    if welcome.sessions.len() > SHOWN {
        rows.push(Row::new().then(Slot::Quiet, MORE));
    }

    rows
}

/// One session: its number, what it was about, and when it was.
///
/// Numbered because the number is what gets typed to reach it. The title gives
/// up columns and the time never does — half a title still identifies a
/// session, and half a date identifies nothing.
fn recent(number: usize, session: &Recent<'_>, columns: usize, glyphs: Glyphs) -> Row {
    let when = width::columns(session.when);

    let mut row = Row::new()
        .then(Slot::Quiet, number.to_string())
        .then(Slot::Plain, "  ");

    let room = columns.saturating_sub(row.columns() + when + GAP);
    row.push(Slot::Plain, fit::elide(session.title, room, glyphs));

    // Against the right edge of the column, which is where the eye goes looking
    // for it once there is more than one row of them.
    row.pad(columns.saturating_sub(when));
    row.push(Slot::Quiet, session.when);
    row
}

/// One row per affordance: the mark, the character, and what it does.
fn tips(glyphs: Glyphs) -> Vec<Row> {
    AFFORDANCES
        .iter()
        .map(|(key, does)| {
            Row::new()
                .then(Slot::Quiet, glyphs.dot())
                .then(Slot::Plain, " ")
                .then(Slot::Accent, *key)
                .then(Slot::Plain, format!("  {does}"))
        })
        .collect()
}
