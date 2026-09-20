//! `/clear`: starting a new session, and leaving this one where `/resume` can
//! find it.
//!
//! An empty context is what is asked for, and the shortest way to one is a
//! session that has said nothing yet. So this is [`resume`] with the session
//! swapped: a fresh log rather than a reopened one, asked of the application as
//! the command it is and picked up by [`Conversation::clear`], which is also what drops the permission answers
//! given for the rest of a session that is now over. The record of what has been read
//! is emptied here for the same reason, exactly as `/resume` empties it, and so
//! are the images pasted at the prompt, the plan the panel above the box is
//! drawn from and the screen itself —
//! an empty context drawn under a full transcript would read as a conversation
//! the agent has no memory of. What goes back up is the opening card and
//! nothing else, so the screen after a clear is the screen a fresh start
//! draws.
//!
//! What was said is not deleted. The log it was written to is closed and stays
//! on the disk, so the session is on `/resume`'s list like any other and
//! nothing about this command is destructive — which is why it asks nothing
//! before running.
//!
//! [`resume`]: super::resume

use crucible_app::Conversation;
use crucible_app::client::{Cleared, Performed};
use crucible_client_api::Command;
use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::client::astray;

use super::super::Held;
use super::Terms;

/// Runs it.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let columns = renderer.columns();

    let unclosed = match terms.perform(conversation, Command::Clear) {
        // A session that has said nothing is already the empty one this would
        // go and open. Starting a second log here would leave two files for a
        // session that never happened, and `--continue` offers the newer of
        // them. The screen is still emptied: whatever a command printed
        // belongs behind the clear the same as a conversation would.
        Performed::Cleared(Cleared::Nothing) => {
            held.kept.forget();
            held.images.clear();
            renderer.empties()?;
            held.opening.commit(renderer)?;

            let rows = [Row::new().then(Slot::Quiet, clip("nothing had been said", columns))];
            return Ok(renderer.present(&rows)?);
        }
        Performed::Cleared(Cleared::Started { unclosed }) => unclosed,
        // A path is in every one of these, so it is committed rather than
        // presented — the same as `/resume`'s. Nothing else changes: the
        // session in hand is still being recorded, and the loop carries on
        // with it.
        Performed::Cleared(Cleared::Failed(problem)) => {
            return Ok(renderer.commit(&format!("! {problem}"))?);
        }
        other => return Ok(renderer.commit(&astray(&other))?),
    };

    // The conversation swapped its session and handed the runner the same one,
    // and everything this loop reads of a session it reads off the
    // conversation. The one being left was closed on the way, and what closing
    // it said about its log came back to be said below.
    // The files remembered were read by a session this is no longer in, and
    // `write` replaces a file on the strength of that record. Emptying it costs
    // a read; leaving it standing would cost the file.
    terms.ledger.forget();

    // And the plan for the same reason said the other way round: it costs
    // nothing to leave, and what it would leave is a panel above the prompt
    // listing work the agent under it has no memory of.
    terms.plan.forget();

    // The tools looked up belong to the conversation that looked them up. Left
    // standing they would be advertised to a session that never asked, which is
    // the schema cost this whole mechanism exists to avoid — paid, and for a
    // model with no memory of why.
    terms.revealed.forget();

    // A model picked mid-turn was picked for the session being left, and is
    // held on the session, so it goes with it rather than landing on the new
    // one it was never chosen for.
    terms.pending_model.take();

    // A mode stepped to mid-turn was stepped for the session being left, and
    // goes with it rather than landing on the new one it was never chosen for.
    terms.pending_mode.take();

    // The last chance to say that the log of the session being left stopped
    // being written. After this there is no session to say it about.
    if let Some(problem) = unclosed {
        renderer.commit(&format!("! {problem}"))?;
    }

    // The transcript on screen was said by the session just left, and an empty
    // context is what was asked for — a screen still holding it would read as a
    // conversation the agent under the box has no memory of. What was held of
    // that session's results goes with the rows that offered them: a key
    // opening what is behind a row nobody can see is worse than no offer.
    held.kept.forget();

    // The images pasted go too: the markers naming them were in prompts of the
    // session just left, and the numbering starts over with the session. Held
    // on, one would ride the first prompt after the clear that says `[Image #1]`.
    held.images.clear();
    renderer.empties()?;

    // The opening again, and nothing else: a screen that looks exactly like a
    // fresh start is the whole of what says one happened. The card's facts
    // were read at launch, so its list of recent sessions does not yet name
    // the one just left — the price of a card that never disagrees with the
    // one the launch drew.
    held.opening.commit(renderer)?;
    Ok(())
}

#[cfg(test)]
mod tests;
