//! Putting a question to the person at the keyboard.
//!
//! Most of what a model does not know it can find out by reading. What it
//! cannot read is which of several shapes somebody wants, and guessing there is
//! how a turn's whole output turns out to be about the wrong thing. So this
//! tool stops and asks, and the answer comes back inside the same call rather
//! than as a paragraph the person has to reply to in prose.
//!
//! **It changes nothing and reaches nothing.** No file is touched, no process
//! is started and nothing leaves the machine, so there is no target to name and
//! nothing a rule could usefully be written about. It carries a read-only
//! sensitivity over a target that resolves to nothing, which is the answer
//! [`crate::ToolSearch`] carries for the same reason.
//!
//! **Two answers are added to every question and are not in the schema.** One
//! lets somebody write an answer nobody offered, and one leaves the whole ask
//! and says so in the prompt instead. They are the surface's rather than the
//! call's, so the way out is there whether or not what wrote the call thought
//! of it.
//!
//! **Nobody answering is a result and not an error.** A person who walked away,
//! a window with no room, a session with nothing to ask on: the turn survives
//! all three and the model is told to ask in the prompt. An error would end the
//! turn over a question, which is a worse outcome than the question going
//! unanswered.
//!
//! Every bound here is a refusal the model can correct, and each says what it
//! missed and by how much. A call that asked for forty questions is one the
//! model rewrites; the turn is worth more than the correction costs.

use std::fmt::Write as _;
use std::sync::Arc;

use crucible_core::{
    Answer, Answered, Approved, Put, Question, Sensitivity, Summary, Target, Tool, ToolArgs,
    ToolError, ToolOutput, Watch,
};

use crate::args::Args;
use crate::summary;

#[cfg(test)]
mod tests;

/// The name the model calls.
const NAME: &str = "ask_user";

/// The fields a call is written with.
const QUESTIONS: &str = "questions";
const HEADING: &str = "heading";
const QUESTION: &str = "question";
const SEVERAL: &str = "several";
const ANSWERS: &str = "answers";
const ANSWER: &str = "answer";
const SAYS: &str = "says";
const SHOWS: &str = "shows";

/// How many questions one call may put.
///
/// Four is already more than anybody wants to answer in a row before the work
/// carries on. Past it the call has stopped being a question and become a form,
/// which is a thing to write in the prompt rather than to put on a panel.
const MOST_QUESTIONS: usize = 4;

/// How many answers one question may offer.
///
/// Eight is where a numbered list stops being read and starts being scanned.
const MOST_ANSWERS: usize = 8;

/// How many rows of a specimen one answer may carry.
///
/// The panel draws this many and no more, so a call that sent more would be
/// writing rows nobody will ever see.
const MOST_SHOWS: usize = 10;

/// How long a heading, an answer or a row of a specimen may be, in bytes.
const SHORT: usize = 200;

/// How long a question may be, in bytes.
const LONG: usize = 500;

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Asks the person at the keyboard to choose, and waits for their answer. Worth using when the work forks on something only they can settle — which of several shapes to build, which of several directions to take — and guessing would put the whole turn's output on the wrong side of the fork. Not worth using for anything you could find out by reading the workspace, and not worth using to confirm what they already told you. Ask once, with every question the fork needs, rather than a question at a time.",
  "type": "object",
  "properties": {
    "questions": {
      "type": "array",
      "description": "The questions to put, in the order they should be answered. At most 4: past that this is a form rather than a question, and belongs in your reply instead.",
      "items": {
        "type": "object",
        "properties": {
          "heading": {
            "type": "string",
            "description": "Two or three words naming this question, shown in a row of all of them so the reader can see where they are. At most 200 bytes."
          },
          "question": {
            "type": "string",
            "description": "The question itself, written to be answered rather than read. At most 500 bytes."
          },
          "several": {
            "type": "boolean",
            "description": "Whether more than one answer may be chosen. Leave it out for a question with one answer."
          },
          "answers": {
            "type": "array",
            "description": "The answers to offer, best first. At most 8. Two more are always added for you — one to write an answer you did not offer, and one to leave the whole thing and reply in the prompt — so do not offer either yourself.",
            "items": {
              "type": "object",
              "properties": {
                "answer": {
                  "type": "string",
                  "description": "What this answer is called, in a few words. At most 200 bytes."
                },
                "says": {
                  "type": "string",
                  "description": "One line saying what choosing it means, for an answer whose name does not say it. Leave it out where the name is enough."
                },
                "shows": {
                  "type": "array",
                  "description": "What this answer would look like, row by row, for a question whose answer is a shape rather than a word — a layout, a format, a line of output. Drawn as given, under the answer, so write the rows as the reader would meet them. At most 10 rows of at most 200 bytes.",
                  "items": { "type": "string" }
                }
              },
              "required": ["answer"]
            }
          }
        },
        "required": ["heading", "question", "answers"]
      }
    }
  },
  "required": ["questions"]
}"#;

/// Puts questions to whoever is at the keyboard.
pub struct AskUser {
    put: Arc<dyn Put>,
}

impl AskUser {
    /// A tool that asks through `put`.
    #[must_use]
    pub fn new(put: Arc<dyn Put>) -> Self {
        Self { put }
    }
}

impl std::fmt::Debug for AskUser {
    /// Written by hand because what it holds is a trait object this crate does
    /// not own, the way core writes one for `dyn Tool`. There is nothing to
    /// redact: the questions arrive with the call and are never held here.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AskUser")
    }
}

impl Tool for AskUser {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA
    }

    /// Reads nothing and reaches nothing. It puts words on the screen and waits
    /// for a key, so there is no target to name and nothing a rule could
    /// usefully be written about.
    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        let Ok(parsed) = Args::parse(NAME, args) else {
            return Summary::new("");
        };
        let Ok(Some(questions)) = parsed.list(QUESTIONS) else {
            return summary::field(NAME, args, QUESTION);
        };

        match questions.len() {
            0 => Summary::new(""),
            1 => questions
                .first()
                .and_then(|one| one.text(QUESTION).ok())
                .map_or_else(|| Summary::new(""), Summary::new),
            many => Summary::new(format!("{many} questions")),
        }
    }

    fn run(&self, approved: Approved, _watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let asked = questions(&args)?;

        let Some(given) = self.put.put(&asked) else {
            return Ok(ToolOutput::ok(
                "Nobody answered. Ask in the prompt instead, in your own words.",
            ));
        };

        Ok(ToolOutput::ok(said(&asked, &given)))
    }
}

/// Every question a call asked for, refused where it asked for something this
/// cannot draw.
fn questions(args: &Args) -> Result<Vec<Question>, ToolError> {
    let Some(written) = args.list(QUESTIONS)? else {
        return Err(args.wrong(format!("{QUESTIONS} is required and must be a list")));
    };

    if written.is_empty() {
        return Err(args.wrong("ask at least one question"));
    }
    if written.len() > MOST_QUESTIONS {
        return Err(args.wrong(format!(
            "{} questions is more than the {MOST_QUESTIONS} one call may put; \
             ask the rest after these are answered",
            written.len()
        )));
    }

    written.iter().map(question).collect()
}

/// One question, and the answers under it.
fn question(args: &Args) -> Result<Question, ToolError> {
    let heading = bounded(args, HEADING, args.text(HEADING)?, SHORT)?;
    let asked = bounded(args, QUESTION, args.text(QUESTION)?, LONG)?;

    let Some(written) = args.list(ANSWERS)? else {
        return Err(args.wrong(format!("{ANSWERS} is required and must be a list")));
    };
    if written.is_empty() {
        return Err(args.wrong("offer at least one answer"));
    }
    if written.len() > MOST_ANSWERS {
        return Err(args.wrong(format!(
            "{} answers is more than the {MOST_ANSWERS} one question may offer",
            written.len()
        )));
    }

    let answers = written.iter().map(answer).collect::<Result<Vec<_>, _>>()?;

    let question = Question::new(heading, asked, answers);
    Ok(if args.flag(SEVERAL, false)? {
        question.several()
    } else {
        question
    })
}

/// One answer, with what it means and what it would look like.
fn answer(args: &Args) -> Result<Answer, ToolError> {
    let name = bounded(args, ANSWER, args.text(ANSWER)?, SHORT)?;
    let mut answer = Answer::new(name);

    if let Some(says) = args.optional_text(SAYS)? {
        answer = answer.saying(bounded(args, SAYS, says, SHORT)?);
    }

    if args.holds(SHOWS) {
        let shows = args.texts(SHOWS)?;
        if shows.len() > MOST_SHOWS {
            return Err(args.wrong(format!(
                "{} rows is more than the {MOST_SHOWS} a specimen may show",
                shows.len()
            )));
        }
        for row in &shows {
            bounded(args, SHOWS, row, SHORT)?;
        }
        answer = answer.showing(shows);
    }

    Ok(answer)
}

/// `text`, or a refusal naming the field, the bound and what was sent.
///
/// Bytes rather than characters, because what this bounds is what the next
/// request pays for and what a panel has to lay out, and the first of those is
/// counted in bytes.
///
/// Length and nothing else. Whether a field may be blank is already answered
/// where it is read — a heading and a question are required and a blank one is
/// a missing one, while **a blank row of a specimen is a blank line in what the
/// answer would look like**, which is a row somebody wrote on purpose.
fn bounded<'a>(args: &Args, field: &str, text: &'a str, most: usize) -> Result<&'a str, ToolError> {
    if text.len() > most {
        return Err(args.wrong(format!(
            "{field} is {} bytes, and at most {most} are allowed",
            text.len()
        )));
    }

    Ok(text)
}

/// What the model is told, one line per question.
///
/// The question is repeated rather than referred to by number, because the
/// answer is read in a transcript where the call's own arguments are not on
/// screen — a line saying *3 → Rust* would need the reader to go and find what
/// three was.
fn said(asked: &[Question], given: &[Answered]) -> String {
    let mut text = String::new();

    for (question, answered) in asked.iter().zip(given) {
        let chosen: Vec<&str> = answered.chosen().collect();
        let chosen = if chosen.is_empty() {
            "nothing chosen".to_owned()
        } else {
            chosen.join(", ")
        };

        let _ = writeln!(text, "{} → {chosen}", question.question());
        if !answered.note().is_empty() {
            let _ = writeln!(text, "    they added: {}", answered.note());
        }
    }

    // A question the answers ran out for is one nobody reached, which happens
    // when an ask is left part-answered. Saying so is what stops the model
    // reading the short list as the whole of it.
    if given.len() < asked.len() {
        let _ = writeln!(
            text,
            "\nThe last {} went unanswered.",
            asked.len() - given.len()
        );
    }

    text
}
