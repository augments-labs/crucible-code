//! Streaming the private conversation record for the reader, independently of
//! the compacted transcript sent to a provider. Only one message and one tool
//! batch's bounded previews are retained here; the renderer owns scrollback.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read as _, Take};

use crucible_core::{Change, Compacted, Compacting, Diff, Line, Message, ToolId};
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
    },
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
    queued: Option<(Message, HashMap<ToolId, Diff>)>,
    ended: bool,
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
            ended: false,
        };
        replay::read_record(&mut history.log, &mut history.raw, replay::RECORD_BYTES)?;
        Ok(history)
    }

    fn read_next(&mut self) -> io::Result<Option<DisplayItem>> {
        if self.ready || self.ended {
            if let Some(notice) = self.notices.pop_front() {
                return Ok(Some(notice));
            }
            self.ready = false;
        }
        if let Some((message, previews)) = self.queued.take() {
            return Ok(Some(DisplayItem::Message { message, previews }));
        }
        if self.ended {
            return Ok(None);
        }
        loop {
            self.raw.clear();
            let read = replay::read_record(&mut self.log, &mut self.raw, replay::RECORD_BYTES)?;
            if read == 0 || !self.raw.ends_with(b"\n") {
                self.ended = true;
                return Ok(self.notices.pop_front());
            }
            let text = std::str::from_utf8(&self.raw)
                .map_err(|_| invalid())?
                .trim_end();
            if text.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
            if let Some(body) = value.get("run_item").and_then(|item| item.get("body")) {
                if body.get("kind").and_then(Value::as_str) == Some("display_compaction") {
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
                        if held > crucible_core::MAX_RUN_HISTORY_BYTES {
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
            if let Some(notice) = self.notices.pop_front() {
                self.queued = Some((message, shown));
                self.ready = true;
                return Ok(Some(notice));
            }
            return Ok(Some(DisplayItem::Message {
                message,
                previews: shown,
            }));
        }
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

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid private session display record",
    )
}

#[cfg(test)]
mod tests;
