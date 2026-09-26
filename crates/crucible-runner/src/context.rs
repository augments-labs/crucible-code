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

use crucible_core::{Ancestry, Aside, Cancel, RunId, Steer};

use crate::policy::RunPolicy;

use crate::{Post, Reporter};
/// One run, and the services it runs against.
///
/// A caller reaches one through [`Runner::starting`], and what it reaches is a
/// root. Descending is this crate's, because a context minted outside it would
/// file a turn's events under runs that never ran:
///
/// The error code is what this fails with today and not a gate: `compile_fail`
/// accepts any compile error, so the snippet is kept to the one call that must
/// not compile.
///
/// ```compile_fail,E0624
/// use crucible_runner::RunContext;
///
/// fn nested<'a>(run: &'a RunContext<'a>) -> RunContext<'a> {
///     run.child(unimplemented!())
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

    /// The stop scope this run answers to, where one started it.
    ///
    /// `None` on a root: a root answers to the flag it was handed. Set on a
    /// descendant to a child of the scope the run that started it answers
    /// to, so requesting the descendant stops it without reaching the run
    /// that started it, while a request of the
    /// parent still reaches down. Owned here rather than borrowed beside
    /// `cancel` because no caller holds it: the scope exists for this run
    /// alone, and handing it out would let a second run answer to a stop
    /// minted for the first.
    scope: Option<Cancel>,

    /// Lines the reader typed while the run was working.
    steer: &'a Steer,

    /// And what finished behind it while it worked.
    aside: &'a Aside,
}

impl<'a> RunContext<'a> {
    /// A run nothing started: its own root, under the policy it was given.
    ///
    /// Crate-visible, because the policy argument is the whole of what this
    /// run may spend and nothing here checks it against a session. That check
    /// is [`Runner::starting`]'s, which is why it is the only way in from
    /// outside; reaching this directly would be the way to a run holding more
    /// than the session it belongs to.
    ///
    /// [`Runner::starting`]: crate::Runner::starting
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
            scope: None,
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
    /// The spend bound narrows differently again: it is reserved, not merely
    /// capped. What the descendant is granted leaves this run — a descendant
    /// granted sixty of a hundred leaves forty behind, and one granted the
    /// whole of what is left leaves nothing. Two descendants started in turn
    /// split the one pool they were started under, so the tree as a whole
    /// cannot outspend the root it descends from. Only the spend bound is a
    /// pool: it is the one figure denominated in units the tree consumes
    /// additively, while the byte ceilings bound one turn's peak memory and
    /// every other figure bounds behaviour a descendant inherits. An unbounded
    /// run grants without depleting, because unbounded minus a grant is still
    /// unbounded.
    ///
    /// Taking `&mut self` is that reservation: starting a run rewrites what
    /// the run that started it may still spend.
    ///
    /// The descendant answers to a stop scope of its own, a child of the
    /// scope this run answers to. Requesting the descendant stops it without stopping this
    /// run; requesting this run still reaches the descendant, because the
    /// scope observes its parent. The scope is owned by the descendant's
    /// context, so [`RunContext::cancel`] answers with it rather than with
    /// the flag the descendant was started under.
    ///
    /// The services are handed straight down otherwise: a descendant's
    /// progress reaches the same screen, and it reads the same typed lines
    /// and notes. They are private fields with read-only accessors, so that
    /// is a property of the type rather than of what its callers remember to
    /// do. Nothing calls this yet — it is here because the ancestry, the
    /// reservation and the narrowing are the three things that have to be
    /// right before anything does.
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
    pub(crate) fn child(&mut self, wanted: RunPolicy) -> Self {
        let policy = self.policy.narrowed(wanted);
        // The grant leaves the holder: `narrowed` capped it at what this run
        // holds, so the remainder is what is left. An unbounded holder grants
        // without depleting.
        self.policy.bounds.spend = match (self.policy.bounds.spend, policy.bounds.spend) {
            (Some(held), Some(granted)) => Some(held.saturating_sub(granted)),
            (remaining, _) => remaining,
        };
        Self {
            ancestry: self.ancestry.child(),
            policy,
            to: self.to,
            cancel: self.cancel,
            scope: Some(self.cancel().child()),
            steer: self.steer,
            aside: self.aside,
        }
    }

    /// The same run, held to `ceiling` as well as to what it already holds.
    ///
    /// Not [`RunContext::child`]: no new run is started and the ancestry is
    /// kept, so the events a turn posts and the result it returns still name
    /// one run. What changes is only the policy, by the same rule a descendant
    /// gets — [`RunPolicy::narrowed`], with the holder on the left — and
    /// without the reservation [`RunContext::child`] keeps: holding spends
    /// nothing, so nothing leaves. The stop scope travels with the run for the
    /// same reason the ancestry does: a held descendant answers to the scope
    /// it was started under, not to the flag that scope observes.
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
            scope: self.scope.clone(),
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
    /// A descendant answers with the scope it was started under rather than
    /// with the flag that scope observes, so stopping the descendant ends it
    /// without ending the run around it. Read-only for the same reason the
    /// policy is: a descendant that could be pointed at a different flag would
    /// go on working after the run that started it was stopped, and a
    /// descendant is handed this one down specifically so that cannot happen.
    #[must_use]
    pub fn cancel(&self) -> &Cancel {
        self.scope.as_ref().unwrap_or(self.cancel)
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
    use std::time::Duration;

    use crate::{Event, EventEnvelope};

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
        let mut run = nowhere.context(RunPolicy::default());
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
        let mut run = nowhere.context(RunPolicy {
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
            tools: crate::ToolScheduling::default(),
            prompt_cache: crucible_core::PromptCachePolicy::default(),
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
            tools: crate::ToolScheduling::default(),
            prompt_cache: crucible_core::PromptCachePolicy::default(),
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

    /// A spend figure for one run: everything else this test's parent holds is
    /// the default, so the assertions below read only what reservation moves.
    fn spending(parent: Option<u64>) -> RunPolicy {
        RunPolicy {
            bounds: Bounds {
                spend: parent,
                ..Bounds::default()
            },
            ..RunPolicy::default()
        }
    }

    #[test]
    fn a_child_granted_spend_leaves_its_parent_with_the_remainder() {
        // The reservation: what the descendant was granted is no longer the
        // run that started it's to spend. A parent that kept its whole ceiling
        // beside the grant would let the tree outspend the root.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(spending(Some(100)));

        let child = run.child(spending(Some(60)));

        assert_eq!(
            child.policy().bounds.spend,
            Some(60),
            "a descendant was granted more than it asked for"
        );
        assert_eq!(
            run.policy().bounds.spend,
            Some(40),
            "starting a descendant left the run that started it with its whole ceiling"
        );
    }

    #[test]
    fn a_child_that_asks_for_more_spend_than_its_parent_holds_gets_what_is_left() {
        // Asking for more is not refused, by the narrowing rule: the
        // descendant gets what the run holds. Reservation is what that takes
        // away — the whole of what is left, down to nothing.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(spending(Some(40)));

        let child = run.child(spending(None));

        assert_eq!(
            child.policy().bounds.spend,
            Some(40),
            "an unbounded ask under a bounded run came back unbounded"
        );
        assert_eq!(
            run.policy().bounds.spend,
            Some(0),
            "a descendant granted the whole remainder left its parent able to spend"
        );
    }

    #[test]
    fn an_unbounded_parent_grants_spend_without_depleting() {
        // Reserving from no bound is not spending it down: unbounded minus a
        // grant is still unbounded, so the next descendant asks against the
        // same absence.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(spending(None));

        let one = run.child(spending(Some(50)));
        let two = run.child(spending(Some(50)));

        assert_eq!(one.policy().bounds.spend, Some(50));
        assert_eq!(two.policy().bounds.spend, Some(50));
        assert_eq!(
            run.policy().bounds.spend,
            None,
            "granting from an unbounded run bounded it"
        );
    }

    #[test]
    fn two_children_started_in_turn_split_the_one_pool_they_were_started_under() {
        // The count the tree holds: the second descendant asks against what
        // the first left, so it cannot be granted what is already spoken for.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(spending(Some(100)));

        let one = run.child(spending(Some(60)));
        let two = run.child(spending(Some(60)));

        assert_eq!(one.policy().bounds.spend, Some(60));
        assert_eq!(
            two.policy().bounds.spend,
            Some(40),
            "a second descendant was granted spend the first had already reserved"
        );
        assert_eq!(run.policy().bounds.spend, Some(0));
    }

    #[test]
    fn requesting_a_descendant_does_not_stop_the_run_that_started_it() {
        // The narrowing: a descendant answers to a scope of its own, so
        // stopping it ends it without ending the run around it.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(RunPolicy::default());

        let child = run.child(RunPolicy::default());
        child.cancel().request();

        assert!(
            child.cancel().requested(),
            "requesting a descendant left the descendant running"
        );
        assert!(
            !run.cancel().requested(),
            "stopping a descendant stopped the run that started it"
        );
    }

    #[test]
    fn a_request_of_the_parent_still_reaches_its_descendant() {
        // The other half of the narrowing: the descendant's scope observes
        // its parent, so stopping the run ends what it started.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(RunPolicy::default());

        let child = run.child(RunPolicy::default());
        run.cancel().request();

        assert!(
            child.cancel().requested(),
            "a request of the run did not reach its descendant"
        );
    }

    #[test]
    fn stopping_an_intermediate_run_stops_its_descendant_without_stopping_the_root() {
        // Depth 2 chains: the grandchild's scope observes the intermediate
        // scope rather than the root directly, so requesting the run in the
        // middle reaches what it started without reaching what started it.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(RunPolicy::default());

        let mut child = run.child(RunPolicy::default());
        let grandchild = child.child(RunPolicy::default());
        child.cancel().request();

        assert!(
            grandchild.cancel().requested(),
            "stopping an intermediate run left its descendant running"
        );
        assert!(
            !run.cancel().requested(),
            "stopping an intermediate run stopped the root"
        );
    }

    #[test]
    fn holding_a_run_to_a_ceiling_keeps_the_stop_scope_it_already_had() {
        // Not `child`: holding re-narrows the run rather than starting one,
        // so the scope travels with it — stopping through the held handle
        // stops the descendant, and neither stops the run that started it.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(RunPolicy::default());

        let child = run.child(RunPolicy::default());
        let held = child.held_to(spending(Some(10)));

        assert_eq!(held.policy().bounds.spend, Some(10));
        held.cancel().request();

        assert!(
            child.cancel().requested(),
            "a held run answered to a different stop than the run it holds"
        );
        assert!(
            !run.cancel().requested(),
            "stopping a held descendant stopped the run that started it"
        );
    }

    #[test]
    fn a_reserved_budget_and_a_narrowed_stop_hold_across_an_await() {
        // The turn that reads these is asynchronous: what the reservation
        // granted and the scope narrowed must be what the turn still sees
        // after a suspension point, not only at the moment of minting.
        crate::fake::Awaited::awaited(async {
            let nowhere = Nowhere::new();
            let mut run = nowhere.context(spending(Some(100)));

            let child = run.child(spending(Some(60)));
            tokio::task::yield_now().await;

            assert_eq!(child.policy().bounds.spend, Some(60));
            assert_eq!(run.policy().bounds.spend, Some(40));
            child.cancel().request();
            assert!(!run.cancel().requested());
        });
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
        // The same argument-order question as `held_to`, on the entry that
        // starts a descendant rather than the one that re-narrows a run that
        // already exists. `reserve` is what makes it visible: a holder that
        // named none keeps its silence, so the two orders answer differently
        // and an inverted `narrowed` shows up here rather than in whichever
        // caller first starts a descendant.
        let nowhere = Nowhere::new();
        let mut run = nowhere.context(RunPolicy::default());

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
        let mut run = nowhere.context(RunPolicy::default());
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
        let mut run = nowhere.context(RunPolicy::default());
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
