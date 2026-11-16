//! Diagrams in an answer: a `mermaid` fence named by its drawer while it
//! streams, and laid out again in new glyphs and in each way a setting draws
//! it, apart from how math is drawn.

use std::sync::Arc;

use super::*;
use crate::chrome::Undrawn;
use crate::drawing::{Drawers, Draws, Laid, Mode, Room, Run};

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

#[test]
fn a_diagram_drawn_is_laid_out_again_in_each_mode_chosen() {
    let mut drawn = boxing();
    // As a setting left alone has it.
    assert_eq!(drawn.render.diagrams_mode(), Mode::Image);
    drawn.answer(ANSWER);
    assert_eq!(drawn.rows(), ["▣ graph TD", "» y"]);

    drawn.render.diagrams(Mode::Written);
    drawn.render.draw().unwrap();
    assert_eq!(drawn.render.diagrams_mode(), Mode::Written);
    // The fence is shown as written, in the frame any fence is; the math is
    // drawn as it was.
    let rows = drawn.rows();
    assert!(!rows.iter().any(|row| row.contains('▣')), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("mermaid")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("graph TD")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("A --> B")), "{rows:#?}");
    assert_eq!(rows.last().map(String::as_str), Some("» y"), "{rows:#?}");

    // An image cannot be shown yet, so it is text, as text is.
    for mode in [Mode::Image, Mode::Text] {
        drawn.render.diagrams(mode);
        drawn.render.draw().unwrap();
        assert_eq!(drawn.rows(), ["▣ graph TD", "» y"], "{mode:?}");
    }
}

#[test]
fn how_math_is_drawn_and_how_diagrams_are_drawn_are_set_apart() {
    let mut drawn = boxing();
    drawn.answer(ANSWER);

    // Math written leaves the diagram drawn.
    drawn.render.math(Mode::Written);
    drawn.render.draw().unwrap();
    assert_eq!(drawn.render.diagrams_mode(), Mode::Image);
    assert_eq!(drawn.rows(), ["▣ graph TD", "$$", "y", "$$"]);

    // Diagrams written leave math as it was set.
    drawn.render.math(Mode::Text);
    drawn.render.diagrams(Mode::Written);
    drawn.render.draw().unwrap();
    assert_eq!(drawn.render.math_mode(), Mode::Text);
    let rows = drawn.rows();
    assert!(!rows.iter().any(|row| row.contains('▣')), "{rows:#?}");
    assert_eq!(rows.last().map(String::as_str), Some("» y"), "{rows:#?}");
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
    // A kind the drawer does not name, a first line still arriving, a fence
    // whose drawer names nothing, and a diagram shown as written: each row
    // names the fence, as it did before any drawer named its block.
    for (streamed, written, row) in [
        ("```mermaid\npie\n", false, "mermaid diagram · 2 lines"),
        ("```mermaid\ngra", false, "mermaid diagram · 2 lines"),
        ("```math\nx^2\n", false, "formula · 2 lines"),
        ("```mermaid\ngraph TD\n", true, "mermaid diagram · 2 lines"),
    ] {
        let mut drawn = boxing();
        if written {
            drawn.render.diagrams(Mode::Written);
        }
        drawn.render.stream(streamed).unwrap();
        let row = row.replace('·', dot);
        assert_eq!(
            foot(&drawn),
            Some(format!("{summed} drawing a {row} so far")),
            "{streamed:?}"
        );
    }
}
