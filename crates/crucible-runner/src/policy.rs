//! What one run is allowed to spend, and what it does when it runs out.
//!
//! Every figure here already bounded a turn; they were constants in the loop
//! and fields on the wiring's compaction answer. Gathering them says who owns
//! them — the run — and makes the one question that has no answer as constants
//! askable: what happens to these when a run starts another run.
//!
//! # Inheritance
//!
//! A run started by another run inherits this policy and may **narrow** it.
//! Never widen it: a descendant cannot raise a ceiling its parent is under,
//! cannot lift a spend bound its parent set, and cannot lengthen the wait its
//! parent is willing to sit through. [`RunPolicy::narrowed`] is that rule
//! written down — it takes what a descendant asks for and returns what it
//! actually gets, so a caller cannot get the answer wrong by writing the
//! comparison itself.
//!
//! Every figure here is a ceiling on one run, not a share of a pool. A
//! descendant narrowing its spend to a thousand tokens is not taking a
//! thousand out of its parent's remaining budget; it is saying that this run
//! stops at a thousand. Two descendants under one parent may each spend the
//! parent's whole ceiling, and what stops the tree as a whole from outspending
//! the root is the root's own ceiling being read on the root's own run — a
//! cumulative budget across a run tree is a different figure, held somewhere
//! that can see every run, and nothing here is it.
//!
//! # Authority
//!
//! The same rule governs what a descendant may *do*: it may narrow what it is
//! allowed to do and never widen it. That half is not implemented here, and
//! cannot be, because neither carrier of authority is a figure. Permission
//! memory is the runner's — a session remembers what it was allowed, and
//! [`Runner::pick_up`] is where it is forgotten — and the question itself goes
//! out through [`Ask`], which is `&mut` and so is a parameter to a turn rather
//! than something a [`RunContext`] can hold. When descendants exist, the
//! narrowing point for authority is whatever hands a descendant its [`Ask`]
//! and its permission memory; it is not [`RunPolicy::narrowed`], and a
//! descendant that inherited a parent's remembered "allow for this session"
//! unrestricted would be widening the rule above by the only route this file
//! does not close.
//!
//! [`Ask`]: crucible_core::Ask
//! [`Runner::pick_up`]: crate::Runner::pick_up
//! [`RunContext`]: crate::RunContext
//!
//! There is one run today and nothing calls this with a second policy. It is
//! defined now because the alternative is defining it later, once descendants
//! exist and something already depends on the ceiling it was handed being the
//! ceiling it asked for.

use std::time::Duration;

/// Everything one run may spend.
///
/// Bytes rather than counts for the two provider-controlled figures: a turn is
/// long because there is work in it, and what actually has to be bounded is
/// memory. Tokens for the spend, because that is the unit the thing being
/// spent is sold in.
///
/// The two byte figures are `usize` where every other figure here is
/// fixed-width, which is deliberate: they are compared against the length of
/// something held in memory and nothing else, so a fixed width would buy a
/// portable spelling at the price of a cast at every comparison. The day one
/// of them is written to a document or a journal line is the day it wants a
/// width that means the same thing on the machine that reads it back.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// The most provider-controlled response data one turn retains, in bytes.
    ///
    /// A bound on memory rather than on how long a turn may run: it exists
    /// against a provider that will not stop talking, and it is what keeps the
    /// peak-memory budget true.
    pub response_bytes: usize,

    /// And the most tool-result text, for the same reason.
    pub tool_output_bytes: usize,

    /// The most one turn may produce before it is stopped, in tokens.
    ///
    /// `None` is unbounded, which is what somebody who has never asked for a
    /// bound has. It is the widest a value here can be, so a descendant asking
    /// for `None` under a parent that set a figure gets the figure.
    pub spend: Option<u64>,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            response_bytes: 16 * 1024 * 1024,
            tool_output_bytes: 4 * 1024 * 1024,
            spend: None,
        }
    }
}

/// What to do about a response that failed before it said a word.
///
/// Asked for again rather than counted as the thing that went wrong. The
/// socket a provider closed while the tools ran is the usual reason, and it is
/// safe to ask again for exactly the reason it is worth doing: nothing
/// arrived, so nothing has been drawn that a second answer could contradict.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// How many more times one response may be asked for after it failed.
    ///
    /// Small on purpose. What this recovers is the moment rather than the
    /// request — a connection the provider closed while the tools ran, a
    /// service busy for a second — and a failure that outlives two goes is one
    /// the user is better off being told about than waited through.
    pub attempts: u8,

    /// How long to wait before the first of them, doubling for the next.
    ///
    /// Short, because the failure this recovers is usually a socket that was
    /// already gone rather than a service asking to be left alone — and
    /// because a user watching a row that says `retrying` is watching this
    /// number.
    pub first_pause: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 2,
            first_pause: Duration::from_millis(250),
        }
    }
}

/// What one run may spend, and what it does when the window fills.
///
/// Three nested answers rather than one flat list of knobs, because they are
/// answered by different people: the bounds are this program's, the compaction
/// policy is the user's documents, and the retry policy is about one provider.
/// A fourth family later is a fourth field, not eight more loose ones.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunPolicy {
    /// Everything this run may spend.
    pub bounds: Bounds,
    /// What it does when the window fills.
    pub compaction: Compaction,
    /// What it does when a response fails before it starts.
    pub retry: Retry,
}

impl RunPolicy {
    /// What a run asking for `wanted` under this policy actually gets.
    ///
    /// The holder is on the left and what is being asked for is on the right.
    /// A call written the other way round compiles and quietly inverts the
    /// rule — `wanted.narrowed(held)` reads the descendant as the ceiling —
    /// so both callers are in `context.rs`, beside the two entries that exist
    /// precisely so nobody else writes the comparison.
    ///
    /// Nine of the ten fields are the tighter of the two, so a descendant
    /// asking for more than its parent holds is given the parent's figure
    /// rather than refused: widening is not an error a caller has to handle,
    /// it is a request that has no effect. Tighter does not point the same way
    /// for all of them, so each direction is written down here rather than
    /// left to be inferred from the `min` beside it. The tenth,
    /// [`Compaction::ask_on_resume`], is not a comparison at all.
    ///
    /// Six point the obvious way, where less is less: the two byte ceilings,
    /// the spend, the retry count, and the two token figures inside the
    /// compaction answer.
    ///
    /// [`Retry::first_pause`] narrows by the same `min` for a different
    /// reason. The resource it bounds is the user's patience, not the
    /// provider's capacity: a descendant that shortened the wait is asking to
    /// be told sooner, and the min gives it that. The count beside it bounds how hard a failing provider is
    /// pressed, and it narrows the usual way — so a descendant can be quicker
    /// to give up but never more persistent, which is the pair that matters.
    ///
    /// [`Compaction::automatic`] is authority rather than a quantity, and the
    /// tighter answer is the one that does less: a run may decline to compact,
    /// never take it up. A session told to fail on a full window has promised
    /// its user that the transcript is left alone, and a run that could switch
    /// compaction back on would be replacing the thing that was promised.
    ///
    /// [`Compaction::reserve`] is tighter when it is *larger*. It is room held
    /// back rather than room granted, so the bigger figure is the one that
    /// fills sooner and leaves less of the window to be spent. It is also the
    /// one figure whose absent case is not symmetric: a holder that named no
    /// reserve keeps its silence, because what absence stands for is derived
    /// from ceilings this crate cannot see and so cannot be compared against.
    /// [`roomier`] carries the reasoning.
    ///
    /// [`Compaction::ask_on_resume`] is the holder's whichever way the two
    /// differ — narrower or wider, the descendant's is dropped — because it is
    /// the one figure here that no run ever reads: it is taken off the session
    /// between turns, which is the only place there is anybody left to ask.
    ///
    /// Written out field by field rather than by carrying a struct across
    /// whole, so a figure added to [`Bounds`], [`Retry`] or [`Compaction`]
    /// later fails to compile here until somebody says which of the two it is.
    ///
    /// In-crate, because the two places a policy is narrowed are both here:
    /// [`RunContext::child`] and [`RunContext::held_to`]. Handing the rule out
    /// as public surface would let a caller apply it somewhere neither of
    /// those covers, and the direction of the arguments is not something the
    /// signature says.
    ///
    /// [`RunContext::child`]: crate::RunContext::child
    /// [`RunContext::held_to`]: crate::RunContext::held_to
    #[must_use]
    pub(crate) fn narrowed(&self, wanted: Self) -> Self {
        Self {
            bounds: Bounds {
                response_bytes: self.bounds.response_bytes.min(wanted.bounds.response_bytes),
                tool_output_bytes: self
                    .bounds
                    .tool_output_bytes
                    .min(wanted.bounds.tool_output_bytes),
                spend: tighter(self.bounds.spend, wanted.bounds.spend),
            },
            compaction: Compaction {
                automatic: self.compaction.automatic && wanted.compaction.automatic,
                reserve: roomier(self.compaction.reserve, wanted.compaction.reserve),
                keep_tokens: self
                    .compaction
                    .keep_tokens
                    .min(wanted.compaction.keep_tokens),
                recap_tokens: self
                    .compaction
                    .recap_tokens
                    .min(wanted.compaction.recap_tokens),
                ask_on_resume: self.compaction.ask_on_resume,
            },
            retry: Retry {
                attempts: self.retry.attempts.min(wanted.retry.attempts),
                first_pause: self.retry.first_pause.min(wanted.retry.first_pause),
            },
        }
    }
}

/// The lower of two bounds, where absent means unbounded.
///
/// Not [`Option::min`], which would read `None` as the smaller answer and let
/// a descendant lift its parent's bound by declining to set one.
fn tighter(held: Option<u64>, wanted: Option<u64>) -> Option<u64> {
    match (held, wanted) {
        (Some(held), Some(wanted)) => Some(held.min(wanted)),
        (bound, None) | (None, bound) => bound,
    }
}

/// The larger of two reserves, where absent means nobody named one.
///
/// Not the mirror of [`tighter`], though the field it settles is the one here
/// that is room held back rather than room granted. Where both sides named a
/// figure the bigger one is the narrower answer, and that half does mirror.
/// The absent side does not.
///
/// Absent is not zero, and it is not unbounded either: it is the wiring
/// declining to name a figure, and the reserve is then derived from the
/// model's own ceilings. That derivation reads a max-tokens figure and a
/// window this crate is deliberately kept away from, so nothing here can tell
/// whether a named figure is larger than the one absence stands for. A holder
/// that named nothing therefore keeps its silence rather than take a
/// descendant's figure on trust — a guess in that direction spends window the
/// session was holding back. It costs the one case where the descendant's
/// figure really was the larger; refusing a real tightening is the safe half
/// of the trade, and granting a widening is not.
///
/// The silent case is the common one, not an edge. [`Compaction::default`]
/// names no reserve and the wiring passes on whatever the document says, which
/// is nothing until somebody writes the key — so on a stock installation the
/// first arm is the arm every descendant meets, and a run that asks to hold
/// more of the window back is answered with the session's silence. That is the
/// rule working rather than failing, and it is worth knowing before the phase
/// that first starts descendants reads a dropped request as a bug.
fn roomier(held: Option<u64>, wanted: Option<u64>) -> Option<u64> {
    match (held, wanted) {
        (None, _) => None,
        (Some(held), None) => Some(held),
        (Some(held), Some(wanted)) => Some(held.max(wanted)),
    }
}

/// What a session does when the window fills.
///
/// Handed over whole by the wiring, so this crate never learns that any of it
/// has a spelling in a file. The default is a session that compacts when it
/// has to and is bounded by nothing else, which is the answer for somebody who
/// has never heard of any of this.
#[derive(Debug, Clone, Copy)]
pub struct Compaction {
    /// Whether a full window is answered by making room rather than by failing.
    pub automatic: bool,
    /// Room to leave for the next exchange, in tokens, where somebody said.
    pub reserve: Option<u64>,
    /// How many tokens of recent turns are kept word for word after the recap.
    ///
    /// Bounded in tokens rather than counted in turns because a turn can be
    /// enormous: the kept tail is what has to fit beside the recap, and only a
    /// figure in the window's own unit can promise that.
    pub keep_tokens: u64,
    /// Maximum output tokens given to the structured recap request.
    pub recap_tokens: u32,
    /// How large a session must be before picking it up asks about it.
    ///
    /// Carried here rather than read where it is used, so the wiring resolves
    /// every compaction answer in one place. This loop never asks anybody
    /// anything about it — a turn already running has nobody to ask.
    pub ask_on_resume: Option<u64>,
}

impl Default for Compaction {
    fn default() -> Self {
        Self {
            automatic: true,
            reserve: None,
            // Enough of the recent turns to hold what the model is doing and
            // the exchange that led to it, which is what "carry on from here"
            // needs. In tokens, so a turn that is mostly tool output cannot
            // blow straight past it.
            keep_tokens: 20_000,
            // A ceiling rather than a target. Ordinary checkpoints finish far
            // below it; long technical sessions are not forced through 4k.
            recap_tokens: 10_240,
            ask_on_resume: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_nobody_bounded_is_held_to_the_figures_this_program_ships() {
        let policy = RunPolicy::default();

        assert_eq!(policy.bounds.response_bytes, 16 * 1024 * 1024);
        assert_eq!(policy.bounds.tool_output_bytes, 4 * 1024 * 1024);
        assert_eq!(
            policy.bounds.spend, None,
            "a spend nobody asked for is not bounded"
        );
        assert_eq!(policy.retry.attempts, 2);
        assert_eq!(policy.retry.first_pause, Duration::from_millis(250));
        assert!(policy.compaction.automatic);
        assert_eq!(policy.compaction.keep_tokens, 20_000);
        assert_eq!(policy.compaction.recap_tokens, 10_240);
        assert_eq!(
            policy.compaction.reserve, None,
            "a reserve nobody named is derived rather than fixed here"
        );
        assert_eq!(
            policy.compaction.ask_on_resume, None,
            "a question nobody answered is asked, not assumed either way"
        );
    }

    #[test]
    fn a_descendant_asking_for_less_gets_what_it_asked_for() {
        // Seven of the nine figures that resolve by comparison, asked for
        // narrower. The two left at the parent's — `automatic` and `reserve`,
        // whose directions are the unobvious ones — have their own tests
        // below. The widening direction has its own test too; this is the half
        // that says the rule is a comparison at all rather than the holder's
        // answer twice.
        let parent = RunPolicy::default();
        let asked = RunPolicy {
            bounds: Bounds {
                response_bytes: 1024,
                tool_output_bytes: 512,
                spend: Some(50),
            },
            compaction: Compaction {
                keep_tokens: 512,
                recap_tokens: 256,
                ..parent.compaction
            },
            retry: Retry {
                attempts: 0,
                first_pause: Duration::from_millis(1),
            },
        };

        let held = parent.narrowed(asked);

        assert_eq!(held.bounds.response_bytes, 1024);
        assert_eq!(held.bounds.tool_output_bytes, 512);
        assert_eq!(held.bounds.spend, Some(50));
        assert_eq!(held.retry.attempts, 0);
        assert_eq!(
            held.compaction.keep_tokens, 512,
            "a run asking to carry less forward was given its session's figure"
        );
        assert_eq!(
            held.compaction.recap_tokens, 256,
            "a run asking for a shorter recap was given its session's figure"
        );
        assert_eq!(
            held.retry.first_pause,
            Duration::from_millis(1),
            "a run asking to be told sooner was made to wait the session's pause"
        );
    }

    #[test]
    fn a_descendant_asking_for_more_gets_what_its_parent_holds() {
        let parent = RunPolicy {
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
        };
        let asked = RunPolicy {
            bounds: Bounds {
                response_bytes: usize::MAX,
                tool_output_bytes: usize::MAX,
                spend: Some(u64::MAX),
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
        };

        let held = parent.narrowed(asked);

        assert_eq!(held.bounds.response_bytes, 1024);
        assert_eq!(held.bounds.tool_output_bytes, 512);
        assert_eq!(held.bounds.spend, Some(50));
        assert_eq!(held.retry.attempts, 1);
        assert_eq!(
            held.retry.first_pause,
            Duration::from_millis(250),
            "a descendant lengthened the wait its parent set"
        );
        assert_eq!(
            held.compaction.keep_tokens, 1_000,
            "a descendant raised the retained-token bound its parent set"
        );
        assert_eq!(
            held.compaction.recap_tokens, 256,
            "a descendant raised an output-token ceiling its parent set"
        );
    }

    /// A policy whose compaction answer is `said` and whose everything else is
    /// the shipped default, for the three tests that differ in one field.
    fn compacting(said: Compaction) -> RunPolicy {
        RunPolicy {
            compaction: said,
            ..RunPolicy::default()
        }
    }

    #[test]
    fn a_descendant_cannot_switch_on_the_compaction_its_holder_switched_off() {
        // The direction the window answer was never asked about. The test that
        // stood here had the descendant asking for *less* compaction than its
        // parent allowed, which is the one arrangement where taking the
        // descendant's answer and taking the narrower one agree — so it went
        // green whether the rule held or not.
        //
        // It matters because a session set to fail on a full window has told
        // its user the transcript is left alone. Compaction replaces it.
        let holder = compacting(Compaction {
            automatic: false,
            ..Compaction::default()
        });
        let asked = compacting(Compaction {
            automatic: true,
            ..Compaction::default()
        });

        assert!(
            !holder.narrowed(asked).compaction.automatic,
            "a run switched on the compaction its session switched off"
        );
    }

    #[test]
    fn a_descendant_may_still_decline_to_compact() {
        // The half that has to keep working: narrowing is a request that has
        // no effect only when it widens, and this one does not.
        let holder = compacting(Compaction::default());
        let asked = compacting(Compaction {
            automatic: false,
            ..Compaction::default()
        });

        assert!(holder.compaction.automatic, "the shipped answer moved");
        assert!(
            !holder.narrowed(asked).compaction.automatic,
            "a run was made to compact when it asked not to"
        );
    }

    #[test]
    fn a_descendant_cannot_hold_back_less_room_than_its_holder() {
        // Larger is narrower here: the reserve is room kept free, so the
        // bigger figure is the one that fills sooner and spends less window.
        let holder = compacting(Compaction {
            reserve: Some(9_000),
            ..Compaction::default()
        });
        let asked = compacting(Compaction {
            reserve: Some(1_000),
            ..Compaction::default()
        });

        assert_eq!(
            holder.narrowed(asked).compaction.reserve,
            Some(9_000),
            "a run kept less of the window free than its session set aside"
        );
        assert_eq!(
            asked.narrowed(holder).compaction.reserve,
            Some(9_000),
            "a run asking to keep more free was not given it"
        );
    }

    #[test]
    fn a_holder_that_named_no_reserve_is_not_given_one_by_its_descendant() {
        // Absent is not zero and it is not unbounded: the wiring leaves it
        // unset and the reserve is derived from the model's own ceilings,
        // which are figures this crate never sees. So there is nothing here
        // to compare a named figure against, and the holder's silence stands.
        let holder = compacting(Compaction {
            reserve: None,
            ..Compaction::default()
        });

        for asked in [Some(0), Some(1_000), Some(u64::MAX)] {
            let asked = compacting(Compaction {
                reserve: asked,
                ..Compaction::default()
            });

            assert_eq!(
                holder.narrowed(asked).compaction.reserve,
                None,
                "a run replaced a derived reserve with a figure of its own"
            );
        }
    }

    #[test]
    fn a_descendant_that_named_no_reserve_leaves_its_holders_standing() {
        let holder = compacting(Compaction {
            reserve: Some(9_000),
            ..Compaction::default()
        });
        let asked = compacting(Compaction {
            reserve: None,
            ..Compaction::default()
        });

        assert_eq!(
            holder.narrowed(asked).compaction.reserve,
            Some(9_000),
            "a run that named nothing dropped the room its session held back"
        );
    }

    #[test]
    fn how_large_a_session_must_be_before_it_asks_is_the_holders_answer() {
        // No run reads this one — it is taken off the session between turns,
        // where there is still somebody to ask — so the holder's answer is the
        // only one that can be right.
        let holder = compacting(Compaction {
            ask_on_resume: Some(10),
            ..Compaction::default()
        });
        let asked = compacting(Compaction {
            ask_on_resume: Some(999),
            ..Compaction::default()
        });

        assert_eq!(
            holder.narrowed(asked).compaction.ask_on_resume,
            Some(10),
            "a run answered a question that is only ever put to the session"
        );
        // Both orders, because `Some(10)` is also the tighter of the two: a
        // rule that compared them would pass the assertion above and fail
        // this one. It is the only figure here that is not a comparison at
        // all, and one of two that can see an argument swap — `reserve` is
        // the other, asymmetric in its absent case rather than in all of
        // them, and it is the one a run reads every turn.
        assert_eq!(
            asked.narrowed(holder).compaction.ask_on_resume,
            Some(999),
            "the tighter figure was taken where the holder's was the answer"
        );
        assert_eq!(
            holder
                .narrowed(compacting(Compaction::default()))
                .compaction
                .ask_on_resume,
            Some(10),
            "a run that named nothing dropped the session's answer"
        );
    }

    #[test]
    fn a_descendant_cannot_lift_a_spend_bound_by_asking_for_none() {
        let parent = RunPolicy {
            bounds: Bounds {
                spend: Some(50),
                ..Bounds::default()
            },
            ..RunPolicy::default()
        };

        let held = parent.narrowed(RunPolicy::default());

        assert_eq!(
            held.bounds.spend,
            Some(50),
            "unbounded was read as a narrower answer than a figure"
        );
    }

    #[test]
    fn an_unbounded_parent_takes_the_figure_its_descendant_asked_for() {
        let asked = RunPolicy {
            bounds: Bounds {
                spend: Some(50),
                ..Bounds::default()
            },
            ..RunPolicy::default()
        };

        let held = RunPolicy::default().narrowed(asked);

        assert_eq!(held.bounds.spend, Some(50));
    }
}
