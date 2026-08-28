//! Putting a question to the reader, and reading what comes back.
//!
//! The moment in the middle of a turn: the worker has stopped on a permission
//! question or an ask, and this thread — the one holding the terminal — draws it
//! and waits for a key. The loop above owns everything either side of that; this
//! owns the pause itself.
//!
//! Two shapes for one question, decided by the room and the keyboard. A panel
//! where there is both, and a row at a time where there is not, which is what a
//! redirected run and a four-row window both get. Neither is a fallback for a
//! failure: a window too small to stand a panel in is not somebody saying no, so
//! the cramped path answers the same question rather than refusing it.
//!
//! Every key that can arrive while a question stands is named rather than caught
//! by a rest arm, in [`Heard`] and in [`Numbered`]. A key is an answer, or news
//! the question has to act on — the window changed, the wheel turned — or
//! something this has decided to ignore; and a key added to `Pressed` has to be
//! decided about here rather than silently join the third group. Both are read
//! apart from the loops that drive them, because those loops read the process's
//! own keyboard and a test cannot drive them; this much of the reading it can.

use std::io::{self, BufRead};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel};

use crucible_core::{
    Answer as Chosen, Answered, Question, Remember, Sensitivity, ToolCall, Verdict,
};
use crucible_tui::{Key, Pressed, Renderer, Terminal};

use super::super::Fatal;
use super::super::draw;
use super::super::seen::{Answer, Given, Putting, Seen};
use super::super::style::Style;
use super::{QUEUED_BYTES, asking};

/// How a permission question gets answered.
pub(super) struct Answers<'a> {
    /// Standard input, for a session with no terminal to read keys from.
    pub(super) input: &'a mut dyn BufRead,
    /// Whether keys are being read rather than lines.
    pub(super) keys: bool,
}

/// Where the two kinds of answer go back.
///
/// Two channels rather than one carrying both: a verdict and an ask's answers
/// are different types, and one channel would need a union nothing keeps apart.
/// One value rather than two arguments, because they are made together, handed
/// over together, and neither is ever the other's.
pub(super) struct Answering {
    /// Where a permission verdict goes.
    pub(super) reply: Sender<Answer>,
    /// Where an ask's answers go.
    pub(super) give: Sender<Given>,
}

impl Answering {
    /// Both reply channels for one turn, with the ask's end already lent to
    /// whoever asks.
    ///
    /// Fresh for each turn, because a reply channel that outlived its turn could
    /// hand the next question an answer meant for the last one. Making the
    /// channel and lending its far end are one act here, so there is no state in
    /// between where one exists and the other has not been given away.
    pub(super) fn new(putting: &Putting, post: &SyncSender<Seen>) -> (Self, Receiver<Answer>) {
        let (reply, hear) = channel();
        let (give, given) = channel();
        putting.open(post.clone(), given);

        (Self { reply, give }, hear)
    }
}

/// One answer to one question.
///
/// A key where there is a keyboard, because raw mode is held for the whole
/// session now and a line-reading terminal is not collecting one. The letter is
/// written out afterwards, since nothing echoed it: an answer that left no mark
/// would leave the record showing a question and no reply.
///
/// This is the one place in the session that waits on a key with no clock on
/// it: the question stands until somebody decides. So it is also the longest a
/// window can change without anybody noticing, which is why the key that says
/// it did is acted on here rather than passed over with the arrows.
pub(super) fn answered<T: Terminal>(
    renderer: &mut Renderer<T>,
    answers: &mut Answers<'_>,
) -> Result<Answer, Fatal> {
    if !answers.keys {
        return Ok(verdict(read(answers.input)?.as_deref()));
    }

    loop {
        let Some(arrived) = renderer.pressed()? else {
            continue;
        };
        let said = match heard(arrived) {
            Heard::Said(said) => said,

            // The renderer is holding a size the screen no longer has, and
            // every band it shares the window out into is measured against it.
            // Taking the new one is the whole of the answer: the question is
            // committed, so its rows are folded again at the width the window
            // has now, and the frame that follows puts them back.
            Heard::Resized => {
                renderer.resized()?;
                continue;
            }

            // The question is committed too, which is what makes the wheel
            // worth answering here: a reader deciding whether to allow a call
            // is reading what was said above it to decide.
            Heard::Scrolled { back } => {
                renderer.notched(back)?;
                continue;
            }

            Heard::Ignored => continue,
        };

        draw::answered(renderer, &said)?;
        return Ok(verdict(Some(&said)));
    }
}

/// What one key pressed at a question means.
///
/// Three answers rather than two, and the third is the whole reason this is a
/// function: a key that is not an answer is not therefore nothing. Written apart
/// from the loop above because that loop cannot be driven from a test — the
/// keyboard it reads is the process's own — and this much of the reading can be.
pub(super) enum Heard {
    /// What was typed, to be read as a verdict. Empty is a refusal.
    Said(String),
    /// The window changed under the question.
    Resized,
    /// The wheel turned, so the transcript the question stands at the foot of
    /// moves. `back` is towards the top of the session.
    Scrolled {
        /// Whether the notch was towards the top of the session.
        back: bool,
    },
    /// Not an answer and not news. Wait for the next key.
    Ignored,
}

/// Reads one key as an answer, as news, or as neither.
///
/// Anything that is not one of the letters is a refusal, which is what an
/// unrecognised line already meant. Escape and Ctrl-C are spelled out among
/// them so that the way out of a question is the way out of everything else.
///
/// Every remaining key is named rather than caught by a rest arm. A key that
/// arrives while a permission question is on screen is either an answer or
/// something this has decided to ignore, and a new one added to `Pressed` must
/// be decided about here rather than silently join the second group.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn heard(arrived: Pressed) -> Heard {
    match arrived {
        Pressed::Key(Key::Char(letter)) => Heard::Said(letter.to_string()),
        Pressed::Key(Key::Interrupt | Key::Eof | Key::Enter) | Pressed::Escape => {
            Heard::Said(String::new())
        }

        Pressed::Resized => Heard::Resized,
        Pressed::Scrolled { back } => Heard::Scrolled { back },

        // An arrow, a click, a mode step, a key that means nothing here — the key
        // about what is already running among them, since this question is what
        // decides whether the command runs at all. None of them is an answer, and
        // none of them may be read as one.
        //
        // Ctrl+E is here for now rather than because it belongs here: the
        // question joins the transcript a row at a time, and there is no second
        // shape of it to toggle into until the panel is what draws it.
        //
        // Ctrl+O is here because this is the question with nowhere to stand: it
        // was put a row at a time precisely because there was no room for a
        // panel, and a view of what was cut needs more room than the panel did.
        //
        // Ctrl+T for the same reason as Ctrl+E: the question took the rows the
        // plan stands in, so there is no panel on screen for the key to open.
        //
        // And Ctrl+Y, which copies the line in the box: this question is what
        // the reader is answering, and no line is being typed behind it.
        //
        // And the key that crosses regions, which needs regions to cross: this
        // question was put a row at a time precisely because there was no room
        // for a panel, and a row at a time is not divided into anything.
        Pressed::Key(_)
        | Pressed::Up
        | Pressed::Down
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Tab
        | Pressed::Explain
        | Pressed::Expand
        | Pressed::Plan
        | Pressed::Pasted(_)
        | Pressed::Clicked { .. }
        | Pressed::Queue
        | Pressed::Copy
        | Pressed::PasteImage
        | Pressed::Rename
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
        | Pressed::Ignored => Heard::Ignored,
    }
}
/// Puts one question and waits for its answer.
///
/// The panel where there is a keyboard and room to stand one; otherwise the
/// question a row at a time, which is what a redirected run and a window with
/// four rows both get.
pub(super) fn asked<T: Terminal>(
    renderer: &mut Renderer<T>,
    call: &ToolCall,
    sensitivity: &Sensitivity,
    answers: &mut Answers<'_>,
    style: Style,
) -> Result<Answer, Fatal> {
    if answers.keys {
        // Read here rather than carried from the worker, because the panel is
        // the only thing that shows it: the row a call gets in the transcript
        // is drawn from what the tool made of the arguments, and this is the
        // model's own sentence about them.
        let account = crucible_tools::account(&call.args);

        // Nothing is drawn under it. The panel stood over the transcript and
        // the rows it covered are back, so a call that was allowed reads exactly
        // like one nothing asked about — which is what the reader is looking at
        // anyway, since the result of the call is the row underneath.
        match asking::ask(renderer, style, call, sensitivity, &account)? {
            asking::Answered::Said(answer) => return Ok(answer),

            // Nothing was drawn and no key was read, so the question still has
            // to be put. Falling through rather than refusing: a window this
            // small is not somebody saying no.
            asking::Answered::Cramped => {}
        }
    }

    draw::question(renderer, call, sensitivity, style)?;
    answered(renderer, answers)
}

/// Puts the questions a row at a time, where there was no room to stand a panel.
///
/// One key per question, which is what a window this small can offer: the panel
/// adds an answer somebody writes themselves, and writing one wants a line
/// editor and the rows to draw it in. Anything that is not one of the numbers
/// leaves the whole ask, the way escape does on the panel.
pub(super) fn cramped<T: Terminal>(
    renderer: &mut Renderer<T>,
    questions: &[Question],
    style: Style,
) -> Result<Given, Fatal> {
    let mut given = Vec::new();

    for (at, question) in questions.iter().enumerate() {
        draw::asking(renderer, question, at, questions.len(), style)?;

        let Some(taken) = took(renderer, question)? else {
            return Ok(None);
        };

        draw::answered(renderer, taken.answer())?;
        given.push(Answered::new([taken.answer()]));
    }

    Ok(Some(given))
}

/// Reads one key as one of `question`'s answers, or nothing where it means to
/// leave.
pub(super) fn took<T: Terminal>(
    renderer: &mut Renderer<T>,
    question: &Question,
) -> Result<Option<Chosen>, Fatal> {
    loop {
        let Some(arrived) = renderer.pressed()? else {
            continue;
        };

        match numbered(arrived, question) {
            Numbered::Chose(answer) => return Ok(Some(answer)),
            Numbered::Left => return Ok(None),

            // The renderer is holding a size the screen no longer has, and the
            // question after this one would be drawn against it — a row folded
            // for a window that has gone. Taking the new size is the whole of
            // the answer, the same as it is one question above: the rows already
            // committed are folded again at the width the window has now, and
            // the next frame puts them back.
            Numbered::Resized => renderer.resized()?,
        }
    }
}

/// What one key pressed at a question put a row at a time means.
///
/// Three answers for the reason [`Heard`] has three: a key that is not one of
/// the numbers is not therefore nothing, and the one that says the window
/// changed has to be told apart from the one that says leave. Written apart
/// from the loop above because that loop cannot be driven from a test — the
/// keyboard it reads is the process's own — and this much of the reading can
/// be.
pub(super) enum Numbered {
    /// The answer whose number was typed.
    Chose(Chosen),
    /// The window changed under the question.
    Resized,
    /// Anything else, which leaves the whole ask.
    Left,
}

/// Reads one key as one of `question`'s numbered answers.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn numbered(arrived: Pressed, question: &Question) -> Numbered {
    if arrived == Pressed::Resized {
        return Numbered::Resized;
    }

    let Pressed::Key(Key::Char(typed)) = arrived else {
        return Numbered::Left;
    };

    let at = usize::try_from(typed.to_digit(10).unwrap_or(0))
        .ok()
        .and_then(|digit| digit.checked_sub(1));

    at.and_then(|at| question.answers().nth(at))
        .map_or(Numbered::Left, |answer| Numbered::Chose(answer.clone()))
}

/// Reads one bounded line, or `None` at end of input.
///
/// `fill_buf` exposes each source block before any of it is copied, so a pipe
/// with no newline can cross the prompt bound without making this process
/// retain the rest first.
pub(super) fn read(input: &mut dyn BufRead) -> Result<Option<String>, Fatal> {
    let mut line = Vec::new();

    loop {
        let available = input.fill_buf().map_err(Fatal::Input)?;
        if available.is_empty() {
            break;
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |at| at + 1);
        if take > QUEUED_BYTES.saturating_sub(line.len()) {
            return Err(Fatal::InputTooLong);
        }

        line.extend_from_slice(available.get(..take).unwrap_or_default());
        input.consume(take);
        if newline.is_some() {
            break;
        }
    }

    if line.is_empty() {
        return Ok(None);
    }

    String::from_utf8(line)
        .map(Some)
        .map_err(|problem| Fatal::Input(io::Error::new(io::ErrorKind::InvalidData, problem)))
}

/// What an answer to a permission question means.
///
/// Anything unrecognised is a refusal, and so is end of input. Every way to say
/// yes is explicit; everything else, including a typo and a closed pipe, leaves
/// the tool unrun.
///
/// Durable rules have no trusted per-workspace store yet, so `always` is not an
/// answer. Treating it as a session answer would promise more than was kept.
pub(super) fn verdict(answer: Option<&str>) -> Answer {
    match answer.map(str::trim) {
        Some("y" | "Y" | "yes") => (Verdict::Allow, Remember::Never),
        Some("s" | "S" | "session") => (Verdict::Allow, Remember::Session),
        _ => (Verdict::Deny, Remember::Never),
    }
}
