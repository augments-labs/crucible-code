//! The question put before anything is sent on a route whose vendor says it
//! uses what is sent to it.
//!
//! Asked on the drawing thread, before the work is handed to a worker, so
//! the message is still this side's to give back. What stands is a panel with
//! the vendor's sentence and the page it comes from; `Use it anyway` writes
//! the yes into the user's own file and lets the work go, and anything else
//! sends nothing. Behind it, every client the run sends through holds the
//! route's origins until the yes is given, and a web source built for a
//! warned model asks that model's route before each request, so a path that
//! reached a send without passing here still sends nothing.
//!
//! A key pressed before the panel was drawn does not answer it: whatever is
//! waiting to be read when it is about to stand is dropped, so the Enter that
//! sent the message twice over cannot also say yes.

use crucible_app::Conversation;
use crucible_app::content_use::Warned;
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
    /// A row or a model about to be chosen.
    Choice,
}

/// How the question was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answer {
    /// `Use it anyway`.
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

/// Said where the panel did not fit at a choice.
pub(super) const CRAMPED_CHOICE: &str =
    "! this route needs an answer first; make the window taller and choose again";

/// Said where no panel fits before room is made.
pub(super) const CRAMPED_ROOM: &str =
    "! this route needs an answer first; make the window taller and try again";

/// Said once room was going to be made and nothing was sent.
pub(super) const UNSENT: &str = "nothing was sent";

/// Said once the message is back in the box.
pub(super) const KEPT: &str = "nothing was sent; your message is back in the prompt box";

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
    let shown: &[Offered<'_>] = match put {
        Put::Send => &SENDING,
        Put::Choice => &CHOOSING,
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
                title: warned.shown,
                said: Some(warned.warning.sentence),
                source: Some(&cited),
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
/// was anything but yes.
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
    let Some(provider) = conversation.serving() else {
        return Ok(false);
    };
    // Every route that holds the send is asked about in turn: two warned
    // routes at one origin each keep the request back until each has its
    // yes. A yes is written down before the next question, so a route comes
    // back only where its yes could not be, and that is said and stops here.
    let said = loop {
        let Some(warned) = terms
            .consent
            .unanswered(provider, conversation.runner().model())
        else {
            return Ok(false);
        };
        // A loop reading lines rather than keys has nobody to put a panel
        // to, and neither has one whose raw mode never came up.
        if !held.answers.keys {
            return Err(Fatal::Unanswered(
                format!("{}: {} {ELSEWHERE}", warned.shown, warned.warning.sentence).into(),
            ));
        }
        break match ask(renderer, terms.style(), &warned, Put::Send)? {
            Answer::Yes => match recorded(terms, &warned) {
                Ok(()) => continue,
                Err(said) => said,
            },
            // Room to be made has no message to give back.
            Answer::Back if matches!(work, Work::Room(_)) => UNSENT.to_owned(),
            Answer::Cramped if matches!(work, Work::Room(_)) => CRAMPED_ROOM.to_owned(),
            Answer::Back => KEPT.to_owned(),
            Answer::Cramped => CRAMPED_SEND.to_owned(),
        };
    };
    if let Work::Turn(prompt, _) = work {
        held.editor.put(prompt);
    }
    renderer.commit(&said)?;
    Ok(true)
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
    let Some(warned) = terms.consent.routes().warned(route).copied() else {
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
