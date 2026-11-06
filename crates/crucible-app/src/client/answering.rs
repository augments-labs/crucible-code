//! The command that answers questions the model asked without waiting.
//!
//! The model's questions wait in a [`Deferred`] registry the front end
//! supplied at startup and keeps a clone of, as it keeps the turn's `Cancel`.
//! [`answer`] settles a batch there and nothing else, so it needs no
//! conversation: it is the door for an answer while a turn has the
//! conversation, and the running turn delivers it. While no turn runs,
//! [`turn`](super::turn) takes the same command and starts the turn that
//! delivers it.
//!
//! An answer settles questions. It is no ruling on a pending action and no yes
//! to a route whose vendor uses what is sent: nothing here reaches the
//! permission engine or the content-use hold.

use crucible_client_api::{
    AnswerOutcome, Capability, Command, ErrorCode, Outcome, Picked, Problem, Refusal, Replied,
    Request, Said, Text,
};
use crucible_tools::{Deferred, Unsettled};
use crucible_types::{Answered, Reply};

/// Settles the batch `request` answers in `held`, and says what came of it.
///
/// The batch is settled whole or not at all. A batch not waiting is refused
/// as [`ErrorCode::StaleQuestions`]; replies the batch cannot take are
/// [`AnswerOutcome::Invalid`], naming the first that does not fit. Either way
/// the batch is as it was, still waiting if it was.
///
/// A client that did not claim [`Capability::Questions`] is refused as
/// [`ErrorCode::NeedsCapability`]: it is sent nothing of what waits, so it
/// cannot be answering it. Any other command is refused as
/// [`ErrorCode::Busy`]; it is not this door's.
#[must_use]
pub fn answer(request: &Request, held: &Deferred) -> Outcome {
    match settle(request, held) {
        Ok(settling) => settled(settling.as_ref().map(|&()| ())),
        Err(refusal) => Outcome::Refused(refusal),
    }
}

/// [`answer`], in the application's own values: the registry's word on the
/// batch, or the refusal of a request that never reached it.
pub(super) fn settle(request: &Request, held: &Deferred) -> Result<Result<(), Unsettled>, Refusal> {
    let Command::Answer { id, replies } = request.command() else {
        return Err(ErrorCode::Busy.into());
    };
    if !request.capabilities().has(Capability::Questions) {
        return Err(ErrorCode::NeedsCapability.into());
    }
    Ok(held.settle(*id, replies.iter().map(reply).collect()))
}

/// What a client is told of settling a batch, in the contract's words.
///
/// Every refusal is named, so one the registry adds decides here whether it
/// is about the batch or about the replies.
pub(super) fn settled(settling: Result<(), &Unsettled>) -> Outcome {
    match settling {
        Ok(()) => Outcome::Answer(AnswerOutcome::Settled),
        Err(Unsettled::Stale(_)) => Outcome::Refused(ErrorCode::StaleQuestions.into()),
        Err(
            unfit @ (Unsettled::Count { .. }
            | Unsettled::OneAnswer { .. }
            | Unsettled::Repeated { .. }
            | Unsettled::WrittenTwice { .. }
            | Unsettled::TooLong { .. }
            | Unsettled::Control { .. }),
        ) => Outcome::Answer(AnswerOutcome::Invalid(Problem {
            code: ErrorCode::InvalidAnswer,
            // The registry's sentences repeat nothing a reply said, so this
            // carries no word of the person's.
            message: Text::cut(&unfit.to_string()),
        })),
    }
}

/// One reply, as the registry takes it. The words come across whole: the
/// contract carried them as `Said`, which refuses what it cannot hold.
fn reply(replied: &Replied) -> Reply {
    match replied {
        Replied::Answered(Picked { chosen, note }) => {
            Reply::Answered(Answered::new(chosen.iter().map(Said::as_str)).noting(note.as_str()))
        }
        Replied::Declined => Reply::Declined,
    }
}
