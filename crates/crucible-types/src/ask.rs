//! What a tool asks the person at the keyboard, and what they answer.
//!
//! A tool that needs a decision only a person can make sends the questions and
//! blocks until they come back. The thread that would answer is the one drawing
//! the screen, so the questions cross a thread boundary — which is why these
//! values live here rather than in the crate that asks or the one that draws.
//! Neither of those may depend on the other. What carries them across is `Put`,
//! in `crucible-tools`.
//!
//! Every string here is somebody's own words: the questions are the model's and
//! the answers are the reader's. So all three values write `Debug` by hand and
//! redact.

use std::fmt;

/// One thing a question offers to be chosen.
#[derive(Clone)]
pub struct Answer {
    name: Box<str>,
    says: Box<str>,
    shows: Box<[Box<str>]>,
}

impl Answer {
    /// An answer with nothing said about it and nothing to show.
    #[must_use]
    pub fn new(answer: impl Into<Box<str>>) -> Self {
        Self {
            name: answer.into(),
            says: Box::from(""),
            shows: Box::new([]),
        }
    }

    /// The same answer, with the one line saying what it means.
    #[must_use]
    pub fn saying(self, says: impl Into<Box<str>>) -> Self {
        Self {
            says: says.into(),
            ..self
        }
    }

    /// The same answer, with the rows showing what it would look like.
    #[must_use]
    pub fn showing(self, shows: impl IntoIterator<Item = impl Into<Box<str>>>) -> Self {
        Self {
            shows: shows.into_iter().map(Into::into).collect(),
            ..self
        }
    }

    /// What the answer is called.
    #[must_use]
    pub fn answer(&self) -> &str {
        &self.name
    }

    /// What it means, or empty where the call said nothing.
    #[must_use]
    pub fn says(&self) -> &str {
        &self.says
    }

    /// The rows of its specimen, in the order they were written.
    pub fn shows(&self) -> impl ExactSizeIterator<Item = &str> {
        self.shows.iter().map(AsRef::as_ref)
    }
}

/// One question, and the answers it offers.
///
/// Cloned to cross a thread boundary, the way [`crate::ToolCall`] is: the tool
/// asking holds a borrow of these and the thread that draws them is somewhere
/// else entirely.
#[derive(Clone)]
pub struct Question {
    heading: Box<str>,
    asks: Box<str>,
    several: bool,
    answers: Box<[Answer]>,
}

impl Question {
    /// A question taking one of its answers.
    #[must_use]
    pub fn new(
        heading: impl Into<Box<str>>,
        question: impl Into<Box<str>>,
        answers: impl IntoIterator<Item = Answer>,
    ) -> Self {
        Self {
            heading: heading.into(),
            asks: question.into(),
            several: false,
            answers: answers.into_iter().collect(),
        }
    }

    /// The same question, taking as many of its answers as are chosen.
    #[must_use]
    pub fn several(self) -> Self {
        Self {
            several: true,
            ..self
        }
    }

    /// The few words it is called where every question is listed at once.
    #[must_use]
    pub fn heading(&self) -> &str {
        &self.heading
    }

    /// The question itself, in the words it was asked in.
    #[must_use]
    pub fn question(&self) -> &str {
        &self.asks
    }

    /// Whether more than one of its answers may be chosen.
    #[must_use]
    pub fn takes_several(&self) -> bool {
        self.several
    }

    /// The answers, in the order they are offered.
    pub fn answers(&self) -> impl ExactSizeIterator<Item = &Answer> {
        self.answers.iter()
    }
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Answer([redacted])")
    }
}

impl fmt::Debug for Question {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Question([redacted])")
    }
}

/// What one question got back.
pub struct Answered {
    chosen: Box<[Box<str>]>,
    note: Box<str>,
}

impl Answered {
    /// The answers chosen, in the order they were offered.
    ///
    /// Empty is an answer: a question taking several may be moved on from with
    /// none of them chosen, and that is a thing the reader did rather than a
    /// thing that failed.
    #[must_use]
    pub fn new(chosen: impl IntoIterator<Item = impl Into<Box<str>>>) -> Self {
        Self {
            chosen: chosen.into_iter().map(Into::into).collect(),
            note: Box::from(""),
        }
    }

    /// The same answer, with the line the reader added beside it.
    #[must_use]
    pub fn noting(self, note: impl Into<Box<str>>) -> Self {
        Self {
            note: note.into(),
            ..self
        }
    }

    /// What was chosen, in the order it was offered.
    pub fn chosen(&self) -> impl ExactSizeIterator<Item = &str> {
        self.chosen.iter().map(AsRef::as_ref)
    }

    /// The reader's own line, or empty where they wrote none.
    #[must_use]
    pub fn note(&self) -> &str {
        &self.note
    }
}

impl fmt::Debug for Answered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Answered([redacted])")
    }
}

#[cfg(test)]
mod tests;
