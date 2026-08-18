use crate::color::Palette;
use crate::dump::dump;

use super::*;

/// The three answers a question about a language offers.
fn languages() -> [Choice<'static>; 3] {
    [
        Choice {
            answer: "Rust",
            says: "crucible's own implementation language",
            chosen: None,
            shows: &[],
        },
        Choice {
            answer: "Python",
            says: "the common scripting choice",
            chosen: None,
            shows: &[],
        },
        Choice {
            answer: "Something else",
            says: "",
            chosen: None,
            shows: &[],
        },
    ]
}

/// The same, where several may be chosen and two are.
fn supported() -> [Choice<'static>; 3] {
    [
        Choice {
            answer: "Reading images",
            says: "a vision-capable model is sent the image",
            chosen: Some(true),
            shows: &[],
        },
        Choice {
            answer: "Pulling text out of PDFs",
            says: "the model is sent the text, not the file",
            chosen: Some(true),
            shows: &[],
        },
        Choice {
            answer: "None of these",
            says: "keep the tool surface small",
            chosen: Some(false),
            shows: &[],
        },
    ]
}

fn stops() -> [Stop<'static>; 4] {
    [
        Stop {
            name: "Language",
            done: true,
            asks: true,
        },
        Stop {
            name: "Support",
            done: false,
            asks: true,
        },
        Stop {
            name: "Status line",
            done: false,
            asks: true,
        },
        Stop {
            name: "Review",
            done: false,
            asks: false,
        },
    ]
}

/// A panel about the language question, with the mark on the first answer.
fn asked<'a>(answers: &'a [Choice<'a>], stops: &'a [Stop<'a>]) -> Asked<'a> {
    Asked {
        subject: "Questions for you",
        stops,
        at: 0,
        statement: "",
        given: &[],
        question: "Which language should the examples be written in?",
        answers,
        marked: 0,
        note: "",
        leaves: "Say it in the prompt instead",
        footer: "esc to cancel · ←→ between questions · n for a note",
    }
}

/// What the panel says, row by row, with no colour in it.
fn art(asked: &Asked<'_>, columns: usize, room: usize) -> Vec<String> {
    asked
        .within(columns, room, Glyphs::Unicode)
        .0
        .iter()
        .map(Row::text)
        .collect()
}

/// A palette that writes every hue it has.
fn colourful() -> Palette {
    Palette::resolve(true, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[test]
fn the_panel_names_whose_the_questions_are_to_answer_and_then_asks_one() {
    let answers = languages();
    let stops = stops();
    let drawn = art(&asked(&answers, &stops), 80, 30);

    assert!(
        drawn.iter().any(|row| row.contains("Questions for you")),
        "{drawn:#?}"
    );
    assert!(
        drawn
            .iter()
            .any(|row| row.contains("Which language should the examples be written in?")),
        "{drawn:#?}"
    );
    assert!(
        drawn.iter().any(|row| row.contains("1. Rust")),
        "{drawn:#?}"
    );
    assert!(
        drawn
            .iter()
            .any(|row| row.contains("crucible's own implementation language")),
        "{drawn:#?}"
    );
    assert!(
        drawn
            .iter()
            .any(|row| row.contains("4. Say it in the prompt instead")),
        "the answer that leaves is numbered after the question's own: {drawn:#?}"
    );
}

#[test]
fn the_marked_answer_is_marked_as_well_as_coloured() {
    // A terminal with no colour still has to report which row a key acts on,
    // which is the one thing a hue may never carry alone.
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 1;

    let drawn = art(&panel, 80, 30);
    let marked = drawn
        .iter()
        .find(|row| row.contains("2. Python"))
        .expect("the row the mark is on");
    let passed = drawn
        .iter()
        .find(|row| row.contains("1. Rust"))
        .expect("a row it has passed over");

    assert!(marked.contains('›'), "{marked:?}");
    assert!(!passed.contains('›'), "{passed:?}");
}

#[test]
fn a_question_taking_several_answers_says_which_are_chosen() {
    let answers = supported();
    let stops = stops();
    let drawn = art(&asked(&answers, &stops), 80, 30);

    let chosen = drawn
        .iter()
        .find(|row| row.contains("Reading images"))
        .expect("a chosen answer");
    let not = drawn
        .iter()
        .find(|row| row.contains("None of these"))
        .expect("an answer nobody chose");

    assert!(chosen.contains('✓'), "{chosen:?}");
    assert!(not.contains('□'), "{not:?}");
}

#[test]
fn a_question_taking_one_answer_draws_no_boxes_at_all() {
    // A box beside a single answer offers a choice that is not being made.
    let answers = languages();
    let stops = stops();
    let drawn = art(&asked(&answers, &stops), 80, 30);

    let numbered: Vec<&String> = drawn
        .iter()
        .filter(|row| row.contains("1. ") || row.contains("2. ") || row.contains("3. "))
        .collect();

    assert!(!numbered.is_empty(), "{drawn:#?}");
    assert!(
        numbered.iter().all(|row| !row.contains('□')),
        "a box beside a single answer offers a choice nobody is making: {numbered:#?}"
    );
}

#[test]
fn a_question_already_answered_wears_the_green_a_finished_task_wears() {
    let answers = languages();
    let stops = stops();
    let painted: Vec<String> = asked(&answers, &stops)
        .within(80, 30, Glyphs::Unicode)
        .0
        .iter()
        .map(|row| row.paint(colourful()))
        .collect();

    let row = painted
        .iter()
        .find(|row| row.contains("Language"))
        .expect("the questions row");

    assert!(row.contains(colourful().open(Slot::DoneMark)), "{row:?}");
    assert!(row.contains(colourful().open(Slot::Accent)), "{row:?}");
}

#[test]
fn one_question_has_no_row_to_step_along() {
    let answers = languages();
    let alone = [Stop {
        name: "Language",
        done: false,
        asks: true,
    }];
    let drawn = art(&asked(&answers, &alone), 80, 30);

    assert!(
        !drawn.iter().any(|row| row.contains("question 1 of 1")),
        "{drawn:#?}"
    );
    assert!(
        drawn.iter().any(|row| row.contains("1. Rust")),
        "{drawn:#?}"
    );
}

#[test]
fn a_row_of_names_too_wide_to_mean_anything_becomes_a_count_instead() {
    // The one thing here that gives way to width rather than to height. Cut,
    // a row of names says less than the two facts a count says whole.
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at = 1;

    let drawn = art(&panel, 40, 30);

    assert!(
        drawn
            .iter()
            .any(|row| row.contains("question 2 of 3 · 1 answered")),
        "{drawn:#?}"
    );
    assert!(
        !drawn.iter().any(|row| row.contains("Status line")),
        "{drawn:#?}"
    );
}

#[test]
fn no_row_is_ever_wider_than_the_window() {
    // Every row of a live region is one row. A row that wrapped would leave the
    // cursor below where the next frame expects it, and the frame after that
    // would erase the wrong lines.
    let answers = languages();
    let several = supported();
    let stops = stops();

    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        for columns in 1..=200 {
            for set in [&answers, &several] {
                let (rows, _) = asked(set, &stops).within(columns, 40, glyphs);

                assert!(
                    rows.iter().all(|row| row.columns() <= columns),
                    "at {columns} in {glyphs:?}: {rows:?}"
                );
            }
        }
    }
}

#[test]
fn a_window_too_short_returns_no_rows_at_all() {
    let answers = languages();
    let stops = stops();

    for room in 0..=4 {
        let (rows, caret) = asked(&answers, &stops).within(80, room, Glyphs::Unicode);

        assert!(rows.is_empty(), "at {room}: {rows:?}");
        assert!(caret.is_none(), "at {room}");
    }
}

#[test]
fn the_footer_is_the_first_thing_given_up_and_the_blanks_are_the_last() {
    let answers = languages();
    let stops = stops();
    let panel = asked(&answers, &stops);

    let whole = art(&panel, 80, 40);
    let footer = whole.last().expect("a panel with a footer");
    assert!(footer.contains("esc to cancel"), "{footer:?}");

    // One row short of the whole panel: the footer goes and everything the
    // reader is deciding with stays.
    let shorter = art(&panel, 80, whole.len() - 1);
    assert!(!shorter.is_empty());
    assert!(
        !shorter.iter().any(|row| row.contains("esc to cancel")),
        "{shorter:#?}"
    );
    assert!(
        shorter
            .iter()
            .any(|row| row.contains("crucible's own implementation language")),
        "the descriptions outlive the footer: {shorter:#?}"
    );
}

#[test]
fn the_stop_that_sends_reads_every_answer_back_before_it_goes() {
    let send = [
        Choice {
            answer: "Send",
            says: "",
            chosen: None,
            shows: &[],
        },
        Choice {
            answer: "Cancel",
            says: "",
            chosen: None,
            shows: &[],
        },
    ];
    let given = [
        Given {
            question: "Which language should the examples be written in?",
            answer: "Rust",
        },
        Given {
            question: "Which of these should crucible support later?",
            answer: "Reading images, Pulling text out of PDFs",
        },
    ];
    let stops = stops();
    let panel = Asked {
        statement: "These are the answers that go back:",
        given: &given,
        question: "Send them?",
        answers: &send,
        leaves: "",
        at: 3,
        ..asked(&send, &stops)
    };

    let drawn = art(&panel, 80, 30);

    assert!(
        drawn
            .iter()
            .any(|row| row.contains("These are the answers that go back:")),
        "{drawn:#?}"
    );
    assert!(drawn.iter().any(|row| row.contains("Rust")), "{drawn:#?}");
    assert!(
        drawn.iter().any(|row| row.contains("Send them?")),
        "{drawn:#?}"
    );
    assert!(
        drawn.iter().any(|row| row.contains("1. Send")),
        "{drawn:#?}"
    );
    assert!(
        !drawn
            .iter()
            .any(|row| row.contains("Say it in the prompt instead")),
        "there is nothing left to leave from here: {drawn:#?}"
    );
}

#[test]
fn the_whole_panel_at_eighty_columns() {
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at = 0;

    insta::assert_snapshot!(dump(&panel.within(80, 30, Glyphs::Unicode).0, 80));
}

#[test]
fn the_whole_panel_where_several_may_be_chosen() {
    let answers = supported();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at = 1;
    panel.question = "Which of these should crucible support later?";
    panel.footer = "esc to cancel · space to choose · ←→ between questions · n for a note";

    insta::assert_snapshot!(dump(&panel.within(80, 30, Glyphs::Unicode).0, 80));
}

#[test]
fn the_whole_panel_with_no_box_drawing_to_hand() {
    let answers = languages();
    let stops = stops();

    insta::assert_snapshot!(dump(
        &asked(&answers, &stops).within(80, 30, Glyphs::Ascii).0,
        80
    ));
}
