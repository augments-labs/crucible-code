//! The commands that take a turn's length, and the one that cuts it short.

use crucible_client_api::{
    Command, ErrorCode, Outcome, Problem, Refusal, Request, Response, RoomOutcome, Text,
    TurnOutcome,
};
use crucible_context::Room;
use crucible_runner::{RunContext, TurnError, Turned};
use crucible_runtime::Cancel;
use crucible_types::{Attachment, Compacting, Spend};

use super::deciding::{Deciding, Front};
use super::reading;
use crate::Conversation;

/// How a command [`turn`] was given ended, in the application's own values.
#[derive(Debug)]
pub enum Ended {
    /// It is not one of [`turn`]'s commands; nothing was done.
    Refused(Refusal),
    /// A prompt, answered or refused before the model was asked, or failed.
    Turn(Result<Turned, TurnError>),
    /// A compaction, and what it made room for.
    Room(Result<Room, TurnError>),
}

/// Takes the turn `request` asks for: a prompt, or a compaction somebody asked
/// for by name.
///
/// `attached` is what the front end standing on the host chose to send beside
/// the prompt. It is not in the request, because a request names no file: what
/// may be attached, and the one read it comes through, are decided on the host
/// before this is called.
///
/// Every permission question the turn raises is put to `front` under an
/// identity the application mints, which no caller supplies or can start
/// again, and settled only by a decision that names it. One handed in here as
/// a command of its own is about no action — the turn it would have to be
/// about is not this one — and is refused as stale, as it is at every door.
/// What the turn reports on the way goes to `run`, as it always has.
pub fn turn(
    conversation: &mut Conversation,
    request: &Request,
    attached: Box<[Attachment]>,
    front: &mut dyn Front,
    run: &RunContext<'_>,
) -> Ended {
    match request.command() {
        Command::Prompt(prompt) => {
            let mut ask = Deciding::new(front, request.capabilities());
            Ended::Turn(conversation.turn(prompt.as_str(), attached, &mut ask, run))
        }
        Command::Compact => {
            // What the recap cost is on the events `run` carried; a caller
            // that keeps a running total reads it there, as the terminal does.
            let mut spent = Spend::NONE;
            Ended::Room(conversation.compact(Compacting::Asked, run, &mut spent))
        }
        Command::Decide(_) => Ended::Refused(ErrorCode::StaleDecision.into()),
        Command::Theme(_)
        | Command::Cancel
        | Command::Clear
        | Command::Resume(_)
        | Command::SelectModel { .. }
        | Command::SetEffort(_)
        | Command::SetMode(_)
        | Command::CycleMode
        | Command::Login { .. }
        | Command::Logout { .. }
        | Command::InspectCache
        | Command::CleanCache
        | Command::Sandbox { .. }
        | Command::Help
        | Command::Exit => Ended::Refused(ErrorCode::Busy.into()),
    }
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
        | Command::SetMode(_)
        | Command::CycleMode
        | Command::Login { .. }
        | Command::Logout { .. }
        | Command::InspectCache
        | Command::CleanCache
        | Command::Sandbox { .. }
        | Command::Help
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
        }
    }
}
