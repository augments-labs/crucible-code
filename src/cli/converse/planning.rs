//! The plan above the box, read back from the tool that writes it.
//!
//! Two ends of one value. The tool holds a plan and puts the whole list down
//! every time the model sends one; this holds the same plan and reads it back
//! for the rows above the box. Nothing else passes between them — the turn runs
//! on its own thread, so a plan written there is on screen on the next frame
//! this thread draws, with nothing posted through the channel and nothing
//! joined.
//!
//! What is here rather than at either end is the copy. Reading the plan takes a
//! lock and the render path may take none, so it is read when the tool says it
//! has moved, kept, and drawn from until it moves again. The count is what says
//! so, and it is read *before* the tasks: the other order stores a count that
//! belongs to a write whose tasks were not the ones taken, and the panel would
//! then be stale for the rest of the session rather than for one frame.

use crucible_tui::{Glyphs, Row};

/// The plan as this side of the session sees it.
#[derive(Debug)]
pub(super) struct Planning {
    /// The other end of what the tool writes into.
    plan: crucible_tools::Plan,
    /// The tasks as they stood when it last moved.
    tasks: Vec<crucible_tools::Task>,
    /// How many times the plan had been written when those tasks were taken.
    read: u64,
    /// Whether the reader has asked for the whole of it. Owned here rather than
    /// by either loop that draws it, because a plan opened while a turn ran is
    /// still open when the turn ends.
    expanded: bool,
}

impl Planning {
    /// Reads the plan the tools were built with.
    ///
    /// Not empty on a resumed session: the wiring replays the last plan the
    /// transcript holds before this is made, so what the panel opens with is
    /// what the agent was working to when the session stopped.
    pub(super) fn new(plan: crucible_tools::Plan) -> Self {
        let read = plan.writes();
        let tasks = plan.tasks();

        Self {
            plan,
            tasks,
            read,
            expanded: false,
        }
    }

    /// Whether the plan has been written since it was last read, reading it
    /// again where it has.
    ///
    /// What the loop above asks between events, beside the same question about
    /// the row that says the turn is running: a plan is written by a tool call
    /// and nothing on this thread hears about it, so this is the only thing that
    /// notices.
    pub(super) fn moved(&mut self) -> bool {
        let writes = self.plan.writes();
        if writes == self.read {
            return false;
        }

        self.read = writes;
        self.tasks = self.plan.tasks();
        true
    }

    /// Opens the whole plan, or bounds it again, and answers whether there was
    /// a panel for the key to act on.
    ///
    /// One key for both, which is what the panel's own last row says it is. The
    /// answer is what the loop above redraws on: a key pressed against a session
    /// with no plan yet costs the frame nothing, the same as an arrow held down
    /// against the end of a line.
    pub(super) fn expand(&mut self) -> bool {
        self.expanded = !self.expanded;
        !self.tasks.is_empty()
    }

    /// The panel, or nothing where there is no plan or no room for one.
    pub(super) fn rows(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        if self.tasks.is_empty() {
            return Vec::new();
        }

        // Borrowed into the panel's own shape per frame, and it cannot be
        // hoisted: the panel borrows the strings this holds, so a value keeping
        // both would borrow itself. What it costs is a pointer and a state per
        // task against a list the tool bounds at sixty-four, built beside the
        // rows the panel fills either way.
        let mut tasks = Vec::with_capacity(self.tasks.len());
        tasks.extend(self.tasks.iter().map(|task| crucible_tui::Task {
            said: task.said(),
            state: shown(task.state()),
        }));

        crucible_tui::Plan {
            tasks: &tasks,
            expanded: self.expanded,
        }
        .rows(columns, room, glyphs)
    }
}

/// The tool's word for a state, in the panel's own.
///
/// Two enums rather than one because neither crate can name the other's: the
/// panel depends on nothing at all, and the tool depends on `core`. Pairing them
/// is the wiring's job, and this is the whole of it.
fn shown(state: crucible_tools::State) -> crucible_tui::State {
    match state {
        crucible_tools::State::Open => crucible_tui::State::Open,
        crucible_tools::State::Doing => crucible_tui::State::Doing,
        crucible_tools::State::Done => crucible_tui::State::Done,
    }
}

#[cfg(test)]
mod tests;
