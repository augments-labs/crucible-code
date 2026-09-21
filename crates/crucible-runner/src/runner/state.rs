//! What one run accumulates, as against what its agent is.
//!
//! The runner used to hold both on one value, and the two were told apart only
//! by which fields a reader happened to know were rewritten mid-session. They
//! are opposite kinds of thing. A definition is settled and shared — two runs
//! may hold the same one at the same time — while everything here belongs to
//! exactly one run and is written on almost every pass.
//!
//! Splitting them is what makes the sharing safe to state: a definition behind
//! an `Arc` cannot be written to, and everything that *is* written lives here,
//! reachable only through the one runner that owns it. Two runs of two agents
//! then share nothing by construction rather than by nobody having written the
//! line that would have shared it.
//!
//! What is *not* here: the provider, the toolset source, the session log, the
//! permission memory and the policy. Those are services and settings a run is
//! given rather than things it accumulates, and they stay on the runner.

use std::fmt;

use crucible_agents::Rejection;
use crucible_core::{PromptCacheAttempt, PromptCacheScopeDigest, ToolSnapshot, Transcript, TurnId};

use super::load::Load;

/// Everything one run has accumulated so far.
///
/// Held by the runner and written by it. Published because a run's state is
/// half of what this crate is about and a caller reading a runner should be
/// able to name the thing it is reading; nothing outside the crate can build
/// one or write to one.
#[derive(Debug)]
pub struct RunState {
    /// What has been said, in the order it was said.
    pub(super) transcript: Transcript,

    /// Which turn the session is on.
    pub(super) turn: TurnId,

    /// What the next request would carry, and what has been spent reaching it.
    pub(super) load: Load,

    /// The exact immutable generation last admitted, narrowed to what this
    /// run's agent declares.
    pub(super) tools: ToolSnapshot,

    /// How much the model in force accepts at once, as this run now believes.
    ///
    /// Seeded from the definition and corrected by what a provider reports: a
    /// request that carried more than the figure written down disproves it, and
    /// the run goes on without one rather than against a number nobody has.
    /// That correction is a fact about this run's evidence, not about the
    /// agent, which is why it is written here and the definition is not
    /// rewritten to hold it.
    pub(super) window: Option<u32>,

    /// The input-guardrail decision the invocation under way has committed to,
    /// if one is.
    ///
    /// Cleared the moment that invocation ends, so it covers a turn that failed
    /// and is being tried again rather than the next thing the caller types.
    pub(super) checked: Option<Checked>,

    /// Latest complete cache state known for one provider send.
    pub(super) prompt_cache_attempt: Option<PromptCacheAttempt>,

    /// The scope the last attempt's persistent resources were owned under.
    pub(super) prompt_cache_owner_scope: Option<PromptCacheScopeDigest>,

    /// The first line the session would not take between turns, held for the
    /// next turn or compaction to end on before it records or sends anything.
    ///
    /// Only the first refusal is kept. It survives picking a session up and is
    /// then reported on the session picked up, naming the bridge rather than
    /// the session; one still held when the runner is dropped is reported to
    /// nobody.
    pub(super) unwritten: Option<crucible_runtime::Unready>,
}

impl RunState {
    /// A run that has not said anything yet, under a model believed to accept
    /// `window`.
    pub(super) fn new(window: Option<u32>) -> Self {
        Self {
            transcript: Transcript::new(),
            turn: TurnId::FIRST,
            load: Load::default(),
            tools: ToolSnapshot::empty(),
            window,
            checked: None,
            prompt_cache_attempt: None,
            prompt_cache_owner_scope: None,
            unwritten: None,
        }
    }

    /// The same, over a roster the caller already materialized.
    pub(super) fn offering(window: Option<u32>, tools: ToolSnapshot) -> Self {
        Self {
            tools,
            ..Self::new(window)
        }
    }

    /// What has been said, in the order it was said.
    #[must_use]
    pub const fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Which turn the session is on.
    #[must_use]
    pub const fn turn(&self) -> TurnId {
        self.turn
    }

    /// How much the model in force accepts at once, as this run now believes.
    #[must_use]
    pub const fn context_window(&self) -> Option<u32> {
        self.window
    }

    /// What this run's input checks made of `asked`, where they have already
    /// been asked about exactly that for the invocation still under way.
    ///
    /// An invocation retried after it failed asks the same question again and is
    /// answered from here rather than by running the checks a second time: the
    /// decision was committed once, and a check that reached out to something
    /// that has since changed its mind must not be able to overturn an
    /// invocation already under way. Different words are a different invocation,
    /// and so are the same words after the invocation they were checked for has
    /// ended.
    pub(super) fn checked(&self, asked: &str) -> Option<&Judged> {
        self.checked
            .as_ref()
            .filter(|checked| &*checked.asked == asked)
            .map(|checked| &checked.decision)
    }

    /// Commits what the input checks made of `asked`, for a retry to restore.
    pub(super) fn commit(&mut self, asked: &str, decision: Judged) {
        self.checked = Some(Checked {
            asked: asked.into(),
            decision,
        });
    }

    /// Ends the invocation a decision was committed for.
    ///
    /// Called where an invocation finishes, and where the session it belonged
    /// to is left behind. What follows is words said to something else, whatever
    /// they spell, and a check exists to be asked about them.
    pub(super) fn forget_checked(&mut self) {
        self.checked = None;
    }
}

/// One committed input decision, and the words it was reached about.
///
/// `Debug` redacts the words, which are the reader's prompt.
pub(super) struct Checked {
    /// Exactly what was asked, so that different words are a different
    /// invocation rather than one this decision covers.
    asked: Box<str>,
    /// What the checks made of it.
    decision: Judged,
}

/// What one pass of checks came to.
///
/// A check answers with its reason alone. This is that answer once the runner
/// has written it under the name of the check it asked, which is the only place
/// a refusal gets a name from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Judged {
    /// Every check allowed it, or there were none.
    Allowed,
    /// One refused, and the rest were not asked.
    Rejected(Rejection),
}

impl fmt::Debug for Checked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Checked")
            .field("asked", &"[redacted]")
            .field("decision", &self.decision)
            .finish()
    }
}
