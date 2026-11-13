//! What was read handed to the engine, the engine's reading held to it, and
//! the lines the engine draws checked before any reaches a row.
//!
//! Invariants:
//!
//! - The engine reads what [`scan`](crate::scan) kept, never the source as it
//!   was written, and its reading agrees with that one or nothing is drawn.
//!   A flowchart's has the same boxes, with the same labels, joined by as
//!   many lines with the same labels. A state diagram's has as many
//!   transitions with the same labels, each state given a label labelled
//!   alike, and no more states than its cap. Its states are held to neither
//!   the scan's names nor its count: the engine names its start and end
//!   markers and its notes by a scheme of its own, adds markers where a
//!   transition reaches a composite state, and leaves out markers joined to
//!   no state. An
//!   entity-relationship diagram's has the same entities, by name, and as
//!   many relationships.
//! - A line the engine draws with a control character in it refuses the
//!   diagram; under the ASCII glyph set, so does a character that is neither
//!   ASCII nor written in the source.
//! - The engine lays a diagram out once, for its room, at its own spacing:
//!   never again at a tighter one, since each layout costs as much as the
//!   first. Every line it draws is measured, and a drawing wider than the
//!   room is refused, never cut.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crucible_tui::drawing::Catching;
use crucible_tui::{Glyphs, columns};
use mermaid_text::{ErDiagram, Error, Graph, RenderOptions, parser, render_with_options};

use crate::scan::{Read, Scanned};
use crate::{Caps, Kind};

/// Why the engine's drawing of a diagram is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unlaid {
    /// The engine read it otherwise, failed, or drew what a row may not hold.
    Unread,
    /// It is wider than the room it was laid out for.
    TooWide,
}

/// The lines of a diagram's drawing, each fitting its room, and the columns
/// the widest of them takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drawing {
    pub(crate) lines: Vec<String>,
    pub(crate) natural: usize,
}

/// `scanned` drawn by the engine in no more than `room` columns.
pub(crate) fn lay(scanned: &Scanned, room: usize, glyphs: Glyphs) -> Result<Drawing, Unlaid> {
    // The engine is code this crate does not own, handed text a model wrote.
    // The read above bounds what reaches it and the checks below bound what
    // comes back, but a panic inside it must still leave the answer shown as
    // written rather than take the drawing thread down; the release profile
    // unwinds, so it is caught here. The guard tells the session's panic hook
    // the panic is caught, so it is said in the transcript rather than
    // written into the frame being drawn.
    let _catching = Catching::begin();
    catch_unwind(AssertUnwindSafe(|| drawn(scanned, room, glyphs))).unwrap_or(Err(Unlaid::Unread))
}

fn drawn(scanned: &Scanned, room: usize, glyphs: Glyphs) -> Result<Drawing, Unlaid> {
    agrees(scanned).then_some(()).ok_or(Unlaid::Unread)?;
    let lines = rendered(scanned, room, glyphs)?;
    let natural = widest(&lines);
    if natural > room {
        return Err(Unlaid::TooWide);
    }
    Ok(Drawing { lines, natural })
}

/// Whether the engine reads `scanned` as it was read.
fn agrees(scanned: &Scanned) -> bool {
    let source = scanned.source.as_str();
    match scanned.kind {
        Kind::Flowchart => {
            parser::flowchart::parse(source).is_ok_and(|graph| same(&graph, &scanned.read))
        }
        Kind::State => {
            parser::state::parse(source).is_ok_and(|graph| state_alike(&graph, &scanned.read))
        }
        // The scan refuses a class diagram before it reaches the engine.
        Kind::Class => false,
        Kind::EntityRelationship => {
            parser::er::parse(source).is_ok_and(|diagram| entities_alike(&diagram, &scanned.read))
        }
    }
}

/// Whether `graph`, a state diagram, holds the transitions `read` found,
/// labelled alike, labels each state `read` gave a label to alike, and has
/// no more states than its cap. Its states are not held to `read`'s, by name
/// or by count, since the engine names, adds and leaves out start and end
/// markers by rules of its own.
fn state_alike(graph: &Graph, read: &Read) -> bool {
    graph.nodes.len() <= Caps::STATE.nodes
        && joined_alike(graph, read)
        && labelled(graph, read, false)
}

/// Whether `diagram` holds the entities `read` found, by name, and as many
/// relationships.
fn entities_alike(diagram: &ErDiagram, read: &Read) -> bool {
    let names: BTreeSet<&str> = diagram
        .entities
        .iter()
        .map(|entity| entity.name.as_str())
        .collect();
    names.len() == diagram.entities.len()
        && names
            .iter()
            .copied()
            .eq(read.boxes.keys().map(String::as_str))
        && diagram.relationships.len() == read.lines.len()
}

/// Whether `graph` holds the boxes and lines `read` found, labelled alike.
fn same(graph: &Graph, read: &Read) -> bool {
    let names: BTreeSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    names
        .iter()
        .copied()
        .eq(read.boxes.keys().map(String::as_str))
        && joined_alike(graph, read)
        && labelled(graph, read, true)
}

/// Whether `graph` holds as many lines as `read` found, with the same labels.
fn joined_alike(graph: &Graph, read: &Read) -> bool {
    let mut drawn: Vec<String> = graph
        .edges
        .iter()
        .map(|edge| edge.label.as_deref().map(plain).unwrap_or_default())
        .collect();
    let mut written: Vec<String> = read
        .lines
        .iter()
        .map(|label| label.as_deref().map(plain).unwrap_or_default())
        .collect();
    drawn.sort();
    written.sort();
    drawn == written
}

/// Whether each box `read` gave a label to the engine labels alike, as a box
/// or as the group drawn around boxes; and, where `named_too`, whether each
/// box given none is labelled with its name.
fn labelled(graph: &Graph, read: &Read, named_too: bool) -> bool {
    read.boxes.iter().all(|(name, label)| {
        if label.is_none() && !named_too {
            return true;
        }
        let written = plain(label.as_deref().unwrap_or(name));
        let boxed = graph
            .nodes
            .iter()
            .find(|node| node.id == *name)
            .map(|node| plain(&node.label));
        let grouped = graph
            .subgraphs
            .iter()
            .find(|group| group.id == *name)
            .map(|group| plain(&group.label));
        boxed.is_some_and(|drawn| drawn == written) || grouped.is_some_and(|drawn| drawn == written)
    })
}

/// `label` as compared between the two readings: with the line breaks Mermaid
/// writes as `<br>`, the quote it writes as `#34;`, and the spaces the engine
/// wraps a long label at, left out.
fn plain(label: &str) -> String {
    label
        .replace("<br/>", "")
        .replace("<br />", "")
        .replace("<br>", "")
        .replace("#34;", "\"")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// The engine's spacing between a flowchart's layers, and between the boxes
/// in one, as `mermaid-text` lays one out unasked.
const SPACING: (usize, usize) = (6, 2);

/// The engine's drawing of `scanned` for `room` columns, trimmed, and
/// checked.
///
/// Given its spacing, the engine lays a flowchart or a state diagram out once
/// rather than again at tighter spacing while it is wider than `room`; an
/// entity-relationship diagram's entities are set in rows as wide as `room`,
/// in one pass.
fn rendered(scanned: &Scanned, room: usize, glyphs: Glyphs) -> Result<Vec<String>, Unlaid> {
    let ascii = glyphs == Glyphs::Ascii;
    let options = RenderOptions {
        max_width: Some(room),
        ascii,
        gaps_override: Some(SPACING),
        ..RenderOptions::default()
    };
    let drawn = render_with_options(&scanned.source, &options).map_err(|error| match error {
        Error::TooWide { .. } => Unlaid::TooWide,
        Error::EmptyInput | Error::UnsupportedDiagram(_) | Error::ParseError(_) => Unlaid::Unread,
    })?;
    let drawn = if ascii { in_ascii(&drawn) } else { drawn };
    let stray = |character: char| {
        character.is_control() && character != '\n'
            || ascii && !character.is_ascii() && !scanned.source.contains(character)
    };
    if drawn.chars().any(stray) {
        return Err(Unlaid::Unread);
    }
    let mut lines: Vec<String> = drawn
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let blank = lines.iter().take_while(|line| line.is_empty()).count();
    lines.drain(..blank);
    if lines.is_empty() {
        return Err(Unlaid::Unread);
    }
    Ok(lines)
}

/// `drawn` with the strokes and heads the engine's own ASCII pass leaves as
/// they are drawn in ASCII as well.
fn in_ascii(drawn: &str) -> String {
    drawn
        .chars()
        .map(|character| match character {
            '╱' => '/',
            '╲' => '\\',
            '⟩' | '▶' | '►' => '>',
            '◄' => '<',
            '▼' => 'v',
            '▲' => '^',
            '╍' => '-',
            '╏' => '|',
            other => other,
        })
        .collect()
}

/// The columns the widest of `lines` takes.
fn widest(lines: &[String]) -> usize {
    lines.iter().map(|line| columns(line)).max().unwrap_or(0)
}

#[cfg(test)]
mod tests;
