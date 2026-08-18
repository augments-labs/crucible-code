use std::sync::Mutex;

use crucible_core::Unwatched;

use crate::sample::allowed;

use super::*;

/// Whoever answers, and what they were shown when they did.
struct Whoever {
    answers: Vec<Answered>,
    saw: Mutex<Vec<String>>,
}

impl Whoever {
    fn saying(answers: Vec<Answered>) -> Self {
        Self {
            answers,
            saw: Mutex::new(Vec::new()),
        }
    }
}

impl Put for Whoever {
    fn put(&self, questions: &[Question]) -> Option<Vec<Answered>> {
        if let Ok(mut saw) = self.saw.lock() {
            saw.extend(questions.iter().map(|one| one.question().to_owned()));
        }
        Some(
            self.answers
                .iter()
                .map(|one| {
                    Answered::new(one.chosen().map(str::to_owned)).noting(one.note().to_owned())
                })
                .collect(),
        )
    }
}

/// Nobody there at all.
struct Nobody;

impl Put for Nobody {
    fn put(&self, _questions: &[Question]) -> Option<Vec<Answered>> {
        None
    }
}

/// One well-formed question, as a call would write it.
const ONE: &str = r#"{"questions":[{"heading":"Language","question":"Which language?",
    "answers":[{"answer":"Rust","says":"crucible's own"},{"answer":"Python"}]}]}"#;

/// Runs a call through a tool that answers with `put`.
fn ran(put: Arc<dyn Put>, args: &str) -> Result<ToolOutput, ToolError> {
    let tool = AskUser::new(put);
    tool.run(allowed(&tool, args), &Unwatched)
}

/// The refusal a call earns, or a panic naming what it produced instead.
fn refused(args: &str) -> String {
    match ran(Arc::new(Nobody), args) {
        Err(problem) => problem.to_string(),
        Ok(output) => panic!("the call was allowed through: {}", output.text()),
    }
}

#[test]
fn a_call_that_reaches_nothing_carries_a_read_only_target_that_resolves_to_nothing() {
    // It touches no file, starts no process and leaves nothing on the machine,
    // so there is nothing a rule could usefully be written about — and no new
    // kind of sensitivity for every mode to be decided about.
    let tool = AskUser::new(Arc::new(Nobody));

    assert!(matches!(
        tool.sensitivity(&ToolArgs::new(ONE)),
        Sensitivity::ReadOnly { .. }
    ));
}

#[test]
fn the_answers_come_back_beside_the_questions_they_answer() {
    let put = Arc::new(Whoever::saying(vec![Answered::new(["Rust"])]));
    let output = ran(put, ONE).expect("a call that asked properly");

    assert!(!output.is_failed());
    assert_eq!(output.text().trim(), "Which language? → Rust");
}

#[test]
fn a_question_answered_with_several_reads_back_as_several() {
    let args = r#"{"questions":[{"heading":"Support","question":"Which of these?","several":true,
        "answers":[{"answer":"Images"},{"answer":"PDFs"},{"answer":"Neither"}]}]}"#;
    let put = Arc::new(Whoever::saying(vec![Answered::new(["Images", "PDFs"])]));

    let output = ran(put, args).expect("a call that asked properly");

    assert_eq!(output.text().trim(), "Which of these? → Images, PDFs");
}

#[test]
fn a_line_the_reader_added_comes_back_under_the_question_it_is_about() {
    let put = Arc::new(Whoever::saying(vec![
        Answered::new(["Rust"]).noting("the examples have to compile"),
    ]));

    let output = ran(put, ONE).expect("a call that asked properly");

    assert!(
        output
            .text()
            .contains("they added: the examples have to compile"),
        "{}",
        output.text()
    );
}

#[test]
fn a_question_moved_on_from_with_nothing_chosen_says_so_rather_than_going_blank() {
    let none: [&str; 0] = [];
    let put = Arc::new(Whoever::saying(vec![Answered::new(none)]));

    let output = ran(put, ONE).expect("a call that asked properly");

    assert_eq!(output.text().trim(), "Which language? → nothing chosen");
}

#[test]
fn nobody_answering_is_a_result_the_turn_survives() {
    // An error would end the turn over a question, which is worse than the
    // question going unanswered — the model can ask again in its own words.
    let output = ran(Arc::new(Nobody), ONE).expect("a result rather than an error");

    assert!(!output.is_failed(), "{}", output.text());
    assert!(
        output.text().contains("Ask in the prompt instead"),
        "{}",
        output.text()
    );
}

#[test]
fn an_ask_left_part_answered_says_how_much_went_unanswered() {
    let args = r#"{"questions":[
        {"heading":"One","question":"First?","answers":[{"answer":"a"}]},
        {"heading":"Two","question":"Second?","answers":[{"answer":"b"}]}]}"#;
    let put = Arc::new(Whoever::saying(vec![Answered::new(["a"])]));

    let output = ran(put, args).expect("a call that asked properly");

    assert!(output.text().contains("First? → a"), "{}", output.text());
    assert!(
        output.text().contains("The last 1 went unanswered."),
        "{}",
        output.text()
    );
}

#[test]
fn every_bound_is_refused_with_the_figure_it_missed() {
    let many = (1..=5)
        .map(|at| {
            format!(r#"{{"heading":"H{at}","question":"Q{at}?","answers":[{{"answer":"a"}}]}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    let over = refused(&format!(r#"{{"questions":[{many}]}}"#));
    assert!(over.contains('5') && over.contains('4'), "{over}");

    let answers = (1..=9)
        .map(|at| format!(r#"{{"answer":"a{at}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let over = refused(&format!(
        r#"{{"questions":[{{"heading":"H","question":"Q?","answers":[{answers}]}}]}}"#
    ));
    assert!(over.contains('9') && over.contains('8'), "{over}");

    let rows = (1..=11)
        .map(|at| format!(r#""row {at}""#))
        .collect::<Vec<_>>()
        .join(",");
    let over = refused(&format!(
        r#"{{"questions":[{{"heading":"H","question":"Q?",
            "answers":[{{"answer":"a","shows":[{rows}]}}]}}]}}"#
    ));
    assert!(over.contains("11") && over.contains("10"), "{over}");

    let long = "q".repeat(LONG + 1);
    let over = refused(&format!(
        r#"{{"questions":[{{"heading":"H","question":"{long}","answers":[{{"answer":"a"}}]}}]}}"#
    ));
    assert!(over.contains(&(LONG + 1).to_string()), "{over}");
}

#[test]
fn a_call_with_nothing_to_ask_or_nothing_to_answer_with_is_refused() {
    assert!(refused(r#"{"questions":[]}"#).contains("at least one question"));
    assert!(
        refused(r#"{"questions":[{"heading":"H","question":"Q?","answers":[]}]}"#)
            .contains("at least one answer")
    );
    // A blank heading is a missing one, and is refused where it is read.
    assert!(
        refused(r#"{"questions":[{"heading":"","question":"Q?","answers":[{"answer":"a"}]}]}"#)
            .contains("is required")
    );
}

#[test]
fn a_blank_row_of_a_specimen_is_a_blank_line_and_not_a_missing_one() {
    // The one place an empty string means something here: a specimen is drawn
    // as it was written, and what parts a mock prompt from the line under it is
    // a row with nothing on it.
    let args = r#"{"questions":[{"heading":"Status","question":"Which one?",
        "answers":[{"answer":"Compact","shows":["› ...","","crucible · main"]}]}]}"#;
    let put = Arc::new(Whoever::saying(vec![Answered::new(["Compact"])]));

    let output = ran(put, args).expect("a specimen with a blank line in it");

    assert_eq!(output.text().trim(), "Which one? → Compact");
}

#[test]
fn the_row_beside_the_name_is_the_question_where_there_is_one_and_a_count_where_there_are_more() {
    let tool = AskUser::new(Arc::new(Nobody));

    assert_eq!(
        tool.summary(&ToolArgs::new(ONE)).as_str(),
        "Which language?"
    );

    let two = r#"{"questions":[
        {"heading":"One","question":"First?","answers":[{"answer":"a"}]},
        {"heading":"Two","question":"Second?","answers":[{"answer":"b"}]}]}"#;
    assert_eq!(tool.summary(&ToolArgs::new(two)).as_str(), "2 questions");
}

#[test]
fn what_the_reader_is_shown_is_what_the_call_asked() {
    let put = Arc::new(Whoever::saying(vec![Answered::new(["Rust"])]));
    let seen = Arc::clone(&put);

    ran(put, ONE).expect("a call that asked properly");

    let saw = seen.saw.lock().expect("nothing else holds it");
    assert_eq!(saw.as_slice(), ["Which language?"]);
}
