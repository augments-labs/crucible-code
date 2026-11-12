//! Math in an answer: what reaches the screen of a dollar that is no math, a
//! formula nothing can draw, the markers inside a formula, a span told of at
//! the foot while it streams, and a formula laid out again in new glyphs and
//! in each way a setting draws it.

use std::sync::Arc;

use super::*;
use crate::chrome::Undrawn;
use crate::drawing::{Drawers, Draws, Laid, Mode, Room, Run};

#[test]
fn dollar_signs_that_are_not_math_stay_as_written() {
    for prose in [
        "It costs $5 and $10.",
        "$5/$10 per seat",
        "$20,000-$30,000 a year",
        "Set $HOME before running.",
        "Run $browser then $deploy on it.",
        "echo $PATH and $1",
        "copy it to $HOME/$USER first",
        "set $a,$b and $c",
    ] {
        let mut drawn = reading(80, 4);
        drawn.answer(&format!("{prose}\n"));
        assert_eq!(drawn.screen().row(0), prose, "{prose:?} changed");
    }
}

#[test]
fn display_math_nothing_can_draw_keeps_every_character() {
    let tex = r"\[ \unknowncommand{a_b} \\ c*d*e \| \{x\} \]";
    let mut drawn = reading(80, 8);
    drawn.answer(&format!("{tex}\n"));
    let shown = drawn.screen().rows().join("\n");
    assert!(
        shown.contains(r"\unknowncommand{a_b} \\ c*d*e \| \{x\}"),
        "{shown}"
    );
}

#[test]
fn inline_math_markers_are_not_read_as_emphasis() {
    let mut drawn = reading(80, 4);
    drawn.answer("the product $a*b*c$ is small\n");
    let screen = drawn.screen();
    let row = screen.row(0);
    assert!(!row.contains("$abc$"), "{row:?}");
}

/// A math drawer that draws any formula behind a mark of the glyph set it is
/// asked in: a span as `mark` and the formula, a block as one row of them.
struct Marked;

impl Marked {
    fn mark(glyphs: Glyphs) -> &'static str {
        match glyphs {
            Glyphs::Unicode => "»",
            Glyphs::Ascii => ">",
        }
    }
}

impl Draws for Marked {
    fn names(&self) -> &'static [&'static str] {
        &["math"]
    }

    fn inline(&self, span: &str, room: &Room) -> Option<Vec<Run>> {
        Some(vec![Run {
            slot: Slot::Emphasis,
            text: format!("{}{span}", Self::mark(room.glyphs)),
        }])
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        let row = Row::new()
            .then(Slot::Plain, format!("{} {source}", Self::mark(room.glyphs)))
            .clipped(room.columns);
        Ok(Laid {
            natural: (row.columns(), 1),
            rows: vec![row],
            narrow: None,
        })
    }
}

/// A reader of 60 columns whose answers' math [`Marked`] draws.
fn marking() -> Drawn {
    let mut drawn = reading(60, 8);
    drawn
        .render
        .drawers(Drawers::default().with(Arc::new(Marked)));
    drawn
}

/// Every row the record keeps, as words.
fn recorded(drawn: &Drawn) -> Vec<String> {
    let record = &drawn.render.record;
    (0..record.lines())
        .flat_map(|line| record.folded(line))
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

#[test]
fn a_span_being_streamed_is_told_of_in_the_live_foot_and_drawn_once_it_closes() {
    let mut drawn = marking();
    drawn.render.stream("so $a*b \u{1b}[31m\u{202e}").unwrap();

    assert!(
        !recorded(&drawn).iter().any(|row| row.contains("a*b")),
        "the held span reached the record"
    );
    assert_eq!(
        drawn.render.holding(60).map(|row| row.text()),
        Some("$a*b ".to_owned())
    );
    let written = drawn.render.terminal.written();
    for hostile in ["\u{1b}[31", "\u{202e}"] {
        assert!(
            !written.contains(hostile),
            "{hostile:?} reached the terminal"
        );
    }

    drawn.render.stream("c$ holds\n").unwrap();
    drawn.render.settle().unwrap();
    assert_eq!(drawn.render.holding(60), None);
    assert_eq!(drawn.rows(), ["so »a*b c holds"]);
}

#[test]
fn a_formula_drawn_is_laid_out_again_when_the_glyphs_change() {
    let mut drawn = marking();
    drawn.answer("Span $x$ here.\n\n$$\ny\n$$\n");
    assert_eq!(drawn.rows(), ["Span »x here.", "» y"]);

    drawn.render.draws(Glyphs::Ascii);
    drawn.render.draw().unwrap();
    // The block is laid out again; the span, said as the line streamed, keeps
    // what it was drawn with, as every other run of prose does.
    assert_eq!(drawn.rows(), ["Span »x here.", "> y"]);

    drawn.answer("\nThen $z$.\n");
    assert_eq!(drawn.rows().last().map(String::as_str), Some("Then >z."));
}

#[test]
fn a_formula_drawn_is_laid_out_again_in_each_mode_chosen() {
    let mut drawn = marking();
    // As a setting left alone has it.
    assert_eq!(drawn.render.math_mode(), Mode::Image);
    drawn.answer("Span $x$ here.\n\n$$\ny\n$$\n");
    assert_eq!(drawn.rows(), ["Span »x here.", "» y"]);

    drawn.render.math(Mode::Written);
    drawn.render.draw().unwrap();
    assert_eq!(drawn.render.math_mode(), Mode::Written);
    // The block is shown as written; the span, said as the line streamed,
    // keeps what it was drawn as, as every other run of prose does.
    assert_eq!(drawn.rows(), ["Span »x here.", "$$", "y", "$$"]);
    drawn.answer("\nThen $z$.\n");
    assert_eq!(drawn.rows().last().map(String::as_str), Some("Then $z$."));

    // An image cannot be shown yet, so it is text.
    drawn.render.math(Mode::Image);
    drawn.render.draw().unwrap();
    assert_eq!(drawn.rows(), ["Span »x here.", "» y", "Then $z$."]);
}
