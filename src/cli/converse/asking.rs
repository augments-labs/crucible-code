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
//! **What the model wrote arrives with the call, because there is nobody to ask
//! for it.** The thread standing this panel sent the question and is blocked on
//! the answer; it has no provider behind it, so ctrl+e cannot go and fetch an
//! explanation when it is pressed. The paragraphs are an argument the schema
//! invites, they travel beside the command, and this is the layer that decides
//! whether they are on screen. A call that sent none gets the panel there was
//! before any of this existed, footer included — a key named where it does
//! nothing is worse than a key nobody was offered.
//!
//! **And they are named as the model's.** Every other row here is written by
//! crucible out of what a tool read from the arguments; these are the words of
//! the thing asking for the permission. The panel says so above them, once, in
//! its own voice.
//!
//! **Nothing here is written down, the answer included.** The panel stood over
//! the transcript and the rows it covered are back, and what a yes leaves behind
//! is the call's own result on the row under it — which is on screen anyway, and
//! is the thing the reader is looking for. A row saying the answer sits between
//! that result and the one before it, in a column of calls that is read by its
//! marks, and says nothing the next row does not.
//!
//! A no leaves its own trace for the same reason: the refusal comes back as
//! that call's result and is drawn as one. So the record is the same shape
//! whichever way it went, and it is the shape it has when nothing was asked at
//! all.

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

/// The quiet row under the frame, where the call explained nothing.
///
/// One key, because escape is the only one this panel offers that is not on it.
/// The arrows and enter are its own picture, and a footer naming them would be
/// restating the thing above it.
const CANCEL: &str = "esc to cancel";

/// The same row where there are paragraphs to open.
const EXPLAIN: &str = "esc to cancel · ctrl+e to explain";

/// And where they are open.
///
/// Each of the three is written out whole rather than joined from parts, because
/// what a footer costs is read off the row a person sees and not off the pieces
/// it was assembled from — and there is no joining a `&'static str` without
/// allocating on the layout path anyway.
const HIDE: &str = "esc to cancel · ctrl+e to hide";

/// The item the panel adds to whichever of those it was given, where the prose
/// did not fit. Named here and drawn there, because the panel is the only party
/// that knows how tall the paragraphs folded to.
const MORE: &str = "↑↓ to see more";

/// Rows kept for the transcript above a panel that arrived on its own.
///
/// Nobody asked for this box, so it leaves the last exchange visible behind it,
/// and a panel with no room under that reserve is one this cannot stand.
const KEPT: usize = 4;

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answered {
    /// This is what was decided.
    ///
    /// The words it was decided in stay on the panel that offered them. They
    /// were carried out of here once, to be written under the call, and what
    /// that put in the record was a row between a call and its result saying
    /// something the result already says.
    Said(Answer),
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
    attribution: String,
    explanation: Vec<String>,
    statement: &'static str,
}

impl Words {
    /// What to say about this call.
    ///
    /// The subject, the payload and the statement are worked out from what the
    /// tool read out of the arguments. The description and the explanation are
    /// the model's own words, and they are flattened for the reason the payload
    /// is. The description is why the caption sits on the command rather than
    /// standing on its own — what is agreed to is above it and was not written
    /// by the same author — and the attribution is that same argument made out
    /// loud, because a page of prose is too much to caption.
    fn of(call: &ToolCall, sensitivity: &Sensitivity, account: &Account) -> Self {
        let name = pascal(&call.name);

        let (subject, payload, statement) = match sensitivity {
            // Never reached through the permission engine, which allows or
            // refuses a read and never asks about one. Worded anyway, so a tool
            // reclassified later has a question rather than a blank panel.
            Sensitivity::ReadOnly { target } => (
                format!("{name} file"),
                flattened(target),
                "This read needs your verdict.",
            ),

            Sensitivity::MutatesFile { target } => (
                format!("{name} file"),
                flattened(target),
                "This change needs your verdict.",
            ),

            // The line as it was sent, never the spelling a rule is matched
            // against: the operators are the difference between two commands
            // and two commands *if the first one worked*, and that difference
            // is part of what is being agreed to.
            // Two statements for one sensitivity, because a command that will
            // outlive the turn is a different thing to consent to: allowing it is
            // allowing a process to go on running after the answer that started it
            // has been given, and a panel that did not say so would be asking
            // about the wrong thing.
            Sensitivity::SpawnsProcess { command } => (
                format!("{name} command"),
                flattened(command.sent()),
                if crucible_tools::backgrounded(&call.args) {
                    "This command needs your verdict. It will be left running \
                     after the turn that starts it has ended."
                } else {
                    "This command needs your verdict."
                },
            ),

            // What is sent, never the host a rule is matched against. The host
            // is what standing policy is written about; this is the thing that
            // actually leaves — the address for a fetch, the query for a
            // search — and a panel showing only where it went would be asking
            // about a request it never quoted.
            Sensitivity::ReachesNetwork { host } => (
                format!("{name} to {host}"),
                flattened(host.sent()),
                "This request needs your verdict. It leaves your machine.",
            ),
        };

        Self {
            subject,
            payload,
            description: flattened(account.description()),
            // The name the call went out under rather than the one the subject
            // shows, because this row is about who wrote the paragraphs and the
            // model wrote them under this spelling.
            attribution: format!("{}'s own account of this call:", call.name),
            explanation: account.explanation().map(flattened).collect(),
            statement,
        }
    }
}

/// What a standing panel keeps between frames.
///
/// The mark alone was enough until the prose could be opened. Now there are four
/// things, and the reason they are one value is that two of them are written in
/// different places: the keys move the mark and the window, and the layout — the
/// only party that knows how tall the paragraphs folded to at this width — is
/// what says how far the window may go.
struct Standing {
    /// Which answer the mark stands on.
    marked: usize,
    /// Whether this call arrived with paragraphs at all.
    ///
    /// Fixed for the life of the panel: it is a fact about the call rather than
    /// about the reading, and it is what keeps ctrl+e from being offered where
    /// there is nothing for it to open.
    explained: bool,
    /// Whether they are open.
    open: bool,
    /// Which row of the prose the window opens at.
    from: usize,
    /// The furthest down it can open, at the size the last frame was laid out
    /// at. Written by the layout and read by the keys, which is what keeps a
    /// held arrow from spending a frame going nowhere.
    end: usize,
}

impl Standing {
    /// A panel about a call that said this much, with the mark on the first
    /// answer and the prose closed.
    fn new(explained: bool) -> Self {
        Self {
            marked: 0,
            explained,
            open: false,
            from: 0,
            end: 0,
        }
    }

    /// Whether the arrows are moving the window rather than the mark.
    ///
    /// One pair of keys, doing whichever job the picture is asking about: prose
    /// that is open and was cut is asking to be read, and anything else on this
    /// panel is asking to be answered. Where the prose is open and fitted there
    /// is nothing to scroll, so the arrows go back to the answers rather than
    /// becoming keys that do nothing.
    fn scrolling(&self) -> bool {
        self.open && self.end > 0
    }

    /// Opens the paragraphs, or closes them.
    ///
    /// The window goes back to the top either way, and by itself: the offset is
    /// clamped every frame against the prose being drawn, and while it is closed
    /// there is none. Which is also what pressing *explain* a second time means.
    fn opened(&mut self) -> Moved {
        if !self.explained {
            return Moved::Still;
        }

        self.open = !self.open;
        Moved::Redraw
    }

    /// The key that goes up, at whichever of the two it is reading.
    fn up(&mut self) -> Moved {
        if self.scrolling() {
            let next = self.from.checked_sub(1);
            step(&mut self.from, next)
        } else {
            let next = self.marked.checked_sub(1);
            step(&mut self.marked, next)
        }
    }

    /// And the key that goes down, which is the one that stops somewhere the
    /// layout worked out rather than somewhere this module knows.
    fn down(&mut self) -> Moved {
        if self.scrolling() {
            let next = Some(self.from + 1).filter(|next| *next <= self.end);
            step(&mut self.from, next)
        } else {
            let next = Some(self.marked + 1).filter(|next| *next < ANSWERS.len());
            step(&mut self.marked, next)
        }
    }
}

/// The quiet row for a panel in this state.
///
/// A key is named only where it does something, which is the whole of why this
/// is a function of the panel rather than a constant: ctrl+e on a call that
/// explained nothing is a key that was offered and then ignored.
fn footer(standing: &Standing) -> &'static str {
    match (standing.explained, standing.open) {
        (false, _) => CANCEL,
        (true, false) => EXPLAIN,
        (true, true) => HIDE,
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
    let told: Vec<&str> = words.explanation.iter().map(String::as_str).collect();

    let mut question = Question {
        subject: &words.subject,
        payload: &payload,
        description: &words.description,
        attribution: "",
        explanation: &[],
        from: 0,
        // Handed over whatever the paragraphs did, because whether they were cut
        // is the panel's to know and it draws this only where they were.
        more: MORE,
        statement: words.statement,
        question: QUESTION,
        answers: &said,
        marked: 0,
        footer: CANCEL,
    };

    let mut standing = Standing::new(!told.is_empty());

    let ended = region::stand(
        renderer,
        |_| style,
        &mut standing,
        |standing, columns, rows| {
            let room = rows.saturating_sub(KEPT);
            let glyphs = style.glyphs();

            question.marked = standing.marked;
            question.footer = footer(standing);

            // The two turn together. The attribution is a row *about* the
            // paragraphs, so a frame with one and not the other is either a
            // claim about nothing or paragraphs with nobody's name on them.
            question.attribution = if standing.open {
                &words.attribution
            } else {
                ""
            };
            question.explanation = if standing.open { told.as_slice() } else { &[] };

            // Asked at this size and answered before the picture is laid out, so
            // the next key acts on a window the frame in front of it agreed with.
            // The clamp is what closing the prose is made of as well: there is
            // none to open a window on, so the end is zero and so is the offset.
            standing.end = question.end(columns, room, glyphs);
            standing.from = standing.from.min(standing.end);
            question.from = standing.from;

            (question.within(columns, room, glyphs), None)
        },
        moving,
    )?;

    Ok(answered(ended, standing.marked))
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
            let (_, answer) = ANSWERS.get(at).copied().unwrap_or(DENIED);
            Answered::Said(answer)
        }
        Ended::Left => Answered::Said(DENIED.1),
        Ended::Cramped => Answered::Cramped,
    }
}

/// What `arrived` does to a standing panel.
///
/// Every key is named rather than caught by a rest arm: a key at a permission
/// question is either an answer or something decided to ignore, and a new
/// [`Pressed`] must be decided about here rather than quietly join the second
/// group.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn moving(arrived: Pressed, standing: &mut Standing) -> Moved {
    match arrived {
        Pressed::Up => standing.up(),
        Pressed::Down => standing.down(),
        Pressed::Explain => standing.opened(),

        Pressed::Key(Key::Char(typed)) => and_took(&mut standing.marked, numbered(typed)),
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,

        // The wheel among them: three answers are reached with an arrow, and
        // what a reader turning a wheel at a permission question is doing is
        // reading back through what was said above it to decide. Answering
        // `Still` is what hands the wheel to the transcript.
        //
        // Ctrl+O among them. What it opens is the transcript's, and the panel
        // stands over the transcript: the view would take the region this panel
        // is in, leaving a question on screen with nothing left to answer it
        // with. Ctrl+T is the same sentence about the plan, which stands in that
        // region too and is not on screen while this question is.
        // The key about what is already running among them: this question is what
        // decides whether the command runs at all, so there is nothing here for it
        // to act on. And the key that copies the line, because the line it copies
        // is in a box nobody is typing into while this is on screen.
        Pressed::Key(_)
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Expand
        | Pressed::Plan
        | Pressed::Clicked { .. }
        | Pressed::Pasted(_)
        | Pressed::Queue
        | Pressed::Copy
        | Pressed::PasteImage
        | Pressed::Scrolled { .. }
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
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
