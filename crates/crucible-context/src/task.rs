//! What the person asked for, kept so it outlives the messages that said it.
//!
//! A compaction replaces the history with a summary the model wrote, and the
//! person's own words go with it: the first request, every instruction typed
//! while a turn ran and every answer they gave to a question the model left
//! waiting. [`Task`] keeps those words, bounded, and [`TaskSection`] gives them
//! back. While the messages that said them are still in the transcript the
//! section adds no copy: it says once that they stand, and a later instruction
//! changes nothing that sentence says. Once a compaction has taken them, or the
//! session cannot tell what the model was told, it restates them whole and
//! verbatim, in the order they were said, as the person's.
//!
//! The section's recorded state is the task itself, so a session picked up
//! from its log has the person's words back without the messages that said
//! them.
//!
//! The task belongs to the conversation the person is in. A run beneath it,
//! such as a subagent's, neither reads it nor adds to it: what a parent tells
//! a child is the parent's words, not the person's.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};

use serde_json::{Map, Value, json};

use crucible_types::{Fragment, Seen};

use crate::ContextSection;

/// The most of the first request kept, in bytes.
pub const REQUEST_BYTES: usize = 16 * 1024;

/// The most of each later instruction kept, in bytes.
pub const AMENDMENT_BYTES: usize = 4 * 1024;

/// The most later instructions kept; the oldest goes first, and is counted.
pub const MOST_AMENDMENTS: usize = 16;

/// Said once a request is known and its messages are still in the transcript.
const STANDING: &str = "The person's request in this conversation, and every instruction \
they give after it, stands until they withdraw it.";

/// The person's request and every later instruction, as they said them.
///
/// Its `Debug` is redacted: these are the person's words, and a log line or a
/// panic message is no place to repeat them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Task {
    request: Option<Kept>,
    amendments: VecDeque<Kept>,
    dropped: u64,
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Task([redacted])")
    }
}

/// One thing the person said, cut to its bound.
#[derive(Clone, PartialEq, Eq)]
struct Kept {
    text: String,
    incomplete: bool,
}

impl Kept {
    /// `text` cut at the last character boundary at or before `bound` bytes.
    fn within(text: &str, bound: usize) -> Self {
        if text.len() <= bound {
            return Self {
                text: text.to_owned(),
                incomplete: false,
            };
        }
        let end = (0..=bound)
            .rev()
            .find(|&at| text.is_char_boundary(at))
            .unwrap_or_default();
        Self {
            text: text.get(..end).unwrap_or_default().to_owned(),
            incomplete: true,
        }
    }

    fn state(&self) -> Value {
        json!({ "text": self.text, "incomplete": self.incomplete })
    }

    /// One recorded entry, bounded again: a log is read back as found.
    fn restored(state: &Value, bound: usize) -> Option<Self> {
        let text = state.get("text")?.as_str()?;
        let mut kept = Self::within(text, bound);
        kept.incomplete |= state
            .get("incomplete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Some(kept)
    }
}

impl Task {
    /// Adds what the person said: the request when nothing is kept yet,
    /// otherwise a later instruction. Empty words add nothing.
    pub fn said(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.request.is_none() {
            self.request = Some(Kept::within(text, REQUEST_BYTES));
            return;
        }
        self.amendments
            .push_back(Kept::within(text, AMENDMENT_BYTES));
        while self.amendments.len() > MOST_AMENDMENTS {
            self.amendments.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Whether the person has said anything yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.request.is_none()
    }

    /// The task a recorded state holds, with every bound applied again.
    ///
    /// Lenient: a member it cannot read is left out rather than failing the
    /// pass, because the task is a reminder of what was said and the
    /// transcript, not this state, is the record of it.
    #[must_use]
    pub fn restored(state: &Value) -> Self {
        let mut task = Self {
            request: state
                .get("request")
                .and_then(|request| Kept::restored(request, REQUEST_BYTES)),
            ..Self::default()
        };
        let recorded: Vec<Kept> = state
            .get("amendments")
            .and_then(Value::as_array)
            .map(|amendments| {
                amendments
                    .iter()
                    .filter_map(|amendment| Kept::restored(amendment, AMENDMENT_BYTES))
                    .collect()
            })
            .unwrap_or_default();
        let over = recorded.len().saturating_sub(MOST_AMENDMENTS);
        task.amendments = recorded.into_iter().skip(over).collect();
        task.dropped = state
            .get("dropped")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(u64::try_from(over).unwrap_or(u64::MAX));
        task
    }

    /// The recorded form: never JSON `null`, anywhere in it.
    fn state(&self) -> Value {
        let mut state = Map::new();
        if let Some(request) = &self.request {
            state.insert("request".into(), request.state());
        }
        state.insert(
            "amendments".into(),
            Value::Array(self.amendments.iter().map(Kept::state).collect()),
        );
        state.insert("dropped".into(), json!(self.dropped));
        Value::Object(state)
    }

    /// The person's words, whole and in order, for a model that no longer
    /// has the messages that said them.
    fn restatement(&self) -> String {
        let mut text = String::from(
            "The person's own words in this conversation follow, verbatim and oldest \
first. Each still stands until the person withdraws it.\n\nTheir request:\n",
        );
        if let Some(request) = &self.request {
            quote(&mut text, request, REQUEST_BYTES);
        }
        if self.dropped > 0 || !self.amendments.is_empty() {
            text.push_str("\nWhat they said after it, oldest first:\n");
        }
        if self.dropped > 0 {
            let _ = writeln!(
                text,
                "({} earlier instruction{} not shown.)",
                self.dropped,
                if self.dropped == 1 { " is" } else { "s are" },
            );
        }
        for amendment in &self.amendments {
            text.push('\n');
            quote(&mut text, amendment, AMENDMENT_BYTES);
        }
        text
    }
}

/// Appends `kept` as a quotation, saying when it was cut.
fn quote(text: &mut String, kept: &Kept, bound: usize) {
    for line in kept.text.lines() {
        text.push_str("> ");
        text.push_str(line);
        text.push('\n');
    }
    if kept.incomplete {
        let _ = writeln!(
            text,
            "(Incomplete: only the first {} KiB of this was kept.)",
            bound / 1024,
        );
    }
}

/// The person's task, as a context section.
pub struct TaskSection<'a> {
    task: Option<&'a Task>,
    state: Value,
}

impl fmt::Debug for TaskSection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TaskSection([redacted])")
    }
}

impl<'a> TaskSection<'a> {
    /// The section for `task`, or for a run beneath the conversation when
    /// `task` is `None`: that run keeps `recorded` as it found it and is told
    /// nothing.
    #[must_use]
    pub fn new(task: Option<&'a Task>, recorded: Option<&'a Value>) -> Self {
        let state = match task {
            Some(task) => task.state(),
            None => recorded.cloned().unwrap_or_else(|| Task::default().state()),
        };
        Self { task, state }
    }
}

impl ContextSection for TaskSection<'_> {
    const ID: &'static str = "task";

    fn snapshot(&self) -> Value {
        self.state.clone()
    }

    fn render(&self, prior: Seen<&Value>) -> Option<Fragment> {
        let task = self.task?;
        if task.is_empty() {
            return None;
        }
        let text = match prior {
            // The model holds the standing sentence or a restatement, and
            // the messages since; a new instruction is one of those messages.
            Seen::Known(told) if told.get("request").is_some() => return None,
            Seen::Known(_) | Seen::Fresh => STANDING.to_owned(),
            Seen::Stale => task.restatement(),
            Seen::Unknown => format!(
                "This {id} context supersedes every earlier {id} context fragment.\n\n{}",
                task.restatement(),
                id = Self::ID,
            ),
        };
        Some(Fragment::new(Self::ID, text))
    }

    fn recognizes(&self, fragment: &Fragment) -> bool {
        fragment.section() == Self::ID
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crucible_types::Seen;

    use super::{AMENDMENT_BYTES, MOST_AMENDMENTS, REQUEST_BYTES, STANDING, Task, TaskSection};
    use crate::ContextSection;

    /// What the section says to a model that no longer holds the task's words.
    fn restated(task: &Task) -> String {
        TaskSection::new(Some(task), None)
            .render(Seen::Stale)
            .expect("a known task is restated")
            .text()
            .to_owned()
    }

    #[test]
    fn a_request_over_its_bound_is_restated_to_the_bound_and_labelled_incomplete() {
        // Two bytes a character, so the bound falls on a boundary only by
        // being looked for.
        let mut task = Task::default();
        task.said(&format!("x{}", "é".repeat(REQUEST_BYTES)));

        let told = restated(&task);

        assert_eq!(told.matches('é').count(), (REQUEST_BYTES - 1) / 2);
        assert!(
            told.contains("(Incomplete: only the first 16 KiB of this was kept.)"),
            "{told:?}"
        );
    }

    #[test]
    fn an_amendment_over_its_bound_is_labelled_incomplete() {
        let mut task = Task::default();
        task.said("the request");
        task.said(&"ж".repeat(AMENDMENT_BYTES));

        let told = restated(&task);

        assert_eq!(told.matches('ж').count(), AMENDMENT_BYTES / 2);
        assert!(told.contains("(Incomplete: only the first 4 KiB of this was kept.)"));
    }

    #[test]
    fn a_seventeenth_amendment_drops_the_oldest_and_the_restatement_says_so() {
        let mut task = Task::default();
        task.said("the request");
        for said in 1..=MOST_AMENDMENTS + 1 {
            task.said(&format!("amendment {said}"));
        }

        let told = restated(&task);

        assert!(
            told.contains("(1 earlier instruction is not shown.)"),
            "{told:?}"
        );
        assert!(!told.contains("> amendment 1\n"));
        assert!(told.contains("> amendment 2\n"));
        assert!(told.contains("> amendment 17\n"));
        assert!(told.contains("> the request\n"));
    }

    #[test]
    fn a_task_read_back_from_its_recorded_state_is_the_task_recorded() {
        let mut task = Task::default();
        task.said(&"r".repeat(REQUEST_BYTES + 1));
        for said in 0..MOST_AMENDMENTS + 3 {
            task.said(&format!("amendment {said}"));
        }

        let recorded = TaskSection::new(Some(&task), None).snapshot();

        assert_eq!(Task::restored(&recorded), task);
    }

    #[test]
    fn a_recorded_state_over_the_bounds_is_bounded_again_when_read_back() {
        let amendments: Vec<Value> = (0..MOST_AMENDMENTS + 2)
            .map(|said| serde_json::json!({ "text": format!("amendment {said}") }))
            .collect();
        let recorded = serde_json::json!({
            "request": { "text": "ж".repeat(REQUEST_BYTES) },
            "amendments": amendments,
            "dropped": 3,
        });

        let told = restated(&Task::restored(&recorded));

        assert_eq!(told.matches('ж').count(), REQUEST_BYTES / 2);
        assert!(told.contains("(Incomplete: only the first 16 KiB of this was kept.)"));
        assert!(
            told.contains("(5 earlier instructions are not shown.)"),
            "{told:?}"
        );
    }

    #[test]
    fn the_words_stand_without_a_copy_while_the_model_holds_them() {
        let mut task = Task::default();
        task.said("make the parser streaming");
        let section = TaskSection::new(Some(&task), None);
        let fresh = section.render(Seen::Fresh).expect("a new task is told");
        assert_eq!(fresh.text(), STANDING);
        let told = section.snapshot();

        task.said("and add a benchmark");
        let changed = TaskSection::new(Some(&task), None);
        assert_eq!(changed.render(Seen::Known(&told)), None);
    }

    #[test]
    fn nothing_said_and_a_run_beneath_the_conversation_are_told_nothing() {
        let empty = Task::default();
        assert_eq!(
            TaskSection::new(Some(&empty), None).render(Seen::Stale),
            None
        );
        assert_eq!(TaskSection::new(None, None).render(Seen::Unknown), None);
    }

    #[test]
    fn a_run_beneath_the_conversation_keeps_the_recorded_task_as_found() {
        let mut task = Task::default();
        task.said("the request");
        let recorded = TaskSection::new(Some(&task), None).snapshot();

        assert_eq!(TaskSection::new(None, Some(&recorded)).snapshot(), recorded);
    }

    #[test]
    fn an_unknown_state_supersedes_every_earlier_task_fragment() {
        let mut task = Task::default();
        task.said("the request");

        let told = TaskSection::new(Some(&task), None)
            .render(Seen::Unknown)
            .expect("an unknown state is restated");

        assert!(told.text().starts_with("This task context supersedes"));
        assert!(told.text().contains("> the request\n"));
    }

    #[test]
    fn the_task_debug_shows_none_of_the_persons_words() {
        let mut task = Task::default();
        task.said("a secret plan");
        assert!(!format!("{task:?}").contains("secret"));
        assert!(!format!("{:?}", TaskSection::new(Some(&task), None)).contains("secret"));
    }
}
