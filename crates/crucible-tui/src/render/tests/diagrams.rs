//! Diagrams in an answer: a `mermaid` fence named by its drawer while it
//! streams, and laid out again in new glyphs.

use std::sync::Arc;

use super::*;
use crate::chrome::Undrawn;
use crate::drawing::{Drawers, Draws, Laid, Room, Run};

/// A diagram drawer that draws a fence's first line behind a mark of the
/// glyph set it is asked in, and math behind another, so one test can tell
/// which of the two a setting reached.
struct Boxed;

impl Boxed {
    fn mark(glyphs: Glyphs) -> &'static str {
        match glyphs {
            Glyphs::Unicode => "▣",
            Glyphs::Ascii => "#",
        }
    }
}

impl Draws for Boxed {
    fn names(&self) -> &'static [&'static str] {
        &["mermaid"]
    }

    fn label(&self, _: &str, first_line: &str) -> Option<&'static str> {
        first_line.starts_with("graph").then_some("a boxed graph")
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        let first = source.lines().next().unwrap_or_default();
        let row = Row::new()
            .then(Slot::Plain, format!("{} {first}", Self::mark(room.glyphs)))
            .clipped(room.columns);
        Ok(Laid {
            natural: (row.columns(), 1),
            rows: vec![row],
            narrow: None,
        })
    }
}

/// A math drawer that draws a formula behind `»`.
struct Marked;

impl Draws for Marked {
    fn names(&self) -> &'static [&'static str] {
        &["math"]
    }

    fn inline(&self, span: &str, _: &Room) -> Option<Vec<Run>> {
        Some(vec![Run {
            slot: Slot::Emphasis,
            text: format!("»{span}"),
        }])
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        let row = Row::new()
            .then(Slot::Plain, format!("» {}", source.trim()))
            .clipped(room.columns);
        Ok(Laid {
            natural: (row.columns(), 1),
            rows: vec![row],
            narrow: None,
        })
    }
}

/// A reader of 60 columns whose answers' diagrams [`Boxed`] draws and whose
/// math [`Marked`] does.
fn boxing() -> Drawn {
    let mut drawn = reading(60, 12);
    drawn.render.drawers(
        Drawers::default()
            .with(Arc::new(Boxed))
            .with(Arc::new(Marked)),
    );
    drawn
}

const ANSWER: &str = "```mermaid\ngraph TD\nA --> B\n```\n\n$$\ny\n$$\n";

#[test]
fn a_diagram_drawn_is_laid_out_again_when_the_glyphs_change() {
    let mut drawn = boxing();
    drawn.answer(ANSWER);
    assert_eq!(drawn.rows(), ["▣ graph TD", "» y"]);

    drawn.render.draws(Glyphs::Ascii);
    drawn.render.draw().unwrap();
    assert_eq!(drawn.rows(), ["# graph TD", "» y"]);
}

/// The live foot's row for a block held, drawn 60 columns wide.
fn foot(drawn: &Drawn) -> Option<String> {
    drawn.render.holding(60).map(|row| row.text())
}

#[test]
fn a_diagram_being_streamed_is_named_by_its_drawer_in_the_live_foot() {
    let (summed, dot) = (Glyphs::Unicode.summed(), Glyphs::Unicode.dot());
    let mut drawn = boxing();
    drawn
        .render
        .stream("```mermaid\ngraph TD\nA --> B\n")
        .unwrap();
    assert_eq!(
        foot(&drawn),
        Some(format!(
            "{summed} drawing a boxed graph {dot} 3 lines so far"
        ))
    );
}

#[test]
fn a_fence_whose_drawer_gives_no_label_keeps_the_row_it_had() {
    let (summed, dot) = (Glyphs::Unicode.summed(), Glyphs::Unicode.dot());
    // A kind the drawer does not name, a first line still arriving and a
    // fence whose drawer names nothing: each row names the fence, as it did
    // before any drawer named its block.
    for (streamed, row) in [
        ("```mermaid\npie\n", "mermaid diagram · 2 lines"),
        ("```mermaid\ngra", "mermaid diagram · 2 lines"),
        ("```math\nx^2\n", "formula · 2 lines"),
    ] {
        let mut drawn = boxing();
        drawn.render.stream(streamed).unwrap();
        let row = row.replace('·', dot);
        assert_eq!(
            foot(&drawn),
            Some(format!("{summed} drawing a {row} so far")),
            "{streamed:?}"
        );
    }
}
