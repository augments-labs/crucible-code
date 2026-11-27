//! The picture written from rows laid out by hand: its bound, its escaping
//! and the lines its strokes merge into.

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::Svg;

use super::picture;

fn lines(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|row| (*row).to_owned()).collect()
}

#[test]
fn a_picture_past_its_cap_is_refused() {
    let row = "word ".repeat(400);
    let rows = vec![row; Svg::MOST / 1000];
    assert_eq!(picture(&rows, 2000).map(|_| ()), Err(Undrawn::PastCap));
    let under = vec!["a b".to_owned(); 8];
    assert!(picture(&under, 3).is_ok());
}

#[test]
fn every_character_markup_reads_is_escaped() {
    let svg = picture(&lines(&["<a> & \"b\" 'c'"]), 13).map(|svg| svg.as_str().to_owned());
    assert_eq!(
        svg.as_deref().ok().and_then(|svg| svg.rsplit("\">").next()),
        Some("&lt;a&gt; &amp; &quot;b&quot; &apos;c&apos;</text></svg>")
    );
}

#[test]
fn the_arms_of_a_row_are_one_line_and_two_spaces_part_its_words() {
    let svg =
        picture(&lines(&["┌──┐", "│ab│ cd  ef", "└──┘"]), 11).map(|svg| svg.as_str().to_owned());
    let svg = svg.unwrap_or_default();
    assert!(svg.contains("M4 8H28"), "{svg}");
    assert!(svg.contains("M4 40H28"), "{svg}");
    assert!(svg.contains("M4 8V40"), "{svg}");
    assert!(svg.contains("M28 8V40"), "{svg}");
    assert!(
        svg.contains("x=\"8\" y=\"28\" textLength=\"16\">ab</text>"),
        "{svg}"
    );
    assert!(
        svg.contains("x=\"40\" y=\"28\" textLength=\"16\">cd</text>"),
        "{svg}"
    );
    assert!(
        svg.contains("x=\"72\" y=\"28\" textLength=\"16\">ef</text>"),
        "{svg}"
    );
    assert_eq!(svg.matches("<path").count(), 1, "{svg}");
}

#[test]
fn a_double_line_is_two_lines() {
    let svg = picture(&lines(&["══"]), 2).map(|svg| svg.as_str().to_owned());
    let svg = svg.unwrap_or_default();
    assert!(svg.contains("M0 6H16M0 10H16"), "{svg}");
}
