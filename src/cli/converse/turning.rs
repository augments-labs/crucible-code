//! What stands above the box while a turn is running.
//!
//! The component draws a mark, a word, a clock and a count; this is where all
//! four come from. The clock starts as the turn leaves for its own thread and
//! never pauses — not for a permission question, which is time somebody is
//! waiting just as much. The word and the count are read off the events the
//! turn reports as they go past, which is why they are read here rather than by
//! the component: the events are this program's, and the component knows
//! nothing about them.
//!
//! The key is named here for the second time on the screen — the row under the
//! box names it too — and that is deliberate. It is the third segment of this
//! row and the first thing a narrow window drops, and the key that stops a turn
//! is the wrong thing for a narrow window to take away.
//!
//! The call whose tool is out is held here too, for the same reason the word
//! is: the events go past this and nowhere else. It is held rather than written
//! so that the line and the result hanging under it reach scrollback together —
//! [`Turning::saw`] hands it back the moment the tool answers, and whoever
//! drives this writes it.

use std::time::{Duration, Instant};

use crucible_core::Event;
use crucible_tui::{Glyphs, Row, Working};

use super::super::draw;

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
    /// What it has spent so far, or `None` until the provider says.
    spent: Option<u64>,
    /// The words of the line of the call whose tool is out, or `None` where
    /// none is. Without the mark, which is the writer's to draw.
    calling: Option<String>,
    /// What the row was last drawn from, so a redraw that would draw the same
    /// row again can be skipped. `None` before the first.
    drawn: Option<Drawn>,
}

/// Everything the row is drawn from, coarsened to what it is drawn *by*.
///
/// The clock counts and the mark turns while nothing arrives, so the loop above
/// redraws on its own — and this is what it asks about first. A value that has
/// not moved is a frame nobody would be able to tell from the last, so anything
/// the row says has to be in here: a segment left out is one that changes on
/// screen only when something else on the row happens to change with it.
type Drawn = (Doing, Option<u64>, u64);

impl Turning {
    /// A turn that starts now.
    pub(super) fn started() -> Self {
        Self {
            since: Instant::now(),
            doing: Doing::Thinking,
            spent: None,
            calling: None,
            drawn: None,
        }
    }

    /// Takes the word from one event on its way to the screen, and hands back
    /// the call line that has stopped being live, where one has.
    ///
    /// Every variant is named rather than caught by a rest arm: an event added
    /// later either changes what the turn is doing or does not, and that is a
    /// decision to make here rather than one to inherit.
    pub(super) fn saw(&mut self, event: &Event) -> Option<String> {
        // Before the guard below, because what a turn spent is true whether it
        // is stopping or not — and a turn asked to stop goes on spending until
        // the response in flight has finished arriving. That is the stretch
        // somebody is most likely to be watching the number.
        if let Event::Spent { spend } = event {
            self.spent = Some(spend.tokens());
        }

        // Before it as well, and for a sharper reason. A turn asked to stop
        // still has its tool out, and that tool still answers; a turn that ends
        // or fails with one out never gets an answer at all. Either way the
        // line has to come back, or a call that was made leaves no record —
        // which is the one thing a transcript may not do.
        let returned = match event {
            Event::ToolRequested { call, summary } => {
                self.calling = Some(draw::called(call, summary));
                None
            }
            Event::ToolFinished { .. } | Event::TurnFinished { .. } | Event::Failed { .. } => {
                self.calling.take()
            }
            Event::TurnStarted { .. } | Event::Delta { .. } | Event::Spent { .. } => None,
        };

        // A turn that has been asked to stop is stopping whatever else it is
        // still reporting. The deltas already in flight arrive after the key,
        // and a row that went back to `writing` would be saying the key missed.
        if self.doing == Doing::Interrupting {
            return returned;
        }

        self.doing = match event {
            Event::Delta { .. } => Doing::Writing,
            Event::ToolRequested { .. } => Doing::Running,
            Event::ToolFinished { .. } => Doing::Thinking,
            Event::TurnStarted { .. }
            | Event::Spent { .. }
            | Event::TurnFinished { .. }
            | Event::Failed { .. } => self.doing,
        };

        returned
    }

    /// Says the turn has been asked to stop.
    pub(super) fn interrupting(&mut self) {
        self.doing = Doing::Interrupting;
    }

    /// Whether the row would now be drawn differently from the last one drawn,
    /// recording this one as drawn.
    ///
    /// What the loop above redraws on between events. The beat is the coarsest
    /// thing the clock is read by, so it stands in for the clock and for the
    /// face the mark is wearing; the other two are what the row says beside
    /// them.
    pub(super) fn moved(&mut self) -> bool {
        let now: Drawn = (self.doing, self.spent, Working::beat(self.running()));
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
            spent: self.spent,
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
    use crucible_core::{
        Spend, StopReason, Summary, ToolArgs, ToolCall, ToolId, ToolOutput, TurnError, TurnId,
    };

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
            summary: Summary::new("src/main.rs"),
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
    fn the_row_says_what_the_turn_has_spent_once_the_provider_has_said() {
        // And says nothing in its place until then, which is what every turn
        // looks like until its first response comes back.
        let mut turning = Turning::started();
        let said = |turning: &Turning| {
            turning
                .rows(80, Glyphs::Unicode, 24)
                .iter()
                .map(Row::text)
                .collect::<String>()
        };

        assert!(!said(&turning).contains('↓'), "{:?}", said(&turning));

        turning.saw(&Event::Spent {
            spend: Spend::new(12_800),
        });

        assert!(said(&turning).contains("↓ 12.8k"), "{:?}", said(&turning));
    }

    #[test]
    fn a_turn_asked_to_stop_goes_on_counting_what_it_spends() {
        // The word stops moving when the key is pressed; the count does not.
        // The response already in flight goes on arriving and goes on costing,
        // and that stretch is the one somebody is most likely to be watching
        // the number through.
        let mut turning = Turning::started();
        turning.interrupting();
        turning.saw(&Event::Spent {
            spend: Spend::new(2_900),
        });

        assert_eq!(turning.spent, Some(2_900));
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

        // And the count is on the row, so it is on the value the loop keys on.
        // Left off, it would reach the screen only on the beat some other
        // segment happened to change — a stale number, arriving late, on the
        // row somebody is reading to find out what is going on.
        turning.saw(&Event::Spent {
            spend: Spend::new(1_400),
        });
        assert!(turning.moved(), "the count changed and the row did not");
    }

    #[test]
    fn a_window_with_no_room_for_the_row_keeps_the_turn_s_own_output_instead() {
        let turning = Turning::started();

        for room in 0..=ROWS {
            assert!(turning.rows(80, Glyphs::Unicode, room).is_empty(), "{room}");
        }

        assert_eq!(turning.rows(80, Glyphs::Unicode, ROWS + 1).len(), ROWS);
    }

    #[test]
    fn the_call_line_comes_back_when_its_tool_answers_and_only_then() {
        // Held from the moment the model asks until then, so that the line and
        // the result hanging under it are written one after the other.
        let mut turning = Turning::started();

        assert_eq!(turning.saw(&requested()), None);
        assert_eq!(turning.saw(&Event::Delta { text: "hi".into() }), None);
        assert_eq!(
            turning.saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            Some("Read(src/main.rs)".to_owned())
        );

        // And once only. A second reading would commit the same line twice.
        assert_eq!(
            turning.saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            None
        );
    }

    #[test]
    fn a_turn_that_ends_with_a_tool_still_out_hands_its_call_back_anyway() {
        // Otherwise a call that was made leaves no record of having been made:
        // its line was still being held, and the turn holding it is over. That
        // is the one thing a transcript may not do -- and it is reached by
        // every turn that fails or is stopped mid-call, which is exactly when
        // somebody goes looking for what ran.
        for ending in [
            Event::TurnFinished {
                turn: TurnId::FIRST,
                stop: StopReason::Cancelled,
            },
            Event::Failed {
                error: TurnError::Refused("read".into()),
            },
        ] {
            let mut turning = Turning::started();
            turning.saw(&requested());

            assert_eq!(
                turning.saw(&ending),
                Some("Read(src/main.rs)".to_owned()),
                "{ending:?}"
            );
        }
    }

    #[test]
    fn a_turn_asked_to_stop_still_lets_the_call_it_had_out_come_back() {
        // The word freezes at `interrupting` when the key is pressed. The line
        // of the call still out is not a word, and holding on to it too would
        // lose the record of the call at the one moment there is most to
        // explain.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.interrupting();

        assert_eq!(
            turning.saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            Some("Read(src/main.rs)".to_owned())
        );
    }
}
