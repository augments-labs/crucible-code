//! Putting a question to the person at the keyboard.
//!
//! Most of what a model does not know it can find out by reading. What it
//! cannot read is which of several shapes somebody wants, and guessing there is
//! how a turn's whole output turns out to be about the wrong thing. So this
//! tool stops and asks, and by default the answer comes back inside the same
//! call rather than as a paragraph the person has to reply to in prose.
//!
//! **A call may choose not to wait.** With `mode` set to deferred the questions
//! are held by [`crucible_tools::Deferred`], and the call returns at once with
//! the id they are held as; the answer reaches the model later, as a message
//! from the person. Nowhere to hold them, or no room left, is a refusal the
//! model can correct and never a quiet fall back to waiting: a model that chose
//! not to wait has work lined up behind the call, and blocking would stall the
//! very turn that asked not to be.
//!
//! **It changes nothing and reaches nothing.** No file is touched, no process
//! is started and nothing leaves the machine; a call that does not wait leaves
//! its questions only in the session's own registry. So there is no target to
//! name and nothing a rule could usefully be written about. It carries a read-only
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

use crucible_runtime::BoxFuture;
use crucible_tools::{
    Approved, Deferred, DescribeTool, FEWEST_ANSWERS, MOST_ANSWERS, MOST_HEADING_BYTES,
    MOST_QUESTIONS, MOST_SHOWS, MOST_TEXT_BYTES, Put, Sensitivity, Summary, Target, Tool,
    ToolContext, ToolError, ToolOutput, is_one_line,
};
use crucible_types::{Answer, Answered, Question, ToolArgs};

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
const RECOMMENDED: &str = "recommended";
const MODE: &str = "mode";

/// The two ways a call can wait for its answer, spelled as the model sends them.
const BLOCKING: &str = "blocking";
const DEFERRED: &str = "deferred";
/// Both, as the schema offers them and the parse accepts them.
const MODES: &[&str] = &[BLOCKING, DEFERRED];

// How many questions one call may put, how many answers each may offer, how
// many rows a specimen may show and how long each text may be are
// `crucible_tools`'s: a question that waits for its answer is held there to the
// same bounds, so what this accepts is what may wait.
//
// Four questions is already more than anybody wants to answer in a row before
// the work carries on. Past it the call has stopped being a question and become
// a form, which is a thing to write in the prompt rather than to put on a
// panel. Eight answers is where a numbered list stops being read and starts
// being scanned. Two is the floor because a question offering one answer is
// not a question — it is a statement with a key to press, and the model wanting
// that should say it in its reply. The two this program adds are not counted:
// they are the way out rather than answers to what was asked. And the panel
// draws ten rows of a specimen and no more, so a call that sent more would be
// writing rows nobody will ever see.

/// The root `description` is the tool's own; everything below it describes the
/// arguments. Every bound is spelled by the constant the code refuses with,
/// so the sentence the model reads cannot drift from the refusal it meets.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: format!(
            "Asks the person at the keyboard to choose, and waits. Use it when the work forks \
             on what only they can settle and a wrong guess would waste the turn; not for what \
             reading the workspace would show or to confirm what they said. Ask once, with every \
             question the fork needs. Set {MODE} to {DEFERRED} when the answer is needed later \
             and there is work to do that does not depend on it."
        ),
        fields: vec![Field {
            name: QUESTIONS,
            about: format!(
                "In answering order, no two the same; more than {MOST_QUESTIONS} is a form for \
                 your reply."
            ),
            needed: true,
            shape: Shape::List {
                of: Box::new(Shape::Fields(vec![
                    Field {
                        name: HEADING,
                        about: format!(
                            "Two or three words on one line, naming the question beside the \
                             others. At most {MOST_HEADING_BYTES} bytes."
                        ),
                        needed: true,
                        shape: Shape::Text,
                    },
                    Field {
                        name: QUESTION,
                        about: format!(
                            "The question, written to be answered. At most {MOST_TEXT_BYTES} bytes."
                        ),
                        needed: true,
                        shape: Shape::Text,
                    },
                    Field {
                        name: SEVERAL,
                        about: "Whether several answers may be chosen; leave it out for one."
                            .into(),
                        needed: false,
                        shape: Shape::Flag,
                    },
                    Field {
                        name: ANSWERS,
                        about: format!(
                            "Best first, {FEWEST_ANSWERS} to {MOST_ANSWERS}, no two the same; one \
                             answer is a statement for your reply. A written-in answer and a way \
                             back to the prompt are added, so offer neither."
                        ),
                        needed: true,
                        shape: Shape::List {
                            of: Box::new(Shape::Fields(vec![
                                Field {
                                    name: ANSWER,
                                    about: format!(
                                        "The answer's name: a few words on one line. At most \
                                         {MOST_TEXT_BYTES} bytes."
                                    ),
                                    needed: true,
                                    shape: Shape::Text,
                                },
                                Field {
                                    name: SAYS,
                                    about: "What choosing it means, where the name does not say \
                                            it."
                                        .into(),
                                    needed: false,
                                    shape: Shape::Text,
                                },
                                Field {
                                    name: RECOMMENDED,
                                    about: "The one answer you would pick; at most one per \
                                            question, and listed first."
                                        .into(),
                                    needed: false,
                                    shape: Shape::Flag,
                                },
                                Field {
                                    name: SHOWS,
                                    about: format!(
                                        "For an answer that is a shape, such as a layout, a \
                                         format or a line of output: its rows, drawn as given \
                                         under it. At most {MOST_SHOWS} rows of \
                                         {MOST_TEXT_BYTES} bytes."
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
        }, Field {
            name: MODE,
            about: format!(
                "{DEFERRED} returns an id at once, and the answer arrives later as a message \
                 from the person. Defaults to {BLOCKING}."
            ),
            needed: false,
            shape: Shape::Choice(MODES),
        }],
    }
    .text()
});

/// Puts questions to whoever is at the keyboard.
pub struct AskUser {
    put: Arc<dyn Put>,
    /// Where a call that does not wait leaves its questions, where anything
    /// was given to hold them.
    held: Option<Deferred>,
}

impl AskUser {
    /// A tool that asks through `put`, and refuses a call that will not wait.
    #[must_use]
    pub fn new(put: Arc<dyn Put>) -> Self {
        Self { put, held: None }
    }

    /// The same tool, holding a call that will not wait in `held` until it is
    /// answered.
    #[must_use]
    pub fn deferring(self, held: Deferred) -> Self {
        Self {
            held: Some(held),
            ..self
        }
    }

    /// How the call waits for its answer, refused where it asks not to and
    /// there is nowhere to hold the questions.
    fn mode<'a>(&'a self, args: &Args) -> Result<Mode<'a>, ToolError> {
        match args.choice(MODE, BLOCKING, MODES)? {
            DEFERRED => self.held.as_ref().map(Mode::Deferred).ok_or_else(|| {
                args.wrong(format!(
                    "nothing in this session can hold a question for later, so {MODE} {DEFERRED} \
                     is not available; ask with {MODE} {BLOCKING}, or in your reply"
                ))
            }),
            _ => Ok(Mode::Blocking),
        }
    }
}

/// How one call waits for its answer.
enum Mode<'a> {
    /// In the call, through [`Put`].
    Blocking,
    /// Later: the questions wait here, and the call returns at once.
    Deferred(&'a Deferred),
}

impl std::fmt::Debug for AskUser {
    /// Written by hand because what it holds is a trait object this crate does
    /// not own, the way `crucible-tools` writes one for `dyn Tool`. There is
    /// nothing to redact: the questions arrive with the call, and the ones that
    /// wait are held by the registry rather than here.
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
        questions(&args)?;
        self.mode(&args).map(drop)
    }

    /// Reads nothing and reaches nothing. It puts words on the screen and waits
    /// for a key, or leaves them in the session's registry to be answered later,
    /// so there is no target to name and nothing a rule could usefully be
    /// written about.
    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::pathless(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        let Ok(parsed) = Args::parse(NAME, args) else {
            return Summary::new("");
        };
        let Ok(Some(questions)) = parsed.list(QUESTIONS) else {
            return summary::field(NAME, args, QUESTION, crucible_tools::Argument::Other);
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

    fn run<'a>(
        &'a self,
        approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let args = Args::parse(NAME, approved.args())?;
            let asked = questions(&args)?;

            match self.mode(&args)? {
                Mode::Blocking => {
                    let Some(given) = self.put.put(&asked).await else {
                        return Ok(ToolOutput::ok(
                            "Nobody answered. Ask in the prompt instead, in your own words.",
                        ));
                    };

                    Ok(ToolOutput::ok(said(&asked, &given)))
                }
                Mode::Deferred(held) => {
                    let id = held.register(asked).map_err(|full| {
                        args.wrong(format!("{full}; ask with {MODE} {BLOCKING} instead"))
                    })?;

                    Ok(ToolOutput::ok(format!(
                        "Asked as {id}, without waiting. The answer will come later, as a \
                         message from the person. Until then carry on only with work that does \
                         not depend on it, and do not ask the same thing again."
                    )))
                }
            }
        })
    }
}

/// Every question a call asked for, refused where it asked for something this
/// cannot draw.
fn questions(args: &Args) -> Result<Vec<Question>, ToolError> {
    args.only(&[QUESTIONS, MODE])?;

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

    let heading = label(args, HEADING, args.text(HEADING)?, MOST_HEADING_BYTES)?;
    let asked = bounded(args, QUESTION, args.text(QUESTION)?, MOST_TEXT_BYTES)?;

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

    // The one the model would pick is the one the reader is shown first, so a
    // call that marks two, or marks one that is not at the top, is asking for a
    // list that says something its order does not.
    let marked = answers.iter().filter(|one| one.is_recommended()).count();
    if marked > 1 {
        return Err(args.wrong(format!(
            "more than one {RECOMMENDED} answer, {marked} in all; \
             mark only the one you would pick"
        )));
    }
    if answers.iter().skip(1).any(Answer::is_recommended) {
        return Err(args.wrong(format!(
            "the {RECOMMENDED} answer must be listed first, and it is not; \
             move it to the top of {ANSWERS}"
        )));
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
    args.only(&[ANSWER, SAYS, SHOWS, RECOMMENDED])?;

    let name = label(args, ANSWER, args.text(ANSWER)?, MOST_TEXT_BYTES)?;
    let mut answer = Answer::new(name);

    if let Some(says) = args.optional_text(SAYS)? {
        answer = answer.saying(bounded(args, SAYS, says, MOST_TEXT_BYTES)?);
    }

    if args.flag(RECOMMENDED, false)? {
        answer = answer.recommending();
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
            bounded(args, SHOWS, row, MOST_TEXT_BYTES)?;
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
/// Length and nothing else; whether a label stays on its line is [`label`]'s.
/// Whether a field may be blank is already answered where it is read — a
/// heading and a question are required and a blank one is a missing one, while
/// **a blank row of a specimen is a blank line in what the
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

/// `text`, bounded as [`bounded`] bounds it, or a refusal where it breaks its
/// line or holds a control character.
///
/// For a heading and an answer, which the answer is said under as labels, each
/// on a line of its own: one carrying a line of its own could read there as
/// something the person wrote. A question and what an answer says or shows
/// are not labels, and may break their lines.
fn label<'a>(args: &Args, field: &str, text: &'a str, most: usize) -> Result<&'a str, ToolError> {
    let text = bounded(args, field, text, most)?;
    if !is_one_line(text) {
        return Err(args.wrong(format!(
            "{field} must stay on one line, with no line break or control character"
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
/// three was. It is read back on one line, whatever line breaks it holds: one
/// line per question is what lets the model tell which words are an answer,
/// and a question written to carry a line shaped like one would otherwise read
/// back as an answer nobody gave.
fn said(asked: &[Question], given: &[Answered]) -> String {
    let mut text = String::new();

    for (question, answered) in asked.iter().zip(given) {
        let chosen: Vec<&str> = answered.chosen().collect();
        let chosen = if chosen.is_empty() {
            "nothing chosen".to_owned()
        } else {
            chosen.join(", ")
        };

        let asked = flattened(question.question());
        let _ = writeln!(text, "{asked} → {chosen}");
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

/// `text` on one line, each character that would break it read as a space.
fn flattened(text: &str) -> String {
    text.chars()
        .map(|one| {
            if is_one_line(one.encode_utf8(&mut [0; 4])) {
                one
            } else {
                ' '
            }
        })
        .collect()
}
