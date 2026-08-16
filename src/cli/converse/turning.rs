//! What stands above the box while a turn is running.
//!
//! The component draws a mark, a word and a clock; this is where all three come
//! from. The clock starts as the turn leaves for its own thread and never
//! pauses — not for a permission question, which is time somebody is waiting
//! just as much. The word is read off the events the turn reports as they go
//! past, which is why it is read here rather than by the component: the events
//! are this program's, and the component knows nothing about them.
//!
//! The key is named here for the second time on the screen — the row under the
//! box names it too — and that is deliberate. It is the third segment of this
//! row and the first thing a narrow window drops, and the key that stops a turn
//! is the wrong thing for a narrow window to take away.

use std::time::{Duration, Instant};

use crucible_core::Event;
use crucible_tui::{Glyphs, Row, Working};

/// How the turn is asked to stop, said after the clock.
const STOPS: &str = "esc to interrupt";

/// The rows this puts above the box, blanks included.
const ROWS: usize = 3;

/// The one word for what a turn is doing at this moment.
///
/// Four, because four is what the events can tell apart. A fifth read off the
/// same events would be a word the screen changed to and nothing else did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Doing {
    /// Asked, and nothing has come back yet.
    Thinking,
    /// Prose is arriving.
    Writing,
    /// A tool was asked for and has not answered.
    Running,
    /// Esc has been pressed and the turn is stopping.
    Interrupting,
}

impl Doing {
    /// The word, as the row says it.
    fn word(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::Writing => "writing",
            Self::Running => "running",
            Self::Interrupting => "interrupting",
        }
    }
}

/// The row above the box, and what it last said.
#[derive(Debug)]
pub(super) struct Turning {
    /// When the turn left, which is what the clock counts from.
    since: Instant,
    /// What it is doing now.
    doing: Doing,
    /// The word and the beat the row was last drawn at, so a redraw that would
    /// draw the same row again can be skipped. `None` before the first.
    drawn: Option<(Doing, u64)>,
}

impl Turning {
    /// A turn that starts now.
    pub(super) fn started() -> Self {
        Self {
            since: Instant::now(),
            doing: Doing::Thinking,
            drawn: None,
        }
    }

    /// Takes the word from one event on its way to the screen.
    ///
    /// Every variant is named rather than caught by a rest arm: an event added
    /// later either changes what the turn is doing or does not, and that is a
    /// decision to make here rather than one to inherit.
    pub(super) fn saw(&mut self, event: &Event) {
        // A turn that has been asked to stop is stopping whatever else it is
        // still reporting. The deltas already in flight arrive after the key,
        // and a row that went back to `writing` would be saying the key missed.
        if self.doing == Doing::Interrupting {
            return;
        }

        self.doing = match event {
            Event::Delta { .. } => Doing::Writing,
            Event::ToolRequested { .. } => Doing::Running,
            Event::ToolFinished { .. } => Doing::Thinking,
            Event::TurnStarted { .. } | Event::TurnFinished { .. } | Event::Failed { .. } => {
                self.doing
            }
        };
    }

    /// Says the turn has been asked to stop.
    pub(super) fn interrupting(&mut self) {
        self.doing = Doing::Interrupting;
    }

    /// Whether the row would now be drawn differently from the last one drawn,
    /// recording this one as drawn.
    ///
    /// What the loop above redraws on between events. Everything on the row is
    /// read from the word and the clock, and the beat is the coarsest thing the
    /// clock is read by — so these two together are the row, and a pair that
    /// has not moved is a frame nobody would be able to tell from the last.
    pub(super) fn moved(&mut self) -> bool {
        let now = (self.doing, Working::beat(self.running()));
        let moved = self.drawn != Some(now);

        self.drawn = Some(now);
        moved
    }

    /// The rows to put above the box, or none where the window has no room.
    ///
    /// A blank either side, so the row belongs to neither the turn's own output
    /// above it nor the box below. `room` is what is left of the window once
    /// the box has taken its share: dropped whole rather than squeezed, because
    /// a footing taller than the window is a region the renderer cannot rewind
    /// over, and one row of turn output is worth more than a clock.
    pub(super) fn rows(&self, columns: usize, glyphs: Glyphs, room: usize) -> Vec<Row> {
        if room <= ROWS {
            return Vec::new();
        }

        let working = Working {
            doing: self.doing.word(),
            running: self.running(),
            stops: (self.doing != Doing::Interrupting).then_some(STOPS),
        };

        vec![Row::new(), working.row(columns, glyphs), Row::new()]
    }

    /// How long the turn has been running.
    fn running(&self) -> Duration {
        self.since.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{ToolArgs, ToolCall, ToolId, ToolOutput, TurnId};

    use super::*;

    /// The word the row says after `event`, from a turn that just started.
    fn after(event: &Event) -> &'static str {
        let mut turning = Turning::started();
        turning.saw(event);
        turning.doing.word()
    }

    fn requested() -> Event {
        Event::ToolRequested {
            call: ToolCall {
                id: ToolId::new("a"),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            },
        }
    }

    #[test]
    fn the_word_says_which_of_the_two_things_a_turn_does_is_happening() {
        // Waiting on the model and waiting on a tool are the two, and they are
        // the two because they fail differently: a turn stuck thinking is a
        // provider that has gone quiet, and one stuck running is a command that
        // has not come back. A single word for both would hide which.
        assert_eq!(
            after(&Event::TurnStarted {
                turn: TurnId::FIRST
            }),
            "thinking"
        );
        assert_eq!(after(&Event::Delta { text: "hi".into() }), "writing");
        assert_eq!(after(&requested()), "running");
        assert_eq!(
            after(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            "thinking"
        );
    }

    #[test]
    fn a_turn_asked_to_stop_goes_on_saying_so_whatever_arrives_after() {
        // The deltas already in flight land after the key. A row that read them
        // and went back to `writing` would be saying the key was missed, at the
        // one moment somebody is watching the row to find out whether it was.
        let mut turning = Turning::started();
        turning.interrupting();
        turning.saw(&Event::Delta { text: "hi".into() });

        assert_eq!(turning.doing.word(), "interrupting");

        // And stops offering the key that has already been pressed.
        let rows = turning.rows(80, Glyphs::Unicode, 24);
        let said = rows.iter().map(Row::text).collect::<String>();

        assert!(said.contains("interrupting"), "{said:?}");
        assert!(!said.contains(STOPS), "{said:?}");
    }

    #[test]
    fn a_row_that_would_be_drawn_the_same_again_is_not_drawn_again() {
        // The whole cost of an animated row on a sixty-times-a-second tick.
        // Without this the box under it is laid out and written on every one of
        // them, to produce the bytes that were already on the screen.
        let mut turning = Turning::started();

        assert!(turning.moved(), "the first row was never drawn");
        assert!(!turning.moved(), "the same row was drawn twice");

        turning.saw(&Event::Delta { text: "hi".into() });
        assert!(turning.moved(), "the word changed and the row did not");
    }

    #[test]
    fn a_window_with_no_room_for_the_row_keeps_the_turn_s_own_output_instead() {
        let turning = Turning::started();

        for room in 0..=ROWS {
            assert!(turning.rows(80, Glyphs::Unicode, room).is_empty(), "{room}");
        }

        assert_eq!(turning.rows(80, Glyphs::Unicode, ROWS + 1).len(), ROWS);
    }
}
