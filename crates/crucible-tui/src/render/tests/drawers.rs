//! A fence of an answer drawn by the drawers the renderer was handed: on the
//! screen, at every width the window comes to, in a run whose output is a
//! file, and when a model writes one to carry an escape.

use std::sync::Arc;

use super::*;
use crate::chrome::Undrawn;
use crate::drawing::{Drawers, Draws, Laid, Room};

/// A drawer that takes `diff` and `patch` and draws back every line it was
/// handed exactly as it was handed it, under a mark no reader draws and cut
/// to the room: whatever reaches it reaches the row.
///
/// It declines a block that says `not a change`, and says one that says
/// `too long a change` is past what it holds.
pub(in crate::render) struct Echo;

impl Draws for Echo {
    fn names(&self) -> &'static [&'static str] {
        &["diff", "patch"]
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        if source.contains("not a change") {
            return Err(Undrawn::NotHere);
        }
        if source.contains("too long a change") {
            return Err(Undrawn::PastCap);
        }
        let mark = match room.glyphs {
            Glyphs::Unicode => "»",
            Glyphs::Ascii => ">",
        };
        let rows: Vec<Row> = source
            .lines()
            .map(|line| {
                Row::new()
                    .then_structural(Slot::Quiet, mark)
                    .then(Slot::Added, format!(" {line}"))
                    .clipped(room.columns)
            })
            .collect();
        Ok(Laid {
            natural: (rows.iter().map(Row::columns).max().unwrap_or(0), rows.len()),
            rows,
            narrow: None,
        })
    }
}

/// The registry holding [`Echo`] alone.
pub(in crate::render) fn echoing() -> Drawers {
    Drawers::default().with(Arc::new(Echo))
}

/// An answer with a change in a fence between two lines of prose.
const ANSWER: &str =
    "Before.\n\n```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n```\n\nAfter.\n";

/// The rows of `rows` that say anything, without the blank cells after them.
fn said(rows: &[String]) -> Vec<&str> {
    rows.iter()
        .map(|row| row.trim_end())
        .filter(|row| !row.is_empty())
        .collect()
}

#[test]
fn a_fence_of_an_answer_is_drawn_by_the_drawer_the_renderer_was_handed() {
    for (glyphs, mark) in [(Glyphs::Unicode, "»"), (Glyphs::Ascii, ">")] {
        let mut drawn = reading(60, 16);
        drawn.render.draws(glyphs);
        drawn.render.drawers(echoing());
        drawn.answer(ANSWER);

        let rows = drawn.rows();
        assert_eq!(
            said(&rows),
            [
                "Before.".to_owned(),
                format!("{mark} --- a/x.rs"),
                format!("{mark} +++ b/x.rs"),
                format!("{mark} @@ -1 +1 @@"),
                format!("{mark} -old"),
                format!("{mark} +new"),
                "After.".to_owned(),
            ],
            "{glyphs:?}"
        );
    }
}

#[test]
fn a_renderer_handed_no_drawer_draws_a_fence_exactly_as_it_was_written() {
    let mut untold = reading(60, 16);
    untold.answer(ANSWER);
    assert_eq!(
        said(&untold.rows()),
        [
            "Before.",
            "┌─ diff",
            "│ --- a/x.rs",
            "│ +++ b/x.rs",
            "│ @@ -1 +1 @@",
            "│ -old",
            "│ +new",
            "└─",
            "After.",
        ]
    );

    // Handed a registry with nothing in it, it writes the same bytes.
    let mut empty = reading(60, 16);
    empty.render.drawers(Drawers::default());
    empty.answer(ANSWER);
    assert_eq!(
        empty.render.terminal.written(),
        untold.render.terminal.written()
    );
}

#[test]
fn a_fence_its_drawer_declines_is_drawn_as_written_and_one_past_its_caps_says_so() {
    let mut declined = reading(60, 16);
    declined.render.drawers(echoing());
    declined.answer("```diff\nnot a change\n```\n");
    assert_eq!(said(&declined.rows()), ["┌─ diff", "│ not a change", "└─"]);

    let mut past = reading(60, 16);
    past.render.drawers(echoing());
    past.answer("```diff\ntoo long a change\n```\n");
    assert_eq!(
        said(&past.rows()),
        [
            "┌─ diff · longer than this can hold · shown as written",
            "│ too long a change",
            "└─"
        ]
    );
}

#[test]
fn a_fence_the_answer_ended_inside_is_drawn_as_written_on_the_screen() {
    let mut drawn = reading(60, 16);
    drawn.render.drawers(echoing());
    drawn.answer("```diff\n-old\n+new\n");

    assert_eq!(said(&drawn.rows()), ["┌─ diff", "│ -old", "│ +new"]);
}

#[test]
fn a_drawn_fence_narrowed_is_the_fence_a_window_that_narrow_draws() {
    let answer = "```diff\n+a line a good deal longer than the narrow window is wide\n```\n";
    let mut fresh = reading(24, 16);
    fresh.render.drawers(echoing());
    fresh.answer(answer);

    let mut drawn = reading(70, 16);
    drawn.render.drawers(echoing());
    drawn.answer(answer);
    assert_eq!(
        said(&drawn.rows()),
        ["» +a line a good deal longer than the narrow window is wide"]
    );
    drawn.resize(24, 16);

    assert_eq!(drawn.rows(), fresh.rows());
    assert!(
        said(&drawn.rows())
            .iter()
            .all(|row| row.chars().count() <= 24),
        "{:#?}",
        drawn.rows()
    );
}

#[test]
fn a_drawn_fence_is_listed_with_the_whole_fence_it_was_written_as() {
    let mut drawn = reading(60, 16);
    drawn.render.drawers(echoing());
    drawn.answer(ANSWER);

    let sources: Vec<(&str, &str)> = drawn
        .sources()
        .map(|source| (source.word, source.text))
        .collect();
    assert_eq!(
        sources,
        [(
            "diff",
            "```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n```\n"
        )]
    );
}

#[test]
fn a_redirected_run_is_written_a_fence_as_its_drawer_lays_it_out() {
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.wears(colourful());
    render.drawers(echoing());
    render.stream(ANSWER).unwrap();
    render.settle().unwrap();

    let written = render.terminal.written();
    assert_eq!(
        written,
        "Before.\n\n» --- a/x.rs\n» +++ b/x.rs\n» @@ -1 +1 @@\n» -old\n» +new\n\nAfter.\n"
    );
}

/// Fences whose paths and lines carry what a terminal would obey: an escape
/// that colours, one that clears the screen, a hyperlink, a bell and a
/// backspace, the controls of the second set, an override, an isolate, a
/// zero width space and a byte order mark.
pub(in crate::render) const HOSTILE_FENCES: [&str; 3] = [
    "```diff\n--- a/\u{1b}[31mred\u{1b}[0m.txt\n+++ b/\u{1b}[31mred\u{1b}[0m\u{8}\u{8}.txt\n@@ -1 +1 @@\n-a\n+b\n```\n",
    "```diff\n--- a/x.txt\n+++ b/x.txt\n@@ -1,3 +1,3 @@\n an escape \u{1b}[2J that clears the screen\n-a link \u{1b}]8;;http://127.0.0.1:9/\u{7}here\u{1b}]8;;\u{7} and a bell \u{7}\n+an override \u{202e}txet a\u{202c} and an isolate \u{2066}x\u{2069}\n a zero width space\u{200b} and a byte order mark\u{feff}\n```\n",
    "```patch\n--- a/\u{9b}31mx\u{85}.rs\n+++ b/\u{202e}sr.x\u{9c}\n@@ -1 +1 @@\n-a \u{1b}Pq\u{1b}\\ string and \u{1b}_Gi=1;AAAA\u{1b}\\ a picture\n+b\u{9b}2J\n```\n",
];

/// What none of [`HOSTILE_FENCES`] may put on a terminal.
pub(in crate::render) const OBEYED: [&str; 14] = [
    "\u{1b}]",
    "\u{1b}P",
    "\u{1b}_",
    "\u{1b}[31",
    "\u{1b}[2J",
    "\u{7}",
    "\u{8}",
    "\u{85}",
    "\u{9b}",
    "\u{9c}",
    "\u{202e}",
    "\u{2066}",
    "\u{200b}",
    "\u{feff}",
];

#[test]
fn a_hostile_path_or_line_in_a_drawn_fence_writes_no_escape() {
    for answer in HOSTILE_FENCES {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let mut drawn = Drawn::new(64, 12);
            drawn.wears(colourful().addressing(true));
            drawn.render.images(kitty_in(9, 18));
            drawn.render.draws(glyphs);
            drawn.render.drawers(echoing());
            drawn.answer(answer);

            // The drawer drew it, so what it was handed is what is on screen.
            let rows = drawn.rows();
            assert!(
                rows.iter().any(|row| row.contains("@@ -1")),
                "{answer:?} with {glyphs:?}: {rows:#?}"
            );
            assert!(
                !rows
                    .iter()
                    .any(|row| row.contains("diff") || row.contains("patch")),
                "{answer:?} with {glyphs:?}: {rows:#?}"
            );

            let written = drawn.render.terminal.written();
            for hostile in OBEYED {
                assert!(
                    !written.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
        }
    }
}

#[test]
fn a_hostile_fence_drawn_into_a_file_writes_no_escape_at_all() {
    for answer in HOSTILE_FENCES {
        let mut render = Renderer::new(Recording::redirected(80, 24));
        render.wears(colourful());
        render.drawers(echoing());
        render.stream(answer).unwrap();
        render.settle().unwrap();

        let written = render.terminal.written();
        // The drawer drew it: its mark is on the lines, and no frame is.
        assert!(written.contains("» @@ -1"), "{written:?}");
        assert!(!written.contains("┌─"), "{written:?}");
        assert!(
            !written
                .chars()
                .any(|character| character.is_control() && character != '\n'),
            "{written:?}"
        );
        for hostile in OBEYED {
            assert!(!written.contains(hostile), "{hostile:?} in {written:?}");
        }
    }
}
