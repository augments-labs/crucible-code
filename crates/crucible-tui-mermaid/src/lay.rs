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
//!   many relationships. A sequence diagram's has the same participants, in
//!   order, with the same labels, the same messages, in order, between the
//!   same participants with the same labels, and nothing else: no note,
//!   activation, block, group or numbering. A gantt's has the same sections
//!   and the same tasks in them, in order, by name. A git graph's has the
//!   same branches, in order, and the same commits, in order, each on the
//!   same branch with the same id and tag, a merge where one was written and
//!   from the same branch, and nothing cherry-picked.
//! - A line the engine draws with a control character in it refuses the
//!   diagram; under the ASCII glyph set, so does a character that is neither
//!   ASCII nor written in the source.
//! - The engine lays a diagram out once, for its room, at its own spacing:
//!   never again at a tighter one, since each layout costs as much as the
//!   first. Every line it draws is measured, and a drawing wider than the
//!   room is refused, never cut. A git graph is drawn without its room, since
//!   the engine would cut a commit's label to fit it, and so is a gantt,
//!   since the engine draws a task's bar a day short and a one-day task with
//!   no bar at all whenever its room holds more cells than the gantt has
//!   days. Without its room the engine draws a gantt a day to a cell, up to
//!   [`DAYS_A_CELL_EACH`] days; past them it draws more days to a cell, can
//!   draw a one-day task with no bar, and a gantt with one is refused.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crucible_tui::drawing::Catching;
use crucible_tui::{Glyphs, columns};
use mermaid_text::{
    CommitKind, ErDiagram, Error, GanttDiagram, GitGraph, Graph, RenderOptions, SequenceDiagram,
    parser, render_with_options,
};

use crate::scan::{Read, Scanned, Step};
use crate::{Caps, Kind};

/// The most days of a gantt the engine draws a day to a cell, without its
/// room; past them it draws more than one to a cell.
const DAYS_A_CELL_EACH: i64 = 60;

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
    // unwinds, so it is caught here, with the checks around the engine call.
    // The guard, held over all the catch covers, tells the session's panic
    // hook the panic is caught, so it is said in the transcript rather than
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
        Kind::Sequence => parser::sequence::parse(source)
            .is_ok_and(|diagram| sequence_alike(&diagram, &scanned.read)),
        Kind::Gantt => {
            parser::gantt::parse(source).is_ok_and(|diagram| gantt_alike(&diagram, &scanned.read))
        }
        Kind::GitGraph => parser::git_graph::parse(source)
            .is_ok_and(|graph| git_graph_alike(&graph, &scanned.read)),
    }
}

/// Whether `diagram` holds the participants `read` found, in order and
/// labelled alike, and the messages, in order, between the same participants
/// and labelled alike, and nothing the scan refuses.
fn sequence_alike(diagram: &SequenceDiagram, read: &Read) -> bool {
    let participants = read.steps.iter().filter_map(|step| match step {
        Step::Participant { name, label } => Some((name.as_str(), label.as_str())),
        _ => None,
    });
    let messages = read.steps.iter().filter_map(|step| match step {
        Step::Message { from, to, label } => Some((from.as_str(), to.as_str(), label.as_str())),
        _ => None,
    });
    diagram.notes.is_empty()
        && diagram.activations.is_empty()
        && diagram.blocks.is_empty()
        && diagram.autonumber_changes.is_empty()
        && diagram.participant_groups.is_empty()
        && diagram
            .participants
            .iter()
            .map(|participant| (participant.id.as_str(), participant.label.as_str()))
            .eq(participants)
        && diagram
            .messages
            .iter()
            .map(|message| {
                (
                    message.from.as_str(),
                    message.to.as_str(),
                    message.text.as_str(),
                )
            })
            .eq(messages)
}

/// Whether `diagram` holds the sections and tasks `read` found, in order and
/// by name, and the engine draws a bar for every task.
fn gantt_alike(diagram: &GanttDiagram, read: &Read) -> bool {
    let drawn = diagram.sections.iter().flat_map(|section| {
        let named = section.name.clone().map(Step::Section);
        named.into_iter().chain(
            section
                .tasks
                .iter()
                .map(|task| Step::Task(task.name.clone())),
        )
    });
    let every_task_drawn = diagram.span_days() <= DAYS_A_CELL_EACH
        || diagram
            .sections
            .iter()
            .flat_map(|section| &section.tasks)
            .all(|task| task.duration_days() > 1);
    every_task_drawn && drawn.eq(read.steps.iter().cloned())
}

/// Whether `graph` holds the branches `read` found, in order, and the
/// commits, in order, each on the same branch, with the same id and tag, and
/// merging the same branch.
fn git_graph_alike(graph: &GitGraph, read: &Read) -> bool {
    let branches = graph.branches.iter().map(|branch| branch.name.as_str());
    let made = std::iter::once("main").chain(read.steps.iter().filter_map(|step| match step {
        Step::Branch(name) => Some(name.as_str()),
        _ => None,
    }));
    let written = read.steps.iter().filter_map(|step| match step {
        Step::Commit {
            branch,
            id,
            tag,
            merged,
        } => Some((branch, id, tag, merged)),
        _ => None,
    });
    branches.eq(made)
        && graph.commits.len() == written.clone().count()
        && graph.commits.iter().zip(written).enumerate().all(
            |(at, (commit, (branch, id, tag, merged)))| {
                let named = id.clone().unwrap_or_else(|| format!("c{at}"));
                let from = commit
                    .merge_parent
                    .and_then(|parent| graph.commits.get(parent))
                    .map(|parent| &parent.branch);
                commit.branch == *branch
                    && commit.id == named
                    && commit.tag == *tag
                    && from == merged.as_ref()
                    && match commit.kind {
                        CommitKind::Normal => merged.is_none(),
                        CommitKind::Merge => merged.is_some(),
                        CommitKind::CherryPick => false,
                    }
            },
        )
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
/// in one pass. The engine lays a sequence diagram out at its own width
/// whatever the room, and a git graph and a gantt are handed none.
fn rendered(scanned: &Scanned, room: usize, glyphs: Glyphs) -> Result<Vec<String>, Unlaid> {
    let ascii = glyphs == Glyphs::Ascii;
    let options = RenderOptions {
        max_width: (!matches!(scanned.kind, Kind::GitGraph | Kind::Gantt)).then_some(room),
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
