//! Where a tool puts the questions only a person can answer.

use crucible_types::{Answered, Question};

/// Where a tool puts its questions to whoever can answer them.
///
/// A trait for the reason `Post` and [`crate::Watch`] are: the thing
/// that answers is a front end, and a second one must need no edit here.
///
/// It is the neighbour of [`crate::Ask`] and answers a different question.
/// That one asks whether a call may run and is owed a verdict, so its silence
/// has to be a refusal — running a tool nobody agreed to is worse than
/// stopping. This one asks a person to decide something, and nothing runs
/// either way, so its silence is nobody answering.
pub trait Put: Send + Sync {
    /// Puts `questions` and blocks until they are answered.
    ///
    /// One [`Answered`] per question, in the order they were asked. `None` is
    /// nobody answered — the ask was left, or there was never anybody there —
    /// and it is not a failure: the tool turns it into a result the turn
    /// survives.
    fn put(&self, questions: &[Question]) -> Option<Vec<Answered>>;
}

#[cfg(test)]
mod tests {
    use crucible_types::Answer;

    use super::*;

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
}
