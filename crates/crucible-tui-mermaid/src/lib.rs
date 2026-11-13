//! Mermaid diagrams in an answer drawn as text: flowcharts, state and entity-relationship diagrams.
//!
//! [`Diagrams`] reads the source in a `mermaid` fence and hands what it read
//! to the `mermaid-text` engine, which lays the boxes and the lines between
//! them out in a terminal's characters. The other kinds of Mermaid diagram
//! (class, sequence, gantt, pie and the rest) are named and shown as written.
//!
//! What this crate holds to:
//!
//! - **The diagram or nothing.** A diagram with a statement this does not
//!   read, or one the engine reads otherwise than Mermaid does, is refused
//!   whole and shown as written: a drawing that leaves out a box or a line,
//!   or draws one that is not there, says what the source did not. The
//!   engine's reading is held to this one as far as the engine's names
//!   allow: a flowchart's boxes, lines and labels; a state diagram's
//!   transitions and their labels and the labels it gave states, with no
//!   more states than its cap; an entity-relationship diagram's entities and
//!   how many relationships.
//! - **Bounded before the engine.** A source past [`MOST_BYTES`] is refused
//!   before it is read, and one past its kind's [`Caps`] (bytes, boxes, lines
//!   or the columns of one label) is refused as the read reaches the one past
//!   the cap, so the engine never lays out more than that. The caps are set so
//!   that the costliest diagram inside them is laid out in well under the 2 ms
//!   a frame's drawing may take: a release build, timing seeded diagrams at
//!   every box count up to the cap with every line the cap allows, ASCII,
//!   CJK and mixed labels grown to the label cap or the byte cap, in rooms of
//!   82 and 402 columns and both glyph sets, then timing the costliest of
//!   them 1000 times, found at most 0.94 ms (median) and 1.09 ms (99th
//!   percentile) for a flowchart, 1.17 and 1.34 ms for a state diagram and
//!   0.67 and 0.79 ms for an entity-relationship diagram. The cost follows the
//!   lines and the width of the labels, not the boxes: a few boxes with every
//!   line between them cost the most, and a flowchart's eight lines with
//!   labels of about 16 columns took 2.06 ms. So a flowchart trades lines
//!   for labels and keeps 16 columns, the width of a two-word label, at six
//!   lines, where labels of about eight columns would have allowed ten; a
//!   state diagram, whose states and events are mostly one word, keeps eight
//!   transitions at 12 columns; an entity-relationship diagram, whose layout
//!   costs far less, caps no label and holds 48 entities and 64
//!   relationships in 4096 bytes. The drawing probe builds each kind's
//!   costliest shape from these caps, four boxes with every line the caps
//!   allow going out from one and back, so a loosened cap is timed at what it
//!   admits; on a quiet machine its 95th percentile was 1.15 ms for a
//!   flowchart, 1.05 ms for a state diagram and 0.20 ms for an
//!   entity-relationship diagram.
//! - **Hostile text taken out, not drawn.** Control, format and invisible
//!   characters, and the characters boxes are drawn with, are taken out of a
//!   label before the engine sees it, so a label neither acts on the terminal
//!   nor draws a stroke that is not the diagram's. A statement that styles,
//!   links or acts (`style`, `classDef`, `click`) is left out: no address in
//!   one reaches a row, and nothing it names is opened.
//! - **ASCII where it is asked for.** Under the ASCII glyph set the boxes,
//!   lines and arrowheads are ASCII; a character a label was written with
//!   stays, and any other that is not ASCII refuses the diagram.
//! - **Drawn alike every time.** A flowchart with a group (`subgraph`) in it
//!   is shown as written before the engine sees it, saying groups are not
//!   drawn as text, and so is a class diagram, named: the engine places a
//!   group's boxes, and a class diagram's classes and the labels between
//!   them, differently from one drawing to the next, and draws a class
//!   diagram's lines through its names.
//! - **Every row fits.** The engine lays a diagram out once, for its room,
//!   and every row of what it draws is measured; a drawing wider than its
//!   room is refused for it, never cut and never laid out again, so a
//!   narrower window shows it as written.

mod entities;
mod flowchart;
mod lay;
mod scan;
mod state;

#[cfg(test)]
mod fits;
#[cfg(test)]
mod tests;

use crucible_tui::chrome::{Frame, Label, Undrawn};
use crucible_tui::drawing::{Draws, Laid, Room};
use crucible_tui::{Row, Slot, columns};

use lay::Unlaid;
use scan::Refused;

/// The columns a frame's gutter takes before each row of a diagram.
const GUTTER: usize = 2;

/// How much of one kind of diagram is handed to the engine. A diagram past
/// any of them is shown as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// The most bytes of source.
    pub bytes: usize,
    /// The most boxes, counting a state diagram's composite states and notes
    /// and its start and end markers.
    pub nodes: usize,
    /// The most lines between boxes.
    pub edges: usize,
    /// The most columns one label takes, where labels are capped: a box's
    /// label, or its name where it is drawn with none; a line's label; each
    /// line of a note.
    pub label: Option<usize>,
}

impl Caps {
    /// A flowchart's caps.
    pub const FLOWCHART: Self = Self {
        bytes: 1024,
        nodes: 16,
        edges: 6,
        label: Some(16),
    };

    /// A state diagram's caps.
    pub const STATE: Self = Self {
        bytes: 1024,
        nodes: 16,
        edges: 8,
        label: Some(12),
    };

    /// An entity-relationship diagram's caps.
    pub const ENTITIES: Self = Self {
        bytes: 4096,
        nodes: 48,
        edges: 64,
        label: None,
    };
}

/// The most bytes of source read before its kind is known: the largest of
/// the kinds' byte caps.
pub const MOST_BYTES: usize = most_bytes([Caps::FLOWCHART, Caps::STATE, Caps::ENTITIES]);

const fn most_bytes(all: [Caps; 3]) -> usize {
    let [one, two, three] = all;
    let mut most = one.bytes;
    if two.bytes > most {
        most = two.bytes;
    }
    if three.bytes > most {
        most = three.bytes;
    }
    most
}

/// A kind of Mermaid diagram drawn as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `graph` or `flowchart`: boxes and the lines between them.
    Flowchart,
    /// `stateDiagram`: states and the transitions between them.
    State,
    /// `classDiagram`: classes, their members, and how they relate.
    Class,
    /// `erDiagram`: entities, their attributes, and their relationships.
    EntityRelationship,
}

impl Kind {
    /// What a reader is told this kind is called.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Flowchart => "flowchart",
            Self::State => "state diagram",
            Self::Class => "class diagram",
            Self::EntityRelationship => "er diagram",
        }
    }

    /// What a diagram of this kind may hold and still be drawn; `None` for a
    /// kind that is shown as written.
    #[must_use]
    pub const fn caps(self) -> Option<Caps> {
        match self {
            Self::Flowchart => Some(Caps::FLOWCHART),
            Self::State => Some(Caps::STATE),
            Self::Class => None,
            Self::EntityRelationship => Some(Caps::ENTITIES),
        }
    }

    /// What the status row says is being drawn while a diagram of this kind
    /// streams in.
    const fn told(self) -> &'static str {
        match self {
            Self::Flowchart => "a mermaid flowchart",
            Self::State => "a mermaid state diagram",
            Self::Class => "a mermaid class diagram",
            Self::EntityRelationship => "a mermaid er diagram",
        }
    }

    /// The kind a diagram opened by `word`, lowercased, is.
    fn opened_by(word: &str) -> Option<Self> {
        match word {
            "graph" | "flowchart" => Some(Self::Flowchart),
            "statediagram" | "statediagram-v2" => Some(Self::State),
            "classdiagram" => Some(Self::Class),
            "erdiagram" => Some(Self::EntityRelationship),
            _ => None,
        }
    }
}

/// Mermaid diagrams drawn as text: the drawer for a `mermaid` fence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Diagrams;

impl Draws for Diagrams {
    fn names(&self) -> &'static [&'static str] {
        &["mermaid"]
    }

    fn label(&self, _: &str, first_line: &str) -> Option<&'static str> {
        let word = first_line.split_whitespace().next()?.to_lowercase();
        Kind::opened_by(&word).map(Kind::told)
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        let scanned = scan::scan(source).map_err(|refused| match refused {
            Refused::Empty => Undrawn::NotHere,
            Refused::PastCap => Undrawn::PastCap,
            Refused::Unread => Undrawn::NotText,
            Refused::Unsupported(kind) => Undrawn::Unsupported(kind),
            Refused::Grouped => Undrawn::Unsupported("groups"),
        })?;
        let inside = room.columns.saturating_sub(GUTTER);
        let drawing = lay::lay(&scanned, inside, room.glyphs).map_err(|unlaid| match unlaid {
            Unlaid::Unread => Undrawn::NotText,
            Unlaid::TooWide => Undrawn::TooWide,
        })?;
        let inner: Vec<Row> = drawing
            .lines
            .into_iter()
            .map(|line| Row::new().then(Slot::Plain, line))
            .collect();
        let name = format!("mermaid {} {}", room.glyphs.dot(), scanned.kind.name());
        let frame = Frame {
            label: Label::new(&name, room.glyphs),
            refusal: None,
        };
        let rows = frame.rows(room.columns, &inner);
        let natural = (
            drawing
                .natural
                .saturating_add(GUTTER)
                .max(columns(&frame.top())),
            rows.len(),
        );
        Ok(Laid {
            rows,
            natural,
            narrow: None,
        })
    }
}
