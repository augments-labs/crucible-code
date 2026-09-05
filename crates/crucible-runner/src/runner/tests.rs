//! What the turn loop does, over a provider that answers from a script and
//! tools that answer from a field.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    AgentId, Approved, Aside, Attachment, Carried, Change, DescribeTool, Diff, EventEnvelope,
    InputTokenUsage, Line, Modalities, Modality, Post, PromptCacheFact, PromptCacheFingerprint,
    PromptCacheIsolation, PromptCachePersistentMode, PromptCachePolicy, PromptCachePolicyDigest,
    PromptCacheResourceBinding, PromptCacheResourceError, PromptCacheResourceHandle,
    PromptCacheResourceId, PromptCacheResourceOperation, PromptCacheResourceOwner,
    PromptCacheResourceRecord, PromptCacheResourceState, PromptCacheResourceStore,
    PromptCacheScopeDigest, ProviderError, ProviderLimit, ProviderUsage, Sensitivity, SessionId,
    Spend, Summary, Target, Tool, ToolArgs, ToolContext, ToolError, ToolId, ToolOutput, Verdict,
};

use sha2::{Digest as _, Sha256};

use super::*;
use crate::fake::{Fixed, Says, Script, Sent, Typing, changing};
use crate::outcome::RunStatus;
use crate::policy::{Bounds, Retry};
use crate::sample::Sample;

/// A policy holding one turn's tool output to `maximum` bytes, so a test can
/// put a turn over the boundary without printing megabytes to get there.
fn holding(maximum: usize) -> RunPolicy {
    RunPolicy {
        bounds: Bounds {
            tool_output_bytes: maximum,
            ..Bounds::default()
        },
        ..RunPolicy::default()
    }
}

/// The conversational messages, leaving typed harness facts to their own
/// context-assembly assertions.
fn conversation(transcript: &Transcript) -> Vec<Message> {
    transcript
        .messages()
        .iter()
        .filter(|message| !matches!(message, Message::Context(_)))
        .cloned()
        .collect()
}

/// What the model these tests ask reads: prose and the pictures they attach.
const READS: Modalities = Modalities::empty()
    .insert(Modality::Text)
    .insert(Modality::Image);

mod aiming;
mod attachments;
mod attribution;
mod compaction;
mod context;
mod lifecycle;
mod lifecycle_audit;
mod outcome;
mod pick_up;
mod preserved;
mod reporting;
mod spec;
mod spending;

/// A destination that keeps the event and lets the attribution go.
///
/// These assertions are about what a turn does rather than about whose turn it
/// was, so they go on reading plain events; the attribution module reads the
/// envelopes itself.
struct Watching(Sender<Event>);

impl Post for Watching {
    fn post(&self, reported: EventEnvelope) {
        drop(self.0.send(reported.into_event()));
    }
}

#[derive(Debug, Clone, Default)]
struct SharedStore(Arc<Mutex<Vec<PromptCacheResourceRecord>>>);

impl PromptCacheResourceStore for SharedStore {
    fn matching(
        &mut self,
        binding: &PromptCacheResourceBinding,
    ) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|record| record.binding() == binding)
            .cloned())
    }

    fn put(&mut self, record: &PromptCacheResourceRecord) -> Result<(), PromptCacheResourceError> {
        let mut records = self.0.lock().unwrap();
        if let Some(found) = records.iter_mut().find(|found| found.id() == record.id()) {
            *found = record.clone();
        } else {
            records.push(record.clone());
        }
        Ok(())
    }

    fn remove(&mut self, id: &PromptCacheResourceId) -> Result<(), PromptCacheResourceError> {
        self.0.lock().unwrap().retain(|record| record.id() != id);
        Ok(())
    }

    fn inspect(
        &mut self,
        maximum: usize,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .take(maximum)
            .cloned()
            .collect())
    }
}

/// A runner over a scripted provider, with somewhere for its events to go.
struct Scripted {
    runner: Runner,
    sent: Sent,
    says: Says,
    cancel: Cancel,
    steer: Steer,
    aside: Aside,
    events: Watching,
    seen: Receiver<Event>,
}

impl Scripted {
    fn new(script: Script, tools: Tools, verdict: Verdict) -> Self {
        Self::recording(script, tools, verdict, Session::nowhere())
    }

    fn recording(script: Script, tools: Tools, verdict: Verdict, session: Session) -> Self {
        let (events, seen) = channel();
        let sent = script.sent();

        Self {
            runner: Runner::new(
                Box::new(script),
                tools,
                AgentSpec::new(
                    AgentId::new("test"),
                    Model {
                        name: "claude-test".into(),
                        max_tokens: 1024,
                        window: None,
                        accepts: Some(READS),
                        effort: None,
                    },
                ),
                ContextInputs::new(std::env::temp_dir()).dated("2026-08-31"),
                session,
            ),
            sent,
            says: Says::new(verdict),
            cancel: Cancel::new(),
            steer: Steer::new(),
            aside: Aside::new(),
            events: Watching(events),
            seen,
        }
    }

    fn storing(mut self, store: SharedStore) -> Self {
        self.runner.prompt_cache_store = Some(Box::new(store));
        self
    }

    /// The same, on a model whose window is known and small.
    ///
    /// A window is what the proactive rail is measured against, so a session
    /// without one never reaches it — which is the point of every other test
    /// here and the thing this one has to undo.
    fn within(script: Script, window: u32, compacting: Compaction) -> Self {
        let mut scripted = Self::new(script, Tools::new(), Verdict::Allow);
        scripted.runner.spec.model.window = Some(window);
        scripted.runner.policy.compaction = compacting;
        scripted
    }

    fn turn(&mut self, prompt: &str) -> Result<StopReason, TurnError> {
        self.turning(prompt, Box::new([]))
    }

    /// Makes room the way `/compact` does.
    ///
    /// Its own piece of work with no turn around it, so it gets a run of its
    /// own — which is what the caller in the binary does too.
    fn compacting(&mut self) -> Result<Room, TurnError> {
        let run = self
            .runner
            .starting(&self.events, &self.cancel, &self.steer, &self.aside);

        self.runner
            .compact(Compacting::Asked, &run, &mut Spend::default())
    }

    /// The same, for a prompt that named files.
    fn turning(
        &mut self,
        prompt: &str,
        attachments: Box<[Attachment]>,
    ) -> Result<StopReason, TurnError> {
        let run = self
            .runner
            .starting(&self.events, &self.cancel, &self.steer, &self.aside);

        self.runner.turn(prompt, attachments, &mut self.says, &run)
    }

    /// The same, under a run that asked for less than the session allows.
    ///
    /// [`Scripted::turning`] mints its run from the session, so the two
    /// policies are equal and no assertion made through it can tell a loop
    /// reading its own run apart from one reading the runner it was started
    /// from. A descendant narrowing a figure is the case the inheritance rule
    /// exists for, and this is the only way to write it here.
    fn turning_under(&mut self, prompt: &str, asking: RunPolicy) -> Result<StopReason, TurnError> {
        let run = RunContext::new(asking, &self.events, &self.cancel, &self.steer, &self.aside);

        self.runner.turn(prompt, Box::new([]), &mut self.says, &run)
    }

    /// The files each request went out without, one entry per request that
    /// went out short, each in transcript order.
    fn aged(&self) -> Vec<Vec<Box<str>>> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Aged { files } => Some(files.iter().map(|one| one.path.clone()).collect()),
                Event::Unread { .. }
                | Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Steered { .. }
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// The files each request went out without because the model does not read
    /// them, one entry per request, each in transcript order.
    fn unread(&self) -> Vec<Vec<Box<str>>> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Unread { files } => Some(files.iter().map(|one| one.path.clone()).collect()),
                Event::Aged { .. }
                | Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Steered { .. }
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    fn said(&self) -> String {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Delta { text } => Some(text.to_string()),
                Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Aged { .. }
                | Event::Unread { .. }
                | Event::Steered { .. }
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// Each remaining-window reading posted while the turn ran.
    fn left(&self) -> Vec<Option<u8>> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Carried { left } => Some(left),
                _ => None,
            })
            .collect()
    }

    /// Which turn each start announced, in order.
    fn started(&self) -> Vec<u32> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::TurnStarted { turn } => Some(turn.get()),
                Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Aged { .. }
                | Event::Unread { .. }
                | Event::Steered { .. }
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// Why each turn ended, in order.
    fn finished(&self) -> Vec<StopReason> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::TurnFinished { stop, .. } => Some(stop),
                Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Aged { .. }
                | Event::Unread { .. }
                | Event::Steered { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// What the turn had spent at each reading, in order.
    fn spent(&self) -> Vec<u64> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Spent { spend } => Some(spend.tokens()),
                Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Aged { .. }
                | Event::Unread { .. }
                | Event::Steered { .. }
                | Event::TurnFinished { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// How many responses were asked for again.
    fn retried(&self) -> usize {
        self.seen
            .try_iter()
            .filter(|event| matches!(event, Event::Retrying))
            .count()
    }

    /// Every event still waiting, for ordering assertions that need more than
    /// one event kind.
    fn events(&self) -> Vec<Event> {
        self.seen.try_iter().collect()
    }

    /// How much transcript each request carried, in order.
    fn asked(&self) -> Vec<usize> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.transcript_len)
            .collect()
    }

    /// The tools the last request advertised.
    fn advertised(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .last()
            .map(|request| {
                request
                    .tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn tools(tools: impl IntoIterator<Item = Fixed>) -> Tools {
    let mut offered = Tools::new();
    for tool in tools {
        offered.add_builtin(tool).unwrap();
    }
    offered
}

#[test]
fn unreported_response_growth_updates_the_conservative_window_reading() {
    // The request establishes this response's four-byte-per-token rate. While
    // the provider has not reported output tokens, the visible bytes use that
    // same cautious calibration rather than making the reading disappear.
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(50_000)),
        Delta::Text("y".repeat(40_000).into()),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut scripted = Scripted::within(script, 200_000, Compaction::default());

    scripted.turn(&"x".repeat(200_000)).expect("a turn");

    assert_eq!(scripted.left(), [Some(72), Some(66)]);
}

#[test]
fn a_tool_result_keeps_a_known_window_reading_present() {
    let mut first = vec![Delta::Carried(Carried::new(50_000))];
    first.extend(calling("a", "read", "{}"));
    let mut second = vec![Delta::Carried(Carried::new(70_000))];
    second.extend(saying("done"));
    let output = "z".repeat(40_000);
    let script = Script::new(vec![first, second]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering(&output)]),
        Verdict::Allow,
    );
    scripted.runner.spec.model.window = Some(200_000);

    scripted.turn(&"x".repeat(200_000)).expect("a turn");

    let left = scripted.left();
    assert!(left.len() >= 3, "the result posted no estimate: {left:?}");
    assert!(
        left.iter().all(Option::is_some),
        "reading blinked: {left:?}"
    );
}

#[test]
fn exact_usage_cannot_make_streamed_tool_content_appear_to_free_room() {
    let id = "i".repeat(10_000);
    let args = format!(r#"{{"padding":"{}"}}"#, "a".repeat(40_000));
    let script = Script::new(vec![
        vec![
            Delta::Carried(Carried::new(50_000)),
            Delta::ToolStarted {
                id: ToolId::new(id),
                name: "read".into(),
            },
            Delta::ToolArgs(args.into()),
            Delta::Spent(Spend::new(2_000)),
            Delta::Stopped(StopReason::WantsTools),
        ],
        saying("done"),
    ]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    scripted.runner.spec.model.window = Some(200_000);

    scripted.turn(&"x".repeat(200_000)).expect("a tool turn");

    assert_eq!(
        scripted.left(),
        [Some(72), Some(70), Some(65), Some(65), Some(65)]
    );
}

/// A scripted turn over a `Steer`, for pushing a line mid-turn.
struct Steering {
    runner: Runner,
    sent: Sent,
    says: Says,
    steer: Steer,
    aside: Aside,
    events: Watching,
    seen: Receiver<Event>,
}

impl Steering {
    fn new(script: Script, tools: Tools) -> Self {
        Self::steered(Steer::new(), script, tools)
    }

    /// The same, over a queue the caller already holds.
    ///
    /// So a stand-in can put a line in it at a moment the test chooses, rather
    /// than only before the turn starts.
    fn steered(steer: Steer, script: Script, tools: Tools) -> Self {
        let (events, seen) = channel();
        let sent = script.sent();
        Self {
            runner: Runner::new(
                Box::new(script),
                tools,
                AgentSpec::new(
                    AgentId::new("test"),
                    Model {
                        name: "claude-test".into(),
                        max_tokens: 1024,
                        window: None,
                        accepts: Some(READS),
                        effort: None,
                    },
                ),
                ContextInputs::new(std::env::temp_dir()).dated("2026-08-31"),
                Session::nowhere(),
            ),
            sent,
            says: Says::new(Verdict::Allow),
            steer,
            aside: Aside::new(),
            events: Watching(events),
            seen,
        }
    }

    fn turn(&mut self, prompt: &str) -> Result<StopReason, TurnError> {
        let cancel = Cancel::new();
        let run = self
            .runner
            .starting(&self.events, &cancel, &self.steer, &self.aside);

        self.runner.turn(prompt, Box::new([]), &mut self.says, &run)
    }

    fn said(&self) -> String {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Delta { text } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    fn left(&self) -> Vec<Option<u8>> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Carried { left } => Some(left),
                _ => None,
            })
            .collect()
    }

    fn asked(&self) -> Vec<usize> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.transcript_len)
            .collect()
    }
}

#[test]
fn a_line_typed_while_a_turn_runs_is_worked_into_it_at_the_next_pass() {
    // The steer is pushed after the first response is out (so the turn is
    // running, mid-exchange) and read at the top of the pass that follows: the
    // next request has to carry it, and the agent adjust course rather than
    // finishing a plan the reader moved past.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering.steer.say("actually do this".into());

    steering.turn("first").expect("a turn");

    // Two requests were made; the second (the pass after the tool) carried the
    // steered line, so it is longer than the first.
    let asked = steering.asked();
    let [first, second] = asked.as_slice() else {
        panic!("two passes: {asked:?}");
    };
    assert!(
        second > first,
        "the next request carries the steered line: {asked:?}"
    );
}

#[test]
fn a_steered_line_keeps_a_known_window_reading_present() {
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(50_000)),
        Delta::Text("done".into()),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut steering = Steering::new(script, Tools::new());
    steering.runner.spec.model.window = Some(200_000);
    steering.steer.say("take this route".into());

    steering.turn("first").expect("a turn");

    let left = steering.left();
    assert!(!left.is_empty(), "the turn posted no reading");
    assert!(
        left.iter().all(Option::is_some),
        "reading blinked: {left:?}"
    );
}

#[test]
fn a_steered_line_lands_before_a_tool_call_that_is_still_out() {
    // The steer is read at the top of the exchange loop, never while a tool
    // call is out. Pushed while the turn is waiting on a tool, it has to be on
    // the request that follows the tool's result — not interrupt the call.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering.steer.say("wait".into());

    let stop = steering.turn("first").expect("a turn");

    assert_eq!(stop, StopReason::Yielded);
    assert_eq!(steering.said(), "done");
}

#[test]
fn steered_lines_typed_in_a_pass_arrive_together() {
    // Two lines pushed between passes both land on the next request, as a
    // burst rather than one per pass.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering.steer.say("one".into());
    steering.steer.say("two".into());

    steering.turn("first").expect("a turn");

    let asked = steering.asked();
    let [first, second] = asked.as_slice() else {
        panic!("two passes: {asked:?}");
    };
    // The second request carried both steered lines.
    assert!(
        second > first,
        "the next request carries both lines: {asked:?}"
    );
}

fn calling(id: &str, name: &str, args: &str) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new(id),
            name: name.into(),
        },
        Delta::ToolArgs(args.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

fn many_calls(first: usize, count: usize) -> Vec<Delta> {
    let mut deltas = Vec::with_capacity(count.saturating_mul(2).saturating_add(1));
    for number in first..first.saturating_add(count) {
        deltas.push(Delta::ToolStarted {
            id: ToolId::new(number.to_string()),
            name: "missing".into(),
        });
        deltas.push(Delta::ToolArgs("{}".into()));
    }
    deltas.push(Delta::Stopped(StopReason::WantsTools));
    deltas
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

/// A valid checkpoint response, with `note` somewhere tests can identify.
fn recap(note: &str) -> Vec<Delta> {
    saying(&format!(
        "## Goal\n{note}\n\n## Constraints & Preferences\n(none)\n\n## Progress\n### Done\n(none)\n\n### In Progress\n{note}\n\n### Blocked\n(none)\n\n## Decisions\n(none)\n\n## Next Steps\n1. {note}\n\n## Critical Context\n(none)"
    ))
}

#[test]
fn what_a_turn_has_spent_is_every_response_of_it_added_up() {
    // Two responses, because that is where the two readings have to be told
    // apart: within one response the number is that response's total so far and
    // replaces the one before it, and across responses they add. Reading either
    // as the other gives a count that stalls or one that doubles, and on a row
    // watched while it moves both look like the truth.
    let script = Script::new(vec![
        vec![
            Delta::Spent(Spend::new(40)),
            Delta::ToolStarted {
                id: ToolId::new("a"),
                name: "read".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::Spent(Spend::new(90)),
            Delta::Stopped(StopReason::WantsTools),
        ],
        vec![
            Delta::Text("found it".into()),
            Delta::Spent(Spend::new(30)),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").expect("the turn to finish");

    assert_eq!(scripted.spent(), [40, 90, 120]);
}

#[test]
fn normalized_usage_is_costed_on_its_attempt_without_double_counting_input() {
    let usage = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(100), Some(20)).unwrap(),
        Some(10),
        None,
        None,
        &[],
    )
    .unwrap();
    let script = Script::new(vec![vec![
        Delta::Usage(usage),
        Delta::Stopped(StopReason::Yielded),
    ]])
    .priced();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("go").expect("the turn to finish");

    let sent = scripted.sent.lock().unwrap();
    let sent_attempt = sent
        .first()
        .expect("one request was sent")
        .cache_attempt
        .expect("every provider request has an attempt");
    drop(sent);
    let events = scripted.events();
    let facts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::UsageReported(fact),
            } => Some(fact),
            _ => None,
        })
        .collect();
    let [fact] = facts.as_slice() else {
        panic!("expected one usage fact, got {facts:?}");
    };
    assert_eq!(fact.attempt, sent_attempt);
    assert_eq!(fact.usage.input.total, Some(100));
    assert_eq!(fact.usage.input.uncached, Some(80));
    assert_eq!(
        fact.cost
            .total
            .expect("all rates are known")
            .femtocurrency(),
        132_000_000_000
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Event::Spent { spend } => Some(spend.tokens()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [10]
    );
}

#[test]
fn partial_usage_readings_merge_on_one_attempt_instead_of_erasing_input() {
    let input = ProviderUsage::new(
        InputTokenUsage::disjoint(Some(70), Some(20), Some(10)).unwrap(),
        None,
        None,
        None,
        &[],
    )
    .unwrap();
    let output = ProviderUsage::new(InputTokenUsage::UNKNOWN, Some(12), None, None, &[]).unwrap();
    let script = Script::new(vec![vec![
        Delta::Usage(input),
        Delta::Usage(output),
        Delta::Stopped(StopReason::Yielded),
    ]])
    .priced();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("go").expect("the turn to finish");

    let facts: Vec<_> = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::UsageReported(fact),
            } => Some(fact),
            _ => None,
        })
        .collect();
    let [_, complete] = facts.as_slice() else {
        panic!("each provider reading must be recorded once: {facts:?}");
    };
    assert_eq!(complete.usage.input.total, Some(100));
    assert_eq!(complete.usage.output, Some(12));
    assert_eq!(complete.usage.total, Some(112));
    assert_eq!(
        complete.outcome,
        crucible_core::PromptCacheOutcome::ReadAndWrite
    );
    assert!(complete.cost.total.is_some());
}

#[test]
fn cancellation_after_usage_keeps_the_provider_fact_on_its_attempt() {
    let usage = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(100), Some(25)).unwrap(),
        Some(7),
        None,
        None,
        &[],
    )
    .unwrap();
    let script = Script::new(vec![vec![
        Delta::Usage(usage.clone()),
        Delta::Stopped(StopReason::Cancelled),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    let attempt = scripted.runner.prompt_cache_attempt().expect("an attempt");
    assert_eq!(attempt.usage.as_ref(), Some(&usage));
    assert_eq!(attempt.outcome, crucible_core::PromptCacheOutcome::Read);
    assert_eq!(
        scripted
            .events()
            .into_iter()
            .filter(|event| matches!(
                event,
                Event::PromptCache {
                    fact: PromptCacheFact::UsageReported(_),
                }
            ))
            .count(),
        1
    );
}

#[test]
fn prefer_records_an_adapter_encoding_failure_then_sends_the_unchanged_request() {
    let script = Script::new(vec![saying("done")]).failing_cache_encoding();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.runner.spec.told("stable fixture instructions");

    scripted
        .turn("go")
        .expect("prefer to fall back before send");

    let sent_guard = scripted.sent.lock().unwrap();
    let [sent] = sent_guard.as_slice() else {
        panic!("prefer fallback must send exactly one request");
    };
    assert!(
        sent.cache_selection
            .is_some_and(|selection| selection.selected().is_none())
    );
    drop(sent_guard);
    let encodings: Vec<_> = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::RequestEncoded(fact),
            } => Some(fact.encoding),
            _ => None,
        })
        .collect();
    assert_eq!(
        encodings,
        [
            PromptCacheEncoding::Failed(
                crucible_core::PromptCacheIneligibleReason::UnsupportedBoundary,
            ),
            PromptCacheEncoding::NoControlIntended,
        ]
    );
}

#[test]
fn require_fails_before_send_when_the_adapter_cannot_lower_the_selected_control() {
    let script = Script::new(vec![saying("must not be sent")]).failing_cache_encoding();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.runner.spec.told("stable fixture instructions");
    scripted.runner.policy.prompt_cache =
        PromptCachePolicy::default().with_mode(crucible_core::PromptCacheMode::Require);

    let problem = scripted.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::PromptCachePreparation(PromptCachePreparationError::Encoding(
            crucible_core::PromptCacheIneligibleReason::UnsupportedBoundary
        ))
    ));
    assert!(scripted.sent.lock().unwrap().is_empty());
}

#[test]
fn persistent_resources_are_ready_before_wire_reference_and_explicit_cleanup_deletes_them() {
    let script = Script::new(vec![saying("done")]).persistent();
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny).storing(store);
    scripted.runner.spec.told("stable fixture instructions");
    scripted.runner.policy.prompt_cache = scripted
        .runner
        .policy
        .prompt_cache
        .with_persistent_resources(PromptCachePersistentMode::Create);

    scripted.turn("go").expect("the turn to finish");

    let sent_guard = scripted.sent.lock().unwrap();
    let [sent] = sent_guard.as_slice() else {
        panic!("persistent fixture must send exactly one request");
    };
    assert!(sent.cache_resource, "{:?}", sent.cache_selection);
    drop(sent_guard);
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("persistent fixture must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ready);
    drop(held);
    assert_eq!(
        scripted
            .runner
            .prompt_cache_attempt()
            .expect("an attempt")
            .encoding,
        PromptCacheEncoding::PersistentResourceReferenced
    );
    let lifecycle_states: Vec<_> = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::ResourceChanged(fact),
            } => Some((fact.operation, fact.state)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle_states,
        [
            (
                Some(PromptCacheResourceOperation::Create),
                PromptCacheResourceState::Creating,
            ),
            (
                Some(PromptCacheResourceOperation::Create),
                PromptCacheResourceState::Ready,
            ),
        ]
    );

    let cleaned = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .expect("bounded cleanup");
    assert_eq!(cleaned.deleted, 1);
    assert!(records.lock().unwrap().is_empty());
}

#[test]
fn retirement_deletes_only_the_current_exclusive_owner_scope() {
    let script = Script::new(vec![saying("done")]).persistent();
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny).storing(store);
    scripted.runner.spec.told("stable fixture instructions");
    scripted.runner.policy.prompt_cache = scripted
        .runner
        .policy
        .prompt_cache
        .with_persistent_resources(PromptCachePersistentMode::Create);

    scripted.turn("go").expect("the turn to finish");

    let current = records
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the current owner resource");
    let binding = PromptCacheResourceBinding::new(
        PromptCacheScopeDigest::new([91; 32]),
        current.binding().provider_scope(),
        PromptCacheScopeDigest::new([92; 32]),
        PromptCacheFingerprint::new([93; 32]),
        PromptCachePolicyDigest::new([94; 32]),
        PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
        current.binding().protocol(),
        "other-session-model",
        Some("script-revision-v1"),
    )
    .unwrap();
    let mut other_owner =
        PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), binding, 100);
    other_owner.ready(
        PromptCacheResourceHandle::new("other-owner-handle").unwrap(),
        u64::MAX,
        110,
    );
    records.lock().unwrap().push(other_owner.clone());

    let retired = scripted
        .runner
        .retire_prompt_cache(&Cancel::new())
        .expect("bounded retirement");

    assert_eq!(retired.inspected, 1);
    assert_eq!(retired.deleted, 1);
    let remaining = records.lock().unwrap();
    let [record] = remaining.as_slice() else {
        panic!("another owner scope must remain untouched");
    };
    assert_eq!(record.id(), other_owner.id());
}

fn ready_resource(
    protocol: &str,
    provider_scope: PromptCacheScopeDigest,
    seed: u8,
) -> PromptCacheResourceRecord {
    let binding = PromptCacheResourceBinding::new(
        PromptCacheScopeDigest::new([seed; 32]),
        provider_scope,
        PromptCacheScopeDigest::new([seed.saturating_add(3); 32]),
        PromptCacheFingerprint::new([seed.saturating_add(1); 32]),
        PromptCachePolicyDigest::new([seed.saturating_add(2); 32]),
        PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
        protocol,
        "claude-test",
        Some("script-revision-v1"),
    )
    .unwrap();
    let mut record =
        PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), binding, 100);
    record.ready(
        PromptCacheResourceHandle::new(format!("provider-handle-{seed}")).unwrap(),
        u64::MAX,
        110,
    );
    record
}

#[test]
fn cleanup_without_the_current_provider_lifecycle_fails_without_relabelling_records() {
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted =
        Scripted::new(Script::new(Vec::new()), Tools::new(), Verdict::Deny).storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records
        .lock()
        .unwrap()
        .push(ready_resource("script", provider_scope, 1));

    let problem = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .unwrap_err();

    assert!(matches!(problem, PromptCacheResourceError::Unsupported));
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("unsupported cleanup must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ready);
}

#[test]
fn cleanup_is_provider_scoped_and_marks_a_conclusive_survivor_orphaned() {
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(
        Script::new(Vec::new()).surviving_delete(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records.lock().unwrap().extend([
        ready_resource("script", provider_scope, 1),
        ready_resource(
            "another-protocol",
            PromptCacheScopeDigest::new([90; 32]),
            10,
        ),
    ]);

    let cleaned = scripted.runner.clean_prompt_cache(&Cancel::new()).unwrap();

    assert_eq!(cleaned.inspected, 1);
    assert_eq!(cleaned.orphaned, 1);
    let records = records.lock().unwrap();
    let [owned, other] = records.as_slice() else {
        panic!("protocol-scoped cleanup must retain both fixture records");
    };
    assert_eq!(owned.state(), PromptCacheResourceState::Orphaned);
    assert_eq!(other.state(), PromptCacheResourceState::Ready);
}

#[test]
fn ambiguous_delete_is_retained_for_reconciliation_and_pre_cancel_changes_nothing() {
    let store = SharedStore::default();
    let resumed_store = store.clone();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(
        Script::new(Vec::new()).ambiguous_delete(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records
        .lock()
        .unwrap()
        .push(ready_resource("script", provider_scope, 1));
    let cancelled = Cancel::new();
    cancelled.request();

    assert!(matches!(
        scripted.runner.clean_prompt_cache(&cancelled),
        Err(PromptCacheResourceError::Cancelled)
    ));
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("pre-cancelled cleanup must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ready);
    drop(held);

    let cleaned = scripted.runner.clean_prompt_cache(&Cancel::new()).unwrap();
    assert_eq!(cleaned.ambiguous, 1);
    assert_eq!(
        cleaned
            .changes()
            .iter()
            .map(|change| change.state)
            .collect::<Vec<_>>(),
        [
            PromptCacheResourceState::Deleting,
            PromptCacheResourceState::Ambiguous,
        ]
    );
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("ambiguous cleanup must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ambiguous);
    assert_eq!(record.pending(), Some(PromptCacheResourceOperation::Delete));
    drop(held);

    let credential_scope = scripted
        .runner
        .provider
        .prompt_cache_route()
        .credential_scope;
    let mut resumed = Scripted::new(
        Script::new(Vec::new())
            .with_credential_scope(credential_scope)
            .persistent(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(resumed_store);
    let reconciled = resumed.runner.clean_prompt_cache(&Cancel::new()).unwrap();

    assert_eq!(reconciled.deleted, 1);
    assert_eq!(
        reconciled
            .changes()
            .iter()
            .map(|change| (change.operation, change.state))
            .collect::<Vec<_>>(),
        [(
            Some(PromptCacheResourceOperation::Delete),
            PromptCacheResourceState::Deleted,
        )]
    );
    assert!(records.lock().unwrap().is_empty());
}

#[test]
fn a_turn_that_is_never_told_what_it_spent_says_nothing_about_it() {
    // Every provider reports this differently and one of them may not report it
    // at all. Nothing here invents a number for that case: no reading, no
    // event, and the row above the box has one segment fewer.
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("go").expect("the turn to finish");

    assert!(scripted.spent().is_empty());
}

#[test]
fn a_turn_that_yields_records_what_the_model_said() {
    let script = Script::new(vec![vec![
        Delta::Text("Hello".into()),
        Delta::Text(", world".into()),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    assert_eq!(scripted.turn("hi").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.said(), "Hello, world");
    assert_eq!(
        conversation(scripted.runner.transcript()),
        [
            Message::said("hi"),
            Message::Agent {
                text: "Hello, world".into(),
                calls: Vec::new(),
                stop: Some(StopReason::Yielded),
            },
        ]
    );
}

#[test]
fn a_tool_call_runs_and_what_it_produced_goes_back_to_the_model() {
    let script = Script::new(vec![
        calling("a", "read", r#"{"path":"x"}"#),
        saying("it says hello"),
    ]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering("fn main() {}")]),
        Verdict::Allow,
    );

    assert_eq!(scripted.turn("read x").unwrap(), StopReason::Yielded);

    let messages = conversation(scripted.runner.transcript());
    assert_eq!(messages.len(), 4, "prompt, call, result, answer");
    assert!(matches!(
        messages.get(2),
        Some(Message::ToolResults(results))
            if results.first().is_some_and(|r| r.output.text() == "fn main() {}")
    ));
}

#[test]
fn the_second_request_carries_the_first_pass_in_full() {
    // Without it the model answers the same question again, having no
    // record of the tool it just called.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").unwrap();

    assert_eq!(
        scripted.asked(),
        [7, 9],
        "the first request adds six sections; the second adds only call and result"
    );
}

#[test]
fn a_turn_runs_as_long_as_there_is_work_in_it() {
    // The failure this whole feature exists to remove. Two hundred tool calls
    // across forty responses used to end the turn on a count — and the message
    // named the vendor for a bound that was crucible's own. Nothing counts them
    // now: a turn is long because there is work in it, and what actually runs
    // out is the model's window, which is measured rather than guessed at.
    let mut rounds: Vec<Vec<Delta>> = (0..40).map(|_| many_calls(0, 5)).collect();
    rounds.push(vec![
        Delta::Text("done".into()),
        Delta::Stopped(StopReason::Yielded),
    ]);

    let mut scripted = Scripted::new(Script::new(rounds), Tools::new(), Verdict::Allow);

    let stop = scripted
        .turn("go")
        .expect("the turn was stopped by a count");

    assert_eq!(stop, StopReason::Yielded);
    assert_eq!(scripted.asked().len(), 41);
}

#[test]
fn tool_results_past_the_retained_boundary_end_the_turn() {
    let script = Script::new(vec![calling("a", "read", "{}")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering("ninebytes")]),
        Verdict::Allow,
    );

    let run = RunContext::new(
        holding(8),
        &scripted.events,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );
    let problem = scripted
        .runner
        .exchange(&mut scripted.says, &run)
        .unwrap_err();

    assert!(matches!(problem, TurnError::ToolOutputBytes { maximum: 8 }));
    assert!(matches!(
        scripted.runner.transcript().messages().last(),
        Some(Message::ToolResults(results)) if results.len() == 1
    ));
}

#[test]
fn a_tool_the_user_refused_ends_the_turn_and_is_still_answered() {
    let script = Script::new(vec![calling("a", "write", "{}")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("write").risking(changing())]),
        Verdict::Deny,
    );

    let problem = scripted.turn("write it").unwrap_err();

    assert_eq!(problem.to_string(), "write was not allowed");
    assert!(
        matches!(
            scripted.runner.transcript().messages().last(),
            Some(Message::ToolResults(results)) if results.len() == 1
        ),
        "a call with no result is a transcript the provider refuses"
    );
}

#[test]
fn a_call_the_model_never_finished_asking_for_is_not_recorded() {
    // Cancelled mid-sentence: the arguments are half a JSON object, and
    // there will never be a result to pair with the call.
    let script = Script::new(vec![vec![
        Delta::Text("looking".into()),
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: "read".into(),
        },
        Delta::ToolArgs("{\"path\":".into()),
        Delta::Stopped(StopReason::Cancelled),
    ]]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    assert_eq!(
        conversation(scripted.runner.transcript()),
        [
            Message::said("go"),
            Message::Agent {
                text: "looking".into(),
                calls: Vec::new(),
                stop: Some(StopReason::Cancelled),
            },
        ]
    );
}

#[test]
fn a_model_that_wants_tools_but_names_none_yields_instead_of_asking_again() {
    // An unchanged transcript sent again produces the same answer again.
    let script = Script::new(vec![vec![Delta::Stopped(StopReason::WantsTools)]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.asked().len(), 1, "the model was asked again");
}

#[test]
fn a_provider_that_fails_ends_the_turn() {
    let mut scripted = Scripted::new(Script::failing(), Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::Provider(ProviderError::Refused { .. })
    ));

    // A key without access says the same thing however many times it is asked,
    // so asking again spends the user's time to reach the same message.
    assert_eq!(scripted.asked().len(), 1, "the request went out again");
    assert_eq!(scripted.retried(), 0);
}

#[test]
fn an_answer_the_connection_broke_off_is_still_in_the_transcript() {
    // Those deltas were posted as they arrived, so the user has read them.
    // Dropping them leaves a transcript the user and the model disagree about:
    // the next prompt follows the last one with nothing in between, and every
    // request for the rest of the session — and every continuation of it —
    // carries the two questions back to back.
    let script = Script::breaking(vec![vec![
        Delta::Text("let me look at ".into()),
        Delta::Text("src/main.rs".into()),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted.turn("what is in main.rs?").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Transport { .. })
        ),
        "{problem}"
    );
    assert_eq!(
        conversation(scripted.runner.transcript()),
        [
            Message::said("what is in main.rs?"),
            Message::Agent {
                text: "let me look at src/main.rs".into(),
                calls: Vec::new(),
                stop: None,
            },
        ]
    );

    // And is never asked for again. The deltas are on screen; a second answer
    // would be written under the half of the first one the user already read.
    assert_eq!(scripted.asked().len(), 1, "the request went out again");
    assert_eq!(scripted.retried(), 0);
}

#[test]
fn a_response_that_went_away_before_it_said_anything_is_asked_for_again() {
    // The failure this exists for: a connection the provider closed while the
    // tools ran. The request is accepted, the stream produces nothing at all,
    // and the turn that would have ended there instead asks once more.
    let script = Script::dropping(1, vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.retried(), 1);
    assert_eq!(scripted.asked().len(), 2, "the request went out once");

    let sent = scripted.sent.lock().unwrap();
    let [first, second] = sent.as_slice() else {
        panic!("the original request and retry were retained")
    };
    assert_ne!(
        first.cache_attempt, second.cache_attempt,
        "a retry reused its provider-attempt identity"
    );
    assert_eq!(
        first.cache_identity, second.cache_identity,
        "an unchanged logical retry forked its scoped cache identity"
    );
    assert!(first.cache_attempt.is_some());
    assert!(first.cache_selection.is_some());
    drop(sent);

    // Nothing of the attempt that went away is left behind: an empty agent
    // message here is one the next request carries, and every request after it.
    assert_eq!(
        conversation(scripted.runner.transcript()),
        [
            Message::said("go"),
            Message::Agent {
                text: "done".into(),
                calls: Vec::new(),
                stop: Some(StopReason::Yielded),
            },
        ]
    );
}

#[test]
fn usage_reported_before_an_ambiguous_retry_is_attributed_once_to_each_attempt() {
    let first_usage = ProviderUsage::new(
        InputTokenUsage::inclusive_read_write(Some(100), Some(10), Some(5)).unwrap(),
        None,
        None,
        Some(100),
        &[],
    )
    .unwrap();
    let second_usage = ProviderUsage::new(
        InputTokenUsage::inclusive_read_write(Some(120), Some(20), Some(0)).unwrap(),
        Some(4),
        None,
        Some(124),
        &[],
    )
    .unwrap();
    let script = Script::dropping_with_usage(
        first_usage.clone(),
        vec![vec![
            Delta::Usage(second_usage.clone()),
            Delta::Stopped(StopReason::Yielded),
        ]],
    )
    .persistent();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("go").unwrap();

    let facts: Vec<_> = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::UsageReported(fact),
            } => Some(fact),
            _ => None,
        })
        .collect();
    assert_eq!(
        facts.len(),
        2,
        "a reported attempt was duplicated or discarded"
    );
    let [first, second] = facts.as_slice() else {
        panic!("retry fixture must report exactly two attempt facts");
    };
    assert_ne!(first.attempt, second.attempt);
    assert_eq!(first.usage, first_usage);
    assert_eq!(second.usage, second_usage);
    assert_eq!(scripted.sent.lock().unwrap().len(), 2);
}

#[test]
fn a_response_that_keeps_going_away_ends_the_turn() {
    let script = Script::dropping(usize::MAX, Vec::new());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Transport { .. })
        ),
        "{problem}"
    );

    // Bounded, and the bound is the constant rather than a number written out
    // here: what this pins is that the loop stops rather than how soon.
    assert_eq!(
        scripted.retried(),
        usize::from(RunPolicy::default().retry.attempts)
    );
    assert_eq!(
        scripted.asked().len(),
        1 + usize::from(RunPolicy::default().retry.attempts)
    );
}

#[test]
fn a_service_that_says_it_is_busy_is_asked_again_and_a_key_without_access_is_not() {
    // Both are refusals and only the status tells them apart, which is the whole
    // of what `transient` decides: 503 is about the moment, 401 about the key.
    let mut busy = Scripted::new(Script::refusing(503), Tools::new(), Verdict::Allow);
    busy.turn("go").unwrap_err();
    assert_eq!(
        busy.asked().len(),
        1 + usize::from(RunPolicy::default().retry.attempts)
    );

    let mut refused = Scripted::new(Script::refusing(403), Tools::new(), Verdict::Allow);
    refused.turn("go").unwrap_err();
    assert_eq!(refused.asked().len(), 1);
}

#[test]
fn a_run_that_asked_to_be_asked_again_fewer_times_is() {
    // The figure above comes off the run, not off the runner, and every test
    // here that goes through `Scripted::turn` mints the two equal. The same
    // busy service, under a run that
    // gave up its retries: one request, where the session would have made
    // more. How many more is the shipped default's business, and the second
    // assertion asks it rather than naming it here.
    let mut busy = Scripted::new(Script::refusing(503), Tools::new(), Verdict::Allow);

    busy.turning_under(
        "go",
        RunPolicy {
            retry: Retry {
                attempts: 0,
                ..Retry::default()
            },
            ..RunPolicy::default()
        },
    )
    .unwrap_err();

    assert_eq!(
        busy.asked().len(),
        1,
        "the session's retry count was used in place of the run's"
    );
    assert!(
        RunPolicy::default().retry.attempts > 1,
        "the session would have asked again, so one request proves the run was read"
    );
}

#[test]
fn a_run_that_asked_for_a_smaller_answer_is_held_to_its_own_ceiling() {
    // The turn-wide response ceiling, likewise: read off the run each pass
    // builds its answer under. Eight bytes is under any real answer and far
    // under the shipped default — which is the constant's business rather than
    // a number written out here — so a loop reading the session's figure would
    // let this whole response through.
    let script = Script::new(vec![saying("more prose than eight bytes of room")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted
        .turning_under(
            "go",
            RunPolicy {
                bounds: Bounds {
                    response_bytes: 8,
                    ..Bounds::default()
                },
                ..RunPolicy::default()
            },
        )
        .expect_err("an answer past the room the run asked for");

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Limit {
                limit: ProviderLimit::TurnResponseBytes,
                maximum: 8,
                ..
            })
        ),
        "stopped for the wrong reason: {problem:?}"
    );
}

#[test]
fn a_raised_cancel_ends_a_pause_instead_of_waiting_it_out() {
    // The pause is the one place a turn waits with nothing arriving, so it is
    // the one place Esc could be swallowed.
    //
    // Asked of `pausing` directly, and with a pause far longer than any the
    // runner uses, because that is what makes the two answers tell each other
    // apart. Honoured, this returns in the time it takes to read a flag;
    // ignored, it sleeps for a minute. Between those, a second of scheduler
    // noise on a shared machine decides nothing.
    //
    // The turn-level version of this measured 125 ms against a real wait of
    // about 225 ms, and failed four times on CI for want of the scheduler
    // rather than for want of the behaviour. Widening it far enough to stop
    // that made it stop failing when the cancel was ignored too, which is the
    // worse of the two.
    let cancel = Cancel::new();
    cancel.request();

    let started = Instant::now();
    let held = Runner::pausing(Duration::from_mins(1), &cancel);

    assert!(!held, "the pause reported that it ran to the end");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the pause held for {:?} with the cancel already raised",
        started.elapsed(),
    );
}

#[test]
fn asking_to_stop_during_the_pause_stops_the_retry() {
    // What the turn does with the above: the attempts that were left are not
    // made. Counted rather than timed — the count is the same on any machine,
    // and it is the fact a user is complaining about when Esc seems ignored.
    let script = Script::dropping(usize::MAX, Vec::new());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let cancel = scripted.cancel.clone();
    let esc = thread::spawn(move || {
        thread::sleep(CANCEL_SLICE);
        cancel.request();
    });

    scripted.turn("go").unwrap_err();
    esc.join().unwrap();

    assert!(
        scripted.asked().len() < 1 + usize::from(RunPolicy::default().retry.attempts),
        "every attempt went out anyway"
    );
}

#[test]
fn the_tools_a_runner_offers_are_advertised_on_every_request() {
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").unwrap();

    assert_eq!(scripted.advertised(), ["read"]);
}

#[test]
fn a_turn_that_finds_the_flag_raised_stops_without_sending_anything() {
    // The press arrived after the caller cleared the flag and before this
    // thread reached its first instruction. Clearing it here instead would wipe
    // it: the user would have pressed Esc and watched the turn carry on.
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.cancel.request();

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    assert!(
        scripted.asked().is_empty(),
        "a request went out for a turn the user had already stopped"
    );
    assert_eq!(
        scripted.finished(),
        [StopReason::Cancelled],
        "the turn ended without saying so"
    );
    assert!(
        scripted.runner.transcript().is_empty(),
        "a turn that never ran recorded a prompt the model was never told"
    );
}
