//! A diagram's source read once, front to back, before the engine is handed
//! any of it: cleaned, its kind found, each statement read and counted, and
//! what the engine is to draw written out again a statement to a line.
//!
//! Invariants:
//!
//! - The source is measured before it is read: past [`MOST_BYTES`] nothing
//!   else is done with it, and past its kind's byte cap nothing but its
//!   kind is read.
//! - The count stops the read: a diagram is refused the moment what it names
//!   passes its kind's [`Caps`] in boxes or lines, or a label it draws is
//!   wider than its kind's label cap, so nothing the engine is handed lays
//!   out more than that.
//! - A statement this does not read refuses the diagram. One that styles,
//!   links, acts or comments (`style`, `classDef`, `click`, `%%`, and their
//!   like) is left out of what the engine is handed, so no address or action
//!   in it reaches a row.
//! - What was read is kept beside what the engine is handed, so the engine's
//!   own reading of it can be held to this one.

use std::collections::BTreeMap;

use crucible_tui::columns;

use crate::{Caps, Kind, MOST_BYTES, entities, flowchart, state};

/// Why a source is not handed to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// There is nothing in it but blank lines and comments.
    Empty,
    /// It is longer than its kind's caps hold, or names more than they do.
    PastCap,
    /// A statement in it is not one this reads.
    Unread,
    /// It is a flowchart with a group in it, which is not drawn as text.
    Grouped,
    /// It is a kind of diagram this does not draw as text, named.
    Unsupported(&'static str),
}

/// A source read whole: its kind, the statements the engine is to draw, and
/// what was read in them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scanned {
    pub(crate) kind: Kind,
    /// The statements kept, one to a line, the header first.
    pub(crate) source: String,
    pub(crate) read: Read,
}

/// What the read found, for the engine's reading to be held to, and the caps
/// it is read against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Read {
    caps: Caps,
    /// Each box by its name, with the label it was written with where it was
    /// given one.
    pub(crate) boxes: BTreeMap<String, Option<String>>,
    /// Each line between boxes, with its label where it has one.
    pub(crate) lines: Vec<Option<String>>,
    /// The groups drawn around boxes: a state diagram's composite states,
    /// and its notes.
    pub(crate) groups: usize,
}

impl Read {
    /// Nothing read yet, against `caps`.
    pub(crate) const fn new(caps: Caps) -> Self {
        Self {
            caps,
            boxes: BTreeMap::new(),
            lines: Vec::new(),
            groups: 0,
        }
    }

    /// Takes `name` as a box, with `label` where the source gives it one and
    /// none was given before. The label is measured, or the name where the
    /// box is given none here, since the engine draws the name then.
    pub(crate) fn named(&mut self, name: &str, label: Option<String>) -> Result<(), Refused> {
        self.labelled(label.as_deref().unwrap_or(name))?;
        self.marked(name, label)
    }

    /// Takes `name` as a box the engine draws as a mark rather than a label:
    /// a state diagram's start and end, whose name only this reading uses.
    pub(crate) fn marked(&mut self, name: &str, label: Option<String>) -> Result<(), Refused> {
        let held = self.boxes.entry(name.to_owned()).or_default();
        if held.is_none() {
            *held = label;
        }
        self.within()
    }

    /// Takes one line between boxes.
    pub(crate) fn joined(&mut self, label: Option<String>) -> Result<(), Refused> {
        if let Some(label) = &label {
            self.labelled(label)?;
        }
        self.lines.push(label);
        self.within()
    }

    /// Whether `label`, as it is drawn on one line, is within the label cap.
    pub(crate) fn labelled(&self, label: &str) -> Result<(), Refused> {
        match self.caps.label {
            Some(most) if columns(label) > most => Err(Refused::PastCap),
            _ => Ok(()),
        }
    }

    /// Takes one group.
    pub(crate) fn grouped(&mut self) -> Result<(), Refused> {
        self.groups = self.groups.saturating_add(1);
        self.within()
    }

    /// Whether what was read so far is inside the caps.
    fn within(&self) -> Result<(), Refused> {
        if self.boxes.len().saturating_add(self.groups) > self.caps.nodes
            || self.lines.len() > self.caps.edges
        {
            return Err(Refused::PastCap);
        }
        Ok(())
    }
}

/// The kinds of Mermaid diagram this crate knows and does not draw as text,
/// by the word that opens one, lowercased, and the name a reader is told.
const NOT_DRAWN: &[(&str, &str)] = &[
    ("sequencediagram", "sequence diagram"),
    ("pie", "pie"),
    ("gantt", "gantt"),
    ("gitgraph", "git graph"),
    ("journey", "journey"),
    ("mindmap", "mindmap"),
    ("timeline", "timeline"),
    ("quadrantchart", "quadrant chart"),
    ("requirementdiagram", "requirement diagram"),
    ("c4context", "c4 diagram"),
    ("c4container", "c4 diagram"),
    ("c4component", "c4 diagram"),
    ("c4dynamic", "c4 diagram"),
    ("c4deployment", "c4 diagram"),
    ("sankey", "sankey"),
    ("sankey-beta", "sankey"),
    ("xychart", "xy chart"),
    ("xychart-beta", "xy chart"),
    ("block", "block diagram"),
    ("block-beta", "block diagram"),
    ("packet", "packet"),
    ("packet-beta", "packet"),
    ("architecture", "architecture"),
    ("architecture-beta", "architecture"),
    ("kanban", "kanban"),
    ("radar", "radar"),
    ("radar-beta", "radar"),
    ("treemap", "treemap"),
    ("treemap-beta", "treemap"),
    ("zenuml", "zenuml"),
];

/// `source` read whole, or why it is not handed to the engine.
pub(crate) fn scan(source: &str) -> Result<Scanned, Refused> {
    if source.len() > MOST_BYTES {
        return Err(Refused::PastCap);
    }
    let cleaned = cleaned(source);
    let kind = kind_of(&cleaned)?;
    let caps = kind.caps().ok_or(Refused::Unsupported(kind.name()))?;
    if source.len() > caps.bytes {
        return Err(Refused::PastCap);
    }
    let read = Read::new(caps);
    let (source, read) = match kind {
        Kind::Flowchart => flowchart::read(&cleaned, read)?,
        Kind::State => state::read(&cleaned, read)?,
        // A kind with no caps is refused above.
        Kind::Class => return Err(Refused::Unsupported(kind.name())),
        Kind::EntityRelationship => entities::read(&cleaned, read)?,
    };
    Ok(Scanned { kind, source, read })
}

/// The kind of diagram `source` opens with: the first word of its first line
/// that is neither blank nor a comment, as Mermaid reads it.
pub(crate) fn kind_of(source: &str) -> Result<Kind, Refused> {
    let first = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .ok_or(Refused::Empty)?;
    let word = first
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    if let Some(kind) = Kind::opened_by(&word) {
        return Ok(kind);
    }
    NOT_DRAWN
        .iter()
        .find(|(opens, _)| *opens == word)
        .map_or(Err(Refused::Unread), |(_, name)| {
            Err(Refused::Unsupported(name))
        })
}

/// `source` with what a terminal would act on, what it draws as nothing, and
/// the characters a diagram is drawn in taken out, so a label can neither
/// move the cursor nor draw a stroke of a box: control characters but the
/// line's end (a tab is a space), format characters, the line and paragraph
/// separators, and the box-drawing, block and geometric-shape characters.
pub(crate) fn cleaned(source: &str) -> String {
    source
        .chars()
        .filter_map(|character| match character {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            _ if character.is_control() || unshown(character) || drawn_with(character) => None,
            _ => Some(character),
        })
        .collect()
}

/// The format characters, which a terminal draws as nothing or which turn
/// the characters around them about, and the line and paragraph separators,
/// which some draw as a line's end.
const fn unshown(character: char) -> bool {
    matches!(
        character,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// The characters the engine draws boxes, lines, arrowheads and markers in:
/// the box-drawing, block-element and geometric-shape blocks.
const fn drawn_with(character: char) -> bool {
    matches!(character, '\u{2500}'..='\u{25ff}')
}

/// `line` up to a `%%` that stands outside double quotes, as Mermaid ends a
/// line at a comment.
pub(crate) fn uncommented(line: &str) -> &str {
    let mut quoted = false;
    for (at, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '%' if !quoted && line.get(at + 1..).is_some_and(|rest| rest.starts_with('%')) => {
                return line.get(..at).unwrap_or(line);
            }
            _ => {}
        }
    }
    line
}

/// Whether `statement` opens with `keyword` as a word of its own: followed by
/// nothing, a space, or one of `also`.
pub(crate) fn opens_with(statement: &str, keyword: &str, also: &[char]) -> bool {
    statement.strip_prefix(keyword).is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|next| next.is_whitespace() || also.contains(&next))
    })
}

/// A name as a box is named by: letters, digits and underscores, and a hyphen
/// between them.
pub(crate) fn is_name(text: &str) -> bool {
    let mut previous = None;
    let named = !text.is_empty()
        && text.chars().all(|character| {
            let fits = character.is_alphanumeric()
                || character == '_'
                || (character == '-' && previous.is_some());
            previous = Some(character);
            fits
        });
    named && !text.ends_with('-')
}
