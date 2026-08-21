//! The live turn footing, its calls, output, queue, and plan.

use crucible_core::{
    Spend, StopReason, Summary, ToolArgs, ToolCall, ToolId, ToolOutput, TurnError, TurnId,
};
use crucible_tui::{Glyphs, Palette};

use super::*;

/// No plan at all, which is what a session has until the agent writes one
/// and is what every test here but the last two is about.
fn nothing() -> Planning {
    Planning::new(crucible_tools::Plan::new())
}

#[test]
fn an_empty_queue_adds_no_row_to_the_footing() {
    // The panel is for a queue that has something in it. With nothing
    // waiting, the footing is the same three rows it has always been — the
    // blank, the word, the blank — and not one row taller for a frame around
    // nothing.
    let rows = Turning::started().rows(&nothing(), 80, Style::plain(), 24);
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

    let plan = crucible_tools::Plan::new();
    plan.replay(&ToolArgs::new(format!(r#"{{"tasks":[{said}]}}"#)));

    Planning::new(plan)
}

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
fn a_response_being_asked_for_again_says_so_until_the_new_one_speaks() {
    // The span it covers is the whole of the second ask — the pause and the
    // request after it — and `thinking` over that span would be a row saying
    // the first answer is still on its way.
    let mut turning = Turning::started();
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
    let mut turning = Turning::started();
    turning.interrupting();
    turning.saw(&Event::Delta { text: "hi".into() });

    assert_eq!(turning.doing.word(), "interrupting");

    // And stops offering the key that has already been pressed.
    let rows = turning.rows(&nothing(), 80, Style::plain(), 24);
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
            .rows(&nothing(), 80, Style::plain(), 24)
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
fn the_window_left_is_a_row_of_its_own_above_the_box_not_on_the_working_row() {
    // The one number that moves while a turn runs stands where it stands
    // between turns — its own row, directly over the box — rather than
    // against the end of the row the word is on.
    let mut turning = Turning::started();
    turning.saw(&Event::Carried { left: Some(72) });

    let rows = turning.rows(&nothing(), 80, Style::plain(), 24);
    let texts: Vec<String> = rows.iter().map(Row::text).collect();

    let working = texts
        .iter()
        .find(|row| row.contains("thinking") || row.contains("writing"))
        .expect("a working row");
    assert!(!working.contains('%'), "the working row: {working:?}");

    let own = texts
        .iter()
        .find(|row| row.contains("72% window left"))
        .expect("a window-left row");
    assert!(
        own.trim_end().ends_with("72% window left"),
        "right-aligned on its own row: {own:?}"
    );
    assert!(
        !own.contains("thinking") && !own.contains("writing"),
        "a row of its own: {own:?}"
    );
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
fn the_bar_moves_on_the_notes_rather_than_on_whatever_else_changes() {
    // The bar is a segment of the row, so it belongs to the value the loop
    // keys a redraw on. Left out of it, it reaches the screen only when
    // something else on the row happens to change with it — and on a
    // request that draws nothing else for a minute, the something else is
    // the clock.
    let mut turning = Turning::started();
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
fn a_window_with_no_room_for_the_row_keeps_the_turn_s_own_output_instead() {
    let turning = Turning::started();

    for room in 0..=ROWS {
        assert!(
            turning
                .rows(&nothing(), 80, Style::plain(), room)
                .is_empty(),
            "{room}"
        );
    }

    assert_eq!(
        turning.rows(&nothing(), 80, Style::plain(), ROWS + 1).len(),
        ROWS
    );
}

#[test]
fn a_call_stands_over_the_row_for_as_long_as_its_tool_is_out() {
    // Here rather than in the transcript, because the mark on it moves: a
    // live row cannot also be a fixed record row. It is committed when the
    // tool answers and not before.
    let mut turning = Turning::started();
    let said = |turning: &Turning| {
        turning
            .rows(&nothing(), 80, Style::plain(), 24)
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
    // Directly under the call with no blank between them: it is a caption on
    // the call rather than a second thing beside it.
    assert!(at(2).contains("(ctrl+b to background)"), "{standing:?}");
    assert!(at(3).is_empty(), "{standing:?}");
    assert!(at(4).contains("running"), "{standing:?}");
    assert!(at(5).is_empty(), "{standing:?}");
}

/// What a running call has printed, as an event.
fn printed(text: &str) -> Event {
    Event::Wrote {
        call: ToolId::new("a"),
        text: crucible_core::Wrote::new(text),
    }
}

#[test]
fn a_command_shows_its_last_lines_and_says_how_many_there_have_been() {
    let mut turning = Turning::started();
    turning.saw(&requested());

    for line in 1..=41 {
        turning.saw(&printed(&format!("Compiling crate-{line} v0.5.0\n")));
    }

    let rows: Vec<String> = turning
        .rows(&nothing(), 80, Style::plain(), 24)
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
fn the_row_under_a_call_offers_to_leave_it_running_before_it_has_printed_anything() {
    // A command silent for thirty-eight seconds is the one most worth putting
    // down, so the row that offers it cannot wait for output to justify
    // itself. It gains the counts in front of the offer once there are any.
    let mut turning = Turning::started();
    turning.saw(&requested());

    let rows = |turning: &Turning| {
        turning
            .rows(&nothing(), 80, Style::plain(), 24)
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
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.saw(&printed("Compiling one\n"));

    turning.saw(&Event::ToolFinished {
        call: ToolId::new("a"),
        output: ToolOutput::ok("done"),
    });

    let rows: Vec<String> = turning
        .rows(&nothing(), 80, Style::plain(), 24)
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
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.saw(&printed("Compiling one\n"));

    let rows: Vec<String> = turning
        .rows(&nothing(), 80, Style::plain(), CALLING + 1)
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
    let mut turning = Turning::started();
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
    let mut turning = Turning::started();
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
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.saw(&printed("Building [==>    ] 41/128\r"));
    turning.saw(&printed("Building [====>  ] 96/128\r"));

    let rows: Vec<String> = turning
        .rows(&nothing(), 80, Style::plain(), 24)
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
    let mut turning = Turning::started();

    assert!(turning.saw(&requested()).is_empty());
    assert!(turning.saw(&Event::Delta { text: "hi".into() }).is_empty());
    assert_eq!(
        turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
        }),
        vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())]
    );

    // And once only. A second reading would commit the same line twice.
    assert!(
        turning
            .saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            })
            .is_empty()
    );
}

#[test]
fn several_requested_calls_return_the_heading_named_by_each_result() {
    // One response can announce every call before any tool starts. Finish them
    // out of order to prove the result identity, not adjacency, selects the row.
    let mut turning = Turning::started();
    let requested = |id: &str, name: &str, about: &str| Event::ToolRequested {
        call: ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new("{}"),
        },
        summary: Summary::new(about),
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
            turning.saw(&Event::ToolFinished {
                call: ToolId::new(id),
                output: ToolOutput::ok("done"),
            }),
            vec![(ToolId::new(id), called.to_owned())]
        );
    }
}

#[test]
fn an_unknown_result_does_not_take_another_calls_heading() {
    let mut turning = Turning::started();
    turning.saw(&requested());

    assert!(
        turning
            .saw(&Event::ToolFinished {
                call: ToolId::new("unknown"),
                output: ToolOutput::ok("done"),
            })
            .is_empty()
    );
    assert_eq!(
        turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
        }),
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
        let mut turning = Turning::started();
        turning.saw(&requested());

        assert_eq!(
            turning.saw(&ending),
            vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())],
            "{ending:?}"
        );
    }
}

#[test]
fn a_terminal_event_drains_every_pending_call_in_request_order() {
    let mut turning = Turning::started();
    for (id, path) in [("first", "one"), ("second", "two"), ("third", "three")] {
        turning.saw(&Event::ToolRequested {
            call: ToolCall {
                id: ToolId::new(id),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            },
            summary: Summary::new(path),
        });
    }

    assert_eq!(
        turning.saw(&Event::Failed {
            error: TurnError::Refused("stopped".into()),
        }),
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
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.interrupting();

    assert_eq!(
        turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
        }),
        vec![(ToolId::new("a"), "Read(src/main.rs)".to_owned())]
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
    let palette = Palette::resolve(true, Theme::Dark, None, &|name| {
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

        moment.call("Read(src/main.rs)", 80, style)
    };
    let (lit, dim) = (face(Duration::ZERO), face(Duration::from_millis(250)));

    assert_ne!(
        lit.paint(&palette),
        dim.paint(&palette),
        "the mark did not pulse"
    );

    // What the row says rather than what it is painted as, because the words
    // are two spans now — the tool's name in the accent and its arguments in
    // the quieter colour — and a sequence between them is bytes rather than
    // a column the words moved by.
    for face in [&lit, &dim] {
        assert!(
            face.text().ends_with("Read(src/main.rs)"),
            "{}",
            face.text()
        );
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

/// A turn with `lines` waiting behind it, drawn to a picture.
///
/// One place the queue is filled and the footing read, so the cases below
/// are about what the panel says rather than how it is fed.
fn queueing(lines: &[&str], columns: usize, room: usize) -> Vec<String> {
    let mut turning = Turning::started();
    turning.queueing(lines.iter().copied(), columns, Style::plain());

    turning
        .rows(&nothing(), columns, Style::plain(), room)
        .iter()
        .map(Row::text)
        .collect()
}

#[test]
fn prompts_finished_while_the_turn_runs_are_named_in_a_box() {
    // The gap this closes: Return during a turn takes the line out of the
    // box, and until the panel nothing on the screen said where it went —
    // and a second or third line was nowhere at all, since only the front
    // one was ever named. The next acknowledgement a line gets is its own
    // turn starting, which is however long the turn in front of it takes.
    let said = queueing(&["fix the failing test", "and then commit"], 80, 24);
    let whole = said.join("\n");

    // Both are named, each led by the mark a line is typed after, inside a
    // frame whose top edge carries the count.
    assert!(whole.contains("2 queued"), "{whole}");
    assert!(whole.contains("› fix the failing test"), "{whole}");
    assert!(whole.contains("› and then commit"), "{whole}");
}

#[test]
fn every_row_of_the_panel_ends_in_the_column_the_box_below_it_ends_in() {
    // The defect this pins: the rows between the borders were padded to the
    // width the left border was already inside, so each of them closed a
    // column short of the top and bottom edges and the right-hand side of
    // the frame stepped in and out. It is read directly above the box, so
    // the column both of them close in is the same column.
    for columns in [Prompt::FRAMED_AT, 40, 80] {
        let said = queueing(&["one", "two longer than the first"], columns, 24);
        let opens = said
            .iter()
            .position(|row| row.starts_with('\u{256d}'))
            .unwrap_or_else(|| panic!("{columns}: no frame in {said:?}"));
        let closes = said
            .iter()
            .position(|row| row.starts_with('\u{2570}'))
            .unwrap_or_else(|| panic!("{columns}: the frame never closes in {said:?}"));
        let panel = said.get(opens..=closes).unwrap_or_default();

        for row in panel {
            assert_eq!(
                crucible_tui::columns(row),
                columns,
                "{columns}: {row:?} in {said:?}"
            );
        }

        // A blank parts the frame from the working row above it: a box is a
        // thing of its own rather than a second line of the row it stands
        // under.
        assert!(
            opens.checked_sub(1).and_then(|above| said.get(above)) == Some(&String::new()),
            "{said:?}"
        );
    }
}

#[test]
fn a_queue_longer_than_the_panel_compacts_the_rest_to_a_count() {
    // Three are named and the rest are a row that says how many and where
    // they are: a full queue cannot push the box off the screen, and a
    // count that is not the count is worse than none.
    let said = queueing(&["one", "two", "three", "four", "five"], 80, 24);
    let whole = said.join("\n");

    assert!(whole.contains("5 queued"), "{whole}");
    assert!(whole.contains("… +2 more"), "{whole}");
    assert!(whole.contains("ctrl+q"), "{whole}");
    assert!(!whole.contains("four"), "{whole}");
}

#[test]
fn an_empty_queue_draws_no_panel() {
    // Absent rather than blank. A frame around nothing is rows of the
    // window spent saying nothing, spent against the turn's own output.
    let said = queueing(&[], 80, 24);
    assert!(!said.join("\n").contains("queued"), "{}", said.join("\n"));
}

#[test]
fn a_turn_with_nothing_waiting_behind_it_draws_no_row_for_it() {
    // Absent rather than blank. A row that says nothing is a row of the
    // window spent, and what it is spent against is the turn's own output
    // above it.
    let turning = Turning::started();
    let rows = turning.rows(&nothing(), 80, Style::plain(), 24);

    assert_eq!(rows.len(), ROWS, "{:?}", rows.iter().map(Row::text));
}

#[test]
fn a_prompt_wider_than_the_panel_is_cut_at_the_right() {
    // Cut rather than wrapped: the footing owns a fixed band, and a height
    // that depended on how much somebody typed would take rows from the
    // transcript one keystroke at a time.
    let long = "a".repeat(200);
    let said = queueing(&[&long], 40, 24);

    let named = said
        .iter()
        .find(|row| row.contains("aaa"))
        .cloned()
        .unwrap_or_default();

    assert!(named.contains('…'), "{named:?}");
    assert!(crucible_tui::columns(&named) <= 40, "{named:?}");
}

#[test]
fn what_is_held_of_a_waiting_prompt_is_a_row_of_it_rather_than_all_of_it() {
    // It is cloned into the value the redraw is keyed on, sixty times a
    // second, and the box lets a prompt reach a megabyte. Cutting it where
    // it is taken is what keeps that clone the size of a row.
    let mut turning = Turning::started();
    let long = "a".repeat(1024 * 1024);
    turning.queueing([long.as_str()].into_iter(), 80, Style::plain());

    let held = turning.queued.lines.first().cloned().unwrap_or_default();
    assert!(crucible_tui::columns(&held) <= 80, "{}", held.len());
}

#[test]
fn the_prompt_waiting_is_on_the_value_the_loop_keys_a_redraw_on() {
    // Left off it, a line finished into the queue would reach the screen
    // on the beat some other segment happened to change -- a box emptied by
    // Return with nothing anywhere saying the line was kept, for as long as
    // a quarter of a second after the press.
    let mut turning = Turning::started();
    turning.moved();

    turning.queueing(["fix the failing test"].into_iter(), 80, Style::plain());
    assert!(
        turning.moved(),
        "the prompt appeared and the footing did not"
    );

    turning.queueing(std::iter::empty(), 80, Style::plain());
    assert!(turning.moved(), "the prompt went and the footing did not");
}

#[test]
fn the_row_naming_a_waiting_prompt_is_never_drawn_past_the_last_column() {
    // The mark that says a line was cut is columns of the row rather than
    // columns past it, and the ascii set spells it with three -- so a row
    // that reserved one column for it would be committed two past the
    // window, and the terminal would wrap it into a row nothing counted.
    for wide in [0, 1, 2, 3, 5, 6, 7, 8, 20, 80] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let style = Style::drawn(glyphs);
            let mut turning = Turning::started();
            turning.queueing(["fix the failing test"].into_iter(), wide, style);

            for row in turning.rows(&nothing(), wide, style, 40) {
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
fn a_window_too_short_for_all_three_drops_the_call_before_the_waiting_prompt() {
    // In that order, because that is the order they stop being worth the
    // room. The call joins the transcript the moment its tool answers and
    // the prompt is still in the queue with its own turn to come; the row
    // saying a turn is running exists nowhere else, so it goes last.
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.queueing(["fix the failing test"].into_iter(), 80, Style::plain());

    let said = |room: usize| {
        turning
            .rows(&nothing(), 80, Style::plain(), room)
            .iter()
            .map(Row::text)
            .collect::<Vec<_>>()
    };

    // Room for all three: the call, the panel, and the row saying a turn
    // is running.
    let whole = said(40);
    assert!(whole.concat().contains("Read"), "{whole:?}");
    assert!(whole.concat().contains("1 queued"), "{whole:?}");
    assert!(whole.concat().contains("running"), "{whole:?}");

    // A window short of rows drops the call first — it joins the transcript
    // the moment its tool answers — and keeps the panel, which
    // is still in the queue with its own turn to come.
    let shorter = said(ROWS + 6);
    assert!(!shorter.concat().contains("Read"), "{shorter:?}");
    assert!(shorter.concat().contains("1 queued"), "{shorter:?}");

    // And the panel gives way before the row that says a turn is running:
    // that row exists nowhere else, so it is the last thing to go.
    let shortest = said(ROWS + 1);
    assert!(!shortest.concat().contains("queued"), "{shortest:?}");
    assert!(shortest.concat().contains("running"), "{shortest:?}");
}

#[test]
fn a_window_too_short_for_both_drops_the_call_before_the_row() {
    // The call joins the transcript the moment its tool answers, so a window
    // that drops it loses nothing a second look does not return.
    // The row saying a turn is running exists nowhere else.
    let mut turning = Turning::started();
    turning.saw(&requested());

    let rows = turning.rows(&nothing(), 80, Style::plain(), CALLING);
    let said = rows.iter().map(Row::text).collect::<String>();

    assert_eq!(rows.len(), ROWS, "{said:?}");
    assert!(said.contains("running"), "{said:?}");
    assert!(!said.contains("Read"), "{said:?}");
}

#[test]
fn the_plan_stands_under_everything_the_turn_says_and_over_the_box() {
    // The only place it can go. What it stands under is the turn — the call
    // out and the row saying one is running — and what it stands over is
    // the line being typed while that happens. The blank at the end parts
    // it from the box, so the panel is the last thing above one.
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.queueing(["fix the failing test"].into_iter(), 80, Style::plain());

    let rows = turning.rows(&planned(3), 80, Style::plain(), 40);
    let said = rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");

    assert!(said.contains("Task 0"), "{said:?}");
    assert!(said.find("Read") < said.find("Task 0"), "{said:?}");
    assert!(said.find("running") < said.find("Task 0"), "{said:?}");
    assert!(said.find("Next:") < said.find("Task 0"), "{said:?}");
    assert_eq!(rows.last().map(Row::text).as_deref(), Some(""), "{said:?}");
}

#[test]
fn a_window_short_of_rows_drops_the_call_and_the_waiting_prompt_before_a_task() {
    // What measuring the panel first buys. The call line and the row naming
    // the prompt behind the turn are the two measured against what the plan
    // left, so they are the two a narrow window drops on its behalf: a call
    // joins the transcript the moment its tool answers and a queued
    // prompt has its own turn coming, while what the agent is working to is
    // on screen nowhere else.
    let mut turning = Turning::started();
    turning.saw(&requested());
    turning.queueing(["fix the failing test"].into_iter(), 80, Style::plain());

    let planning = planned(3);
    let panel = planning.rows(80, 40, Style::plain().glyphs()).len();

    let said = |room: usize| {
        turning
            .rows(&planning, 80, Style::plain(), room)
            .iter()
            .map(Row::text)
            .collect::<String>()
    };

    // Room for all of it: the call, the queue panel, and the plan.
    let whole = said(panel + 12);
    assert!(whole.contains("Read"), "{whole:?}");
    assert!(whole.contains("1 queued"), "{whole:?}");
    assert!(whole.contains("Task 2"), "{whole:?}");

    // A window short of rows drops the call before the panel — the call joins
    // the transcript the moment its tool answers — and keeps both
    // the queue and the plan, which are on screen nowhere else.
    let shorter = said(panel + 8);
    assert!(!shorter.contains("Read"), "{shorter:?}");
    assert!(shorter.contains("1 queued"), "{shorter:?}");
    assert!(shorter.contains("Task 2"), "{shorter:?}");

    // And the panel gives way before the plan does, for the same reason the
    // call does: a queued prompt has its own turn coming to say it.
    let shortest = said(panel + 4);
    assert!(!shortest.contains("queued"), "{shortest:?}");
    assert!(shortest.contains("Task 2"), "{shortest:?}");
}
