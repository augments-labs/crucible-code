use super::*;

#[test]
fn a_question_reads_back_the_answers_it_was_given_in_the_order_it_was_given_them() {
    let question = Question::new(
        "Language",
        "Which language should the examples be written in?",
        [
            Answer::new("Rust").saying("crucible's own implementation language"),
            Answer::new("Python"),
        ],
    );

    assert_eq!(question.heading(), "Language");
    assert_eq!(
        question.question(),
        "Which language should the examples be written in?"
    );

    let answers: Vec<&str> = question.answers().map(Answer::answer).collect();
    assert_eq!(answers, ["Rust", "Python"]);

    let first = question.answers().next().expect("the answer just given");
    assert_eq!(first.says(), "crucible's own implementation language");
}

#[test]
fn an_answer_that_was_given_no_line_and_no_specimen_reads_back_empty_rather_than_absent() {
    // Both are the ordinary case: most answers are a name and nothing else, and
    // a caller drawing one asks the same question of every answer rather than
    // asking first whether there was anything to ask about.
    let bare = Answer::new("Nothing at all");

    assert_eq!(bare.says(), "");
    assert_eq!(bare.shows().len(), 0);
}

#[test]
fn an_answer_is_recommended_only_where_it_was_marked_and_its_name_stays_as_given() {
    let plain = Answer::new("Rust");
    assert!(!plain.is_recommended());

    let marked = Answer::new("Rust").saying("the one").recommending();
    assert!(marked.is_recommended());
    assert_eq!(marked.answer(), "Rust");
    assert_eq!(marked.says(), "the one");
}

#[test]
fn a_question_keeps_the_recommended_mark_on_its_first_answer_only() {
    let marks = |question: &Question| -> Vec<bool> {
        question.answers().map(Answer::is_recommended).collect()
    };

    let second = Question::new(
        "Language",
        "Which one?",
        [Answer::new("Rust"), Answer::new("Python").recommending()],
    );
    assert_eq!(marks(&second), [false, false]);

    let both = Question::new(
        "Language",
        "Which one?",
        [
            Answer::new("Rust").recommending(),
            Answer::new("Python").recommending(),
            Answer::new("Go").recommending(),
        ],
    );
    assert_eq!(marks(&both), [true, false, false]);
    assert_eq!(both.answers().nth(1).map(Answer::answer), Some("Python"));
}

#[test]
fn a_question_takes_one_answer_unless_it_was_asked_to_take_several() {
    let one = Question::new("Language", "Which one?", [Answer::new("Rust")]);
    assert!(!one.takes_several());

    let many = Question::new("Support", "Which of these?", [Answer::new("Images")]).several();
    assert!(many.takes_several());
}

#[test]
fn a_specimen_reads_back_row_for_row() {
    let answer = Answer::new("Compact").showing(["› ...", "", "crucible · opus-5 · main"]);

    assert_eq!(
        answer.shows().collect::<Vec<_>>(),
        ["› ...", "", "crucible · opus-5 · main"]
    );
}

#[test]
fn nothing_here_ever_shows_what_was_written_in_it() {
    // The questions are the model's own words and the answers are the reader's,
    // and both quote whatever the session is about. A value printed once by a
    // `{:?}` anywhere is a value in every log line and panic payload that
    // formats it.
    let question = Question::new(
        "heading-canary",
        "question-canary",
        [Answer::new("answer-canary")
            .saying("says-canary")
            .showing(["shows-canary"])],
    );

    let shown = format!("{question:?}");
    for canary in [
        "heading-canary",
        "question-canary",
        "answer-canary",
        "says-canary",
        "shows-canary",
    ] {
        assert!(!shown.contains(canary), "{shown}");
    }
    assert!(shown.contains("redacted"), "{shown}");

    let answer = Answer::new("answer-canary").saying("says-canary");
    let shown = format!("{answer:?}");
    assert!(!shown.contains("answer-canary"), "{shown}");
    assert!(!shown.contains("says-canary"), "{shown}");
    assert!(shown.contains("redacted"), "{shown}");
}

#[test]
fn an_answer_carries_the_line_the_reader_added_beside_it() {
    let answered = Answered::new(["Rust"]).noting("the examples have to compile as they stand");

    assert_eq!(answered.chosen().collect::<Vec<_>>(), ["Rust"]);
    assert_eq!(
        answered.note(),
        "the examples have to compile as they stand"
    );
}

#[test]
fn a_question_moved_on_from_with_nothing_chosen_is_answered_rather_than_unanswered() {
    // The difference this keeps: a question taking several answers and left
    // with none is a decision, and the review stop shows it as one. Nobody
    // answering at all is `None` from `Put`, which is a different thing.
    let none: [&str; 0] = [];
    let answered = Answered::new(none);

    assert_eq!(answered.chosen().len(), 0);
    assert_eq!(answered.note(), "");
}

#[test]
fn a_question_id_reads_back_from_the_text_it_is_written_as() {
    for number in [0, 1, 7, 42, u64::MAX] {
        let id = QuestionId::numbered(number);

        assert_eq!(id.number(), number);
        assert_eq!(QuestionId::parse(&id.to_string()), Some(id));
    }
    assert_eq!(QuestionId::numbered(3).to_string(), "q3");
}

#[test]
fn a_question_id_is_only_the_form_it_is_written_in() {
    for not_one in [
        "",
        "q",
        "3",
        "Q3",
        "q+3",
        "q-3",
        "q03",
        "q 3",
        " q3",
        "q3 ",
        "q3a",
        "q١",
        "q18446744073709551616",
    ] {
        assert_eq!(QuestionId::parse(not_one), None, "{not_one:?}");
    }
    assert_eq!(QuestionId::parse("q0"), Some(QuestionId::numbered(0)));
}

#[test]
fn a_batch_and_a_reply_never_show_what_was_asked_or_answered() {
    let batch = Batch::new(
        QuestionId::numbered(1),
        [Question::new(
            "heading-canary",
            "question-canary",
            [Answer::new("answer-canary").saying("says-canary")],
        )],
    );
    let reply = Reply::Answered(Answered::new(["chosen-canary"]).noting("note-canary"));

    let shown = format!("{batch:?} {reply:?} {:?}", Reply::Declined);
    for canary in [
        "heading-canary",
        "question-canary",
        "answer-canary",
        "says-canary",
        "chosen-canary",
        "note-canary",
    ] {
        assert!(!shown.contains(canary), "{shown}");
    }
    assert_eq!(batch.id(), QuestionId::numbered(1));
    assert_eq!(batch.questions().len(), 1);
}
