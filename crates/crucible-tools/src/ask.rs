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
use std::sync::{Arc, LazyLock};

use crucible_core::{
    Answer, Answered, Approved, DescribeTool, Put, Question, Sensitivity, Summary, Target, Tool,
    ToolArgs, ToolContext, ToolError, ToolOutput,
};

use crate::args::Args;
use crate::schema::{Field, Schema, Shape};
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

/// How many answers one question may offer, and how few.
///
/// Eight is where a numbered list stops being read and starts being scanned.
/// Two is the floor because a question offering one answer is not a question —
/// it is a statement with a key to press, and the model wanting that should say
/// it in its reply. The two this program adds are not counted: they are the way
/// out rather than answers to what was asked.
const MOST_ANSWERS: usize = 8;
const FEWEST_ANSWERS: usize = 2;

/// How long a heading may be, in bytes.
///
/// Every heading is drawn on one row across the top of the panel, so what
/// bounds this is not what a heading needs but what the row can hold: four of
/// them at eighty columns leaves about fifteen each, marks and gaps included.
/// Past this the row gives way to a count, and a heading nobody ever sees is a
/// heading the model spent tokens on for nothing.
const HEADING_SHORT: usize = 24;

/// How many rows of a specimen one answer may carry.
///
/// The panel draws this many and no more, so a call that sent more would be
/// writing rows nobody will ever see.
const MOST_SHOWS: usize = 10;

/// How long an answer, its one-line meaning or a row of a specimen may be, in
/// bytes.
///
/// The same length a question gets: an answer is folded across the panel the
/// way the question above it is, so what bounds it is the reader's patience
/// rather than a row — and a specimen row past the panel's width is clipped,
/// which its own description says.
const SHORT: usize = 500;

/// How long a question may be, in bytes.
const LONG: usize = 500;

/// The root `description` is the tool's own; everything below it describes the
/// arguments. Every bound is spelled by the constant the code refuses with,
/// so the sentence the model reads cannot drift from the refusal it meets.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Asks the person at the keyboard to choose, and waits for their answer. Worth \
                using when the work forks on something only they can settle — which of several \
                shapes to build, which of several directions to take — and guessing would put \
                the whole turn's output on the wrong side of the fork. Not worth using for \
                anything you could find out by reading the workspace, and not worth using to \
                confirm what they already told you. Ask once, with every question the fork \
                needs, rather than a question at a time."
            .into(),
        fields: vec![Field {
            name: QUESTIONS,
            about: format!(
                "The questions to put, in the order they should be answered. At most \
                 {MOST_QUESTIONS}, and no two the same: past that this is a form rather than a \
                 question, and belongs in your reply instead."
            ),
            needed: true,
            shape: Shape::List {
                of: Box::new(Shape::Fields(vec![
                    Field {
                        name: HEADING,
                        about: format!(
                            "Two or three words naming this question, shown in a row of all of \
                             them so the reader can see where they are. At most {HEADING_SHORT} \
                             bytes: the row holds every heading at once, and a longer one costs \
                             the whole row."
                        ),
                        needed: true,
                        shape: Shape::Text,
                    },
                    Field {
                        name: QUESTION,
                        about: format!(
                            "The question itself, written to be answered rather than read. At \
                             most {LONG} bytes."
                        ),
                        needed: true,
                        shape: Shape::Text,
                    },
                    Field {
                        name: SEVERAL,
                        about: "Whether more than one answer may be chosen. Leave it out for a \
                                question with one answer."
                            .into(),
                        needed: false,
                        shape: Shape::Flag,
                    },
                    Field {
                        name: ANSWERS,
                        about: format!(
                            "The answers to offer, best first. At least {FEWEST_ANSWERS} and at \
                             most {MOST_ANSWERS}, and no two the same. One answer is a statement \
                             rather than a question — say that in your reply instead. Two more \
                             are always added for you — one to write an answer you did not \
                             offer, and one to leave the whole thing and reply in the prompt — \
                             so do not offer either yourself."
                        ),
                        needed: true,
                        shape: Shape::List {
                            of: Box::new(Shape::Fields(vec![
                                Field {
                                    name: ANSWER,
                                    about: format!(
                                        "What this answer is called, in a few words. At most \
                                         {SHORT} bytes."
                                    ),
                                    needed: true,
                                    shape: Shape::Text,
                                },
                                Field {
                                    name: SAYS,
                                    about: "One line saying what choosing it means, for an \
                                            answer whose name does not say it. Leave it out \
                                            where the name is enough."
                                        .into(),
                                    needed: false,
                                    shape: Shape::Text,
                                },
                                Field {
                                    name: SHOWS,
                                    about: format!(
                                        "What this answer would look like, row by row, for a \
                                         question whose answer is a shape rather than a word — \
                                         a layout, a format, a line of output. Drawn as given, \
                                         under the answer, so write the rows as the reader \
                                         would meet them. At most {MOST_SHOWS} rows of at most \
                                         {SHORT} bytes."
                                    ),
                                    needed: false,
                                    shape: Shape::List {
                                        of: Box::new(Shape::Text),
                                        fewest: None,
                                        most: Some(MOST_SHOWS),
                                    },
                                },
                            ])),
                            fewest: Some(FEWEST_ANSWERS),
                            most: Some(MOST_ANSWERS),
                        },
                    },
                ])),
                fewest: Some(1),
                most: Some(MOST_QUESTIONS),
            },
        }],
    }
    .text()
});

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

impl DescribeTool for AskUser {
    fn name(&self) -> &str {
        NAME
    }

    fn schema(&self) -> &str {
        SCHEMA.as_str()
    }
}

impl Tool for AskUser {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        let args = Args::parse(NAME, args)?;
        questions(&args).map(drop)
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

    fn run(&self, approved: Approved, _context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
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
    args.only(&[QUESTIONS])?;

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

    let asked: Vec<Question> = written.iter().map(question).collect::<Result<_, _>>()?;

    // The same question twice is one the reader answers once and is asked again,
    // and the two answers come back under one heading with no way to tell which
    // was which.
    if let Some(twice) = repeated(asked.iter().map(Question::question)) {
        return Err(args.wrong(format!("{twice} is asked twice")));
    }

    Ok(asked)
}

/// One question, and the answers under it.
fn question(args: &Args) -> Result<Question, ToolError> {
    args.only(&[HEADING, QUESTION, SEVERAL, ANSWERS])?;

    let heading = bounded(args, HEADING, args.text(HEADING)?, HEADING_SHORT)?;
    let asked = bounded(args, QUESTION, args.text(QUESTION)?, LONG)?;

    let Some(written) = args.list(ANSWERS)? else {
        return Err(args.wrong(format!("{ANSWERS} is required and must be a list")));
    };
    if written.len() < FEWEST_ANSWERS {
        return Err(args.wrong(format!(
            "a question offers at least {FEWEST_ANSWERS} answers; one answer is \
             a statement rather than a question, and belongs in your reply"
        )));
    }
    if written.len() > MOST_ANSWERS {
        return Err(args.wrong(format!(
            "{} answers is more than the {MOST_ANSWERS} one question may offer",
            written.len()
        )));
    }

    let answers = written.iter().map(answer).collect::<Result<Vec<_>, _>>()?;

    // Two answers spelled the same are two the reader cannot choose between,
    // and one of them can never be what comes back.
    if let Some(twice) = repeated(answers.iter().map(Answer::answer)) {
        return Err(args.wrong(format!("{twice} is offered twice")));
    }

    let question = Question::new(heading, asked, answers);
    Ok(if args.flag(SEVERAL, false)? {
        question.several()
    } else {
        question
    })
}

/// One answer, with what it means and what it would look like.
fn answer(args: &Args) -> Result<Answer, ToolError> {
    args.only(&[ANSWER, SAYS, SHOWS])?;

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

/// The first thing `every` holds twice, where it holds anything twice.
fn repeated<'a>(every: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = Vec::new();

    for one in every {
        if seen.contains(&one) {
            return Some(one);
        }
        seen.push(one);
    }

    None
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
