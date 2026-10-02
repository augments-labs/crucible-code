//! The commands that take a turn's length, and the one that cuts it short.

use crucible_client_api::{
    Command, ErrorCode, Missing, Outcome, Problem, Refusal, Request, Response, RoomOutcome, Text,
    TurnOutcome,
};
use crucible_context::Room;
use crucible_runner::{RunContext, TurnError, Turned};
use crucible_runtime::Cancel;
use crucible_types::{Attachment, Compacting, Spend};

use super::deciding::{Deciding, Front};
use super::reading;
use crate::Conversation;
use crate::content_use::Warned;
use crate::providers;
use crate::remember::RememberError;

/// How a command [`turn`] was given ended, in the application's own values.
#[derive(Debug)]
pub enum Ended {
    /// It is not one of [`turn`]'s commands; nothing was done.
    Refused(Refusal),
    /// Nothing was recorded and nothing was sent: there is no model to ask,
    /// for want of what this names.
    Unasked(providers::Missing),
    /// A prompt, answered or refused before the model was asked, or failed.
    Turn(Result<Turned, TurnError>),
    /// A compaction, and what it made room for.
    Room(Result<Room, TurnError>),
    /// Nothing was sent: the route it would go on is one whose vendor uses
    /// what is sent, and no yes to it was given. The question was declined,
    /// went unanswered, or could not be put to the client.
    Warned(Warned),
    /// Nothing was sent: a yes was given and could not be written down.
    Unrecorded(RememberError),
}

/// Takes the turn `request` asks for: a prompt, or a compaction somebody asked
/// for by name.
///
/// `attached` is what the front end standing on the host chose to send beside
/// the prompt. It is not in the request, because a request names no file: what
/// may be attached, and the one read it comes through, are decided on the host
/// before this is called.
///
/// Where [`Conversation::missing`] says there is no model to ask, neither is
/// taken: the answer is [`Ended::Unasked`], and the session records nothing.
///
/// Every permission question the turn raises is put to `front` under an
/// identity the application mints, which no caller supplies or can start
/// again, and settled only by a decision that names it. One handed in here as
/// a command of its own is about no action — the turn it would have to be
/// about is not this one — and is refused as stale, as it is at every door.
/// What the turn reports on the way goes to `run`, as it always has.
pub async fn turn(
    conversation: &mut Conversation,
    request: &Request,
    attached: Box<[Attachment]>,
    front: &mut dyn Front,
    run: &RunContext<'_>,
) -> Ended {
    // Before anything else, and at this door so that every client is answered
    // the same way: a prompt or a request for room with no model to ask is
    // not a turn, and is answered with what is missing before the session
    // hears of it. Ahead of the question below, which is about a vendor this
    // would never reach.
    if matches!(request.command(), Command::Prompt(_) | Command::Compact)
        && let Some(missing) = conversation.missing()
    {
        return Ended::Unasked(missing);
    }
    // Before anything could be sent, and at this door so that every client is
    // asked the same way: a route whose vendor uses what is sent, with no yes
    // to it, is put to the front as a pending action, and a yes given there is
    // written into the user's own file before the turn goes.
    //
    // Every route that holds the send is put in turn: two warned routes at
    // one origin each keep the request back until each has its yes. A yes is
    // written down before the next is put, so none comes back.
    while matches!(request.command(), Command::Prompt(_) | Command::Compact)
        && let Some(warned) = unanswered(conversation)
    {
        let accepted = super::deciding::warned(request.capabilities(), front, &warned).await;
        let Some(consent) = conversation.consent().filter(|_| accepted) else {
            return Ended::Warned(warned);
        };
        // A yes that could not be written down is not a going back: said as
        // the failure it is, and nothing is sent.
        if let Err(problem) = consent.accept(&warned) {
            return Ended::Unrecorded(problem);
        }
    }
    match request.command() {
        Command::Prompt(prompt) => {
            let mut ask = Deciding::new(front, request.capabilities());
            Ended::Turn(
                conversation
                    .turn(prompt.as_str(), attached, &mut ask, run)
                    .await,
            )
        }
        Command::Compact => {
            // What the recap cost is on the events `run` carried; a caller
            // that keeps a running total reads it there, as the terminal does.
            let mut spent = Spend::NONE;
            Ended::Room(
                conversation
                    .compact(Compacting::Asked, run, &mut spent)
                    .await,
            )
        }
        Command::Decide(_) => Ended::Refused(ErrorCode::StaleDecision.into()),
        Command::Theme(_)
        | Command::Cancel
        | Command::Clear
        | Command::Resume(_)
        | Command::SelectModel { .. }
        | Command::SetEffort(_)
        | Command::SetSpeed(_)
        | Command::SetMode(_)
        | Command::CycleMode
        | Command::Login { .. }
        | Command::Logout { .. }
        | Command::InspectCache
        | Command::CleanCache
        | Command::Sandbox { .. }
        | Command::Help
        | Command::ReleaseNotes { .. }
        | Command::Context
        | Command::Exit => Ended::Refused(ErrorCode::Busy.into()),
    }
}

/// The warned route a turn of `conversation` would go on with no yes.
fn unanswered(conversation: &Conversation) -> Option<Warned> {
    conversation
        .consent()?
        .unanswered(conversation.serving()?, conversation.runner().model())
}

/// Asks the turn `cancel` belongs to to stop.
///
/// The turn ends when it next looks, and says so in its own outcome: this is
/// the request having been heard and nothing more. No conversation is needed,
/// which is what lets it be asked while a turn has the conversation.
///
/// A decision handed in here is refused as stale rather than as busy: the
/// turn's pending action is settled through the [`Front`] it was put to, and
/// this door holds no action for a decision to name.
pub fn interrupt(request: &Request, cancel: &Cancel) -> Outcome {
    match request.command() {
        Command::Cancel => {
            cancel.request();
            Outcome::Cancelling
        }
        Command::Decide(_) => Outcome::Refused(ErrorCode::StaleDecision.into()),
        Command::Prompt(_)
        | Command::Compact
        | Command::Theme(_)
        | Command::Clear
        | Command::Resume(_)
        | Command::SelectModel { .. }
        | Command::SetEffort(_)
        | Command::SetSpeed(_)
        | Command::SetMode(_)
        | Command::CycleMode
        | Command::Login { .. }
        | Command::Logout { .. }
        | Command::InspectCache
        | Command::CleanCache
        | Command::Sandbox { .. }
        | Command::Help
        | Command::ReleaseNotes { .. }
        | Command::Context
        | Command::Exit => Outcome::Refused(ErrorCode::Busy.into()),
    }
}

impl Ended {
    /// The answer to `request`, as a client is sent it.
    #[must_use]
    pub fn response(&self, request: &Request) -> Response {
        Response {
            correlation: Some(request.correlation()),
            outcome: self.outcome(),
        }
    }

    /// What happened, as a client is told it.
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Refused(refusal) => Outcome::Refused(*refusal),
            Self::Unasked(missing) => Outcome::Unasked(match missing {
                providers::Missing::Credential => Missing::Credential,
                providers::Missing::Provider => Missing::Provider,
                providers::Missing::Model => Missing::Model,
            }),
            Self::Turn(Ok(Turned::Ran(result))) => Outcome::Turn(TurnOutcome::Ran {
                stop: reading::stop(result.stop()),
            }),
            Self::Turn(Ok(Turned::Rejected { rejection, stop })) => {
                Outcome::Turn(TurnOutcome::Rejected {
                    // Both were kept to a ceiling under this one before they
                    // got here, so whether they are whole is theirs to say.
                    guard: Text::cut_again(rejection.guard(), rejection.guard_was_cut()),
                    why: Text::cut_again(rejection.why(), rejection.why_was_cut()),
                    stop: stop.map(reading::stop),
                })
            }
            Self::Turn(Ok(Turned::Undecided { problem, stop })) => {
                Outcome::Turn(TurnOutcome::Undecided {
                    problem: Problem {
                        code: ErrorCode::Failed,
                        message: Text::cut_again(&problem.to_string(), problem.was_cut()),
                    },
                    stop: stop.map(reading::stop),
                })
            }
            Self::Turn(Err(problem)) => {
                Outcome::Turn(TurnOutcome::Failed(Problem::failed(problem)))
            }
            Self::Room(Ok(Room::Made(compacted))) => Outcome::Room(RoomOutcome::Made {
                replaced: reading::count(compacted.replaced),
            }),
            Self::Room(Ok(Room::Nothing)) => Outcome::Room(RoomOutcome::Nothing),
            Self::Room(Ok(Room::Stopped)) => Outcome::Room(RoomOutcome::Stopped),
            Self::Room(Err(problem)) => {
                Outcome::Room(RoomOutcome::Failed(Problem::failed(problem)))
            }
            Self::Unrecorded(problem) => {
                Outcome::Turn(TurnOutcome::Failed(Problem::failed(problem)))
            }
            Self::Warned(warned) => Outcome::Turn(TurnOutcome::Warned {
                route: Text::cut(warned.route),
                sentence: Text::cut(warned.warning.sentence),
                source: Text::cut(&warned.warning.cited()),
            }),
        }
    }
}
