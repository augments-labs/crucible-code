//! Streaming the private conversation record for the reader, independently of
//! the compacted transcript sent to a provider. Only one message, one tool
//! batch's bounded previews and one agent turn's bounded steps are retained
//! here; the renderer owns scrollback.
//!
//! An installed agent's calls are kept as display-only steps, each written as
//! the turn drew it and handed back with the answer it was said in. Model
//! replay passes over them, as an older build passes over the whole kind and
//! this one passes over a step in a shape it does not know.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read as _, Take};

use crucible_tools::{Argument, Looking, Summary};
use crucible_types::{
    Change, Compacted, Compacting, Diff, Line, Message, RecordedToolOutput, ToolId, ToolResult,
};
use serde_json::{Value, json};

use super::{replay, wire};

/// One visible fact in a session's original chronological history.
#[derive(Debug)]
pub enum DisplayItem {
    /// Original conversation content, and the previews kept for the reader.
    Message {
        /// What the participants said, exactly as the record holds it.
        message: Message,
        /// Bounded change previews the log kept for this batch's results.
        ///
        /// Beside the record rather than inside it, because a preview is the
        /// one thing here the model was never sent: a result carrying it would
        /// say different things to the two readers of the same call. Empty for
        /// every message that is not a tool-result batch, and for a batch
        /// whose log kept no preview.
        previews: HashMap<ToolId, Diff>,
        /// What an installed agent did while it said this, in the order it
        /// did it. Empty for every message no agent's turn ended in.
        steps: Vec<AgentStep>,
    },
    /// What an installed agent did in a turn that ended with nothing said, or
    /// in the part of a turn too long to hold until its answer was read.
    Agent(Vec<AgentStep>),
    /// A completed compaction with the exact live measurements.
    Compacted(Compacted),
    /// A legacy compaction or pruning whose live measurements were not saved.
    LegacyCompacted {
        /// Number of earlier messages replaced; zero means output pruning.
        replaced: usize,
    },
    /// A legacy model-context reset; existing terminal scrollback stayed visible.
    ContextReset,
}

/// A message read past a notice it has to wait behind, with its previews, its
/// steps and where its record begins.
type Queued = (Message, HashMap<ToolId, Diff>, Vec<AgentStep>, u64);

/// A fixed, bounded-record view of a protected session log.
///
/// This iterator never executes tools, reads workspace files or changes model
/// context. Its contents may be sensitive and must only reach the renderer.
pub struct DisplayHistory {
    log: BufReader<Take<File>>,
    raw: Vec<u8>,
    previews: HashMap<ToolId, Diff>,
    preview_bytes: usize,
    calls: Vec<ToolId>,
    notices: VecDeque<DisplayItem>,
    ready: bool,
    queued: Option<Queued>,
    /// The steps of the agent turn being read, and the bytes they came from.
    steps: Vec<AgentStep>,
    step_bytes: usize,
    /// Where the record that ended a turn of steps begins, read again once
    /// those steps have been handed out.
    again: Option<u64>,
    ended: bool,
    /// How many bytes of the log have been read, which is where the next
    /// record begins.
    read: u64,
    /// Where the record the last message handed out came from begins.
    last: Option<u64>,
}

impl fmt::Debug for DisplayHistory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DisplayHistory")
            .field("contents", &"[redacted]")
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl DisplayHistory {
    pub(super) fn open(file: File) -> io::Result<Self> {
        let length = file.metadata()?.len();
        let mut history = Self {
            log: BufReader::new(file.take(length)),
            raw: Vec::new(),
            previews: HashMap::new(),
            preview_bytes: 0,
            calls: Vec::new(),
            notices: VecDeque::new(),
            ready: false,
            queued: None,
            steps: Vec::new(),
            step_bytes: 0,
            again: None,
            ended: false,
            read: 0,
            last: None,
        };
        let header = replay::read_record(&mut history.log, &mut history.raw, replay::RECORD_BYTES)?;
        history.read = u64::try_from(header).unwrap_or(u64::MAX);
        Ok(history)
    }

    fn read_next(&mut self) -> io::Result<Option<DisplayItem>> {
        if self.ready || self.ended {
            if let Some(notice) = self.notices.pop_front() {
                return Ok(Some(notice));
            }
            self.ready = false;
        }
        if let Some((message, previews, steps, at)) = self.queued.take() {
            self.last = Some(at);
            return Ok(Some(DisplayItem::Message {
                message,
                previews,
                steps,
            }));
        }
        if self.ended {
            return Ok(None);
        }
        loop {
            // A record that ended a turn of steps is still in hand, and is
            // read again now that the steps have gone out ahead of it.
            let at = if let Some(at) = self.again.take() {
                at
            } else {
                {
                    self.raw.clear();
                    let at = self.read;
                    let read =
                        replay::read_record(&mut self.log, &mut self.raw, replay::RECORD_BYTES)?;
                    self.read = self
                        .read
                        .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                    if read == 0 || !self.raw.ends_with(b"\n") {
                        self.ended = true;
                        if !self.steps.is_empty() {
                            let steps = self.stepped();
                            self.notices.push_back(DisplayItem::Agent(steps));
                        }
                        return Ok(self.notices.pop_front());
                    }
                    at
                }
            };
            let text = std::str::from_utf8(&self.raw)
                .map_err(|_| invalid())?
                .trim_end();
            if text.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
            let kind = value
                .get("run_item")
                .and_then(|item| item.get("body"))
                .and_then(|body| body.get("kind"))
                .and_then(Value::as_str);
            if kind == Some("agent_step") {
                // Only drawn, never sent: a step in a shape a later build
                // wrote is passed over rather than costing the whole session.
                let Some(step) = value
                    .get("run_item")
                    .and_then(|item| item.get("body"))
                    .and_then(read_step)
                else {
                    continue;
                };
                self.steps.push(step);
                self.step_bytes = self.step_bytes.saturating_add(text.len());
                // A turn may go on for longer than is worth holding: what it
                // did so far goes out on its own, ahead of what it said.
                if self.step_bytes > crucible_storage::MAX_RUN_HISTORY_BYTES
                    || self.steps.len() >= crucible_storage::MAX_RUN_ITEMS
                {
                    return Ok(Some(DisplayItem::Agent(self.stepped())));
                }
                continue;
            }
            // Anything a reader sees ends the turn the steps were taken in,
            // except the answer they were taken while saying, which carries
            // them. Context is the model's alone and ends nothing here.
            if !self.steps.is_empty() && ends_steps(kind, text) {
                self.again = Some(at);
                return Ok(Some(DisplayItem::Agent(self.stepped())));
            }
            if let Some(body) = value.get("run_item").and_then(|item| item.get("body")) {
                if kind == Some("display_compaction") {
                    let details = read_compacted(body).ok_or_else(invalid)?;
                    let pruned = match body.get("pruned") {
                        Some(value) => value.as_bool().ok_or_else(invalid)?,
                        None => false,
                    };
                    // A live operation writes at most a pruning and a recap.
                    // Its exact record identifies whether both belong to it;
                    // older unmatched facts must remain in chronological order.
                    let companions = if pruned && details.replaced > 0 { 2 } else { 1 };
                    for _ in 0..companions {
                        if !matches!(
                            self.notices.pop_back(),
                            Some(DisplayItem::LegacyCompacted { .. })
                        ) {
                            return Err(invalid());
                        }
                    }
                    self.notices.push_back(DisplayItem::Compacted(details));
                    self.ready = true;
                    return Ok(self.notices.pop_front());
                } else if let Some(result) = body
                    .get("invocation_state")
                    .and_then(|state| state.get("result"))
                    && let Some(preview) = result.get("display_diff")
                {
                    let call = ToolId::new(
                        result
                            .get("id")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?,
                    );
                    if self.calls.contains(&call) {
                        let diff = read_preview(preview).ok_or_else(invalid)?;
                        if self.previews.contains_key(&call) {
                            return Err(invalid());
                        }
                        let held = self
                            .preview_bytes
                            .checked_add(diff.retained())
                            .ok_or_else(invalid)?;
                        // A batch may contain 128 live previews, each with
                        // 64 lines of 1024 four-byte characters. The framework
                        // history ceiling admits that complete batch while
                        // bounding a corrupt log independently of its call IDs.
                        if held > crucible_storage::MAX_RUN_HISTORY_BYTES {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "session display previews exceed the tool-batch byte limit",
                            ));
                        }
                        self.preview_bytes = held;
                        self.previews.insert(call, diff);
                    }
                }
                continue;
            }
            if let Some((replaced, _)) = wire::made_room(text) {
                if let Some(previous) = self.legacy(replaced) {
                    return Ok(Some(previous));
                }
                continue;
            }
            if wire::cleared(text).is_some() {
                if let Some(previous) = self.legacy(0) {
                    return Ok(Some(previous));
                }
                continue;
            }
            if wire::forgets(text) {
                self.previews.clear();
                self.preview_bytes = 0;
                self.calls.clear();
                self.notices.push_back(DisplayItem::ContextReset);
                self.ready = true;
                return Ok(self.notices.pop_front());
            }
            let Some(message) = wire::message(text) else {
                // Format requirements, context patches and usage calibrations
                // were validated by model replay and are not visible records.
                continue;
            };
            let mut shown = HashMap::new();
            match &message {
                Message::Context(_) => continue,
                Message::Agent { calls, .. } => {
                    self.previews.clear();
                    self.preview_bytes = 0;
                    self.calls = calls.iter().map(|call| call.id.clone()).collect();
                }
                Message::ToolResults(results) => {
                    for result in results {
                        if let Some(diff) = self.previews.remove(&result.id) {
                            shown.insert(result.id.clone(), diff);
                        }
                    }
                    self.previews.clear();
                    self.preview_bytes = 0;
                    self.calls.clear();
                }
                Message::User { .. } => {}
            }
            let steps = if matches!(message, Message::Agent { .. }) {
                self.stepped()
            } else {
                Vec::new()
            };
            if let Some(notice) = self.notices.pop_front() {
                self.queued = Some((message, shown, steps, at));
                self.ready = true;
                return Ok(Some(notice));
            }
            self.last = Some(at);
            return Ok(Some(DisplayItem::Message {
                message,
                previews: shown,
                steps,
            }));
        }
    }

    /// The steps held so far, handed over.
    fn stepped(&mut self) -> Vec<AgentStep> {
        self.step_bytes = 0;
        std::mem::take(&mut self.steps)
    }

    /// Where in the log the record the last message handed out came from
    /// begins, which is what a [`super::Place`] of one of its results holds.
    ///
    /// Asked between items rather than carried in them, so the items stay what
    /// the reader draws and the position stays what the log is read back by.
    /// `None` until a message has been handed out.
    #[must_use]
    pub const fn placed(&self) -> Option<u64> {
        self.last
    }

    /// Two facts are enough to match the largest live compaction operation.
    /// Emit anything older before reading further, regardless of log length.
    fn legacy(&mut self, replaced: usize) -> Option<DisplayItem> {
        self.notices
            .push_back(DisplayItem::LegacyCompacted { replaced });
        if self.notices.len() > 2 {
            self.notices.pop_front()
        } else {
            None
        }
    }
}

/// Whether this record is one a reader sees that is not an agent's answer,
/// which is what ends a turn of steps before an answer came.
fn ends_steps(kind: Option<&str>, text: &str) -> bool {
    if kind == Some("display_compaction") {
        return true;
    }
    if kind.is_some() {
        return false;
    }
    if wire::made_room(text).is_some() || wire::cleared(text).is_some() || wire::forgets(text) {
        return true;
    }
    matches!(
        wire::message(text),
        Some(Message::User { .. } | Message::ToolResults(_))
    )
}

impl Iterator for DisplayHistory {
    type Item = io::Result<DisplayItem>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.read_next() {
            Ok(item) => item.map(Ok),
            Err(problem) => {
                self.ended = true;
                self.notices.clear();
                self.queued = None;
                Some(Err(problem))
            }
        }
    }
}

pub(super) fn preview(diff: &Diff) -> Value {
    json!({
        "added": diff.added(), "removed": diff.removed(), "dropped": diff.dropped(),
        "lines": diff.lines().iter().map(|line| json!({
            "number": line.number(), "text": line.text(),
            "change": match line.change() { Change::Kept => "kept", Change::Added => "added", Change::Removed => "removed" },
        })).collect::<Vec<_>>(),
    })
}

fn count(value: &Value, name: &str) -> Option<usize> {
    usize::try_from(value.get(name)?.as_u64()?).ok()
}

fn read_preview(value: &Value) -> Option<Diff> {
    let rows = value.get("lines")?.as_array()?;
    if rows.len() > Diff::LINES {
        return None;
    }
    let lines = rows
        .iter()
        .map(|row| {
            let text = row.get("text")?.as_str()?;
            if text.chars().count() > Line::TEXT {
                return None;
            }
            let change = match row.get("change")?.as_str()? {
                "kept" => Change::Kept,
                "added" => Change::Added,
                "removed" => Change::Removed,
                _ => return None,
            };
            Some(Line::new(count(row, "number")?, change, text))
        })
        .collect::<Option<Vec<_>>>()?;
    Diff::restored(
        lines,
        count(value, "added")?,
        count(value, "removed")?,
        count(value, "dropped")?,
    )
}

pub(super) fn compacted(compacted: Compacted, pruned: bool) -> String {
    json!({ "run_item": { "version": 2, "body": {
        "kind": "display_compaction", "pruned": pruned,
        "why": match compacted.why { Compacting::Asked => "asked", Compacting::Resumed => "resumed", Compacting::Full => "full", Compacting::Refused => "refused" },
        "replaced": compacted.replaced, "before": compacted.before, "after": compacted.after, "kept": compacted.kept,
    }}}).to_string()
}

fn read_compacted(value: &Value) -> Option<Compacted> {
    Some(Compacted {
        why: match value.get("why")?.as_str()? {
            "asked" => Compacting::Asked,
            "resumed" => Compacting::Resumed,
            "full" => Compacting::Full,
            "refused" => Compacting::Refused,
            _ => return None,
        },
        replaced: count(value, "replaced")?,
        before: value.get("before")?.as_u64()?,
        after: value.get("after")?.as_u64()?,
        kept: count(value, "kept")?,
    })
}

/// One thing an installed agent did during a turn, kept so that a session
/// picked up again draws it where the turn drew it.
///
/// For the reader alone, as a preview is: the model's transcript holds the
/// turn's prompt and answer, and nothing here is ever sent anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStep {
    /// How many bytes of the turn's answer had been said when it happened,
    /// which is where among the words it goes back.
    pub at: usize,
    /// How many round trips the turn had made by then. A run of lookups is
    /// counted on one line within a round and no further, as it was live.
    pub round: u32,
    /// What it was.
    pub did: AgentDid,
}

/// What an installed agent did, by its own account.
///
/// The three things the turn drew a row for. Every text is the agent's,
/// bounded and redacted before the turn was told it.
#[derive(Clone, PartialEq, Eq)]
pub enum AgentDid {
    /// It began one of its own calls.
    Called {
        /// The agent's identifier for the call.
        call: ToolId,
        /// Which tool, as the agent named it.
        tool: Box<str>,
        /// What the call was about.
        summary: Summary,
        /// What kind of looking-around it was, where that was all it was.
        looking: Option<Looking>,
    },
    /// One of its calls came back.
    Answered {
        /// Which call.
        call: ToolId,
        /// What it said the call came back with.
        output: RecordedToolOutput,
        /// What it said the call did to a file.
        diff: Option<Diff>,
    },
    /// It said it did something it named no call for.
    Reported {
        /// Which tool, as the agent named it.
        tool: Box<str>,
        /// What it did with it.
        summary: Box<str>,
    },
}

impl fmt::Debug for AgentDid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Called { call, looking, .. } => f
                .debug_struct("Called")
                .field("call", call)
                .field("looking", looking)
                .finish_non_exhaustive(),
            Self::Answered { call, output, .. } => f
                .debug_struct("Answered")
                .field("call", call)
                .field("output", output)
                .finish_non_exhaustive(),
            Self::Reported { .. } => f.debug_struct("Reported").finish_non_exhaustive(),
        }
    }
}

pub(super) fn agent_step(step: &AgentStep) -> String {
    let did = match &step.did {
        AgentDid::Called {
            call,
            tool,
            summary,
            looking,
        } => {
            let mut called = json!({
                "id": call.as_str(), "tool": &**tool, "summary": summary.as_str(),
                "about": match summary.argument() {
                    Argument::Path => "path", Argument::Address => "address",
                    Argument::Command => "command", Argument::Other => "other",
                },
            });
            if let (Some(looking), Value::Object(called)) = (looking, &mut called) {
                called.insert("looking".to_owned(), json!(looked(*looking)));
            }
            json!({ "called": called })
        }
        AgentDid::Answered { call, output, diff } => {
            let mut answered = wire::answered(&ToolResult {
                id: call.clone(),
                output: output.clone(),
            });
            if let (Some(diff), Value::Object(answered)) = (diff, &mut answered) {
                answered.insert("diff".to_owned(), preview(diff));
            }
            json!({ "answered": answered })
        }
        AgentDid::Reported { tool, summary } => {
            json!({ "reported": { "tool": &**tool, "summary": &**summary } })
        }
    };
    json!({ "run_item": { "version": 2, "body": {
        "kind": "agent_step", "at": step.at, "round": step.round, "did": did,
    }}})
    .to_string()
}

const fn looked(looking: Looking) -> &'static str {
    match looking {
        Looking::Pattern => "pattern",
        Looking::File => "file",
        Looking::Directory => "directory",
        Looking::Command => "command",
        Looking::WebSearch => "web_search",
        Looking::WebPage => "web_page",
    }
}

fn read_step(body: &Value) -> Option<AgentStep> {
    let did = body.get("did")?;
    let did = if let Some(called) = did.get("called") {
        AgentDid::Called {
            call: ToolId::new(called.get("id")?.as_str()?),
            tool: called.get("tool")?.as_str()?.into(),
            summary: Summary::of(
                match called.get("about")?.as_str()? {
                    "path" => Argument::Path,
                    "address" => Argument::Address,
                    "command" => Argument::Command,
                    "other" => Argument::Other,
                    _ => return None,
                },
                called.get("summary")?.as_str()?,
            ),
            looking: match called.get("looking") {
                Some(looking) => Some(match looking.as_str()? {
                    "pattern" => Looking::Pattern,
                    "file" => Looking::File,
                    "directory" => Looking::Directory,
                    "command" => Looking::Command,
                    "web_search" => Looking::WebSearch,
                    "web_page" => Looking::WebPage,
                    _ => return None,
                }),
                None => None,
            },
        }
    } else if let Some(answered) = did.get("answered") {
        let result = wire::result(answered)?;
        AgentDid::Answered {
            call: result.id,
            output: result.output,
            diff: match answered.get("diff") {
                Some(diff) => Some(read_preview(diff)?),
                None => None,
            },
        }
    } else {
        let reported = did.get("reported")?;
        AgentDid::Reported {
            tool: reported.get("tool")?.as_str()?.into(),
            summary: reported.get("summary")?.as_str()?.into(),
        }
    };
    Some(AgentStep {
        at: count(body, "at")?,
        round: u32::try_from(body.get("round")?.as_u64()?).ok()?,
        did,
    })
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid private session display record",
    )
}

#[cfg(test)]
mod tests;
