//! Where a fenced block ends: on a line of the marks that opened it, at least
//! as many of them, indented three spaces at most and followed by nothing but
//! spaces and tabs. Inside a quote that line, and every line of code, is read
//! past the quote's mark, and a line that ends the quote ends the block; inside
//! an item, past the item's indentation. Read on the held path and on the
//! streamed one, and however the answer is cut into deltas.

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
fn a_closing_run_with_more_blanks_after_it_than_are_kept_is_a_line_of_code() {
    // Past the blanks a line that may close a block keeps, it is code: every
    // one of them is drawn, and the block stays open to the run that closes it.
    let line = format!("```{}x", " ".repeat(5000));
    assert_eq!(
        streamed(&format!("```\ninner\n{line}\n```\nafter\n")),
        format!("┌─\n│ inner\n│ {line}\n└─\nafter\n")
    );

    let fence = format!("```diff\n-a\n{line}\n```\n");
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(&fence));
}

#[test]
fn a_closing_run_with_only_blanks_after_it_closes_the_block_however_many_there_are() {
    // On both paths, on either side of the blanks a line that may close a
    // block keeps.
    for blanks in [MOST - 1, MOST, MOST + 1, 5000] {
        let closer = format!("```{}", " ".repeat(blanks));
        assert_eq!(
            streamed(&format!("```\ninner\n{closer}\nafter\n")),
            "┌─\n│ inner\n└─\nafter\n",
            "{blanks} blanks"
        );

        let fence = format!("```diff\n-a\n{closer}\n");
        assert_eq!(
            held(&format!("{fence}after\n")),
            closed_diff(&fence),
            "{blanks} blanks"
        );
    }
}

#[test]
fn the_blanks_past_those_a_closing_line_keeps_are_given_back_as_they_were() {
    // Past what is kept, the blanks are counted, so a line of code draws every
    // one of them, a tab as a tab.
    let line = format!("```{}{}x", " ".repeat(MOST), "\t".repeat(MOST));
    assert_eq!(
        streamed(&format!("```\ninner\n{line}\n```\nafter\n")),
        format!("┌─\n│ inner\n│ {line}\n└─\nafter\n")
    );

    // Where spaces and tabs mix past what is kept, the count cannot say them,
    // so the line is code on both paths, drawn as it was written.
    let line = format!("```{}\t \t", " ".repeat(MOST));
    assert_eq!(
        streamed(&format!("```\ninner\n{line}\n```\nafter\n")),
        format!("┌─\n│ inner\n│ {line}\n└─\nafter\n")
    );
    let fence = format!("```diff\n-a\n{line}\n```\n");
    assert_eq!(held(&format!("{fence}after\n")), closed_diff(&fence));
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

#[test]
fn a_block_fenced_inside_a_quote_is_closed_by_the_quotes_own_fence() {
    assert_eq!(
        streamed("> A quoted block:\n> ```rust\n> let quoted = true;\n> ```\n\nAfter the quote."),
        "│ A quoted block:\n│ ┌─ rust\n│ │ let quoted = true;\n│ └─\n\nAfter the quote."
    );
    assert_eq!(
        streamed("> [!NOTE]\n> ```\n> code\n> ```\nafter\n"),
        "▎ ◆ Note\n▎ ┌─\n▎ │ code\n▎ └─\nafter\n"
    );
    // The spaces in front of the quote's mark are the quote's, on every row.
    assert_eq!(
        streamed("  > ```\n  > code\n  > ```\nafter\n"),
        "  │ ┌─\n  │ │ code\n  │ └─\nafter\n"
    );

    // A quoted block is never one handed on whole, and is drawn as it streams.
    assert_eq!(
        held("> ```diff\n> -a\n> ```\nafter\n"),
        (Vec::new(), "│ ┌─ diff\n│ │ -a\n│ └─\nafter\n".to_owned())
    );
}

#[test]
fn a_line_that_ends_the_quote_ends_the_block_fenced_inside_it() {
    assert_eq!(
        streamed("> ```\n> code\nafter\n"),
        "│ ┌─\n│ │ code\n│ └─\nafter\n"
    );
    assert_eq!(
        streamed("> ```\n> code\n\nafter\n"),
        "│ ┌─\n│ │ code\n│ └─\n\nafter\n"
    );
    assert_eq!(
        streamed("> [!TIP]\n> ```\n> code\nafter\n"),
        "▎ ✓ Tip\n▎ ┌─\n▎ │ code\n▎ └─\nafter\n"
    );

    assert_eq!(
        held("> ```diff\n> -a\nafter\n"),
        (Vec::new(), "│ ┌─ diff\n│ │ -a\n│ └─\nafter\n".to_owned())
    );
}

#[test]
fn what_follows_the_quote_mark_and_one_space_is_the_code() {
    assert_eq!(
        streamed("> ```\n>> nested\n>no space\n>   two more\n>     ```\n>    ```\nafter\n"),
        "│ ┌─\n│ │ > nested\n│ │ no space\n│ │   two more\n│ │     ```\n│ └─\nafter\n"
    );
}

#[test]
fn a_block_fenced_in_an_item_is_read_past_the_items_indentation() {
    assert_eq!(
        streamed("- item\n  ```\n  code\n    more\n  ```\nafter\n"),
        "• item\n┌─\n│ code\n│   more\n└─\nafter\n"
    );
    assert_eq!(
        streamed("1. item\n   ```\n   code\n   ```\nafter\n"),
        "1. item\n┌─\n│ code\n└─\nafter\n"
    );
    // A nested item's words stand further in, and so may the line that closes
    // a block written under them.
    assert_eq!(
        streamed("- item\n  - nested\n    ```\n    code\n    ```\n  after\n"),
        "• item\n  • nested\n┌─\n│ code\n└─\n  after\n"
    );
    // Three past the item's indentation is as far as a closing line may stand.
    assert_eq!(
        streamed("- item\n  ```\n  code\n      ```\n     ```\nafter\n"),
        "• item\n┌─\n│ code\n│     ```\n└─\nafter\n"
    );
    // A block may open on the item's own line.
    assert_eq!(
        streamed("- ```\n  code\n  ```\nafter\n"),
        "• ┌─\n│ code\n└─\nafter\n"
    );

    let (blocks, _) = closed_diff("```diff\n-a\n```\n");
    assert_eq!(
        held("- item\n  ```diff\n  -a\n  ```\nafter\n"),
        (blocks.clone(), "• item\nafter\n".to_owned())
    );
    assert_eq!(
        held("- item\n  - nested\n    ```diff\n    -a\n    ```\n  after\n"),
        (blocks, "• item\n  • nested\n  after\n".to_owned())
    );
}

#[test]
fn a_block_held_under_an_item_is_named_from_its_lines_past_the_items_indentation() {
    // What the live foot names a held block by while it is gathered.
    let answer = "- item\n  ```mermaid\n  graph TD\n";
    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        let (first, second) = answer.split_at(cut);
        let mut markdown = Markdown::default();
        for delta in [first, second] {
            markdown.read(delta, ROOM, &mut |_| {});
        }
        assert_eq!(
            markdown.opened(),
            Some(("mermaid", "graph TD")),
            "{answer:?} cut at {cut}"
        );
    }
}

#[test]
fn a_run_of_backticks_whose_info_string_holds_a_backtick_opens_no_block() {
    // What follows a run of tildes may hold one, and names the block.
    assert_eq!(
        streamed("~~~a`b\ncode\n~~~\nafter\n"),
        "┌─ a`b\n│ code\n└─\nafter\n"
    );

    // The line is the prose it would be with fewer backticks, so the line
    // after it is too; the spaces or the item in front of it stand as they
    // stand in front of any line of prose.
    let line = streamed("```a`b\nafter\n");
    assert!(
        !line.contains(['┌', '│', '└']) && line.ends_with("\nafter\n"),
        "{line:?}"
    );
    for (answer, lead) in [("  ```a`b\nafter\n", "  "), ("- ```a`b\nafter\n", "• ")] {
        assert_eq!(streamed(answer), format!("{lead}{line}"), "{answer:?}");
    }
    for answer in [
        "``` `x`\nafter\n",
        "````go `x`\nafter\n",
        "```rust x`\nafter\n",
        "> ```a`b\n\nafter\n",
    ] {
        let drawn = streamed(answer);
        assert!(
            !drawn.contains(['┌', '└']) && drawn.ends_with("\nafter\n"),
            "{answer:?} drew {drawn:?}"
        );
    }

    // Nor is a block held for the name it would have had.
    assert_eq!(held("```diff `x`\nafter\n").0, Vec::new());
}
