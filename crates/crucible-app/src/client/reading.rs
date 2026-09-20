//! The application's values as a client reads them.
//!
//! Every translation is a match written out here. Nothing below this crate is
//! serialized as it stands: a runner event holds tool calls, outputs and
//! errors whose shapes are the engine's to change, so a client is told the few
//! bounded things about each that the contract has a word for, and an event
//! with no word — a sandbox fact, a cache fact, a receipt — is not sent.

use crucible_client_api as api;
use crucible_client_api::{
    Capabilities, Capability, Model, Name, Percent, Problem, Progress, Snapshot, Stop, Text,
};
use crucible_models::Effort;
use crucible_runner::Event;
use crucible_tools::Mode;
use crucible_types::StopReason;

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
        Event::Retrying => Progress::Retrying,
        Event::Compacting { part, .. } => Progress::Compacting {
            part: u64::from(*part),
        },
        Event::Compacted { compacted } => Progress::Compacted {
            replaced: count(compacted.replaced),
        },
        Event::Spent { spend } => Progress::Spent {
            tokens: spend.tokens(),
        },
        Event::TurnFinished { turn, stop } => Progress::Finished {
            turn: u64::from(turn.get()),
            stop: self::stop(*stop),
        },
        Event::Failed { error } => Progress::Failed(Problem::failed(error)),
        Event::PromptCache { .. }
        | Event::Sandbox { .. }
        | Event::Wrote { .. }
        | Event::Carried { .. }
        | Event::Aged { .. }
        | Event::Unread { .. }
        | Event::Steered { .. } => return None,
    })
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
