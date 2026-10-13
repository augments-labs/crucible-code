//! What an agent reports of its session while a turn runs.
//!
//! Every kind of `session/update` the protocol defines is a [`Kind`], and
//! [`Updates::heard`] matches each one with an arm of its own, so a kind added
//! here makes the reader decide what it means. An update of a kind not named
//! is dropped and counted: it reports, and asks nothing.
//!
//! Usage arrives as the session's running cost. A turn reports the difference
//! between what the session had cost when it began and what it costs at its
//! end, and only where both are known: a session opened fresh has cost
//! nothing, but one resumed has cost an amount nobody said, so its first turn
//! reports no figure and the turns after it do.

use crucible_execution::{Cost, Currency, Event, Observation, Scope, Text, Usage};
use crucible_types::TurnId;
use serde_json::Value;

use crate::Act;

/// The largest amount a cost may report, in its currency's units: far beyond
/// any session, and small enough that its millionths fit in a `u64`.
const AMOUNT_CEILING: f64 = 1e12;

/// One kind of session update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    /// The user's prompt, said back.
    UserMessageChunk,
    /// More of the answer.
    AgentMessageChunk,
    /// More of the agent's reasoning.
    AgentThoughtChunk,
    /// A tool call began.
    ToolCall,
    /// A tool call moved on.
    ToolCallUpdate,
    /// The agent's plan.
    Plan,
    /// A change to the agent's plan.
    PlanUpdate,
    /// The agent's plan was dropped.
    PlanRemoved,
    /// The commands the agent offers.
    AvailableCommandsUpdate,
    /// The mode the agent is in.
    CurrentModeUpdate,
    /// A setting of the agent's.
    ConfigOptionUpdate,
    /// The session's title or other facts about it.
    SessionInfoUpdate,
    /// What the session has used.
    UsageUpdate,
    /// The agent is compacting what it holds.
    CompactionUpdate,
    /// More of the summary compaction left.
    CompactionSummaryChunk,
}

impl Kind {
    /// Every kind.
    pub(crate) const ALL: [Self; 15] = [
        Self::UserMessageChunk,
        Self::AgentMessageChunk,
        Self::AgentThoughtChunk,
        Self::ToolCall,
        Self::ToolCallUpdate,
        Self::Plan,
        Self::PlanUpdate,
        Self::PlanRemoved,
        Self::AvailableCommandsUpdate,
        Self::CurrentModeUpdate,
        Self::ConfigOptionUpdate,
        Self::SessionInfoUpdate,
        Self::UsageUpdate,
        Self::CompactionUpdate,
        Self::CompactionSummaryChunk,
    ];

    /// Its name on the wire.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::UserMessageChunk => "user_message_chunk",
            Self::AgentMessageChunk => "agent_message_chunk",
            Self::AgentThoughtChunk => "agent_thought_chunk",
            Self::ToolCall => "tool_call",
            Self::ToolCallUpdate => "tool_call_update",
            Self::Plan => "plan",
            Self::PlanUpdate => "plan_update",
            Self::PlanRemoved => "plan_removed",
            Self::AvailableCommandsUpdate => "available_commands_update",
            Self::CurrentModeUpdate => "current_mode_update",
            Self::ConfigOptionUpdate => "config_option_update",
            Self::SessionInfoUpdate => "session_info_update",
            Self::UsageUpdate => "usage_update",
            Self::CompactionUpdate => "compaction_update",
            Self::CompactionSummaryChunk => "compaction_summary_chunk",
        }
    }

    /// The kind named `name`, where one is.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// What the session is known to have cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spent {
    /// This much, or nothing yet.
    Known(Option<Cost>),
    /// An amount nobody said.
    Unknown,
}

/// The reader of one session's updates.
#[derive(Debug)]
pub(crate) struct Updates {
    /// What the session had cost when the running turn began.
    before: Spent,
    /// The running cost the running turn last reported.
    latest: Option<Cost>,
    /// How many updates of no kind defined were dropped, which no event a
    /// turn reports carries yet.
    pub(crate) unknown: u32,
}

impl Updates {
    /// The reader of a session opened fresh, which has cost nothing.
    pub(crate) const fn fresh() -> Self {
        Self {
            before: Spent::Known(None),
            latest: None,
            unknown: 0,
        }
    }

    /// The reader of a session resumed, whose cost so far nobody said.
    pub(crate) const fn resumed() -> Self {
        Self {
            before: Spent::Unknown,
            latest: None,
            unknown: 0,
        }
    }

    /// What the update `update` means.
    pub(crate) fn heard(&mut self, update: &Value) -> Vec<Act> {
        let named = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .and_then(Kind::named);
        let Some(kind) = named else {
            self.unknown = self.unknown.saturating_add(1);
            return Vec::new();
        };
        match kind {
            Kind::AgentMessageChunk => text(update.get("content"))
                .map(|said| Act::Say(Event::Delta(Text::new(said))))
                .into_iter()
                .collect(),
            Kind::ToolCall => tool_call(update),
            Kind::ToolCallUpdate => tool_call_update(update),
            Kind::UsageUpdate => {
                if let Some(cost) = update.get("cost").and_then(cost) {
                    self.latest = Some(cost);
                }
                Vec::new()
            }
            // What the user said, said back; the transcript has it already.
            Kind::UserMessageChunk
            // The agent's reasoning and its plans are its own working, and a
            // turn reports what it did.
            | Kind::AgentThoughtChunk
            | Kind::Plan
            | Kind::PlanUpdate
            | Kind::PlanRemoved
            // Its commands, modes, settings and title are what it offers its
            // own front end, and none of them is chosen through this one.
            | Kind::AvailableCommandsUpdate
            | Kind::CurrentModeUpdate
            | Kind::ConfigOptionUpdate
            | Kind::SessionInfoUpdate
            // Compaction changes what it holds, which Crucible cannot see.
            | Kind::CompactionUpdate
            | Kind::CompactionSummaryChunk => Vec::new(),
        }
    }

    /// The usage of the turn `turn`, which has ended, where its cost is known.
    pub(crate) fn settled(&mut self, turn: TurnId) -> Option<Act> {
        let now = self.latest.take()?;
        let spent = match self.before {
            Spent::Known(None) => Some(now),
            Spent::Known(Some(before)) if before.currency() == now.currency() => now
                .millionths()
                .checked_sub(before.millionths())
                .map(|millionths| Cost::new(millionths, now.currency())),
            Spent::Known(Some(_)) | Spent::Unknown => None,
        };
        self.before = Spent::Known(Some(now));
        let usage = Usage::of(turn).cost(spent?);
        Some(Act::Say(Event::Usage(Observation::live(
            usage,
            Scope::Session,
        ))))
    }
}

/// The text of a content block, where it is text.
fn text(content: Option<&Value>) -> Option<&str> {
    let content = content?;
    (content.get("type")?.as_str()? == "text").then_some(())?;
    content.get("text")?.as_str()
}

/// The tool a call names, as its kind or else its title.
fn tool(update: &Value) -> Text {
    let named = update
        .get("kind")
        .or_else(|| update.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    Text::new(named)
}

/// What a tool call that began means.
fn tool_call(update: &Value) -> Vec<Act> {
    let summary = update
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut acts = vec![Act::Say(Event::ToolActivity {
        tool: tool(update),
        summary: Text::new(summary),
    })];
    acts.extend(changed(update));
    acts
}

/// What a tool call moving on means: what it reported once it ended, and the
/// files it changed.
fn tool_call_update(update: &Value) -> Vec<Act> {
    let ended = matches!(
        update.get("status").and_then(Value::as_str),
        Some("completed" | "failed")
    );
    if !ended {
        return Vec::new();
    }
    let report: String = blocks(update)
        .filter_map(|block| text(block.get("content")))
        .collect::<Vec<_>>()
        .join("\n");
    let mut acts = Vec::new();
    if !report.is_empty() {
        acts.push(Act::Say(Event::ToolReport {
            tool: tool(update),
            report: Text::new(&report),
        }));
    }
    acts.extend(changed(update));
    acts
}

/// The files a completed call's diffs changed.
fn changed(update: &Value) -> Vec<Act> {
    if update.get("status").and_then(Value::as_str) != Some("completed") {
        return Vec::new();
    }
    blocks(update)
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("diff"))
        .filter_map(|block| block.get("path").and_then(Value::as_str))
        .map(|path| {
            Act::Say(Event::FileChanged {
                path: Text::new(path),
            })
        })
        .collect()
}

/// A call's content blocks.
fn blocks(update: &Value) -> impl Iterator<Item = &Value> {
    update
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The cost `cost` reports, where it is an amount in a currency.
fn cost(cost: &Value) -> Option<Cost> {
    let amount = cost.get("amount")?.as_f64()?;
    let currency = Currency::new(cost.get("currency")?.as_str()?)?;
    if !amount.is_finite() || !(0.0..=AMOUNT_CEILING).contains(&amount) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the amount is finite, not negative and at most the ceiling, so its millionths, rounded, fit"
    )]
    let millionths = (amount * 1e6).round() as u64;
    Some(Cost::new(millionths, currency))
}
