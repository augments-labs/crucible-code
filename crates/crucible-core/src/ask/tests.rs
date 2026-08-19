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
fn what_answers_a_question_is_reached_as_a_trait_and_may_answer_nobody() {
    struct Nobody;
    impl Put for Nobody {
        fn put(&self, _questions: &[Question]) -> Option<Vec<Answered>> {
            None
        }
    }

    struct Always(&'static str);
    impl Put for Always {
        fn put(&self, questions: &[Question]) -> Option<Vec<Answered>> {
            Some(questions.iter().map(|_| Answered::new([self.0])).collect())
        }
    }

    let asked = [
        Question::new("One", "Which?", [Answer::new("Rust")]),
        Question::new("Two", "And?", [Answer::new("Python")]),
    ];

    let nobody: &dyn Put = &Nobody;
    assert!(nobody.put(&asked).is_none());

    let always: &dyn Put = &Always("Rust");
    let given = always.put(&asked).expect("an answer to every question");
    assert_eq!(given.len(), 2);
    assert_eq!(
        given
            .iter()
            .map(|one| one.chosen().collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        [["Rust"], ["Rust"]]
    );
}
