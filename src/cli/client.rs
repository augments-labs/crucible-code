//! The terminal as one front end of the application.
//!
//! What this loop asks of a conversation it asks the way any front end does:
//! as a [`Request`] of the client contract — the protocol version, what this
//! front end can answer, an identity of its own, and one command — carried
//! out by [`crucible_app::client`]. A consumer with no terminal comes through
//! the same doors with the same values, so there is nothing the keyboard can
//! do to a conversation that it cannot.
//!
//! What comes back is the application's own answer, and the command that
//! asked draws it. The sentences and rows stay on this side: a screen is made
//! from the whole value, never from the copy the contract cuts to its
//! ceilings, and nothing drawn is ever serialized.
//!
//! A terminal has somebody at it and a screen, so it claims every capability.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_app::Conversation;
use crucible_app::client::{Desk, Performed, interrupt, keep, perform};
use crucible_app::providers::Providers;
use crucible_client_api::{
    Capabilities, Command, Correlation, Decision, Name, Outcome, Pending, Refusal, Request, Theme,
};
use crucible_core::Cancel;

use super::converse::Terms;

/// What is told as requests, answers, pending actions and decisions cross
/// between this front end and the application.
///
/// Nothing that ships listens: a terminal draws what it is answered and keeps
/// no record of it, so [`Client::new`] holds no witness and every crossing
/// costs one branch. It is here so that what crosses can be held beside what a
/// client with no terminal was answered by the code that ships, with nothing
/// compiled in or out to let it.
pub(crate) trait Witness: std::fmt::Debug + Send + Sync {
    /// `request` was answered `outcome`, and `conversation` stands as it does
    /// once it had been.
    fn answered(&self, request: &Request, conversation: &Conversation, outcome: Outcome);

    /// `request`, which needs no conversation, was answered `outcome`.
    fn apart(&self, request: &Request, outcome: Outcome);

    /// A turn stopped on `pending`, and the terminal was put it.
    fn put(&self, pending: &Pending);

    /// What the terminal said about the action it was put.
    fn decided(&self, decision: &Decision);
}

/// The requests this process has made, numbered as they are made.
///
/// Cloned to the worker a turn runs on: both sides number from one count, so
/// an identity names one request for as long as the process runs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Client {
    sent: Arc<AtomicU64>,
    /// Who is told what crosses, where anybody is.
    witness: Option<Arc<dyn Witness>>,
}

impl Client {
    /// A client that has asked nothing yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The next request, asking for `command`.
    pub(crate) fn asking(&self, command: Command) -> Request {
        let number = self.sent.fetch_add(1, Ordering::Relaxed).saturating_add(1);

        Request::new(Capabilities::every(), Correlation::new(number), command)
    }

    /// Asks the turn `cancel` belongs to to stop.
    ///
    /// What the application answers is that it heard, and the turn says the
    /// rest in its own outcome, so there is nothing here to draw.
    pub(crate) fn interrupt(&self, cancel: &Cancel) {
        let request = self.asking(Command::Cancel);
        let heard = interrupt(&request, cancel);

        if let Some(witness) = &self.witness {
            witness.apart(&request, heard);
        }
    }

    /// Tells whoever is listening what `request` was answered with. The
    /// outcome is made only where somebody is.
    pub(crate) fn answered(
        &self,
        request: &Request,
        conversation: &Conversation,
        outcome: impl FnOnce() -> Outcome,
    ) {
        if let Some(witness) = &self.witness {
            witness.answered(request, conversation, outcome());
        }
    }

    /// Tells whoever is listening that the terminal was put `pending`.
    pub(crate) fn put(&self, pending: &Pending) {
        if let Some(witness) = &self.witness {
            witness.put(pending);
        }
    }

    /// Tells whoever is listening what the terminal said about a pending
    /// action.
    pub(crate) fn decided(&self, decision: &Decision) {
        if let Some(witness) = &self.witness {
            witness.decided(decision);
        }
    }
}

impl Terms {
    /// Asks the application for `command`, against the providers in force as
    /// it is asked, and hands back what came of it for the caller to draw.
    pub(crate) fn perform(&self, conversation: &mut Conversation, command: Command) -> Performed {
        let request = self.client.asking(command);
        let providers = self.providers.snapshot();
        let performed = perform(conversation, &request, &self.desk(&providers));

        self.client
            .answered(&request, conversation, || performed.outcome());

        performed
    }
}

impl Terms {
    /// Asks for the command `asking` makes of `provider`, a name out of the
    /// registry. One the contract would not carry is refused here, as it would
    /// be on arriving.
    pub(crate) fn perform_naming(
        &self,
        conversation: &mut Conversation,
        provider: &str,
        asking: impl FnOnce(Name) -> Command,
    ) -> Performed {
        match Name::new(provider) {
            Ok(provider) => self.perform(conversation, asking(provider)),
            Err(refusal) => Performed::Refused(refusal),
        }
    }

    /// Asks for what is written down about this machine and is about no
    /// conversation, which is what lets it be asked while a turn has it.
    pub(crate) fn keep(&self, command: Command) -> Performed {
        let request = self.client.asking(command);
        let providers = self.providers.snapshot();
        let kept = keep(&request, &self.desk(&providers));

        if let Some(witness) = &self.client.witness {
            witness.apart(&request, kept.outcome());
        }

        kept
    }

    /// Asks the turn `cancel` belongs to to stop. Needs no conversation, which
    /// is what lets a key ask it while a turn has the conversation.
    pub(crate) fn interrupt(&self, cancel: &Cancel) {
        self.client.interrupt(cancel);
    }

    /// What the host lends a command, from the providers in force.
    fn desk<'a>(&'a self, providers: &'a Providers) -> Desk<'a> {
        Desk {
            switching: self.switching(providers),
            sessions: &self.sessions,
            workspace: &self.workspace,
            reads: |named| crucible_tui::syntax::colours(named).is_some(),
        }
    }
}

/// What asking for `theme` to be written down came to: nothing where it was,
/// and otherwise the line saying why this session is the only one drawn so.
pub(crate) fn unwritten(terms: &Terms, theme: Result<Theme, Refusal>) -> Option<String> {
    let kept = match theme {
        Ok(theme) => terms.keep(Command::Theme(theme)),
        Err(refusal) => Performed::Refused(refusal),
    };

    match kept {
        Performed::Theme(Ok(())) => None,
        Performed::Theme(Err(problem)) => Some(format!("! {problem}")),
        other => Some(astray(&other)),
    }
}

/// The one line for an answer the command that asked has no row for.
///
/// A refusal is the only one a terminal can meet: a name too long for the
/// contract, typed after `/model`. Every other arm answers a different
/// command, and is named rather than dropped so that a request routed wrongly
/// is a line on the screen and not a silence.
pub(crate) fn astray(performed: &Performed) -> String {
    match performed {
        Performed::Refused(refusal) => format!("! {refusal}"),
        other => format!("! the application answered {}", other.outcome().kind()),
    }
}

#[cfg(test)]
pub(crate) mod tests;
