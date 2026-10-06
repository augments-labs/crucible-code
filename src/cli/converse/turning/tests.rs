//! The live turn footing, its calls, output, queue, and plan.

use crucible_runner::TurnError;
use crucible_tools::{Summary, ToolOutput};
use crucible_tui::{Editor, Glyphs, Key};
use crucible_types::{Spend, StopReason, ToolArgs, ToolCall, ToolId, TurnId};

use super::*;

/// No plan at all, which is what a session has until the agent writes one
/// and is what every test here but the last two is about.
fn nothing() -> Planning {
    Planning::new(crucible_builtins::Plan::new())
}

impl Turning {
    /// Both bands of the footing as one run of rows, over an empty queue: the
    /// layout every case here but the queue's is about, where nothing stands
    /// over the transcript and the two bands are the one.
    // The inputs `rows` takes less the two this fixes, so one over the limit
    // for the reason it gives.
    #[allow(clippy::too_many_arguments)]
    fn laid(
        &self,
        planning: &Planning,
        counting: &str,
        columns: usize,
        style: Style,
        room: usize,
    ) -> Vec<Row> {
        let (mut turn, over) = self.rows(
            planning,
            counting,
            &Prompts::default(),
            across(columns),
            style,
            room,
        );
        turn.extend(over);
        turn
    }
}

#[test]
fn an_empty_queue_adds_no_row_to_the_footing() {
    // The panel is for a queue that has something in it. With nothing
    // waiting, the footing is the same three rows it has always been — the
    // blank, the word, the blank — and not one row taller for a frame around
    // nothing.
    let rows = Turning::started(Breakdown::default()).laid(&nothing(), "", 80, Style::plain(), 24);
    assert_eq!(rows.len(), ROWS, "{:?}", rows.iter().map(Row::text));
}

/// A plan of `count` open tasks, each named after where it is in the list.
///
/// Written through the tool the way the model writes one, because that is
/// the only way anything gets into a plan and the panel is drawn from what
/// came out the other side.
fn planned(count: usize) -> Planning {
    let said = (0..count)
        .map(|at| format!(r#"{{"task":"Task {at}","state":"open"}}"#))
        .collect::<Vec<_>>()
        .join(",");

    let plan = crucible_builtins::Plan::new();
    plan.replay(&ToolArgs::new(format!(r#"{{"tasks":[{said}]}}"#)));

    Planning::new(plan)
}

/// The word the row says after `event`, from a turn that just started.
fn after(event: &Event) -> &'static str {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(event);
    turning.doing.word()
}

fn requested() -> Event {
    requested_as("read", false)
}

fn requested_as(name: &str, backgroundable: bool) -> Event {
    Event::ToolRequested {
        call: ToolCall {
            id: ToolId::new("a"),
            name: name.into(),
            args: ToolArgs::new("{}"),
        },
        summary: Summary::new("src/main.rs"),
        backgroundable,
        looking: None,
        alone: false,
    }
}

/// One call of a pass of several, named by `id` so a batch can be built.
fn requested_of_batch(id: &str, name: &str, about: &str, backgroundable: bool) -> Event {
    Event::ToolRequested {
        call: ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new("{}"),
        },
        summary: Summary::new(about),
        backgroundable,
        looking: None,
        alone: false,
    }
}

/// Everything the footing says, one string per row.
/// The delay `output.pinAfterSeconds` holds a running call back by when unset.
const PINNED: Duration = Duration::from_secs(3);

/// Every call out, as if it had been out for as long as a call is before it
/// stands over the row: what a test drawing a live call is about.
fn aged(turning: &mut Turning) {
    for calling in &mut turning.calling {
        calling.asked = Instant::now()
            .checked_sub(turning.pinned_after)
            .unwrap_or(calling.asked);
    }
}

fn footing(turning: &Turning) -> Vec<String> {
    turning
        .laid(&nothing(), "", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect()
}

#[test]
fn a_pass_of_calls_out_is_counted_rather_than_named_one_at_a_time() {
    // Eight fetches asked for at once used to put the first of the eight above
    // the box and leave it there until it answered, so a reader watched one
    // URL for as long as the whole batch took. What is out is a count now.
    let mut turning = Turning::started(Breakdown::default());
    for at in 0..8 {
        turning.saw(&requested_of_batch(
            &format!("f-{at}"),
            "web_fetch",
            &format!("https://example.com/{at}"),
            false,
        ));
    }

    let said = footing(&turning).join("\n");
    assert!(said.contains("8 WebFetch"), "{said}");
    assert!(!said.contains("https://example.com/0"), "{said}");
}

#[test]
fn a_pass_of_mixed_calls_counts_each_kind_in_the_order_it_was_asked_for() {
    let mut turning = Turning::started(Breakdown::default());
    for (id, name, about) in [
        ("f-0", "web_fetch", "https://example.com/0"),
        ("r-0", "read", "src/main.rs"),
        ("r-1", "read", "src/lib.rs"),
        ("f-1", "web_fetch", "https://example.com/1"),
    ] {
        turning.saw(&requested_of_batch(id, name, about, false));
    }

    let said = footing(&turning).join("\n");
    assert!(said.contains("2 WebFetch and 2 Read"), "{said}");
}

#[test]
fn one_call_out_is_still_named_rather_than_counted() {
    // A count of one says less than the call it counts, and the row a single
    // call has always had is the one thing that still fits.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested_of_batch(
        "f-0",
        "web_fetch",
        "https://example.com/0",
        false,
    ));

    let said = footing(&turning).join("\n");
    assert!(said.contains("WebFetch(https://example.com/0)"), "{said}");
    assert!(!said.contains("1 WebFetch and"), "{said}");
}

#[test]
fn a_pass_holding_a_command_keeps_the_command_row_it_can_be_backgrounded_from() {
    // The key that leaves one running points at the row that names it, and a
    // count is not something `ctrl+b` can act on.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested_of_batch("b-0", "bash", "cargo test", true));
    turning.saw(&requested_of_batch(
        "f-0",
        "web_fetch",
        "https://example.com",
        false,
    ));

    let said = footing(&turning).join("\n");
    assert!(said.contains("Bash(cargo test)"), "{said}");
    assert!(turning.can_background());
}

#[test]
fn a_call_answered_out_of_a_pass_moves_the_frame_the_count_is_drawn_from() {
    // The front of the queue is unchanged when the third of eight answers, so
    // a redraw keyed on which call is at the front would leave the new count
    // to reach the screen on whatever moved next.
    let mut turning = Turning::started(Breakdown::default());
    for at in 0..3 {
        turning.saw(&requested_of_batch(
            &format!("f-{at}"),
            "web_fetch",
            &format!("https://example.com/{at}"),
            false,
        ));
    }

    assert!(turning.moved());
    assert!(!turning.moved(), "nothing happened between the two frames");

    turning.saw(&Event::ToolFinished {
        call: ToolId::new("f-1"),
        output: ToolOutput::ok("a page"),
        receipt: None,
    });

    assert!(turning.moved(), "the count went from 3 to 2 unseen");
}

/// A call the runner reported as the only one of its pass.
///
/// Separate from [`requested_as`] rather than a flag on it, because the two
/// describe different facts about a turn and the tests below turn on which:
/// `alone` is what the runner counted in the response, not what a test happened
/// to send one of.
fn requested_alone(name: &str, about: &str, backgroundable: bool) -> Event {
    Event::ToolRequested {
        call: ToolCall {
            id: ToolId::new("a"),
            name: name.into(),
            args: ToolArgs::new("{}"),
        },
        summary: Summary::new(about),
        backgroundable,
        looking: None,
        alone: true,
    }
}

#[test]
fn a_lone_call_with_nothing_live_about_it_is_returned_where_it_is_asked_for() {
    // A fetch is the case: it prints nothing while it runs, it cannot be
    // backgrounded, and its row says the same words at the end that it said at
    // the start. Nothing about it moves, so nothing about it needs the footing,
    // and holding it there would leave the reader watching the bottom of the
    // screen for as long as the network takes.
    let mut turning = Turning::started(Breakdown::default());

    assert_eq!(
        lines(turning.saw(&requested_alone("web_fetch", "https://example.com", false))),
        vec![(ToolId::new("a"), "WebFetch(https://example.com)".to_owned())],
    );
}

#[test]
fn a_call_written_where_it_was_asked_for_does_not_stand_in_the_footing() {
    // And is not handed back a second time when the tool answers: the row is
    // the transcript's already, and a turn that returned it twice would write
    // the call once for the asking and once for the answer.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested_alone("web_search", "rust traits", false));

    let rows = turning.laid(&nothing(), "", 80, Style::plain(), 24);
    assert_eq!(rows.len(), ROWS, "{:?}", rows.iter().map(Row::text));

    assert_eq!(
        lines(turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("four results"),
            receipt: None,
        })),
        Vec::new(),
    );
}

#[test]
fn a_lone_call_that_can_be_backgrounded_stands_in_the_footing_as_before() {
    // The footing is where the key that leaves it running points and where the
    // output it prints goes. A command has both, so being the only call of its
    // pass buys it nothing.
    let mut turning = Turning::started(Breakdown::default());

    assert_eq!(
        lines(turning.saw(&requested_alone("bash", "cargo test", true))),
        Vec::new(),
    );
    assert!(turning.can_background());
}

/// The lines that came back, as the pairs these tests are written about: which
/// call it was and what its row says.
fn lines(settled: Vec<Settled>) -> Vec<(ToolId, String)> {
    settled
        .into_iter()
        .map(|one| (one.call, one.said.text().to_owned()))
        .collect()
}

#[test]
fn the_active_call_decides_whether_the_background_key_is_live() {
    let mut turning = Turning::started(Breakdown::default());
    assert!(!turning.can_background());

    turning.saw(&requested_as("web_search", false));
    assert!(!turning.can_background());

    turning.saw(&Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::ok("done"),
        receipt: None,
    });
    turning.saw(&requested_as("bash", true));
    assert!(turning.can_background());
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
            receipt: None,
        }),
        "thinking"
    );
}

#[test]
fn a_response_being_asked_for_again_says_so_until_the_new_one_speaks() {
    // The span it covers is the whole of the second ask — the pause and the
    // request after it — and `thinking` over that span would be a row saying
    // the first answer is still on its way.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&Event::Retrying);

    assert_eq!(turning.doing.word(), "retrying");

    turning.saw(&Event::Delta { text: "hi".into() });
    assert_eq!(turning.doing.word(), "writing");
}

#[test]
fn a_turn_asked_to_stop_goes_on_saying_so_whatever_arrives_after() {
    // The deltas already in flight land after the key. A row that read them
    // and went back to `writing` would be saying the key was missed, at the
    // one moment somebody is watching the row to find out whether it was.
    let mut turning = Turning::started(Breakdown::default());
    turning.interrupting();
    turning.saw(&Event::Delta { text: "hi".into() });

    assert_eq!(turning.doing.word(), "interrupting");

    // And stops offering the key that has already been pressed.
    let rows = turning.laid(&nothing(), "", 80, Style::plain(), 24);
    let said = rows.iter().map(Row::text).collect::<String>();

    assert!(said.contains("interrupting"), "{said:?}");
    assert!(!said.contains(STOPS), "{said:?}");
}

#[test]
fn the_row_says_what_the_turn_has_spent_once_the_provider_has_said() {
    // And says nothing in its place until then, which is what every turn
    // looks like until its first response comes back.
    let mut turning = Turning::started(Breakdown::default());
    let said = |turning: &Turning| {
        turning
            .laid(&nothing(), "", 80, Style::plain(), 24)
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
fn the_window_left_is_handed_to_the_prompt_and_takes_no_turn_row() {
    // The prompt border now owns the one place this reading stands. Turning
    // retains the latest event value for it, but lays out no duplicate row.
    let carried = crate::cli::converse::tests::measured();
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&Event::Carried { breakdown: carried });

    let rows = turning.laid(&nothing(), "", 80, Style::plain(), 24);
    let texts: Vec<String> = rows.iter().map(Row::text).collect();

    assert!(carried.left().is_some(), "{carried:?}");
    assert_eq!(turning.left(), carried.left());
    assert!(
        texts.iter().all(|row| !row.contains("window left")),
        "{texts:?}"
    );
}

#[test]
fn a_turn_asked_to_stop_goes_on_counting_what_it_spends() {
    // The word stops moving when the key is pressed; the count does not.
    // The response already in flight goes on arriving and goes on costing,
    // and that stretch is the one somebody is most likely to be watching
    // the number through.
    let mut turning = Turning::started(Breakdown::default());
    turning.interrupting();
    turning.saw(&Event::Spent {
        spend: Spend::new(2_900),
    });

    assert_eq!(turning.spent, Some(2_900));
}

#[test]
fn a_turn_asked_to_stop_keeps_factual_window_and_compaction_state_current() {
    let started = Breakdown::default();
    let carried = crate::cli::converse::tests::measured();
    assert_ne!(started.left(), carried.left());
    let mut turning = Turning::started(started);
    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: 12,
    });
    turning.interrupting();

    turning.saw(&Event::Carried { breakdown: carried });
    assert_eq!(turning.left(), carried.left());

    turning.saw(&Event::Compacted {
        compacted: crucible_types::Compacted {
            why: Compacting::Asked,
            replaced: 3,
            before: 80,
            after: 70,
            kept: 1,
        },
    });
    assert_eq!(turning.part, 100);
    assert_eq!(turning.doing, Doing::Interrupting);
}

#[test]
fn completed_compaction_stays_full_long_enough_to_be_seen() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: 64,
    });
    turning.saw(&Event::Compacted {
        compacted: crucible_types::Compacted {
            why: Compacting::Asked,
            replaced: 3,
            before: 80,
            after: 20,
            kept: 1,
        },
    });

    let complete = turning
        .laid(&nothing(), "", 80, Style::plain(), 24)
        .into_iter()
        .map(|row| row.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(complete.contains("compacting"), "{complete:?}");
    assert!(complete.contains("100%"), "{complete:?}");
    assert!(turning.moved(), "the completed bar did not produce a frame");

    let completed = turning.completed.expect("completion was not timed");
    turning.finished_frame(
        (completed + COMPLETE_FOR)
            .checked_sub(Duration::from_millis(1))
            .expect("the completion deadline to exceed one millisecond"),
    );
    assert_eq!(turning.part, 100, "completion disappeared before its dwell");
    assert!(!turning.moved(), "the held completion invented a new frame");

    turning.finished_frame(completed + COMPLETE_FOR);
    assert!(
        turning.moved(),
        "clearing the completed bar did not guarantee a following frame"
    );
    let gone = turning
        .laid(&nothing(), "", 80, Style::plain(), 24)
        .into_iter()
        .map(|row| row.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!gone.contains('%'), "{gone:?}");
    assert!(gone.contains("thinking"), "{gone:?}");
}

#[test]
fn a_row_that_would_be_drawn_the_same_again_is_not_drawn_again() {
    // The whole cost of an animated row on a sixty-times-a-second tick.
    // Without this the box under it is laid out and written on every one of
    // them, to produce the bytes that were already on the screen.
    let mut turning = Turning::started(Breakdown::default());

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
fn the_bar_moves_on_the_notes_rather_than_on_whatever_else_changes() {
    // The bar is a segment of the row, so it belongs to the value the loop
    // keys a redraw on. Left out of it, it reaches the screen only when
    // something else on the row happens to change with it — and on a
    // request that draws nothing else for a minute, the something else is
    // the clock.
    let mut turning = Turning::started(Breakdown::default());
    assert!(turning.moved(), "the first row was never drawn");

    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: 0,
    });
    assert!(
        turning.moved(),
        "room was asked for and the row did not say"
    );

    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: 12,
    });
    assert!(turning.moved(), "the bar moved and the row did not");
}

#[test]
fn compacting_keeps_the_latest_window_reading_until_its_replacement_arrives() {
    let started = crate::cli::converse::tests::measured();
    let mut turning = Turning::started(started);

    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: 12,
    });

    assert!(started.left().is_some(), "{started:?}");
    assert_eq!(turning.left(), started.left());
}

#[test]
fn the_bar_arrives_with_the_notes_rather_than_standing_at_nothing() {
    // Nothing is measurable until the first word of the recap arrives: the
    // request is out and the model is reading the session it is about to
    // write down, which on a full window is seconds. There is no row until
    // then, because a bar at nothing is claiming a length it does not have.
    let style = Style::plain();
    let glyphs = style.glyphs();

    assert!(making(0, 80, style).is_none(), "a bar at nothing drew");

    let under = making(12, 80, style).expect("a row").text();

    assert!(under.contains(glyphs.filled()), "{under:?}");
    assert!(under.contains("12%"), "{under:?}");

    // The bar starts in the column the word above it starts in: the mark
    // and the space after it, so it reads as a second line of that row.
    let gutter = Working::gutter(glyphs);
    assert_eq!(
        under.chars().take(gutter).filter(|c| *c == ' ').count(),
        gutter,
        "{under:?}"
    );
}

#[test]
fn another_compaction_before_the_completed_frame_keeps_the_new_progress() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: 64,
    });
    turning.saw(&Event::Compacted {
        compacted: crucible_types::Compacted {
            why: Compacting::Asked,
            replaced: 3,
            before: 80,
            after: 20,
            kept: 1,
        },
    });
    turning.saw(&Event::Compacting {
        why: Compacting::Full,
        part: 12,
    });

    turning.finished_frame(Instant::now() + COMPLETE_FOR);

    assert_eq!(turning.making, Some(Compacting::Full));
    assert_eq!(turning.part, 12);
}

#[test]
fn an_in_progress_value_cannot_claim_or_clear_completion() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&Event::Compacting {
        why: Compacting::Asked,
        part: u8::MAX,
    });

    assert_eq!(turning.part, 99);
    turning.finished_frame(Instant::now() + COMPLETE_FOR);
    assert!(turning.making.is_some());
}

#[test]
fn the_compaction_bar_uses_two_thirds_of_the_available_cells_and_never_overflows() {
    let style = Style::plain();
    let glyphs = style.glyphs();
    let short = making(50, 44, style).expect("a short bar");
    let long = making(50, 80, style).expect("a long bar");
    let cells = |row: &Row| {
        row.text()
            .matches(glyphs.filled())
            .count()
            .saturating_add(row.text().matches(glyphs.hollow()).count())
    };

    let available = |columns: usize| {
        columns
            .saturating_sub(Working::gutter(glyphs))
            .saturating_sub(BAR_TAIL)
            .min(BAR_MAX)
    };
    assert_eq!(
        cells(&short),
        available(44) * BAR_NUMERATOR / BAR_DENOMINATOR
    );
    assert_eq!(
        cells(&long),
        available(80) * BAR_NUMERATOR / BAR_DENOMINATOR
    );
    assert!(
        cells(&long) > cells(&short),
        "{} / {}",
        short.text(),
        long.text()
    );

    for columns in 0..=80 {
        if let Some(row) = making(64, columns, style) {
            assert!(row.columns() <= columns, "{columns}: {}", row.text());
        }
    }
}

#[test]
fn a_window_with_no_room_for_the_row_keeps_the_turn_s_own_output_instead() {
    let turning = Turning::started(Breakdown::default());

    for room in 0..=ROWS {
        assert!(
            turning
                .laid(&nothing(), "", 80, Style::plain(), room)
                .is_empty(),
            "{room}"
        );
    }

    assert_eq!(
        turning
            .laid(&nothing(), "", 80, Style::plain(), ROWS + 1)
            .len(),
        ROWS
    );
}

#[test]
fn a_call_stands_over_the_row_for_as_long_as_its_tool_is_out() {
    // Here rather than in the transcript, because the mark on it moves: a
    // live row cannot also be a fixed record row. It is committed when the
    // tool answers and not before.
    let mut turning = Turning::started(Breakdown::default());
    let said = |turning: &Turning| {
        turning
            .laid(&nothing(), "", 80, Style::plain(), 24)
            .iter()
            .map(Row::text)
            .collect::<Vec<_>>()
    };

    assert!(!said(&turning).iter().any(|row| row.contains("Read")));

    turning.saw(&requested());
    let standing = said(&turning);

    assert_eq!(standing.len(), CALLING - 1, "{standing:?}");

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
    assert!(
        !standing.iter().any(|row| row.contains("ctrl+b")),
        "a read call advertised Bash's background action: {standing:?}"
    );
}

/// What a running call has printed, as an event.
fn printed(text: &str) -> Event {
    Event::Wrote {
        call: ToolId::new("a"),
        text: crucible_tools::Wrote::new(text),
    }
}

#[test]
fn a_command_shows_its_last_lines_and_says_how_many_there_have_been() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    for line in 1..=41 {
        turning.saw(&printed(&format!("Compiling crate-{line} v0.5.0\n")));
    }

    let rows: Vec<String> = turning
        .laid(&nothing(), "", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect();
    let sample: Vec<&String> = rows
        .iter()
        .filter(|row| row.contains("Compiling"))
        .collect();

    assert_eq!(sample.len(), SAMPLE, "{rows:?}");
    // The last of them, not the first: what a build is doing now is the
    // question, and the first five lines answered it a minute ago.
    assert!(
        sample.last().is_some_and(|row| row.contains("crate-41")),
        "{rows:?}"
    );
    assert!(
        sample.first().is_some_and(|row| row.contains("crate-37")),
        "{rows:?}"
    );

    // And the count row is what keeps five rows from reading as everything
    // the command has said. Indented with the sample, because it is a caption
    // on those rows rather than a row of its own — the one thing here a row
    // test can check and a reader would notice first.
    let counted = rows
        .iter()
        .find(|row| row.contains("41 lines"))
        .expect("the sample never said how much of it was not shown");

    assert!(counted.starts_with("    41 lines"), "{counted:?}");
    assert!(
        counted.contains(" B") || counted.contains("kB") || counted.contains("MB"),
        "the count never said how many bytes: {counted:?}"
    );
}

#[test]
fn a_native_web_call_does_not_offer_to_leave_it_running() {
    for name in ["web_search", "web_fetch"] {
        let mut turning = Turning::started(Breakdown::default());
        turning.saw(&requested_as(name, false));

        let rows = turning.laid(&nothing(), "", 80, Style::plain(), 24);
        assert!(
            !rows.iter().any(|row| row.text().contains("ctrl+b")),
            "{name} advertised an unavailable action: {:?}",
            rows.iter().map(Row::text).collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_row_under_a_call_offers_to_leave_it_running_before_it_has_printed_anything() {
    // A command silent for thirty-eight seconds is the one most worth putting
    // down, so the row that offers it cannot wait for output to justify
    // itself. It gains the counts in front of the offer once there are any.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested_as("bash", true));

    let rows = |turning: &Turning| {
        turning
            .laid(&nothing(), "", 80, Style::plain(), 24)
            .iter()
            .map(Row::text)
            .collect::<Vec<_>>()
    };

    let quiet = rows(&turning);
    assert!(
        quiet
            .iter()
            .any(|row| row.contains("(ctrl+b to background)")),
        "{quiet:?}"
    );
    assert!(
        !quiet.iter().any(|row| row.contains("lines")),
        "a command that has printed nothing was given a count: {quiet:?}"
    );

    turning.saw(&printed("Compiling one\n"));
    let printing = rows(&turning);
    let counted = printing
        .iter()
        .find(|row| row.contains("(ctrl+b to background)"))
        .expect("the offer went away when the command spoke");

    assert!(counted.contains("1 line"), "{counted:?}");
    assert!(counted.starts_with("    1 line"), "{counted:?}");
}

#[test]
fn what_a_command_printed_is_handed_back_when_its_tool_answers() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());
    turning.saw(&printed("Compiling one\n"));

    turning.saw(&Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::ok("done"),
        receipt: None,
    });

    let rows: Vec<String> = turning
        .laid(&nothing(), "", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect();

    assert!(
        !rows.iter().any(|row| row.contains("Compiling")),
        "the sample outlived the call it belonged to: {rows:?}"
    );
}

#[test]
fn a_window_short_of_rows_drops_the_sample_before_the_call_line() {
    // The order things give way. The sample is the one of them a second look
    // gets back whatever the window did, so it goes first.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());
    turning.saw(&printed("Compiling one\n"));

    let rows: Vec<String> = turning
        .laid(&nothing(), "", 80, Style::plain(), CALLING + 1)
        .iter()
        .map(Row::text)
        .collect();

    assert!(
        rows.iter().any(|row| row.contains("Read(src/main.rs)")),
        "the call line gave way before the sample: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("Compiling")),
        "the sample took room the call line needed: {rows:?}"
    );
}

#[test]
fn the_sample_is_on_the_value_the_loop_keys_a_redraw_on() {
    // Otherwise a command's output reaches the screen only on the frames
    // something else on the footing happens to change — a second at a time,
    // when the clock ticks.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());
    assert!(turning.moved());

    turning.saw(&printed("Compiling one\n"));
    assert!(
        turning.moved(),
        "output arrived and the footing did not think it had changed"
    );

    assert!(!turning.moved(), "a frame nobody could tell from the last");
}

#[test]
fn a_turn_that_ran_no_command_gets_no_frame_out_of_the_sample() {
    // Every turn ends, and the end empties the sample. A turn that never had
    // a command running must not be redrawn for that: the region is being
    // handed back at that moment, and a frame with nothing behind it scrolls
    // the terminal by a row nobody asked for.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&Event::Delta { text: "hi".into() });
    assert!(turning.moved());

    turning.saw(&Event::TurnFinished {
        turn: TurnId::FIRST,
        stop: StopReason::Yielded,
    });

    assert!(
        !turning.moved(),
        "the end of a turn with no command invented a frame"
    );
}

#[test]
fn a_line_rewritten_in_place_replaces_the_row_rather_than_adding_one() {
    // What a progress bar does: a carriage return and the line again. Kept as
    // one row, because that is what the terminal it was written for would do.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());
    turning.saw(&printed("Building [==>    ] 41/128\r"));
    turning.saw(&printed("Building [====>  ] 96/128\r"));

    let rows: Vec<String> = turning
        .laid(&nothing(), "", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect();
    let building: Vec<&String> = rows.iter().filter(|row| row.contains("Building")).collect();

    assert_eq!(building.len(), 1, "{rows:?}");
    assert!(
        building.first().is_some_and(|row| row.contains("96/128")),
        "{rows:?}"
    );
}

#[test]
fn the_call_line_comes_back_when_its_tool_answers_and_only_then() {
    let mut turning = Turning::started(Breakdown::default());

    assert!(turning.saw(&requested()).is_empty());
    assert!(turning.saw(&Event::Delta { text: "hi".into() }).is_empty());
    assert_eq!(
        lines(turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
            receipt: None,
        })),
        vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())]
    );

    // And once only. A second reading would commit the same line twice.
    assert!(
        turning
            .saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
                receipt: None,
            })
            .is_empty()
    );
}

#[test]
fn several_requested_calls_return_the_heading_named_by_each_result() {
    // One response can announce every call before any tool starts. Finish them
    // out of order to prove the result identity, not adjacency, selects the row.
    let mut turning = Turning::started(Breakdown::default());
    let requested = |id: &str, name: &str, about: &str| Event::ToolRequested {
        call: ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new("{}"),
        },
        summary: Summary::new(about),
        backgroundable: false,
        looking: None,
        alone: false,
    };

    turning.saw(&requested("read", "read", "src/main.rs"));
    turning.saw(&requested("fetch", "web_fetch", "https://example.com"));
    turning.saw(&requested("grep", "grep", "needle"));

    for (id, called) in [
        ("fetch", "WebFetch(https://example.com)"),
        ("grep", "Grep(needle)"),
        ("read", "Read(src/main.rs)"),
    ] {
        assert_eq!(
            lines(turning.saw(&Event::ToolFinished {
                call: ToolId::new(id),
                output: ToolOutput::ok("done"),
                receipt: None,
            })),
            vec![(ToolId::new(id), called.to_owned())]
        );
    }
}

#[test]
fn an_unknown_result_does_not_take_another_calls_heading() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    assert!(
        turning
            .saw(&Event::ToolFinished {
                call: ToolId::new("unknown"),
                output: ToolOutput::ok("done"),
                receipt: None,
            })
            .is_empty()
    );
    assert_eq!(
        lines(turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
            receipt: None,
        })),
        vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())]
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
        let mut turning = Turning::started(Breakdown::default());
        turning.saw(&requested());

        assert_eq!(
            lines(turning.saw(&ending)),
            vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())],
            "{ending:?}"
        );
    }
}

#[test]
fn a_terminal_event_drains_every_pending_call_in_request_order() {
    let mut turning = Turning::started(Breakdown::default());
    for (id, path) in [("first", "one"), ("second", "two"), ("third", "three")] {
        turning.saw(&Event::ToolRequested {
            call: ToolCall {
                id: ToolId::new(id),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            },
            summary: Summary::new(path),
            backgroundable: false,
            looking: None,
            alone: false,
        });
    }

    assert_eq!(
        lines(turning.saw(&Event::Failed {
            error: TurnError::Refused("stopped".into()),
        })),
        vec![
            (ToolId::new("first"), "Read(one)".to_owned()),
            (ToolId::new("second"), "Read(two)".to_owned()),
            (ToolId::new("third"), "Read(three)".to_owned()),
        ]
    );
    assert!(
        turning
            .saw(&Event::TurnFinished {
                turn: TurnId::FIRST,
                stop: StopReason::Cancelled,
            })
            .is_empty()
    );
}

#[test]
fn a_turn_asked_to_stop_still_lets_the_call_it_had_out_come_back() {
    // The word freezes at `interrupting` when the key is pressed. The line
    // of the call still out is not a word, and freezing it too would lose
    // the record of the call at the one moment there is most to explain.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());
    turning.interrupting();

    assert_eq!(
        lines(turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
            receipt: None,
        })),
        vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())]
    );
}

#[test]
fn the_dot_on_a_live_call_appears_and_disappears_in_the_readers_own_colour() {
    let style = Style::plain();
    let now = Instant::now();

    // Exact beats rather than sleeps: a stalled test process must not decide
    // which animation frames are compared.
    let face = |beat: u64| {
        let moment = Turning {
            since: now
                .checked_sub(Duration::from_millis(250 * beat))
                .expect("a clock past its own epoch"),
            ..Turning::started(Breakdown::default())
        };

        moment.call(
            &draw::Called::new("Read(src/main.rs)", crucible_tools::Argument::Path),
            80,
            style,
        )
    };
    let frames = (0..4).map(face).collect::<Vec<_>>();
    let dots = frames
        .iter()
        .map(|row| row.text().matches(style.glyphs().called()).count())
        .collect::<Vec<_>>();

    assert_eq!(dots, [1, 0, 1, 0]);
    // The live row a run is counted on wears the same mark in the same slot,
    // as the line the transcript settles it into does.
    let counting = Turning::started(Breakdown::default()).counted("Read 2 files", 80, style);
    assert_eq!(counting.kinds().next(), Some(Slot::Plain), "{counting:?}");
    let command_columns = frames
        .iter()
        .map(|row| {
            let text = row.text();
            let (before, _) = text.split_once("Read").expect("the command");
            crucible_tui::columns(before)
        })
        .collect::<Vec<_>>();
    assert!(
        command_columns
            .windows(2)
            .all(|pair| pair.first() == pair.last()),
        "the command shifted between frames: {command_columns:?}"
    );
    for row in &frames {
        assert!(row.text().ends_with("Read(src/main.rs)"), "{}", row.text());
        assert!(row.columns() <= 80, "{}", row.text());
        assert_eq!(row.kinds().next(), Some(Slot::Plain), "{row:?}");
    }

    for columns in 0..=20 {
        for beat in 0..4 {
            let row = Turning {
                since: now
                    .checked_sub(Duration::from_millis(250 * beat))
                    .expect("a clock past its own epoch"),
                ..Turning::started(Breakdown::default())
            }
            .call(
                &draw::Called::new("Read(a/very/long/path.rs)", crucible_tools::Argument::Path),
                columns,
                style,
            );
            assert!(row.columns() <= columns, "{columns}: {}", row.text());
        }
    }
}

#[test]
fn the_call_line_is_on_the_value_the_loop_keys_a_redraw_on() {
    // Left off it, a call would appear on screen only on the beat some
    // other segment happened to change -- so the line naming what is
    // running would arrive after the tool it names had already answered.
    let mut turning = Turning::started(Breakdown::default());
    turning.moved();

    turning.saw(&requested());
    assert!(turning.moved(), "the call appeared and the footing did not");

    turning.saw(&Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::ok("done"),
        receipt: None,
    });
    assert!(turning.moved(), "the call went and the footing did not");
}

/// A queue holding `lines`, as the box leaves one the moment each is finished
/// under a running turn.
fn waiting(lines: &[&str]) -> Prompts {
    let mut queue = Prompts::default();
    for line in lines {
        let mut editor = Editor::new();
        for key in line.chars() {
            editor.press(Key::Char(key));
        }
        queue.accept(&mut editor);
    }
    queue
}

/// Both bands of the footing over a queue holding `lines`, as plain text: the
/// turn's, laid out at `columns`, and what stands over the transcript, laid
/// out at `window`.
fn queueing(
    turning: &Turning,
    planning: &Planning,
    lines: &[&str],
    widths: Widths,
    room: usize,
) -> (Vec<String>, Vec<String>) {
    let (turn, over) = turning.rows(planning, "", &waiting(lines), widths, Style::plain(), room);
    (
        turn.iter().map(Row::text).collect(),
        over.iter().map(Row::text).collect(),
    )
}

/// The same width for the transcript and the window, as a window too narrow
/// for the rail lays them out.
const fn across(columns: usize) -> Widths {
    Widths {
        columns,
        window: columns,
    }
}

#[test]
fn prompts_waiting_stand_in_the_panel_under_the_row_saying_a_turn_is_running() {
    // The row and the panel are about what comes next rather than what the
    // turn has done, so both leave the turn's band: it ends in the blank that
    // parts the turn's output from them, and the working row heads the band
    // over the transcript with the panel's rule directly under it.
    let turning = Turning::started(Breakdown::default());
    let (turn, over) = queueing(
        &turning,
        &nothing(),
        &["fix the failing test", "and then commit"],
        across(80),
        24,
    );

    assert_eq!(turn, vec![String::new()], "{over:#?}");
    assert!(
        over.first()
            .is_some_and(|row| row.contains("esc to interrupt")),
        "{over:#?}"
    );
    assert_eq!(over.get(1), Some(&"\u{2500}".repeat(80)), "{over:#?}");
    assert!(
        over.contains(&"2 queued \u{b7} ctrl+enter to send all now".to_owned()),
        "{over:#?}"
    );
    assert!(
        over.contains(&"\u{203a} fix the failing test".to_owned()),
        "{over:#?}"
    );
    assert!(over.contains(&"  and then commit".to_owned()), "{over:#?}");
}

#[test]
fn the_panel_and_the_row_over_it_take_the_window_while_the_call_keeps_the_transcripts_width() {
    // The rail is the transcript's, so the call beside it is cut where the
    // transcript ends, and the rule over the queue runs from edge to edge of
    // the window, the way it does between turns where there is no rail.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());
    let (turn, over) = queueing(
        &turning,
        &nothing(),
        &["fix the failing test"],
        Widths {
            columns: 78,
            window: 80,
        },
        40,
    );

    assert!(turn.iter().any(|row| row.contains("Read")), "{turn:#?}");
    for row in &turn {
        assert!(crucible_tui::columns(row) <= 78, "{row:?} in {turn:#?}");
    }
    assert_eq!(over.get(1), Some(&"\u{2500}".repeat(80)), "{over:#?}");
}

#[test]
fn the_panel_ends_the_footing_so_the_box_stands_under_its_last_blank() {
    // The panel closes on a blank of its own, so the footing adds none: a
    // second would be a row of the window spent between the queue and the
    // box the keys it names are pressed in.
    let turning = Turning::started(Breakdown::default());
    let (_, over) = queueing(&turning, &nothing(), &["one", "two"], across(80), 24);

    let footer = over
        .iter()
        .position(|row| row.contains("to walk"))
        .unwrap_or_else(|| panic!("no footer in {over:#?}"));
    assert_eq!(over.len(), footer + 2, "{over:#?}");
    assert_eq!(over.last(), Some(&String::new()), "{over:#?}");
}

#[test]
fn a_plan_under_the_panel_keeps_the_blank_under_the_footing() {
    // The plan stands under the panel rather than being the panel's own, so
    // the footing's last row is the blank that parts it from the box.
    let turning = Turning::started(Breakdown::default());
    let (_, over) = queueing(&turning, &planned(2), &["one", "two"], across(80), 40);

    let footer = over
        .iter()
        .position(|row| row.contains("to walk"))
        .unwrap_or_else(|| panic!("no footer in {over:#?}"));
    let task = over
        .iter()
        .position(|row| row.contains("Task 0"))
        .unwrap_or_else(|| panic!("no plan in {over:#?}"));
    assert!(footer < task, "the plan stands under the panel: {over:#?}");
    assert_eq!(over.last(), Some(&String::new()), "{over:#?}");
}

#[test]
fn an_empty_queue_leaves_nothing_over_the_transcript() {
    // With nothing waiting the working row is the turn's last row again, at
    // the transcript's width, and the band over the transcript is empty.
    let turning = Turning::started(Breakdown::default());
    let (turn, over) = queueing(&turning, &nothing(), &[], across(80), 24);

    assert_eq!(turn.len(), ROWS, "{turn:#?}");
    assert!(over.is_empty(), "{over:#?}");
}

#[test]
fn no_row_of_the_footing_over_a_queue_is_drawn_past_the_last_column() {
    // The mark that says a line was cut is columns of the row rather than
    // columns past it, and the ascii set spells it with three -- so a row
    // that reserved one column for it would be committed two past the
    // window, and the terminal would wrap it into a row nothing counted.
    let queue = waiting(&["fix the failing test"]);
    for wide in [0, 1, 2, 3, 5, 6, 7, 8, 20, 80] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let style = Style::drawn(glyphs);
            let turning = Turning::started(Breakdown::default());
            let (turn, over) = turning.rows(&nothing(), "", &queue, across(wide), style, 40);

            for row in turn.iter().chain(&over) {
                let said = row.text();
                assert!(
                    crucible_tui::columns(&said) <= wide,
                    "{wide} {glyphs:?}: {said:?}"
                );
            }
        }
    }
}

#[test]
fn a_window_too_short_for_all_three_drops_the_call_before_the_waiting_prompts() {
    // In that order, because that is the order they stop being worth the
    // room. The call joins the transcript the moment its tool answers and
    // the prompts are still in the queue with their own turns to come; the
    // row saying a turn is running exists nowhere else, so it goes last.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let said = |room: usize| {
        let (turn, over) = queueing(
            &turning,
            &nothing(),
            &["fix the failing test"],
            across(80),
            room,
        );
        [turn, over].concat().concat()
    };

    // Room for all three: the call, the panel, and the row saying a turn
    // is running.
    let whole = said(40);
    assert!(whole.contains("Read"), "{whole:?}");
    assert!(whole.contains("1 queued"), "{whole:?}");
    assert!(whole.contains("running"), "{whole:?}");

    // A window short of rows drops the call first and keeps the panel.
    let shorter = said(ROWS + 10);
    assert!(!shorter.contains("Read"), "{shorter:?}");
    assert!(shorter.contains("1 queued"), "{shorter:?}");

    // And the panel gives way before the row that says a turn is running.
    let shortest = said(ROWS + 1);
    assert!(!shortest.contains("queued"), "{shortest:?}");
    assert!(shortest.contains("running"), "{shortest:?}");
}

#[test]
fn a_window_too_short_for_both_drops_the_call_before_the_row() {
    // The call joins the transcript the moment its tool answers, so a window
    // that drops it loses nothing a second look does not return.
    // The row saying a turn is running exists nowhere else.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let rows = turning.laid(&nothing(), "", 80, Style::plain(), CALLING - 1);
    let said = rows.iter().map(Row::text).collect::<String>();

    assert_eq!(rows.len(), ROWS, "{said:?}");
    assert!(said.contains("running"), "{said:?}");
    assert!(!said.contains("Read"), "{said:?}");
}

#[test]
fn the_plan_stands_under_everything_the_turn_says_and_over_the_box() {
    // The only place it can go. What it stands under is the turn — the call
    // out, the row saying one is running and the prompts behind it — and
    // what it stands over is the line being typed while that happens. The
    // blank at the end parts it from the box.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let (turn, over) = queueing(
        &turning,
        &planned(3),
        &["fix the failing test"],
        across(80),
        40,
    );
    let said = [turn, over.clone()].concat().join("\n");

    assert!(said.contains("Task 0"), "{said:?}");
    assert!(said.find("Read") < said.find("Task 0"), "{said:?}");
    assert!(said.find("running") < said.find("Task 0"), "{said:?}");
    assert!(said.find("1 queued") < said.find("Task 0"), "{said:?}");
    assert_eq!(over.last().map(String::as_str), Some(""), "{said:?}");
}

#[test]
fn a_window_short_of_rows_drops_the_call_and_the_waiting_prompts_before_a_task() {
    // What measuring the plan first buys. The call line and the panel naming
    // the prompts behind the turn are the two measured against what the plan
    // left, so they are the two a short window drops on its behalf: a call
    // joins the transcript the moment its tool answers and a queued prompt
    // has its own turn coming, while what the agent is working to is on
    // screen nowhere else.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let planning = planned(3);
    let plan = planning.rows(80, 40, Style::plain().glyphs()).len();

    let said = |room: usize| {
        let (turn, over) = queueing(
            &turning,
            &planning,
            &["fix the failing test"],
            across(80),
            room,
        );
        [turn, over].concat().concat()
    };

    // Room for all of it: the call, the queue's panel, and the plan.
    let whole = said(plan + 16);
    assert!(whole.contains("Read"), "{whole:?}");
    assert!(whole.contains("1 queued"), "{whole:?}");
    assert!(whole.contains("Task 2"), "{whole:?}");

    // A window short of rows drops the call before the panel, and keeps both
    // the queue and the plan, which are on screen nowhere else.
    let shorter = said(plan + 12);
    assert!(!shorter.contains("Read"), "{shorter:?}");
    assert!(shorter.contains("1 queued"), "{shorter:?}");
    assert!(shorter.contains("Task 2"), "{shorter:?}");

    // And the panel gives way before the plan does, for the same reason the
    // call does: a queued prompt has its own turn coming to say it.
    let shortest = said(plan + 4);
    assert!(!shortest.contains("queued"), "{shortest:?}");
    assert!(shortest.contains("Task 2"), "{shortest:?}");
}

#[test]
fn a_run_of_calls_that_only_looked_around_stands_over_the_call_that_is_out() {
    // Two rows, because they answer two questions: what the turn has been
    // doing, and what it is doing at this instant. The run goes on top
    // because it is what has already happened, and the transcript below the
    // footing grows the same way down.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let rows: Vec<String> = turning
        .laid(&nothing(), "Reading 4 files", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect();

    let run = rows
        .iter()
        .position(|row| row.contains("Reading 4 files"))
        .unwrap_or_else(|| panic!("the run went unsaid: {rows:?}"));
    let call = rows
        .iter()
        .position(|row| row.contains("Read(src/main.rs)"))
        .unwrap_or_else(|| panic!("the call went unsaid: {rows:?}"));

    assert_eq!(run + 1, call, "{rows:?}");
}

#[test]
fn a_footing_with_no_run_going_stands_no_row_for_one() {
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let alone = turning.laid(&nothing(), "", 80, Style::plain(), 24).len();
    let over = turning
        .laid(&nothing(), "Reading 4 files", 80, Style::plain(), 24)
        .len();

    assert_eq!(alone + 1, over, "{alone} {over}");
}

#[test]
fn a_run_still_says_itself_between_the_calls_in_it() {
    // One tool has answered and the next has not gone out yet. The run is
    // still going, and this row is the only thing on the screen saying so.
    let rows: Vec<String> = Turning::started(Breakdown::default())
        .laid(&nothing(), "Reading 4 files", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect();

    assert!(
        rows.iter().any(|row| row.contains("Reading 4 files")),
        "{rows:?}"
    );
}

#[test]
fn the_run_wears_the_same_mark_as_the_call_beneath_it() {
    // Which is the mark that appears and disappears on the turn's beat. Both
    // are read out of one layout, so both are of the same instant: a run
    // blinking against the call under it would read as two turns rather than
    // one turn doing two things.
    let mut turning = Turning::started(Breakdown::default());
    turning.saw(&requested());

    let rows: Vec<String> = turning
        .laid(&nothing(), "Reading 4 files", 80, Style::plain(), 24)
        .iter()
        .map(Row::text)
        .collect();

    let mark = |said: &str| {
        rows.iter()
            .find(|row| row.contains(said))
            .and_then(|row| row.chars().next())
            .unwrap_or_else(|| panic!("{said} went unsaid: {rows:?}"))
    };

    assert_eq!(
        mark("Reading 4 files"),
        mark("Read(src/main.rs)"),
        "{rows:?}"
    );
}

#[test]
fn a_failed_lookup_settles_as_an_individual_call() {
    let mut turning = Turning::started(Breakdown::default());
    let mut requested = requested();
    if let Event::ToolRequested { looking, .. } = &mut requested {
        *looking = Some(Looking::File);
    }
    turning.saw(&requested);
    let returned = turning.saw(&Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::failed("file missing"),
        receipt: None,
    });
    assert_eq!(returned.len(), 1);
    assert!(returned.first().unwrap().looking.is_none());
}

#[test]
fn usage_a_turn_keeps_what_it_last_reported_for_the_panel_over_it() {
    // `/usage` stands over a running turn while the runner is away on it, so
    // the footing keeps what it started from and then what the turn posts.
    use crucible_types::{Window, WindowReading};
    use std::time::{Duration, UNIX_EPOCH};

    let windows = |percent| {
        crucible_types::PlanWindows::new(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .with(Window::Weekly, WindowReading::new(percent, None))
    };
    let seeded = Totals::new();
    let mut turning = Turning::started(Breakdown::default()).using(seeded, Some(windows(10)));
    assert_eq!(turning.totals(), seeded);
    assert_eq!(turning.limits(), Some(windows(10)));

    let posted = Totals::new();
    turning.saw(&Event::Used { totals: posted });
    turning.saw(&Event::PlanLimits {
        windows: windows(42),
    });

    assert_eq!(turning.totals(), posted);
    assert_eq!(turning.limits(), Some(windows(42)));
}

#[test]
fn the_footing_over_a_queue_follows_the_colour_rule() {
    // The panel brings its own accents — the rule and the highlighted line —
    // and the working row over it keeps its one, at either width the rows
    // over the transcript are laid out at.
    for lines in [
        &["fix the failing test"][..],
        &["one", "two", "three", "four", "five", "six", "seven"][..],
    ] {
        for columns in [80, 40] {
            let turning = Turning::started(Breakdown::default());
            let (mut rows, over) = turning.rows(
                &nothing(),
                "",
                &waiting(lines),
                across(columns),
                Style::plain(),
                24,
            );
            rows.extend(over);

            crate::cli::colour_rule::holds(
                &format!("queue at {columns}"),
                &rows,
                crate::cli::colour_rule::marked,
            );
        }
    }
}

#[test]
fn a_command_that_answers_before_the_delay_is_never_drawn_over_the_row() {
    // Most commands are over in a fraction of a second, and a row put up for
    // each and taken down a moment later is the screen blinking on every
    // call. The footing at every instant of this one's life is what the
    // reader would have seen, and the call is in none of them: the word on
    // the row says `running`, and the band stays the three rows it was.
    let mut turning = Turning::started(Breakdown::default()).pinning(PINNED);

    turning.saw(&requested_as("bash", true));
    let asked = footing(&turning);
    turning.saw(&printed("Compiling one\n"));
    let printing = footing(&turning);

    for frame in [&asked, &printing] {
        assert_eq!(
            frame.len(),
            ROWS,
            "a call younger than the delay was drawn: {frame:?}"
        );
        assert!(
            !frame
                .iter()
                .any(|row| row.contains("Bash") || row.contains("ctrl+b")),
            "{frame:?}"
        );
    }
    let settled = turning.saw(&Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::ok("done"),
        receipt: None,
    });
    assert_eq!(
        settled.len(),
        1,
        "the call was not handed to the transcript"
    );
    assert_eq!(footing(&turning).len(), ROWS);
}

#[test]
fn a_command_still_running_past_the_delay_stands_over_the_row_with_its_key() {
    let mut turning = Turning::started(Breakdown::default()).pinning(PINNED);
    turning.saw(&requested_as("bash", true));
    turning.saw(&printed("Compiling one\n"));
    assert!(turning.moved(), "the first frame was never drawn");

    aged(&mut turning);

    // The frame it comes due on is one the loop sees as new, so it is drawn
    // at the delay rather than whenever something else next happens.
    assert!(turning.moved(), "the call came due and the footing did not");
    assert!(
        turning.can_background(),
        "the offer stood and the key was dead"
    );

    let frame = footing(&turning);
    assert!(
        frame.iter().any(|row| row.contains("Bash(src/main.rs)")),
        "{frame:?}"
    );
    assert!(
        frame
            .iter()
            .any(|row| row.contains("1 line") && row.contains("(ctrl+b to background)")),
        "{frame:?}"
    );
}

#[test]
fn a_command_can_be_backgrounded_before_its_row_is_drawn() {
    // The delay is about drawing, not about the key. Somebody who knows a
    // command will run long presses ctrl+b the moment it starts, and the row
    // offering it is not up yet; the press still reaches the call. The hint
    // waits for the row all the same.
    let mut early = Turning::started(Breakdown::default()).pinning(PINNED);
    early.saw(&requested_as("bash", true));
    let frame = footing(&early);
    assert!(
        early.can_background(),
        "ctrl+b did nothing before the row was drawn"
    );
    assert_eq!(frame.len(), ROWS, "the row was drawn early: {frame:?}");
    assert!(!frame.iter().any(|row| row.contains("ctrl+b")), "{frame:?}");

    // What the transcript is handed once the command lets go is what it is
    // handed when the key is pressed under the drawn row, and the band goes
    // back to the rows it was.
    let mut late = Turning::started(Breakdown::default()).pinning(PINNED);
    late.saw(&requested_as("bash", true));
    aged(&mut late);
    assert!(late.can_background());

    let left = || Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::ok("Left running in the background"),
        receipt: None,
    };
    assert_eq!(lines(early.saw(&left())), lines(late.saw(&left())));
    assert_eq!(footing(&early).len(), ROWS);
    assert!(!early.can_background());
}
