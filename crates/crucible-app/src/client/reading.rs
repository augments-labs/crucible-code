//! The application's values as a client reads them.
//!
//! Every translation is a match written out here. Nothing below this crate is
//! serialized as it stands: a runner event holds tool calls, outputs and
//! errors whose shapes are the engine's to change, so a client is told the few
//! bounded things about each that the contract has a word for, and an event
//! with no word — a sandbox fact, a cache fact, a receipt — is not sent.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_client_api as api;
use crucible_client_api::{
    Capabilities, Capability, ErrorCode, Model, Name, Percent, Problem, Progress, Snapshot, Stop,
    Text,
};
use crucible_models::{Effort, Served, Speed};
use crucible_runner::{Breakdown, Category, Event, PlanLimitStop, SessionCost, Totals, TurnError};
use crucible_tools::Mode;
use crucible_types::{CostAmount, PlanWindows, StopReason, Utc, Window};

use crate::Conversation;
use crate::switching::Retained;

/// Where `conversation` stands.
///
/// Read off the conversation itself, so it is the same whatever progress a
/// client did or did not see go by. It is pending on nothing: a turn that is
/// waiting on an answer has the conversation, so whoever can read one here is
/// reading a conversation no turn holds, and no caller's word is taken for
/// what it waits on. Called by a consumer with no terminal — today the one the
/// headless tests drive; the terminal draws its status from the conversation
/// directly and does not ask for one.
#[must_use]
pub fn snapshot(conversation: &Conversation) -> Snapshot {
    let runner = conversation.runner();
    let transcript = runner.transcript();

    Snapshot {
        session: conversation.session().id().cloned(),
        provider: conversation.serving().and_then(|name| Name::new(name).ok()),
        model: Model::new(runner.model()),
        effort: runner.effort().map(rung),
        speed: pace(runner.speed()),
        served: match runner.served() {
            Served::Fast => Some(api::Pace::Fast),
            Served::Standard => Some(api::Pace::Standard),
            Served::Unsaid => None,
        },
        mode: mode_out(runner.mode()),
        messages: count(transcript.len()),
        turns: count(transcript.turns()),
        carrying: runner.carrying(),
        left: runner.left().and_then(Percent::new),
        pending: None,
    }
}

/// What a client that said it has `capabilities` is told of one thing a running
/// turn reported, where the contract has a word for it.
///
/// Nothing, for a client that did not ask for [`Capability::Progress`]: this is
/// the one place progress is handed to a client, so it is where that is held.
/// Called by a consumer with no terminal — today the one the headless tests
/// drive; the terminal draws a turn from the runner's events as they stand.
#[must_use]
pub fn progress(capabilities: Capabilities, event: &Event) -> Option<Progress> {
    if !capabilities.has(Capability::Progress) {
        return None;
    }

    Some(match event {
        Event::TurnStarted { turn } => Progress::Started {
            turn: u64::from(turn.get()),
        },
        Event::Delta { text } => Progress::Delta {
            text: Text::cut(text),
        },
        Event::ToolRequested { call, summary, .. } => Progress::ToolRequested {
            call: Text::cut(call.id.as_str()),
            tool: Text::cut(&call.name),
            summary: Text::cut(summary.as_str()),
        },
        Event::ToolFinished { call, output, .. } => Progress::ToolFinished {
            call: Text::cut(call.as_str()),
            failed: output.is_failed(),
        },
        // Sent once more before anything was answered, as a retry is; the
        // speed it leaves in force is the snapshot's to say.
        Event::Retrying | Event::FastRefused { resent: true, .. } => Progress::Retrying,
        Event::Compacting { part, .. } => Progress::Compacting {
            part: u64::from(*part),
        },
        Event::Compacted { compacted } => Progress::Compacted {
            replaced: count(compacted.replaced),
        },
        Event::Spent { spend } => Progress::Spent {
            tokens: spend.tokens(),
        },
        // What a client with no terminal reads as `/context` while a turn
        // runs; the terminal takes the same figures from the runner's event.
        // The model is the snapshot's to say.
        Event::Carried { breakdown } => Progress::Context(counted(breakdown)),
        // What a client with no terminal reads as `/usage` while a turn runs,
        // beside the context above: the session's totals as each response or
        // edit moved them, and the plan windows a response carried.
        Event::Used { totals } => Progress::Used(used(totals)),
        Event::PlanLimits { windows } => Progress::Limits(limits(windows)),
        Event::TurnFinished { turn, stop } => Progress::Finished {
            turn: u64::from(turn.get()),
            stop: self::stop(*stop),
        },
        Event::Failed { error } => Progress::Failed(failed(error)),
        // A refusal whose second send a stop ended is no retry.
        Event::FastRefused { resent: false, .. }
        | Event::PromptCache { .. }
        | Event::Sandbox { .. }
        | Event::Wrote { .. }
        | Event::Aged { .. }
        | Event::Unread { .. }
        | Event::Steered { .. } => return None,
    })
}

/// What a client is told of a turn that ended on `error`.
///
/// A used-up plan has a code of its own, so that a client can tell a time to
/// come back from a failure to report, and a sentence written here from the
/// window's name and the reset in UTC: nothing the vendor wrote reaches it.
/// Every other ending reads as what the terminal would have shown.
pub(super) fn failed(error: &TurnError) -> Problem {
    match error {
        TurnError::PlanLimit {
            window,
            resets_at,
            stopped,
        } => Problem {
            code: ErrorCode::PlanLimit,
            message: Text::cut(&used_up(*window, *resets_at, *stopped)),
        },
        other => Problem::failed(other),
    }
}

/// The sentence a client is told a used-up plan in.
fn used_up(
    window: Option<Window>,
    resets_at: Option<SystemTime>,
    stopped: PlanLimitStop,
) -> String {
    let window = window.map_or_else(String::new, |window| format!(" on the {}", window.named()));
    let resets = resets_at.map_or_else(
        || "the reset was not reported".to_owned(),
        |at| format!("it resets at {}", Utc::new(at)),
    );
    let stopped = match stopped {
        PlanLimitStop::BeforeSending => "the turn stopped before sending",
        PlanLimitStop::Refused => "the vendor refused the request",
    };
    format!("the plan's usage limit is reached{window}; {resets}; {stopped}")
}

/// How the window of the next request to `model` is spent, as a client reads
/// it.
///
/// Every category the runner counts is placed by name, so one it adds is a
/// category this match, and so the contract, has to be told about. The
/// reading of what is left is the one the prompt line and [`snapshot`] show.
/// Called between turns by [`perform`](super::perform), and by the terminal
/// mid-turn with the breakdown the turn last reported; a client with no
/// terminal is streamed the same figures by [`progress`].
#[must_use]
pub fn context(model: &str, breakdown: &Breakdown) -> api::Context {
    api::Context {
        model: Model::new(model),
        ..counted(breakdown)
    }
}

/// What a session has used, and the plan windows its vendor last reported,
/// as a client reads them.
///
/// Every figure is one the runner already holds: reading this sends nothing
/// anywhere, so no request is made to learn a limit. The wall time is read as
/// this is called. Called between turns by [`perform`](super::perform), and
/// by the terminal mid-turn with the figures that turn last reported; a client
/// with no terminal is streamed the same figures by [`progress`].
#[must_use]
pub fn usage(
    model: &str,
    breakdown: &Breakdown,
    totals: &Totals,
    limits: Option<&PlanWindows>,
) -> api::Usage {
    api::Usage {
        used: used(totals),
        context: context(model, breakdown),
        limits: limits.map_or_else(api::Limits::default, self::limits),
    }
}

/// What the session's requests and edits have added up to, as a client reads
/// it. The wall time is read as this is called: as the response ended or the
/// edit counted, for one streamed mid-turn.
fn used(totals: &Totals) -> api::Used {
    let millis = |of: Duration| u64::try_from(of.as_millis()).unwrap_or(u64::MAX);
    api::Used {
        cost: cost(totals.cost()),
        api_ms: millis(totals.api()),
        wall_ms: millis(totals.started().elapsed()),
        added: totals.added(),
        removed: totals.removed(),
        input: totals.input(),
        output: totals.output(),
        cache_read: totals.cache_read(),
        cache_write: totals.cache_write(),
    }
}

/// A session's cost as it crosses, in millionths of its currency.
pub(super) fn cost(cost: SessionCost) -> api::Cost {
    match cost {
        SessionCost::Unspent => api::Cost::Unspent,
        SessionCost::Priced(amount) => stated(amount)
            .map_or(api::Cost::NotPriced, |(currency, micros)| {
                api::Cost::Priced { currency, micros }
            }),
        SessionCost::AtLeast(amount) => stated(amount)
            .map_or(api::Cost::NotPriced, |(currency, micros)| {
                api::Cost::AtLeast { currency, micros }
            }),
        SessionCost::NotPriced => api::Cost::NotPriced,
    }
}

/// An amount as its currency's code and millionths of it, where it can be
/// stated.
fn stated(amount: CostAmount) -> Option<(Name, u64)> {
    // A femtocurrency is a billionth of a micro. A sum too large for the
    // contract, or under a code that is not a name, is one nobody can read,
    // so it is not stated: a capped figure would read as what was spent.
    let micros = u64::try_from(amount.femtocurrency() / 1_000_000_000).ok()?;
    Some((Name::new(amount.currency().as_str()).ok()?, micros))
}

/// Every window a vendor reported, each placed by name.
pub(super) fn limits(windows: &PlanWindows) -> api::Limits {
    let mut limits = api::Limits::default();
    for (window, reading) in windows.reported() {
        let limit = Percent::new(reading.percent()).map(|used| api::Limit {
            used,
            resets_at: reading
                .resets_at()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_secs()),
        });
        match window {
            Window::FiveHour => limits.five_hour = limit,
            Window::Weekly => limits.weekly = limit,
            Window::Monthly => limits.monthly = limit,
        }
    }
    limits
}

/// The parts of `breakdown`, with no model named.
fn counted(breakdown: &Breakdown) -> api::Context {
    let mut context = api::Context {
        model: None,
        window: breakdown.window().map(u64::from),
        left: breakdown.left().and_then(Percent::new),
        system: 0,
        instructions: 0,
        tools: 0,
        mcp: 0,
        messages: 0,
        reserve: 0,
        free: 0,
    };
    for category in Category::EVERY {
        let tokens = breakdown.tokens(category);
        match category {
            Category::SystemPrompt => context.system = tokens,
            Category::ProjectInstructions => context.instructions = tokens,
            Category::ToolSchemas => context.tools = tokens,
            Category::McpToolSchemas => context.mcp = tokens,
            Category::Messages => context.messages = tokens,
            Category::Reserve => context.reserve = tokens,
            Category::Free => context.free = tokens,
        }
    }
    context
}

/// A count as it crosses: a `usize` on every machine this builds for fits.
pub(super) fn count(of: usize) -> u64 {
    u64::try_from(of).unwrap_or(u64::MAX)
}

pub(super) fn retained(retained: Retained) -> api::Retained {
    api::Retained {
        ambiguous: count(retained.ambiguous),
        orphaned: count(retained.orphaned),
    }
}

pub(super) const fn stop(stop: StopReason) -> Stop {
    match stop {
        StopReason::Yielded => Stop::Yielded,
        StopReason::WantsTools => Stop::WantsTools,
        StopReason::OutOfTokens => Stop::OutOfTokens,
        StopReason::WindowExceeded => Stop::WindowExceeded,
        StopReason::Filtered => Stop::Filtered,
        StopReason::Paused => Stop::Paused,
        StopReason::Cancelled => Stop::Cancelled,
        StopReason::Unknown => Stop::Unknown,
    }
}

/// `mode`, as the contract spells it: what a front end holding the engine's
/// own value puts in a request.
#[must_use]
pub const fn mode_out(mode: Mode) -> api::Mode {
    match mode {
        Mode::Ask => api::Mode::Ask,
        Mode::AllowEdits => api::Mode::AllowEdits,
        Mode::FullAccess => api::Mode::FullAccess,
    }
}

pub(super) const fn mode_in(mode: api::Mode) -> Mode {
    match mode {
        api::Mode::Ask => Mode::Ask,
        api::Mode::AllowEdits => Mode::AllowEdits,
        api::Mode::FullAccess => Mode::FullAccess,
    }
}

pub(super) const fn effort(rung: api::Rung) -> Effort {
    match rung {
        api::Rung::Low => Effort::Low,
        api::Rung::Medium => Effort::Medium,
        api::Rung::High => Effort::High,
        api::Rung::Xhigh => Effort::Xhigh,
        api::Rung::Max => Effort::Max,
    }
}

pub(super) const fn speed(pace: api::Pace) -> Speed {
    match pace {
        api::Pace::Standard => Speed::Standard,
        api::Pace::Fast => Speed::Fast,
    }
}

/// `speed`, as the contract spells it.
#[must_use]
pub const fn pace(speed: Speed) -> api::Pace {
    match speed {
        Speed::Standard => api::Pace::Standard,
        Speed::Fast => api::Pace::Fast,
    }
}

/// `effort`, as the contract spells it.
#[must_use]
pub const fn rung(effort: Effort) -> api::Rung {
    match effort {
        Effort::Low => api::Rung::Low,
        Effort::Medium => api::Rung::Medium,
        Effort::High => api::Rung::High,
        Effort::Xhigh => api::Rung::Xhigh,
        Effort::Max => api::Rung::Max,
    }
}

#[cfg(test)]
mod tests;
