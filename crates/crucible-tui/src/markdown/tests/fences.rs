//! Where a fenced block ends: on a line of the marks that opened it, at least
//! as many of them, indented three spaces at most and followed by nothing but
//! spaces and tabs. Read on the held path and on the streamed one, and however
//! the answer is cut into deltas.

use super::*;

/// What `answer` draws once it has ended, read whole; and the same answer cut
/// in two at every byte a cut can fall on, each of which must draw the same.
fn streamed(answer: &str) -> String {
    let whole = standing_text(&drawn(&ended(answer)));
    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        let (first, second) = answer.split_at(cut);
        assert_eq!(
            standing_text(&drawn(&ended_in(&[first, second]))),
            whole,
            "{answer:?} cut at {cut}"
        );
    }
    whole
}

/// The blocks `answer` hands on and the text around them, read whole; and
/// the same answer cut in two at every byte a cut can fall on, each of which
/// must hand on the same.
fn held(answer: &str) -> (Vec<Block>, String) {
    let whole = headed_in(&[answer]);
    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        let (first, second) = answer.split_at(cut);
        assert_eq!(
            headed_in(&[first, second]),
            whole,
            "{answer:?} cut at {cut}"
        );
    }
    whole
}

/// A diff fence written as `fence`, closed, and the line after it.
fn closed_diff(fence: &str) -> (Vec<Block>, String) {
    (
        vec![Block {
            kind: Kind::Diff,
            source: fence.to_owned(),
            closed: true,
        }],
        "after\n".to_owned(),
    )
}

#[test]
fn a_block_fenced_with_four_backticks_is_not_closed_by_three() {
    assert_eq!(
        streamed("````\n```rust\nlet x = 1;\n```\n````\nafter\n"),
        "┌─\n│ ```rust\n│ let x = 1;\n│ ```\n└─\nafter\n"
    );
    // Read in the language it names, which holds the line back to read it.
    assert_eq!(
        streamed("````rust\n```\nlet x = 1;\n```\n````\nafter\n"),
        "┌─ rust\n│ ```\n│ let x = 1;\n│ ```\n└─\nafter\n"
    );
    // Past the six marks any other run is counted to.
    assert_eq!(
        streamed("````````\n```````\ninner\n````````\nafter\n"),
        "┌─\n│ ```````\n│ inner\n└─\nafter\n"
    );

    let fence = "````diff\n```\n-a\n```\n````\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
    let fence = "````````diff\n```````\n-a\n````````\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
}

#[test]
fn a_block_fenced_with_four_tildes_is_not_closed_by_three() {
    assert_eq!(
        streamed("~~~~\n~~~\ninner\n~~~\n~~~~\nafter\n"),
        "┌─\n│ ~~~\n│ inner\n│ ~~~\n└─\nafter\n"
    );

    let fence = "~~~~diff\n~~~\n-a\n~~~\n~~~~\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
}

#[test]
fn a_closing_run_longer_than_the_opening_one_closes_the_block() {
    assert_eq!(
        streamed("```\ninner\n`````\nafter\n"),
        "┌─\n│ inner\n└─\nafter\n"
    );
    assert_eq!(
        streamed("~~~\ninner\n~~~~~~~~\nafter\n"),
        "┌─\n│ inner\n└─\nafter\n"
    );

    let fence = "```diff\n-a\n`````\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
}

#[test]
fn a_closing_run_indented_four_spaces_is_a_line_of_code() {
    assert_eq!(
        streamed("```\ninner\n    ```\n```\nafter\n"),
        "┌─\n│ inner\n│     ```\n└─\nafter\n"
    );
    // Three is as far as a closing run may stand in.
    assert_eq!(
        streamed("```\ninner\n   ```\nafter\n"),
        "┌─\n│ inner\n└─\nafter\n"
    );
    // A blank line's spaces are the code's, as they always were.
    assert_eq!(
        streamed("```\ninner\n   \n```\nafter\n"),
        "┌─\n│ inner\n│    \n└─\nafter\n"
    );

    let fence = "```diff\n-a\n    ```\n```\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
    let fence = "```diff\n-a\n   ```\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
}

#[test]
fn a_closing_run_with_more_written_after_it_is_a_line_of_code() {
    assert_eq!(
        streamed("```\ninner\n``` not yet\n```\nafter\n"),
        "┌─\n│ inner\n│ ``` not yet\n└─\nafter\n"
    );
    assert_eq!(
        streamed("```\ninner\n```` \tnot yet\n```\nafter\n"),
        "┌─\n│ inner\n│ ```` \tnot yet\n└─\nafter\n"
    );
    assert_eq!(
        streamed("```\ninner\n```rust\n```\nafter\n"),
        "┌─\n│ inner\n│ ```rust\n└─\nafter\n"
    );

    let fence = "```diff\n-a\n``` not yet\n```\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
}

#[test]
fn backticks_never_close_a_block_fenced_with_tildes_nor_tildes_one_of_backticks() {
    assert_eq!(streamed("~~~\n```\n~~~\nafter\n"), "┌─\n│ ```\n└─\nafter\n");
    assert_eq!(streamed("```\n~~~\n```\nafter\n"), "┌─\n│ ~~~\n└─\nafter\n");

    let fence = "~~~diff\n```\n~~~\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
    let fence = "```diff\n~~~\n```\n";
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(fence));
}

#[test]
fn a_closing_run_followed_by_spaces_or_a_tab_closes_the_block() {
    for closer in ["```  ", "```\t", "``` \t ", "   ```\t"] {
        assert_eq!(
            streamed(&format!("```\ninner\n{closer}\nafter\n")),
            "┌─\n│ inner\n└─\nafter\n",
            "{closer:?}"
        );
        // The end of the message ends the line as a break would.
        assert_eq!(
            streamed(&format!("```\ninner\n{closer}")),
            "┌─\n│ inner\n└─\n",
            "{closer:?}"
        );

        let fence = format!("```diff\n-a\n{closer}\n");
        assert_eq!(held(&format!("{fence}after\n")), closed_diff(&fence));
    }
}
