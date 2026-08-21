//! What the renderer draws, asserted against the window it drew on.
//!
//! Almost nothing here reads bytes. A frame names the row it writes, so the
//! bytes are replayed into a [`Picture`] and the assertions are about what a
//! reader would be looking at — which is the thing that has to be right, and
//! the thing that stays readable when the sequences underneath it change. The
//! sequences themselves are asserted once, next to the type that writes them.

use unicode_width::UnicodeWidthStr;

use super::*;
use crate::color::{Palette, Slot, Theme};
use crate::row::Row;
use crate::terminal::{Picture, Recording};

/// A renderer on a window of the given size, and the screen it has drawn so
/// far.
struct Drawn {
    /// The renderer under test.
    render: Renderer<Recording>,
}

impl Drawn {
    /// A session on a window this size, with nothing drawn on it yet.
    fn new(columns: usize, rows: usize) -> Self {
        Self {
            render: Renderer::new(Recording::new(columns, rows)),
        }
    }

    /// What the window shows, given everything written to it.
    fn screen(&self) -> Picture {
        self.render.terminal.picture()
    }

    /// Forgets what was written, so the next assertion is about one frame.
    fn take(&mut self) -> String {
        self.render.terminal.take()
    }
}

impl std::ops::Deref for Drawn {
    type Target = Renderer<Recording>;

    fn deref(&self) -> &Self::Target {
        &self.render
    }
}

impl std::ops::DerefMut for Drawn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.render
    }
}

/// How many rows a frame named, counted by the addresses in it.
///
/// One more than the rows it drew, because a frame parks the cursor when it is
/// done and that is an address too. What a frame costs is asserted through this
/// rather than through its length: bytes vary with what the rows say, and the
/// thing the budget is about is how many of them a redraw touches.
fn addressed(frame: &str) -> usize {
    frame
        .split("\x1b[")
        .skip(1)
        .filter(|piece| {
            piece.split_once('H').is_some_and(|(at, _)| {
                !at.is_empty() && at.chars().all(|byte| byte.is_ascii_digit() || byte == ';')
            })
        })
        .count()
}

/// A palette that writes every hue it has, without an environment to say so.
fn colourful() -> Palette {
    Palette::resolve(true, Theme::Dark, None, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

/// A prompt-shaped box and where its cursor goes: three rows, typed on the
/// middle one.
fn boxed() -> (Vec<Row>, Caret) {
    let rows = vec![
        Row::plain("╭────╮"),
        Row::plain("│ ›  │"),
        Row::plain("╰────╯"),
    ];
    (rows, Caret { row: 1, column: 4 })
}

/// What a turn stands under itself while it runs.
fn standing() -> Vec<Row> {
    vec![Row::plain("· thinking")]
}

// Where things land on a window this process owns.

#[test]
fn a_session_reads_from_the_top_of_the_window_down() {
    // The first thing anybody sees. A record that does not fill the transcript
    // band yet starts at the top of it, as a terminal's own scrollback would,
    // rather than sitting at the bottom over a screen of nothing.
    let mut drawn = Drawn::new(80, 24);
    drawn.commit("hello").unwrap();

    assert_eq!(drawn.screen().row(0), "hello");
}

#[test]
fn the_transcript_is_the_band_that_scrolls_and_the_box_is_not() {
    // The whole of what the full-screen renderer buys. The box is drawn on the
    // same rows before and after a screenful of answer went past it.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    let before = drawn.screen().said();

    for line in 0..50 {
        drawn.commit(&format!("line {line}")).unwrap();
    }

    let after = drawn.screen();
    assert_eq!(after.row(7), "╭────╮");
    assert_eq!(after.row(8), "│ ›  │");
    assert_eq!(after.row(9), "╰────╯");
    assert_eq!(before, vec!["╭────╮", "│ ›  │", "╰────╯"]);
    // And the transcript above it is showing the foot of the session.
    assert_eq!(after.row(6), "line 49");
}

#[test]
fn what_a_turn_stands_under_sits_between_the_transcript_and_the_box() {
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.commit("answer").unwrap();
    drawn.under(&standing(), None, Palette::plain()).unwrap();
    drawn.live(&rows, caret, Palette::plain()).unwrap();

    let screen = drawn.screen();
    assert_eq!(screen.row(0), "answer");
    assert_eq!(screen.row(6), "· thinking");
    assert_eq!(screen.row(7), "╭────╮");
}

#[test]
fn taking_a_standing_row_back_takes_it_off_the_screen() {
    let mut drawn = Drawn::new(40, 10);
    drawn.under(&standing(), None, Palette::plain()).unwrap();
    assert!(drawn.screen().said().iter().any(|row| row == "· thinking"));

    drawn.under(&[], None, Palette::plain()).unwrap();

    assert!(drawn.screen().said().is_empty());
}

#[test]
fn what_stands_in_a_band_never_reaches_the_record() {
    // A box and a turn's own row are facts about the session rather than things
    // that were said, so the transcript reads afterwards as though neither had
    // been there.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();

    drawn.commit("answer").unwrap();
    let lines = drawn.lines();

    drawn.under(&standing(), None, Palette::plain()).unwrap();
    drawn.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(drawn.lines(), lines);
}

#[test]
fn a_box_that_grew_takes_its_rows_from_the_transcript() {
    let mut drawn = Drawn::new(40, 10);
    let (three, caret) = boxed();
    drawn.live(&three, caret, Palette::plain()).unwrap();
    let short = drawn.bands().transcript.len();

    let tall: Vec<Row> = (0..5).map(|at| Row::plain(format!("row {at}"))).collect();
    drawn.live(&tall, caret, Palette::plain()).unwrap();

    assert_eq!(drawn.bands().transcript.len(), short - 2);
}

// The cursor.

#[test]
fn the_cursor_parks_where_the_box_says_it_does() {
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();

    // The box stands on the last three rows, and the caret named the middle
    // one, four columns along.
    assert_eq!(drawn.screen().caret(), (8, 4));
}

#[test]
fn the_cursor_parks_where_the_box_would_be_when_there_is_none() {
    // Between the box being taken down and the next one going up there is
    // still a cursor, and the row it belongs on is the one the box will be on
    // — the last of the window, since an empty prompt band sits against the
    // bottom edge and a cursor may not be parked past it.
    let drawn = Drawn::new(40, 10);
    let bands = drawn.bands();

    assert_eq!(drawn.parked(&bands), (9, 0));
}

#[test]
fn a_question_asked_mid_turn_takes_the_cursor() {
    // Nothing is being typed into a box, because there is no box: the turn is
    // running and what it put up is what has the keyboard.
    let mut drawn = Drawn::new(40, 10);
    let asking = vec![Row::plain("allow this? "), Row::plain("y/n")];
    drawn
        .under(
            &asking,
            Some(Caret { row: 0, column: 12 }),
            Palette::plain(),
        )
        .unwrap();

    assert_eq!(drawn.screen().caret(), (8, 12));
}

// What a frame costs.

#[test]
fn a_frame_is_one_write_and_one_flush() {
    // The burst budget is about frames, not bytes: a redraw written row by row
    // would tear on a slow terminal and cost a syscall each.
    let mut drawn = Drawn::new(80, 24);
    drawn.commit("one\ntwo\nthree").unwrap();

    assert_eq!(drawn.render.terminal.flushes(), 1);
}

#[test]
fn a_frame_that_changed_nothing_writes_nothing() {
    // What a turn mostly is: a redraw asked for by something that turned out
    // not to have moved. The bracket holding the screen is bytes too.
    let mut drawn = Drawn::new(80, 24);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    drawn.take();

    drawn.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(drawn.render.terminal.written(), "");
    assert_eq!(drawn.render.terminal.flushes(), 1);
}

#[test]
fn only_the_rows_whose_picture_changed_are_written() {
    // A keystroke on a window this tall may not cost a screen. One row of the
    // box changed, and one row plus the park is what goes down the wire.
    let mut drawn = Drawn::new(40, 24);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    drawn.take();

    let typed = vec![
        Row::plain("╭────╮"),
        Row::plain("│ ›a │"),
        Row::plain("╰────╯"),
    ];
    drawn.live(&typed, caret, Palette::plain()).unwrap();

    let frame = drawn.render.terminal.written();
    assert_eq!(addressed(frame), 2, "{frame:?}");
    assert!(frame.contains("│ ›a │"), "{frame:?}");
}

#[test]
fn a_frame_is_the_size_of_the_window_however_long_the_session_is() {
    // The reason the record is bounded and the frame is not proportional to it:
    // five thousand lines in, one delta still writes a window.
    let mut drawn = Drawn::new(40, 8);
    for line in 0..5_000 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.take();

    drawn.commit("and one more").unwrap();

    let frame = drawn.render.terminal.written();
    assert!(addressed(frame) <= 8 + 1, "wrote {} rows", addressed(frame));
}

#[test]
fn no_row_is_drawn_wider_than_the_window() {
    // A row the terminal wrapped is a row this process did not put there, and
    // on a screen it owns that is a row of some other band overwritten.
    let mut drawn = Drawn::new(20, 8);
    drawn.commit(&"x".repeat(200)).unwrap();
    drawn.present(&[Row::plain("y".repeat(200))]).unwrap();

    for row in drawn.screen().rows() {
        assert!(row.width() <= 20, "{row:?} is {} columns", row.width());
    }
}

// A window the reader resized.

#[test]
fn a_resize_folds_the_record_again_rather_than_redrawing_it_wrongly() {
    let mut drawn = Drawn::new(20, 8);
    drawn.commit("the quick brown fox jumps").unwrap();
    assert_eq!(drawn.screen().row(0), "the quick brown fox");

    drawn.render.terminal.resize(40, 8);
    drawn.resized().unwrap();

    assert_eq!(drawn.screen().row(0), "the quick brown fox jumps");
}

#[test]
fn a_resize_that_changed_nothing_writes_nothing() {
    let mut drawn = Drawn::new(20, 8);
    drawn.commit("hello").unwrap();
    drawn.take();

    drawn.resized().unwrap();

    assert_eq!(drawn.render.terminal.written(), "");
}

#[test]
fn a_resize_drops_what_was_standing_rather_than_drawing_it_at_the_wrong_size() {
    // A box laid out against a window that has gone. The caller lays out the
    // next one; drawing this one again would put a row past the edge.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();

    drawn.render.terminal.resize(20, 10);
    drawn.resized().unwrap();

    assert!(drawn.screen().said().is_empty());
}

// A run whose output is a file.

#[test]
fn a_redirected_run_writes_no_escape_at_all() {
    // Not "writes few": a pipe never receives an escape byte, and it is the
    // path rather than a filter that makes that true.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.wears(colourful());
    render.commit("plain").unwrap();
    render.stream("a **loud** word").unwrap();
    render.settle().unwrap();
    render
        .present(&[Row::new().then(Slot::Strong, "composed")])
        .unwrap();
    render.live(&boxed().0, boxed().1, colourful()).unwrap();
    render.under(&standing(), None, colourful()).unwrap();

    let written = render.terminal.written();
    assert!(!written.contains('\x1b'), "{written:?}");
}

#[test]
fn a_redirected_run_is_given_every_line_as_text() {
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.commit("first").unwrap();
    render.present(&[Row::plain("second")]).unwrap();

    assert_eq!(render.terminal.written(), "first\nsecond\n");
}

#[test]
fn a_redirected_run_ends_the_line_a_question_was_left_on() {
    // `prompt` is written through unterminated, because whatever is reading has
    // to see the question before it can answer. What comes next owes it an
    // ending rather than continuing the row.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.prompt(Slot::Quiet, "ask › ").unwrap();
    render.present(&[Row::plain("answered")]).unwrap();

    assert_eq!(render.terminal.written(), "ask › \nanswered\n");
}

#[test]
fn a_redirected_run_stands_nothing_and_scrolls_nothing() {
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render
        .live(&boxed().0, boxed().1, Palette::plain())
        .unwrap();

    assert_eq!(render.terminal.written(), "");
    assert!(!render.scrolled(-3).unwrap());
}

// What the reader is looking at.

#[test]
fn scrolling_up_leaves_the_foot_and_sending_something_returns_to_it() {
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    let foot = drawn.screen().row(7).to_owned();

    assert!(drawn.scrolled(-4).unwrap());
    assert_ne!(drawn.screen().row(7), foot);

    drawn.follows().unwrap();

    assert_eq!(drawn.screen().row(7), foot);
}

#[test]
fn text_arriving_while_somebody_reads_back_does_not_move_them() {
    // The one thing scrolling has to get right. A reader who scrolled up is
    // reading; an answer arriving below is not a reason to take them away from
    // it.
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.scrolled(-4).unwrap();
    let showing = drawn.screen().said();

    drawn.commit("arriving").unwrap();

    assert_eq!(drawn.screen().said(), showing);
}

#[test]
fn one_notch_of_the_wheel_moves_what_the_run_asked_it_to() {
    // The setting reaches the wheel and nowhere else: a renderer told six moves
    // six rows a notch, and the same picture is reachable a row at a time.
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.rolls(6);

    assert!(drawn.notched(true).unwrap());
    let wheeled = drawn.screen().said();

    drawn.follows().unwrap();
    assert!(drawn.scrolled(-6).unwrap());

    assert_eq!(drawn.screen().said(), wheeled);
}

#[test]
fn the_wheel_goes_towards_the_top_of_the_session_and_back() {
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    let foot = drawn.screen().said();

    assert!(drawn.notched(true).unwrap());
    assert_ne!(drawn.screen().said(), foot);

    assert!(drawn.notched(false).unwrap());
    assert_eq!(drawn.screen().said(), foot);
}

#[test]
fn a_wheel_nobody_configured_still_moves_the_transcript() {
    // The failure this is here for is a notch of nought: it looks like a
    // terminal that has stopped reporting rather than like a setting waiting to
    // be made.
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }

    assert!(drawn.notched(true).unwrap());
}

#[test]
fn a_scroll_that_could_not_move_says_so() {
    let mut drawn = Drawn::new(40, 8);
    drawn.commit("one line").unwrap();

    assert!(!drawn.scrolled(-4).unwrap());
}

// What is under a click.

#[test]
fn a_click_on_the_transcript_names_the_line_under_it() {
    // The number `lines` handed back when the line went in is the number
    // `aimed` hands back when somebody clicks it, however much has arrived
    // since.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("first").unwrap();
    let wanted = drawn.lines() - 1;
    drawn.commit("second").unwrap();

    assert_eq!(drawn.aimed(0), Some(Aimed::Line(wanted)));
    assert_eq!(drawn.aimed(1), Some(Aimed::Line(wanted + 1)));
}

#[test]
fn a_click_below_the_last_line_names_nothing() {
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("only line").unwrap();

    assert_eq!(drawn.aimed(4), None);
}

#[test]
fn a_click_on_the_box_is_a_row_of_the_box() {
    // What lets somebody put the cursor in the middle of a long prompt: the
    // renderer answers with the row of the thing standing there, and the
    // component works in the rows it drew.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(drawn.aimed(7), Some(Aimed::Boxed(0)));
    assert_eq!(drawn.aimed(9), Some(Aimed::Boxed(2)));
}

#[test]
fn a_click_on_what_is_over_the_box_is_not_a_click_on_the_box() {
    // The two stand one above the other and both answer in their own rows, so
    // the row number alone says nothing: a list three rows tall over a box
    // three rows tall has a row 0 in each. Told apart here, because nothing
    // further down could — a component asked about a row it did not draw puts
    // the cursor somewhere nobody pointed at.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    drawn.under(&standing(), None, Palette::plain()).unwrap();

    assert_eq!(drawn.aimed(6), Some(Aimed::Stood(0)));
    assert_eq!(drawn.aimed(7), Some(Aimed::Boxed(0)));
}

#[test]
fn a_click_on_what_a_turn_is_showing_is_a_row_of_that() {
    let mut drawn = Drawn::new(40, 10);
    drawn.under(&standing(), None, Palette::plain()).unwrap();

    assert_eq!(drawn.aimed(9), Some(Aimed::Stood(0)));
}

// Colour, and the markers it replaces.

#[test]
fn a_run_with_colour_in_it_reads_the_markers_out_of_the_answer() {
    let mut drawn = Drawn::new(80, 24);
    drawn.wears(colourful());
    drawn.stream("a **loud** word").unwrap();
    drawn.settle().unwrap();

    let written = drawn.render.terminal.written();
    let read = format!(
        "a {}loud{} word",
        colourful().open(Slot::Strong),
        colourful().close()
    );

    assert!(written.contains(&read), "{written:?}");
    // And the reader sees the words without them.
    assert_eq!(drawn.screen().row(0), "a loud word");
}

#[test]
fn a_slot_costs_the_answer_the_columns_it_would_have_taken_plain() {
    // Two windows of the same width, the same words, one of them told a
    // palette. A slot that cost a column would fold one and not the other.
    let mut plain = Drawn::new(12, 24);
    let mut coloured = Drawn::new(12, 24);
    coloured.wears(colourful());

    plain.stream("the loud word\n").unwrap();
    coloured.stream("the **loud** word\n").unwrap();

    assert_eq!(plain.screen().said(), coloured.screen().said());
}

#[test]
fn a_run_with_no_colour_in_it_keeps_every_marker_the_model_wrote() {
    // Dropping one here would take the emphasis away and put nothing in its
    // place. A file of markdown is worth more than a file it was taken out of.
    let mut drawn = Drawn::new(80, 24);
    drawn.stream("a **loud** word").unwrap();
    drawn.settle().unwrap();

    assert_eq!(drawn.screen().row(0), "a **loud** word");
}

#[test]
fn a_theme_chosen_mid_session_repaints_what_is_already_on_screen() {
    // The record holds spans wearing slots rather than the bytes a terminal
    // would receive, so the palette decides at the moment a row is drawn — and
    // the next frame after a theme is chosen draws every row of the window,
    // including the ones that were already on it.
    let mut drawn = Drawn::new(80, 24);
    drawn.wears(colourful());
    drawn.stream("a **loud** word").unwrap();
    drawn.settle().unwrap();
    drawn.take();

    drawn.wears(Palette::plain());
    drawn.commit("after").unwrap();

    let frame = drawn.render.terminal.written();
    assert!(
        !frame.contains(colourful().open(Slot::Strong).as_str()),
        "{frame:?}"
    );
    let screen = Picture::of(frame, 80, 24);
    assert_eq!(screen.row(0), "a loud word");
    assert_eq!(screen.row(1), "after");
}

#[test]
fn a_fence_the_model_never_closed_does_not_reach_the_next_message() {
    let mut drawn = Drawn::new(80, 24);
    drawn.wears(colourful());
    drawn.stream("```\nunclosed").unwrap();
    drawn.settle().unwrap();
    drawn.take();

    drawn.stream("after **it**").unwrap();
    drawn.settle().unwrap();

    let written = drawn.render.terminal.written();
    assert!(written.contains("after "), "{written:?}");
    assert!(
        written.contains(colourful().open(Slot::Strong).as_str()),
        "{written:?}"
    );
}

// The blank row between one block and the next.

#[test]
fn nothing_is_parted_from_the_start_of_the_session() {
    // A blank first row is a session that opens one line lower than it needed
    // to.
    let mut drawn = Drawn::new(80, 24);
    drawn.apart().unwrap();
    drawn.commit("first").unwrap();

    assert_eq!(drawn.screen().row(0), "first");
}

#[test]
fn one_blank_row_stands_between_two_blocks() {
    let mut drawn = Drawn::new(80, 24);
    drawn.commit("first").unwrap();
    drawn.apart().unwrap();
    drawn.commit("second").unwrap();

    let screen = drawn.screen();
    assert_eq!(screen.row(0), "first");
    assert_eq!(screen.row(1), "");
    assert_eq!(screen.row(2), "second");
}

#[test]
fn blank_rows_do_not_accumulate() {
    let mut drawn = Drawn::new(80, 24);
    drawn.commit("first").unwrap();
    drawn.apart().unwrap();
    drawn.apart().unwrap();
    drawn.apart().unwrap();
    drawn.commit("second").unwrap();

    assert_eq!(drawn.screen().said(), vec!["first", "second"]);
}

#[test]
fn a_row_is_never_parted_into_a_line_that_is_still_arriving() {
    // A caller asks on every delta, because the first is the only one it can
    // ask on. It gets a row before the answer and none inside it.
    let mut drawn = Drawn::new(80, 24);
    drawn.commit("asked").unwrap();
    drawn.apart().unwrap();
    drawn.stream("the ").unwrap();
    drawn.apart().unwrap();
    drawn.stream("answer").unwrap();

    let screen = drawn.screen();
    assert_eq!(screen.row(0), "asked");
    assert_eq!(screen.row(1), "");
    assert_eq!(screen.row(2), "the answer");
    assert_eq!(screen.row(3), "");
}

#[test]
fn rows_this_program_composed_settle_the_question_too() {
    let mut drawn = Drawn::new(80, 24);
    drawn.present(&[Row::plain("composed")]).unwrap();
    drawn.apart().unwrap();
    drawn.commit("after").unwrap();

    assert_eq!(drawn.screen().row(1), "");
    assert_eq!(drawn.screen().row(2), "after");
}

// Counting.

#[test]
fn the_record_counts_every_line_that_has_gone_into_it() {
    let mut drawn = Drawn::new(80, 24);
    assert_eq!(drawn.lines(), 0);

    drawn.commit("one").unwrap();
    drawn.commit("two").unwrap();
    drawn
        .present(&[Row::plain("three"), Row::plain("four")])
        .unwrap();

    assert_eq!(drawn.lines(), 4);
}

// The clipboard.

#[test]
fn a_line_copied_out_reaches_the_terminal_as_one_request_and_nothing_else() {
    // Between frames, deliberately: it is not a row and changes nothing about
    // what the window shows.
    let mut drawn = Drawn::new(80, 24);
    drawn.commit("hello").unwrap();
    let showing = drawn.screen().said();
    drawn.take();

    assert!(drawn.copied("hello").unwrap());

    let written = drawn.render.terminal.written();
    assert!(written.starts_with("\x1b]52;"), "{written:?}");
    assert_eq!(Picture::of(written, 80, 24).said(), Vec::<&str>::new());
    assert_eq!(showing, vec!["hello"]);
}

#[test]
fn a_redirected_run_asks_for_no_clipboard_at_all() {
    let mut render = Renderer::new(Recording::redirected(80, 24));

    assert!(!render.copied("hello").unwrap());
    assert_eq!(render.terminal.written(), "");
}

// What a drag over the window takes.

/// The bytes that would ask a terminal to put `text` on the clipboard.
fn onto_the_clipboard(text: &str) -> String {
    crate::clipboard::copying(text).expect("the sequence was refused")
}

/// A drag from one place on the window to another, and the bytes it wrote.
fn drag(drawn: &mut Drawn, from: (usize, usize), to: (usize, usize)) -> String {
    drawn
        .took(Pressed::Clicked {
            row: from.0,
            column: from.1,
        })
        .unwrap();
    drawn
        .took(Pressed::Dragged {
            row: to.0,
            column: to.1,
        })
        .unwrap();
    drawn.take();
    drawn
        .took(Pressed::Released {
            row: to.0,
            column: to.1,
        })
        .unwrap();
    drawn.take()
}

#[test]
fn a_drag_across_the_transcript_puts_what_it_covered_on_the_clipboard() {
    // The whole gesture, end to end: where it opened, how far it reached, and
    // the text that came back off the rows it covered rather than off the
    // record those rows were folded from.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("first line").unwrap();
    drawn.commit("second line").unwrap();

    let wrote = drag(&mut drawn, (0, 6), (1, 5));

    assert!(
        wrote.contains(&onto_the_clipboard("line\nsecond")),
        "{wrote:?}"
    );
}

#[test]
fn a_drag_takes_what_it_covers_wherever_on_the_window_that_is() {
    // The band the row belongs to is not asked. A reader dragging over their
    // own prompt gets their own prompt, which is the answer for the one place
    // the record could not have given it.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();

    let wrote = drag(&mut drawn, (8, 0), (8, 5));

    assert!(wrote.contains(&onto_the_clipboard("│ ›  │")), "{wrote:?}");
}

#[test]
fn a_drag_is_answered_here_and_the_loop_underneath_never_hears_it() {
    // A drag that reached an input loop would be read as whatever that loop
    // makes of a click — a caret moved, a cut result opened — once per row the
    // pointer crossed.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("a line").unwrap();

    let opened = Pressed::Clicked { row: 0, column: 0 };
    assert_eq!(drawn.took(opened.clone()).unwrap(), Some(opened));
    assert_eq!(
        drawn.took(Pressed::Dragged { row: 0, column: 4 }).unwrap(),
        None
    );
    assert_eq!(
        drawn.took(Pressed::Released { row: 0, column: 4 }).unwrap(),
        None
    );
}

#[test]
fn a_press_that_never_moved_reaches_the_loop_and_copies_nothing() {
    // Clicking is how the caret is placed and how a cut result is opened, and
    // it goes on being that.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("a line").unwrap();
    drawn.take();

    let opened = Pressed::Clicked { row: 0, column: 2 };
    assert_eq!(drawn.took(opened.clone()).unwrap(), Some(opened));
    drawn.took(Pressed::Released { row: 0, column: 2 }).unwrap();

    assert!(!drawn.take().contains("\x1b]52;"));
}

#[test]
fn a_scroll_lets_go_of_a_selection_rather_than_holding_it_over_other_words() {
    // The two ends are screen rows. Moving the picture under them without
    // dropping them leaves a highlight over text nobody dragged over, and a
    // release would copy it.
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 1, column: 4 }).unwrap();
    assert!(drawn.scrolled(-3).unwrap());
    drawn.take();
    drawn.took(Pressed::Released { row: 1, column: 4 }).unwrap();

    assert!(!drawn.take().contains("\x1b]52;"));
}

#[test]
fn a_redirected_run_hands_every_press_straight_on() {
    // Nothing is drawn there, so there is nothing under the pointer to take,
    // and the loop underneath is the only thing that could still make sense of
    // a click.
    let mut drawn = Drawn {
        render: Renderer::new(Recording::redirected(40, 10)),
    };

    for arrived in [
        Pressed::Clicked { row: 0, column: 0 },
        Pressed::Dragged { row: 1, column: 4 },
        Pressed::Released { row: 1, column: 4 },
    ] {
        assert_eq!(drawn.took(arrived.clone()).unwrap(), Some(arrived));
    }
}

#[test]
fn the_pointer_is_only_heard_where_it_crossed_onto_another_row() {
    // A terminal reporting motion reports it a cell at a time. Handing every
    // one of those on would ask the loop above whether the row is worth
    // lighting once per column the pointer moved across it.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("a line").unwrap();

    assert_eq!(
        drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap(),
        Some(Pressed::Hovered { row: 0, column: 0 })
    );
    assert_eq!(
        drawn.took(Pressed::Hovered { row: 0, column: 7 }).unwrap(),
        None
    );
    assert_eq!(
        drawn.took(Pressed::Hovered { row: 1, column: 7 }).unwrap(),
        Some(Pressed::Hovered { row: 1, column: 7 })
    );
}

#[test]
fn lighting_a_row_writes_it_and_lighting_it_again_writes_nothing() {
    // What lets the loop above answer every hearing the same way, without
    // keeping its own record of which row is lit.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("a line").unwrap();
    drawn.take();

    assert!(drawn.hovering(Some(0)).unwrap());
    assert!(drawn.take().contains("\x1b[7m"));

    assert!(!drawn.hovering(Some(0)).unwrap());
    assert_eq!(drawn.take(), String::new());
}

#[test]
fn a_lit_row_goes_dark_when_the_pointer_leaves_it() {
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("a line").unwrap();
    drawn.hovering(Some(0)).unwrap();
    drawn.take();

    assert!(drawn.hovering(None).unwrap());
    let wrote = drawn.take();
    assert!(
        wrote.contains("a line") && !wrote.contains("\x1b[7m"),
        "{wrote:?}"
    );
}

#[test]
fn a_scroll_puts_out_the_light_and_lets_the_pointer_be_heard_again() {
    // Both halves of one fact: the row under the pointer is showing another
    // line now, so what it was lit for has gone, and the very next report of a
    // pointer that never moved has to be able to light whatever is there.
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap();
    drawn.hovering(Some(0)).unwrap();

    assert!(drawn.scrolled(-3).unwrap());
    drawn.take();

    assert_eq!(
        drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap(),
        Some(Pressed::Hovered { row: 0, column: 0 })
    );
    assert!(drawn.hovering(Some(0)).unwrap());
}

#[test]
fn nothing_is_lit_on_a_run_that_is_writing_somewhere_other_than_a_screen() {
    // There is no picture to light, and a reverse-video escape in a redirected
    // run is a byte in somebody's log file.
    let mut drawn = Drawn {
        render: Renderer::new(Recording::redirected(40, 10)),
    };
    drawn.commit("a line").unwrap();
    drawn.take();

    assert!(!drawn.hovering(Some(0)).unwrap());
    assert_eq!(drawn.take(), String::new());
}
