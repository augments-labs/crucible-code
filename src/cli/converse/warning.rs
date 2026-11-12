//! The question put before anything is sent on a route whose vendor says it
//! uses what is sent to it.
//!
//! Asked on the drawing thread, before the work is handed to a worker, so the
//! message is still this side's to give back, and the dock's answers held back
//! where keys are read stay in the dock, refused with their drafts kept; a
//! message that would start an installed agent is asked about on that agent's
//! own route, and says what starting it runs, unless the start would be refused
//! whatever the answer: that message is not asked about, and goes on to be
//! refused as a failed turn, before anything starts. Put before an installed
//! agent is first started any other way, it is the application's question,
//! answered on the drawing thread while the operation that asked it waits: it
//! says the same, and the application writes down its yes. What stands is a
//! panel with the vendor's sentence and the page it comes from; `Use it anyway`
//! writes the yes into the user's own file and lets the work go, and anything
//! else sends nothing. Behind it, every client the run sends through holds the
//! route's origins until the yes is given, and a web source built for a warned
//! model asks that model's route before each request, so a path that reached a
//! send without passing here still sends nothing.
//!
//! A key pressed before the panel was drawn does not answer it: whatever is
//! waiting to be read when it is about to stand is dropped, so the Enter that
//! sent the message twice over cannot also say yes.

use crucible_app::Conversation;
use crucible_app::content_use::{self, Warned};
use crucible_client_api::Author;
use crucible_tui::{Offered, Panel, Renderer, Terminal};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::region::{self, Ended};
use super::{Held, Terms, Work, picking};

/// What the question is put before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Put {
    /// A message about to be sent.
    Send,
    /// The application's note about a command that ended, about to start a
    /// turn. Held back, it is dropped rather than put in the prompt box,
    /// where it would read as the person's message.
    Note,
    /// The lines queued behind a turn about to be sent as the next one.
    /// Held back, they stay in the queue, and the box is left as it is.
    Queue,
    /// The dock's answers about to be sent.
    Answer,
    /// Room about to be made, which sends the conversation for its recap.
    /// There is no message to send or to keep.
    Room,
    /// A row or a model about to be chosen.
    Choice,
    /// An installed agent about to be started for the first time.
    Launch,
}

impl Put {
    /// The question put before `work` is sent.
    pub(super) fn before(work: &Work) -> Self {
        match work {
            Work::Answer(_) => Self::Answer,
            Work::Turn(_, _, Author::Application) => Self::Note,
            Work::Turn(_, _, Author::Person) => Self::Send,
            Work::Queued(..) => Self::Queue,
            Work::Room(_) => Self::Room,
        }
    }

    /// The answers offered, where they are fixed: all but a launch's, which
    /// name the agent it starts.
    pub(super) fn offered(self) -> Option<&'static [Offered<'static>]> {
        match self {
            Self::Send => Some(&SENDING),
            Self::Note => Some(&NOTING),
            Self::Queue => Some(&QUEUING),
            Self::Answer => Some(&ANSWERING),
            Self::Room => Some(&MAKING_ROOM),
            Self::Choice => Some(&CHOOSING),
            Self::Launch => None,
        }
    }
}

/// How the question was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answer {
    /// `Use it anyway`, or `Continue` before a launch.
    Yes,
    /// `Go back`, or escape.
    Back,
    /// There was no room to stand the panel whole, so it was not asked.
    Cramped,
}

/// The answers offered before a send.
const SENDING: [Offered<'static>; 2] = [
    Offered {
        name: "Use it anyway",
        says: "Sends your message; this route is not asked about again",
    },
    Offered {
        name: "Go back",
        says: "Keeps your message in the prompt box; nothing is sent",
    },
];

/// The answers offered before the lines queued behind a turn are sent, which
/// stay in the queue where they are not.
const QUEUING: [Offered<'static>; 2] = [
    Offered {
        name: "Use it anyway",
        says: "Sends your queued messages; this route is not asked about again",
    },
    Offered {
        name: "Go back",
        says: "Keeps them in the queue for your next prompt; nothing is sent",
    },
];

/// The answers offered before the dock's answers are sent, which stay in the
/// dock where they are not.
const ANSWERING: [Offered<'static>; 2] = [
    Offered {
        name: "Use it anyway",
        says: "Sends your answers; this route is not asked about again",
    },
    Offered {
        name: "Go back",
        says: "Keeps your answers in the dock; nothing is sent",
    },
];

/// The answers offered before room is made.
const MAKING_ROOM: [Offered<'static>; 2] = [
    Offered {
        name: "Use it anyway",
        says: "Sends the conversation for its recap; this route is not asked about again",
    },
    Offered {
        name: "Go back",
        says: "Leaves the conversation as it is; nothing is sent",
    },
];

/// The answers offered before the application's note starts a turn.
const NOTING: [Offered<'static>; 2] = [
    Offered {
        name: "Use it anyway",
        says: "Sends the note; this route is not asked about again",
    },
    Offered {
        name: "Go back",
        says: "Leaves the prompt box as it is; nothing is sent",
    },
];

/// The answers offered before a choice.
const CHOOSING: [Offered<'static>; 2] = [
    Offered {
        name: "Use it anyway",
        says: "Takes this choice; this route is not asked about again",
    },
    Offered {
        name: "Go back",
        says: "Returns to the list; nothing is chosen",
    },
];

/// Said where the panel did not fit at a send.
pub(super) const CRAMPED_SEND: &str =
    "! this route needs an answer first; make the window taller and send again";

/// Said where no panel fits before the lines queued behind a turn are sent.
pub(super) const CRAMPED_QUEUE: &str = "! this route needs an answer first; make the window taller, and your queue goes with your next prompt";

/// Said where the panel did not fit at a choice.
pub(super) const CRAMPED_CHOICE: &str =
    "! this route needs an answer first; make the window taller and choose again";

/// Said where no panel fits before room is made, or before the dock's answers
/// are sent.
pub(super) const CRAMPED_ROOM: &str =
    "! this route needs an answer first; make the window taller and try again";

/// Said once room was going to be made, or the dock's answers were going to
/// be sent, and nothing was sent.
pub(super) const UNSENT: &str = "nothing was sent";

/// Said once the message is back in the box.
pub(super) const KEPT: &str = "nothing was sent; your message is back in the prompt box";

/// Said once the queue is back as it was, waiting for the reader's next prompt.
pub(super) const QUEUE_KEPT: &str =
    "nothing was sent; your queued messages go with the next prompt you send";

/// Said once the note about a command that ended was going to start a turn,
/// and nothing was sent.
pub(super) const DROPPED: &str = "nothing was sent; the model was not told the command ended";

/// Said where no panel fits before the note about a command that ended starts
/// a turn. There is no sending it again: the note is dropped.
pub(super) const CRAMPED_NOTE: &str = "! this route needs an answer first; nothing was sent, so the model was not told the command ended";

/// Said after the words under a run with no terminal.
pub(super) const ELSEWHERE: &str = "Nothing was sent; answer it once in a terminal.";

/// Stands the warning for `warned` and reads keys until it is answered.
///
/// The panel is drawn whole or not at all: a warning without the vendor's
/// sentence is a question nobody could answer.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    warned: &Warned,
    put: Put,
) -> Result<Answer, Fatal> {
    ask_while(renderer, style, warned, put, &mut |_| Ok(()))
}

/// [`ask`], with a running turn's drain run once a pass so what it reports
/// goes on arriving behind the panel.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn ask_while<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    warned: &Warned,
    put: Put,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Answer, Fatal> {
    let agent = &*warned.shown;
    let starts = format!("Starts {agent} to check the account; this route is not asked again");
    let launching = [
        Offered {
            name: "Continue",
            says: &starts,
        },
        Offered {
            name: "Go back",
            says: "Nothing starts",
        },
    ];
    let launched: &[&str] = if warned.unrecorded() {
        &[content_use::LAUNCHING, content_use::UNRECORDED]
    } else {
        &[content_use::LAUNCHING]
    };
    let shown: &[Offered<'_>] = put.offered().unwrap_or(&launching);
    let told: &[&str] = match put {
        // A message, a note or an answer on an installed agent's route that starts
        // the agent says what starting it runs, as a launch does.
        Put::Send | Put::Note | Put::Queue | Put::Answer
            if content_use::launches(&warned.route) =>
        {
            launched
        }
        Put::Launch => launched,
        // Room is not made on an agent's route, so making it starts nothing.
        Put::Send | Put::Note | Put::Queue | Put::Answer | Put::Room | Put::Choice => &[],
    };
    let cited = warned.warning.cited();
    let footer = format!("enter to choose {} esc to go back", style.glyphs().dot());

    region::unheard(renderer)?;
    let mut at = 0;
    let ended = region::stand_while(
        renderer,
        |_| style,
        &mut at,
        |marked, columns, _| {
            let panel = Panel {
                title: &warned.shown,
                said: Some(&warned.warning.sentence),
                source: Some(&cited),
                told,
                shown,
                chosen: *marked,
                footer: &footer,
            };
            (panel.rows(columns, style.glyphs()), None)
        },
        |arrived, at| picking::moving(arrived, at, shown.len()),
        while_waiting,
    )?;

    Ok(match ended {
        Ended::Took if at == 0 => Answer::Yes,
        Ended::Took | Ended::Left => Answer::Back,
        Ended::Cramped => Answer::Cramped,
    })
}

/// Writes the yes to `warned` into the user's own file, then lets its
/// requests go. Nothing is let go where the file could not be written.
///
/// # Errors
///
/// What stopped the file being written, in a sentence for the reader.
pub(super) fn recorded(terms: &Terms, warned: &Warned) -> Result<(), String> {
    terms
        .consent
        .accept(warned)
        .map_err(|problem| format!("! your answer could not be written down: {problem}"))
}

/// Whether `work` is held back rather than sent: asked about first where the
/// route it would go on has no yes, and handed back whole where the answer
/// was anything but yes, or a yes that could not be written down. The
/// application's note about a command that ended is not handed back: it is
/// dropped, and the prompt box keeps what it held. A turn
/// [`Conversation::unlaunchable`] refuses is not held: it goes on to that
/// refusal unasked.
///
/// # Errors
///
/// [`Fatal::Unanswered`] with no terminal to ask on, and [`Fatal::Terminal`]
/// if the terminal could not be drawn on or read from.
pub(super) fn held<T: Terminal>(
    conversation: &Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    work: &Work,
    held: &mut Held<'_>,
) -> Result<bool, Fatal> {
    // Every route that holds the send is asked about in turn: two warned
    // routes at one origin each keep the request back until each has its
    // yes. A yes is written down before the next question, so a route comes
    // back only where its yes could not be, and that is said, with what
    // became of the work, and stops here.
    let said = loop {
        let Some(warned) = unsent(conversation, terms, work) else {
            return Ok(false);
        };
        // A turn that would be refused before it started the agent, whatever
        // the answer, is not asked about: it goes on to be refused as it
        // would be after a yes, drawn as a failed turn is, and nothing starts.
        if let Work::Turn(_, attached, _) | Work::Queued(_, attached) = work
            && conversation.unlaunchable(attached).is_err()
        {
            return Ok(false);
        }
        // A loop reading lines rather than keys has nobody to put a panel
        // to, and neither has one whose raw mode never came up.
        if !held.answers.keys {
            return Err(Fatal::Unanswered(unanswered(&warned)));
        }
        break match ask(renderer, terms.style(), &warned, Put::before(work))? {
            // Nothing was sent where the yes could not be written down, as
            // after going back, so what became of the work is said after why.
            Answer::Yes => match recorded(terms, &warned) {
                Ok(()) => continue,
                Err(why) => {
                    renderer.commit(&why)?;
                    left(work, false).to_owned()
                }
            },
            Answer::Back => left(work, false).to_owned(),
            Answer::Cramped => left(work, true).to_owned(),
        };
    };
    if let Some(prompt) = returned(work) {
        held.editor.put(prompt);
    }
    if let Work::Queued(prompt, _) = work {
        held.queued.kept(prompt, &terms.steer);
    }
    renderer.commit(&said)?;
    Ok(true)
}

/// What is said once `work` was held back unsent, where the panel was answered
/// with no or with a yes that could not be written down, or did not fit where
/// `cramped`. Room to be made has no message to give back and the dock's
/// answers stay in the dock; the application's note is not the person's to
/// have back, and is dropped; the queued lines stay queued.
pub(super) fn left(work: &Work, cramped: bool) -> &'static str {
    match (work, cramped) {
        (Work::Turn(_, _, Author::Person), false) => KEPT,
        (Work::Turn(_, _, Author::Person), true) => CRAMPED_SEND,
        (Work::Turn(_, _, Author::Application), false) => DROPPED,
        (Work::Turn(_, _, Author::Application), true) => CRAMPED_NOTE,
        (Work::Queued(..), false) => QUEUE_KEPT,
        (Work::Queued(..), true) => CRAMPED_QUEUE,
        (Work::Room(_) | Work::Answer(_), false) => UNSENT,
        (Work::Room(_) | Work::Answer(_), true) => CRAMPED_ROOM,
    }
}

/// The message held back that goes back in the prompt box: only one the
/// person wrote in it. Putting it back replaces what the box holds, so the
/// application's note would overwrite a half-typed line and then be sent as
/// the person's own, and the queue's first line would overwrite it and leave
/// the lines behind it to be asked about one by one.
pub(super) fn returned(work: &Work) -> Option<&str> {
    match work {
        Work::Turn(prompt, _, Author::Person) => Some(prompt),
        Work::Turn(_, _, Author::Application)
        | Work::Queued(..)
        | Work::Room(_)
        | Work::Answer(_) => None,
    }
}

/// What a run with no terminal says when `warned` has no yes: the route's
/// name, the vendor's sentence with what starting an installed agent runs
/// after it where the route is one, and where to answer.
pub(super) fn unanswered(warned: &Warned) -> Box<str> {
    format!(
        "{}: {} {ELSEWHERE}",
        warned.shown,
        content_use::full_sentence(warned)
    )
    .into()
}

/// The warned route `work` would go on with no yes: the one a send to the
/// conversation's provider is held by, or on an installed agent's route, the
/// agent's own where the turn would start it. Room is not made on an agent's
/// route, so nothing is started for it. Either route is read from the yes the
/// question writes to, so a route answered here is not asked again. The dock's
/// answers that reach this are asked about as a turn would be, and always:
/// the check that spares a turn the question when it would be refused before
/// it started the agent is made in `held` for a turn alone, so an answer is
/// asked about whatever its launch would do. Ones with nothing left to settle
/// were refused before it.
fn unsent(conversation: &Conversation, terms: &Terms, work: &Work) -> Option<Warned> {
    match conversation.serving() {
        Some(provider) => terms
            .consent
            .unanswered(provider, conversation.settled().model()),
        None => match work {
            Work::Turn(..) | Work::Queued(..) | Work::Answer(_) => {
                terms.consent.asks(&conversation.launching()?)
            }
            Work::Room(_) => None,
        },
    }
}

/// What becomes of a choice once the question before it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Chosen {
    /// It is taken: its route has a yes, now or from before, or nothing is
    /// asked about it.
    Take,
    /// Back to where it was chosen from.
    Back,
    /// Not taken, and this line says why.
    Stop(String),
}

/// Asks about each route a choice of `model` on `provider` would send on
/// that has no yes, before the choice is taken. A yes given here is written
/// down at once.
///
/// With no keys to read the choice is taken unasked: choosing sends nothing,
/// and the send that would is where a run with no terminal ends.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn choosing<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    (provider, model): (&str, &str),
    keys: bool,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Chosen, Fatal> {
    if !keys {
        return Ok(Chosen::Take);
    }
    // Each route that would hold the choice's sends, in turn, as at a send.
    while let Some(warned) = terms.consent.unanswered(provider, model) {
        match ask_while(renderer, terms.style(), &warned, Put::Choice, while_waiting)? {
            Answer::Yes => {
                if let Err(said) = recorded(terms, &warned) {
                    return Ok(Chosen::Stop(said));
                }
            }
            Answer::Back => return Ok(Chosen::Back),
            Answer::Cramped => return Ok(Chosen::Stop(CRAMPED_CHOICE.to_owned())),
        }
    }
    Ok(Chosen::Take)
}

/// Writes down the yes given to `route` at a `/login` choice, now that its
/// credential is stored; nothing where none was given.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the line saying it could not be written down could
/// not be drawn.
pub(super) fn stored<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    route: &str,
) -> Result<(), Fatal> {
    let Some(warned) = terms.consent.routes().warned(route).cloned() else {
        return Ok(());
    };
    if !terms.consent.given(route) {
        return Ok(());
    }
    if let Err(said) = recorded(terms, &warned) {
        renderer.commit(&said)?;
    }
    Ok(())
}
