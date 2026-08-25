//! The plan the agent is working to, and the one tool that writes it.
//!
//! A plan is an ordered list of tasks, each a line of text and one of three
//! states. `todo_write` puts down a whole one, discarding whatever was there.
//! Replacing rather than amending is what keeps the model's idea of the plan
//! and this one the same value: a call that named two tasks to change would be
//! a call about a plan the model cannot see, and the two copies would disagree
//! the first time one of them was wrong.
//!
//! Which is also why there is no tool for reading it back. The call answers
//! with the plan as it now stands, one task per line, so the model reads its
//! own plan on every write and has nothing left to ask for.
//!
//! It touches no file, and that is the whole of its permission story. What it
//! changes is a value inside this process, so there is no path for a rule to
//! be written about — it carries a target that resolves to nothing, no rule
//! matches it, and every mode lets a read through. Nobody is asked, because
//! there is nothing here anybody could be asked about.
//!
//! It is a session's memory, held the way the read record beside it is: one
//! value handed to the tool when it is built, and the binary is the only place
//! that knows the loop drawing the prompt holds the other end of it. A session
//! that is resumed reads its plan back out of the call that wrote it, which is
//! already in the transcript being replayed — so nothing about a session file
//! has to change for a plan to survive one.
//!
//! Both bounds are refusals rather than errors. A plan of ten thousand tasks
//! and a plan with three of them under way are calls the model can correct,
//! and the turn is worth more than the correction costs: the result says which
//! bound was missed and by how much, and the model writes again.

use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

use crucible_core::{
    Approved, Sensitivity, Summary, Target, Tool, ToolArgs, ToolError, ToolOutput, Watch,
};

use crate::args::Args;
use crate::schema::{Field, Schema, Shape};

/// The name the model calls.
const NAME: &str = "todo_write";

/// The field the tasks arrive under, and the two fields one task is written
/// with.
const TASKS: &str = "tasks";
const TASK: &str = "task";
const STATE: &str = "state";

/// The three states, spelled as the model sends them and as it reads them back.
const OPEN: &str = "open";
const DOING: &str = "doing";
const DONE: &str = "done";

/// How many tasks a plan holds.
///
/// Sixty-four is far past the point a plan stops being one. A list this long is
/// the work itself rather than a plan for it, and the panel that draws it shows
/// seven rows — so the figure is not what the reader can take in, it is where a
/// call has plainly stopped meaning to write a plan at all.
const KEPT: usize = 64;

/// How long one task may be, in bytes.
///
/// A task is a line, and this is a generous line. Bytes rather than characters
/// because it bounds what the answer carries and what the next request pays
/// for, and both of those are counted in bytes; what the panel can draw is a
/// question about display width, and it belongs to the code doing the drawing.
const SAID: usize = 256;

/// The root `description` is the tool's own; everything below it describes the
/// arguments. Every ceiling is spelled by the constant the code holds it
/// with, so the sentence the model reads cannot drift from the bound the call
/// meets.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Writes down the plan for the work in hand, replacing it whole. The user reads it \
                as a panel above the prompt, and the call answers with the plan as it now \
                stands. Worth using for work of several steps: write the plan out before \
                starting, and call again as each task changes state."
            .into(),
        fields: vec![Field {
            name: TASKS,
            about: format!(
                "Every task in the plan, in the order they should be read. This replaces the \
                 plan entirely, so a task left out of a call has been removed from the plan. At \
                 most {KEPT} tasks, and at most {SAID} bytes each."
            ),
            needed: true,
            shape: Shape::List {
                of: Box::new(Shape::Fields(vec![
                    Field {
                        name: TASK,
                        about: "What the task is, in one line: Build the gate script.".into(),
                        needed: true,
                        shape: Shape::Text,
                    },
                    Field {
                        name: STATE,
                        about: format!(
                            "Where the task is. {OPEN} is not started, and is what a task with \
                             no state is read as; {DOING} is under way, and at most one task in \
                             a plan may be; {DONE} is finished."
                        ),
                        needed: true,
                        shape: Shape::Choice(&[OPEN, DOING, DONE]),
                    },
                ])),
                fewest: None,
                most: Some(KEPT),
            },
        }],
    }
    .text()
});

/// Where a task is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Not started.
    Open,
    /// The task the plan is on. At most one task in a plan is this.
    Doing,
    /// Finished.
    Done,
}

impl State {
    /// The word the model writes and reads back.
    fn word(self) -> &'static str {
        match self {
            Self::Open => OPEN,
            Self::Doing => DOING,
            Self::Done => DONE,
        }
    }
}

/// One line of a plan.
#[derive(Clone, Eq, PartialEq)]
pub struct Task {
    said: Box<str>,
    state: State,
}

impl Task {
    /// What the task says.
    #[must_use]
    pub fn said(&self) -> &str {
        &self.said
    }

    /// Where it is.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }
}

impl fmt::Debug for Task {
    /// Redacted for the reason [`Summary`] is: this is made out of a call's
    /// arguments, and prose a model wrote about the work in hand quotes what
    /// the work was about as often as not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("said", &"[redacted]")
            .field("state", &self.state)
            .finish()
    }
}

/// The plan this session is working to.
///
/// Cloning shares it rather than copying it, which is the whole point: the tool
/// that writes the plan and the loop that draws it are two objects holding one
/// answer.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    held: Arc<Mutex<Held>>,
}

/// The tasks, and how many times they have been put down.
///
/// The count is under the same lock as the tasks because it is a fact about
/// them: read separately, it could say the plan had moved while the tasks it
/// was read beside were the ones from before.
#[derive(Debug, Default)]
struct Held {
    tasks: Vec<Task>,
    writes: u64,
}

impl Plan {
    /// A plan with nothing in it yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The tasks, in the order they were written.
    ///
    /// A copy, and deliberately: what asks for this is the code that draws the
    /// panel, and holding the lock while a frame is laid out would put the
    /// render path behind a tool call. The bounds above are what make the copy
    /// affordable — sixty-four lines of a couple of hundred bytes, taken when
    /// the plan changes rather than when a frame is drawn.
    #[must_use]
    pub fn tasks(&self) -> Vec<Task> {
        let Ok(held) = self.held.lock() else {
            return Vec::new();
        };

        held.tasks.clone()
    }

    /// How many times this plan has been put down.
    ///
    /// What the loop above the prompt watches. The row it draws is redrawn only
    /// when the picture has changed, and a number that moves on every write is
    /// what says the plan is part of that picture — comparing the tasks
    /// themselves would be sixty-four string comparisons sixty times a second
    /// to learn what one integer says.
    #[must_use]
    pub fn writes(&self) -> u64 {
        let Ok(held) = self.held.lock() else {
            return 0;
        };

        held.writes
    }

    /// Fills the plan from a call this session already made.
    ///
    /// What a resumed session runs over the last `todo_write` in the transcript
    /// it replayed. Reading the call again rather than storing the plan
    /// anywhere is what keeps a session file a record of what happened and
    /// nothing else, and it is the same reading the tool did — so a call the
    /// tool refused is refused again here, rather than seeding a plan that was
    /// never written.
    ///
    /// A call this cannot read leaves the plan alone. The session it belongs to
    /// has already ended and there is nobody left to tell; what a wrong answer
    /// would cost is a panel describing work somebody else did.
    pub fn replay(&self, args: &ToolArgs) {
        let Ok(args) = Args::parse(NAME, args) else {
            return;
        };

        if let Ok(Sent::Tasks(tasks)) = sent(&args) {
            self.write(tasks);
        }
    }

    /// Empties it.
    ///
    /// What `/clear` runs. The plan belonged to the session those tasks were
    /// written in, and one that outlived its session would be a panel above the
    /// prompt describing work the agent has no memory of.
    pub fn forget(&self) {
        self.write(Vec::new());
    }

    /// Puts down a whole plan, discarding the one before it.
    ///
    /// A poisoned lock means a thread ended while holding this, which cannot
    /// happen from anything in this module. Doing nothing leaves the plan as it
    /// was and the count where it was, so the panel is not redrawn and the
    /// reader keeps the last plan this actually wrote — which is a plan that
    /// stopped moving rather than a plan that says something untrue.
    fn write(&self, tasks: Vec<Task>) {
        if let Ok(mut held) = self.held.lock() {
            held.tasks = tasks;
            held.writes = held.writes.saturating_add(1);
        }
    }
}

/// Writes down the agent's plan.
#[derive(Debug)]
pub struct TodoWrite {
    plan: Plan,
}

impl TodoWrite {
    /// Writes into `plan`, which the loop drawing the prompt holds the other
    /// end of.
    #[must_use]
    pub fn new(plan: Plan) -> Self {
        Self { plan }
    }
}

impl Tool for TodoWrite {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA.as_str()
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        // Not a file and not a process: what this changes is a value inside
        // this process, and the target that resolves to nothing is the honest
        // answer to what a rule could be written about.
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        tally(args)
    }

    fn run(&self, approved: Approved, _watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;

        match sent(&args)? {
            Sent::Refused(why) => Ok(ToolOutput::failed(why)),
            Sent::Tasks(tasks) => {
                let answer = report(&tasks);
                self.plan.write(tasks);
                Ok(ToolOutput::ok(answer))
            }
        }
    }
}

/// What a call turned out to be.
enum Sent {
    /// The tasks it wrote, in the order it wrote them.
    Tasks(Vec<Task>),
    /// It parsed and does not hold up, in the words the model reads.
    Refused(String),
}

/// Reads the tasks a call wrote.
///
/// # Errors
///
/// [`ToolError::Arguments`] where the call is not the shape this tool takes at
/// all. A plan that is the right shape and past a bound comes back as
/// [`Sent::Refused`], which continues the turn.
fn sent(args: &Args) -> Result<Sent, ToolError> {
    let Some(written) = args.list(TASKS)? else {
        return Err(args.wrong(format!("{TASKS} is required")));
    };

    // Before the tasks are built rather than after, so a call naming ten
    // thousand of them is answered rather than assembled.
    if written.len() > KEPT {
        return Ok(Sent::Refused(format!(
            "a plan holds at most {KEPT} tasks and this one has {}: \
             write down the work rather than every step of it",
            written.len()
        )));
    }

    let mut tasks = Vec::with_capacity(written.len());
    for one in &written {
        tasks.push(Task {
            said: one.text(TASK)?.into(),
            state: state(one)?,
        });
    }

    Ok(match refused(&tasks) {
        Some(why) => Sent::Refused(why),
        None => Sent::Tasks(tasks),
    })
}

/// Where one task says it is.
///
/// Absent reads as open. The schema asks for the field and a call that left it
/// out is a call about a task nobody has started, which is the one state that
/// can be assumed without claiming anything.
fn state(one: &Args) -> Result<State, ToolError> {
    Ok(match one.choice(STATE, OPEN, &[OPEN, DOING, DONE])? {
        DOING => State::Doing,
        DONE => State::Done,
        _ => State::Open,
    })
}

/// Why a plan of these tasks cannot be written, in the words the model reads.
fn refused(tasks: &[Task]) -> Option<String> {
    let long = tasks
        .iter()
        .enumerate()
        .find(|(_, task)| task.said.len() > SAID);

    if let Some((at, task)) = long {
        return Some(format!(
            "a task is at most {SAID} bytes and {TASKS}[{at}] is {}: one line each",
            task.said.len()
        ));
    }

    let under_way = tasks
        .iter()
        .filter(|task| task.state == State::Doing)
        .count();

    (under_way > 1).then(|| {
        format!(
            "a plan has at most one task under way and this one has {under_way}: \
             leave one doing and mark the rest open or done"
        )
    })
}

/// The plan as it now stands, for the model to read its own plan back.
///
/// The counts first, on a line of their own, because that line is the one the
/// transcript hangs under the call — a reader who is not going to open the
/// result still learns what the plan did.
///
/// Bounded by the figures above rather than by [`crate::bound`]: sixty-four
/// tasks of two hundred and fifty-six bytes is a fifth of what that module
/// allows, and it is bounded before the plan is written rather than after it is
/// read, so there is no answer here that could have been cut.
fn report(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "the plan is empty".to_owned();
    }

    let mut answer = counted(tasks);
    for task in tasks {
        answer.push('\n');
        answer.push_str(task.state.word());
        answer.push_str(": ");
        answer.push_str(&task.said);
    }

    answer
}

/// How many tasks are in each state, in the order somebody reads them: what is
/// behind, what is happening, what is left.
///
/// A state nothing is in is left out. `0 done` is a word and a number spent
/// saying that a plan nobody has finished anything in is a plan nobody has
/// finished anything in.
fn counted(tasks: &[Task]) -> String {
    [State::Done, State::Doing, State::Open]
        .into_iter()
        .filter_map(|state| {
            let count = tasks.iter().filter(|task| task.state == state).count();
            (count > 0).then(|| format!("{count} {}", state.word()))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// How many tasks the call is writing, for the row the transcript shows.
///
/// The count rather than any one task: this call is about the plan as a whole,
/// and the first task in it is not what changed.
fn tally(args: &ToolArgs) -> Summary {
    let counted = Args::parse(NAME, args)
        .ok()
        .and_then(|args| args.list(TASKS).ok().flatten())
        .map(|written| written.len());

    Summary::new(match counted {
        Some(1) => "1 task".to_owned(),
        Some(count) => format!("{count} tasks"),
        None => String::new(),
    })
}

#[cfg(test)]
mod tests {
    use crucible_core::Unwatched;

    use super::*;
    use crate::sample::allowed;

    /// Runs one call the only way a call can be run.
    fn write(plan: &Plan, args: &str) -> ToolOutput {
        let tool = TodoWrite::new(plan.clone());
        tool.run(allowed(&tool, args), &Unwatched).unwrap()
    }

    /// The arguments a call with these tasks would carry.
    fn call(tasks: &[(&str, &str)]) -> String {
        let written: Vec<String> = tasks
            .iter()
            .map(|(said, state)| format!(r#"{{"task":"{said}","state":"{state}"}}"#))
            .collect();

        format!(r#"{{"tasks":[{}]}}"#, written.join(","))
    }

    /// What a plan says, one task per line, for an assertion that reads like
    /// the panel does.
    fn said(plan: &Plan) -> Vec<String> {
        plan.tasks()
            .iter()
            .map(|task| format!("{} {}", task.state().word(), task.said()))
            .collect()
    }

    #[test]
    fn a_plan_written_is_the_plan_read_back() {
        let plan = Plan::new();

        write(
            &plan,
            &call(&[
                ("Build the gate script", "done"),
                ("Run the validation spikes", "doing"),
                ("Design the architecture", "open"),
            ]),
        );

        assert_eq!(
            said(&plan),
            [
                "done Build the gate script",
                "doing Run the validation spikes",
                "open Design the architecture",
            ]
        );
    }

    #[test]
    fn a_second_call_replaces_the_plan_rather_than_adding_to_it() {
        // The whole reason there is one tool rather than two. A call that
        // amended would be a call about a plan the model cannot see, and the
        // two copies would part company the first time one of them was wrong.
        let plan = Plan::new();
        write(&plan, &call(&[("Build the gate script", "doing")]));

        write(&plan, &call(&[("Design the architecture", "open")]));

        assert_eq!(said(&plan), ["open Design the architecture"]);
    }

    #[test]
    fn two_clones_are_one_plan_rather_than_two() {
        // The reason the plan is shared at all: the tool that writes it is not
        // the object the prompt draws from.
        let draws = Plan::new();
        let writes = draws.clone();

        write(&writes, &call(&[("Build the gate script", "doing")]));

        assert_eq!(said(&draws), ["doing Build the gate script"]);
    }

    #[test]
    fn the_answer_is_the_plan_as_it_now_stands() {
        // There is no tool for reading the plan back, so this is where the
        // model learns what it just wrote.
        let plan = Plan::new();

        let output = write(
            &plan,
            &call(&[
                ("Build the gate script", "done"),
                ("Run the validation spikes", "doing"),
                ("Design the architecture", "open"),
            ]),
        );

        assert_eq!(
            output.text(),
            "1 done · 1 doing · 1 open\n\
             done: Build the gate script\n\
             doing: Run the validation spikes\n\
             open: Design the architecture"
        );
    }

    #[test]
    fn the_counts_leave_out_a_state_nothing_is_in() {
        let plan = Plan::new();

        let output = write(
            &plan,
            &call(&[
                ("Build the gate script", "open"),
                ("Design the architecture", "open"),
            ]),
        );

        assert_eq!(
            output.text().lines().next(),
            Some("2 open"),
            "{}",
            output.text()
        );
    }

    #[test]
    fn a_plan_with_two_tasks_under_way_is_refused_without_ending_the_turn() {
        let plan = Plan::new();
        write(&plan, &call(&[("Build the gate script", "doing")]));

        let output = write(
            &plan,
            &call(&[
                ("Run the validation spikes", "doing"),
                ("Design the architecture", "doing"),
            ]),
        );

        assert!(output.is_failed());
        assert!(
            output.text().starts_with("a plan has at most one task"),
            "{}",
            output.text()
        );
        assert!(output.text().contains("has 2"), "{}", output.text());
        // And the plan is the one that was there, not half of the refused call.
        assert_eq!(said(&plan), ["doing Build the gate script"]);
    }

    #[test]
    fn a_plan_past_the_task_ceiling_is_refused_and_names_the_figure() {
        let plan = Plan::new();
        let many: Vec<(String, &str)> = (0..=KEPT).map(|n| (format!("task {n}"), OPEN)).collect();
        let written: Vec<(&str, &str)> = many
            .iter()
            .map(|(text, state)| (text.as_str(), *state))
            .collect();

        let output = write(&plan, &call(&written));

        assert!(output.is_failed());
        assert_eq!(
            output.text(),
            format!(
                "a plan holds at most {KEPT} tasks and this one has {}: \
                 write down the work rather than every step of it",
                KEPT + 1
            )
        );
        assert!(plan.tasks().is_empty());
    }

    #[test]
    fn a_task_past_the_byte_ceiling_is_refused_and_names_which_one() {
        // Which one is the half a model can act on: a plan of forty tasks with
        // one long line in it is not a plan to rewrite, it is a line to cut.
        let plan = Plan::new();
        let long = "a".repeat(SAID + 1);

        let output = write(&plan, &call(&[("short enough", OPEN), (&long, OPEN)]));

        assert!(output.is_failed());
        assert_eq!(
            output.text(),
            format!(
                "a task is at most {SAID} bytes and tasks[1] is {}: one line each",
                SAID + 1
            )
        );
    }

    #[test]
    fn a_task_with_no_state_is_one_nobody_has_started() {
        let plan = Plan::new();

        write(&plan, r#"{"tasks":[{"task":"Build the gate script"}]}"#);

        assert_eq!(said(&plan), ["open Build the gate script"]);
    }

    #[test]
    fn a_state_nobody_offers_ends_the_turn_and_names_the_words_that_are() {
        // A word outside the set is a call asking for something this tool does
        // not do, and reading it as `open` would be the failure the model
        // cannot see.
        let tool = TodoWrite::new(Plan::new());

        let problem = tool
            .run(
                allowed(
                    &tool,
                    r#"{"tasks":[{"task":"Build it","state":"blocked"}]}"#,
                ),
                &Unwatched,
            )
            .unwrap_err();

        assert_eq!(
            problem.to_string(),
            "todo_write: tasks[0] state must be one of open, doing, done"
        );
    }

    #[test]
    fn a_call_with_no_tasks_at_all_ends_the_turn() {
        let tool = TodoWrite::new(Plan::new());

        let problem = tool.run(allowed(&tool, "{}"), &Unwatched).unwrap_err();

        assert_eq!(problem.to_string(), "todo_write: tasks is required");
    }

    #[test]
    fn an_empty_list_is_how_a_plan_is_put_away() {
        // The agent that has finished is entitled to drop the plan, and a panel
        // with nothing in it is not drawn at all.
        let plan = Plan::new();
        write(&plan, &call(&[("Build the gate script", "done")]));

        let output = write(&plan, r#"{"tasks":[]}"#);

        assert!(!output.is_failed());
        assert_eq!(output.text(), "the plan is empty");
        assert!(plan.tasks().is_empty());
    }

    #[test]
    fn the_row_a_transcript_shows_counts_the_tasks() {
        let tool = TodoWrite::new(Plan::new());

        let one = tool.summary(&ToolArgs::new(call(&[("Build it", OPEN)])));
        let several = tool.summary(&ToolArgs::new(call(&[
            ("Build it", OPEN),
            ("Ship it", OPEN),
        ])));

        assert_eq!(one.as_str(), "1 task");
        assert_eq!(several.as_str(), "2 tasks");
    }

    #[test]
    fn a_call_nobody_can_read_is_summarised_as_nothing() {
        // The tool refuses it a moment later and says why. A row that invented
        // a count for it would be the wrong explanation arriving first.
        let tool = TodoWrite::new(Plan::new());

        for unreadable in ["not json", "{}", r#"{"tasks":7}"#] {
            assert!(
                tool.summary(&ToolArgs::new(unreadable)).is_empty(),
                "{unreadable} was summarised as something"
            );
        }
    }

    #[test]
    fn writing_the_plan_is_what_moves_the_number_the_prompt_watches() {
        // The row above the box redraws only when the picture has changed, so
        // a write the count does not move is a panel that goes stale.
        let plan = Plan::new();
        let before = plan.writes();

        write(&plan, &call(&[("Build the gate script", "doing")]));
        let after = plan.writes();
        plan.forget();

        assert!(after > before, "{before} then {after}");
        assert!(plan.writes() > after);
    }

    #[test]
    fn a_refused_call_moves_nothing_the_prompt_watches() {
        let plan = Plan::new();
        write(&plan, &call(&[("Build the gate script", "doing")]));
        let after = plan.writes();

        write(
            &plan,
            &call(&[("Run the spikes", "doing"), ("Design it", "doing")]),
        );

        assert_eq!(plan.writes(), after);
    }

    #[test]
    fn forgetting_leaves_no_plan() {
        // What `/clear` runs. The session those tasks were written in is over,
        // so the plan is over with it.
        let plan = Plan::new();
        write(&plan, &call(&[("Build the gate script", "doing")]));

        plan.forget();

        assert!(plan.tasks().is_empty());
    }

    #[test]
    fn a_resumed_session_reads_its_plan_back_out_of_the_call_that_wrote_it() {
        // Which is what keeps a session file a record of what happened: the
        // call is already in the transcript being replayed, so nothing about
        // the format has to change for a plan to survive a resume.
        let plan = Plan::new();

        plan.replay(&ToolArgs::new(call(&[
            ("Build the gate script", "done"),
            ("Run the validation spikes", "doing"),
        ])));

        assert_eq!(
            said(&plan),
            [
                "done Build the gate script",
                "doing Run the validation spikes",
            ]
        );
    }

    #[test]
    fn a_recorded_call_the_tool_would_have_refused_is_refused_again() {
        // The same reading, so a plan the tool never wrote is not one a resume
        // can seed. Otherwise the panel would come back describing a call that
        // failed.
        let plan = Plan::new();

        plan.replay(&ToolArgs::new(call(&[
            ("Run the spikes", "doing"),
            ("Design it", "doing"),
        ])));

        assert!(plan.tasks().is_empty());
    }

    #[test]
    fn a_recorded_call_this_build_cannot_read_leaves_the_plan_alone() {
        // A transcript written by another build, and a session that has already
        // ended. There is nobody left to tell, and the plan that is there is
        // the better of the two answers.
        let plan = Plan::new();
        write(&plan, &call(&[("Build the gate script", "doing")]));

        for unreadable in ["", "not json", "{}", r#"{"tasks":[{"task":7}]}"#] {
            plan.replay(&ToolArgs::new(unreadable));
            assert_eq!(said(&plan), ["doing Build the gate script"], "{unreadable}");
        }
    }

    #[test]
    fn the_call_names_nothing_a_rule_could_be_written_about() {
        // No file and no process, so no prompt: the target resolves to nothing,
        // no rule matches it, and every mode lets a read through.
        let tool = TodoWrite::new(Plan::new());

        let sensitivity = tool.sensitivity(&ToolArgs::new(call(&[("Build it", OPEN)])));

        assert_eq!(
            sensitivity,
            Sensitivity::ReadOnly {
                target: Target::unresolved()
            }
        );
    }

    #[test]
    fn the_fullest_plan_answers_well_inside_what_a_tool_may_answer_with() {
        // The bounds are what stand in for `crate::bound` here, so they owe the
        // same promise: no answer this tool produces was ever going to be cut.
        let plan = Plan::new();
        let long: Vec<String> = (0..KEPT).map(|n| format!("{n:0>256}")).collect();
        let written: Vec<(&str, &str)> = long.iter().map(|task| (task.as_str(), OPEN)).collect();

        let output = write(&plan, &call(&written));

        assert!(!output.is_failed(), "{}", output.text());
        assert!(
            output.text().len() < crate::bound::OUTPUT,
            "the fullest plan answered with {} bytes",
            output.text().len()
        );
    }

    #[test]
    fn a_task_never_shows_what_the_model_wrote_when_it_is_printed_for_a_reader() {
        let plan = Plan::new();
        write(&plan, &call(&[("task-debug-canary", DOING)]));

        let shown = format!("{plan:?}");

        assert!(!shown.contains("task-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
