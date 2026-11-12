//! What a formula reads as, in a line and in a block, read back from the runs
//! and the rows: the conversions the approved picture shows, the refusals,
//! the caps, the ASCII pass, and the pictures checked in beside the drawer:
//! of blocks, and of math in a line of prose read through the reader.

use std::sync::Arc;

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Drawers, Draws, Mode, Room};
use crucible_tui::markdown::{Markdown, Said};
use crucible_tui::testing::dump;
use crucible_tui::{Glyphs, Row, Slot};

use crate::{Formulas, MOST_BYTES, MOST_DEPTH, MOST_ROWS};

const UNICODE: Glyphs = Glyphs::Unicode;
const ASCII: Glyphs = Glyphs::Ascii;

fn room(columns: usize, glyphs: Glyphs) -> Room {
    Room { columns, glyphs }
}

/// What `span` reads as in a line, or `None` where it is shown as written.
fn inline(span: &str, glyphs: Glyphs) -> Option<String> {
    let runs = Formulas.inline(span, &room(64, glyphs))?;
    Some(runs.into_iter().map(|run| run.text).collect())
}

fn block(source: &str, columns: usize, glyphs: Glyphs) -> Result<Vec<Row>, Undrawn> {
    Formulas
        .block(source, &room(columns, glyphs))
        .map(|laid| laid.rows)
}

fn texts(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

const QUADRATIC: &str = r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}";

#[test]
fn the_approved_picture_s_spans_read_as_their_formulas() {
    for (span, read) in [
        (QUADRATIC, "x = (−b ± √(b² − 4ac))/(2a)"),
        (r"a \neq 0", "a ≠ 0"),
        ("E = mc^2", "E = mc²"),
        (r"\sum 1/n^2", "∑ 1/n²"),
    ] {
        assert_eq!(inline(span, UNICODE).as_deref(), Some(read), "{span}");
    }
}

#[test]
fn a_span_is_one_emphasised_run() {
    let runs = Formulas
        .inline("E = mc^2", &room(64, UNICODE))
        .unwrap_or_default();
    assert_eq!(runs.len(), 1);
    assert!(runs.iter().all(|run| run.slot == Slot::Emphasis));
}

#[test]
fn a_span_under_ascii_keeps_its_content_and_loses_the_unicode_minus() {
    assert_eq!(
        inline(QUADRATIC, ASCII).as_deref(),
        Some("x = (-b ± √(b² - 4ac))/(2a)")
    );
}

#[test]
fn scripts_with_no_script_character_are_marked_and_grouped() {
    assert_eq!(inline("x^y", UNICODE).as_deref(), Some("x^y"));
    assert_eq!(inline("x_{ab}", UNICODE).as_deref(), Some("x_(ab)"));
    assert_eq!(inline("e^{i\\theta}", UNICODE).as_deref(), Some("e^(iθ)"));
    assert_eq!(inline("a_1^2", UNICODE).as_deref(), Some("a₁²"));
}

#[test]
fn greek_operators_and_words_read_as_their_characters() {
    assert_eq!(
        inline(r"\alpha \leq \pi \to \infty", UNICODE).as_deref(),
        Some("α ≤ π → ∞")
    );
    assert_eq!(
        inline(r"\text{if } x \in \mathbb{R}", UNICODE).as_deref(),
        Some("if x ∈ R")
    );
    assert_eq!(
        inline(r"\left( \frac{1}{2} \right)", UNICODE).as_deref(),
        Some("( 1/2 )")
    );
}

#[test]
fn a_formula_with_a_part_that_does_not_convert_is_refused_whole() {
    for span in [
        r"\unknownmacro{x}",
        r"\frac{a}{b",
        r"a}",
        r"a & b",
        r"x^2^3",
        r"\begin{tikzpicture} \end{tikzpicture}",
        r"\begin{pmatrix} a",
        r"\text{a $b$}",
        r"\left\unknown x \right)",
        "a \u{1b}[31m b",
        "a \u{202e} b",
    ] {
        assert_eq!(inline(span, UNICODE), None, "{span:?} was drawn");
        assert_eq!(
            block(span, 120, UNICODE).err(),
            Some(Undrawn::NotText),
            "{span:?} was drawn as a block"
        );
    }
}

#[test]
fn a_whole_document_is_shown_as_written_with_its_reason() {
    let document = "\\documentclass{article}\n\\begin{document}\n$x$\n\\end{document}";
    assert_eq!(block(document, 120, UNICODE).err(), Some(Undrawn::NotText));
    assert_eq!(inline(document, UNICODE), None);
}

#[test]
fn nothing_to_draw_is_left_to_the_reader() {
    for blank in ["", "   ", "\n\t", r"\displaystyle"] {
        assert_eq!(
            block(blank, 64, UNICODE).err(),
            Some(Undrawn::NotHere),
            "{blank:?}"
        );
        assert_eq!(inline(blank, UNICODE), None, "{blank:?}");
    }
}

#[test]
fn a_formula_past_its_caps_is_refused_before_it_is_built() {
    let long = "x".repeat(MOST_BYTES + 1);
    assert_eq!(block(&long, 200, UNICODE).err(), Some(Undrawn::PastCap));
    assert_eq!(inline(&long, UNICODE), None);

    // A command a column wide, as many as the cap holds, made up to the cap
    // with spaces the reading trims.
    let commands = "\\alpha".repeat(MOST_BYTES / 6);
    let at_the_cap = format!("{commands}{}", " ".repeat(MOST_BYTES - commands.len()));
    assert_eq!(at_the_cap.len(), MOST_BYTES);
    assert!(inline(&at_the_cap, UNICODE).is_some());

    let deep = format!(
        "{}x{}",
        "{".repeat(MOST_DEPTH + 1),
        "}".repeat(MOST_DEPTH + 1)
    );
    assert_eq!(block(&deep, 200, UNICODE).err(), Some(Undrawn::PastCap));
    let shallow = format!(
        "{}x{}",
        "{".repeat(MOST_DEPTH - 1),
        "}".repeat(MOST_DEPTH - 1)
    );
    assert!(block(&shallow, 200, UNICODE).is_ok());

    let tall = format!(
        r"\begin{{matrix}} {} \end{{matrix}}",
        vec!["a"; MOST_ROWS + 1].join(r" \\ ")
    );
    assert_eq!(block(&tall, 200, UNICODE).err(), Some(Undrawn::PastCap));
    let fits = format!(
        r"\begin{{matrix}} {} \end{{matrix}}",
        vec!["a"; MOST_ROWS].join(r" \\ ")
    );
    assert!(block(&fits, 200, UNICODE).is_ok());
}

#[test]
fn a_block_wider_than_its_room_is_refused_for_that_room_and_drawn_in_a_wider_one() {
    let wide = format!("{} = 0", "a + ".repeat(20));
    assert_eq!(block(&wide, 40, UNICODE).err(), Some(Undrawn::TooWide));
    let rows = block(&wide, 120, UNICODE).unwrap_or_default();
    assert!(texts(&rows).iter().any(|row| row.contains("a + a")));
}

#[test]
fn a_block_is_framed_as_math_and_stacks_its_fraction() {
    let rows = block(QUADRATIC, 64, UNICODE).unwrap_or_default();
    let texts = texts(&rows);
    assert_eq!(texts.first().map(String::as_str), Some("┌─ math"));
    assert_eq!(texts.last().map(String::as_str), Some("└─"));
    assert!(texts.iter().any(|row| row.contains("x = ─")), "{texts:#?}");
    assert!(texts.iter().any(|row| row.contains("2a")), "{texts:#?}");
}

#[test]
fn scripts_stacked_over_a_rule_stand_beside_their_base() {
    // No subscript character for `q`, so the scripts are stacked, and the
    // rule is wider than they are, so the numerator is centred over it.
    for source in [
        r"\frac{x_{qqq}^{2}}{yyyyyyy}",
        r"\frac{x_{qq}^{2}}{yyyyyyyy}",
    ] {
        let texts = texts(&block(source, 64, UNICODE).unwrap_or_default());
        let at = |text: &str| texts.iter().find_map(|row| row.find(text));
        let base = at("x").map(|column| column + 1);
        assert_eq!(at("2"), base, "{source}: {texts:#?}");
        assert_eq!(at("q"), base, "{source}: {texts:#?}");
    }
}

/// What the ASCII set must not draw in a structure: box drawing, the bracket
/// pieces and the Unicode minus.
fn structural_unicode(character: char) -> bool {
    matches!(character, '\u{2500}'..='\u{257f}' | '\u{239b}'..='\u{23ad}' | '−' | '‖')
}

#[test]
fn under_ascii_every_structure_is_ascii() {
    for source in [
        QUADRATIC,
        r"\begin{pmatrix} a & b \\ c & d \\ e & f \end{pmatrix}",
        r"\begin{bmatrix} 1 \\ 2 \\ 3 \end{bmatrix}",
        r"\begin{Bmatrix} 1 \\ 2 \\ 3 \end{Bmatrix}",
        r"\begin{vmatrix} 1 \\ 2 \end{vmatrix}",
        r"\begin{Vmatrix} 1 \\ 2 \end{Vmatrix}",
        r"f(x) = \begin{cases} x & x \geq 0 \\ -x & x < 0 \\ 0 & \text{else} \end{cases}",
        r"\frac{\frac{1}{2}}{\frac{3}{4}}",
    ] {
        let rows = block(source, 64, ASCII).unwrap_or_default();
        assert!(!rows.is_empty(), "{source} was not drawn");
        for row in texts(&rows) {
            assert!(
                !row.chars().any(structural_unicode),
                "{source} drew {row:?} under ASCII"
            );
        }
        let compact = inline(source, ASCII).unwrap_or_default();
        assert!(!compact.chars().any(structural_unicode), "{compact:?}");
    }
}

#[test]
fn every_row_of_a_block_fits_its_room() {
    for columns in 1..=120 {
        for glyphs in [UNICODE, ASCII] {
            if let Ok(rows) = block(QUADRATIC, columns, glyphs) {
                for row in &rows {
                    assert!(row.columns() <= columns, "{columns} {glyphs:?}");
                }
            }
        }
    }
}

#[test]
fn the_drawer_takes_the_math_fences() {
    assert_eq!(Formulas.names(), &["math", "latex", "tex", "katex"]);
}

const PICTURES: [(&str, &str); 5] = [
    ("quadratic", QUADRATIC),
    (
        "cases",
        r"|x| = \begin{cases} x & \text{if } x \geq 0 \\ -x & \text{otherwise} \end{cases}",
    ),
    (
        "matrix",
        r"A = \begin{pmatrix} a_{11} & a_{12} \\ a_{21} & a_{22} \\ \alpha & \beta \end{pmatrix}",
    ),
    (
        "aligned",
        r"\begin{aligned} f(x) &= (x+1)^2 \\ &= x^2 + 2x + 1 \end{aligned}",
    ),
    (
        "sum",
        r"\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
    ),
];

#[test]
fn pictures_of_formulas() {
    for (name, source) in PICTURES {
        for (set, glyphs) in [("unicode", UNICODE), ("ascii", ASCII)] {
            let rows = block(source, 64, glyphs).unwrap_or_default();
            insta::assert_snapshot!(format!("{name}_64_{set}"), dump(&rows, 64));
        }
    }
}

/// Prose holding math in a line, the formulas of the first answer converting
/// and those of the second not.
const PROSE: &[(&str, &str)] = &[
    (
        "converted",
        "The roots are $x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$ when\n\\(a \\neq 0\\), and $\\alpha_1 \\le \\beta^2$ holds for *every* $n$.\n",
    ),
    (
        "unconverted",
        "Here $\\unknown{x}*y*$ stays as written, and so does\n\\(\\foo_1 + b_2\\) beside *emphasis* and $5 of money.\n",
    ),
];

/// The rows `answer` reads as in prose at `columns`, its math drawn by
/// [`Formulas`] as text: each line its runs, as the reader says them.
fn prose(answer: &str, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let drawers = Drawers::default().with(Arc::new(Formulas));
    let mut markdown = Markdown::new(glyphs).drawing(&drawers, Mode::Text);
    let mut rows = vec![Row::new()];
    let mut say = |said: Said<'_>| {
        if let Said::Run { slot, text, .. } = said {
            let mut lines = text.split('\n');
            if let Some(line) = lines.next()
                && let Some(row) = rows.last_mut()
            {
                *row = std::mem::take(row).then(slot, line);
            }
            for line in lines {
                rows.push(Row::new().then(slot, line));
            }
        }
    };
    markdown.read(answer, columns, &mut say);
    markdown.finish(columns, &mut say);
    rows.retain(|row| !row.is_empty());
    rows.into_iter().map(|row| row.clipped(columns)).collect()
}

#[test]
fn pictures_of_math_in_prose() {
    for (name, answer) in PROSE {
        for (set, glyphs) in [("unicode", UNICODE), ("ascii", ASCII)] {
            insta::assert_snapshot!(
                format!("prose_{name}_64_{set}"),
                dump(&prose(answer, 64, glyphs), 64)
            );
        }
    }
}
