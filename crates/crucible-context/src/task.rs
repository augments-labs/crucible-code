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
//! An answer is delivered in words that quote the model's question and the
//! answers it offered beside the person's reply, so it is kept as an answer,
//! never as the request, and its restatement says that only the reply is the
//! person's. A restatement never claims more than it holds: a request that was
//! not kept is said to be missing, and a later instruction left out, by the
//! bound or because a recorded entry could not be read, is counted as not
//! shown.
//!
//! The section's recorded state is the task itself, so a session picked up
//! from its log has the person's words back without the messages that said
//! them. A log that holds what was said but no task, because it ended before
//! the task was recorded or was written before tasks were kept, gives back a
//! task whose request was not kept ([`Task::unkept`]): what is said next is a
//! later instruction, never taken for the request, and the request is said to
//! be missing.
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

/// Said before an answer: the words it quotes are not all the person's.
const ANSWER: &str = "Their answer to a question the model asked; the question and the \
answers it offered are the model's words, and only the reply is theirs:";

/// The person's request and every later instruction, as they said them.
///
/// Its `Debug` is redacted: these are the person's words, and a log line or a
/// panic message is no place to repeat them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Task {
    request: Option<Kept>,
    /// Whether a request was said that this task does not hold, so the next
    /// words are never taken for it.
    lost: bool,
    amendments: VecDeque<Amendment>,
    dropped: u64,
}

/// What the person said after the request, and how they said it.
#[derive(Clone, PartialEq, Eq)]
struct Amendment {
    words: Kept,
    origin: Origin,
}

/// How a later instruction reached the conversation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Typed by the person, as a prompt or a line steered into a turn.
    Instruction,
    /// An answer to a question the model asked: its words quote the
    /// model's question beside the person's reply.
    Answer,
}

impl Amendment {
    fn state(&self) -> Value {
        let mut state = self.words.state();
        if self.origin == Origin::Answer
            && let Value::Object(fields) = &mut state
        {
            fields.insert("answer".into(), Value::Bool(true));
        }
        state
    }

    fn restored(state: &Value) -> Option<Self> {
        let words = Kept::restored(state, AMENDMENT_BYTES)?;
        let origin = if state.get("answer").and_then(Value::as_bool) == Some(true) {
            Origin::Answer
        } else {
            Origin::Instruction
        };
        Some(Self { words, origin })
    }
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
    /// Adds what the person typed: the request when nothing is kept yet,
    /// otherwise a later instruction. Empty words add nothing.
    pub fn said(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.is_empty() {
            self.request = Some(Kept::within(text, REQUEST_BYTES));
            return;
        }
        self.amend(text, Origin::Instruction);
    }

    /// Adds the words an answer to a question the model asked was delivered
    /// in. They quote the model's question, so they are never the request,
    /// and the restatement says which part is the person's.
    pub fn answered(&mut self, text: &str) {
        if !text.is_empty() {
            self.amend(text, Origin::Answer);
        }
    }

    fn amend(&mut self, text: &str, origin: Origin) {
        self.amendments.push_back(Amendment {
            words: Kept::within(text, AMENDMENT_BYTES),
            origin,
        });
        while self.amendments.len() > MOST_AMENDMENTS {
            self.amendments.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// A task whose request was said but not kept: for a session whose log
    /// holds the person's words and no task. Nothing said after it becomes
    /// the request, and its restatement says the request is missing.
    #[must_use]
    pub fn unkept() -> Self {
        Self {
            lost: true,
            ..Self::default()
        }
    }

    /// Whether the person has said anything yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.request.is_none() && !self.lost && self.amendments.is_empty() && self.dropped == 0
    }

    /// The task a recorded state holds, with every bound applied again.
    ///
    /// Lenient: a member it cannot read is left out rather than failing the
    /// pass, because the task is a reminder of what was said and the
    /// transcript, not this state, is the record of it. A later instruction
    /// left out is counted with those not shown, so the restatement never
    /// claims to be whole when it is not.
    #[must_use]
    pub fn restored(state: &Value) -> Self {
        let mut task = Self {
            request: state
                .get("request")
                .and_then(|request| Kept::restored(request, REQUEST_BYTES))
                .filter(|request| !request.text.is_empty()),
            lost: state.get("lost").and_then(Value::as_bool).unwrap_or(false),
            ..Self::default()
        };
        let entries = state
            .get("amendments")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let recorded: Vec<Amendment> = entries.iter().filter_map(Amendment::restored).collect();
        let unread = entries.len() - recorded.len();
        let over = recorded.len().saturating_sub(MOST_AMENDMENTS);
        task.amendments = recorded.into_iter().skip(over).collect();
        task.dropped = state
            .get("dropped")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(u64::try_from(over + unread).unwrap_or(u64::MAX));
        task
    }

    /// The recorded form: never JSON `null`, anywhere in it.
    fn state(&self) -> Value {
        let mut state = Map::new();
        if let Some(request) = &self.request {
            state.insert("request".into(), request.state());
        }
        if self.lost {
            state.insert("lost".into(), Value::Bool(true));
        }
        state.insert(
            "amendments".into(),
            Value::Array(self.amendments.iter().map(Amendment::state).collect()),
        );
        state.insert("dropped".into(), json!(self.dropped));
        Value::Object(state)
    }

    /// The person's words, whole and in order, for a model that no longer
    /// has the messages that said them.
    fn restatement(&self) -> String {
        let mut text = String::from(
            "The person's words in this conversation follow, verbatim and oldest first; \
an answer says which of its words are the model's. Each still stands until the person \
withdraws it.\n\nTheir request:\n",
        );
        match &self.request {
            Some(request) => quote(&mut text, request, REQUEST_BYTES),
            None => text.push_str("(Their request was not kept.)\n"),
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
            if amendment.origin == Origin::Answer {
                text.push_str(ANSWER);
                text.push('\n');
            }
            quote(&mut text, &amendment.words, AMENDMENT_BYTES);
        }
        text
    }
}

/// Appends `kept` as a quotation, saying when it was cut.
///
/// Every line break Unicode names starts a quoted line, not only the two
/// [`str::lines`] knows: what is quoted can be the model's, and a break the
/// quotation missed would let it draw a line of its own outside the quote.
fn quote(text: &mut String, kept: &Kept, bound: usize) {
    let words = kept.text.replace("\r\n", "\n");
    let breaks = |c| {
        matches!(
            c,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
    };
    for line in words.split_terminator(breaks) {
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
            Seen::Known(told) if !Task::restored(told).is_empty() => return None,
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
    fn the_first_words_after_a_recorded_empty_task_are_said_to_stand() {
        let told = Task::default().state();
        let mut task = Task::default();
        task.said("make the parser streaming");

        let section = TaskSection::new(Some(&task), None);

        let fresh = section
            .render(Seen::Known(&told))
            .expect("new words are told");
        assert_eq!(fresh.text(), STANDING);
    }

    #[test]
    fn every_line_break_starts_a_quoted_line() {
        let mut task = Task::default();
        task.said("a\rb\u{85}c\u{2028}d\u{2029}e\u{b}f\u{c}g\r\nh\ni");

        let told = restated(&task);

        assert!(
            told.contains("> a\n> b\n> c\n> d\n> e\n> f\n> g\n> h\n> i\n"),
            "{told:?}"
        );
        for forged in ['\r', '\u{85}', '\u{2028}', '\u{2029}', '\u{b}', '\u{c}'] {
            assert!(!told.contains(forged), "{forged:?} in {told:?}");
        }
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
    fn an_answer_says_the_question_is_the_models_and_only_the_reply_is_the_persons() {
        let mut task = Task::default();
        task.said("add a cache");
        task.answered("The person answered question batch q1:\n- Storage: SQLite\n");
        task.said("go on");

        let told = restated(&task);

        let marked = "the question and the answers it offered are the model's words";
        assert_eq!(told.matches(marked).count(), 1, "{told:?}");
        let request = told.find("> add a cache").expect("the request");
        let answer = told.find(marked).expect("the answer is marked");
        let reply = told.find("> - Storage: SQLite").expect("the reply");
        let next = told.find("> go on").expect("the next instruction");
        assert!(
            request < answer && answer < reply && reply < next,
            "{told:?}"
        );
        let recorded = TaskSection::new(Some(&task), None).snapshot();
        assert_eq!(Task::restored(&recorded), task);
    }

    #[test]
    fn later_words_with_no_request_kept_are_told_and_say_the_request_was_not() {
        let mut task = Task::restored(&serde_json::json!({ "amendments": [{ "text": "b" }] }));
        task.said("c");

        let told = restated(&task);

        assert!(told.contains("(Their request was not kept.)"), "{told:?}");
        let earlier = told.find("> b").expect("the recorded instruction");
        let later = told.find("> c").expect("the instruction after it");
        assert!(earlier < later, "{told:?}");
    }

    #[test]
    fn a_request_said_but_not_kept_stays_missing_once_read_back() {
        let unkept = Task::unkept();
        assert!(!unkept.is_empty());

        let mut task = Task::restored(&TaskSection::new(Some(&unkept), None).snapshot());
        assert_eq!(task, unkept);
        task.said("go on");

        let told = restated(&task);
        assert!(
            told.contains("Their request:\n(Their request was not kept.)\n"),
            "{told:?}"
        );
        assert!(told.contains("What they said after it"), "{told:?}");
        assert!(told.contains("> go on\n"), "{told:?}");
    }

    #[test]
    fn an_entry_the_record_cannot_read_is_counted_as_not_shown() {
        let recorded = serde_json::json!({
            "request": { "text": "the request" },
            "amendments": [{ "text": "kept" }, { "txt": 1 }, 7],
            "dropped": 0,
        });

        let told = restated(&Task::restored(&recorded));

        assert!(
            told.contains("(2 earlier instructions are not shown.)"),
            "{told:?}"
        );
        assert!(told.contains("> kept\n"));
    }

    #[test]
    fn an_empty_recorded_request_is_no_request() {
        let mut task = Task::restored(&serde_json::json!({ "request": { "text": "" } }));
        assert!(task.is_empty());

        task.said("the real request");

        assert!(
            restated(&task).contains("Their request:\n> the real request\n"),
            "{:?}",
            restated(&task)
        );
    }

    #[test]
    fn empty_words_add_nothing_and_a_request_at_its_bound_is_kept_whole() {
        let mut task = Task::default();
        task.said("");
        assert!(task.is_empty());

        task.said(&"a".repeat(REQUEST_BYTES));

        let told = restated(&task);
        assert!(told.contains(&format!("> {}\n", "a".repeat(REQUEST_BYTES))));
        assert!(!told.contains("Incomplete"), "{told:?}");
    }

    #[test]
    fn the_task_debug_shows_none_of_the_persons_words() {
        let mut task = Task::default();
        task.said("a secret plan");
        assert!(!format!("{task:?}").contains("secret"));
        assert!(!format!("{:?}", TaskSection::new(Some(&task), None)).contains("secret"));
    }
}
