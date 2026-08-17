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
//! A call standing above that row is the other thing held here. A tool that has
//! been asked for and has not answered gets its line drawn with a mark that
//! pulses, and it lives here rather than in scrollback because a line the
//! renderer moves back over cannot also be committed. When the tool answers the
//! line is handed back — [`Turning::saw`] returns it — and whoever drives this
//! commits it still, so what reaches scrollback is the same words with the
//! motion gone.

use std::time::{Duration, Instant};

use crucible_core::Event;
use crucible_tui::{Row, Slot, Working};

use super::super::draw;
use super::super::style::Style;

/// How the turn is asked to stop, said after the clock.
const STOPS: &str = "esc to interrupt";

/// The rows this puts above the box, blanks included.
const ROWS: usize = 3;

/// And with a call standing over it, the blank that parts them included.
///
/// The call line is what a narrow window gives up first, because it is the one
/// of the two that a second look gets back anyway: the tool answers and the
/// line is committed to scrollback either way. The row saying a turn is running
/// exists nowhere else.
const CALLING: usize = ROWS + 2;

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
    /// The words of the call whose tool is out, or `None` where none is. What
    /// the tool said the call was about, without the mark: the mark is the part
    /// that moves, and it is drawn per frame.
    calling: Option<String>,
    /// What the footing was last drawn from, so a redraw that would draw the
    /// same rows again can be skipped. `None` before the first.
    drawn: Option<Drawn>,
}

/// Everything the footing is drawn from, coarsened to what it is drawn *by*.
///
/// The clock counts and the marks move while nothing arrives, so the loop above
/// redraws on its own — and this is what it asks about first. A value that has
/// not moved is a frame nobody would be able to tell from the last, so anything
/// the rows say has to be in here: a segment left out is one that changes on
/// screen only when something else happens to change with it.
#[derive(Debug, PartialEq, Eq)]
struct Drawn {
    /// The word the row says.
    doing: Doing,
    /// The count beside it.
    spent: Option<u64>,
    /// The clock, and the face both marks are wearing, coarsened to the one
    /// number every unit of them divides.
    beat: u64,
    /// The call standing over the row, where one is.
    calling: Option<String>,
}

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
        let now = Drawn {
            doing: self.doing,
            spent: self.spent,
            beat: Working::beat(self.running()),

            // Cloned rather than borrowed, because it is kept until the next
            // frame to be compared against. A tool's name and a path is tens of
            // bytes, taken at most four times a second, and it is freed the
            // moment the tool answers — nothing here grows with the transcript.
            calling: self.calling.clone(),
        };
        let moved = self.drawn.as_ref() != Some(&now);

        self.drawn = Some(now);
        moved
    }

    /// The rows to put above the box, or none where the window has no room.
    ///
    /// A blank either side, so the rows belong to neither the turn's own output
    /// above them nor the box below, and a blank between the call and the row
    /// under it for the same reason: the call is a thing that is happening and
    /// the row is what is happening to the turn. `room` is what is left of the
    /// window once the box has taken its share: dropped whole rather than
    /// squeezed, because a footing taller than the window is a region the
    /// renderer cannot rewind over, and one row of turn output is worth more
    /// than a clock.
    pub(super) fn rows(&self, columns: usize, style: Style, room: usize) -> Vec<Row> {
        if room <= ROWS {
            return Vec::new();
        }

        let working = Working {
            doing: self.doing.word(),
            running: self.running(),
            spent: self.spent,
            stops: (self.doing != Doing::Interrupting).then_some(STOPS),
        };

        let mut rows = Vec::new();

        if let Some(said) = self.calling.as_deref().filter(|_| room > CALLING) {
            rows.push(Row::new());
            rows.push(self.call(said, columns, style));
        }

        rows.push(Row::new());
        rows.push(working.row(columns, style.glyphs()));
        rows.push(Row::new());

        rows
    }

    /// The line for the call whose tool is out.
    ///
    /// The mark pulses rather than turning: a call is one thing waiting on one
    /// answer, and the row below it is already carrying the mark that says the
    /// turn as a whole is moving. Two marks cycling through four faces beside
    /// each other read as two independent things rather than as one inside the
    /// other. On the same beat as that one, so the footing moves as one picture.
    ///
    /// The words go through the same clipping the committed line uses, so the
    /// line does not change shape at the moment it stops moving.
    fn call(&self, said: &str, columns: usize, style: Style) -> Row {
        let lit = Working::beat(self.running()).is_multiple_of(2);
        let slot = if lit { Slot::Accent } else { Slot::Quiet };

        let row = Row::new().then(slot, style.glyphs().called());

        // The mark alone where the window left no room for the words, as the
        // committed line does: the space after it would be the one column a
        // window that narrow has not got.
        match draw::words(said, columns, style) {
            words if words.is_empty() => row,
            words => row.then(Slot::Plain, format!(" {words}")),
        }
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
    use crucible_tui::Palette;

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
        let rows = turning.rows(80, Style::plain(), 24);
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
                .rows(80, Style::plain(), 24)
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
            assert!(turning.rows(80, Style::plain(), room).is_empty(), "{room}");
        }

        assert_eq!(turning.rows(80, Style::plain(), ROWS + 1).len(), ROWS);
    }

    #[test]
    fn a_call_stands_over_the_row_for_as_long_as_its_tool_is_out() {
        // Here rather than in scrollback, because the mark on it moves: a line
        // the renderer rewinds over every frame cannot also be a line it never
        // rewinds over. It is committed when the tool answers and not before.
        let mut turning = Turning::started();
        let said = |turning: &Turning| {
            turning
                .rows(80, Style::plain(), 24)
                .iter()
                .map(Row::text)
                .collect::<Vec<_>>()
        };

        assert!(!said(&turning).iter().any(|row| row.contains("Read")));

        turning.saw(&requested());
        let standing = said(&turning);

        assert_eq!(standing.len(), CALLING, "{standing:?}");

        // By position, since what is under test is the order: the call over the
        // row that says a turn is running, and a blank above the call and
        // another between the two, so neither belongs to the output above nor
        // to the box under them.
        let at = |row: usize| standing.get(row).cloned().unwrap_or_default();

        assert!(at(0).is_empty(), "{standing:?}");
        assert!(at(1).contains("Read(src/main.rs)"), "{standing:?}");
        assert!(at(2).is_empty(), "{standing:?}");
        assert!(at(3).contains("running"), "{standing:?}");
        assert!(at(4).is_empty(), "{standing:?}");
    }

    #[test]
    fn the_call_line_comes_back_when_its_tool_answers_and_only_then() {
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
        // the line was never committed, and the turn it was standing in is
        // over. That is the one thing a transcript may not do -- and it is
        // reached by every turn that fails or is stopped mid-call, which is
        // exactly when somebody goes looking for what ran.
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
        // of the call still out is not a word, and freezing it too would lose
        // the record of the call at the one moment there is most to explain.
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

    #[test]
    fn the_mark_on_a_live_call_pulses_and_the_words_beside_it_do_not_move() {
        // Two frames, half a beat apart. The mark is painted one way and then
        // the other; everything after it is the same string in the same
        // columns, because a call line that changed width four times a second
        // would be unreadable next to the row it stands over.
        // Against a palette that writes colour, because the pulse *is* colour:
        // on a terminal without any, the two faces are the same mark and the
        // row is still and correct. What is under test is the beat reaching the
        // slot, so the instrument has to be one that can tell two slots apart.
        let style = Style::plain();
        let palette = Palette::resolve(true, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        });
        let now = Instant::now();

        // One beat apart to the microsecond, rather than two readings of the
        // clock a beat apart in wall time: what is under test is that the face
        // changes from one beat to the next, and a machine that stalled between
        // two readings would be testing how long the stall was.
        let face = |beat: Duration| {
            let moment = Turning {
                since: now.checked_sub(beat).expect("a clock past its own epoch"),
                ..Turning::started()
            };

            moment.call("Read(src/main.rs)", 80, style).paint(palette)
        };
        let (lit, dim) = (face(Duration::ZERO), face(Duration::from_millis(250)));

        assert_ne!(lit, dim, "the mark did not pulse");
        for face in [&lit, &dim] {
            assert!(face.contains("Read(src/main.rs)"), "{face}");
        }
    }

    #[test]
    fn the_call_line_is_on_the_value_the_loop_keys_a_redraw_on() {
        // Left off it, a call would appear on screen only on the beat some
        // other segment happened to change -- so the line naming what is
        // running would arrive after the tool it names had already answered.
        let mut turning = Turning::started();
        turning.moved();

        turning.saw(&requested());
        assert!(turning.moved(), "the call appeared and the footing did not");

        turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
        });
        assert!(turning.moved(), "the call went and the footing did not");
    }

    #[test]
    fn a_window_too_short_for_both_drops_the_call_before_the_row() {
        // The call is written to scrollback the moment its tool answers, so a
        // window that drops it loses nothing a second look does not return.
        // The row saying a turn is running exists nowhere else.
        let mut turning = Turning::started();
        turning.saw(&requested());

        let rows = turning.rows(80, Style::plain(), CALLING);
        let said = rows.iter().map(Row::text).collect::<String>();

        assert_eq!(rows.len(), ROWS, "{said:?}");
        assert!(said.contains("running"), "{said:?}");
        assert!(!said.contains("Read"), "{said:?}");
    }
}
