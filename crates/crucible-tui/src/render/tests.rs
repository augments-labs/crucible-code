//! What the renderer draws, asserted against a recording terminal.

use unicode_width::UnicodeWidthStr;

use super::*;
use crate::color::{Palette, Slot};
use crate::row::Row;
use crate::terminal::Recording;

/// The escape a frame starts with when `rows` rows are already on screen.
/// Written out literally rather than borrowed from `frame`, so a change
/// there has to be asserted here too.
fn rewind(rows: usize) -> String {
    match rows {
        0 | 1 => "\r\x1b[J".to_owned(),
        n => format!("\r\x1b[{}A\x1b[J", n - 1),
    }
}

#[test]
fn the_first_frame_does_not_move_the_cursor_up() {
    // There is nothing above it yet, and moving up would eat a line the
    // shell printed.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hello").unwrap();

    assert_eq!(render.terminal.written(), format!("{}hello", rewind(0)));
}

#[test]
fn a_second_frame_erases_the_first() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hel").unwrap();
    render.terminal.take();

    render.stream("lo").unwrap();

    assert_eq!(render.terminal.written(), format!("{}hello", rewind(1)));
}

#[test]
fn a_frame_is_one_write_and_one_flush() {
    // The burst budget is about frames, not bytes: a redraw that wrote row
    // by row would tear on a slow terminal and cost a syscall each.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("one\ntwo\nthree").unwrap();

    assert_eq!(render.terminal.flushes(), 1);
}

#[test]
fn nothing_ever_erases_upward() {
    // The property that makes scrollback safe. If one of these appears,
    // committed output is reachable and the design is gone.
    let mut render = Renderer::new(Recording::new(20, 3));
    for turn in 0..200 {
        render.stream(&format!("line {turn}\n")).unwrap();
    }

    let written = render.terminal.written();
    for upward in ["\x1b[2J", "\x1b[1J", "\x1b[3J", "\x1b[H"] {
        assert!(
            !written.contains(upward),
            "the renderer wrote {upward:?}, which can reach scrollback"
        );
    }
}

#[test]
fn rows_pushed_out_of_the_tail_are_written_once() {
    // Committed rows must appear exactly once in the byte stream. Twice
    // means the tail redrew something it had already let go of.
    let mut render = Renderer::new(Recording::new(80, 2));
    render.stream("alpha\nbeta\ngamma\ndelta").unwrap();

    let written = render.terminal.written();
    assert_eq!(written.matches("alpha").count(), 1, "{written:?}");
    assert_eq!(written.matches("beta").count(), 1, "{written:?}");
}

#[test]
fn memory_does_not_grow_with_the_length_of_the_session() {
    // The reason there is no alternate screen. What this process holds is
    // one screen of rows no matter how long the model talks.
    let mut render = Renderer::new(Recording::new(40, 4));
    for turn in 0..5_000 {
        render.stream(&format!("line {turn}\n")).unwrap();
        render.terminal.take();
    }

    assert!(render.drawn <= 4, "drew {} rows", render.drawn);
    assert!(render.tail.len() <= 4, "held {} rows", render.tail.len());
    assert!(render.overflow.is_empty(), "overflow was not drained");
}

#[test]
fn settling_leaves_the_tail_in_scrollback_and_starts_fresh() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("answer").unwrap();
    render.settle().unwrap();
    render.terminal.take();

    render.stream("next").unwrap();

    assert_eq!(render.terminal.written(), format!("{}next", rewind(0)));
}

#[test]
fn settling_twice_writes_nothing_the_second_time() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("answer").unwrap();
    render.settle().unwrap();
    render.terminal.take();

    render.settle().unwrap();

    assert_eq!(render.terminal.written(), "");
}

#[test]
fn a_committed_line_is_never_drawn_a_second_time() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.commit("$ cargo build").unwrap();
    render.stream("done").unwrap();
    render.stream(" now").unwrap();

    let written = render.terminal.written();
    assert_eq!(written.matches("$ cargo build").count(), 1, "{written:?}");
}

#[test]
fn no_row_is_drawn_wider_than_the_terminal() {
    // The property the whole width question is about: what the tail counts and
    // what the terminal lays out have to agree, or the terminal wraps a row
    // this process believed fitted and the next frame rewinds to the wrong one.
    // The selectors are spelled out because they are invisible in a source file.
    let mut render = Renderer::new(Recording::new(6, 40));
    render
        .stream("\u{26A0}\u{FE0F} warning 日本語 e\u{301}x 1\u{FE0F}\u{20E3} ok")
        .unwrap();

    for row in render.tail.rows() {
        let columns = UnicodeWidthStr::width(row);
        assert!(columns <= 6, "row {row:?} is {columns} columns wide");
    }
}

#[test]
fn a_cut_lands_where_the_tail_wraps() {
    // A caller shortening a line and the renderer drawing it have to agree
    // about the same string, which they do only by counting the same way.
    let warnings = "\u{26A0}\u{FE0F}".repeat(3);
    for text in [
        "abcdef",
        "日本語です",
        warnings.as_str(),
        "ab\tcd",
        "e\u{301}xyz",
    ] {
        let mut tail = Tail::new(4, 5);
        tail.push(text, &mut Vec::new());

        let kept = match crate::width::cut(text, 4) {
            Some(at) => text.get(..at),
            None => Some(text),
        };

        assert_eq!(tail.rows().next(), kept, "{text:?}");
    }
}

#[test]
fn a_committed_line_wider_than_the_terminal_still_wraps() {
    let mut render = Renderer::new(Recording::new(4, 24));
    render.commit("abcdefghij").unwrap();

    let written = render.terminal.written();
    assert!(
        written.contains("abcd\r\nefgh\r\nij\r\n"),
        "expected wrapped rows, got {written:?}"
    );
}

#[test]
fn a_redirected_run_takes_the_pipe_path() {
    // The wiring: that a terminal reporting itself redirected reaches
    // `plain` at all. What that path produces is its own module's tests.
    let mut render = Renderer::new(Recording::redirected(80, 2));
    render.stream("alpha\nbeta\ngamma\ndelta").unwrap();
    render.settle().unwrap();

    assert_eq!(
        render.terminal.written(),
        "alpha\nbeta\ngamma\ndelta\n",
        "every row must reach the pipe, in order, once, and with no escapes"
    );
}

#[test]
fn a_resize_rewraps_instead_of_redrawing_wrongly() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hello").unwrap();

    render.terminal.resize(10, 24);
    render.resized().unwrap();
    render.terminal.take();

    render.stream("abcdefghijkl").unwrap();

    let written = render.terminal.written();
    assert!(
        written.contains("abcdefghij\r\nkl"),
        "expected a wrap at the new width, got {written:?}"
    );
}

#[test]
fn a_resize_that_changes_nothing_writes_nothing() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hello").unwrap();
    render.terminal.take();

    render.resized().unwrap();

    assert_eq!(render.terminal.written(), "");
}

#[test]
fn a_resize_writes_no_escapes_into_a_pipe() {
    // A redirected run still reads the window size, because the terminal is
    // asked for it and not the stream. So resizing the window during
    // `crucible > out.txt` reaches this path, and an erase sequence here
    // ends up in the file the user is keeping. The row that was live is
    // settled into the file rather than erased, because there is nothing to
    // erase and no way to write it again later.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.stream("hello").unwrap();
    render.terminal.take();

    render.terminal.resize(10, 24);
    render.resized().unwrap();

    assert_eq!(render.terminal.written(), "hello\n");
}

#[test]
fn a_resize_does_not_delete_what_a_pipe_was_already_sent() {
    // On a terminal the live rows are dropped because they are on screen and
    // about to be drawn again. A pipe has neither, so dropping them takes the
    // tail of the answer out of `crucible > answer.txt` with no error and no
    // marker in the file.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.stream("the answer so far").unwrap();

    render.terminal.resize(40, 24);
    render.resized().unwrap();
    render.stream(" and the rest\n").unwrap();
    render.settle().unwrap();

    assert_eq!(
        render.terminal.written(),
        "the answer so far\n and the rest\n"
    );
}

#[test]
fn a_window_that_only_got_shorter_is_still_a_resize() {
    // The height is the tail's bound. Left at the old one the tail holds more
    // rows than the screen can show, the rewind reaches above the top, and the
    // rows the terminal scrolls off are pushed into scrollback again on every
    // delta that follows.
    let mut render = Renderer::new(Recording::new(80, 50));
    render.terminal.resize(80, 10);
    render.resized().unwrap();
    render.terminal.take();

    for row in 0..60 {
        render.stream(&format!("line {row}\n")).unwrap();
    }

    assert!(render.tail.len() <= 10, "held {} rows", render.tail.len());
    assert!(render.drawn <= 10, "drew {} rows", render.drawn);

    let written = render.terminal.written();
    for up in 10..50 {
        assert!(
            !written.contains(&format!("\x1b[{up}A")),
            "the cursor moved {up} rows up a ten-row screen"
        );
    }
}

#[test]
fn settling_blank_rows_does_not_leave_them_in_the_tail() {
    // They were erased from the screen; keeping them draws them again on the
    // next frame and settles them into the record as blank lines the model
    // never sent.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("\n\n").unwrap();
    render.settle().unwrap();
    render.terminal.take();

    render.stream("next").unwrap();

    assert_eq!(render.terminal.written(), format!("{}next", rewind(0)));
    assert_eq!(render.drawn, 1);
}

#[test]
fn a_prompt_is_written_verbatim_after_the_live_region_ends() {
    // What the user types goes on this row, so it ends without a line ending
    // and nothing ever moves back over it.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("answer").unwrap();
    render.terminal.take();

    render.prompt("> ").unwrap();

    assert_eq!(
        render.terminal.written(),
        format!("{}answer\r\n> ", rewind(1))
    );
    assert_eq!(render.drawn, 0);
}

#[test]
fn a_resize_still_rewraps_what_a_pipe_writes_next() {
    // Silent, not inert: the new width has to reach the tail, or the rest
    // of the session wraps at the width the window had at startup.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.stream("hello\n").unwrap();
    render.terminal.resize(10, 24);
    render.resized().unwrap();
    render.terminal.take();

    render.stream("abcdefghijkl\n").unwrap();
    render.settle().unwrap();

    let written = render.terminal.written();
    assert!(
        written.contains("abcdefghij\nkl"),
        "expected a wrap at the new width, got {written:?}"
    );
}

// The region a prompt is typed into: drawn where it stands, redrawn as it
// changes, and taken off the screen whole.

/// A prompt-shaped region and where its cursor goes: two rows, typed on the
/// first, so one row is left parked below the cursor.
fn region() -> (Vec<Row>, Caret) {
    let rows = vec![Row::plain("› ask"), Row::plain("ask before edits")];
    (rows, Caret { row: 0, column: 6 })
}

#[test]
fn a_live_region_is_redrawn_from_its_own_top_row() {
    // The cursor is parked on the first of two rows, so the way back up is no
    // rows at all. Rewinding by the height of the region would reach over a
    // row the last frame left below the cursor.
    let mut render = Renderer::new(Recording::new(80, 24));
    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();
    render.terminal.take();

    render.live(&rows, caret, Palette::plain()).unwrap();

    assert!(
        render.terminal.written().starts_with(&rewind(1)),
        "{:?}",
        render.terminal.written()
    );
}

#[test]
fn a_live_region_never_lands_on_top_of_what_was_streamed() {
    // What the last turn left live belongs above the prompt, and it has to be
    // written down before the prompt reaches over it.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("answer").unwrap();
    render.terminal.take();

    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();

    let written = render.terminal.written();
    assert!(written.contains("answer\r\n"), "{written:?}");
}

#[test]
fn ending_a_live_region_takes_every_row_of_it_off_the_screen() {
    // Including the rows below the cursor, which the region's own arithmetic
    // is what keeps track of.
    let mut render = Renderer::new(Recording::new(80, 24));
    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();
    render.terminal.take();

    render.settle().unwrap();

    assert_eq!(render.terminal.written(), rewind(1));
    assert_eq!(render.drawn, 0);
    assert_eq!(render.parked, 0);
}

// Rows that stand under the tail: on screen for as long as the turn writing
// above them, and in the record afterwards nowhere at all.

/// What a turn stands under itself: one row, saying which mode it is running
/// in.
fn standing() -> Vec<Row> {
    vec![Row::plain("ask mode on")]
}

#[test]
fn a_standing_row_is_drawn_under_every_delta_that_arrives() {
    // The tail, the row under it, and then back up onto the row the next delta
    // appends to -- eleven columns along, where "the answer" stopped.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.terminal.take();

    render.stream("the answer").unwrap();

    assert_eq!(
        render.terminal.written(),
        format!("{}the answer\r\nask mode on\x1b[1A\x1b[11G", rewind(1))
    );
}

/// What a turn stands under once the prompt box stays up for it: a box, and the
/// mode under that.
fn boxed() -> Vec<Row> {
    vec![
        Row::plain("╭──╮"),
        Row::plain("│ ›│"),
        Row::plain("╰──╯"),
        Row::plain("ask mode on"),
    ]
}

#[test]
fn the_region_never_grows_past_the_screen_however_long_the_answer_is() {
    // The bug this exists to stop, and the reason it took a long answer to
    // show: the tail is bounded by the height of the window, so a tail that
    // had filled it plus rows standing under it was a region taller than the
    // screen. The top of one has already scrolled out of reach, so the next
    // rewind erases rows the terminal has taken -- which the reader sees as
    // the box and the mode being eaten away as the answer gets longer.
    let rows = 8;
    let mut render = Renderer::new(Recording::new(20, rows));
    render.under(&boxed(), None, Palette::plain()).unwrap();

    for delta in 0..40 {
        render.stream(&format!("line {delta}\n")).unwrap();

        assert!(
            render.tail.len() + render.footing.len() <= rows,
            "after {delta} deltas the region was {} rows on a screen {rows} tall",
            render.tail.len() + render.footing.len()
        );
    }
}

#[test]
fn the_tail_gives_back_the_rows_the_footing_takes() {
    // The tail is the half that can give: what stands under it is a component
    // somebody asked for, and the tail is a window onto a transcript the
    // terminal is keeping anyway.
    let mut render = Renderer::new(Recording::new(20, 8));

    for delta in 0..20 {
        render.stream(&format!("line {delta}\n")).unwrap();
    }
    let alone = render.tail.len();

    render.under(&boxed(), None, Palette::plain()).unwrap();

    assert_eq!(alone, 8, "the tail alone may fill the window");
    assert_eq!(
        render.tail.len() + render.footing.len(),
        8,
        "the two together may not"
    );
}

#[test]
fn a_line_committed_under_a_standing_row_still_lands_above_it() {
    // A tool call arrives in the middle of a turn and belongs to the record, so
    // it goes to scrollback -- above the row that is standing, not through it.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.terminal.take();

    render.commit("$ cargo build").unwrap();

    assert_eq!(
        render.terminal.written(),
        format!(
            "{}$ cargo build\r\n\r\nask mode on\x1b[1A\x1b[1G",
            rewind(1)
        )
    );
}

#[test]
fn a_standing_row_never_reaches_the_record() {
    // It says which mode the turn ran in, which is not something the turn said.
    // Settled once a frame it would be most of the session's scrollback.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.stream("one").unwrap();
    render.stream(" two").unwrap();
    render.terminal.take();

    render.settle().unwrap();

    assert_eq!(
        render.terminal.written(),
        format!("{}one two\r\n", rewind(1))
    );
    assert_eq!(render.drawn, 0);
    assert_eq!(render.parked, 0);
}

#[test]
fn a_standing_row_comes_back_after_a_question_was_asked_in_the_middle_of_a_turn() {
    // The question settles the region to write itself, which takes the row off
    // the screen. The turn is still running, so the next delta puts it back.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.stream("about to run").unwrap();
    render.settle().unwrap();
    render.terminal.take();

    render.stream("carrying on").unwrap();

    assert_eq!(
        render.terminal.written(),
        format!("{}carrying on\r\nask mode on\x1b[1A\x1b[12G", rewind(0))
    );
}

#[test]
fn taking_a_standing_row_back_leaves_the_tail_where_it_was() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.stream("the answer").unwrap();
    render.terminal.take();

    render.under(&[], None, Palette::plain()).unwrap();

    assert_eq!(
        render.terminal.written(),
        format!("{}the answer", rewind(1))
    );
    assert_eq!(render.drawn, 1);
    assert_eq!(render.parked, 0);
}

#[test]
fn a_prompt_drawn_over_a_standing_row_takes_it_off_the_screen() {
    // Two claims on the bottom of the screen, and the box carries the mode
    // itself. A rewind that stopped short of the row would leave it under the
    // box, saying the same thing a second time.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.terminal.take();

    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();

    let written = render.terminal.written();
    assert!(written.starts_with(&rewind(1)), "{written:?}");
    assert_eq!(written.matches("ask mode on").count(), 0, "{written:?}");
}

#[test]
fn a_redirected_run_stands_nothing_under_anything() {
    // The reason a live region is not drawn into a pipe: there is no bottom row
    // to hold something at, and the escapes that would hold it there end up in
    // whatever kept the output.
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render.under(&standing(), None, Palette::plain()).unwrap();
    render.stream("the answer\n").unwrap();
    render.settle().unwrap();

    assert_eq!(render.terminal.written(), "the answer\n");
}

#[test]
fn a_redirected_run_draws_no_live_region_at_all() {
    // There is no cursor to park in a pipe, and the escapes that would park
    // one end up in whatever kept the output.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    let (rows, caret) = region();

    render.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(render.terminal.written(), "");
    assert_eq!(render.terminal.flushes(), 0);
}

/// A palette that writes every hue it has, without an environment to say so.
fn colourful() -> Palette {
    Palette::resolve(true, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[test]
fn a_run_with_colour_in_it_reads_the_markers_out_of_the_answer() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.wears(colourful());

    render.stream("a **loud** word").unwrap();
    render.settle().unwrap();

    // The markers are gone and the word wears the slot in their place.
    let written = render.terminal.written();
    let read = format!(
        "a {}loud{} word",
        colourful().open(Slot::Strong),
        colourful().close()
    );

    assert!(written.contains(&read), "{written:?}");
}

#[test]
fn a_slot_costs_the_answer_the_columns_it_would_have_taken_plain() {
    // Two renderers of the same width, the same words, one of them told a
    // palette. A slot that cost a column would wrap one and not the other.
    let mut plain = Renderer::new(Recording::new(12, 24));
    let mut coloured = Renderer::new(Recording::new(12, 24));
    coloured.wears(colourful());

    plain.stream("the loud word\n").unwrap();
    coloured.stream("the **loud** word\n").unwrap();

    assert_eq!(plain.drawn, coloured.drawn);
}

#[test]
fn a_run_with_no_colour_in_it_keeps_every_marker_the_model_wrote() {
    // Dropping one here would take the emphasis away and put nothing in its
    // place. A file of markdown is worth more than a file it was taken out of.
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render.stream("a **loud** word\n").unwrap();
    render.settle().unwrap();

    assert_eq!(render.terminal.written(), "a **loud** word\n");
}

#[test]
fn a_fence_the_model_never_closed_does_not_reach_the_next_message() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.wears(colourful());

    render.stream("```rust\nlet it = 1;\n").unwrap();
    render.settle().unwrap();
    render.stream("plain again").unwrap();

    let written = render.terminal.written();
    let tail = written.split("let it = 1;").last().unwrap_or_default();
    assert!(
        !tail.contains(colourful().open(Slot::Quiet)),
        "the fence ended with the message: {tail:?}"
    );
}
