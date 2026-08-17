//! The panel a call waits behind, and the keys that answer it.
//!
//! `crucible-tui` draws the box and knows none of the words in it — it depends
//! on nothing and cannot be told what a `Sensitivity` is. So this is where a
//! call becomes a question: the subject over the frame, the command or the path
//! inside it, the sentence saying why this stopped, and the three answers.
//!
//! The wording is the half that decides what somebody consents to, so it is
//! worked out here rather than inside the loop, which no test can drive.
//!
//! The panel is not written down. What belongs in the record is the answer,
//! which is committed by the caller in the same shape whichever way the
//! question was asked: a window with no room for a panel still asks, a row at a
//! time, and a transcript should not say which of the two happened.

use crucible_core::{Account, Remember, Sensitivity, ToolCall, Verdict};
use crucible_tui::{Key, Pressed, Question, Renderer, Terminal};

use crate::cli::Fatal;
use crate::cli::draw::{flattened, pascal};
use crate::cli::seen::Answer;
use crate::cli::style::Style;

use super::region::{self, Ended, Moved, step};

/// The answers, in the order they are numbered, and what each one means.
///
/// `always` is not among them: durable rules have no trusted per-workspace
/// store yet, so this offers only answers whose lifetime it can honour.
const ANSWERS: [(&str, Answer); 3] = [
    ("Yes, once", (Verdict::Allow, Remember::Never)),
    (
        "Yes, and don't ask again this session",
        (Verdict::Allow, Remember::Session),
    ),
    DENIED,
];

/// The refusal, spelled once.
///
/// It is the third answer and it is also what escape means. Two spellings would
/// be two to keep in step, and the one that drifted would be a denial that read
/// as an allow.
const DENIED: (&str, Answer) = ("No, and end the turn", (Verdict::Deny, Remember::Never));

/// The question under the answers.
const QUESTION: &str = "Do you want to proceed?";

/// The quiet row under the frame.
///
/// One key, because escape is the only one this panel offers that is not on it.
/// The arrows and enter are its own picture, and a footer naming them would be
/// restating the thing above it.
const FOOTER: &str = "esc to cancel";

/// Rows kept for the transcript above a panel that arrived on its own.
///
/// Nobody asked for this box, so it leaves the last exchange visible behind it,
/// and a panel with no room under that reserve is one this cannot stand.
const KEPT: usize = 4;

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answered {
    /// This is what was decided, and these are the words it was decided in.
    Said(Answer, &'static str),
    /// There was no room to stand a panel. Nothing was drawn and no key was
    /// read, so the caller still owes an answer.
    Cramped,
}

/// The wording of one question, worked out from the call it is about.
///
/// Owned rather than borrowed because every field is built here: the subject is
/// two words joined and the payload is the model's own text, flattened.
struct Words {
    subject: String,
    payload: String,
    description: String,
    statement: &'static str,
}

impl Words {
    /// What to say about this call.
    ///
    /// Everything but the description is worked out from what the tool read out
    /// of the arguments; the description is the model's own sentence, and it is
    /// flattened for the reason the payload is. It is the one row here written
    /// by the thing being consented to, which is why it is a caption on the
    /// command rather than a line standing on its own — what is agreed to sits
    /// above it and was not written by the same author.
    fn of(call: &ToolCall, sensitivity: &Sensitivity, account: &Account) -> Self {
        let name = pascal(&call.name);
        let description = flattened(account.description());

        match sensitivity {
            // Never reached through the permission engine, which allows or
            // refuses a read and never asks about one. Worded anyway, so a tool
            // reclassified later has a question rather than a blank panel.
            Sensitivity::ReadOnly { target } => Self {
                subject: format!("{name} file"),
                payload: flattened(target),
                description,
                statement: "This read needs your verdict.",
            },

            Sensitivity::MutatesFile { target } => Self {
                subject: format!("{name} file"),
                payload: flattened(target),
                description,
                statement: "This change needs your verdict.",
            },

            // The line as it was sent, never the spelling a rule is matched
            // against: the operators are the difference between two commands
            // and two commands *if the first one worked*, and that difference
            // is part of what is being agreed to.
            Sensitivity::SpawnsProcess { command } => Self {
                subject: format!("{name} command"),
                payload: flattened(command.sent()),
                description,
                statement: "This command needs your verdict.",
            },
        }
    }
}

/// Stands a panel where the prompt box was and reads keys until it is answered.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    call: &ToolCall,
    sensitivity: &Sensitivity,
    account: &Account,
) -> Result<Answered, Fatal> {
    let words = Words::of(call, sensitivity, account);

    // One entry, because a command is one thing being agreed to however many
    // rows it folds to. The slice is for a call that would show two, and none
    // does yet.
    let payload = [words.payload.as_str()];
    let said = ANSWERS.map(|(said, _)| said);

    let mut question = Question {
        subject: &words.subject,
        payload: &payload,
        description: &words.description,
        explanation: &[],
        from: 0,
        more: "",
        statement: words.statement,
        question: QUESTION,
        answers: &said,
        marked: 0,
        footer: FOOTER,
    };

    let mut at = 0;

    let ended = region::stand(
        renderer,
        style,
        &mut at,
        |marked, columns, rows| {
            question.marked = marked;
            question.within(columns, rows.saturating_sub(KEPT), style.glyphs())
        },
        moving,
    )?;

    Ok(answered(ended, at))
}

/// What a panel that ended this way decided.
///
/// Left is the refusal: every way to say yes is a key somebody pressed on
/// purpose, so every other way out arrives here. The mark cannot miss — only
/// keys that checked it against the answers move it — and a miss falls to the
/// refusal for the same reason.
fn answered(ended: Ended, at: usize) -> Answered {
    match ended {
        Ended::Took => {
            let (said, answer) = ANSWERS.get(at).copied().unwrap_or(DENIED);
            Answered::Said(answer, said)
        }
        Ended::Left => Answered::Said(DENIED.1, DENIED.0),
        Ended::Cramped => Answered::Cramped,
    }
}

/// What `arrived` does to a standing panel with `at` marked.
///
/// Every key is named rather than caught by a rest arm: a key at a permission
/// question is either an answer or something decided to ignore, and a new
/// [`Pressed`] must be decided about here rather than quietly join the second
/// group. Ctrl+E is among the ignored for now — nothing on this panel opens,
/// because nothing yet arrives with the call for it to open.
fn moving(arrived: Pressed, at: &mut usize) -> Moved {
    match arrived {
        Pressed::Up => step(at, at.checked_sub(1)),
        Pressed::Down => step(at, Some(*at + 1).filter(|next| *next < ANSWERS.len())),

        Pressed::Key(Key::Char(typed)) => and_took(at, numbered(typed)),
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,

        Pressed::Key(_)
        | Pressed::Cycle
        | Pressed::Explain
        | Pressed::Clicked { .. }
        | Pressed::Ignored => Moved::Still,
    }
}

/// The answer a typed character is the number of, if it is the number of one.
///
/// The numbers are drawn on the answers by the panel, which is what makes them
/// keys: a picture that numbers three things and then ignores the numbers is
/// one that promised something. They start at one because that is what is
/// drawn, so `0` names nothing, and so does a digit past the last answer.
fn numbered(typed: char) -> Option<usize> {
    let at = usize::try_from(typed.to_digit(10)?).ok()?.checked_sub(1)?;

    (at < ANSWERS.len()).then_some(at)
}

/// Moves the mark to `next` and takes what it now stands on.
///
/// In that order, so the frame the panel leaves behind has the mark on what was
/// taken. A key naming nothing moves nothing and takes nothing — taking what
/// the mark happened to be standing on would make a mistyped key an answer.
fn and_took(at: &mut usize, next: Option<usize>) -> Moved {
    match next {
        Some(next) => {
            *at = next;
            Moved::Took
        }
        None => Moved::Still,
    }
}

#[cfg(test)]
mod tests;
