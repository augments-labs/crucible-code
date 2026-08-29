//! What one run is, while it is running.
//!
//! The turn loop needs six things that are not the session: who this run is,
//! how to stop it, what the reader typed at it, what finished behind it, where
//! its progress goes, and what it may spend. They travelled as loose arguments
//! through every signature the cancel already crossed, which is why the two
//! functions in the middle of the loop carry a note apologising for their
//! length.
//!
//! Bundled here, borrowed rather than cloned: the run does not own the channel
//! it posts to or the flag the keyboard raises, and a bundle that owned them
//! would be a second thing to keep in step with the one the caller has.
//!
//! The permission prompt stays outside. It is `&mut` — asking is a
//! conversation with a person, and the answer is remembered — so it cannot
//! join a bundle every reader of the loop holds shared.

use std::fmt;

use crucible_core::{Ancestry, Aside, Cancel, Post, RunId, Steer};

use crate::policy::RunPolicy;

/// One run, and the services it runs against.
pub struct RunContext<'a> {
    /// Which run this is, and which run started it.
    ancestry: Ancestry,

    /// What this run may spend, after everything above it has had its say.
    policy: RunPolicy,

    /// Where progress goes, because the thread that draws is not this one.
    pub events: &'a dyn Post,

    /// Whether somebody has asked this run to stop.
    pub cancel: &'a Cancel,

    /// Lines the reader typed while the run was working.
    pub steer: &'a Steer,

    /// And what finished behind it while it worked.
    pub aside: &'a Aside,
}

impl<'a> RunContext<'a> {
    /// A run nothing started: its own root, under the policy it was given.
    pub fn new(
        policy: RunPolicy,
        events: &'a dyn Post,
        cancel: &'a Cancel,
        steer: &'a Steer,
        aside: &'a Aside,
    ) -> Self {
        Self {
            ancestry: Ancestry::new(),
            policy,
            events,
            cancel,
            steer,
            aside,
        }
    }

    /// A run this one started, asking for `wanted` and getting no more than
    /// this run holds.
    ///
    /// The narrowing happens here rather than at the caller so that starting a
    /// run is the only way to get a context for one, and the rule cannot be
    /// skipped by a caller that writes the comparison itself. What it does is
    /// [`RunPolicy::narrowed`].
    ///
    /// The services are handed straight down: a descendant stops when its
    /// parent is stopped, and its progress reaches the same screen. Nothing
    /// calls this yet — it is here because the ancestry and the narrowing are
    /// the two things that have to be right before anything does.
    #[must_use]
    pub fn child(&self, wanted: RunPolicy) -> Self {
        Self {
            ancestry: self.ancestry.child(),
            policy: self.policy.narrowed(wanted),
            events: self.events,
            cancel: self.cancel,
            steer: self.steer,
            aside: self.aside,
        }
    }

    /// Which run this is.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.ancestry.run()
    }

    /// Which run this is, and which run started it.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// What this run may spend.
    ///
    /// Read-only, because a run that could rewrite its own policy could widen
    /// it, and the whole point of [`RunContext::child`] is that it cannot.
    #[must_use]
    pub const fn policy(&self) -> &RunPolicy {
        &self.policy
    }
}

impl fmt::Debug for RunContext<'_> {
    /// What a run *is*, without the services it runs against.
    ///
    /// The three that can be named here are the three a reader of a log wants:
    /// which run, whose descendant, and what it may spend. Where it posts and
    /// what flag it watches are the caller's objects, and printing an address
    /// for them would say nothing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunContext")
            .field("ancestry", &self.ancestry)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc::channel;

    use crucible_core::Event;

    use crate::policy::Bounds;

    /// Somewhere for a context's services to point, since these tests are
    /// about what the context says rather than what it carries.
    struct Nowhere {
        events: std::sync::mpsc::Sender<Event>,
        cancel: Cancel,
        steer: Steer,
        aside: Aside,
    }

    impl Nowhere {
        fn new() -> Self {
            let (events, _seen) = channel();
            Self {
                events,
                cancel: Cancel::new(),
                steer: Steer::new(),
                aside: Aside::new(),
            }
        }

        fn context(&self, policy: RunPolicy) -> RunContext<'_> {
            RunContext::new(policy, &self.events, &self.cancel, &self.steer, &self.aside)
        }
    }

    #[test]
    fn a_run_nothing_started_is_its_own_root() {
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy::default());

        assert_eq!(run.ancestry().root(), run.run());
        assert_eq!(run.ancestry().parent(), None);
        assert_eq!(run.ancestry().depth(), 0);
    }

    #[test]
    fn a_run_a_run_started_names_it_and_keeps_the_root() {
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy::default());
        let child = run.child(RunPolicy::default());

        assert_eq!(child.ancestry().parent(), Some(run.run()));
        assert_eq!(child.ancestry().root(), run.run());
        assert_eq!(child.ancestry().depth(), 1);
        assert_ne!(
            child.run(),
            run.run(),
            "a descendant reused its parent's run"
        );
    }

    #[test]
    fn a_run_a_run_started_cannot_spend_more_than_the_one_that_started_it() {
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy {
            bounds: Bounds {
                response_bytes: 1024,
                tool_output_bytes: 512,
                spend: Some(50),
            },
            ..RunPolicy::default()
        });

        let child = run.child(RunPolicy {
            bounds: Bounds {
                response_bytes: usize::MAX,
                tool_output_bytes: usize::MAX,
                spend: None,
            },
            ..RunPolicy::default()
        });

        assert_eq!(child.policy().bounds.response_bytes, 1024);
        assert_eq!(child.policy().bounds.tool_output_bytes, 512);
        assert_eq!(child.policy().bounds.spend, Some(50));
    }

    #[test]
    fn the_services_a_run_was_given_are_the_ones_it_hands_down() {
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy::default());
        let child = run.child(RunPolicy::default());

        nowhere.cancel.request();

        assert!(
            child.cancel.requested(),
            "a descendant did not hear the stop its parent heard"
        );
    }
}
