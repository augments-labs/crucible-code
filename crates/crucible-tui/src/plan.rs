//! The plan the agent is working to, drawn above the prompt.
//!
//! A rule with air on both sides of it, a line of counts, a row per task, and a
//! line saying what did not fit. It joins the rows that are redrawn in place
//! rather than being written into scrollback, so a plan rewritten twenty times
//! in one turn costs twenty rewrites of the same rows rather than twenty copies
//! down the transcript.
//!
//! The rule is what says *a different thing starts here*, and a rule with rows
//! hard against it on both sides says it much less well: it reads as an
//! underline for what is above and a lid on what is below, which is the one
//! reading it must not have. So the blank rows are the panel's own, spent
//! before anything else it draws — they are what the rule is made of, rather
//! than padding somebody could take back.
//!
//! Like [`crate::Working`] it returns rows and draws nothing, so what it says is
//! decided with no terminal anywhere near it, and the same plan in the same
//! window gives the same rows every time.
//!
//! The order the tasks are drawn in is the order the bound keeps them in, and
//! that is one decision rather than two: the task under way, then what is open,
//! then what is finished with the most recent of it first. A bounded panel is
//! the first however many of that sequence, so what it drops is the least worth
//! keeping, and opening it adds the rest *underneath* the rows already on
//! screen. Nothing anybody was reading moves, which is what makes the key worth
//! pressing in the middle of a turn.
//!
//! The same prefix answers a window too short to hold the panel. Rows give way
//! from the end — what is finished, then what is open — and a window that
//! cannot hold the rule, the counts and one task is a window with no panel in
//! it at all.
//!
//! A task's text is a model's own words, arriving from a tool call's arguments,
//! and it is drawn as text: [`spoken`] is where the sequences a terminal would
//! obey come out of it.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{self, clip};

/// How many task rows the panel shows before it starts counting what is left.
///
/// The panel is read at a glance, above a prompt somebody is typing into, and a
/// list longer than this is one that is scanned instead. What it costs is
/// nothing that cannot be got back: everything past it is counted on the last
/// line and one key puts it on screen.
const KEPT: usize = 7;

/// The rows the panel spends before it draws a task: a blank, the rule, another
/// blank, and the counts.
///
/// The two blanks are counted here with the rule rather than left to the caller
/// because they are part of what the rule says. A window with no room for them
/// has no room for the panel: it would be drawing the separator without the
/// separation.
const FRAME: usize = 4;

/// The words the three states are counted in. One word per concept, and these
/// are the same three the tool writes into its own answer.
const OPEN: &str = "open";
const DOING: &str = "doing";
const DONE: &str = "done";

/// What the last line offers, in each of the two states it is read in.
const EXPAND: &str = "ctrl+t to expand";
const COLLAPSE: &str = "ctrl+t to collapse";

/// What the count on the last line is of, where all of it is finished work and
/// where it is not.
const COMPLETED: &str = "completed";
const MORE: &str = "more";

/// Where a task is.
///
/// Three, and no fourth. A task the plan has given up on is one the next write
/// leaves out, and a blocked one is one the plan has not reached — both would
/// put dead rows in a picture whose whole virtue is that it is short.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Nobody has started it.
    Open,
    /// Under way. At most one task in a plan is.
    Doing,
    /// Finished with.
    Done,
}

/// One task, as the panel draws it.
#[derive(Clone, Copy, Debug)]
pub struct Task<'a> {
    /// What the task is, in one line.
    pub said: &'a str,
    /// Where it is.
    pub state: State,
}

impl Task<'_> {
    /// The row this task is drawn as, for a terminal `columns` wide.
    ///
    /// The mark says which state this is and the colour says it again, in that
    /// order: a terminal with no colour at all still has three different marks,
    /// and one with colour has the one warm mark on the screen sitting against
    /// the one row in a weight of its own.
    fn row(&self, columns: usize, glyphs: Glyphs) -> Row {
        let (mark, marked, said) = match self.state {
            State::Open => (glyphs.open(), Slot::Quiet, Slot::Plain),
            State::Doing => (glyphs.doing(), Slot::DoingMark, Slot::Doing),
            State::Done => (glyphs.done(), Slot::DoneMark, Slot::Done),
        };

        // The space is the reader's own foreground rather than the text's slot,
        // so the line through a finished task starts where its words do.
        Row::new().then(marked, mark).then(Slot::Plain, " ").then(
            said,
            spoken(self.said, columns.saturating_sub(gutter(glyphs))),
        )
    }
}

/// The plan, and whether the whole of it is being shown.
#[derive(Clone, Copy, Debug)]
pub struct Plan<'a> {
    /// Every task, in the order they were written down.
    pub tasks: &'a [Task<'a>],
    /// Whether the bound is off, which is what the key toggles.
    pub expanded: bool,
}

impl<'a> Plan<'a> {
    /// The panel, drawn for a terminal `columns` wide in `room` rows.
    ///
    /// Never wider than that and never taller: a row past the last column is
    /// one the terminal wraps itself, and a row past the last the caller
    /// offered is one drawn over something else in the live region.
    ///
    /// Empty where there is no plan, and empty where what is left of the window
    /// cannot hold the rule, the counts and a task between them. Nothing is
    /// reserved and nothing says *empty* — a panel with no plan in it is a
    /// picture of an agent that has not started, drawn every time one has not.
    #[must_use]
    pub fn rows(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        let shown = self.shown(room, self.expanded);

        if shown == 0 || columns <= gutter(glyphs) {
            return Vec::new();
        }

        let mut rows = Vec::with_capacity(FRAME.saturating_add(shown).saturating_add(1));

        rows.push(Row::new());
        rows.push(Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(columns)));
        rows.push(Row::new());
        rows.push(self.header(columns, glyphs));

        for task in self.ordered().take(shown) {
            rows.push(task.row(columns, glyphs));
        }
        if let Some(last) = self.last(shown, room, columns, glyphs) {
            rows.push(last);
        }

        rows
    }

    /// How many task rows are drawn, in `room` rows and at that setting of the
    /// key.
    ///
    /// Takes `expanded` rather than reading the field, because the last line
    /// has to ask this same question about the panel *collapsed* in order to
    /// know whether the key it is offering was worth pressing.
    fn shown(&self, room: usize, expanded: bool) -> usize {
        let room = room.saturating_sub(FRAME);
        let bound = if expanded { room } else { room.min(KEPT) };
        let shown = self.tasks.len().min(bound);

        if shown < self.tasks.len() && shown == room {
            // A plan that does not fit owes a line saying so, and that line is
            // a row like any other. Where the window is what bounded it there
            // is nowhere for that row to come from but the tasks: a panel drawn
            // to the last row the caller offered and then given one more is a
            // panel drawn over something else.
            //
            // Where [`KEPT`] bounded it the window has rows to spare, so the
            // line is one of those and every row the bound allowed is a task.
            shown.saturating_sub(1)
        } else {
            shown
        }
    }

    /// Every task, in the order they are drawn and dropped in.
    ///
    /// The task under way first, because *what is the agent on* is the question
    /// the panel exists to answer. Then what is open, in the order the plan
    /// wrote it. Then what is finished, most recent first — a plan is written
    /// in the order it will be worked through, so the last of the done ones is
    /// the one that was just ticked off, and it is the one worth the row.
    fn ordered(&self) -> impl Iterator<Item = &Task<'a>> {
        let under_way = self.tasks.iter().filter(|task| task.state == State::Doing);
        let open = self.tasks.iter().filter(|task| task.state == State::Open);
        let done = self
            .tasks
            .iter()
            .rev()
            .filter(|task| task.state == State::Done);

        under_way.chain(open).chain(done)
    }

    /// The line of counts.
    ///
    /// The total, then what is in each state that anything is in. The figures
    /// take the accent and the words are quiet, so the row reads as numbers
    /// before it reads as a sentence — which is the order somebody glancing at
    /// it wants them in.
    fn header(&self, columns: usize, glyphs: Glyphs) -> Row {
        let total = self.tasks.len().to_string();
        let mut row = Row::new().then(Slot::Accent, clip(&total, columns));

        let word = if self.tasks.len() == 1 {
            " task"
        } else {
            " tasks"
        };
        row.push(
            Slot::Plain,
            clip(word, columns.saturating_sub(row.columns())),
        );

        let tally = self.tally(glyphs);
        let wide: usize = tally.iter().map(|(_, said)| width::columns(said)).sum();

        // All of it or none of it. Half a parenthesis is worse than no counts,
        // and the total in front of them has already said the useful part.
        if wide <= columns.saturating_sub(row.columns()) {
            for (slot, said) in tally {
                row.push(slot, said);
            }
        }

        row
    }

    /// What is in each state, as the spans it is drawn in.
    ///
    /// Empty for a plan that is all in one state, where the parenthesis would
    /// be the total said a second time in smaller words.
    fn tally(&self, glyphs: Glyphs) -> Vec<(Slot, String)> {
        let counted: Vec<(usize, &str)> = [
            (State::Done, DONE),
            (State::Doing, DOING),
            (State::Open, OPEN),
        ]
        .into_iter()
        .map(|(state, word)| (self.count(state), word))
        .filter(|(count, _)| *count > 0)
        .collect();

        if counted.len() < 2 {
            return Vec::new();
        }

        let mut spans = vec![(Slot::Quiet, " (".to_owned())];

        for (at, (count, word)) in counted.into_iter().enumerate() {
            if at > 0 {
                spans.push((Slot::Quiet, format!(" {} ", glyphs.dot())));
            }
            spans.push((Slot::Accent, count.to_string()));
            spans.push((Slot::Quiet, format!(" {word}")));
        }
        spans.push((Slot::Quiet, ")".to_owned()));

        spans
    }

    /// How many tasks are in `state`.
    fn count(&self, state: State) -> usize {
        self.tasks.iter().filter(|task| task.state == state).count()
    }

    /// The line under the tasks, where there is one.
    ///
    /// What did not fit, counted, and the key that changes that. A plan with
    /// nothing left over is offered nothing — there is nothing to open, so the
    /// press does nothing and the row saying otherwise would be a row spent on
    /// an offer that is not real.
    fn last(&self, shown: usize, room: usize, columns: usize, glyphs: Glyphs) -> Option<Row> {
        let hidden = self.tasks.len().saturating_sub(shown);
        let key = if self.expanded { COLLAPSE } else { EXPAND };

        if hidden == 0 {
            // Open, with everything on screen. The key that puts it back is
            // still owed, and only here: a plan that fits either way was never
            // opened, and grows no row for having been asked.
            let holds_back = self.tasks.len() > self.shown(room, false);

            return (self.expanded && holds_back).then(|| line(key, columns));
        }

        let finished = self
            .ordered()
            .skip(shown)
            .all(|task| task.state == State::Done);
        let word = if finished { COMPLETED } else { MORE };

        Some(line(
            &format!(
                "{} +{hidden} {word} {} {key}",
                glyphs.ellipsis(),
                glyphs.dot()
            ),
            columns,
        ))
    }
}

/// How far the words on a task row stand from the left, in columns.
///
/// The mark and the space after it, measured off one of the three marks rather
/// than off the one being drawn: they are a column apiece and a test in
/// [`crate::glyphs`] is what keeps them so, which is what lets three states
/// share one gutter and the rows under each other line up.
fn gutter(glyphs: Glyphs) -> usize {
    width::columns(glyphs.doing()).saturating_add(1)
}

/// A quiet row, no wider than the window.
fn line(said: &str, columns: usize) -> Row {
    Row::new().then(Slot::Quiet, clip(said, columns))
}

/// One row of what a model wrote, with nothing in it a terminal would obey.
///
/// A task's text is made out of a tool call's arguments, which is to say out of
/// whatever the model was reading when it wrote them. [`clip`] measures an
/// escape sequence as the nothing it costs, which is the right answer for the
/// arithmetic and the wrong one for the screen: the bytes are still inside the
/// slice it hands back, and a terminal sent them would move a cursor this
/// process believes it is tracking. So the walk decides where the row ends and
/// this drops what may not be drawn — every character that costs no column,
/// which is a sequence's parameters, the escape that opened it, and any control
/// byte that arrived on its own.
///
/// Nothing here changes the width, because nothing dropped was ever counted.
fn spoken(said: &str, columns: usize) -> String {
    width::spoken(clip(said, columns))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan the design was drawn against: two tasks finished in the order
    /// they were written, one under way, three still to start.
    fn plan() -> Vec<Task<'static>> {
        vec![
            Task {
                said: "Choose the crate split",
                state: State::Done,
            },
            Task {
                said: "Write the contributor guide",
                state: State::Done,
            },
            Task {
                said: "Set up the gate script and wire CI to it",
                state: State::Doing,
            },
            Task {
                said: "Build the provider seam",
                state: State::Open,
            },
            Task {
                said: "Run the validation spikes",
                state: State::Open,
            },
            Task {
                said: "Design the v0.0.1 architecture",
                state: State::Open,
            },
        ]
    }

    /// A plan long enough that [`KEPT`] is what shortens it rather than the
    /// window, with every task it holds back finished work.
    fn long() -> Vec<Task<'static>> {
        let named = [
            ("Write the contributor guide", State::Done),
            ("Choose the crate split", State::Done),
            ("Set up the gate script", State::Doing),
            ("Wire CI to the gate", State::Open),
            ("Run the validation spikes", State::Open),
            ("Design the v0.0.1 architecture", State::Open),
            ("Build the provider seam", State::Open),
            ("Build the tool seam", State::Open),
            ("Draw the prompt", State::Open),
        ];

        named
            .into_iter()
            .map(|(said, state)| Task { said, state })
            .collect()
    }

    /// A plan, bounded.
    fn panel<'a>(tasks: &'a [Task<'a>]) -> Plan<'a> {
        Plan {
            tasks,
            expanded: false,
        }
    }

    /// The same plan, with the key pressed.
    fn opened<'a>(tasks: &'a [Task<'a>]) -> Plan<'a> {
        Plan {
            tasks,
            expanded: true,
        }
    }

    /// What the panel says, one string per row.
    fn drawn(plan: &Plan<'_>, columns: usize, room: usize) -> Vec<String> {
        plan.rows(columns, room, Glyphs::Unicode)
            .iter()
            .map(Row::text)
            .collect()
    }

    /// The slot each run of a row asked for, and what it says.
    fn spans(plan: &Plan<'_>, at: usize) -> Vec<(Slot, String)> {
        plan.rows(72, 20, Glyphs::Unicode)
            .get(at)
            .map(|row| {
                row.spans()
                    .map(|(slot, said)| (slot, said.to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_panel_is_air_a_rule_air_the_counts_and_a_row_a_task() {
        // The whole picture in one assertion, because the order is the point: a
        // rule with room around it says a different thing starts here, the
        // counts say how much of it there is, and the rows say what it is.
        let tasks = plan();

        assert_eq!(
            drawn(&panel(&tasks), 48, 20),
            [
                String::new(),
                "─".repeat(48),
                String::new(),
                "6 tasks (2 done · 1 doing · 3 open)".to_owned(),
                "■ Set up the gate script and wire CI to it".to_owned(),
                "□ Build the provider seam".to_owned(),
                "□ Run the validation spikes".to_owned(),
                "□ Design the v0.0.1 architecture".to_owned(),
                "✓ Write the contributor guide".to_owned(),
                "✓ Choose the crate split".to_owned(),
            ]
        );
    }

    #[test]
    fn the_rule_is_never_drawn_hard_against_what_is_above_or_below_it() {
        // Without the blank either side it reads as an underline for the row
        // over it and a lid on the row under it, which is the one thing it is
        // not saying.
        let tasks = plan();
        let said = drawn(&panel(&tasks), 72, 20);

        assert_eq!(said.first().map(String::as_str), Some(""));
        assert_eq!(said.get(2).map(String::as_str), Some(""));
    }

    #[test]
    fn the_task_under_way_is_drawn_first_and_what_is_finished_last() {
        // Whatever order the plan was written in. The panel answers "what is
        // the agent on" before it answers anything else, and what is finished
        // is the part somebody is reading past.
        let tasks = plan();
        let said = drawn(&panel(&tasks), 72, 20);

        assert_eq!(
            said.get(4).map(String::as_str),
            Some("■ Set up the gate script and wire CI to it")
        );
        assert_eq!(
            said.last().map(String::as_str),
            Some("✓ Choose the crate split")
        );
    }

    #[test]
    fn what_is_finished_is_drawn_with_the_most_recent_of_it_first() {
        // A plan is written in the order it will be worked through, so the last
        // of the done ones was ticked off most recently. It is the one worth a
        // row, which means it is the one drawn where the bound would keep it.
        let tasks = plan();
        let said = drawn(&panel(&tasks), 72, 20);
        let finished: Vec<&String> = said.iter().filter(|row| row.starts_with('✓')).collect();

        assert_eq!(
            finished,
            ["✓ Write the contributor guide", "✓ Choose the crate split"]
        );
    }

    #[test]
    fn the_bound_is_seven_tasks_and_the_line_counting_the_rest_is_an_eighth_row() {
        // The line is not paid for out of the seven. A window with rows to
        // spare has one for it, and taking a task's row instead would drop a
        // task in order to say a task was dropped.
        let tasks = long();
        let said = drawn(&panel(&tasks), 72, 20);

        assert_eq!(said.len(), FRAME + KEPT + 1);
    }

    #[test]
    fn opening_the_panel_adds_rows_under_the_ones_already_on_screen() {
        // The invariant the key rests on: what is drawn bounded is a prefix of
        // what is drawn open, so nothing anybody was reading moves and the
        // press costs them no place. The rows that arrive, arrive underneath.
        let tasks = long();
        let bounded = drawn(&panel(&tasks), 72, 20);
        let opened = drawn(&opened(&tasks), 72, 20);

        // Every row but the last is the same row in the same place; the last is
        // the one line that was always going to change.
        let kept = bounded.len().saturating_sub(1);

        assert_eq!(bounded.get(..kept), opened.get(..kept));
        assert_eq!(
            bounded.last().map(String::as_str),
            Some("… +2 completed · ctrl+t to expand")
        );
        assert_eq!(
            opened.last().map(String::as_str),
            Some("ctrl+t to collapse")
        );
        assert_eq!(opened.len(), FRAME + tasks.len() + 1);
    }

    #[test]
    fn what_did_not_fit_is_called_completed_where_all_of_it_is_finished_work() {
        let tasks = long();
        let bounded = drawn(&panel(&tasks), 72, 20);

        assert_eq!(
            bounded.last().map(String::as_str),
            Some("… +2 completed · ctrl+t to expand")
        );
    }

    #[test]
    fn what_did_not_fit_is_called_more_where_any_of_it_is_still_to_do() {
        // Because "completed" is a promise about what is behind the line, and a
        // reader deciding whether to press the key is deciding on that word.
        let tasks = long();
        let bounded = drawn(&panel(&tasks), 72, FRAME + 5);

        assert_eq!(
            bounded.last().map(String::as_str),
            Some("… +5 more · ctrl+t to expand")
        );
    }

    #[test]
    fn a_plan_with_nothing_left_over_is_offered_no_key_and_draws_no_last_line() {
        // There is nothing to open, so nothing is offered. A row saying
        // otherwise is a row spent on an offer that does nothing.
        let tasks = plan();
        let said = drawn(&panel(&tasks), 72, 20);

        assert_eq!(said.len(), FRAME + 6);
        assert!(!said.iter().any(|row| row.contains("ctrl+t")), "{said:?}");
    }

    #[test]
    fn a_panel_opened_when_it_already_fitted_grows_no_row_for_it() {
        // The press is answered wherever it is pressed, so a plan that never
        // held anything back can arrive here open. It looks exactly as it did.
        let tasks = plan();

        assert_eq!(
            drawn(&opened(&tasks), 72, 20),
            drawn(&panel(&tasks), 72, 20)
        );
    }

    #[test]
    fn an_opened_panel_that_held_something_back_still_offers_the_key_that_shuts_it() {
        let tasks = long();

        assert_eq!(
            drawn(&opened(&tasks), 72, 20).last().map(String::as_str),
            Some("ctrl+t to collapse")
        );
    }

    #[test]
    fn a_plan_nobody_has_written_is_no_panel_at_all() {
        assert!(drawn(&panel(&[]), 72, 20).is_empty());
    }

    #[test]
    fn the_counts_leave_out_a_state_nothing_is_in() {
        let tasks = [
            Task {
                said: "Run validation spikes",
                state: State::Doing,
            },
            Task {
                said: "Build gate script and CI",
                state: State::Open,
            },
        ];

        assert_eq!(
            drawn(&panel(&tasks), 72, 20).get(3).map(String::as_str),
            Some("2 tasks (1 doing · 1 open)")
        );
    }

    #[test]
    fn a_plan_all_in_one_state_says_the_total_once() {
        // `3 tasks (3 open)` is the same figure twice, in a parenthesis that
        // exists to say how the total divides.
        let tasks = [
            Task {
                said: "Build gate script and CI",
                state: State::Open,
            },
            Task {
                said: "Run validation spikes",
                state: State::Open,
            },
        ];

        assert_eq!(
            drawn(&panel(&tasks), 72, 20).get(3).map(String::as_str),
            Some("2 tasks")
        );
    }

    #[test]
    fn one_task_is_a_task() {
        let tasks = [Task {
            said: "Build gate script and CI",
            state: State::Open,
        }];

        assert_eq!(
            drawn(&panel(&tasks), 72, 20).get(3).map(String::as_str),
            Some("1 task")
        );
    }

    #[test]
    fn a_short_window_gives_up_what_is_finished_then_what_is_open_then_the_panel() {
        // The give-way order is the drawing order read backwards, which is what
        // makes it one decision rather than two: the rows worth least are
        // already the rows at the bottom.
        let tasks = plan();
        let panel = panel(&tasks);

        assert_eq!(
            drawn(&panel, 72, FRAME + 3),
            [
                String::new(),
                "─".repeat(72),
                String::new(),
                "6 tasks (2 done · 1 doing · 3 open)".to_owned(),
                "■ Set up the gate script and wire CI to it".to_owned(),
                "□ Build the provider seam".to_owned(),
                "… +4 more · ctrl+t to expand".to_owned(),
            ]
        );

        // And a window with no room for the rule, the counts and a task between
        // them has no panel in it. The rule and the counts go with it: a
        // heading over nothing is two rows saying there is a plan and not what
        // it is.
        assert!(drawn(&panel, 72, FRAME + 1).is_empty());
        assert!(drawn(&panel, 72, 0).is_empty());
    }

    #[test]
    fn an_expansion_that_does_not_fit_is_bounded_by_the_window_and_keeps_its_key() {
        // Opened, the seven comes off and the window is what is left bounding
        // it. The last line is never the row that goes, because it is the one
        // holding the key that puts this back.
        let tasks = long();
        let said = drawn(&opened(&tasks), 72, FRAME + 5);

        assert_eq!(said.len(), FRAME + 5);
        assert_eq!(
            said.last().map(String::as_str),
            Some("… +5 more · ctrl+t to collapse")
        );
    }

    #[test]
    fn the_task_under_way_is_the_only_row_in_a_weight_of_its_own() {
        // And its mark the only warm colour on the screen. That is the answer
        // to "what is the agent on", found without reading -- so a second row
        // in either would be two rows asking to be looked at.
        let tasks = plan();
        let panel = panel(&tasks);

        assert_eq!(
            spans(&panel, 4),
            [
                (Slot::DoingMark, "■".to_owned()),
                (Slot::Plain, " ".to_owned()),
                (
                    Slot::Doing,
                    "Set up the gate script and wire CI to it".to_owned()
                ),
            ]
        );

        let rest = panel.rows(72, 20, Glyphs::Unicode);
        let emphasised = rest
            .iter()
            .flat_map(Row::spans)
            .filter(|(slot, _)| matches!(slot, Slot::Doing | Slot::DoingMark))
            .count();

        assert_eq!(emphasised, 2);
    }

    #[test]
    fn a_task_nobody_has_started_is_a_quiet_mark_and_the_readers_own_words() {
        let tasks = plan();

        assert_eq!(
            spans(&panel(&tasks), 5),
            [
                (Slot::Quiet, "□".to_owned()),
                (Slot::Plain, " ".to_owned()),
                (Slot::Plain, "Build the provider seam".to_owned()),
            ]
        );
    }

    #[test]
    fn what_is_finished_is_struck_through_from_where_its_words_start() {
        // The space after the mark is the reader's own foreground, so the line
        // through a finished task begins at its first letter rather than a
        // column early -- which would read as a mark crossed out.
        let tasks = plan();

        assert_eq!(
            spans(&panel(&tasks), 8),
            [
                (Slot::DoneMark, "✓".to_owned()),
                (Slot::Plain, " ".to_owned()),
                (Slot::Done, "Write the contributor guide".to_owned()),
            ]
        );
    }

    #[test]
    fn the_figures_on_the_line_of_counts_are_what_takes_the_accent() {
        let tasks = plan();
        let accented: Vec<String> = spans(&panel(&tasks), 3)
            .into_iter()
            .filter(|(slot, _)| *slot == Slot::Accent)
            .map(|(_, said)| said)
            .collect();

        assert_eq!(accented, ["6", "2", "1", "3"]);
    }

    #[test]
    fn a_task_never_sends_the_terminal_an_instruction_a_model_wrote() {
        // The text is a model's own words, arriving from a tool call's
        // arguments, which is to say from whatever it was reading. A sequence
        // left in would move a cursor this process believes it is tracking, and
        // the next frame would erase somebody else's rows.
        let tasks = [Task {
            said: "Build \x1b[31mthe\x1b[0m gate\x07 script",
            state: State::Open,
        }];
        let said = drawn(&panel(&tasks), 72, 20);

        assert_eq!(
            said.get(4).map(String::as_str),
            Some("□ Build the gate script")
        );
        assert!(!said.iter().any(|row| row.contains('\x1b')), "{said:?}");
    }

    #[test]
    fn a_task_is_one_row_however_many_the_model_wrote() {
        // A plan is a list of lines and a task that arrived with a second one
        // in it would push every row under it down by one, which is a live
        // region taller than the caller made room for.
        let tasks = [Task {
            said: "Build the gate\nand then run it",
            state: State::Open,
        }];

        assert_eq!(
            drawn(&panel(&tasks), 72, 20).get(4).map(String::as_str),
            Some("□ Build the gate")
        );
    }

    #[test]
    fn nothing_is_ever_drawn_past_the_last_column() {
        let tasks = plan();

        for wide in [0, 1, 2, 3, 8, 20, 40, 71, 72, 200] {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for expanded in [false, true] {
                    let panel = Plan {
                        tasks: &tasks,
                        expanded,
                    };

                    for row in panel.rows(wide, 20, glyphs) {
                        assert!(row.columns() <= wide, "{wide} {glyphs:?}: {:?}", row.text());
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_is_ever_drawn_past_the_last_row_the_caller_offered() {
        let tasks = plan();

        for room in 0..16 {
            for expanded in [false, true] {
                let panel = Plan {
                    tasks: &tasks,
                    expanded,
                };

                assert!(
                    panel.rows(72, room, Glyphs::Unicode).len() <= room,
                    "{room}"
                );
            }
        }
    }

    #[test]
    fn the_rule_reaches_the_last_column_in_both_sets() {
        let tasks = plan();

        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let rows = panel(&tasks).rows(72, 20, glyphs);

            assert_eq!(rows.get(1).map(Row::columns), Some(72), "{glyphs:?}");
        }
    }

    #[test]
    fn every_mark_on_the_panel_comes_out_of_the_glyph_set() {
        // A terminal with a font for neither set is the reason the setting
        // exists, and a mark written here rather than asked for is one that
        // stays a hollow square on it.
        let tasks = plan();
        let said = drawn(&panel(&tasks), 72, FRAME + 3).join("\n");

        for mark in ["─", "■", "□", "…"] {
            assert!(said.contains(mark), "{mark} missing from {said}");
        }

        let ascii = panel(&tasks)
            .rows(72, FRAME + 3, Glyphs::Ascii)
            .iter()
            .map(Row::text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(ascii.is_ascii(), "{ascii}");
    }
}
