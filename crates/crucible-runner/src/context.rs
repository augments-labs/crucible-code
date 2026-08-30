//! What one run is, while it is running.
//!
//! The turn loop needs six things that are not the session: who this run is,
//! how to stop it, what the reader typed at it, what finished behind it, where
//! its progress goes, and what it may spend. They travelled as loose arguments
//! through every signature the cancel already crossed, and four functions in
//! the loop carried a note apologising for their length — `turn`, `exchange`
//! and `made_room` here, `recap` in the compaction module. Bundling them is
//! what took all four notes away.
//!
//! Bundled here, borrowed rather than cloned: the run does not own the channel
//! it posts to or the flag the keyboard raises, and a bundle that owned them
//! would be a second thing to keep in step with the one the caller has.
//!
//! The permission prompt stays outside. It is `&mut` — asking is a
//! conversation with a person, and the answer is remembered — so it cannot
//! join a bundle every reader of the loop holds shared.

use std::fmt;

use crucible_core::{Ancestry, Aside, Cancel, Post, Reporter, RunId, Steer};

use crate::policy::RunPolicy;

/// One run, and the services it runs against.
pub struct RunContext<'a> {
    /// Which run this is, and which run started it.
    ancestry: Ancestry,

    /// What this run may spend, after everything above it has had its say.
    policy: RunPolicy,

    /// Where progress goes, because the thread that draws is not this one.
    ///
    /// Private, because reaching it directly is how an event gets reported
    /// without saying which run it belongs to. [`RunContext::reporting`] is
    /// the way to it, and it stamps.
    to: &'a dyn Post,

    /// Whether somebody has asked this run to stop.
    cancel: &'a Cancel,

    /// Lines the reader typed while the run was working.
    steer: &'a Steer,

    /// And what finished behind it while it worked.
    aside: &'a Aside,
}

impl<'a> RunContext<'a> {
    /// A run nothing started: its own root, under the policy it was given.
    ///
    /// Crate-visible, because a root is what a descendant is not: everything
    /// outside reaches a context through the runner that started it, and a
    /// nested run is spelled [`RunContext::child`] or it is not spelled at
    /// all. Public, this is the two-line way to give a descendant a fresh
    /// ancestry and a policy nobody narrowed.
    #[must_use]
    pub(crate) fn new(
        policy: RunPolicy,
        events: &'a dyn Post,
        cancel: &'a Cancel,
        steer: &'a Steer,
        aside: &'a Aside,
    ) -> Self {
        Self {
            ancestry: Ancestry::new(),
            policy,
            to: events,
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
    /// parent is stopped, and its progress reaches the same screen. They are
    /// private fields with read-only accessors, so that is a property of the
    /// type rather than of what its callers remember to do. Nothing calls this
    /// yet — it is here because the ancestry and the narrowing are the two
    /// things that have to be right before anything does.
    #[must_use]
    pub fn child(&self, wanted: RunPolicy) -> Self {
        Self {
            ancestry: self.ancestry.child(),
            policy: self.policy.narrowed(wanted),
            to: self.to,
            cancel: self.cancel,
            steer: self.steer,
            aside: self.aside,
        }
    }

    /// The same run, held to `ceiling` as well as to what it already holds.
    ///
    /// Not [`RunContext::child`]: no new run is started and the ancestry is
    /// kept, so the events a turn posts and the result it returns still name
    /// one run. What changes is only the policy, by the same rule a descendant
    /// gets — [`RunPolicy::narrowed`], with the holder on the left.
    ///
    /// This is how the session's own policy becomes a ceiling rather than a
    /// starting point. A context is minted from the session's policy at the
    /// moment it is asked for, and a session narrowed after that would
    /// otherwise run the turn under the figure it used to hold; a caller can
    /// also hand in a context it built for a different session entirely.
    /// Holding it here costs nothing when the two agree, which is every call
    /// the binary makes.
    #[must_use]
    pub(crate) fn held_to(&self, ceiling: RunPolicy) -> Self {
        Self {
            ancestry: self.ancestry,
            policy: ceiling.narrowed(self.policy),
            to: self.to,
            cancel: self.cancel,
            steer: self.steer,
            aside: self.aside,
        }
    }

    /// Where this run reports, saying it was this run that did.
    ///
    /// Returned by value rather than borrowed from the context, so the loop can
    /// keep one in hand while it holds the runner mutably. Both halves are
    /// cheap: an [`Ancestry`] is `Copy` and the destination is a shared borrow
    /// this context is already holding.
    #[must_use]
    pub const fn reporting(&self) -> Reporter<'a> {
        Reporter::new(self.ancestry, self.to)
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

    /// Whether somebody has asked this run to stop.
    ///
    /// Read-only for the same reason the policy is: a descendant that could
    /// be pointed at a different flag would go on working after the run that
    /// started it was stopped, which is the one thing [`RunContext::child`]
    /// hands down specifically so it cannot happen.
    #[must_use]
    pub const fn cancel(&self) -> &'a Cancel {
        self.cancel
    }

    /// Lines the reader typed while this run was working.
    #[must_use]
    pub const fn steer(&self) -> &'a Steer {
        self.steer
    }

    /// What finished behind this run while it worked.
    #[must_use]
    pub const fn aside(&self) -> &'a Aside {
        self.aside
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

    use crucible_core::{Event, EventEnvelope};

    use std::time::Duration;

    use crate::policy::{Bounds, Compaction, Retry};

    /// Somewhere for a context's services to point, since these tests are
    /// about what the context says rather than what it carries.
    struct Nowhere {
        events: std::sync::mpsc::Sender<EventEnvelope>,
        seen: std::sync::mpsc::Receiver<EventEnvelope>,
        cancel: Cancel,
        steer: Steer,
        aside: Aside,
    }

    impl Nowhere {
        fn new() -> Self {
            let (events, seen) = channel();
            Self {
                events,
                seen,
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
            retry: Retry {
                attempts: 1,
                first_pause: Duration::from_millis(250),
            },
            compaction: Compaction {
                keep_tokens: 1_000,
                recap_tokens: 256,
                ..Compaction::default()
            },
        });

        let child = run.child(RunPolicy {
            bounds: Bounds {
                response_bytes: usize::MAX,
                tool_output_bytes: usize::MAX,
                spend: None,
            },
            retry: Retry {
                attempts: u8::MAX,
                first_pause: Duration::from_hours(1),
            },
            compaction: Compaction {
                keep_tokens: u64::MAX,
                recap_tokens: u32::MAX,
                ..Compaction::default()
            },
        });

        // Every budget, not only the three in `Bounds`: the name says what a
        // descendant may not do, so the body has to look at everything it
        // could have done it with.
        assert_eq!(child.policy().bounds.response_bytes, 1024);
        assert_eq!(child.policy().bounds.tool_output_bytes, 512);
        assert_eq!(child.policy().bounds.spend, Some(50));
        assert_eq!(child.policy().retry.attempts, 1);
        assert_eq!(child.policy().retry.first_pause, Duration::from_millis(250));
        assert_eq!(child.policy().compaction.keep_tokens, 1_000);
        assert_eq!(child.policy().compaction.recap_tokens, 256);
    }

    #[test]
    fn a_run_held_to_a_ceiling_is_the_same_run_under_less() {
        // Not `child`: holding a run to a ceiling starts nothing. If it minted
        // a run the events a turn posts would name one run and the result it
        // returns another, which is the shape attribution exists to prevent.
        let nowhere = Nowhere::new();
        let asking = nowhere.context(RunPolicy::default());

        let held = asking.held_to(RunPolicy {
            bounds: Bounds {
                spend: Some(50),
                ..Bounds::default()
            },
            ..RunPolicy::default()
        });

        assert_eq!(held.run(), asking.run(), "a ceiling started a second run");
        assert_eq!(held.ancestry().root(), asking.ancestry().root());
        assert_eq!(held.ancestry().parent(), asking.ancestry().parent());
        assert_eq!(held.ancestry().depth(), asking.ancestry().depth());
        assert_eq!(held.policy().bounds.spend, Some(50));
    }

    #[test]
    fn a_ceiling_wider_than_the_run_it_holds_leaves_it_where_it_was() {
        // The direction that would make this a way round the rule rather than
        // the rule itself: the loop calls it with the session's policy on every
        // turn, and a run already narrower must stay where it is.
        let nowhere = Nowhere::new();
        let asking = nowhere.context(RunPolicy {
            bounds: Bounds {
                spend: Some(50),
                ..Bounds::default()
            },
            ..RunPolicy::default()
        });

        let held = asking.held_to(RunPolicy::default());

        assert_eq!(
            held.policy().bounds.spend,
            Some(50),
            "a wider ceiling lifted the bound the run was already under"
        );
    }

    #[test]
    fn the_services_a_run_was_given_are_the_ones_it_hands_down() {
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy::default());
        let child = run.child(RunPolicy::default());

        // All four, not the one that is easiest to reach: the name claims the
        // services, and a `child` that pointed any of them somewhere else —
        // a fresh queue, a second flag — would go on working against a
        // parent that had stopped, or swallow what the reader typed.
        nowhere.cancel.request();
        nowhere.steer.say("a line".into());
        nowhere.aside.say("a note".into());
        child.reporting().post(Event::Delta {
            text: "said".into(),
        });

        assert!(
            child.cancel().requested(),
            "a descendant did not hear the stop its parent heard"
        );
        assert_eq!(
            child.steer().take(),
            ["a line"],
            "a descendant read a different queue than the one typed into"
        );
        assert_eq!(
            child.aside().take(),
            ["a note"],
            "a descendant read different notes than the ones left"
        );
        assert!(
            matches!(
                nowhere.seen.try_recv().map(EventEnvelope::into_event),
                Ok(Event::Delta { text }) if &*text == "said"
            ),
            "a descendant reported somewhere its parent was not reading"
        );
    }
}
