//! Mermaid diagrams in an answer drawn as text: flowcharts, state, entity-relationship and sequence diagrams, gantts and git graphs.
//!
//! [`Diagrams`] reads the source in a `mermaid` fence and hands what it read
//! to the `mermaid-text` engine, which lays the boxes and the lines between
//! them out in a terminal's characters: a sequence diagram's participants
//! over their lifelines, a gantt's tasks as bars along its days, a git
//! graph's commits down its branches. The other kinds of Mermaid diagram
//! (class, pie and the rest) are named and shown as written.
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
//!   how many relationships; a sequence diagram's participants and their
//!   labels and its messages, their ends and labels, in order; a gantt's
//!   sections and its tasks' names, in order, and that a bar is drawn for
//!   every task; a git graph's branches and its commits, each with the branch
//!   it is made on, its id, its tag and the branch it merges, in order.
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
//!   entity-relationship diagram. A sequence diagram, a gantt and a git graph
//!   cost far less: a release build timing each 31 times in a room of 80
//!   columns found a sequence diagram of 16 participants and 64 messages
//!   with labels of 40 columns, in 3830 bytes, at 0.56 ms (95th percentile),
//!   and a gantt of 96 tasks and a git graph of 12 branches, each in more
//!   bytes than its cap, under 0.2 ms. So their caps are set by what a
//!   terminal shows rather than by time: a sequence diagram holds 8
//!   participants and 32 messages with labels of 24 columns, a gantt 32
//!   sections and tasks with names of 15 columns, the widest the engine
//!   draws whole, and a git graph 32 commits and 8 branches beside `main`,
//!   each in 2048 bytes. The drawing probe builds each of these from its caps
//!   too, with every box and line they hold and labels as wide as they allow;
//!   on the same machine its 95th percentile was 0.79 ms for a sequence
//!   diagram, 0.16 ms for a gantt and 0.16 ms for a git graph.
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
//!   narrower window shows it as written. A sequence diagram is laid out at
//!   its own width whatever its room, and a gantt and a git graph are laid
//!   out with none, a day of a gantt to a cell and a git graph's labels
//!   whole, so each is drawn as it is or refused as too wide, never squeezed.
//! - **A picture is the same layout.** [`Diagrams`] also draws a diagram as
//!   an SVG picture, from the one layout its text drawing is drawn from and
//!   refused for the same reasons: the characters lines, blocks and
//!   arrowheads are drawn with become strokes and shapes, every other one is
//!   written as text in the cell the text drawing puts it in, and every ink
//!   is a slot's class, never a colour. Nothing here rasterises it.

mod entities;
mod flowchart;
mod gantt;
mod git_graph;
mod lay;
mod picture;
mod scan;
mod sequence;
mod state;

#[cfg(test)]
mod fits;
#[cfg(test)]
mod tests;

use std::ops::RangeInclusive;

use crucible_tui::chrome::{Frame, Label, Undrawn};
use crucible_tui::drawing::{Draws, Laid, Room, Svg};
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
    /// and its start and end markers; a sequence diagram's participants; a
    /// gantt's sections and tasks; a git graph's commits.
    pub nodes: usize,
    /// The most lines between boxes: a sequence diagram's messages; a git
    /// graph's branches beside `main`.
    pub edges: usize,
    /// The most columns one label takes, where labels are capped: a box's
    /// label, or its name where it is drawn with none; a line's label; each
    /// line of a note; a gantt task's name.
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

    /// A sequence diagram's caps.
    pub const SEQUENCE: Self = Self {
        bytes: 2048,
        nodes: 8,
        edges: 32,
        label: Some(24),
    };

    /// A gantt's caps. Its label cap is the widest task name the engine
    /// draws whole: one wider is cut short with an ellipsis.
    pub const GANTT: Self = Self {
        bytes: 2048,
        nodes: 32,
        edges: 0,
        label: Some(15),
    };

    /// A git graph's caps.
    pub const GIT_GRAPH: Self = Self {
        bytes: 2048,
        nodes: 32,
        edges: 8,
        label: None,
    };
}

/// The most bytes of source read before its kind is known: the largest of
/// the kinds' byte caps.
pub const MOST_BYTES: usize = most_bytes(&[
    Caps::FLOWCHART,
    Caps::STATE,
    Caps::ENTITIES,
    Caps::SEQUENCE,
    Caps::GANTT,
    Caps::GIT_GRAPH,
]);

const fn most_bytes(all: &[Caps]) -> usize {
    let mut most = 0;
    let mut rest = all;
    while let [caps, after @ ..] = rest {
        if caps.bytes > most {
            most = caps.bytes;
        }
        rest = after;
    }
    most
}

/// The letters a picture of a diagram writes; a diagram whose text holds any
/// other is not drawn as a picture.
///
/// A picture is written in the one font whatever rasterises it carries, and
/// a letter that font has no glyph for would be drawn as an empty box. The
/// set is kept small enough to be sure of: printable ASCII, the printable
/// Latin-1 letters and signs but the soft hyphen, Latin Extended-A, the
/// dashes, curly quotes, bullet and ellipsis, the four arrows and the two
/// comparisons a label or a gantt's dates are written with. The command line,
/// which composes the drawers with the raster, holds its font to having a
/// glyph for every one of them.
pub const PICTURE_LETTERS: &[RangeInclusive<char>] = &[
    ' '..='~',
    '\u{a0}'..='\u{ac}',
    '\u{ae}'..='\u{17f}',
    '–'..='—',
    '‘'..='’',
    '“'..='”',
    '•'..='•',
    '…'..='…',
    '←'..='↓',
    '≤'..='≥',
];

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
    /// `sequenceDiagram`: participants and the messages between them.
    Sequence,
    /// `gantt`: tasks in sections, as bars along the days they take.
    Gantt,
    /// `gitGraph`: commits on branches, and the merges between them.
    GitGraph,
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
            Self::Sequence => "sequence diagram",
            Self::Gantt => "gantt",
            Self::GitGraph => "git graph",
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
            Self::Sequence => Some(Caps::SEQUENCE),
            Self::Gantt => Some(Caps::GANTT),
            Self::GitGraph => Some(Caps::GIT_GRAPH),
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
            Self::Sequence => "a mermaid sequence diagram",
            Self::Gantt => "a mermaid gantt",
            Self::GitGraph => "a mermaid git graph",
        }
    }

    /// The kind a diagram opened by `word`, lowercased, is.
    fn opened_by(word: &str) -> Option<Self> {
        match word {
            "graph" | "flowchart" => Some(Self::Flowchart),
            "statediagram" | "statediagram-v2" => Some(Self::State),
            "classdiagram" => Some(Self::Class),
            "erdiagram" => Some(Self::EntityRelationship),
            "sequencediagram" => Some(Self::Sequence),
            "gantt" => Some(Self::Gantt),
            "gitgraph" => Some(Self::GitGraph),
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
        let (kind, drawing) = laid(source, room)?;
        let inner: Vec<Row> = drawing
            .lines
            .into_iter()
            .map(|line| Row::new().then(Slot::Plain, line))
            .collect();
        let name = format!("mermaid {} {}", room.glyphs.dot(), kind.name());
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

    fn picture(&self, source: &str, room: &Room) -> Result<Svg, Undrawn> {
        let (_, drawing) = laid(source, room)?;
        picture::picture(&drawing.lines, drawing.natural)
    }
}

/// `source` read and laid out by the engine for `room`, inside the frame's
/// gutter: the one layout the text drawing and the picture are both drawn
/// from, refused alike.
fn laid(source: &str, room: &Room) -> Result<(Kind, lay::Drawing), Undrawn> {
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
    Ok((scanned.kind, drawing))
}
