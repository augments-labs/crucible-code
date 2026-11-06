//! Where a tool puts the questions only a person can answer.
//!
//! Two ways, for two kinds of asking. [`Put`] is the one a tool awaits: the
//! call holds until somebody decides. [`Deferred`] is the one it does not: the
//! questions are left waiting, the call returns, and the answer is delivered
//! later as a message of its own. The bounds on what may wait, and the one
//! place an answer becomes the words the model reads, are here with it.

use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crucible_runtime::BoxFuture;
use crucible_types::{Answer, Answered, Batch, Question, QuestionId, Reply};
use serde_json::{Value, json};

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
    /// Puts `questions` and awaits the answer.
    ///
    /// A question's wait is human-length, so this hands back a future rather
    /// than blocking inside a ready one: an implementation answers `Pending`
    /// the moment it is asked and wakes its waker once somebody has decided,
    /// so the thread polling it is never held for the wait.
    ///
    /// One [`Answered`] per question, in the order they were asked. `None` is
    /// nobody answered — the ask was left, or there was never anybody there
    /// — and it is not a failure: the tool turns it into a result the turn
    /// survives.
    fn put<'a>(&'a self, questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>>;
}

/// The most batches held at once, waiting or settled and not yet taken.
///
/// A settled batch keeps its place until the runner takes it, so the bound
/// holds over everything this keeps rather than over the waiting alone.
pub const MOST_HELD: usize = 8;

/// The most question text, in bytes, the held batches carry between them.
pub const MOST_HELD_BYTES: usize = 64 * 1024;

/// The most bytes one note, or one written-in answer, may be.
pub const MOST_REPLY_BYTES: usize = 4 * 1024;

/// The most questions one batch asks.
///
/// This and the three bounds below are the shape of a batch a person can be
/// asked. A tool that parses questions holds its caller to these same values,
/// so that what it accepts is what may be held.
pub const MOST_QUESTIONS: usize = 4;

/// The fewest answers one question offers.
pub const FEWEST_ANSWERS: usize = 2;

/// The most answers one question offers.
pub const MOST_ANSWERS: usize = 8;

/// The most rows one answer shows.
pub const MOST_SHOWS: usize = 10;

/// Questions asked without waiting for the answer, held until it is delivered.
///
/// One concrete handle rather than a trait beside [`Put`]: there is one way to
/// hold a question, and it is this. Cloning shares the batches, as cloning
/// `Steer` shares its lines, so the clone the tool registers on and the one a
/// client settles through see the same ones.
///
/// A batch is waiting from [`Deferred::register`] until [`Deferred::settle`]
/// takes a reply for every question in it at once, and settled from then
/// until [`Deferred::take_settled`] hands it out to be delivered. Every change
/// happens under one lock, so two settlements of one batch cannot both win,
/// and a batch taken cannot be taken again.
///
/// Everything is bounded before it is stored, and a refusal changes nothing.
/// Poisoning is not one of the ways a question is lost, for the reason it is
/// not one of the ways a steered line is.
#[derive(Clone, Default)]
pub struct Deferred(Arc<Mutex<Held>>);

/// What the registry holds, all under its one lock.
#[derive(Default)]
struct Held {
    /// The number the next batch registered is given.
    next: u64,
    /// Moved by every change, so a reader can tell whether to look again.
    revision: u64,
    /// The batches waiting for an answer, oldest first.
    waiting: Vec<Batch>,
    /// The batches answered and not yet taken, in the order they were.
    settled: Vec<Delivered>,
}

/// One settled batch, handed out to be delivered.
pub struct Delivered {
    batch: Batch,
    replies: Vec<Reply>,
}

/// Why a batch was not registered.
#[derive(Debug, thiserror::Error)]
pub enum Full {
    /// There is no room for it.
    #[error(transparent)]
    Room(#[from] Room),
    /// Every id has been handed out, and handing one out again could make
    /// an old answer settle a batch it was never meant for.
    #[error("every question batch id has been handed out; no more questions can wait")]
    Ids,
    /// The batch is not one a person can be asked.
    #[error(transparent)]
    Shape(Misshapen),
}

/// Why there is no room for more batches: the one refusal registering a
/// batch and restoring a saved state both meet.
#[derive(Debug, thiserror::Error)]
pub enum Room {
    /// As many batches are held as may be.
    #[error(
        "{MOST_HELD} question batches are already held, waiting for an answer or for it to be \
         delivered; one must be answered and delivered before another can wait"
    )]
    Batches,
    /// The batches would carry the held question text past its bound.
    #[error(
        "the questions are {bytes} bytes, and with the {held} already held that is {} bytes \
         over the {MOST_HELD_BYTES} that may be held",
        (.bytes + .held).saturating_sub(MOST_HELD_BYTES)
    )]
    Bytes {
        /// The bytes of question text the batches carry.
        bytes: usize,
        /// The bytes of question text already held.
        held: usize,
    },
}

/// Why a batch is not one a person can be asked.
///
/// Questions are counted from one, and no refusal repeats what was asked.
#[derive(Debug, thiserror::Error)]
pub enum Misshapen {
    /// It asks nothing.
    #[error("a batch asks at least one question")]
    Empty,
    /// It asks more than one batch may.
    #[error("{0} questions is more than the {MOST_QUESTIONS} one batch may ask")]
    Questions(usize),
    /// A question offers too few answers, or too many.
    #[error(
        "question {question} offers {given} answers, and a question offers {FEWEST_ANSWERS} to \
         {MOST_ANSWERS}"
    )]
    Answers {
        /// The question, counted from one.
        question: usize,
        /// How many it offers.
        given: usize,
    },
    /// An answer shows more rows than one may.
    #[error(
        "an answer to question {question} shows {given} rows, more than the {MOST_SHOWS} one may"
    )]
    Shows {
        /// The question, counted from one.
        question: usize,
        /// How many rows the answer shows.
        given: usize,
    },
    /// A question offers one answer twice.
    #[error("question {question} offers the same answer twice")]
    RepeatedAnswer {
        /// The question, counted from one.
        question: usize,
    },
    /// A question is asked twice.
    #[error("question {question} asks what an earlier question asks")]
    RepeatedQuestion {
        /// The second time it is asked, counted from one.
        question: usize,
    },
}

/// Why a batch's replies were not taken.
///
/// Questions are counted from one, as they are shown. No refusal repeats what
/// a reply said: the words are the reader's, and an error is shown and logged
/// where they must not be.
#[derive(Debug, thiserror::Error)]
pub enum Unsettled {
    /// No batch by that id is waiting: it was settled already, or never asked.
    #[error(
        "no question batch {0} is waiting for an answer: it was answered already, or never asked"
    )]
    Stale(QuestionId),
    /// A reply is owed to every question, and the count was wrong.
    #[error("{asked} questions were asked and {replied} replies given; give one for each")]
    Count {
        /// How many questions the batch asks.
        asked: usize,
        /// How many replies were given.
        replied: usize,
    },
    /// A question taking one answer was given some other number.
    #[error("question {question} takes one answer and was given {given}")]
    OneAnswer {
        /// The question, counted from one.
        question: usize,
        /// How many it was given.
        given: usize,
    },
    /// One answer was chosen twice.
    #[error("question {question} was given the same answer twice")]
    Repeated {
        /// The question, counted from one.
        question: usize,
    },
    /// More than one answer was one the question did not offer.
    #[error("question {question} was given more than one written-in answer")]
    WrittenTwice {
        /// The question, counted from one.
        question: usize,
    },
    /// A note or a written-in answer is longer than it may be.
    #[error(
        "the {part} for question {question} is {bytes} bytes, {} bytes over the \
         {MOST_REPLY_BYTES} it may be",
        .bytes.saturating_sub(MOST_REPLY_BYTES)
    )]
    TooLong {
        /// The question, counted from one.
        question: usize,
        /// Which of its words.
        part: Part,
        /// How long they are.
        bytes: usize,
    },
    /// A note or a written-in answer holds a control character.
    ///
    /// Refused rather than stripped: what is delivered is what was written or
    /// nothing, and a terminal escape in a reply is not something to deliver.
    #[error("the {part} for question {question} holds a control character")]
    Control {
        /// The question, counted from one.
        question: usize,
        /// Which of its words.
        part: Part,
    },
}

/// Which of a reply's own words a refusal is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The line the reader added beside their choice.
    Note,
    /// An answer the question did not offer.
    WrittenIn,
}

/// Why a saved state was not restored. Nothing is restored when it is not.
#[derive(Debug, thiserror::Error)]
pub enum Unrestored {
    /// The state is not one [`Deferred::state`] writes.
    #[error("the saved questions are not in the form they are saved in: {0}")]
    Malformed(&'static str),
    /// The state holds more than may wait.
    #[error(transparent)]
    Room(#[from] Room),
    /// A batch is not one a person can be asked.
    #[error(transparent)]
    Shape(Misshapen),
}

impl Deferred {
    /// Holds `questions` as one waiting batch, and answers what names it.
    ///
    /// # Errors
    ///
    /// [`Full`] where the batch would hold one batch more, or more question
    /// text, than may wait, where every id has been handed out, or where the
    /// batch is not one a person can be asked; nothing is stored then.
    pub fn register(&self, questions: Vec<Question>) -> Result<QuestionId, Full> {
        shape(&questions).map_err(Full::Shape)?;
        let mut held = self.held();
        // The last number is never handed out: the one after it is not there.
        let after = held.next.checked_add(1).ok_or(Full::Ids)?;
        held.admit(1, text_bytes(&questions))?;

        let id = QuestionId::numbered(held.next);
        held.next = after;
        held.waiting.push(Batch::new(id, questions));
        held.changed();
        Ok(id)
    }

    /// The batches waiting for an answer, oldest first.
    #[must_use]
    pub fn waiting(&self) -> Vec<Batch> {
        self.held().waiting.clone()
    }

    /// Settles the batch `id` names with `replies`, one per question in order.
    ///
    /// Every reply is checked before anything changes, so a submission is
    /// taken whole or not at all, and a batch refused is still waiting.
    ///
    /// # Errors
    ///
    /// [`Unsettled::Stale`] where no batch by that id is waiting, and another
    /// [`Unsettled`] naming the first reply that is not one the batch can take.
    pub fn settle(&self, id: QuestionId, replies: Vec<Reply>) -> Result<(), Unsettled> {
        let mut held = self.held();
        let Some(at) = held.waiting.iter().position(|batch| batch.id() == id) else {
            return Err(Unsettled::Stale(id));
        };
        if let Some(batch) = held.waiting.get(at) {
            check(batch.questions(), &replies)?;
        }

        let batch = held.waiting.remove(at);
        held.settled.push(Delivered { batch, replies });
        held.changed();
        Ok(())
    }

    /// Whether a settled batch is waiting to be taken.
    #[must_use]
    pub fn any_settled(&self) -> bool {
        !self.held().settled.is_empty()
    }

    /// Takes every settled batch, in the order they were settled.
    ///
    /// A batch taken here is the caller's to deliver: it is never handed out
    /// again.
    pub fn take_settled(&self) -> Vec<Delivered> {
        let mut held = self.held();
        if held.settled.is_empty() {
            return Vec::new();
        }

        let taken = std::mem::take(&mut held.settled);
        held.changed();
        taken
    }

    /// A number that moves on every change, and on nothing else.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.held().revision
    }

    /// The waiting batches, as the value they are saved as.
    ///
    /// The questions only: a settled batch is being delivered, and its replies
    /// are the reader's words, which a saved state does not carry. The number
    /// the next batch would be given goes with them, so that no id this session
    /// handed out before a resume is handed out again after it, to a batch an
    /// old answer naming that id was never meant for. That holds for the
    /// session the state was taken from: another session's state restored after
    /// [`Deferred::clear`] brings ids of its own, and an answer held from before
    /// the clear is for whoever holds it to check against what it was shown.
    #[must_use]
    pub fn state(&self) -> Value {
        let held = self.held();
        let waiting: Vec<Value> = held.waiting.iter().map(saved_batch).collect();
        json!({ "next": held.next, "waiting": waiting })
    }

    /// Replaces the waiting batches with those `state` saved.
    ///
    /// Taken whole or not at all: a batch it cannot read, an id it repeats or
    /// could never have handed out, a batch no person can be asked, or more
    /// than may wait refuses the lot, and nothing changes. An id restored is
    /// never handed out again.
    ///
    /// # Errors
    ///
    /// [`Unrestored`] saying why nothing was restored.
    pub fn restore(&self, state: &Value) -> Result<(), Unrestored> {
        let next = state
            .get("next")
            .and_then(Value::as_u64)
            .ok_or(Unrestored::Malformed(
                "there is no number for the next batch",
            ))?;
        let restored = state
            .get("waiting")
            .and_then(Value::as_array)
            .ok_or(Unrestored::Malformed("there is no list of waiting batches"))?
            .iter()
            .map(read_batch)
            .collect::<Option<Vec<Batch>>>()
            .ok_or(Unrestored::Malformed(
                "a waiting batch is not one that was saved",
            ))?;
        if restored.iter().any(|batch| batch.id().number() == u64::MAX) {
            return Err(Unrestored::Malformed(
                "a waiting batch has an id that is never handed out",
            ));
        }
        for batch in &restored {
            shape(batch.questions()).map_err(Unrestored::Shape)?;
        }

        let mut held = self.held();
        let mut ids: Vec<QuestionId> = restored
            .iter()
            .map(Batch::id)
            .chain(held.settled.iter().map(Delivered::id))
            .collect();
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair.first() == pair.get(1)) {
            return Err(Unrestored::Malformed("two batches have the same id"));
        }

        // Measured against the settled alone: the waiting are being replaced.
        let settled_bytes = held
            .settled
            .iter()
            .map(|done| text_bytes(done.batch.questions()))
            .sum();
        let bytes = restored
            .iter()
            .map(|batch| text_bytes(batch.questions()))
            .sum();
        fits(held.settled.len(), settled_bytes, restored.len(), bytes)?;

        // Below the last number, as checked above, so one more is there.
        let after = restored
            .iter()
            .map(|batch| batch.id().number().saturating_add(1));
        held.next = after.fold(held.next.max(next), u64::max);
        held.waiting = restored;
        held.changed();
        Ok(())
    }

    /// Forgets every batch, waiting or settled.
    pub fn clear(&self) {
        let mut held = self.held();
        if held.waiting.is_empty() && held.settled.is_empty() {
            return;
        }

        held.waiting.clear();
        held.settled.clear();
        held.changed();
    }

    /// The batches, whether or not a thread came apart while holding them.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Held {
    /// Whether `count` more batches carrying `bytes` of question text fit
    /// beside everything already held.
    fn admit(&self, count: usize, bytes: usize) -> Result<(), Room> {
        let held_bytes = self
            .waiting
            .iter()
            .chain(self.settled.iter().map(|done| &done.batch))
            .map(|batch| text_bytes(batch.questions()))
            .sum();
        fits(
            self.waiting.len() + self.settled.len(),
            held_bytes,
            count,
            bytes,
        )
    }

    /// Records that something changed.
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Whether `count` batches carrying `bytes` fit beside `held` carrying
/// `held_bytes`: the one place both bounds are applied.
fn fits(held: usize, held_bytes: usize, count: usize, bytes: usize) -> Result<(), Room> {
    if held.saturating_add(count) > MOST_HELD {
        return Err(Room::Batches);
    }
    if held_bytes.saturating_add(bytes) > MOST_HELD_BYTES {
        return Err(Room::Bytes {
            bytes,
            held: held_bytes,
        });
    }
    Ok(())
}

/// The bytes of text `questions` carry: every heading, question and answer,
/// with what each answer says and shows.
fn text_bytes(questions: &[Question]) -> usize {
    questions
        .iter()
        .map(|question| {
            let answers: usize = question
                .answers()
                .map(|answer| {
                    answer.answer().len()
                        + answer.says().len()
                        + answer.shows().map(str::len).sum::<usize>()
                })
                .sum();
            question.heading().len() + question.question().len() + answers
        })
        .sum()
}

/// Whether `questions` are a batch a person can be asked, naming the first way
/// they are not.
///
/// The text is bounded in bytes where it is held; this bounds the rest, so that
/// a batch of empty strings is no cheaper to hold than it is to show.
fn shape(questions: &[Question]) -> Result<(), Misshapen> {
    if questions.is_empty() {
        return Err(Misshapen::Empty);
    }
    if questions.len() > MOST_QUESTIONS {
        return Err(Misshapen::Questions(questions.len()));
    }

    for (at, question) in questions.iter().enumerate() {
        let at = at + 1;
        let given = question.answers().len();
        if !(FEWEST_ANSWERS..=MOST_ANSWERS).contains(&given) {
            return Err(Misshapen::Answers {
                question: at,
                given,
            });
        }
        if let Some(given) = question
            .answers()
            .map(|answer| answer.shows().len())
            .find(|rows| *rows > MOST_SHOWS)
        {
            return Err(Misshapen::Shows {
                question: at,
                given,
            });
        }
        let names: Vec<&str> = question.answers().map(Answer::answer).collect();
        if repeats(&names) {
            return Err(Misshapen::RepeatedAnswer { question: at });
        }
    }

    let asked: Vec<&str> = questions.iter().map(Question::question).collect();
    if let Some(at) = (1..asked.len()).find(|at| {
        let (earlier, rest) = asked.split_at(*at);
        rest.first().is_some_and(|this| earlier.contains(this))
    }) {
        return Err(Misshapen::RepeatedQuestion { question: at + 1 });
    }
    Ok(())
}

/// Whether any of `texts` appears twice.
fn repeats(texts: &[&str]) -> bool {
    texts
        .iter()
        .enumerate()
        .any(|(index, one)| texts.iter().skip(index + 1).any(|other| other == one))
}

/// Whether `replies` are ones `questions` can take, naming the first that is
/// not.
fn check(questions: &[Question], replies: &[Reply]) -> Result<(), Unsettled> {
    if questions.len() != replies.len() {
        return Err(Unsettled::Count {
            asked: questions.len(),
            replied: replies.len(),
        });
    }

    for (at, (question, reply)) in questions.iter().zip(replies).enumerate() {
        let Reply::Answered(answered) = reply else {
            continue;
        };
        let at = at + 1;
        let chosen: Vec<&str> = answered.chosen().collect();

        if !question.takes_several() && chosen.len() != 1 {
            return Err(Unsettled::OneAnswer {
                question: at,
                given: chosen.len(),
            });
        }
        if repeats(&chosen) {
            return Err(Unsettled::Repeated { question: at });
        }

        let mut written = chosen
            .iter()
            .filter(|one| !question.answers().any(|offered| offered.answer() == **one));
        if let Some(own) = written.next() {
            if written.next().is_some() {
                return Err(Unsettled::WrittenTwice { question: at });
            }
            words(at, Part::WrittenIn, own)?;
        }
        words(at, Part::Note, answered.note())?;
    }
    Ok(())
}

/// Whether the reader's own `text` is within its bound and holds no control
/// character, nor the line and paragraph separators that break a line as one
/// does without being one.
fn words(question: usize, part: Part, text: &str) -> Result<(), Unsettled> {
    if text.len() > MOST_REPLY_BYTES {
        return Err(Unsettled::TooLong {
            question,
            part,
            bytes: text.len(),
        });
    }
    if text
        .chars()
        .any(|one| one.is_control() || matches!(one, '\u{2028}' | '\u{2029}'))
    {
        return Err(Unsettled::Control { question, part });
    }
    Ok(())
}

/// One waiting batch, as it is saved.
fn saved_batch(batch: &Batch) -> Value {
    let questions: Vec<Value> = batch
        .questions()
        .iter()
        .map(|question| {
            let answers: Vec<Value> = question
                .answers()
                .map(|answer| {
                    json!({
                        "answer": answer.answer(),
                        "says": answer.says(),
                        "shows": answer.shows().collect::<Vec<_>>(),
                        "recommended": answer.is_recommended(),
                    })
                })
                .collect();
            json!({
                "heading": question.heading(),
                "question": question.question(),
                "several": question.takes_several(),
                "answers": answers,
            })
        })
        .collect();
    json!({ "id": batch.id().to_string(), "questions": questions })
}

/// One waiting batch read back from what [`saved_batch`] wrote, or `None`
/// where it is not that.
fn read_batch(saved: &Value) -> Option<Batch> {
    let id = QuestionId::parse(saved.get("id")?.as_str()?)?;
    let questions = saved
        .get("questions")?
        .as_array()?
        .iter()
        .map(read_question)
        .collect::<Option<Vec<_>>>()?;
    Some(Batch::new(id, questions))
}

fn read_question(saved: &Value) -> Option<Question> {
    let text = |field: &str| saved.get(field)?.as_str();
    let answers = saved
        .get("answers")?
        .as_array()?
        .iter()
        .map(read_answer)
        .collect::<Option<Vec<_>>>()?;
    // `Question::new` would clear a mark below the first; one `saved_batch`
    // wrote is never there, so it is refused rather than quietly dropped.
    if answers.iter().skip(1).any(Answer::is_recommended) {
        return None;
    }

    let question = Question::new(text("heading")?, text("question")?, answers);
    Some(if saved.get("several")?.as_bool()? {
        question.several()
    } else {
        question
    })
}

fn read_answer(saved: &Value) -> Option<Answer> {
    let shows = saved
        .get("shows")?
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;

    let answer = Answer::new(saved.get("answer")?.as_str()?)
        .saying(saved.get("says")?.as_str()?)
        .showing(shows);
    Some(if saved.get("recommended")?.as_bool()? {
        answer.recommending()
    } else {
        answer
    })
}

impl Delivered {
    /// What names the batch.
    #[must_use]
    pub fn id(&self) -> QuestionId {
        self.batch.id()
    }

    /// What the model is told: the one place an answer becomes words.
    ///
    /// It names the batch, then each question by its heading with its reply.
    /// A declined question reads as unresolved and names none of the answers
    /// it offered — not even the one the asker recommended — so that declining
    /// is never read as agreeing to the default. A written-in answer goes on a
    /// line of its own, apart from the answers offered, so that one holding a
    /// comma never reads as two of them.
    #[must_use]
    pub fn said(&self) -> String {
        let mut text = format!(
            "The person answered question batch {}, asked earlier without waiting:\n",
            self.id()
        );

        for (question, reply) in self.batch.questions().iter().zip(&self.replies) {
            let heading = question.heading();
            match reply {
                Reply::Answered(answered) => {
                    let (offered, own): (Vec<&str>, Vec<&str>) = answered
                        .chosen()
                        .partition(|one| question.answers().any(|it| it.answer() == *one));
                    let offered = if !offered.is_empty() {
                        offered.join(", ")
                    } else if own.is_empty() {
                        "nothing chosen".to_owned()
                    } else {
                        "none of the answers offered".to_owned()
                    };
                    let _ = writeln!(text, "- {heading}: {offered}");
                    for own in own {
                        let _ = writeln!(text, "  their own answer: {own}");
                    }
                    if !answered.note().is_empty() {
                        let _ = writeln!(text, "  they added: {}", answered.note());
                    }
                }
                Reply::Declined => {
                    let _ = writeln!(
                        text,
                        "- {heading}: unresolved; they declined to decide it, so assume none of \
                         the answers offered"
                    );
                }
            }
        }
        text
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Note => "note",
            Self::WrittenIn => "written-in answer",
        })
    }
}

/// By hand: the batches are the model's questions and the reader's answers.
impl fmt::Debug for Deferred {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let held = self.held();
        write!(
            f,
            "Deferred({} waiting, {} settled, redacted)",
            held.waiting.len(),
            held.settled.len()
        )
    }
}

/// By hand, for the reason [`Deferred`]'s is.
impl fmt::Debug for Delivered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Delivered({}, {} replies redacted)",
            self.id(),
            self.replies.len()
        )
    }
}

#[cfg(test)]
mod tests;
