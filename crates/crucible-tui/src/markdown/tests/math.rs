//! Math in an answer, read from exactly what was written: a span in a line
//! drawn by the drawers' inline answer or shown as written, a dollar that
//! opens no span left as it was, a display block handed to the math drawer
//! from between its markers, and nothing in either read as markdown.

use std::sync::Arc;

use super::*;
use crate::chrome::Undrawn;
use crate::drawing::{Draws, Laid, Mode, Run};

/// A math drawer that draws any formula but one naming `\bad`: a span as
/// itself with `\alpha` as its letter, emphasised, and a block as one row
/// saying what it was handed, its lines joined by a space.
struct Formulas;

impl Draws for Formulas {
    fn names(&self) -> &'static [&'static str] {
        &["math", "latex"]
    }

    fn inline(&self, span: &str, _: &Room) -> Option<Vec<Run>> {
        (!span.contains(r"\bad")).then(|| {
            vec![Run {
                slot: Slot::Emphasis,
                text: span.replace(r"\alpha", "α"),
            }]
        })
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        if source.contains(r"\bad") {
            return Err(Undrawn::NotText);
        }
        let row = Row::new()
            .then(
                Slot::Plain,
                format!(
                    "drawn: {}",
                    source.split_whitespace().collect::<Vec<_>>().join(" ")
                ),
            )
            .clipped(room.columns);
        Ok(Laid {
            natural: (row.columns(), 1),
            rows: vec![row],
            narrow: None,
        })
    }
}

fn formulas() -> Drawers {
    Drawers::default().with(Arc::new(Formulas))
}

/// A reader drawing math by `drawers` in `mode`.
fn reader(drawers: &Drawers, mode: Mode) -> Markdown {
    Markdown::new(Glyphs::Unicode).drawing(drawers, mode)
}

/// Every run `deltas` come to, read one after another and finished, each
/// block laid out by `drawers` and said as its rows, each ending its line.
fn said(deltas: &[&str], drawers: &Drawers, mode: Mode) -> Vec<(Slot, String)> {
    let mut markdown = reader(drawers, mode);
    let mut runs = Vec::new();
    let mut say = |said: Said<'_>| match said {
        Said::Run { slot, text, .. } => runs.push((slot, text.to_owned())),
        Said::Block(block) => {
            for row in lay(&block, ROOM, Glyphs::Unicode, &Links::default(), drawers) {
                for (slot, text) in row.spans() {
                    runs.push((slot, text.to_owned()));
                }
                runs.push((Slot::Plain, "\n".to_owned()));
            }
        }
        Said::Retract | Said::Aside(_) | Said::Shortened(_) => {}
    };
    for delta in deltas {
        markdown.read(delta, ROOM, &mut say);
    }
    markdown.finish(ROOM, &mut say);
    runs
}

/// What one answer read whole draws, as text.
fn text(answer: &str, drawers: &Drawers, mode: Mode) -> String {
    drawn(&said(&[answer], drawers, mode))
}

/// The slot `words` were said under, where one run said exactly them.
fn worn(said: &[(Slot, String)], words: &str) -> Option<Slot> {
    said.iter()
        .find(|(_, text)| text == words)
        .map(|(slot, _)| *slot)
}

#[test]
fn a_span_is_drawn_by_the_drawers_and_nothing_in_it_is_read_as_markdown() {
    let said = said(&["the product $a*b*c$ is small\n"], &formulas(), Mode::Text);
    assert_eq!(drawn(&said), "the product a*b*c is small\n");
    assert_eq!(worn(&said, "a*b*c"), Some(Slot::Emphasis));
}

#[test]
fn a_span_after_the_first_line_is_drawn_as_one_on_it_is() {
    assert_eq!(
        text(
            "First.\n\n- then $a*b$ and\n  \\(c*d\\)\n",
            &formulas(),
            Mode::Text
        ),
        "First.\n\n• then a*b and\n  c*d\n"
    );
}

#[test]
fn a_span_between_brackets_is_drawn_as_one_between_dollars_is() {
    assert_eq!(
        text(r"so \(\alpha_1 + b_2\) holds", &formulas(), Mode::Text),
        "so α_1 + b_2 holds"
    );
    assert_eq!(
        text("so $\\alpha_1 + b_2$ holds\n", &formulas(), Mode::Text),
        "so α_1 + b_2 holds\n"
    );
}

#[test]
fn image_draws_a_span_as_text_until_a_picture_can_be_shown() {
    for answer in ["the product $a*b*c$ is small\n", "and \\(x_1\\) too\n"] {
        assert_eq!(
            said(&[answer], &formulas(), Mode::Image),
            said(&[answer], &formulas(), Mode::Text),
            "{answer:?}"
        );
    }
}

#[test]
fn a_span_nothing_draws_shows_every_character_it_was_written_with() {
    for (answer, drawers) in [
        ("so $\\bad *x* \\| y_1$ holds\n", formulas()),
        ("so \\(\\bad_y \\{z\\}\\) holds\n", formulas()),
        ("so $a*b*c$ and \\(x_1\\) hold\n", Drawers::default()),
    ] {
        let said = said(&[answer], &drawers, Mode::Text);
        assert_eq!(drawn(&said), answer);
        assert!(
            said.iter().all(|(slot, _)| *slot == Slot::Plain),
            "{said:?}"
        );
    }
}

/// Display math under it is the record's to lay out as written: the reader
/// hands the block on whatever the mode.
#[test]
fn written_shows_every_span_as_written() {
    for answer in ["the product $a*b*c$ is small\n", "and \\(x_1\\) too\n"] {
        assert_eq!(
            text(answer, &formulas(), Mode::Written),
            answer,
            "{answer:?}"
        );
    }
}

#[test]
fn a_dollar_that_opens_no_span_is_left_as_it_was() {
    for answer in [
        // The closing dollar is followed by a letter, a digit or `_`.
        "$\\alpha$s and $n$th and $x$1 and $y$_\n",
        // The opening dollar is followed by a space, a tab or a comma.
        "$ x$ and $\tx$ and $,x$\n",
        // The space just inside one dollar is not inside the other.
        "the $x $ here\n",
        // Over the end of the line.
        "a $x\ny$ b\n",
        // Holding an address.
        "see $http://example.com$ now\n",
        // Two dollars in the middle of a line.
        "a $$ b\n",
        // Money and the shell, with a drawer that would draw a span.
        "It costs $5 and $10.\n",
        "$5/$10 per seat\n",
        "$20,000-$30,000 a year\n",
        "Set $HOME before running.\n",
        "Run $browser then $deploy on it.\n",
        "echo $PATH and $1\n",
        "copy it to $HOME/$USER first\n",
        "set $a,$b and $c\n",
    ] {
        assert_eq!(text(answer, &formulas(), Mode::Text), answer, "{answer:?}");
    }
}

#[test]
fn a_span_holding_a_backtick_is_no_span_and_the_code_in_it_is_code() {
    let said = said(&["a $x `y` z$ b\n"], &formulas(), Mode::Text);
    assert_eq!(drawn(&said), "a $x y z$ b\n");
    assert_eq!(worn(&said, "y"), Some(Slot::Code));
}

#[test]
fn a_dollar_a_backslash_stands_before_is_a_dollar() {
    assert_eq!(
        text("costs \\$5 and \\$6\n", &formulas(), Mode::Text),
        "costs $5 and $6\n"
    );
    assert_eq!(
        text("so $a \\$ b$ holds\n", &formulas(), Mode::Text),
        "so a \\$ b holds\n"
    );
}

#[test]
fn nothing_in_code_is_math() {
    let said = said(
        &["run `$a*b$` and\n```\n$x$\n```\n"],
        &formulas(),
        Mode::Text,
    );
    assert_eq!(worn(&said, "$a*b$"), Some(Slot::Code));
    assert!(drawn(&said).contains("$x$"), "{said:?}");
}

#[test]
fn a_span_takes_the_emphasis_around_it() {
    let said = said(&["*see $x$ now*\n"], &formulas(), Mode::Text);
    assert_eq!(drawn(&said), "see x now\n");
    assert_eq!(slots(&said), vec![Slot::Emphasis, Slot::Plain]);
}

#[test]
fn display_math_is_drawn_by_the_math_drawer_from_between_its_markers() {
    for (answer, body) in [
        ("$$\nx^2\n$$\n", "x^2"),
        ("$$ x^2 $$\n", "x^2"),
        ("\\[\na+b\n\\]\n", "a+b"),
        ("\\[ a+b \\]\n", "a+b"),
        (
            "\\begin{align}\na &= b\n\\end{align}\n",
            "\\begin{align} a &= b \\end{align}",
        ),
        ("```latex\nE = mc^2\n```\n", "E = mc^2"),
    ] {
        assert_eq!(
            text(answer, &formulas(), Mode::Text),
            format!("drawn: {body}\n"),
            "{answer:?}"
        );
    }
}

#[test]
fn display_math_nothing_draws_keeps_every_character() {
    for (answer, drawers) in [
        (
            "$$\n\\unknown{a_b} \\\\ c*d*e \\| \\{x\\}\n$$\n",
            Drawers::default(),
        ),
        (
            "\\[ \\unknown{a_b} \\\\ c*d*e \\| \\{x\\} \\]\n",
            Drawers::default(),
        ),
        (
            "\\begin{gather}\n*a* `b` _c_ ~d~ \\$e$\n- f\n\\end{gather}\n",
            Drawers::default(),
        ),
        // Text after the closer leaves no formula to hand on.
        ("$$ a_b $$ c*d*\n", formulas()),
    ] {
        let said = said(&[answer], &drawers, Mode::Text);
        assert_eq!(drawn(&said), answer, "{answer:?}");
    }
}

#[test]
fn display_math_a_drawer_refuses_is_framed_with_why_and_kept_whole() {
    for (answer, refused) in [
        (
            "$$\n\\bad{a_b} \\\\ c*d*e\n$$\n",
            "$$\n\\bad{a_b} \\\\ c*d*e\n$$",
        ),
        ("\\[ \\bad `x` \\]\n", "\\[ \\bad `x` \\]"),
    ] {
        let shown = text(answer, &formulas(), Mode::Text);
        assert_eq!(
            shown,
            text(
                &format!("````math\n{refused}\n````\n"),
                &formulas(),
                Mode::Text
            ),
            "{answer:?}"
        );
        assert!(
            shown
                .lines()
                .next()
                .is_some_and(|top| top.contains("math · not drawn as text")),
            "{shown}"
        );
    }
}

#[test]
fn an_unclosed_span_or_block_stays_as_written() {
    for answer in [
        "x $a+b",
        "x $a+b\n",
        "x \\(a+b",
        "x \\(a_b\n",
        "$$\na*b_c\n",
        "\\[ a*b_c\n",
    ] {
        assert_eq!(text(answer, &formulas(), Mode::Text), answer, "{answer:?}");
    }
}

#[test]
fn a_span_or_a_block_cut_at_any_byte_reads_as_it_does_whole() {
    for answer in [
        "a $x*y$ b \\(z_1\\) c $5 and $6.\n",
        "Before.\n\n$$\na*b \\\\ c_d\n$$\n\nAfter $x$.\n",
        "\\[ a*b \\]\n",
    ] {
        let whole = said(&[answer], &formulas(), Mode::Text);
        for cut in 1..answer.len() {
            let (head, tail) = answer.split_at(cut);
            assert_eq!(
                drawn(&said(&[head, tail], &formulas(), Mode::Text)),
                drawn(&whole),
                "{answer:?} cut at {cut}"
            );
        }
    }
}

#[test]
fn a_span_being_read_is_shown_at_the_foot_as_written() {
    let drawers = formulas();
    let mut markdown = reader(&drawers, Mode::Text);
    let mut said = Vec::new();
    markdown.read("so $a*b \u{1b}[31m", ROOM, &mut |said_now| {
        if let Said::Run { text, .. } = said_now {
            said.push(text.to_owned());
        }
    });
    assert_eq!(said.concat(), "so ");
    assert!(markdown.holds_a_span());
    assert_eq!(
        markdown.spanning(ROOM).map(|row| row.text()),
        Some("$a*b ".to_owned())
    );
    markdown.read("$.", ROOM, &mut |_| {});
    assert!(!markdown.holds_a_span());
    assert!(markdown.spanning(ROOM).is_none());
}
