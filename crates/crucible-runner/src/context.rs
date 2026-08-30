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
///
/// A caller reaches one through [`Runner::starting`], and what it reaches is a
/// root. Descending is this crate's, because a context minted outside it would
/// file a turn's events under runs that never ran:
///
/// ```compile_fail,E0624
/// use crucible_runner::{RunContext, RunPolicy};
///
/// fn nested<'a>(run: &'a RunContext<'a>) -> RunContext<'a> {
///     run.child(RunPolicy::default())
/// }
/// ```
///
/// [`Runner::starting`]: crate::Runner::starting
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
    /// skipped by a caller that writes the comparison itself.
    ///
    /// Most of `wanted` narrows the way it reads: ask for less and you get
    /// less, ask for more and you get what this run holds. Three do not, and
    /// they are the ones worth knowing before calling this.
    /// [`Compaction::reserve`] is room held back, so the *larger* figure is
    /// the narrower answer — and a run that named no reserve at all is not
    /// given one by its descendant, because what absence stands for is derived
    /// from model ceilings and cannot be compared against a number.
    /// [`Compaction::automatic`] can only be switched off by a descendant,
    /// never back on. [`Compaction::ask_on_resume`] is this run's outright and
    /// a descendant's is dropped whichever way the two differ.
    ///
    /// The services are handed straight down: a descendant stops when its
    /// parent is stopped, and its progress reaches the same screen. They are
    /// private fields with read-only accessors, so that is a property of the
    /// type rather than of what its callers remember to do. Nothing calls this
    /// yet — it is here because the ancestry and the narrowing are the two
    /// things that have to be right before anything does.
    ///
    /// Not published while that is true. A descendant is a run this crate
    /// starts, and one minted from outside would deepen the ancestry of every
    /// event a turn posts without a run ever having been there.
    ///
    /// [`Compaction::reserve`]: crate::Compaction::reserve
    /// [`Compaction::automatic`]: crate::Compaction::automatic
    /// [`Compaction::ask_on_resume`]: crate::Compaction::ask_on_resume
    #[must_use]
    // Defined before the first nested run, so outside the tests that prove the
    // narrowing there is no caller yet. `expect` rather than `allow`, and only
    // where it is true: the attribute goes away by itself the day a phase adds
    // one, rather than sitting here silencing the question forever.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller is a later phase's")
    )]
    pub(crate) fn child(&self, wanted: RunPolicy) -> Self {
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
    /// it, and the whole point of descending is that it cannot.
    #[must_use]
    pub const fn policy(&self) -> &RunPolicy {
        &self.policy
    }

    /// Whether somebody has asked this run to stop.
    ///
    /// Read-only for the same reason the policy is: a descendant that could
    /// be pointed at a different flag would go on working after the run that
    /// started it was stopped, and a descendant is handed this one down
    /// specifically so that cannot happen.
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
    fn a_ceiling_is_read_as_the_holder_and_not_as_the_thing_held() {
        // Which side of `RunPolicy::narrowed` the ceiling goes on. Seven of
        // the other nine figures resolve by `min` or `&&`, and `spend` by
        // `tighter`; all of those are symmetric, so writing the two arguments
        // the wrong way round is invisible in every one of them. `reserve` is
        // the second field that can see a swap, and only where one side is
        // absent. This is the field that sees it whenever the two differ at
        // all, which is why the order is pinned here.
        let nowhere = Nowhere::new();
        let asking = nowhere.context(RunPolicy {
            compaction: Compaction {
                ask_on_resume: Some(999),
                ..Compaction::default()
            },
            ..RunPolicy::default()
        });

        let held = asking.held_to(RunPolicy {
            compaction: Compaction {
                ask_on_resume: Some(10),
                ..Compaction::default()
            },
            ..RunPolicy::default()
        });

        assert_eq!(
            held.policy().compaction.ask_on_resume,
            Some(10),
            "the run was read as the ceiling and the session as the request"
        );
    }

    #[test]
    fn a_run_that_starts_one_is_read_as_the_holder_and_not_as_the_thing_asked_for() {
        // The same argument-order question as `held_to`, on the public entry
        // point rather than the crate-private one. `reserve` is what makes it
        // visible: a holder that named none keeps its silence, so the two
        // orders answer differently and an inverted `narrowed` shows up here
        // rather than in whichever caller first starts a descendant.
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy::default());

        let child = run.child(RunPolicy {
            compaction: Compaction {
                reserve: Some(5_000),
                ..Compaction::default()
            },
            ..RunPolicy::default()
        });

        assert_eq!(
            child.policy().compaction.reserve,
            None,
            "the request was read as the holder and the run as the thing asked for"
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

    #[test]
    fn an_event_a_descendant_posts_names_the_descendant_rather_than_the_root() {
        // The test above drops the attribution before it looks, which is right
        // for what it asks and is exactly what leaves this unasked: every
        // envelope carries the whole ancestry, and the question is which of the
        // two ids the reader is handed as the one that produced the event.
        //
        // A reader telling two runs apart is the only thing that can see the
        // difference, and there is no such reader yet — so this is the
        // assertion that has to exist before there is one, because an event
        // already drawn was drawn without saying whose it was.
        let nowhere = Nowhere::new();
        let run = nowhere.context(RunPolicy::default());
        let child = run.child(RunPolicy::default());

        child.reporting().post(Event::Delta {
            text: "said".into(),
        });

        let reported = nowhere.seen.try_recv().expect("what the descendant posted");

        assert_eq!(
            reported.run(),
            child.run(),
            "a descendant's event was filed under the run it descends from"
        );
        assert_ne!(
            reported.run(),
            reported.ancestry().root(),
            "the two ids this distinguishes are the same, so it distinguishes nothing"
        );
    }
}
