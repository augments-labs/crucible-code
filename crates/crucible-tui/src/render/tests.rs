//! What the renderer draws, asserted against the window it drew on.
//!
//! Almost nothing here reads bytes. A frame names the row it writes, so the
//! bytes are replayed into a [`Picture`] and the assertions are about what a
//! reader would be looking at — which is the thing that has to be right, and
//! the thing that stays readable when the sequences underneath it change. The
//! sequences themselves are asserted once, next to the type that writes them.

use std::sync::atomic::{AtomicBool, Ordering};

use unicode_width::UnicodeWidthStr;

use super::*;
use crate::color::{Palette, Slot, Theme};
use crate::prompt::Prompt;
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
fn rows_standing_instead_of_the_box_take_it_off_in_one_frame() {
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.commit("answer").unwrap();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    let before = drawn.terminal().flushes();

    drawn
        .instead(&[], &[Row::plain("view")], None, Palette::plain())
        .unwrap();

    let screen = drawn.screen();
    assert_eq!(screen.row(0), "answer");
    assert_eq!(screen.row(9), "view");
    assert!((0..10).all(|at| !screen.row(at).starts_with('\u{256d}')));
    assert_eq!(drawn.terminal().flushes(), before + 1);
}

#[test]
fn replacing_the_foot_flushes_turn_prompt_and_status_once() {
    let mut drawn = Drawn::new(60, 10);
    let old = vec![Row::plain("old prompt"), Row::plain("old status")];
    drawn
        .replace(
            PromptRows {
                rows: &old,
                caret: Caret::default(),
                pointed: None,
            },
            &[Row::plain("old turn")],
            Palette::plain(),
        )
        .unwrap();
    let before = drawn.terminal().flushes();

    let new = vec![Row::plain("new prompt"), Row::plain("new status")];
    drawn
        .replace(
            PromptRows {
                rows: &new,
                caret: Caret::default(),
                pointed: None,
            },
            &[Row::plain("new turn")],
            Palette::plain(),
        )
        .unwrap();

    assert_eq!(drawn.terminal().flushes(), before + 1);
    let screen = drawn.screen().said();
    assert!(screen.iter().any(|row| row == "new turn"), "{screen:?}");
    assert!(screen.iter().any(|row| row == "new prompt"), "{screen:?}");
    assert!(screen.iter().any(|row| row == "new status"), "{screen:?}");
}

#[test]
fn prompt_row_construction_rejects_mismatched_targets_and_carets() {
    let rows = vec![Row::plain("prompt"), Row::plain("2 commands")];
    let other = Row::plain("different action");
    let target = rows.get(1).expect("the command row");

    assert!(PromptRows::new(&rows, Caret::default(), Some((2, target))).is_none());
    assert!(PromptRows::new(&rows, Caret::default(), Some((1, &other))).is_none());
    assert!(PromptRows::new(&rows, Caret { row: 3, column: 0 }, None,).is_none());
    assert!(PromptRows::new(&rows, Caret::default(), Some((1, target))).is_some());
}

#[test]
fn a_pointable_prompt_defers_one_frame_until_its_target_is_replaced() {
    let mut drawn = Drawn::new(60, 10);
    drawn.wears(colourful());
    let prompt = vec![
        Row::plain("prompt"),
        Row::new().then(Slot::Accent, "2 commands"),
    ];
    let pointed = Row::new().then(Slot::Pointed, "2 commands");
    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            colourful(),
        )
        .unwrap();
    drawn.take();
    let at = drawn.bands().prompt.start + 1;

    assert_eq!(
        drawn.took(Pressed::Hovered { row: at, column: 0 }).unwrap(),
        None
    );
    assert_eq!(drawn.take(), "", "the renderer flushed before replacement");
    assert!(drawn.pointed_changed());

    let before = drawn.terminal().flushes();
    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            colourful(),
        )
        .unwrap();
    assert_eq!(drawn.terminal().flushes(), before + 1);
    assert!(drawn.take().contains("48;"), "the target gained no ground");

    drawn.took(Pressed::Hovered { row: at, column: 1 }).unwrap();
    assert!(
        !drawn.pointed_changed(),
        "motion inside one target asked for a frame"
    );
    assert_eq!(drawn.take(), "");

    drawn
        .took(Pressed::Hovered {
            row: at.saturating_sub(1),
            column: 1,
        })
        .unwrap();
    assert!(
        drawn.pointed_changed(),
        "leaving the target was not reported"
    );
    assert_eq!(drawn.take(), "", "leaving flushed before replacement");

    let before = drawn.terminal().flushes();
    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            colourful(),
        )
        .unwrap();
    assert_eq!(drawn.terminal().flushes(), before + 1);
}

#[test]
fn a_panel_that_fills_the_room_keeps_its_last_row() {
    // The height a panel is laid out into is the room it is given. A panel
    // handed more than that loses its bottom row — which is the row every one
    // of them says which keys it answers to on.
    let mut drawn = Drawn::new(40, 10);

    let room = drawn.room();
    let rows: Vec<Row> = (0..room)
        .map(|at| Row::new().then(Slot::Plain, format!("row {at}")))
        .collect();
    drawn.under(&rows, None, Palette::plain()).unwrap();

    let last = format!("row {}", room - 1);
    let said = drawn.screen().said();
    assert!(said.iter().any(|row| row == &last), "{said:?}");
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
fn a_resize_no_poller_reported_is_drawn_at_the_new_size_on_the_next_frame() {
    // A window resized while nothing waits on the keyboard, as while an
    // answer streams: the operating system says so, and the next frame is
    // drawn at the size the window has now rather than the one it had.
    let mut drawn = Drawn::new(20, 8);
    let resizes = ResizeFlag::default();
    drawn.watches_size(resizes.clone());
    drawn.commit("the quick brown fox jumps").unwrap();
    assert_eq!(drawn.screen().row(0), "the quick brown fox");

    drawn.render.terminal.resize(40, 8);
    resizes.raise();
    drawn.commit("over the lazy dog").unwrap();

    assert_eq!(drawn.screen().row(0), "the quick brown fox jumps");
    assert_eq!(drawn.columns(), 40);
}

#[test]
fn a_committed_prompt_is_laid_out_again_when_the_window_widens() {
    let mut drawn = Drawn::new(20, 8);
    let said = "the quick brown fox jumps";
    drawn
        .responsive(
            said.len(),
            Box::new(move |columns| Prompt::committed(said, columns, Glyphs::Unicode, false)),
        )
        .unwrap();
    assert!(drawn.screen().said().len() > 1, "the prompt did not wrap");

    drawn.render.terminal.resize(40, 8);
    drawn.resized().unwrap();

    assert_eq!(drawn.screen().row(0), format!("› {said}"));
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
fn a_resize_keeps_what_was_standing_cut_at_the_new_width() {
    // A box laid out against a window that has gone stays on screen until the
    // caller lays out the next one, cut at the new edge rather than drawn
    // past it.
    let mut drawn = Drawn::new(40, 10);
    drawn
        .live(
            &[Row::plain("a box row that is thirty-four wide")],
            Caret::default(),
            Palette::plain(),
        )
        .unwrap();

    drawn.render.terminal.resize(20, 10);
    drawn.resized().unwrap();

    assert_eq!(drawn.screen().said(), ["a box row that is th"]);
}

#[test]
fn a_standing_row_wider_than_it_was_laid_for_is_kept_no_wider() {
    // What stands is kept to be painted again at the next width, and what a
    // window that widens later can show of it is what it was laid out for.
    // A row a hundred thousand columns wide handed to a window forty wide
    // was kept whole beside its painted copy of forty.
    let mut drawn = Drawn::new(40, 10);
    let wide = "w".repeat(100_000);
    running(&mut drawn, &[Row::plain(&wide)], &[Row::plain(&wide)]);

    let kept: Vec<usize> = drawn
        .standing
        .running
        .iter()
        .chain(&drawn.standing.over)
        .map(Row::columns)
        .collect();
    assert_eq!(kept, [drawn.transcript_columns(), drawn.columns()]);
}

#[test]
fn a_standing_row_laid_for_the_window_comes_back_whole_when_it_widens_again() {
    // Kept no wider than it was laid for is still kept whole: a window that
    // narrows and widens back before the caller lays out the next one shows
    // the row as it was handed in.
    let mut drawn = Drawn::new(40, 10);
    let row = "a turn row laid out to forty columns wide";
    let row = &row[..40];
    running(&mut drawn, &[Row::plain(row)], &[Row::plain(row)]);

    drawn.render.terminal.resize(20, 10);
    drawn.resized().unwrap();
    drawn.render.terminal.resize(40, 10);
    drawn.resized().unwrap();

    let screen = drawn.screen();
    let bands = drawn.bands();
    assert_eq!(screen.row(bands.turn.start), row);
    assert_eq!(screen.row(bands.turn.start + 1), row);
}

#[test]
fn a_resize_press_taken_by_the_renderer_folds_the_record_again() {
    // The seam every input loop reads through takes the new size itself, so
    // the loop is handed the press only to lay out again what it stands.
    let mut drawn = Drawn::new(20, 8);
    drawn.commit("the quick brown fox jumps").unwrap();

    drawn.render.terminal.resize(40, 8);
    let left = drawn.render.took(Pressed::Resized).unwrap();

    assert_eq!(left, Some(Pressed::Resized));
    assert_eq!(drawn.screen().row(0), "the quick brown fox jumps");
}

#[test]
fn a_resize_drops_a_stale_prompt_hover_target_until_replacement_reflows_it() {
    let mut drawn = Drawn::new(60, 10);
    let prompt = vec![
        Row::plain("prompt"),
        Row::new().then(Slot::Accent, "2 commands"),
    ];
    let pointed = Row::new().then(Slot::Pointed, "2 commands");
    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            colourful(),
        )
        .unwrap();
    let target = drawn.bands().prompt.start + 1;
    drawn
        .took(Pressed::Hovered {
            row: target,
            column: 0,
        })
        .unwrap();
    assert!(drawn.pointed_changed());

    drawn.render.terminal.resize(40, 10);
    drawn.resized().unwrap();

    assert!(drawn.prompt_target.is_none());
    assert!(!drawn.pointed_changed());
    // The box stays, painted again as it was handed in, without the pointed
    // row the hover put in it.
    assert_eq!(drawn.screen().said(), ["prompt", "2 commands"]);

    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            colourful(),
        )
        .unwrap();
    assert_eq!(drawn.prompt_target, Some((1, 0..10)));
    assert!(drawn.prompt_pointed());
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
fn a_format_character_from_elsewhere_reaches_no_screen_and_no_file() {
    // Streamed, committed or written to a pipe, a right-to-left override would
    // reorder what is drawn after it, so it is dropped on the way in.
    for mut drawn in [
        Renderer::new(Recording::new(80, 24)),
        Renderer::new(Recording::redirected(80, 24)),
    ] {
        drawn.commit("result \u{202e}txt.exe").unwrap();
        drawn.stream("an \u{2067}answer\u{2069}").unwrap();
        drawn.settle().unwrap();

        let written = drawn.terminal.written();
        assert!(!written.contains('\u{202e}'), "{written:?}");
        assert!(!written.contains('\u{2067}'), "{written:?}");
        assert!(written.contains("result txt.exe"), "{written:?}");
    }
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
fn a_click_on_any_row_of_a_cut_result_names_the_row_that_offered_it() {
    // A result long enough to wrap is laid as several rows wearing the cut
    // slot, and the offer to open it was kept against the first. A reader
    // pointing at the second row of the sentence is pointing at the sentence,
    // so every row of it answers with the first — and the rows either side of
    // the result, which do not wear the slot, still answer for themselves.
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("the call").unwrap();
    let offered = drawn.lines();
    drawn
        .present(&[
            Row::new().then(Slot::Cut, "the first row of what came back"),
            Row::new().then(Slot::Cut, "and the second row of it"),
        ])
        .unwrap();
    drawn.commit("the answer").unwrap();

    assert_eq!(drawn.aimed(0), Some(Aimed::Line(offered - 1)));
    assert_eq!(drawn.aimed(1), Some(Aimed::Line(offered)));
    assert_eq!(drawn.aimed(2), Some(Aimed::Line(offered)));
    assert_eq!(drawn.aimed(3), Some(Aimed::Line(offered + 2)));
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

#[test]
fn a_row_standing_over_the_box_counts_only_the_cells_it_drew() {
    // A list's row is the list's only as far as it drew: the indent before
    // its mark and the blank after its last character are the window's, the
    // same as on a row of the transcript. A wide character is two cells.
    let mut drawn = Drawn::new(40, 10);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    drawn
        .under(
            &[
                Row::plain("  › first"),
                Row::plain("    ‹日本›"),
                Row::new(),
            ],
            None,
            Palette::plain(),
        )
        .unwrap();
    let top = drawn.bands().turn.start;

    assert_eq!(drawn.aimed(top), Some(Aimed::Stood(0)));
    assert_eq!(drawn.cells(top), 2..9);
    assert_eq!(drawn.cells(top + 1), 4..10);
    assert_eq!(drawn.cells(top + 2), 0..0, "a blank row draws nothing");
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
        colourful().open(Slot::Bold),
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
        !frame.contains(colourful().open(Slot::Bold).as_str()),
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
        written.contains(colourful().open(Slot::Bold).as_str()),
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
fn a_block_cut_at_a_line_break_draws_what_an_uncut_one_draws() {
    // The cut the reader cannot see. A delta ending exactly at the line break
    // leaves it holding nothing at all — the line it was reading is finished,
    // and the block that line belongs to is not — so the row a caller asks for
    // between blocks lands inside one.
    const SAID: &str = "```\nfirst\nsecond\n```\n";

    let whole = {
        let mut drawn = Drawn::new(80, 24);
        drawn.wears(colourful());
        drawn.apart().unwrap();
        drawn.stream(SAID).unwrap();
        drawn.settle().unwrap();
        drawn.screen().rows()
    };

    let mut drawn = Drawn::new(80, 24);
    drawn.wears(colourful());
    for piece in SAID.split_inclusive('\n') {
        drawn.apart().unwrap();
        drawn.stream(piece).unwrap();
    }
    drawn.settle().unwrap();

    assert_eq!(drawn.screen().rows(), whole);
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
fn a_structural_result_glyph_is_not_lit_or_copied() {
    let mut drawn = Drawn::new(40, 10);
    let from = drawn.lines();
    drawn.commit("result").unwrap();
    drawn.subordinate(from, Glyphs::Unicode).unwrap();
    drawn.take();

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 0, column: 7 }).unwrap();
    let highlighted = drawn.take();
    assert!(!highlighted.contains("\x1b[7m⎿"), "{highlighted:?}");

    drawn.took(Pressed::Released { row: 0, column: 7 }).unwrap();
    let copied = drawn.take();
    assert!(
        copied.contains(&onto_the_clipboard(" result")),
        "{copied:?}"
    );
    assert!(
        !copied.contains(&onto_the_clipboard("⎿ result")),
        "{copied:?}"
    );
}

#[test]
fn a_redirected_result_is_not_given_a_late_trailing_prefix() {
    let mut drawn = Drawn {
        render: Renderer::new(Recording::redirected(40, 10)),
    };
    let from = drawn.lines();
    drawn.commit("result").unwrap();
    drawn.subordinate(from, Glyphs::Unicode).unwrap();

    assert_eq!(drawn.render.terminal.written(), "result\n");
}

#[test]
fn a_literal_result_glyph_remains_lit_and_copied() {
    let mut drawn = Drawn::new(40, 10);
    drawn.commit("⎿ literal").unwrap();
    drawn.take();

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 0, column: 1 }).unwrap();
    let highlighted = drawn.take();
    assert!(highlighted.contains("\x1b[7m⎿"), "{highlighted:?}");

    drawn.took(Pressed::Released { row: 0, column: 1 }).unwrap();
    let copied = drawn.take();
    assert!(copied.contains(&onto_the_clipboard("⎿")), "{copied:?}");
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

/// Forty one-row lines on an eight-row window with nothing else standing in
/// it: following the foot, the band shows `line 32` to `line 39`.
fn forty_lines() -> Drawn {
    let mut drawn = Drawn::new(40, 8);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.take();
    drawn
}

#[test]
fn a_scroll_carries_a_selection_with_the_words_it_covered() {
    // The two ends name record rows, not window rows. Moving the band under
    // them moves the highlight with the text, and a release copies the words
    // the reader dragged over rather than whatever has arrived under the rows.
    let mut drawn = forty_lines();

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 1, column: 4 }).unwrap();
    drawn.took(Pressed::Released { row: 1, column: 4 }).unwrap();
    drawn.take();

    assert!(drawn.scrolled(-3).unwrap());
    assert_eq!(drawn.screen().row(3), "line 32");
    let frame = drawn.take();
    assert!(frame.contains("\x1b[7mline 32"), "{frame:?}");
    assert!(!frame.contains("\x1b[7mline 29"), "{frame:?}");
}

#[test]
fn a_wheel_turned_with_the_button_down_scrolls_and_reaches_the_words_under_the_pointer() {
    // Answered here rather than handed on: the loop underneath would scroll it
    // a second time, and the drag would be left short of the rows that just
    // arrived under the pointer.
    let mut drawn = forty_lines();

    drawn.took(Pressed::Clicked { row: 2, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 3, column: 4 }).unwrap();
    assert_eq!(
        drawn.took(Pressed::Scrolled { back: true }).unwrap(),
        None,
        "the wheel reached the loop underneath"
    );
    assert_eq!(drawn.screen().row(0), "line 29");
    drawn.take();

    drawn.took(Pressed::Released { row: 3, column: 4 }).unwrap();
    let wrote = drawn.take();
    assert!(
        wrote.contains(&onto_the_clipboard(" 32\nline 33\nl")),
        "{wrote:?}"
    );
}

#[test]
fn a_wheel_turned_after_the_button_came_up_is_the_loops_to_answer() {
    let mut drawn = forty_lines();

    drawn.took(Pressed::Clicked { row: 2, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 3, column: 4 }).unwrap();
    drawn.took(Pressed::Released { row: 3, column: 4 }).unwrap();

    assert_eq!(
        drawn.took(Pressed::Scrolled { back: true }).unwrap(),
        Some(Pressed::Scrolled { back: true })
    );
}

#[test]
fn a_drag_reaching_the_top_of_the_transcript_scrolls_it_back_a_row() {
    let mut drawn = forty_lines();

    drawn.took(Pressed::Clicked { row: 3, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 0, column: 0 }).unwrap();

    assert_eq!(drawn.screen().row(0), "line 31");
    // The row the button went down on has moved with the words.
    assert_eq!(drawn.screen().row(4), "line 35");
    let frame = drawn.take();
    assert!(frame.contains("\x1b[7mline 31"), "{frame:?}");
    assert!(frame.contains("\x1b[7ml\x1b[27mine 35"), "{frame:?}");
}

#[test]
fn a_drag_resting_at_the_top_of_the_transcript_keeps_scrolling_it_back() {
    // Nothing more arrives from the terminal while the pointer rests, so the
    // input wait wakes at the creep's deadline and takes the next row itself.
    let mut drawn = forty_lines();

    drawn.took(Pressed::Clicked { row: 3, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 0, column: 0 }).unwrap();
    assert_eq!(drawn.screen().row(0), "line 31");

    assert!(
        drawn.rests_in().is_some_and(|rest| rest <= CREEP),
        "no wake was scheduled for the resting drag"
    );
    drawn.render.creeps = Some(Instant::now());
    assert!(drawn.render.repose().unwrap());
    assert_eq!(drawn.screen().row(0), "line 30");

    // Released, the pointer is nobody's clock.
    drawn.took(Pressed::Released { row: 0, column: 0 }).unwrap();
    assert_eq!(drawn.rests_in(), None);
}

#[test]
fn a_drag_reaching_the_foot_of_the_transcript_scrolls_it_on_until_the_record_ends() {
    let mut drawn = forty_lines();
    assert!(drawn.scrolled(-10).unwrap());
    assert_eq!(drawn.screen().row(0), "line 22");

    drawn.took(Pressed::Clicked { row: 2, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 7, column: 3 }).unwrap();
    assert_eq!(drawn.screen().row(0), "line 23");

    for _ in 0..20 {
        drawn.render.creeps = Some(Instant::now());
        drawn.render.repose().unwrap();
    }
    assert_eq!(drawn.screen().row(0), "line 32");
    assert_eq!(drawn.rests_in(), None, "the foot is not a clock to keep");
}

#[test]
fn what_a_drag_scrolled_off_the_window_is_copied_with_the_rest() {
    // The button went down on `line 22`, and the band was carried on until
    // that row was above the window. The reader dragged over everything
    // between, so everything between is what they get.
    let mut drawn = forty_lines();
    assert!(drawn.scrolled(-10).unwrap());

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 6, column: 6 }).unwrap();
    assert!(drawn.scrolled(5).unwrap());
    drawn.took(Pressed::Dragged { row: 6, column: 6 }).unwrap();
    drawn.take();
    drawn.took(Pressed::Released { row: 6, column: 6 }).unwrap();

    let expected: Vec<String> = (22..=33).map(|line| format!("line {line}")).collect();
    let wrote = drawn.take();
    assert!(
        wrote.contains(&onto_the_clipboard(&expected.join("\n"))),
        "{wrote:?}"
    );
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

// A pointer resting on a result the transcript cut short.

/// A row of the transcript offering more of a result than it is showing.
fn cut(said: &str) -> Row {
    Row::new().then(Slot::Cut, said)
}

/// What that row's words look like on the wire while nothing points at them.
///
/// The quiet, which is what every subdued row of the transcript wears — the
/// point being that at rest a cut result is not told apart from one.
fn quietly(said: &str) -> String {
    format!(
        "{}{said}{}",
        colourful().open(Slot::Quiet),
        colourful().close()
    )
}

#[test]
fn a_pointer_lights_the_cut_result_it_is_on_and_leaves_the_others_alone() {
    // What a pointer asks is what *this* opens, so the one it is over is the
    // one that lights. A click on that row opens that result, and a reader who
    // saw two light would have been shown one thing and given another.
    let mut drawn = Drawn::new(40, 10);
    drawn.wears(colourful());
    drawn.present(&[cut("first")]).unwrap();
    drawn.commit("a line with nothing cut from it").unwrap();
    drawn.present(&[cut("second")]).unwrap();

    // Both of them subdued, drawn before a pointer was ever heard of.
    let resting = drawn.take();
    assert!(resting.contains(&quietly("first")), "{resting:?}");
    assert!(resting.contains(&quietly("second")), "{resting:?}");

    drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap();

    // A frame writes only what changed, so the one left alone is not in it at
    // all -- which is the strongest thing the wire can say about a row that did
    // not move: it is still wearing what it was.
    let frame = drawn.take();
    assert!(frame.contains("first"), "{frame:?}");
    assert!(!frame.contains(&quietly("first")), "{frame:?}");
    assert!(!frame.contains("second"), "{frame:?}");
}

#[test]
fn a_pointer_moved_from_one_cut_result_to_another_lights_the_one_it_arrived_at() {
    let mut drawn = Drawn::new(40, 10);
    drawn.wears(colourful());
    drawn.present(&[cut("first")]).unwrap();
    drawn.commit("the call the next result answers").unwrap();
    drawn.present(&[cut("second")]).unwrap();
    drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap();
    drawn.take();

    drawn.took(Pressed::Hovered { row: 2, column: 0 }).unwrap();

    let frame = drawn.take();
    assert!(frame.contains(&quietly("first")), "{frame:?}");
    assert!(frame.contains("second"), "{frame:?}");
    assert!(!frame.contains(&quietly("second")), "{frame:?}");
}

#[test]
fn a_result_written_down_over_several_rows_lights_on_all_of_them() {
    // A result is written down in one go and nothing is written in the middle
    // of it, so the rows of one are next to each other. Pointing at any of them
    // lights the whole result -- which is what a reader needs to know before
    // clicking, since what opens is the result rather than the row.
    let mut drawn = Drawn::new(40, 10);
    drawn.wears(colourful());
    drawn.commit("the call it answers").unwrap();
    drawn
        .present(&[cut("head of it"), cut("more of it"), cut("foot of it")])
        .unwrap();
    drawn.commit("what the model said next").unwrap();
    drawn.take();

    // The middle row, so the light has to reach in both directions.
    drawn.took(Pressed::Hovered { row: 2, column: 0 }).unwrap();

    let frame = drawn.take();
    for said in ["head of it", "more of it", "foot of it"] {
        assert!(frame.contains(said), "{said}: {frame:?}");
        assert!(!frame.contains(&quietly(said)), "{said}: {frame:?}");
    }

    // And nothing either side of the result went anywhere.
    assert!(!frame.contains("the call it answers"), "{frame:?}");
    assert!(!frame.contains("what the model said next"), "{frame:?}");
}

#[test]
fn a_pointer_that_moved_off_puts_the_cut_result_back_in_the_quiet() {
    let mut drawn = Drawn::new(40, 10);
    drawn.wears(colourful());
    drawn.present(&[cut("first")]).unwrap();
    drawn.commit("a line with nothing cut from it").unwrap();
    drawn.present(&[cut("second")]).unwrap();
    drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap();
    drawn.take();

    drawn.took(Pressed::Hovered { row: 1, column: 0 }).unwrap();

    let frame = drawn.take();
    assert!(frame.contains(&quietly("first")), "{frame:?}");
    assert!(!frame.contains("second"), "{frame:?}");
}

#[test]
fn a_cut_result_that_moved_out_from_under_a_still_pointer_goes_quiet_again() {
    // What is under the pointer is worked out for every frame rather than
    // remembered, which is what keeps it right while an answer arrives under a
    // pointer nobody has touched: the row stays where it was and the transcript
    // does not.
    let mut drawn = Drawn::new(40, 4);
    drawn.wears(colourful());
    drawn.commit("one").unwrap();
    drawn.present(&[cut("alpha")]).unwrap();
    drawn.commit("two").unwrap();
    drawn.commit("three").unwrap();
    drawn.take();

    drawn.took(Pressed::Hovered { row: 1, column: 0 }).unwrap();
    let frame = drawn.take();
    assert!(frame.contains("alpha"), "{frame:?}");
    assert!(!frame.contains(&quietly("alpha")), "{frame:?}");

    // One more line, and the row the pointer is on is one the transcript said
    // in full.
    drawn.commit("four").unwrap();

    let frame = drawn.take();
    assert!(frame.contains(&quietly("alpha")), "{frame:?}");
}

// What the pointer lights is a result's own cells, not the row it is on.

/// A result row laid the way a turn lays one: hung off the call under a
/// corner, the words it was cut to, then the offer to open the rest.
fn hung(said: &str, offer: &str) -> Row {
    Row::new()
        .then(Slot::Plain, "  ")
        .then_structural(Slot::Quiet, "⎿")
        .then(Slot::Quiet, " ")
        .then(Slot::Cut, said)
        .then(Slot::Quiet, offer)
}

/// Whether the frame `drawn` wrote last lit `said`: written, and not in the
/// quiet a result rests in.
fn lit(drawn: &mut Drawn, said: &str) -> bool {
    let frame = drawn.take();
    frame.contains(said) && !frame.contains(&quietly(said))
}

#[test]
fn a_pointer_right_of_a_cut_result_lights_nothing() {
    // The row ends where its last character does, and a wide one is two cells:
    // "  ⎿ alpha 漢" draws twelve, so the twelfth lights and the cell after it,
    // blank to the end of the window, is no part of the result.
    let mut drawn = Drawn::new(40, 8);
    drawn.wears(colourful());
    drawn.present(&[hung("alpha 漢", "")]).unwrap();
    drawn.take();

    drawn.took(Pressed::Hovered { row: 0, column: 12 }).unwrap();
    assert!(drawn.take().is_empty(), "a blank cell lit the result");
    drawn.took(Pressed::Hovered { row: 0, column: 30 }).unwrap();
    assert!(drawn.take().is_empty(), "a blank cell lit the result");

    drawn.took(Pressed::Hovered { row: 0, column: 11 }).unwrap();
    assert!(
        lit(&mut drawn, "alpha 漢"),
        "its last cell did not light it"
    );
}

#[test]
fn a_pointer_left_of_a_cut_result_lights_nothing() {
    // The corner is the first cell the row drew; the two before it are the
    // indent under the call, and blank.
    let mut drawn = Drawn::new(40, 8);
    drawn.wears(colourful());
    drawn.present(&[hung("alpha", "")]).unwrap();
    drawn.take();

    for column in [0, 1] {
        drawn.took(Pressed::Hovered { row: 0, column }).unwrap();
        assert!(
            drawn.take().is_empty(),
            "blank cell {column} lit the result"
        );
    }

    drawn.took(Pressed::Hovered { row: 0, column: 2 }).unwrap();
    assert!(lit(&mut drawn, "alpha"), "the corner did not light it");
}

#[test]
fn a_pointer_on_the_offer_lights_the_result() {
    // The offer is drawn by the result's row, so it is the result's to light.
    let offer = " (+3 lines · ctrl+o to expand)";
    let mut drawn = Drawn::new(60, 8);
    drawn.wears(colourful());
    drawn.present(&[hung("alpha", offer)]).unwrap();
    drawn.take();

    let on = width::columns("  ⎿ alpha (+3 lines · ctrl+o");
    drawn.took(Pressed::Hovered { row: 0, column: on }).unwrap();
    assert!(
        lit(&mut drawn, "alpha"),
        "the offer did not light the result"
    );
}

#[test]
fn a_pointer_on_the_second_row_of_a_wrapped_result_lights_it_only_on_its_drawn_cells() {
    // A result wrapped under a mark is indented on the rows after the first,
    // and shorter than the first: the indent is blank, and so is every cell
    // past the row's own last character, however far the row above reaches.
    let mut drawn = Drawn::new(40, 8);
    drawn.wears(colourful());
    drawn
        .present(&[
            Row::new()
                .then(Slot::Plain, "● ")
                .then(Slot::Cut, "Read 3 files, searched for"),
            Row::plain("  ").then(Slot::Cut, "2 patterns"),
        ])
        .unwrap();
    drawn.take();

    for column in [0, 1, 12, 20] {
        drawn.took(Pressed::Hovered { row: 1, column }).unwrap();
        assert!(
            drawn.take().is_empty(),
            "blank cell {column} lit the result"
        );
    }

    drawn.took(Pressed::Hovered { row: 1, column: 2 }).unwrap();
    let frame = drawn.take();
    for said in ["Read 3 files, searched for", "2 patterns"] {
        assert!(frame.contains(said), "{said}: {frame:?}");
        assert!(!frame.contains(&quietly(said)), "{said}: {frame:?}");
    }
}

// What the pointer lights on the row under the box is the count's own cells.

/// A pointable row saying the mode before the count, the way the row under the
/// box says them, drawn in its resting state, and the window row it is on.
fn counting() -> (Drawn, usize) {
    let mut drawn = Drawn::new(60, 10);
    drawn.wears(colourful());
    let prompt = vec![
        Row::plain("prompt"),
        Row::new()
            .then(Slot::Plain, "ask mode")
            .then(Slot::Quiet, " · ")
            .then(Slot::Accent, "2 commands"),
    ];
    let pointed = Row::new()
        .then(Slot::Plain, "ask mode")
        .then(Slot::Quiet, " · ")
        .then(Slot::Pointed, "2 commands");
    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            colourful(),
        )
        .unwrap();
    drawn.take();
    let at = drawn.bands().prompt.start + 1;
    (drawn, at)
}

#[test]
fn the_count_s_cells_are_the_words_naming_it() {
    let (drawn, at) = counting();
    assert_eq!(drawn.cells(at), 11..21);
    assert_eq!(drawn.cells(at - 1), 0..0, "a row of the box with no door");
}

#[test]
fn a_pointer_beside_the_count_lights_nothing() {
    let (mut drawn, at) = counting();

    for column in [0, 10, 21, 59] {
        drawn.took(Pressed::Hovered { row: at, column }).unwrap();
        assert!(
            !drawn.pointed_changed(),
            "blank or mode cell {column} lit the count"
        );
    }

    drawn
        .took(Pressed::Hovered {
            row: at,
            column: 11,
        })
        .unwrap();
    assert!(drawn.pointed_changed(), "the count's own cell lit nothing");
    drawn
        .took(Pressed::Hovered {
            row: at,
            column: 20,
        })
        .unwrap();
    assert!(
        !drawn.pointed_changed(),
        "motion inside the count asked for a frame"
    );
    drawn
        .took(Pressed::Hovered {
            row: at,
            column: 21,
        })
        .unwrap();
    assert!(
        drawn.pointed_changed(),
        "leaving the count was not reported"
    );
}

// The scroll rail.

/// A renderer with the rail on, on a window sixty columns wide and ten rows
/// tall, holding eighty numbered lines with a prompt mark before every
/// twentieth. Following the foot, the band shows `line 70` to `line 79`.
fn railed() -> Drawn {
    railed_in(Palette::plain())
}

/// [`railed`], painted from `palette`.
fn railed_in(palette: Palette) -> Drawn {
    let mut drawn = Drawn::new(60, 10);
    drawn.rails(true);
    drawn.wears(palette);
    for line in 0..80 {
        if line % 20 == 0 {
            drawn.landmark();
        }
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn
}

/// The rail's column, read down the transcript band: one character a row,
/// a space where nothing is drawn.
fn rail_of(drawn: &Drawn) -> String {
    let screen = drawn.screen();
    let column = drawn.columns() - 1;
    drawn
        .bands()
        .transcript
        .map(|row| screen.row(row).chars().nth(column).unwrap_or(' '))
        .collect()
}

/// A press on the rail's column at window row `row`.
fn rail_click(drawn: &mut Drawn, row: usize) -> Option<Pressed> {
    let column = drawn.columns() - 1;
    drawn.took(Pressed::Clicked { row, column }).unwrap()
}

#[test]
fn a_rail_left_off_draws_nothing_and_takes_no_column() {
    let mut drawn = Drawn::new(40, 8);
    let (rows, caret) = boxed();
    drawn.live(&rows, caret, Palette::plain()).unwrap();
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    let full = "x".repeat(40);
    drawn.commit(&full).unwrap();

    // The transcript has the whole width, and no row of the window is held
    // back for anything under the box.
    assert_eq!(drawn.transcript_columns(), 40);
    let screen = drawn.screen();
    let foot = drawn.bands().transcript.end - 1;
    assert_eq!(screen.row(foot), full);
    assert_eq!(screen.row(7), "╰────╯");
    assert_eq!(drawn.room(), 8);
    for row in screen.rows() {
        assert!(
            !row.contains(['┃', '│', '•']) || row.starts_with('│'),
            "{row:?}"
        );
    }
}

#[test]
fn the_rail_takes_the_last_column_and_the_transcript_folds_one_narrower() {
    let mut drawn = Drawn::new(40, 8);
    drawn.rails(true);
    for line in 0..40 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.commit(&"x".repeat(40)).unwrap();

    assert_eq!(drawn.transcript_columns(), 39);
    let screen = drawn.screen();
    assert_eq!(screen.row(6), format!("{}┃", "x".repeat(39)));
    assert_eq!(screen.row(7), format!("x{}┃", " ".repeat(38)));

    // And again at whatever width the window is pulled to.
    drawn.render.terminal.resize(30, 8);
    drawn.resized().unwrap();
    assert_eq!(drawn.transcript_columns(), 29);
    assert_eq!(drawn.screen().row(6), format!("{}┃", "x".repeat(29)));
}

#[test]
fn the_rail_thumb_stands_at_the_end_the_middle_and_the_top_of_the_transcript() {
    // Eighty rows on a rail of ten: eight to a rail row, the band two rail
    // rows long, and the prompts at lines 0, 20, 40 and 60 on rail rows 0, 2,
    // 5 and 7.
    let mut drawn = railed();
    assert_eq!(rail_of(&drawn), "•│•││•│●┃┃");

    drawn.scrolled(-35).unwrap();
    assert_eq!(
        drawn
            .screen()
            .row(0)
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>(),
        ["line", "35"]
    );
    assert_eq!(rail_of(&drawn), "•│•│┃●│•││");

    drawn.scrolled(-35).unwrap();
    assert_eq!(rail_of(&drawn), "●┃•││•│•││");
}

#[test]
fn a_click_on_the_rail_seeks_there_and_goes_no_further() {
    let mut drawn = railed();

    // Rail row 3 carries no mark, so the thumb's middle goes there: the
    // band's top is sixteen rows in.
    assert_eq!(rail_click(&mut drawn, 3), None);
    assert!(
        drawn.screen().row(0).starts_with("line 16 "),
        "{:?}",
        drawn.screen().rows()
    );
    assert_eq!(rail_of(&drawn), "•│●┃│•│•││");
    assert_eq!(
        drawn
            .took(Pressed::Released { row: 3, column: 59 })
            .unwrap(),
        None
    );
}

#[test]
fn a_drag_on_the_rail_thumb_scrolls_the_transcript_with_it() {
    let mut drawn = railed();
    let starts = |drawn: &Drawn, line: &str| drawn.screen().row(0).starts_with(&format!("{line} "));

    // Taken by its last row, so that row follows the pointer.
    assert_eq!(rail_of(&drawn), "•│•││•│●┃┃");
    assert_eq!(rail_click(&mut drawn, 9), None);
    assert!(starts(&drawn, "line 70"), "the press on the thumb moved it");

    assert_eq!(
        drawn.took(Pressed::Dragged { row: 1, column: 0 }).unwrap(),
        None
    );
    assert!(starts(&drawn, "line 0"), "{:?}", drawn.screen().rows());

    assert_eq!(
        drawn.took(Pressed::Dragged { row: 3, column: 59 }).unwrap(),
        None
    );
    assert!(starts(&drawn, "line 16"), "{:?}", drawn.screen().rows());
    assert_eq!(rail_of(&drawn), "•│●┃│•│•││");

    // Past the foot of the band is the foot of the record.
    assert_eq!(
        drawn
            .took(Pressed::Dragged {
                row: 12,
                column: 59
            })
            .unwrap(),
        None
    );
    assert!(starts(&drawn, "line 70"), "{:?}", drawn.screen().rows());

    assert_eq!(
        drawn
            .took(Pressed::Released {
                row: 12,
                column: 59
            })
            .unwrap(),
        None
    );

    // Let go: a drag now is the selection's again, and moves nothing.
    drawn.took(Pressed::Dragged { row: 1, column: 59 }).unwrap();
    assert!(starts(&drawn, "line 70"));
}

#[test]
fn a_click_off_the_rail_lets_go_of_a_thumb_whose_release_was_lost() {
    let mut drawn = railed();
    let starts = |drawn: &Drawn, line: &str| drawn.screen().row(0).starts_with(&format!("{line} "));

    // Taken, and the button's release never arrives.
    assert_eq!(rail_click(&mut drawn, 9), None);
    assert!(starts(&drawn, "line 70"));

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 1, column: 59 }).unwrap();
    assert!(
        starts(&drawn, "line 70"),
        "the drag scrolled: {:?}",
        drawn.screen().rows()
    );
    drawn.take();

    drawn
        .took(Pressed::Released { row: 1, column: 59 })
        .unwrap();
    let copied = drawn.take();
    let wanted = onto_the_clipboard("line 70\nline 71");
    assert!(copied.contains(&wanted), "{copied:?}");
}

#[test]
fn a_click_on_a_rail_mark_lands_on_its_prompt() {
    let mut drawn = railed();

    assert_eq!(rail_click(&mut drawn, 5), None);
    assert!(
        drawn.screen().row(0).starts_with("line 40 "),
        "{:?}",
        drawn.screen().rows()
    );

    assert_eq!(rail_click(&mut drawn, 0), None);
    assert!(
        drawn.screen().row(0).starts_with("line 0 "),
        "{:?}",
        drawn.screen().rows()
    );
}

#[test]
fn a_selection_dragged_across_the_rail_neither_lights_nor_copies_it() {
    let mut drawn = railed();

    drawn.took(Pressed::Clicked { row: 0, column: 0 }).unwrap();
    drawn.took(Pressed::Dragged { row: 1, column: 59 }).unwrap();
    let lit = drawn.take();
    assert!(lit.contains("\x1b[7m"), "nothing was lit: {lit:?}");
    for cell in ["┃", "│", "•"] {
        let reversed = format!("\x1b[7m{cell}");
        assert!(!lit.contains(&reversed), "{lit:?}");
    }

    drawn
        .took(Pressed::Released { row: 1, column: 59 })
        .unwrap();
    let copied = drawn.take();
    let wanted = onto_the_clipboard("line 70\nline 71");
    assert!(copied.contains(&wanted), "{copied:?}");
}

#[test]
fn a_pointer_on_the_rail_lights_no_cut_result() {
    let mut drawn = Drawn::new(40, 4);
    drawn.rails(true);
    drawn.wears(colourful());
    for line in 0..8 {
        drawn.present(&[cut(&format!("cut {line}"))]).unwrap();
    }
    drawn.take();

    drawn.took(Pressed::Hovered { row: 0, column: 39 }).unwrap();
    let frame = drawn.take();
    for line in 0..8 {
        let said = format!("cut {line}");
        assert!(
            !frame.contains(&said) || frame.contains(&quietly(&said)),
            "the rail lit the row beside it: {frame:?}"
        );
    }

    drawn.took(Pressed::Hovered { row: 0, column: 0 }).unwrap();
    assert!(!drawn.take().is_empty(), "the row itself did not light");
}

/// A rail cell as the wire carries it, worn in `slot`.
fn worn(slot: Slot, cell: &str) -> String {
    format!("{}{cell}{}", colourful().open(slot), colourful().close())
}

/// A pointer moved to window row `row` of the rail's column.
fn rail_hover(drawn: &mut Drawn, row: usize) {
    let column = drawn.columns() - 1;
    assert_eq!(drawn.took(Pressed::Hovered { row, column }).unwrap(), None);
}

/// What was written to the window after the first `from` bytes of it.
///
/// Read rather than taken, so the picture is still the whole window's.
fn since(drawn: &Drawn, from: usize) -> String {
    drawn
        .terminal()
        .written()
        .get(from..)
        .unwrap_or("")
        .to_owned()
}

#[test]
fn a_pointer_on_the_rail_lights_its_track_and_marks_and_grows_the_mark_under_it() {
    let mut drawn = railed_in(colourful());
    let resting = since(&drawn, 0);
    assert!(resting.contains(&worn(Slot::Quiet, "│")), "{resting:?}");
    assert!(resting.contains(&worn(Slot::Quiet, "•")), "{resting:?}");
    let from = resting.len();

    rail_hover(&mut drawn, 2);

    assert_eq!(rail_of(&drawn), "•│●││•│●┃┃");
    let frame = since(&drawn, from);
    for cell in ["│", "•", "●"] {
        assert!(
            frame.contains(&worn(Slot::Accent, cell)),
            "{cell}: {frame:?}"
        );
    }
    assert!(!frame.contains(&worn(Slot::Quiet, "│")), "{frame:?}");
    assert!(!frame.contains(&worn(Slot::Quiet, "•")), "{frame:?}");
    assert!(!frame.contains(&worn(Slot::Quiet, "●")), "{frame:?}");
}

#[test]
fn a_pointer_moving_along_the_rail_grows_the_mark_it_arrives_at() {
    let mut drawn = railed();

    rail_hover(&mut drawn, 2);
    assert_eq!(rail_of(&drawn), "•│●││•│●┃┃");
    rail_hover(&mut drawn, 5);
    assert_eq!(rail_of(&drawn), "•│•││●│●┃┃");
    rail_hover(&mut drawn, 3);
    assert_eq!(rail_of(&drawn), "•│•││•│●┃┃");
    // The thumb grows nothing under the pointer: the thumb is what is there.
    rail_hover(&mut drawn, 9);
    assert_eq!(rail_of(&drawn), "•│•││•│●┃┃");
}

#[test]
fn a_pointer_leaving_the_rail_puts_it_back_at_rest_on_the_next_frame() {
    let mut drawn = railed_in(colourful());
    rail_hover(&mut drawn, 2);
    let from = since(&drawn, 0).len();

    assert_eq!(
        drawn.took(Pressed::Hovered { row: 2, column: 0 }).unwrap(),
        None
    );

    assert_eq!(rail_of(&drawn), "•│•││•│●┃┃");
    let frame = since(&drawn, from);
    assert!(frame.contains(&worn(Slot::Quiet, "│")), "{frame:?}");
    assert!(frame.contains(&worn(Slot::Quiet, "•")), "{frame:?}");
    assert!(!frame.contains(&worn(Slot::Accent, "│")), "{frame:?}");
}

#[test]
fn a_pointer_on_the_rail_asks_for_a_frame_only_when_the_row_under_it_changes() {
    // With a pointable prompt standing, the caller draws the frame, so what
    // the renderer says is whether one is owed at all.
    let mut drawn = railed();
    let prompt = vec![Row::plain("prompt"), Row::plain("2 commands")];
    let pointed = Row::new().then(Slot::Pointed, "2 commands");
    drawn
        .replace(
            PromptRows {
                rows: &prompt,
                caret: Caret::default(),
                pointed: Some((1, &pointed)),
            },
            &[],
            Palette::plain(),
        )
        .unwrap();

    rail_hover(&mut drawn, 3);
    assert!(drawn.pointed_changed(), "entering the rail asked for none");
    rail_hover(&mut drawn, 3);
    assert!(
        !drawn.pointed_changed(),
        "resting on one rail row asked again"
    );
    rail_hover(&mut drawn, 4);
    assert!(drawn.pointed_changed(), "moving a row asked for none");
    drawn.took(Pressed::Hovered { row: 4, column: 0 }).unwrap();
    assert!(drawn.pointed_changed(), "leaving the rail asked for none");
    drawn.took(Pressed::Hovered { row: 4, column: 1 }).unwrap();
    assert!(
        !drawn.pointed_changed(),
        "motion off the rail asked for one"
    );
}

#[test]
fn an_ascii_rail_grows_the_mark_under_the_pointer_to_a_star() {
    let mut drawn = railed();
    drawn.draws(Glyphs::Ascii);

    rail_hover(&mut drawn, 5);

    assert_eq!(rail_of(&drawn), "-|-||*|*##");
}

#[test]
fn the_current_prompt_s_rail_mark_is_grown_quiet_on_the_track_and_accent_on_the_thumb() {
    // At the foot the latest prompt, line 60, is above the band: its mark is
    // grown on the track, in the track's colour.
    let mut drawn = railed_in(colourful());
    assert_eq!(rail_of(&drawn), "•│•││•│●┃┃");
    let resting = since(&drawn, 0);
    assert!(resting.contains(&worn(Slot::Quiet, "●")), "{resting:?}");
    let from = resting.len();

    // Thirty-five rows up the band holds line 40, and the thumb with it: the
    // mark that was hidden there is grown on the thumb, in the thumb's colour.
    drawn.scrolled(-35).unwrap();
    assert_eq!(rail_of(&drawn), "•│•│┃●│•││");
    let frame = since(&drawn, from);
    assert!(frame.contains(&worn(Slot::Accent, "●")), "{frame:?}");
    assert!(!frame.contains(&worn(Slot::Quiet, "●")), "{frame:?}");
}

#[test]
fn an_ascii_rail_draws_the_current_prompt_s_mark_as_a_star_on_the_thumb_too() {
    let mut drawn = railed();
    drawn.draws(Glyphs::Ascii);

    drawn.scrolled(-35).unwrap();
    assert_eq!(rail_of(&drawn), "-|-|#*|-||");

    drawn.scrolled(35).unwrap();
    assert_eq!(rail_of(&drawn), "-|-||-|*##");
}

#[test]
fn the_prompt_a_rail_click_lands_on_stays_current_while_it_is_in_the_band() {
    // Prompts at lines 40 and 48 are on rail rows 5 and 6, and the band that
    // starts at line 40 holds both. The latest of them would be current; the
    // one the click landed on is.
    let mut drawn = Drawn::new(60, 10);
    drawn.rails(true);
    for line in 0..80 {
        if [0, 20, 40, 48, 60].contains(&line) {
            drawn.landmark();
        }
        drawn.commit(&format!("line {line}")).unwrap();
    }
    assert_eq!(rail_of(&drawn), "•│•││••●┃┃");

    assert_eq!(rail_click(&mut drawn, 5), None);
    assert!(
        drawn.screen().row(0).starts_with("line 40 "),
        "{:?}",
        drawn.screen().rows()
    );
    assert_eq!(rail_of(&drawn), "•│•││●┃•││");

    // A row on, line 40 has left the band, and the latest prompt at or above
    // the band's last row is current again.
    drawn.scrolled(1).unwrap();
    assert_eq!(rail_of(&drawn), "•│•││┃●•││");
}

#[test]
fn a_prompt_sent_after_a_rail_landing_is_the_current_prompt() {
    // Prompts at lines 0, 10 and 32 of forty. From the top, a press on line
    // 32's mark lands on it, and the band can go no lower than the foot,
    // which starts at line 30 and so still holds it.
    let mut drawn = Drawn::new(60, 10);
    drawn.rails(true);
    for line in 0..40 {
        if [0, 10, 32].contains(&line) {
            drawn.landmark();
        }
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.scrolled(-100).unwrap();
    assert_eq!(rail_click(&mut drawn, 8), None);
    assert!(
        drawn.screen().row(0).starts_with("line 30 "),
        "{:?}",
        drawn.screen().rows()
    );

    // Sent as a prompt is: back to the foot, then its mark and its rows.
    drawn.follows().unwrap();
    drawn.landmark();
    for line in 40..42 {
        drawn.commit(&format!("line {line}")).unwrap();
    }

    // Line 32 starts in the band still, on the thumb, and the new prompt at
    // line 40 is the one being read under.
    assert!(
        drawn.screen().row(0).starts_with("line 32 "),
        "{:?}",
        drawn.screen().rows()
    );
    assert_eq!(rail_of(&drawn), "•│•││││┃┃●");
}

#[test]
fn the_prompt_a_rail_click_lands_on_stays_current_across_a_resize_that_relays_the_opening() {
    // An opening one row tall at the railed sixty columns and four below
    // fifty-five, so pulling the window to fifty renumbers every line under
    // it. The prompts are at lines 0, 20, 40, 48 and 60 of what follows it.
    let mut drawn = Drawn::new(60, 10);
    drawn.rails(true);
    drawn
        .opens(Box::new(|columns, _| {
            let rows = if columns < 55 { 4 } else { 1 };
            (0..rows)
                .map(|row| Row::plain(format!("card {row}")))
                .collect()
        }))
        .unwrap();
    for line in 0..80 {
        if [0, 20, 40, 48, 60].contains(&line) {
            drawn.landmark();
        }
        drawn.commit(&format!("line {line}")).unwrap();
    }

    assert_eq!(rail_click(&mut drawn, 5), None);
    assert!(
        drawn.screen().row(0).starts_with("line 40 "),
        "{:?}",
        drawn.screen().rows()
    );
    assert_eq!(rail_of(&drawn), "•│•││●┃•││");

    drawn.render.terminal.resize(50, 10);
    drawn.resized().unwrap();

    assert!(
        drawn.screen().row(0).starts_with("line 40 "),
        "{:?}",
        drawn.screen().rows()
    );
    assert_eq!(rail_of(&drawn), "•│•││●┃•││");
}

#[test]
fn a_drag_on_the_rail_thumb_grows_only_the_mark_under_the_pointer() {
    let mut drawn = railed();
    drawn.scrolled(-35).unwrap();
    assert_eq!(rail_of(&drawn), "•│•│┃●│•││");

    rail_hover(&mut drawn, 5);
    assert_eq!(rail_click(&mut drawn, 5), None);
    assert_eq!(
        drawn.took(Pressed::Dragged { row: 0, column: 59 }).unwrap(),
        None
    );
    assert!(
        drawn.screen().row(0).starts_with("line 0 "),
        "{:?}",
        drawn.screen().rows()
    );
    // The press was on row 5, and its mark is no longer under the pointer.
    assert_eq!(rail_of(&drawn), "●┃•││•│•││");

    // Dragged off the rail's column, nothing on it is under the pointer: the
    // one grown mark left is line 0's, the latest prompt above the band.
    assert_eq!(
        drawn.took(Pressed::Dragged { row: 2, column: 0 }).unwrap(),
        None
    );
    assert_eq!(rail_of(&drawn), "●┃┃││•│•││");
}

#[test]
fn a_pointer_on_a_rail_over_a_record_that_fits_changes_nothing() {
    let mut drawn = Drawn::new(60, 10);
    drawn.rails(true);
    drawn.wears(colourful());
    drawn.landmark();
    drawn.commit("one line").unwrap();
    drawn.take();

    rail_hover(&mut drawn, 0);

    assert_eq!(drawn.take(), "");
    assert_eq!(rail_of(&drawn), " ".repeat(10));
}

#[test]
fn after_the_record_is_emptied_the_rail_thumb_still_reaches_the_foot() {
    // Eighty one-row lines, then emptied, as `/clear` and `/resume` do, and a
    // new session of thirty lines that each fold to two rows: sixty rows on a
    // rail of ten, six to a rail row, so the thumb at the foot is the last two
    // rail rows.
    let mut drawn = railed();
    drawn.empties().unwrap();
    drawn.landmark();
    for line in 0..30 {
        drawn
            .commit(&format!("again {line:02} {}", "word ".repeat(12)))
            .unwrap();
    }

    let foot = drawn.bands().transcript.end - 1;
    assert!(
        drawn.screen().row(foot - 1).starts_with("again 29 word"),
        "{:?}",
        drawn.screen().rows()
    );
    assert_eq!(rail_of(&drawn), "●│││││││┃┃");
}

/// The next number from a seeded generator, below `below`.
///
/// A linear congruential step from a fixed start, so a failing case is the
/// same case on every run and the case number its failure prints is enough to
/// find it again.
fn next(seed: &mut u64, below: usize) -> usize {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    usize::try_from(*seed >> 33).unwrap_or(0) % below.max(1)
}

#[test]
fn the_rail_thumb_and_marks_stand_on_the_rows_the_drawn_band_scales_to() {
    // 300 seeded cases across record shapes, widths, band heights and scroll
    // positions. What the rail is checked against is read off the screen: the
    // band's rows found in the whole record as drawn, so the oracle is the row
    // space a reader sees rather than the one the record keeps.
    let mut seed = 0x5eed_u64;
    for case in 0..300 {
        let columns = crate::scroll_rail::NARROWEST + next(&mut seed, 100);
        let mut drawn = Drawn::new(columns, 4 + next(&mut seed, 30));
        drawn.rails(true);
        let mut word = 0;
        let mut session = |drawn: &mut Drawn, seed: &mut u64, lines: usize| {
            for line in 0..lines {
                if next(seed, 6) == 0 {
                    drawn.landmark();
                    drawn.commit(&format!("› p{case}x{line}")).unwrap();
                    continue;
                }
                // Every word distinct, so every row of words is too, and now
                // and then a long unbroken run to be cut mid-word. The run is
                // numbered pieces, `q<run>n<piece>`, each far shorter than the
                // narrowest fold, so every full row of it holds a piece no
                // other row does and the band is found in one place.
                let mut text = Vec::new();
                for _ in 0..next(seed, 40) {
                    word += 1;
                    let pad = "abcdefghijklmnop".get(..next(seed, 16)).unwrap_or("");
                    text.push(format!("w{word}{pad}"));
                }
                if next(seed, 8) == 0 {
                    word += 1;
                    let run = (0..200)
                        .map(|piece| format!("q{word}n{piece}"))
                        .collect::<Vec<_>>()
                        .concat();
                    let run = run.get(..next(seed, 200)).unwrap_or("");
                    text.push(format!("r{word}{run}"));
                }
                let text = text.join(" ");
                drawn.commit(&format!("l{case}x{line} {text}")).unwrap();
            }
        };
        let lines = 1 + next(&mut seed, 80);
        session(&mut drawn, &mut seed, lines);
        if next(&mut seed, 4) == 0 {
            drawn.empties().unwrap();
            let lines = 1 + next(&mut seed, 80);
            session(&mut drawn, &mut seed, lines);
        }

        // The whole record as drawn, read while the band follows the foot.
        let all: Vec<String> = drawn
            .tail(20_000)
            .iter()
            .map(|row| row.text().trim_end().to_owned())
            .collect();
        let up = next(&mut seed, all.len() + 1);
        drawn.scrolled(-i32::try_from(up).unwrap()).unwrap();

        let bands = drawn.bands();
        let height = bands.transcript.len();
        let screen = drawn.screen();
        let band: Vec<String> = bands
            .transcript
            .clone()
            .map(|row| {
                let text: String = screen.row(row).chars().take(columns - 1).collect();
                text.trim_end().to_owned()
            })
            .take(all.len())
            .collect();
        let found: Vec<usize> = (0..=all.len() - band.len())
            .filter(|top| all.get(*top..*top + band.len()) == Some(&band[..]))
            .collect();
        let [top] = found[..] else {
            panic!("case {case}: band found at {found:?}\n{band:#?}");
        };

        let total = all.len();
        let rail = drawn.rail(&bands).unwrap();
        if total <= height {
            assert_eq!(rail.thumb(), None, "case {case}");
            continue;
        }
        let scaled = |row: usize| row * height / total;
        let first = scaled(top);
        let last = scaled(top + height - 1);
        assert_eq!(
            rail.thumb(),
            Some(first..last + 1),
            "case {case}: {columns} columns, band {height} rows at {top} of {total}"
        );
        let prompts: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, row)| row.starts_with("› p"))
            .map(|(at, _)| scaled(at))
            .collect();
        for at in 0..height {
            assert_eq!(
                rail.marked(at),
                prompts.contains(&at),
                "case {case}: rail row {at}, prompts on {prompts:?}"
            );
        }
    }
}

#[test]
fn a_rail_over_a_transcript_that_fits_is_blank() {
    let mut drawn = Drawn::new(40, 8);
    drawn.rails(true);
    drawn.commit("a line").unwrap();

    assert_eq!(rail_of(&drawn), " ".repeat(8));
}

#[test]
fn a_blank_rail_names_no_cut_result_and_with_the_rail_off_the_column_does() {
    // A result as wide as the window, so the last column is one it drew once
    // the rail is off and the rail's once it is on.
    for rails in [true, false] {
        let mut drawn = Drawn::new(40, 8);
        drawn.rails(rails);
        drawn.wears(colourful());
        drawn.present(&[cut(&"x".repeat(40))]).unwrap();
        drawn.take();

        drawn.took(Pressed::Hovered { row: 0, column: 39 }).unwrap();
        assert_eq!(drawn.take().is_empty(), rails, "hover, rail {rails}");

        let click = Pressed::Clicked { row: 0, column: 39 };
        let passed = drawn.took(click.clone()).unwrap();
        assert_eq!(passed, (!rails).then_some(click), "click, rail {rails}");
    }
}

#[test]
fn the_rail_is_not_drawn_at_the_narrowest_width_that_cannot_spare_it() {
    let narrowest = crate::scroll_rail::NARROWEST;
    for (columns, drawn_at) in [(narrowest - 1, false), (narrowest, true)] {
        let mut drawn = Drawn::new(columns, 6);
        drawn.rails(true);
        for line in 0..20 {
            drawn.commit(&format!("{line}")).unwrap();
        }

        assert_eq!(drawn.transcript_columns(), columns - usize::from(drawn_at));
        assert_eq!(rail_of(&drawn).contains('┃'), drawn_at, "{columns}");
        let click = Pressed::Clicked {
            row: 0,
            column: columns - 1,
        };
        assert_eq!(
            drawn.took(click.clone()).unwrap().is_some(),
            !drawn_at,
            "{columns}"
        );
    }
}

#[test]
fn a_redirected_run_has_no_rail_to_fold_for() {
    let mut drawn = Drawn {
        render: Renderer::new(Recording::redirected(40, 8)),
    };
    drawn.rails(true);
    drawn.commit(&"x".repeat(40)).unwrap();

    assert_eq!(drawn.transcript_columns(), 40);
    assert_eq!(
        drawn.render.terminal.written(),
        format!("{}\n", "x".repeat(40))
    );
}

// A line amended after it was written.

/// What the tests below amend a line with: every span quiet, the words kept.
fn quieted(rows: &mut [Row]) {
    for row in rows {
        *row = Row::new().then(Slot::Quiet, row.text());
    }
}

#[test]
fn a_line_amended_after_it_was_written_is_drawn_amended() {
    // What stops a row offering once what it offered has gone: the row stays
    // where it was written, with the words it was drawn with, and no longer
    // wears the slot that lights it under the pointer.
    let mut drawn = Drawn::new(40, 8);
    drawn.commit("before it").unwrap();
    let at = drawn.render.lines();
    drawn
        .render
        .present(&[Row::new()
            .then(Slot::Cut, "what was cut")
            .then(Slot::Quiet, " and the rest")])
        .unwrap();
    assert!(drawn.render.record.wears(at, Slot::Cut));

    drawn.render.amend(at, quieted).unwrap();

    assert!(!drawn.render.record.wears(at, Slot::Cut));
    assert_eq!(drawn.screen().row(1), "what was cut and the rest");
}

#[test]
fn a_block_amended_after_it_was_written_stays_amended_at_every_width() {
    // A block laid out again at each width is laid out again amended: the edit
    // is kept with what lays it out, not only with the rows it had then.
    let mut drawn = Drawn::new(40, 8);
    let at = drawn.render.lines();
    drawn
        .responsive(
            0,
            Box::new(|_| vec![Row::new().then(Slot::Cut, "a change that was offered")]),
        )
        .unwrap();
    assert!(drawn.render.record.wears(at, Slot::Cut));

    drawn.render.amend(at, quieted).unwrap();
    assert!(!drawn.render.record.wears(at, Slot::Cut));

    drawn.render.terminal.resize(30, 8);
    drawn.resized().unwrap();

    assert!(!drawn.render.record.wears(at, Slot::Cut));
    assert_eq!(drawn.screen().row(0), "a change that was offered");
}

#[test]
fn a_line_no_longer_held_is_not_amended_and_nothing_else_is() {
    let mut drawn = Drawn::new(40, 8);
    drawn
        .render
        .present(&[Row::new().then(Slot::Cut, "offered")])
        .unwrap();
    let past = drawn.render.lines();

    drawn.render.amend(past, quieted).unwrap();

    assert!(drawn.render.record.wears(past - 1, Slot::Cut));
}

#[test]
fn a_glyph_set_taken_mid_answer_leaves_the_answer_read_where_it_was() {
    // `/settings` stands over a running turn, so a new set can arrive between
    // two deltas of one answer. The marker already opened is still closed,
    // and what is drawn after it is drawn in the new set.
    let mut drawn = Drawn::new(80, 24);
    drawn.wears(colourful());
    drawn.stream("a **lo").unwrap();
    drawn.draws(Glyphs::Ascii);
    drawn.stream("ud** word\n- next\n").unwrap();
    drawn.settle().unwrap();

    let said = drawn.screen().said();
    assert_eq!(
        said.first().map(String::as_str),
        Some("a loud word"),
        "{said:#?}"
    );
    assert!(
        said.iter()
            .any(|row| row.starts_with(Glyphs::Ascii.bullet()) && row.ends_with("next")),
        "{said:#?}"
    );
}

// The rail beside a running turn.

/// A renderer with the rail on, on a window sixty columns wide and thirty rows
/// tall, holding eighty numbered lines with the box under them.
fn railed_turn() -> Drawn {
    let mut drawn = Drawn::new(60, 30);
    drawn.rails(true);
    for line in 0..80 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn
}

/// Stands `turn` under the transcript as a running turn's own rows, with
/// `over` under them and the box under both.
fn running(drawn: &mut Drawn, turn: &[Row], over: &[Row]) {
    let (rows, caret) = boxed();
    drawn
        .replace_running(
            PromptRows {
                rows: &rows,
                caret,
                pointed: None,
            },
            turn,
            over,
            Palette::plain(),
        )
        .unwrap();
}

/// `count` rows of a running command's output.
fn printed(count: usize) -> Vec<Row> {
    (0..count).map(|n| Row::plain(format!("out {n}"))).collect()
}

/// The rail's column, read down every row from the top of the transcript band
/// to the foot of the turn band.
fn rail_down(drawn: &Drawn) -> String {
    let screen = drawn.screen();
    let column = drawn.columns() - 1;
    let bands = drawn.bands();
    (bands.transcript.start..bands.turn.end)
        .map(|row| screen.row(row).chars().nth(column).unwrap_or(' '))
        .collect()
}

/// How many rows of `rail` the thumb covers.
fn thumb_of(rail: &str) -> usize {
    rail.chars().filter(|cell| *cell == '┃').count()
}

#[test]
fn a_running_turn_that_grows_barely_moves_the_rail_thumb() {
    // A running command's output coming and going under the transcript took
    // the band from twenty-six rows to eighteen, and the rail and thumb with
    // it: a thumb of eight rows became one of four, every time the output
    // appeared. The turn's rows are the transcript's tail while it runs, so the
    // rail stands beside them too and the thumb keeps its length.
    let mut drawn = railed_turn();
    running(&mut drawn, &standing(), &[]);
    let short = rail_down(&drawn);
    running(&mut drawn, &printed(9), &[]);
    let tall = rail_down(&drawn);

    assert_eq!(drawn.bands().transcript.len(), 18, "{tall:?}");
    assert_eq!(short.chars().count(), tall.chars().count());
    assert!(
        thumb_of(&short).abs_diff(thumb_of(&tall)) <= 1,
        "one row of turn: {short:?}, nine: {tall:?}"
    );
    // Following the foot, the thumb reaches the rail's last row either way.
    assert!(
        short.ends_with('┃') && tall.ends_with('┃'),
        "{short:?} {tall:?}"
    );
}

#[test]
fn a_running_turn_s_rows_leave_the_rail_its_column() {
    let mut drawn = railed_turn();
    running(&mut drawn, &[Row::plain("x".repeat(80))], &[]);

    let screen = drawn.screen();
    let at = drawn.bands().turn.start;
    assert_eq!(screen.row(at), format!("{}┃", "x".repeat(59)));
}

#[test]
fn a_list_under_a_running_turn_stands_beside_no_rail_and_is_not_counted() {
    // The list a line opened is not the transcript's: it keeps the window's
    // width and the rail stops at the turn's last row. Twenty-four band rows
    // and the turn's one are twenty-five rail rows over eighty-one, so the
    // thumb is eight rows; counting the list too would have made it nine.
    let mut drawn = railed_turn();
    let list = vec![Row::plain("y".repeat(60)), Row::plain("/help")];
    running(&mut drawn, &standing(), &list);

    let bands = drawn.bands();
    let screen = drawn.screen();
    assert_eq!(bands.transcript.len(), 24);
    assert_eq!(screen.row(bands.turn.end - 2), "y".repeat(60));
    assert_eq!(screen.row(bands.turn.end - 1), "/help");
    assert_eq!(
        rail_down(&drawn),
        format!("{}{}y ", "│".repeat(17), "┃".repeat(8))
    );
}

#[test]
fn a_turn_s_rows_standing_in_the_box_s_place_stand_beside_the_rail() {
    // What the turn was showing over the box goes on standing over what takes
    // the box's place, and is still the transcript's: the rail stands beside
    // it and counts it, as it did over the box, and stops before the rows
    // under it. Twenty-seven band rows and the blank are twenty-eight rail rows
    // over eighty-one, so the thumb is ten rows; leaving the blank out would
    // have made it nine.
    let mut drawn = railed_turn();
    let rows = [Row::plain("writing"), Row::plain("view")];
    drawn
        .instead(&[Row::new()], &rows, None, Palette::plain())
        .unwrap();

    let bands = drawn.bands();
    let screen = drawn.screen();
    assert_eq!(bands.transcript.len(), 27);
    assert_eq!(screen.row(bands.turn.end - 2), "writing");
    assert_eq!(screen.row(bands.turn.end - 1), "view");
    let rail = rail_down(&drawn);
    assert!(rail.ends_with("\u{2503}  "), "{rail:?}");
    assert_eq!(thumb_of(&rail), 10, "{rail:?}");
}

#[test]
fn rows_stood_under_the_transcript_by_anything_but_a_turn_stand_beside_no_rail() {
    // A question or a picker stands in the same band, and is not the
    // transcript's either: the rail is the band's alone, as between turns.
    let mut drawn = railed_turn();
    drawn.under(&printed(9), None, Palette::plain()).unwrap();

    let bands = drawn.bands();
    let screen = drawn.screen();
    assert_eq!(screen.row(bands.turn.start), "out 0");
    assert_eq!(rail_of(&drawn).chars().count(), bands.transcript.len());
}

#[test]
fn a_press_on_the_rail_beside_a_running_turn_steers_the_transcript() {
    let mut drawn = railed_turn();
    running(&mut drawn, &printed(9), &[]);
    drawn.scrolled(-1000).unwrap();
    assert!(drawn.screen().row(0).starts_with("line 0 "));

    // The rail's last row stands beside the turn's last row: a press there
    // takes the band back to the foot, and is the rail's, not the turn's.
    let foot = drawn.bands().turn.end - 1;
    assert_eq!(rail_click(&mut drawn, foot), None);
    let bands = drawn.bands();
    assert!(
        drawn
            .screen()
            .row(bands.transcript.end - 1)
            .starts_with("line 79 "),
        "{:?}",
        drawn.screen().rows()
    );
    assert!(rail_down(&drawn).ends_with('┃'));
}

// A wait on the keyboard something outside can call off.

/// A recall that is called off once `noted` is set, as a signal would set it.
#[derive(Debug, Default)]
struct Noting {
    /// Whether the word to call the wait off has come.
    noted: AtomicBool,
    /// Whether the word comes as the wait says it is over, as a signal landing
    /// after the wait last looked would.
    as_it_ends: bool,
}

impl Noting {
    fn note(&self) {
        self.noted.store(true, Ordering::SeqCst);
    }
}

impl Recall for Noting {
    fn waiting(&self) -> bool {
        true
    }

    fn recalled(&self) -> bool {
        self.noted.load(Ordering::SeqCst)
    }

    fn waited(&self) {
        if self.as_it_ends {
            self.note();
        }
    }
}

/// A renderer whose waits `recall` watches.
fn watched_by(recall: &Arc<Noting>) -> Renderer<Recording> {
    let mut render = Renderer::new(Recording::new(80, 24));
    render.recalled_by(Arc::clone(recall) as Arc<dyn Recall>);
    render
}

#[test]
fn a_recall_during_the_last_beat_of_a_wait_calls_it_off() {
    // The beat that runs the patience out is a beat like any other: a word
    // that came while it slept is not left for nobody to read.
    let recall = Arc::new(Noting::default());
    let mut render = watched_by(&recall);

    let waited = render.waiting_from(Duration::from_millis(10), |_| {
        recall.note();
        Ok(false)
    });

    assert!(matches!(waited, Err(TerminalError::Recalled)), "{waited:?}");
}

#[test]
fn a_recall_while_the_key_is_read_calls_the_wait_off() {
    let recall = Arc::new(Noting::default());
    let mut render = watched_by(&recall);

    let pressed = render.pressed_from(
        |_| Ok(true),
        || {
            recall.note();
            Ok(Pressed::Ignored)
        },
    );

    assert!(
        matches!(pressed, Err(TerminalError::Recalled)),
        "{pressed:?}"
    );
}

#[test]
fn a_recall_as_a_wait_ends_calls_it_off() {
    // After the wait has last looked and before it has said it is over, the
    // recall's word is noted rather than acted on where it lands, so the wait
    // looks once more after saying so.
    let recall = Arc::new(Noting {
        as_it_ends: true,
        ..Noting::default()
    });
    let mut render = watched_by(&recall);

    let waited = render.waiting_from(Duration::from_millis(10), |_| Ok(true));
    assert!(matches!(waited, Err(TerminalError::Recalled)), "{waited:?}");

    recall.noted.store(false, Ordering::SeqCst);
    let pressed = render.pressed_from(|_| Ok(true), || Ok(Pressed::Ignored));
    assert!(
        matches!(pressed, Err(TerminalError::Recalled)),
        "{pressed:?}"
    );
}
