use crucible_core::Answer;

use super::*;

/// Two questions: one taking a single answer, one taking several.
fn asked() -> Vec<Question> {
    vec![
        Question::new(
            "Language",
            "Which language?",
            [Answer::new("Rust"), Answer::new("Python")],
        ),
        Question::new(
            "Support",
            "Which of these?",
            [Answer::new("Images"), Answer::new("PDFs")],
        )
        .several(),
    ]
}

/// One question on its own.
fn alone() -> Vec<Question> {
    vec![Question::new(
        "Language",
        "Which language?",
        [Answer::new("Rust"), Answer::new("Python")],
    )]
}

/// A key, pressed.
fn key(typed: Key) -> Pressed {
    Pressed::Key(typed)
}

#[test]
fn the_mark_stops_at_each_end_of_both_axes_rather_than_wrapping() {
    // A ring puts the first thing one key past the last, so the key that went
    // too far becomes the key that goes further.
    let questions = asked();
    let mut standing = Standing::new(&questions);

    assert_eq!(moving(Pressed::Up, &mut standing, &questions), Moved::Still);
    assert_eq!(standing.marked(), 0);

    // Two answers and the written one under them.
    for _ in 0..3 {
        moving(Pressed::Down, &mut standing, &questions);
    }
    assert_eq!(standing.marked(), 2);
    assert_eq!(
        moving(Pressed::Down, &mut standing, &questions),
        Moved::Still
    );

    assert_eq!(
        moving(key(Key::Left), &mut standing, &questions),
        Moved::Still
    );
    assert_eq!(standing.at, 0);

    // Two questions and the stop that sends.
    for _ in 0..2 {
        moving(key(Key::Right), &mut standing, &questions);
    }
    assert_eq!(standing.at, 2);
    assert_eq!(
        moving(key(Key::Right), &mut standing, &questions),
        Moved::Still
    );
}

#[test]
fn a_digit_naming_no_answer_moves_nothing_and_takes_nothing() {
    let questions = alone();
    let mut standing = Standing::new(&questions);

    for typed in ['0', '9', 'q'] {
        assert_eq!(
            moving(key(Key::Char(typed)), &mut standing, &questions),
            Moved::Still,
            "{typed}"
        );
        assert_eq!(standing.marked(), 0);
    }
}

#[test]
fn the_number_under_the_rule_leaves_the_whole_ask() {
    // Two answers, the written one, then the row under the rule — which is what
    // escape means as well.
    let questions = alone();
    let mut standing = Standing::new(&questions);

    assert_eq!(
        moving(key(Key::Char('4')), &mut standing, &questions),
        Moved::Left
    );
}

#[test]
fn a_question_already_answered_is_found_as_it_was_left() {
    // Going back to check is the reason the arrows go both ways, and a question
    // that had forgotten the answer would make that pointless.
    let questions = asked();
    let mut standing = Standing::new(&questions);

    moving(Pressed::Down, &mut standing, &questions);
    assert_eq!(standing.marked(), 1);

    moving(key(Key::Right), &mut standing, &questions);
    assert_eq!(standing.at, 1);

    moving(key(Key::Left), &mut standing, &questions);
    assert_eq!(standing.at, 0);
    assert_eq!(standing.marked(), 1);
}

#[test]
fn choosing_and_unchoosing_only_happens_where_several_may_be_chosen() {
    let questions = asked();
    let mut standing = Standing::new(&questions);

    // The first question takes one answer, so space is a key that does nothing.
    assert_eq!(
        moving(key(Key::Char(' ')), &mut standing, &questions),
        Moved::Still
    );

    standing.at = 1;
    assert_eq!(
        moving(key(Key::Char(' ')), &mut standing, &questions),
        Moved::Redraw
    );
    let chosen = standing.held.get(1).map(|held| held.chosen.clone());
    assert_eq!(chosen, Some(vec![true, false]));

    moving(key(Key::Char(' ')), &mut standing, &questions);
    let chosen = standing.held.get(1).map(|held| held.chosen.clone());
    assert_eq!(chosen, Some(vec![false, false]));
}

#[test]
fn escape_while_writing_stops_the_writing_and_keeps_the_line() {
    // Only the whole ask is what a second escape leaves.
    let questions = alone();
    let mut standing = Standing::new(&questions);
    standing.writing = Some(Writer::Wrote);

    for typed in "Zig".chars() {
        moving(key(Key::Char(typed)), &mut standing, &questions);
    }

    assert_eq!(
        moving(Pressed::Escape, &mut standing, &questions),
        Moved::Redraw
    );
    assert!(standing.writing.is_none());
    assert_eq!(
        standing
            .held
            .first()
            .map(|held| held.wrote.text().to_owned()),
        Some("Zig".to_owned())
    );

    assert_eq!(
        moving(Pressed::Escape, &mut standing, &questions),
        Moved::Left
    );
}

#[test]
fn landing_on_the_answer_nobody_offered_opens_it_rather_than_taking_it() {
    let questions = alone();
    let mut standing = Standing::new(&questions);

    moving(Pressed::Down, &mut standing, &questions);
    moving(Pressed::Down, &mut standing, &questions);
    assert_eq!(standing.marked(), 2);

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Redraw
    );
    assert_eq!(standing.writing, Some(Writer::Wrote));
}

#[test]
fn an_ask_of_one_question_has_no_last_stop_and_enter_sends_it() {
    // A screen reading back one answer says what the screen before it said.
    let questions = alone();
    let mut standing = Standing::new(&questions);

    assert!(!standing.reviews());
    assert_eq!(standing.stops(), 1);
    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Took
    );
}

#[test]
fn an_ask_of_several_reads_them_back_before_it_sends() {
    let questions = asked();
    let mut standing = Standing::new(&questions);

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Redraw
    );
    assert_eq!(standing.at, 1);

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Redraw
    );
    assert!(standing.sending());

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Took
    );
}

#[test]
fn cancel_on_the_last_stop_leaves_the_ask() {
    let questions = asked();
    let mut standing = Standing::new(&questions);
    standing.at = 2;
    standing.sending = 1;

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Left
    );
}

#[test]
fn every_way_out_but_enter_leaves_the_ask_unanswered() {
    let questions = alone();

    for arrived in [
        Pressed::Escape,
        key(Key::Interrupt),
        key(Key::Eof),
        key(Key::Char('4')),
    ] {
        let mut standing = Standing::new(&questions);
        assert_eq!(
            moving(arrived.clone(), &mut standing, &questions),
            Moved::Left,
            "{arrived:?}"
        );
    }
}

#[test]
fn the_footer_names_a_key_only_where_it_does_something() {
    let questions = asked();
    let mut standing = Standing::new(&questions);

    assert_eq!(footer(&standing, false), ONE);
    assert_eq!(footer(&standing, true), MANY);

    standing.writing = Some(Writer::Note);
    assert_eq!(footer(&standing, true), WRITING);

    let alone = alone();
    let standing = Standing::new(&alone);
    assert_eq!(footer(&standing, false), ONLY);
}

#[test]
fn what_comes_back_is_what_was_chosen_and_the_line_beside_it() {
    let questions = asked();
    let mut standing = Standing::new(&questions);

    moving(Pressed::Down, &mut standing, &questions);
    moving(key(Key::Enter), &mut standing, &questions);
    moving(key(Key::Char(' ')), &mut standing, &questions);

    standing.writing = Some(Writer::Note);
    for typed in "later".chars() {
        moving(key(Key::Char(typed)), &mut standing, &questions);
    }

    let given: Vec<Answered> = standing
        .held
        .iter()
        .zip(&questions)
        .map(|(held, question)| held.answered(question))
        .collect();

    let first = given.first().expect("the first question");
    assert_eq!(first.chosen().collect::<Vec<_>>(), ["Python"]);

    let second = given.get(1).expect("the second question");
    assert_eq!(second.chosen().collect::<Vec<_>>(), ["Images"]);
    assert_eq!(second.note(), "later");
}

#[test]
fn the_written_answer_is_what_comes_back_where_it_is_the_one_taken() {
    let questions = alone();
    let mut standing = Standing::new(&questions);

    moving(Pressed::Down, &mut standing, &questions);
    moving(Pressed::Down, &mut standing, &questions);
    moving(key(Key::Enter), &mut standing, &questions);
    for typed in "Zig".chars() {
        moving(key(Key::Char(typed)), &mut standing, &questions);
    }

    let held = standing.held.first().expect("the only question");
    let question = questions.first().expect("the only question");

    assert_eq!(
        held.answered(question).chosen().collect::<Vec<_>>(),
        ["Zig"]
    );
}

#[test]
fn the_written_answer_row_reads_back_what_was_written() {
    // "Something else" is the offer, not the answer. Once a line has been
    // written, a row still saying the offer reads as the line having been
    // dropped — the one place the reader can check what they wrote is the row
    // that will be taken.
    let questions = alone();
    let mut standing = Standing::new(&questions);

    moving(Pressed::Down, &mut standing, &questions);
    moving(Pressed::Down, &mut standing, &questions);
    moving(key(Key::Enter), &mut standing, &questions);
    for typed in "Zig".chars() {
        moving(key(Key::Char(typed)), &mut standing, &questions);
    }
    moving(key(Key::Enter), &mut standing, &questions);

    let (rows, _) = drawn(&mut standing, &questions, 80, 30, Style::plain());
    let art: Vec<String> = rows.iter().map(crucible_tui::Row::text).collect();

    assert!(art.iter().any(|row| row.contains("Zig")), "{art:#?}");
    assert!(!art.iter().any(|row| row.contains(ELSE)), "{art:#?}");
}

#[test]
fn a_specimen_reaches_the_panel_that_draws_it() {
    // The defect this catches: every answer was handed to the panel with an
    // empty specimen, so a question whose whole point was showing what an
    // answer looks like drew nothing under it. The panel had tests and the
    // wiring between them did not, which is exactly where it hid.
    let questions = vec![Question::new(
        "Status",
        "Which status line?",
        [
            Answer::new("Compact").showing(["› ...", "", "crucible · main"]),
            Answer::new("Nothing at all"),
        ],
    )];
    let mut standing = Standing::new(&questions);

    let (rows, _) = drawn(&mut standing, &questions, 80, 30, Style::plain());
    let art: Vec<String> = rows.iter().map(crucible_tui::Row::text).collect();

    assert!(
        art.iter().any(|row| row.contains("crucible · main")),
        "the specimen never reached the panel: {art:#?}"
    );

    // And the answer beside it, which has none, still gets the box.
    standing.held.first_mut().expect("the question").marked = 1;
    let (rows, _) = drawn(&mut standing, &questions, 80, 30, Style::plain());
    let art: Vec<String> = rows.iter().map(crucible_tui::Row::text).collect();

    assert!(
        art.iter()
            .any(|row| row.contains("nothing to show for this one")),
        "{art:#?}"
    );
}

#[test]
fn a_question_taking_several_answers_reaches_the_panel_as_one() {
    // The other half of the same hole: `several` decides both the boxes beside
    // the answers and the key the footer names, so a question that lost it drew
    // a single-choice panel under a question asking for any number.
    let questions = vec![
        Question::new("One", "Which?", [Answer::new("a"), Answer::new("b")]),
        Question::new(
            "Any",
            "Which of these?",
            [Answer::new("a"), Answer::new("b")],
        )
        .several(),
    ];
    let mut standing = Standing::new(&questions);

    // The questions row carries a mark of its own for a question nobody has
    // answered. What is read here is the numbered rows, and the mark on those is
    // bracketed precisely so the two are never mistaken for each other.
    let numbered = |art: &[String]| -> usize {
        art.iter()
            .filter(|row| row.contains(" 1. ") || row.contains(" 2. "))
            .filter(|row| row.contains("[ ]") || row.contains("[✓]"))
            .count()
    };

    let (rows, _) = drawn(&mut standing, &questions, 80, 30, Style::plain());
    let art: Vec<String> = rows.iter().map(crucible_tui::Row::text).collect();
    assert_eq!(numbered(&art), 0, "{art:#?}");

    standing.at = 1;
    let (rows, _) = drawn(&mut standing, &questions, 80, 30, Style::plain());
    let art: Vec<String> = rows.iter().map(crucible_tui::Row::text).collect();

    assert_eq!(
        numbered(&art),
        2,
        "the boxes never reached the answers: {art:#?}"
    );
    assert!(
        art.iter().any(|row| row.contains("space to choose")),
        "the footer never named the key: {art:#?}"
    );
}

#[test]
fn one_question_taking_several_answers_still_reads_them_back() {
    // Where several may be chosen, enter would otherwise mean both "choose this
    // one" and "I am done", and a key that means two things does the wrong one.
    let questions = vec![
        Question::new(
            "Any",
            "Which of these?",
            [Answer::new("a"), Answer::new("b")],
        )
        .several(),
    ];
    let mut standing = Standing::new(&questions);

    assert!(
        standing.reviews(),
        "a single multi-answer question had no last stop"
    );

    // Space chooses without moving on; enter is what reaches the last stop.
    assert_eq!(
        moving(key(Key::Char(' ')), &mut standing, &questions),
        Moved::Redraw
    );
    assert!(!standing.sending());

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Redraw
    );
    assert!(standing.sending());

    assert_eq!(
        moving(key(Key::Enter), &mut standing, &questions),
        Moved::Took
    );
}
