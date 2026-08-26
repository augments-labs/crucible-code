use crate::color::Palette;
use crate::color::Theme;
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
        writing: None,
        at_note: false,
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
    Palette::resolve(true, Theme::Dark, None, &|name| {
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

    assert!(chosen.contains("[✓]"), "{chosen:?}");
    assert!(not.contains("[ ]"), "{not:?}");

    // And never the mark the row of questions above it uses, which says a
    // different thing about a different list.
    assert!(!chosen.contains('□'), "{chosen:?}");
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
        numbered.iter().all(|row| !row.contains('[')),
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
        .map(|row| row.paint(&colourful()))
        .collect();

    let row = painted
        .iter()
        .find(|row| row.contains("Language"))
        .expect("the questions row");

    assert!(
        row.contains(colourful().open(Slot::DoneMark).as_str()),
        "{row:?}"
    );
    assert!(
        row.contains(colourful().open(Slot::Accent).as_str()),
        "{row:?}"
    );
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
    // Every row a component answers with is one row of the window. A row that
    // wrapped would take a second, and the second comes out of the band drawn
    // under this one.
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

/// A question whose answers are shapes: two with a specimen, one without, and
/// the specimens deliberately different in width and in height.
fn shapes() -> [Choice<'static>; 3] {
    [
        Choice {
            answer: "Compact",
            says: "",
            chosen: None,
            shows: &["› ...", "", "crucible · opus-5 · main"],
        },
        Choice {
            answer: "With the workspace and the spend",
            says: "",
            chosen: None,
            shows: &["crucible · opus-5 · main* · ~/src/crucible · 12.4k"],
        },
        Choice {
            answer: "Nothing at all",
            says: "",
            chosen: None,
            shows: &[],
        },
    ]
}

#[test]
fn the_panel_is_the_same_height_whichever_answer_is_marked() {
    // The block moves with the mark, so it is drawn at the size of the widest
    // and tallest specimen in the question rather than of the one on screen.
    // Otherwise every arrow would change the height of the panel under it.
    let answers = shapes();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.question = "You have no status line. Which one shall I set up here?";

    let mut heights = Vec::new();
    let mut widths = Vec::new();
    for marked in 0..answers.len() {
        panel.marked = marked;
        let rows = panel.within(80, 40, Glyphs::Unicode).0;

        heights.push(rows.len());
        widths.push(
            rows.iter()
                .filter(|row| row.text().contains('┌') || row.text().contains('└'))
                .map(Row::columns)
                .collect::<Vec<_>>(),
        );
    }

    let one_height = heights
        .first()
        .is_some_and(|first| heights.iter().all(|height| height == first));
    let one_width = widths
        .first()
        .is_some_and(|first| widths.iter().all(|width| width == first));

    assert!(
        one_height,
        "the panel changed height as the mark moved: {heights:?}"
    );
    assert!(
        one_width,
        "the block changed width as the mark moved: {widths:?}"
    );
}

#[test]
fn an_answer_with_nothing_to_show_still_draws_the_box_and_says_so() {
    // A block that vanished would move every row under it, which is the same
    // defect as the panel changing height.
    let answers = shapes();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 2;

    let drawn = art(&panel, 80, 40);

    assert!(drawn.iter().any(|row| row.contains('┌')), "{drawn:#?}");
    assert!(
        drawn
            .iter()
            .any(|row| row.contains("nothing to show for this one")),
        "{drawn:#?}"
    );
}

#[test]
fn a_specimen_past_the_bound_is_cut_and_the_row_says_by_how_much() {
    // Every tool here bounds what it hands back and says when it cut it. A
    // block that could be any height would let whatever wrote the call decide
    // how tall this panel is.
    let many: Vec<String> = (1..=25).map(|at| format!("row {at}")).collect();
    let rows: Vec<&str> = many.iter().map(String::as_str).collect();
    let answers = [Choice {
        answer: "Long",
        says: "",
        chosen: None,
        shows: &rows,
    }];
    let stops = stops();
    let panel = asked(&answers, &stops);

    let drawn = art(&panel, 80, 60);

    assert!(drawn.iter().any(|row| row.contains("row 1")), "{drawn:#?}");
    assert!(
        !drawn.iter().any(|row| row.contains("row 25")),
        "the block ran past its bound: {drawn:#?}"
    );
    assert!(
        drawn.iter().any(|row| row.contains("more rows")),
        "it was cut and never said so: {drawn:#?}"
    );
}

#[test]
fn a_specimen_stands_beside_the_answers_where_both_fit_whole() {
    // The reader is choosing between shapes, so the shape belongs beside the
    // choices rather than under them — but only where neither gives ground:
    // every answer unfolded and the widest specimen uncut. Narrower than
    // that, the box keeps to the rows under the answers, where the full
    // width keeps most of the picture.
    let answers = shapes();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 1;

    let drawn = art(&panel, 100, 40);
    assert!(
        drawn
            .iter()
            .any(|row| row.contains("Compact") && row.contains('┌')),
        "{drawn:#?}"
    );

    let drawn = art(&panel, 80, 40);
    assert!(
        !drawn
            .iter()
            .any(|row| row.contains("Compact") && row.contains('┌')),
        "the box squeezed in where it cannot stand whole: {drawn:#?}"
    );
    assert!(drawn.iter().any(|row| row.contains('┌')), "{drawn:#?}");
}

#[test]
fn the_whole_panel_with_a_specimen_beside_the_answers() {
    let answers = shapes();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at = 2;
    panel.question = "You have no status line. Which one shall I set up here?";
    panel.marked = 1;

    insta::assert_snapshot!(dump(&panel.within(100, 40, Glyphs::Unicode).0, 100));
}

#[test]
fn a_question_with_no_specimens_draws_no_block() {
    let answers = languages();
    let stops = stops();
    let drawn = art(&asked(&answers, &stops), 80, 40);

    assert!(
        !drawn.iter().any(|row| row.contains('┌')),
        "a block was drawn for a question that shows nothing: {drawn:#?}"
    );
}

#[test]
fn a_specimen_is_clipped_rather_than_folded() {
    // A folded specimen is a picture of something else. Cut, it is at least a
    // picture of the first columns of the right thing.
    let wide = "x".repeat(200);
    let rows = [wide.as_str()];
    let answers = [Choice {
        answer: "Wide",
        says: "",
        chosen: None,
        shows: &rows,
    }];
    let stops = stops();

    let drawn = art(&asked(&answers, &stops), 80, 40);

    assert!(drawn.iter().all(|row| row.chars().count() <= 80));
    assert_eq!(
        drawn.iter().filter(|row| row.contains("xxx")).count(),
        1,
        "the specimen folded onto more rows than it was given: {drawn:#?}"
    );
}

#[test]
fn the_whole_panel_with_a_specimen_under_the_marked_answer() {
    let answers = shapes();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at = 2;
    panel.question = "You have no status line. Which one shall I set up here?";
    panel.marked = 1;

    insta::assert_snapshot!(dump(&panel.within(80, 40, Glyphs::Unicode).0, 80));
}

#[test]
fn the_whole_panel_on_an_answer_with_nothing_to_show() {
    let answers = shapes();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at = 2;
    panel.question = "You have no status line. Which one shall I set up here?";
    panel.marked = 2;

    insta::assert_snapshot!(dump(&panel.within(80, 40, Glyphs::Unicode).0, 80));
}

#[test]
fn an_empty_answer_being_written_shows_its_placeholder_and_parks_the_cursor_in_it() {
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 2;
    panel.writing = Some(Writing {
        text: "",
        column: 0,
        placeholder: "Something else",
    });

    let (rows, caret) = panel.within(80, 40, Glyphs::Unicode);
    let caret = caret.expect("the cursor belongs in the line being written");

    let row = rows.get(caret.row).expect("a row the panel drew");
    assert!(row.text().contains("Something else"), "{:?}", row.text());

    // The placeholder is not an answer's name, so it is drawn quiet — and the
    // cursor sits on its first column rather than after it.
    let quiet = row
        .paint(&colourful())
        .contains(colourful().open(Slot::Quiet).as_str());
    assert!(quiet, "{:?}", row.paint(&colourful()));
    assert_eq!(caret.column, 1 + 2 + 1 + " 3. ".len());
}

#[test]
fn the_cursor_lands_on_the_column_the_editor_says() {
    // Display columns, not characters: a CJK glyph is two columns wide, and a
    // cursor placed by counting characters would sit inside one.
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 2;
    panel.writing = Some(Writing {
        text: "日本語",
        column: 6,
        placeholder: "Something else",
    });

    let (rows, caret) = panel.within(80, 40, Glyphs::Unicode);
    let caret = caret.expect("the cursor belongs in the line being written");

    assert_eq!(caret.column, 1 + 2 + 1 + " 3. ".len() + 6);
    assert!(
        rows.get(caret.row)
            .is_some_and(|row| row.text().contains("日本語")),
        "{rows:?}"
    );
}

#[test]
fn a_note_is_drawn_only_once_there_is_one() {
    let answers = languages();
    let stops = stops();
    let bare = art(&asked(&answers, &stops), 80, 40);
    assert!(
        !bare.iter().any(|row| row.contains("Note:")),
        "a row was spent on an offer the footer already makes: {bare:#?}"
    );

    let mut panel = asked(&answers, &stops);
    panel.note = "the examples have to compile as they stand";
    let noted = art(&panel, 80, 40);

    assert!(
        noted
            .iter()
            .any(|row| row.contains("Note: the examples have to compile as they stand")),
        "{noted:#?}"
    );
}

#[test]
fn a_note_being_written_takes_the_cursor_rather_than_the_answer() {
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at_note = true;
    panel.writing = Some(Writing {
        text: "compile as they stand",
        column: 21,
        placeholder: "",
    });

    let (rows, caret) = panel.within(80, 40, Glyphs::Unicode);
    let caret = caret.expect("the cursor belongs in the note");
    let row = rows.get(caret.row).expect("a row the panel drew");

    assert!(
        row.text().contains("Note: compile as they stand"),
        "{row:?}"
    );
    assert_eq!(caret.column, 1 + 4 + "Note: ".len() + 21);
}

#[test]
fn a_line_past_what_the_row_has_room_for_slides_inside_it() {
    // The line is typed on one row, so past the room it slides rather than
    // folds: what is shown is the columns just behind the cursor — the reader
    // is watching what they type — and the caret never leaves the frame.
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 2;
    let written = format!("{}TAIL-END99", "x".repeat(50));
    panel.writing = Some(Writing {
        text: &written,
        column: 60,
        placeholder: "Something else",
    });

    let (rows, caret) = panel.within(40, 40, Glyphs::Unicode);
    let caret = caret.expect("the cursor belongs in the line being written");
    let row = rows.get(caret.row).expect("a row the panel drew");

    assert!(row.text().contains("TAIL-END99"), "{:?}", row.text());
    assert!(
        caret.column < 39,
        "the caret walked into the frame: {caret:?}"
    );
    assert_eq!(caret.column, 1 + 2 + 1 + " 3. ".len() + 30);
}

#[test]
fn a_note_past_what_the_row_has_room_for_slides_the_same_way() {
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.at_note = true;
    let written = format!("{}TAIL-END99", "x".repeat(50));
    panel.writing = Some(Writing {
        text: &written,
        column: 60,
        placeholder: "",
    });

    let (rows, caret) = panel.within(40, 40, Glyphs::Unicode);
    let caret = caret.expect("the cursor belongs in the note");
    let row = rows.get(caret.row).expect("a row the panel drew");

    assert!(row.text().contains("TAIL-END99"), "{:?}", row.text());
    assert!(
        caret.column < 39,
        "the caret walked into the frame: {caret:?}"
    );
    assert_eq!(caret.column, 1 + 4 + "Note: ".len() + 25);
}

#[test]
fn a_row_that_was_not_drawn_is_never_pointed_at() {
    // A caret on a row the ladder gave up would park the cursor somewhere the
    // frame never wrote, which is a caret the reader cannot find.
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.writing = Some(Writing {
        text: "Zig",
        column: 3,
        placeholder: "Something else",
    });
    panel.marked = 2;

    for room in 0..=4 {
        let (rows, caret) = panel.within(80, room, Glyphs::Unicode);
        assert!(rows.is_empty(), "at {room}");
        assert!(caret.is_none(), "at {room}");
    }

    // And wherever it does point, it points at a row that exists.
    for room in 5..=40 {
        let (rows, caret) = panel.within(80, room, Glyphs::Unicode);
        if let Some(caret) = caret {
            assert!(caret.row < rows.len(), "at {room}: {caret:?}");
        }
    }
}

#[test]
fn the_whole_panel_with_an_answer_being_written() {
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.marked = 2;
    panel.writing = Some(Writing {
        text: "Zig",
        column: 3,
        placeholder: "Something else",
    });
    panel.footer = "esc to stop typing · enter to keep it";

    insta::assert_snapshot!(dump(&panel.within(80, 40, Glyphs::Unicode).0, 80));
}

#[test]
fn the_whole_panel_with_a_note_on_the_question() {
    let answers = languages();
    let stops = stops();
    let mut panel = asked(&answers, &stops);
    panel.note = "the examples have to compile as they stand";

    insta::assert_snapshot!(dump(&panel.within(80, 40, Glyphs::Unicode).0, 80));
}

#[test]
fn nothing_a_call_wrote_can_move_the_cursor_or_set_an_attribute() {
    // The words on this panel are the model's, and a model reads files somebody
    // else may have written. A terminal reads an escape as an instruction, so
    // one carried through here could move the cursor out of the band this panel
    // was given or leave an attribute set for every row after it — either way
    // writing somewhere nothing asked it to.
    let hostile = "\x1b[2Jwiped\x1b[31m";
    let rows = "\x1b[Ainside".to_owned();
    let shows = [rows.as_str()];
    let answers = [
        Choice {
            answer: hostile,
            says: hostile,
            chosen: None,
            shows: &shows,
        },
        Choice {
            answer: "Plain",
            says: "",
            chosen: None,
            shows: &[],
        },
    ];
    let stops = [
        Stop {
            name: hostile,
            done: false,
            asks: true,
        },
        Stop {
            name: "Other",
            done: false,
            asks: true,
        },
    ];

    let panel = Asked {
        subject: hostile,
        stops: &stops,
        at: 0,
        statement: hostile,
        given: &[Given {
            question: hostile,
            answer: hostile,
        }],
        question: hostile,
        answers: &answers,
        marked: 0,
        note: hostile,
        writing: Some(Writing {
            text: hostile,
            column: 0,
            placeholder: hostile,
        }),
        at_note: true,
        leaves: hostile,
        footer: hostile,
    };

    let (drawn, _) = panel.within(80, 40, Glyphs::Unicode);

    for row in &drawn {
        let said = row.text();
        assert!(
            !said.contains('\x1b'),
            "an escape reached the terminal: {said:?}"
        );
        assert!(
            !said.chars().any(char::is_control),
            "a control character reached the terminal: {said:?}"
        );
    }
}
