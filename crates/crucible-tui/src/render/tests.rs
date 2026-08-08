//! What the renderer draws, asserted against a recording terminal.

use super::*;
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

    assert_eq!(render.terminal().written(), format!("{}hello", rewind(0)));
}

#[test]
fn a_second_frame_erases_the_first() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hel").unwrap();
    render.terminal().take();

    render.stream("lo").unwrap();

    assert_eq!(render.terminal().written(), format!("{}hello", rewind(1)));
}

#[test]
fn a_frame_is_one_write_and_one_flush() {
    // The burst budget is about frames, not bytes: a redraw that wrote row
    // by row would tear on a slow terminal and cost a syscall each.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("one\ntwo\nthree").unwrap();

    assert_eq!(render.terminal().flushes(), 1);
}

#[test]
fn nothing_ever_erases_upward() {
    // The property that makes scrollback safe. If one of these appears,
    // committed output is reachable and the design is gone.
    let mut render = Renderer::new(Recording::new(20, 3));
    for turn in 0..200 {
        render.stream(&format!("line {turn}\n")).unwrap();
    }

    let written = render.terminal().written();
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

    let written = render.terminal().written();
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
        render.terminal().take();
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
    render.terminal().take();

    render.stream("next").unwrap();

    assert_eq!(render.terminal().written(), format!("{}next", rewind(0)));
}

#[test]
fn settling_twice_writes_nothing_the_second_time() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("answer").unwrap();
    render.settle().unwrap();
    render.terminal().take();

    render.settle().unwrap();

    assert_eq!(render.terminal().written(), "");
}

#[test]
fn a_committed_line_is_never_drawn_a_second_time() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.commit("$ cargo build").unwrap();
    render.stream("done").unwrap();
    render.stream(" now").unwrap();

    let written = render.terminal().written();
    assert_eq!(written.matches("$ cargo build").count(), 1, "{written:?}");
}

#[test]
fn a_committed_line_wider_than_the_terminal_still_wraps() {
    let mut render = Renderer::new(Recording::new(4, 24));
    render.commit("abcdefghij").unwrap();

    let written = render.terminal().written();
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
        render.terminal().written(),
        "alpha\nbeta\ngamma\ndelta\n",
        "every row must reach the pipe, in order, once, and with no escapes"
    );
}

#[test]
fn a_resize_rewraps_instead_of_redrawing_wrongly() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hello").unwrap();

    render.terminal().resize(10, 24);
    render.resized().unwrap();
    render.terminal().take();

    render.stream("abcdefghijkl").unwrap();

    let written = render.terminal().written();
    assert!(
        written.contains("abcdefghij\r\nkl"),
        "expected a wrap at the new width, got {written:?}"
    );
}

#[test]
fn a_resize_that_changes_nothing_writes_nothing() {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("hello").unwrap();
    render.terminal().take();

    render.resized().unwrap();

    assert_eq!(render.terminal().written(), "");
}

#[test]
fn a_resize_writes_no_escapes_into_a_pipe() {
    // A redirected run still reads the window size, because the terminal is
    // asked for it and not the stream. So resizing the window during
    // `crucible > out.txt` reaches this path, and an erase sequence here
    // ends up in the file the user is keeping.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.stream("hello").unwrap();
    render.terminal().take();

    render.terminal().resize(10, 24);
    render.resized().unwrap();

    assert_eq!(render.terminal().written(), "");
}

#[test]
fn a_resize_still_rewraps_what_a_pipe_writes_next() {
    // Silent, not inert: the new width has to reach the tail, or the rest
    // of the session wraps at the width the window had at startup.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.stream("hello\n").unwrap();
    render.terminal().resize(10, 24);
    render.resized().unwrap();
    render.terminal().take();

    render.stream("abcdefghijkl\n").unwrap();
    render.settle().unwrap();

    let written = render.terminal().written();
    assert!(
        written.contains("abcdefghij\nkl"),
        "expected a wrap at the new width, got {written:?}"
    );
}
