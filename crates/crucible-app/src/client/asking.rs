//! Asking a plan how much of each of its limits is used.
//!
//! The one request made of a vendor outside a turn, and only of a sign-in
//! whose vendor keeps a source of those figures: the provider says whether it
//! has one, and by default none does. It is made when a client asks for it
//! with [`Command::AskLimits`] — the terminal does as `/usage` or the
//! `/settings` Usage tab opens — and never on a timer.
//!
//! It is in three steps, so a question out never holds the conversation.
//! [`asking`] decides, with the conversation in hand, whether to ask at all,
//! and starts the question. The [`Asking`] it hands back borrows nothing, so it
//! is awaited wherever the caller likes: the terminal on its runtime, while it
//! keeps drawing and reading keys. [`asked`] takes the answer back to the
//! conversation. [`perform`](super::perform) does all three in a row, for a
//! client with nothing else to do while it waits.
//!
//! Nothing is asked:
//!
//! - of a vendor that uses what it is sent, until the use of what is sent
//!   there has been agreed to — the content-use hold a turn waits on;
//! - of the same credential more than once a minute, whatever came of the
//!   last time;
//! - of a credential the source refused, or where the source was not there,
//!   for the rest of the session.
//!
//! A credential is told apart by the provider's registry name and the scope
//! its adapter says it holds, the identity a prompt cache is kept under. What
//! is remembered of them is bounded to [`KEPT`] credentials, the oldest
//! forgotten first.
//!
//! An answer replaces what the conversation knew of the plan. One that arrives
//! once the provider or its credential changed is about a plan no longer being
//! asked, and is set aside. A failure leaves what was known as it was.

use std::time::{Duration, Instant};

use crucible_client_api::{Command, ErrorCode, Refusal, Request};
use crucible_models::Asked;
use crucible_runtime::BoxFuture;
use crucible_types::CredentialScopeId;

use super::performing::Performed;
use super::reading;
use crate::Conversation;

/// How long after asking a credential's plan it is asked again at the soonest.
pub const FLOOR: Duration = Duration::from_mins(1);

/// How many credentials what was asked of them is remembered for.
pub const KEPT: usize = 16;

/// Which credential a plan was asked through: the provider's registry name and
/// the scope its adapter holds.
type Credential = (&'static str, CredentialScopeId);

/// What a conversation remembers of the plans it asked.
#[derive(Debug, Default)]
pub(crate) struct Asks {
    /// When each credential's plan was last asked, oldest first.
    sent: Vec<(Credential, Instant)>,
    /// Credentials the source refused, or that it was not there for, oldest
    /// first.
    closed: Vec<Credential>,
}

impl Asks {
    /// Whether `credential` may be asked at `now`.
    fn open(&self, credential: Credential, now: Instant) -> bool {
        let recent = self
            .sent
            .iter()
            .any(|(each, at)| *each == credential && now.saturating_duration_since(*at) < FLOOR);
        !recent && !self.closed.contains(&credential)
    }

    /// `credential` is being asked at `now`.
    fn sending(&mut self, credential: Credential, now: Instant) {
        self.sent.retain(|(each, _)| *each != credential);
        if self.sent.len() >= KEPT {
            self.sent.remove(0);
        }
        self.sent.push((credential, now));
    }

    /// `credential` is not to be asked again this session.
    fn close(&mut self, credential: Credential) {
        if self.closed.contains(&credential) {
            return;
        }
        if self.closed.len() >= KEPT {
            self.closed.remove(0);
        }
        self.closed.push(credential);
    }
}

/// A question out to a plan, borrowing nothing of the conversation that
/// started it.
pub struct Asking {
    credential: Credential,
    question: BoxFuture<'static, Asked>,
}

impl Asking {
    /// The registry name of the provider whose plan is being asked.
    #[must_use]
    pub const fn provider(&self) -> &'static str {
        self.credential.0
    }

    /// Waits for the plan's answer. Dropping this before it ends closes the
    /// request, and nothing is remembered of an answer that never came.
    pub async fn answered(self) -> Answered {
        Answered {
            credential: self.credential,
            asked: self.question.await,
        }
    }
}

impl std::fmt::Debug for Asking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asking")
            .field("provider", &self.credential.0)
            .finish_non_exhaustive()
    }
}

/// What a plan answered, to be handed back to [`asked`].
#[derive(Debug)]
pub struct Answered {
    credential: Credential,
    asked: Asked,
}

/// Starts what `request` asks of the plan of the provider `conversation` is
/// asking, as of `now`, where it is to be asked at all.
///
/// `Ok(None)` where nothing is sent: no provider is being asked, its
/// credential has no source, the use of what is sent there is not agreed to,
/// its plan was asked less than [`FLOOR`] ago, or its source refused it this
/// session. What the conversation knows is then what
/// [`Command::Usage`] reads.
///
/// # Errors
///
/// [`ErrorCode::Busy`] for any command but [`Command::AskLimits`]: this door
/// answers that one alone, and nothing is done.
pub fn asking(
    conversation: &mut Conversation,
    request: &Request,
    now: Instant,
) -> Result<Option<Asking>, Refusal> {
    match request.command() {
        Command::AskLimits => Ok(started(conversation, now)),
        Command::Prompt(_)
        | Command::Compact
        | Command::Cancel
        | Command::Decide(_)
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
        | Command::Theme(_)
        | Command::Setting { .. }
        | Command::Help
        | Command::ReleaseNotes { .. }
        | Command::Context
        | Command::Usage
        | Command::Exit => Err(ErrorCode::Busy.into()),
    }
}

/// The question to put to the plan of the provider `conversation` is asking,
/// as of `now`, where it is to be put.
pub(super) fn started(conversation: &mut Conversation, now: Instant) -> Option<Asking> {
    let credential = credential(conversation)?;
    if super::turning::unanswered(conversation).is_some()
        || !conversation.asks.open(credential, now)
    {
        return None;
    }
    let question = conversation.runner.provider().ask_limits()?;
    conversation.asks.sending(credential, now);
    Some(Asking {
        credential,
        question,
    })
}

/// Takes what a plan answered back to `conversation`, and answers with what
/// the session has used and the plan's limits as they now stand.
pub fn asked(conversation: &mut Conversation, answered: Answered) -> Performed {
    match answered.asked {
        Asked::Answered(windows) if credential(conversation) == Some(answered.credential) => {
            conversation.runner.answered_limits(windows);
        }
        Asked::Closed => conversation.asks.close(answered.credential),
        Asked::Answered(_) | Asked::Failed(_) => {}
    }
    usage(conversation)
}

/// What the session has used and the plan's limits as `conversation` knows
/// them, asking nobody.
pub(super) fn usage(conversation: &Conversation) -> Performed {
    let runner = conversation.runner();
    Performed::Usage(Box::new(reading::usage(
        runner.model(),
        &runner.breakdown(),
        &runner.totals(),
        runner.plan_limits().as_ref(),
    )))
}

/// The credential `conversation` asks through, where it asks anybody.
fn credential(conversation: &Conversation) -> Option<Credential> {
    let serving = conversation.serving()?;
    let scope = conversation
        .runner()
        .provider()
        .prompt_cache_route()
        .credential_scope;
    Some((serving, scope))
}
