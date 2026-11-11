//! Every corpus answer as a picture, at the widths its layout has to survive.
//!
//! The whole-screen suite shows an answer through a real terminal, which is the
//! only place the composition, the bands and the box can be seen together. This
//! shows the same answer through the reader and the record alone: the rows the
//! transcript holds, with the slot each run wears, laid out by `dump.rs`. A
//! construct that moves here has moved in the drawing rather than in the
//! terminal around it, which is the difference a later redraw has to explain.
//!
//! Forty, eighty and one hundred and twenty columns, in both glyph sets. The
//! four answers whose layout changes shape with the width are also kept at the
//! widths where it does: a table splitting its columns, and a diagram or a
//! display formula taking the narrow form its text drawing has at that width.
//! Every width from eight to one hundred and sixty is a property a later task
//! asserts, not a picture kept here.
//!
//! A picture is what the reader drew, so nothing here asserts a rule: the
//! accepted file is the assertion, and a moved one is a drawing that changed.

use crate::color::Theme;
use crate::dump::dump;
use crate::escape::Escapes;
use crate::glyphs::Glyphs;
use crate::record::Record;
use crate::row::Row;
use crate::width;

use super::corpus;
use super::{Markdown, Said};

/// What the answer called `name` lays out as, at `columns`, in `glyphs`.
///
/// The same three steps the renderer takes for a run: escape sequences and
/// format characters go, the text lands in a record at the width, and what the
/// record kept is written out a row at a time. A block lands as the renderer
/// lands one, kept with its source and laid out by the record.
fn pictured(name: &str, columns: usize, glyphs: Glyphs) -> String {
    let answer = corpus::answer(name).unwrap_or_else(|| panic!("no corpus answer named {name}"));
    picture(answer, columns, glyphs)
}

/// What `answer` lays out as, at `columns`, in `glyphs`: the steps
/// [`pictured`] describes, for an answer that is not in the corpus.
pub(crate) fn picture(answer: &str, columns: usize, glyphs: Glyphs) -> String {
    dump(&laid(answer, columns, glyphs), columns)
}

/// The rows the record holds for `answer` at `columns`, in `glyphs`.
pub(crate) fn laid(answer: &str, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut record = Record::new(columns);
    let mut markdown = Markdown::new(glyphs);
    let mut escapes = Escapes::default();
    let mut free = String::new();
    let mut write = |said: Said<'_>| match said {
        Said::Run { slot, text, link } => {
            free.clear();
            free.extend(
                text.chars()
                    .filter(|character| !escapes.holds(*character) && !width::unshown(*character)),
            );
            record.write(slot, &free, link);
        }
        // A block is kept with its source, as the renderer keeps one; the
        // theme is any, since a picture is glyphs and slots and no colour.
        Said::Block(mut block) => {
            block.source = block
                .source
                .chars()
                .filter(|character| !escapes.holds(*character) && !width::unshown(*character))
                .collect();
            record.drawn(block, glyphs, Theme::Dark);
        }
        Said::Retract => record.retract(),
        Said::Aside(text) => {
            free.clear();
            free.extend(
                text.chars()
                    .filter(|character| !escapes.holds(*character) && !width::unshown(*character)),
            );
            record.aside(&free);
        }
        Said::Shortened(address) => record.shortened(address),
    };

    markdown.read(answer, columns, &mut write);
    markdown.finish(columns, &mut write);

    record.end();
    let mut rows = Vec::new();
    for line in 0..record.lines() {
        rows.extend(record.folded(line));
    }
    rows
}

/// Every answer is named once, ends with the line a case waits for, and is
/// reachable under its own name.
#[test]
fn every_answer_is_named_once_and_ends_with_the_line_a_case_waits_for() {
    for (at, (name, text)) in corpus::ANSWERS.iter().enumerate() {
        assert!(
            text.ends_with(&format!("{}\n", corpus::END)),
            "{name} does not end with the line a case waits for"
        );
        assert!(
            !corpus::ANSWERS
                .iter()
                .skip(at + 1)
                .any(|(other, _)| other == name),
            "{name} is named twice"
        );
        assert_eq!(corpus::answer(name), Some(*text), "{name} is not reachable");
    }
    assert!(corpus::answer("no such answer").is_none());
}

// tables

#[test]
fn transcript_tables_40_unicode() {
    insta::assert_snapshot!(pictured("tables", 40, Glyphs::Unicode));
}

#[test]
fn transcript_tables_40_ascii() {
    insta::assert_snapshot!(pictured("tables", 40, Glyphs::Ascii));
}

#[test]
fn transcript_tables_60_unicode() {
    insta::assert_snapshot!(pictured("tables", 60, Glyphs::Unicode));
}

#[test]
fn transcript_tables_60_ascii() {
    insta::assert_snapshot!(pictured("tables", 60, Glyphs::Ascii));
}

#[test]
fn transcript_tables_80_unicode() {
    insta::assert_snapshot!(pictured("tables", 80, Glyphs::Unicode));
}

#[test]
fn transcript_tables_80_ascii() {
    insta::assert_snapshot!(pictured("tables", 80, Glyphs::Ascii));
}

#[test]
fn transcript_tables_100_unicode() {
    insta::assert_snapshot!(pictured("tables", 100, Glyphs::Unicode));
}

#[test]
fn transcript_tables_100_ascii() {
    insta::assert_snapshot!(pictured("tables", 100, Glyphs::Ascii));
}

#[test]
fn transcript_tables_120_unicode() {
    insta::assert_snapshot!(pictured("tables", 120, Glyphs::Unicode));
}

#[test]
fn transcript_tables_120_ascii() {
    insta::assert_snapshot!(pictured("tables", 120, Glyphs::Ascii));
}

// rules

#[test]
fn transcript_rules_40_unicode() {
    insta::assert_snapshot!(pictured("rules", 40, Glyphs::Unicode));
}

#[test]
fn transcript_rules_40_ascii() {
    insta::assert_snapshot!(pictured("rules", 40, Glyphs::Ascii));
}

#[test]
fn transcript_rules_80_unicode() {
    insta::assert_snapshot!(pictured("rules", 80, Glyphs::Unicode));
}

#[test]
fn transcript_rules_80_ascii() {
    insta::assert_snapshot!(pictured("rules", 80, Glyphs::Ascii));
}

#[test]
fn transcript_rules_120_unicode() {
    insta::assert_snapshot!(pictured("rules", 120, Glyphs::Unicode));
}

#[test]
fn transcript_rules_120_ascii() {
    insta::assert_snapshot!(pictured("rules", 120, Glyphs::Ascii));
}

// lists

#[test]
fn transcript_lists_40_unicode() {
    insta::assert_snapshot!(pictured("lists", 40, Glyphs::Unicode));
}

#[test]
fn transcript_lists_40_ascii() {
    insta::assert_snapshot!(pictured("lists", 40, Glyphs::Ascii));
}

#[test]
fn transcript_lists_80_unicode() {
    insta::assert_snapshot!(pictured("lists", 80, Glyphs::Unicode));
}

#[test]
fn transcript_lists_80_ascii() {
    insta::assert_snapshot!(pictured("lists", 80, Glyphs::Ascii));
}

#[test]
fn transcript_lists_120_unicode() {
    insta::assert_snapshot!(pictured("lists", 120, Glyphs::Unicode));
}

#[test]
fn transcript_lists_120_ascii() {
    insta::assert_snapshot!(pictured("lists", 120, Glyphs::Ascii));
}

// quotes

#[test]
fn transcript_quotes_40_unicode() {
    insta::assert_snapshot!(pictured("quotes", 40, Glyphs::Unicode));
}

#[test]
fn transcript_quotes_40_ascii() {
    insta::assert_snapshot!(pictured("quotes", 40, Glyphs::Ascii));
}

#[test]
fn transcript_quotes_80_unicode() {
    insta::assert_snapshot!(pictured("quotes", 80, Glyphs::Unicode));
}

#[test]
fn transcript_quotes_80_ascii() {
    insta::assert_snapshot!(pictured("quotes", 80, Glyphs::Ascii));
}

#[test]
fn transcript_quotes_120_unicode() {
    insta::assert_snapshot!(pictured("quotes", 120, Glyphs::Unicode));
}

#[test]
fn transcript_quotes_120_ascii() {
    insta::assert_snapshot!(pictured("quotes", 120, Glyphs::Ascii));
}

// code

#[test]
fn transcript_code_40_unicode() {
    insta::assert_snapshot!(pictured("code", 40, Glyphs::Unicode));
}

#[test]
fn transcript_code_40_ascii() {
    insta::assert_snapshot!(pictured("code", 40, Glyphs::Ascii));
}

#[test]
fn transcript_code_80_unicode() {
    insta::assert_snapshot!(pictured("code", 80, Glyphs::Unicode));
}

#[test]
fn transcript_code_80_ascii() {
    insta::assert_snapshot!(pictured("code", 80, Glyphs::Ascii));
}

#[test]
fn transcript_code_120_unicode() {
    insta::assert_snapshot!(pictured("code", 120, Glyphs::Unicode));
}

#[test]
fn transcript_code_120_ascii() {
    insta::assert_snapshot!(pictured("code", 120, Glyphs::Ascii));
}

// headings

#[test]
fn transcript_headings_40_unicode() {
    insta::assert_snapshot!(pictured("headings", 40, Glyphs::Unicode));
}

#[test]
fn transcript_headings_40_ascii() {
    insta::assert_snapshot!(pictured("headings", 40, Glyphs::Ascii));
}

#[test]
fn transcript_headings_80_unicode() {
    insta::assert_snapshot!(pictured("headings", 80, Glyphs::Unicode));
}

#[test]
fn transcript_headings_80_ascii() {
    insta::assert_snapshot!(pictured("headings", 80, Glyphs::Ascii));
}

#[test]
fn transcript_headings_120_unicode() {
    insta::assert_snapshot!(pictured("headings", 120, Glyphs::Unicode));
}

#[test]
fn transcript_headings_120_ascii() {
    insta::assert_snapshot!(pictured("headings", 120, Glyphs::Ascii));
}

// alerts

#[test]
fn transcript_alerts_40_unicode() {
    insta::assert_snapshot!(pictured("alerts", 40, Glyphs::Unicode));
}

#[test]
fn transcript_alerts_40_ascii() {
    insta::assert_snapshot!(pictured("alerts", 40, Glyphs::Ascii));
}

#[test]
fn transcript_alerts_80_unicode() {
    insta::assert_snapshot!(pictured("alerts", 80, Glyphs::Unicode));
}

#[test]
fn transcript_alerts_80_ascii() {
    insta::assert_snapshot!(pictured("alerts", 80, Glyphs::Ascii));
}

#[test]
fn transcript_alerts_120_unicode() {
    insta::assert_snapshot!(pictured("alerts", 120, Glyphs::Unicode));
}

#[test]
fn transcript_alerts_120_ascii() {
    insta::assert_snapshot!(pictured("alerts", 120, Glyphs::Ascii));
}

// images

#[test]
fn transcript_images_40_unicode() {
    insta::assert_snapshot!(pictured("images", 40, Glyphs::Unicode));
}

#[test]
fn transcript_images_40_ascii() {
    insta::assert_snapshot!(pictured("images", 40, Glyphs::Ascii));
}

#[test]
fn transcript_images_80_unicode() {
    insta::assert_snapshot!(pictured("images", 80, Glyphs::Unicode));
}

#[test]
fn transcript_images_80_ascii() {
    insta::assert_snapshot!(pictured("images", 80, Glyphs::Ascii));
}

#[test]
fn transcript_images_120_unicode() {
    insta::assert_snapshot!(pictured("images", 120, Glyphs::Unicode));
}

#[test]
fn transcript_images_120_ascii() {
    insta::assert_snapshot!(pictured("images", 120, Glyphs::Ascii));
}

// notes

#[test]
fn transcript_notes_40_unicode() {
    insta::assert_snapshot!(pictured("notes", 40, Glyphs::Unicode));
}

#[test]
fn transcript_notes_40_ascii() {
    insta::assert_snapshot!(pictured("notes", 40, Glyphs::Ascii));
}

#[test]
fn transcript_notes_80_unicode() {
    insta::assert_snapshot!(pictured("notes", 80, Glyphs::Unicode));
}

#[test]
fn transcript_notes_80_ascii() {
    insta::assert_snapshot!(pictured("notes", 80, Glyphs::Ascii));
}

#[test]
fn transcript_notes_120_unicode() {
    insta::assert_snapshot!(pictured("notes", 120, Glyphs::Unicode));
}

#[test]
fn transcript_notes_120_ascii() {
    insta::assert_snapshot!(pictured("notes", 120, Glyphs::Ascii));
}

// links

#[test]
fn transcript_links_40_unicode() {
    insta::assert_snapshot!(pictured("links", 40, Glyphs::Unicode));
}

#[test]
fn transcript_links_40_ascii() {
    insta::assert_snapshot!(pictured("links", 40, Glyphs::Ascii));
}

#[test]
fn transcript_links_80_unicode() {
    insta::assert_snapshot!(pictured("links", 80, Glyphs::Unicode));
}

#[test]
fn transcript_links_80_ascii() {
    insta::assert_snapshot!(pictured("links", 80, Glyphs::Ascii));
}

#[test]
fn transcript_links_120_unicode() {
    insta::assert_snapshot!(pictured("links", 120, Glyphs::Unicode));
}

#[test]
fn transcript_links_120_ascii() {
    insta::assert_snapshot!(pictured("links", 120, Glyphs::Ascii));
}

// inline

#[test]
fn transcript_inline_40_unicode() {
    insta::assert_snapshot!(pictured("inline", 40, Glyphs::Unicode));
}

#[test]
fn transcript_inline_40_ascii() {
    insta::assert_snapshot!(pictured("inline", 40, Glyphs::Ascii));
}

#[test]
fn transcript_inline_80_unicode() {
    insta::assert_snapshot!(pictured("inline", 80, Glyphs::Unicode));
}

#[test]
fn transcript_inline_80_ascii() {
    insta::assert_snapshot!(pictured("inline", 80, Glyphs::Ascii));
}

#[test]
fn transcript_inline_120_unicode() {
    insta::assert_snapshot!(pictured("inline", 120, Glyphs::Unicode));
}

#[test]
fn transcript_inline_120_ascii() {
    insta::assert_snapshot!(pictured("inline", 120, Glyphs::Ascii));
}

// math

#[test]
fn transcript_math_40_unicode() {
    insta::assert_snapshot!(pictured("math", 40, Glyphs::Unicode));
}

#[test]
fn transcript_math_40_ascii() {
    insta::assert_snapshot!(pictured("math", 40, Glyphs::Ascii));
}

#[test]
fn transcript_math_50_unicode() {
    insta::assert_snapshot!(pictured("math", 50, Glyphs::Unicode));
}

#[test]
fn transcript_math_50_ascii() {
    insta::assert_snapshot!(pictured("math", 50, Glyphs::Ascii));
}

#[test]
fn transcript_math_80_unicode() {
    insta::assert_snapshot!(pictured("math", 80, Glyphs::Unicode));
}

#[test]
fn transcript_math_80_ascii() {
    insta::assert_snapshot!(pictured("math", 80, Glyphs::Ascii));
}

#[test]
fn transcript_math_100_unicode() {
    insta::assert_snapshot!(pictured("math", 100, Glyphs::Unicode));
}

#[test]
fn transcript_math_100_ascii() {
    insta::assert_snapshot!(pictured("math", 100, Glyphs::Ascii));
}

#[test]
fn transcript_math_120_unicode() {
    insta::assert_snapshot!(pictured("math", 120, Glyphs::Unicode));
}

#[test]
fn transcript_math_120_ascii() {
    insta::assert_snapshot!(pictured("math", 120, Glyphs::Ascii));
}

// mermaid_graphs

#[test]
fn transcript_mermaid_graphs_40_unicode() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 40, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_graphs_40_ascii() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 40, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_graphs_60_unicode() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 60, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_graphs_60_ascii() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 60, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_graphs_80_unicode() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 80, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_graphs_80_ascii() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 80, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_graphs_100_unicode() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 100, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_graphs_100_ascii() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 100, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_graphs_120_unicode() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 120, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_graphs_120_ascii() {
    insta::assert_snapshot!(pictured("mermaid_graphs", 120, Glyphs::Ascii));
}

// mermaid_timelines

#[test]
fn transcript_mermaid_timelines_40_unicode() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 40, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_timelines_40_ascii() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 40, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_timelines_60_unicode() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 60, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_timelines_60_ascii() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 60, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_timelines_80_unicode() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 80, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_timelines_80_ascii() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 80, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_timelines_100_unicode() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 100, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_timelines_100_ascii() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 100, Glyphs::Ascii));
}

#[test]
fn transcript_mermaid_timelines_120_unicode() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 120, Glyphs::Unicode));
}

#[test]
fn transcript_mermaid_timelines_120_ascii() {
    insta::assert_snapshot!(pictured("mermaid_timelines", 120, Glyphs::Ascii));
}

// diff

#[test]
fn transcript_diff_40_unicode() {
    insta::assert_snapshot!(pictured("diff", 40, Glyphs::Unicode));
}

#[test]
fn transcript_diff_40_ascii() {
    insta::assert_snapshot!(pictured("diff", 40, Glyphs::Ascii));
}

#[test]
fn transcript_diff_80_unicode() {
    insta::assert_snapshot!(pictured("diff", 80, Glyphs::Unicode));
}

#[test]
fn transcript_diff_80_ascii() {
    insta::assert_snapshot!(pictured("diff", 80, Glyphs::Ascii));
}

#[test]
fn transcript_diff_120_unicode() {
    insta::assert_snapshot!(pictured("diff", 120, Glyphs::Unicode));
}

#[test]
fn transcript_diff_120_ascii() {
    insta::assert_snapshot!(pictured("diff", 120, Glyphs::Ascii));
}

// hostile

#[test]
fn transcript_hostile_40_unicode() {
    insta::assert_snapshot!(pictured("hostile", 40, Glyphs::Unicode));
}

#[test]
fn transcript_hostile_40_ascii() {
    insta::assert_snapshot!(pictured("hostile", 40, Glyphs::Ascii));
}

#[test]
fn transcript_hostile_80_unicode() {
    insta::assert_snapshot!(pictured("hostile", 80, Glyphs::Unicode));
}

#[test]
fn transcript_hostile_80_ascii() {
    insta::assert_snapshot!(pictured("hostile", 80, Glyphs::Ascii));
}

#[test]
fn transcript_hostile_120_unicode() {
    insta::assert_snapshot!(pictured("hostile", 120, Glyphs::Unicode));
}

#[test]
fn transcript_hostile_120_ascii() {
    insta::assert_snapshot!(pictured("hostile", 120, Glyphs::Ascii));
}

// tool_edit

#[test]
fn transcript_tool_edit_40_unicode() {
    insta::assert_snapshot!(pictured("tool_edit", 40, Glyphs::Unicode));
}

#[test]
fn transcript_tool_edit_40_ascii() {
    insta::assert_snapshot!(pictured("tool_edit", 40, Glyphs::Ascii));
}

#[test]
fn transcript_tool_edit_80_unicode() {
    insta::assert_snapshot!(pictured("tool_edit", 80, Glyphs::Unicode));
}

#[test]
fn transcript_tool_edit_80_ascii() {
    insta::assert_snapshot!(pictured("tool_edit", 80, Glyphs::Ascii));
}

#[test]
fn transcript_tool_edit_120_unicode() {
    insta::assert_snapshot!(pictured("tool_edit", 120, Glyphs::Unicode));
}

#[test]
fn transcript_tool_edit_120_ascii() {
    insta::assert_snapshot!(pictured("tool_edit", 120, Glyphs::Ascii));
}
