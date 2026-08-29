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
//! The rule is about budgets, which is everything here except the three window
//! answers [`RunPolicy::narrowed`] names. Permission is not one of them and is
//! not modelled here at all: what a run may do lives behind [`Ask`] and the
//! runner's own permission state, so nothing in this file either grants or
//! withholds authority.
//!
//! [`Ask`]: crucible_core::Ask
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    /// Every budget is the tighter of the two, so a descendant asking for more
    /// than its parent holds is given the parent's figure rather than refused:
    /// widening is not an error a caller has to handle, it is a request that
    /// has no effect. Six figures are budgets that way — the two byte
    /// ceilings, the spend, the retry count and pause, and the two token
    /// figures inside the compaction answer.
    ///
    /// Three are not, and are the descendant's whichever way they differ:
    /// whether to compact at all, how much room to leave, and how large a
    /// session must be before picking it up asks about it. None of those is a
    /// quantity a parent can be said to hold, so there is no tighter answer to
    /// take — a descendant that declines to compact is not spending anything
    /// its parent did not allow.
    ///
    /// Written out field by field rather than by carrying a struct across
    /// whole, so a figure added to [`Bounds`], [`Retry`] or [`Compaction`]
    /// later fails to compile here until somebody says which of the two it is.
    #[must_use]
    pub fn narrowed(&self, wanted: Self) -> Self {
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
                automatic: wanted.compaction.automatic,
                reserve: wanted.compaction.reserve,
                keep_tokens: self
                    .compaction
                    .keep_tokens
                    .min(wanted.compaction.keep_tokens),
                recap_tokens: self
                    .compaction
                    .recap_tokens
                    .min(wanted.compaction.recap_tokens),
                ask_on_resume: wanted.compaction.ask_on_resume,
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

/// What a session does when the window fills.
///
/// Handed over whole by the wiring, so this crate never learns that any of it
/// has a spelling in a file. The default is a session that compacts when it
/// has to and is bounded by nothing else, which is the answer for somebody who
/// has never heard of any of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    }

    #[test]
    fn a_descendant_asking_for_less_gets_what_it_asked_for() {
        let parent = RunPolicy::default();
        let asked = RunPolicy {
            bounds: Bounds {
                response_bytes: 1024,
                tool_output_bytes: 512,
                spend: Some(50),
            },
            retry: Retry {
                attempts: 0,
                ..Retry::default()
            },
            ..parent
        };

        let held = parent.narrowed(asked);

        assert_eq!(held.bounds.response_bytes, 1024);
        assert_eq!(held.bounds.tool_output_bytes, 512);
        assert_eq!(held.bounds.spend, Some(50));
        assert_eq!(held.retry.attempts, 0);
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

    #[test]
    fn a_descendant_keeps_the_window_answer_it_asked_for() {
        // The three that are genuinely not budgets: whether to compact at all,
        // how much room to leave, and how large a session has to be before
        // picking it up asks. There is no tighter or looser answer to take, so
        // these are the descendant's whichever way they differ.
        let parent = RunPolicy {
            compaction: Compaction {
                automatic: true,
                reserve: Some(1_000),
                ask_on_resume: Some(10),
                ..Compaction::default()
            },
            ..RunPolicy::default()
        };
        let asked = RunPolicy {
            compaction: Compaction {
                automatic: false,
                reserve: Some(9_000),
                ask_on_resume: None,
                ..Compaction::default()
            },
            ..RunPolicy::default()
        };

        let held = parent.narrowed(asked);

        assert!(!held.compaction.automatic);
        assert_eq!(held.compaction.reserve, Some(9_000));
        assert_eq!(held.compaction.ask_on_resume, None);
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
