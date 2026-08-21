//! The panel a model's questions stand in, and the keys that answer them.
//!
//! `crucible-tui` draws the rows and knows none of the words in them, so this is
//! where a call becomes something to answer: the questions across the top, the
//! answers under them, and the two this program adds to every question that the
//! call never wrote.
//!
//! **Two axes, and each pair of arrows is named under the thing it moves.** Up
//! and down walk the answers; left and right step the questions. Both stop at
//! each end rather than wrapping, so the key that went too far is not the key
//! that goes further.
//!
//! **Enter always means the same thing: take what is marked and move on.** On
//! the last stop there is nowhere to move on to, so it sends. An ask of one
//! question has no last stop, and enter on it sends for the same reason — a
//! screen reading back one answer says what the screen before it said.
//!
//! **A question is remembered as it was left.** Stepping back to one already
//! answered finds the mark where it was and the answers still chosen, because
//! going back to check is the reason the arrows go both ways.
//!
//! **While a line is being written, the keys belong to the line.** A letter is a
//! character rather than a command, and the footer says so. Escape then stops
//! the writing and keeps what was typed; it is only the whole ask that a second
//! escape leaves.
//!
//! Nothing here is written down. The panel stood over the transcript and the
//! rows it covered are back; what is left behind is the call's own result on the
//! row under it, which is the bargain the permission panel already makes.

use crucible_core::{Answer, Answered, Question};
use crucible_tui::{
    Asked, Caret, Choice, Editor, Given, Key, Pressed, Renderer, Row, Stop, Terminal, Typed,
    Writing,
};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::region::{self, Ended, Moved, step};

#[cfg(test)]
mod tests;

/// The subject over an ask of several questions, and of one.
const SUBJECT: &str = "Questions for you";
const ALONE: &str = "Question for you";

/// The answer this program adds to every question, so somebody can write one
/// nobody offered.
const ELSE: &str = "Something else";

/// And the one under the rule, which answers the whole ask rather than the
/// question above it. Escape means exactly this, the way the permission panel's
/// refusal is both its third answer and what escape means.
const LEAVE: &str = "Say it in the prompt instead";

/// What the last stop is called, and what it says above the answers read back.
const REVIEW: &str = "Review";
const SENDING: &str = "These are the answers that go back:";
const SEND: &str = "Send them?";

/// The two answers the last stop offers.
const SENT: [&str; 2] = ["Send", "Cancel"];

/// What the last stop says a question was left with.
const NOTHING: &str = "nothing chosen";

/// Rows kept for the transcript above a panel nobody asked for.
const KEPT: usize = 4;

/// The quiet row under the frame, in each state it means something different
/// in. Written out whole rather than joined, because what a footer costs is
/// read off the row somebody sees and not off the pieces it came from.
const ONE: &str = "esc to cancel · ←→ between questions · n for a note";
const MANY: &str = "esc to cancel · space to choose · ←→ between questions · n for a note";
const ONLY: &str = "esc to cancel · n for a note";
const WRITING: &str = "esc to stop typing · enter to keep it";
const LAST: &str = "esc to cancel · ←→ between questions";

/// How an ask ended.
#[derive(Debug)]
pub(super) enum Put {
    /// This is what was answered, one per question.
    Said(Vec<Answered>),
    /// It was left with nothing sent.
    Left,
    /// There was no room to stand a panel. Nothing was drawn and no key was
    /// read, so the questions still have to be put.
    Cramped,
}

/// What one question keeps between frames.
struct Held {
    /// Which answer the mark stands on, the added one included.
    marked: usize,
    /// Which answers are chosen, where several may be.
    chosen: Vec<bool>,
    /// The answer nobody offered.
    wrote: Editor,
    /// The reader's own line about this question.
    note: Editor,
    /// Whether this question has been answered at all, which is what the mark
    /// on the row across the top is drawn from.
    answered: bool,
}

impl Held {
    /// A question nobody has reached yet.
    fn new(question: &Question) -> Self {
        Self {
            marked: 0,
            chosen: vec![false; question.answers().len()],
            wrote: Editor::new(),
            note: Editor::new(),
            answered: false,
        }
    }

    /// Every answer this question offers, the written one last.
    ///
    /// `shown` is the specimens, already gathered by the caller: a `Choice`
    /// borrows its rows, so they have to outlive the panel and cannot be built
    /// here.
    fn choices<'a>(
        &'a self,
        question: &'a Question,
        several: bool,
        shown: &'a [Vec<&'a str>],
    ) -> Vec<Choice<'a>> {
        let mut choices: Vec<Choice<'a>> = question
            .answers()
            .enumerate()
            .map(|(at, answer)| Choice {
                answer: answer.answer(),
                says: answer.says(),
                chosen: several.then(|| self.chosen.get(at).copied().unwrap_or_default()),
                shows: shown.get(at).map_or(&[][..], Vec::as_slice),
            })
            .collect();

        choices.push(Choice {
            answer: ELSE,
            says: "",
            chosen: None,
            shows: &[],
        });

        choices
    }

    /// What this question was answered with.
    fn answered(&self, question: &Question) -> Answered {
        let written = self.wrote.text();
        let named: Vec<&str> = question.answers().map(Answer::answer).collect();
        let last = named.len();

        let chosen: Vec<String> = if question.takes_several() {
            named
                .iter()
                .enumerate()
                .filter(|(at, _)| self.chosen.get(*at).copied().unwrap_or_default())
                .map(|(_, name)| (*name).to_owned())
                .collect()
        } else if self.marked == last {
            if written.is_empty() {
                Vec::new()
            } else {
                vec![written.to_owned()]
            }
        } else {
            named
                .get(self.marked)
                .map(|name| (*name).to_owned())
                .into_iter()
                .collect()
        };

        Answered::new(chosen).noting(self.note.text().to_owned())
    }
}

/// Which of the two lines somebody is writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    /// The answer nobody offered.
    Wrote,
    /// The line beside whatever was chosen.
    Note,
}

/// What a standing ask keeps between frames.
struct Standing {
    /// Which stop is on screen. The last one, where there is one, sends.
    at: usize,
    /// Which questions take several answers, kept because whether this ask
    /// reads its answers back is asked on every frame and the questions are not
    /// always to hand.
    asks: Vec<bool>,
    /// One per question, in the order they are asked.
    held: Vec<Held>,
    /// Which of the last stop's two answers is marked.
    sending: usize,
    /// What is being written, where anything is.
    writing: Option<Writer>,
}

impl Standing {
    /// An ask nobody has answered yet.
    fn new(questions: &[Question]) -> Self {
        Self {
            at: 0,
            asks: questions.iter().map(Question::takes_several).collect(),
            held: questions.iter().map(Held::new).collect(),
            sending: 0,
            writing: None,
        }
    }

    /// Whether this ask reads its answers back before sending them.
    ///
    /// More than one question, or any question taking several answers. The
    /// second is the one worth saying out loud: where several may be chosen,
    /// `enter` would otherwise mean both *choose this one* and *I am done*, and
    /// a key that means two things is a key that does the wrong one. A stop
    /// that says *these are the ones* is where being done happens instead.
    fn reviews(&self) -> bool {
        self.held.len() > 1 || self.asks.iter().any(|several| *several)
    }

    /// How many stops there are, the one that sends included.
    fn stops(&self) -> usize {
        self.held.len() + usize::from(self.reviews())
    }

    /// Whether the stop on screen is the one that sends.
    fn sending(&self) -> bool {
        self.reviews() && self.at == self.held.len()
    }

    /// How many answers the stop on screen offers, the written one included.
    fn choices(&self, questions: &[Question]) -> usize {
        if self.sending() {
            return SENT.len();
        }

        questions
            .get(self.at)
            .map_or(0, |question| question.answers().len() + 1)
    }

    /// Which answer the mark stands on, wherever it is.
    #[cfg(test)]
    fn marked(&self) -> usize {
        if self.sending() {
            self.sending
        } else {
            self.held.get(self.at).map_or(0, |held| held.marked)
        }
    }
}

/// Stands the questions where the prompt box was and reads keys until they are
/// answered or the ask is left.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn put<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    questions: &[Question],
) -> Result<Put, Fatal> {
    if questions.is_empty() {
        return Ok(Put::Left);
    }

    let mut standing = Standing::new(questions);

    let ended = region::stand(
        renderer,
        |_| style,
        &mut standing,
        |standing, columns, rows| {
            let room = rows.saturating_sub(KEPT);
            drawn(standing, questions, columns, room, style)
        },
        |arrived, standing| moving(arrived, standing, questions),
    )?;

    Ok(match ended {
        Ended::Took => Put::Said(
            standing
                .held
                .iter()
                .zip(questions)
                .map(|(held, question)| held.answered(question))
                .collect(),
        ),
        Ended::Left => Put::Left,
        Ended::Cramped => Put::Cramped,
    })
}

/// The panel as it stands, and where the cursor belongs in it.
fn drawn(
    standing: &mut Standing,
    questions: &[Question],
    columns: usize,
    room: usize,
    style: Style,
) -> (Vec<Row>, Option<Caret>) {
    let stops: Vec<Stop<'_>> = questions
        .iter()
        .zip(&standing.held)
        .map(|(question, held)| Stop {
            name: question.heading(),
            done: held.answered,
            asks: true,
        })
        .chain(standing.reviews().then_some(Stop {
            name: REVIEW,
            done: false,
            asks: false,
        }))
        .collect();

    let subject = if standing.reviews() { SUBJECT } else { ALONE };

    if standing.sending() {
        let around = Around {
            stops: &stops,
            subject,
            columns,
            room,
            style,
        };

        return sending(standing, questions, &around);
    }

    let Some(question) = questions.get(standing.at) else {
        return (Vec::new(), None);
    };
    let Some(held) = standing.held.get(standing.at) else {
        return (Vec::new(), None);
    };

    let several = question.takes_several();

    // Gathered here rather than inside the answers, because a `Choice` borrows
    // its specimen's rows and they have to outlive the panel that draws them.
    let shown: Vec<Vec<&str>> = question
        .answers()
        .map(|answer| answer.shows().collect())
        .collect();
    let answers = held.choices(question, several, &shown);
    let writing = standing.writing.map(|writer| {
        let line = match writer {
            Writer::Wrote => &held.wrote,
            Writer::Note => &held.note,
        };
        Writing {
            text: line.text(),
            column: line.column(),
            placeholder: ELSE,
        }
    });

    let panel = Asked {
        subject,
        stops: &stops,
        at: standing.at,
        statement: "",
        given: &[],
        question: question.question(),
        answers: &answers,
        marked: held.marked,
        note: held.note.text(),
        writing,
        at_note: standing.writing == Some(Writer::Note),
        leaves: LEAVE,
        footer: footer(standing, several),
    };

    panel.within(columns, room, style.glyphs())
}

/// Everything a panel is drawn around rather than out of: the row across the
/// top, the words over it, and the window.
///
/// One value rather than five arguments, because they are worked out together
/// and neither stop draws without all of them.
#[derive(Clone, Copy)]
struct Around<'a> {
    stops: &'a [Stop<'a>],
    subject: &'a str,
    columns: usize,
    room: usize,
    style: Style,
}

/// The last stop: every answer read back, and the two answers that send or do
/// not.
///
/// Its own function rather than a branch of the one above, because it draws
/// different rows out of different values and shares only the frame around them.
fn sending(
    standing: &Standing,
    questions: &[Question],
    around: &Around<'_>,
) -> (Vec<Row>, Option<Caret>) {
    let &Around {
        stops,
        subject,
        columns,
        room,
        style,
    } = around;

    // Joined here rather than in the panel, because how many answers a question
    // took is this side's to know and the panel draws one row per question
    // either way.
    let read: Vec<String> = questions
        .iter()
        .zip(&standing.held)
        .map(|(question, held)| {
            let answered = held.answered(question);
            let chosen: Vec<&str> = answered.chosen().collect();
            if chosen.is_empty() {
                NOTHING.to_owned()
            } else {
                chosen.join(", ")
            }
        })
        .collect();

    let given: Vec<Given<'_>> = questions
        .iter()
        .zip(&read)
        .map(|(question, answer)| Given {
            question: question.question(),
            answer,
        })
        .collect();

    let answers: Vec<Choice<'_>> = SENT
        .iter()
        .map(|answer| Choice {
            answer,
            says: "",
            chosen: None,
            shows: &[],
        })
        .collect();

    let panel = Asked {
        subject,
        stops,
        at: standing.at,
        statement: SENDING,
        given: &given,
        question: SEND,
        answers: &answers,
        marked: standing.sending,
        note: "",
        writing: None,
        at_note: false,
        leaves: "",
        footer: LAST,
    };

    panel.within(columns, room, style.glyphs())
}

/// The quiet row for an ask in this state.
///
/// A key is named only where it does something, which is why this is a function
/// of the panel rather than a constant.
fn footer(standing: &Standing, several: bool) -> &'static str {
    if standing.writing.is_some() {
        return WRITING;
    }
    if !standing.reviews() {
        return ONLY;
    }
    if several { MANY } else { ONE }
}

/// What `arrived` does to a standing ask.
///
/// Every key is named rather than caught by a rest arm: a key here is either an
/// answer, a step, or something decided to ignore, and a new [`Pressed`] must be
/// decided about rather than quietly join the third group.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn moving(arrived: Pressed, standing: &mut Standing, questions: &[Question]) -> Moved {
    if standing.writing.is_some() {
        return written(arrived, standing);
    }

    let choices = standing.choices(questions);
    let several = questions
        .get(standing.at)
        .is_some_and(Question::takes_several);

    match arrived {
        Pressed::Up => up(standing),
        Pressed::Down => down(standing, choices),
        Pressed::Key(Key::Left) => {
            let back = standing.at.checked_sub(1);
            step(&mut standing.at, back)
        }
        Pressed::Key(Key::Right) => {
            let stops = standing.stops();
            let next = Some(standing.at + 1).filter(|next| *next < stops);
            step(&mut standing.at, next)
        }
        Pressed::Key(Key::Enter) => took(standing, questions),
        Pressed::Key(Key::Char(' ')) if several => chose(standing),
        Pressed::Key(Key::Char('n')) if !standing.sending() => opened(standing),
        Pressed::Key(Key::Char(typed)) => numbered(typed, standing, questions),
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,

        // The wheel among them: an ask is walked with the arrows, and the
        // transcript it stands over is what a reader reaching for a wheel meant
        // to move. Answering `Still` is what hands it there.
        //
        // Ctrl+O, ctrl+T and the rest open things that stand in this same
        // region, which would leave a question on screen with nothing left to
        // answer it with. The key that copies the line is here for the other
        // reason: an ask is not a line being typed, and there is nothing in the
        // box for it to take.
        Pressed::Key(_)
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Explain
        | Pressed::Expand
        | Pressed::Plan
        | Pressed::Pasted(_)
        | Pressed::Clicked { .. }
        | Pressed::Queue
        | Pressed::Copy
        | Pressed::Scrolled { .. }
        | Pressed::Dragged { .. }
        | Pressed::Released { .. }
        | Pressed::Hovered { .. }
        | Pressed::Ignored => Moved::Still,
    }
}

/// What a key does while a line is being written.
///
/// The letters are the line's, so nothing here is a command except the two keys
/// that stop the writing — and escape keeps what was typed, because the whole
/// ask is what a second escape leaves.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn written(arrived: Pressed, standing: &mut Standing) -> Moved {
    let Some(writer) = standing.writing else {
        return Moved::Still;
    };
    let Some(held) = standing.held.get_mut(standing.at) else {
        return Moved::Still;
    };

    match arrived {
        Pressed::Key(Key::Enter) | Pressed::Escape => {
            standing.writing = None;
            Moved::Redraw
        }
        Pressed::Key(key) => {
            let line = match writer {
                Writer::Wrote => &mut held.wrote,
                Writer::Note => &mut held.note,
            };
            match line.press(key) {
                Typed::Changed => Moved::Redraw,
                _ => Moved::Still,
            }
        }
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// The key that goes up the answers.
fn up(standing: &mut Standing) -> Moved {
    let sending = standing.sending();
    let which = standing.at;
    let at = if sending {
        &mut standing.sending
    } else {
        match standing.held.get_mut(which) {
            Some(held) => &mut held.marked,
            None => return Moved::Still,
        }
    };

    let next = at.checked_sub(1);
    step(at, next)
}

/// And the one that goes down it.
fn down(standing: &mut Standing, choices: usize) -> Moved {
    let sending = standing.sending();
    let which = standing.at;
    let at = if sending {
        &mut standing.sending
    } else {
        match standing.held.get_mut(which) {
            Some(held) => &mut held.marked,
            None => return Moved::Still,
        }
    };

    let next = Some(*at + 1).filter(|next| *next < choices);
    step(at, next)
}

/// Chooses the marked answer, or unchooses it.
///
/// Never the written one: it is chosen by having something in it.
fn chose(standing: &mut Standing) -> Moved {
    let Some(held) = standing.held.get_mut(standing.at) else {
        return Moved::Still;
    };
    let Some(chosen) = held.chosen.get_mut(held.marked) else {
        return Moved::Still;
    };

    *chosen = !*chosen;
    held.answered = true;
    Moved::Redraw
}

/// Opens the reader's own line about this question.
fn opened(standing: &mut Standing) -> Moved {
    standing.writing = Some(Writer::Note);
    Moved::Redraw
}

/// Takes what is marked and moves on, and sends where there is nowhere to move
/// on to.
fn took(standing: &mut Standing, questions: &[Question]) -> Moved {
    if standing.sending() {
        return if standing.sending == 0 {
            Moved::Took
        } else {
            Moved::Left
        };
    }

    let choices = standing.choices(questions);
    let Some(held) = standing.held.get_mut(standing.at) else {
        return Moved::Still;
    };

    // The written answer is a line rather than a name, so landing on it opens
    // it instead of taking it. Taking it is what enter does the second time.
    if held.marked + 1 == choices && held.wrote.is_empty() && standing.writing.is_none() {
        standing.writing = Some(Writer::Wrote);
        return Moved::Redraw;
    }

    held.answered = true;

    let next = standing.at + 1;
    if next < standing.stops() {
        standing.at = next;
        return Moved::Redraw;
    }

    Moved::Took
}

/// The answer a typed character is the number of.
///
/// The numbers are drawn on the answers, which is what makes them keys: a
/// picture that numbers things and then ignores the numbers has promised
/// something. They start at one, so `0` names nothing, and the one past the
/// last answer is the row under the rule — which leaves.
fn numbered(typed: char, standing: &mut Standing, questions: &[Question]) -> Moved {
    let Some(at) = usize::try_from(typed.to_digit(10).unwrap_or(0))
        .ok()
        .and_then(|digit| digit.checked_sub(1))
    else {
        return Moved::Still;
    };

    let choices = standing.choices(questions);
    if at == choices && !standing.sending() {
        return Moved::Left;
    }
    if at >= choices {
        return Moved::Still;
    }

    if standing.sending() {
        standing.sending = at;
        return took(standing, questions);
    }

    let several = questions
        .get(standing.at)
        .is_some_and(Question::takes_several);
    let Some(held) = standing.held.get_mut(standing.at) else {
        return Moved::Still;
    };
    held.marked = at;

    if several {
        return chose(standing);
    }

    took(standing, questions)
}
