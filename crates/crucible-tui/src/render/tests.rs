//! What the renderer draws, asserted against a recording terminal.

use crate::color::Theme;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::color::{Palette, Slot};
use crate::row::Row;
use crate::terminal::Recording;

/// The escape a frame starts with when `rows` rows are already on screen: the
/// sequence asking the terminal to hold what it has, then the move back over
/// what the last frame left. Written out literally rather than borrowed from
/// `frame`, so a change there has to be asserted here too.
fn rewind(rows: usize) -> String {
    let back = match rows {
        0 | 1 => "\r\x1b[J".to_owned(),
        n => format!("\r\x1b[{}A\x1b[J", n - 1),
    };

    format!("\x1b[?2026h{back}")
}

/// The whole of one frame: the rewind, what it drew, and the sequence that
/// shows the two pictures swapped at once.
fn shown(rows: usize, body: &str) -> String {
    format!("{}{body}\x1b[?2026l", rewind(rows))
}

#[test]
fn the_first_frame_does_not_move_the_cursor_up() {
    // There is nothing above it yet, and moving up would eat a line the
    // shell printed.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hello").unwrap();

    assert_eq!(render.terminal.written(), shown(0, "hello"));
}

#[test]
fn a_second_frame_erases_the_first() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hel").unwrap();
    render.terminal.take();

    render.stream("lo").unwrap();

    assert_eq!(render.terminal.written(), shown(1, "hello"));
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

    assert_eq!(render.terminal.written(), shown(0, "next"));
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
fn a_window_that_shrank_erases_no_further_back_than_it_is_tall() {
    // The region was counted on the old window and the erase is written to the
    // new one. Past the top of that screen the rows belong to scrollback: they
    // are committed, they are still being read, and this is the renderer's one
    // promise -- it never reaches above what it drew.
    let mut render = Renderer::new(Recording::new(80, 24));
    for row in 0..20 {
        render.stream(&format!("line {row}\n")).unwrap();
    }
    render.terminal.take();

    render.terminal.resize(80, 8);
    render.resized().unwrap();

    let written = render.terminal.written();
    for up in 8..24 {
        assert!(
            !written.contains(&format!("\x1b[{up}A")),
            "the cursor moved {up} rows up an eight-row screen: {written:?}"
        );
    }
}

#[test]
fn a_resize_leaves_a_row_the_renderer_never_counted_alone() {
    // A prompt is written verbatim onto a row no frame will ever move back
    // over, and the renderer says so by counting none of it. So there is
    // nothing of its own on screen to drop here -- and what is on that row is
    // a question waiting to be answered, which erasing would leave somebody
    // deciding from memory.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.prompt("  [y]es  [s]ession  [n]o › ").unwrap();
    render.terminal.take();

    render.terminal.resize(40, 24);
    render.resized().unwrap();

    assert_eq!(render.terminal.written(), "");
    assert_eq!(render.columns(), 40, "the new width still has to arrive");
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

    assert_eq!(render.terminal.written(), shown(0, "next"));
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
        format!("{}> ", shown(1, "answer\r\n"))
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

    assert_eq!(render.terminal.written(), shown(1, ""));
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
        shown(1, "the answer\r\nask mode on\x1b[1A\x1b[11G")
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
    // show: a tail that had filled the window, plus rows standing under it, was
    // a region taller than the screen. The top of one has already scrolled out
    // of reach, so the next rewind erases rows the terminal has taken -- which
    // the reader sees as the box and the mode being eaten away as the answer
    // gets longer. A one-row tail leaves the rest of the window to whatever
    // stands under it; this asserts the outcome rather than the constant.
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
fn the_tail_is_one_row_whatever_stands_under_it() {
    // Streamed text only grows at the end, so the row being written to is the
    // only one a later delta can change. Everything above it went to scrollback
    // as it was written, which leaves the footing nothing to negotiate for.
    let mut render = Renderer::new(Recording::new(20, 8));

    for delta in 0..20 {
        render.stream(&format!("line {delta}\n")).unwrap();
    }
    let alone = render.tail.len();

    render.under(&boxed(), None, Palette::plain()).unwrap();

    assert_eq!(
        alone, 1,
        "the tail holds the row still being written and no more"
    );
    assert_eq!(render.tail.len(), 1, "and standing rows do not shrink it");
}

#[test]
fn a_delta_costs_the_same_on_a_tall_window_as_on_a_short_one() {
    // What a frame writes is what is live, and a delta is what causes a frame.
    // The defect this exists to stop: the tail was bounded by the height of the
    // window, so every delta rewound over a screen of rows and wrote them all
    // again -- an answer costing the window's height per delta rather than the
    // delta's own length, on the one path the burst budget bounds.
    let cost = |rows: usize| {
        let mut render = Renderer::new(Recording::new(40, rows));
        render.under(&boxed(), None, Palette::plain()).unwrap();
        render.terminal.take();

        for delta in 0..40 {
            render.stream(&format!("line {delta}\n")).unwrap();
        }

        render.terminal.take().len()
    };

    assert_eq!(
        cost(8),
        cost(40),
        "a taller window drew more for the same answer"
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
        shown(1, "$ cargo build\r\n\r\nask mode on\x1b[1A\x1b[1G")
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

    assert_eq!(render.terminal.written(), shown(1, "one two\r\n"));
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
        shown(0, "carrying on\r\nask mode on\x1b[1A\x1b[12G")
    );
}

#[test]
fn taking_a_standing_row_back_leaves_the_tail_where_it_was() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.stream("the answer").unwrap();
    render.terminal.take();

    render.under(&[], None, Palette::plain()).unwrap();

    assert_eq!(render.terminal.written(), shown(1, "the answer"));
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
fn a_presented_row_lands_above_a_standing_row_and_leaves_it_on_the_screen() {
    // A tool result is rows this program composed, so it arrives through
    // `present` rather than `commit` -- and `present` ends the live region to
    // write above it, which takes the standing row off the screen. Every other
    // frame puts it back on the way past; this one wrote its rows and stopped.
    // What the reader saw was the box under a running turn blinking out under
    // each result and coming back on the next tick of the clock.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.under(&standing(), None, Palette::plain()).unwrap();
    render.terminal.take();

    render
        .present(&[Row::plain("Bash(cargo build)")], Palette::plain())
        .unwrap();

    // The empty row between the two is the tail's own: where the next delta of
    // the turn still running would land. A committed line under a standing row
    // leaves the same one, and this is the same picture reached a frame sooner.
    assert_eq!(
        render.terminal.written(),
        format!(
            "{}Bash(cargo build)\r\n{}",
            shown(0, ""),
            shown(0, "\r\nask mode on\x1b[1A\x1b[1G")
        )
    );
    assert_eq!(render.drawn, 2);
    assert_eq!(render.parked, 1);
}

#[test]
fn a_presented_row_with_nothing_standing_under_it_is_one_frame_and_no_more() {
    // The other half of the same rule. Between turns there is nothing under the
    // tail to put back, and a frame drawn to put nothing back is a write and a
    // flush per row of a transcript that is only getting longer.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("the answer").unwrap();
    render.terminal.take();

    render
        .present(&[Row::plain("Bash(cargo build)")], Palette::plain())
        .unwrap();

    assert_eq!(
        render.terminal.written(),
        format!("{}Bash(cargo build)\r\n", shown(1, "the answer\r\n"))
    );
    assert_eq!(render.drawn, 0);
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
    Palette::resolve(true, Theme::Dark, None, &|name| {
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
        !tail.contains(colourful().open(Slot::Quiet).as_str()),
        "the fence ended with the message: {tail:?}"
    );
}

#[test]
fn nothing_is_parted_from_the_start_of_the_session() {
    // The top of the transcript is already a boundary. A blank row spent here
    // is one the shell's own last line is pushed away by, for nothing.
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render.apart().unwrap();
    render.commit("the first line").unwrap();

    assert_eq!(render.terminal.written(), "the first line\n");
}

#[test]
fn one_blank_row_stands_between_two_blocks() {
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render.commit("what was answered").unwrap();
    render.apart().unwrap();
    render.commit("● Read(src/main.rs)").unwrap();

    assert_eq!(
        render.terminal.written(),
        "what was answered\n\n● Read(src/main.rs)\n"
    );
}

#[test]
fn blank_rows_do_not_accumulate() {
    // Two blocks in a row each ask on their way in, and what separates them is
    // still one row. Asking is how a block says it is a block, not how many
    // rows it wants.
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render.commit("what was answered").unwrap();
    render.apart().unwrap();
    render.apart().unwrap();
    render.apart().unwrap();
    render.commit("● Read(src/main.rs)").unwrap();

    assert_eq!(
        render.terminal.written(),
        "what was answered\n\n● Read(src/main.rs)\n"
    );
}

#[test]
fn a_row_is_never_parted_into_a_line_that_is_still_arriving() {
    // The caller cannot tell the first delta of an answer from the tenth, so it
    // asks on every one. What comes next while the tail holds something is the
    // rest of that line, and a row put there would cut the answer in half.
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render.commit("  └ 128 lines").unwrap();
    for delta in ["Which ", "means ", "the file ", "is short."] {
        render.apart().unwrap();
        render.stream(delta).unwrap();
    }
    render.settle().unwrap();

    assert_eq!(
        render.terminal.written(),
        "  └ 128 lines\n\nWhich means the file is short.\n"
    );
}

#[test]
fn rows_this_program_composed_settle_the_question_too() {
    // `present` is the other way into the record, and a component that ends on
    // a blank row -- which several do, to keep what follows off them -- has
    // already parted itself.
    let mut render = Renderer::new(Recording::redirected(80, 24));

    render
        .present(
            &[Row::new().then(Slot::Plain, "› what is 2+2"), Row::new()],
            Palette::plain(),
        )
        .unwrap();
    render.apart().unwrap();
    render.commit("Two plus two is four.").unwrap();

    assert_eq!(
        render.terminal.written(),
        "› what is 2+2\n\nTwo plus two is four.\n"
    );
}

#[test]
fn the_record_counts_the_rows_that_have_gone_into_it() {
    // Rows written rather than rows kept: this goes on rising for the length of
    // a session, because what it is for is the difference between two readings
    // of it rather than either reading on its own.
    let mut render = Renderer::new(Recording::new(80, 24));
    assert_eq!(render.record(), 0);

    render
        .present(&[Row::plain("one"), Row::plain("two")], Palette::plain())
        .unwrap();
    assert_eq!(render.record(), 2);

    render.stream("an answer").unwrap();
    assert_eq!(render.record(), 2, "a live row is not in the record yet");

    render.settle().unwrap();
    assert_eq!(render.record(), 3);
}

#[test]
fn a_row_is_found_again_by_the_count_that_was_read_when_it_went() {
    // The whole of what the two are for. A caller that means to point at a row
    // later keeps the count at the moment it was written, and the difference
    // between that and where the region is now is how far the row has
    // travelled — which is the only thing an inline renderer can know about a
    // row it has let go of.
    let mut render = Renderer::new(Recording::new(80, 24));
    render
        .present(
            &[Row::plain("(+128 lines · ctrl+o to expand)")],
            Palette::plain(),
        )
        .unwrap();
    let at = render.record() - 1;

    for _ in 0..4 {
        render
            .present(&[Row::plain("and then this")], Palette::plain())
            .unwrap();
    }

    // Nothing is standing, so the cursor is on the row after the record: five
    // rows have gone and the last of them is the row above the cursor.
    assert_eq!(render.recorded(9, 10), Some(at + 4));
    assert_eq!(render.recorded(5, 10), Some(at));
}

#[test]
fn a_row_of_the_live_region_is_no_row_of_the_record() {
    // The record holds what has been let go of. A pointer inside the region —
    // on the box, or on a view standing under a turn — is on something still
    // being drawn, and answering it with a row of the record would hand back a
    // result nobody pointed at.
    let mut render = Renderer::new(Recording::new(80, 24));
    render
        .present(&[Row::plain("one"), Row::plain("two")], Palette::plain())
        .unwrap();

    // Two rows of region with the cursor parked on the first of them, so the
    // region starts on the row the cursor is on and the record ends above it.
    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(render.recorded(8, 8), None, "the region's own first row");
    assert_eq!(render.recorded(9, 8), None, "the region's second row");
    assert_eq!(render.recorded(12, 8), None, "below everything drawn");

    assert_eq!(render.recorded(7, 8), Some(1));
    assert_eq!(render.recorded(6, 8), Some(0));
    assert_eq!(
        render.recorded(5, 8),
        None,
        "a row belonging to whatever ran before this process"
    );
}

#[test]
fn the_region_is_read_from_the_same_place_the_record_is() {
    // The other half of the same arithmetic, and the reason it is one place:
    // the box reads a click against the region and the transcript reads one
    // against the record, and two answers to where the region starts would be
    // two pictures of the same screen.
    let mut render = Renderer::new(Recording::new(80, 24));
    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(render.within(4, 4), Some(0));
    assert_eq!(render.within(5, 4), Some(1));

    assert_eq!(render.within(3, 4), None, "above the region");
    assert_eq!(render.within(6, 4), None, "below the last row of it");
}
