use crucible_tui::Recording;

use super::*;

/// A view over everything, opened `from` rows down with `end` rows below it.
///
/// Everything rather than one result because that is what the keys are read
/// against: which of the two is standing changes what is in the window and
/// changes nothing about walking it.
fn standing(from: usize, end: usize) -> View {
    View {
        from,
        end,
        laid: end,
        was: 0,
        page: 0,
        starts: Vec::new(),
        over: Over::Everything(0),
        back: Vec::new(),
        refused: None,
    }
}

/// Results cut down to a row, under the lines of the calls they answer.
///
/// The record row each offer went onto is its place in `called`, which is not
/// what the loop passes but has what these need of it: one per result, and no
/// two the same.
fn cut(called: &[&str]) -> Kept {
    let mut kept = Kept::default();

    for (at, one) in called.iter().enumerate() {
        let call = crucible_types::ToolId::new(format!("call-{at}"));
        kept.calling(call.clone(), (*one).to_owned());
        kept.finished(
            &call,
            format!("{one} answered this\nand then this\n").into(),
            at,
        );
    }

    kept
}

/// The window a view is open over, or a failed test if it is closed.
fn opened(standing: &mut Standing) -> &mut View {
    match standing {
        Standing::Open(view) => view,
        Standing::Closed => panic!("nothing is standing"),
    }
}

/// One result long enough to fill any window it is stood in.
fn overflowing() -> Kept {
    let mut kept = Kept::default();

    let call = crucible_types::ToolId::new("build");
    kept.calling(call.clone(), "Bash(cargo build)".to_owned());
    kept.finished(&call, "a line of it\n".repeat(200).into(), 0);

    kept
}

#[test]
fn the_view_under_a_turn_leaves_the_tail_a_row_to_go_on_writing_into() {
    // The view stands over the transcript, and the transcript keeps a row
    // whatever is asked of it — so a view as tall as the window would leave the
    // turn nowhere to go on writing. The row it gives up is that one.
    //
    // Asserted against the same picture drawn by hand at that height rather
    // than against a count of rows, so the caret is pinned with it: it parks on
    // the view's last row, where the box parks it too.
    let held = overflowing();

    let mut standing = Standing::default();
    standing.open(&held);

    let mut stood = Renderer::new(Recording::new(80, 24));
    assert!(under(&mut stood, Style::plain(), &held, &mut standing).expect("drawn"));

    let mut same = Standing::default();
    same.open(&held);
    let rows = laying(&held, opened(&mut same), Glyphs::Unicode, 80, 23);
    assert_eq!(rows.len(), 23);

    let mut by_hand = Renderer::new(Recording::new(80, 24));
    let caret = Caret {
        row: rows.len().saturating_sub(1),
        column: 0,
    };
    by_hand
        .instead(&rows, Some(caret), Style::plain().palette())
        .expect("drawn");

    assert_eq!(stood.terminal().written(), by_hand.terminal().written());
}

#[test]
fn a_window_with_no_room_for_the_view_closes_it_and_gives_the_box_back() {
    // The chrome alone is four rows, so a window this short would stand a
    // header and a footer with none of the text they are about. The caller
    // draws the box when this answers no, which is the same answer the region
    // gives between turns.
    let held = overflowing();

    let mut standing = Standing::default();
    standing.open(&held);

    let mut render = Renderer::new(Recording::new(80, 4));
    assert!(!under(&mut render, Style::plain(), &held, &mut standing).expect("drawn"));

    assert_eq!(standing, Standing::Closed);
    assert_eq!(render.terminal().written(), "");
}

#[test]
fn nothing_opens_where_nothing_was_cut() {
    // The key is offered by the rows that were cut, so a session with none of
    // them has made no offer — and a view put up in answer to a press nobody
    // meant is one that took the prompt away for no reason.
    let mut standing = Standing::default();
    standing.open(&Kept::default());

    assert_eq!(standing, Standing::Closed);
    assert!(!standing.is_open());
}

#[test]
fn a_click_stands_the_one_result_the_row_it_landed_on_offered() {
    // What separates the two ways in. Ctrl+O names no result so it stands them
    // all; a click names one by landing on the row that made the offer, and
    // standing the rest of them would be answering a question about one call
    // with three calls' output.
    let held = cut(&["Bash(cargo build)", "Read(src/main.rs)", "Bash(cargo test)"]);

    let mut standing = Standing::default();
    standing.one(&held, 1);

    let rows = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 40);
    let said = rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");

    assert!(said.contains("Read(src/main.rs)"), "{said}");
    assert!(!said.contains("Bash(cargo build)"), "{said}");
    assert!(!said.contains("Bash(cargo test)"), "{said}");
}

#[test]
fn a_click_on_a_row_that_offered_nothing_opens_nothing() {
    // Most rows of the record offered nothing — a line of an answer, a blank
    // row, the shell's own output before crucible started. The answer to a
    // click on one of those is the screen the reader was already looking at.
    let held = cut(&["Bash(cargo build)"]);

    let mut standing = Standing::default();
    standing.one(&held, 7);

    assert_eq!(standing, Standing::Closed);
}

#[test]
fn a_view_over_a_result_the_ceiling_dropped_lays_out_no_rows() {
    // Which both callers read as the view closing. The row is still on screen
    // saying what it could not fit, and the text behind it has gone; a frame of
    // chrome with nothing under it would say the result was empty.
    let mut held = cut(&["Bash(cargo build)"]);

    let mut standing = Standing::default();
    standing.one(&held, 0);

    let call = crucible_types::ToolId::new("cat");
    held.calling(call.clone(), "Bash(cat big)".to_owned());
    held.finished(&call, "x".repeat(1024 * 1024).into_boxed_str(), 1);

    let rows = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 40);
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn a_view_stands_still_while_the_turn_under_it_goes_on_cutting() {
    // What it stands over is what had been cut when it opened. Letting in the
    // results arriving underneath would slide the rows being read down the
    // screen as each one landed, which is unreadable exactly when there is most
    // to read.
    let mut held = cut(&["Bash(cargo build)"]);

    let mut standing = Standing::default();
    standing.open(&held);
    let before = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);

    let call = crucible_types::ToolId::new("test");
    held.calling(call.clone(), "Bash(cargo test)".to_owned());
    held.finished(&call, "something else entirely\n".into(), 1);

    let after = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);
    assert_eq!(before, after);
}

#[test]
fn opening_it_again_is_what_brings_the_newer_results_in() {
    // Which is why standing still costs the reader nothing: what a turn cut
    // while they were reading is one press away, and the press is the one they
    // already know.
    let mut held = cut(&["Bash(cargo build)"]);

    let mut standing = Standing::default();
    standing.open(&held);
    let before = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);

    let call = crucible_types::ToolId::new("test");
    held.calling(call.clone(), "Bash(cargo test)".to_owned());
    held.finished(&call, "something else entirely\n".into(), 1);

    standing.open(&held);
    let after = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);
    assert_ne!(before, after);
}

#[test]
fn the_key_that_opened_it_closes_it_under_a_running_turn_too() {
    // The half of the toggle that has no loop of its own: under a turn the view
    // is handed one key at a time by the loop reading for the box, and the way
    // out has to be the same key it is between turns.
    let mut standing = Standing::default();
    standing.open(&cut(&["Bash(cargo build)"]));

    assert!(standing.against(Pressed::Expand, 6));
    assert_eq!(standing, Standing::Closed);
}

#[test]
fn esc_under_a_turn_closes_the_view_rather_than_stopping_the_turn() {
    // Esc belongs to whatever is standing, and while the view is standing that
    // is the view. The turn is still running behind it and the row that offers
    // to interrupt comes back with the box.
    let mut standing = Standing::default();
    standing.open(&cut(&["Bash(cargo build)"]));

    assert!(standing.against(Pressed::Escape, 6));
    assert_eq!(standing, Standing::Closed);
}

#[test]
fn a_key_that_moves_nothing_under_a_turn_owes_no_frame() {
    // The loop this is called from redraws whenever anything says it moved, and
    // a frame here lays the whole of what was cut out again. A key against the
    // top of the view must not be one.
    let mut standing = Standing::Open(standing(0, 20));

    assert!(!standing.against(Pressed::Up, 6));
    assert!(standing.against(Pressed::Down, 6));
    assert!(standing.is_open());
}

#[test]
fn the_arrows_walk_the_window_one_row_at_a_time() {
    let mut open = standing(4, 20);

    assert_eq!(moving(Pressed::Down, &mut open), Moved::Redraw);
    assert_eq!(open, standing(5, 20));

    assert_eq!(moving(Pressed::Up, &mut open), Moved::Redraw);
    assert_eq!(open, standing(4, 20));
}

#[test]
fn an_arrow_against_an_end_costs_no_frame() {
    // A key held down against the top or the bottom is the whole of what this
    // saves: the picture has not changed, so nothing is drawn for it.
    let mut top = standing(0, 20);
    assert_eq!(moving(Pressed::Up, &mut top), Moved::Still);
    assert_eq!(top, standing(0, 20));

    let mut bottom = standing(20, 20);
    assert_eq!(moving(Pressed::Down, &mut bottom), Moved::Still);
    assert_eq!(bottom, standing(20, 20));
}

#[test]
fn the_wheel_walks_this_view_rather_than_the_transcript_under_it() {
    // The one component the wheel does not fall through: it is a window over
    // more text than its rows hold, which is the thing somebody turning a wheel
    // is pointing at.
    let mut open = standing(4, 20);

    assert_eq!(
        moving(Pressed::Scrolled { back: false }, &mut open),
        Moved::Redraw
    );
    assert_eq!(open, standing(5, 20));

    assert_eq!(
        moving(Pressed::Scrolled { back: true }, &mut open),
        Moved::Redraw
    );
    assert_eq!(open, standing(4, 20));
}

#[test]
fn a_wheel_against_an_end_leaves_it_for_the_transcript() {
    // `Still` is not "nothing happened" here — it is what the loop reading this
    // takes as the transcript's turn, so a reader who walked to the top of a
    // view goes on reading back past it.
    let mut top = standing(0, 20);
    assert_eq!(
        moving(Pressed::Scrolled { back: true }, &mut top),
        Moved::Still
    );

    let mut bottom = standing(20, 20);
    assert_eq!(
        moving(Pressed::Scrolled { back: false }, &mut bottom),
        Moved::Still
    );
}

#[test]
fn a_view_with_nothing_below_it_does_not_scroll() {
    // Everything fitted, so the last row is on screen and the footer does not
    // name the arrows. One that moved anyway would be a window walking off the
    // bottom of what it holds.
    let mut whole = standing(0, 0);

    assert_eq!(moving(Pressed::Down, &mut whole), Moved::Still);
    assert_eq!(whole, standing(0, 0));
}

#[test]
fn the_key_that_opened_it_closes_it() {
    // The rows offering it say `ctrl+o to expand` and nothing else, so the same
    // key against what it opened is the whole of the way back.
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Expand, &mut open), Moved::Left);
}

#[test]
fn esc_closes_it_the_way_esc_closes_everything_else() {
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Escape, &mut open), Moved::Left);
}

#[test]
fn the_keys_the_line_underneath_owns_take_the_view_with_them() {
    // Ctrl-C and Ctrl-D belong to the line, but the view is what is standing:
    // a press closes it and is consumed there, so the line sees only the next one.
    for key in [Key::Interrupt, Key::Eof] {
        let mut open = standing(3, 20);
        assert_eq!(moving(Pressed::Key(key), &mut open), Moved::Left, "{key:?}");
    }
}

#[test]
fn return_scrolls_nothing_and_sends_nothing() {
    // The line under this is not being read. Closing on Return would send
    // whatever is in the box to the model the moment somebody meant to scroll.
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Key(Key::Enter), &mut open), Moved::Still);
    assert_eq!(open, standing(3, 20));
}

#[test]
fn a_resize_owes_the_next_frame() {
    // How many rows the results came to is a fact about the width, so the whole
    // picture is laid out again and `end` is answered again with it.
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Resized, &mut open), Moved::Redraw);
    assert_eq!(open, standing(3, 20));
}

/// The rows of the view over `kept` at 80 columns and `rows` rows, as text.
fn frame(kept: &Kept, standing: &mut Standing, rows: usize) -> Vec<String> {
    laying(kept, opened(standing), Glyphs::Unicode, 80, rows)
        .iter()
        .map(Row::text)
        .collect()
}

#[test]
fn page_down_moves_the_view_by_its_rows_less_one() {
    // Twenty-four rows of window show twenty of results, so a page keeps the
    // last of them in sight at the top of the next: the reader never has to
    // find their place again after the jump.
    let held = overflowing();
    let mut standing = Standing::default();
    standing.open(&held);
    frame(&held, &mut standing, 24);

    assert!(standing.against(Pressed::PageDown, 3));
    assert_eq!(opened(&mut standing).from, 19);
    frame(&held, &mut standing, 24);
    assert!(standing.against(Pressed::PageDown, 3));
    assert_eq!(opened(&mut standing).from, 38);

    // And back the same way, as far as the top and no further.
    assert!(standing.against(Pressed::PageUp, 3));
    assert_eq!(opened(&mut standing).from, 19);
    opened(&mut standing).from = 7;
    assert!(standing.against(Pressed::PageUp, 3));
    assert_eq!(opened(&mut standing).from, 0);
    assert!(!standing.against(Pressed::PageUp, 3));

    // At the end it moves nothing either, and owes no frame.
    let end = opened(&mut standing).end;
    opened(&mut standing).from = end;
    assert!(!standing.against(Pressed::PageDown, 3));
    assert_eq!(opened(&mut standing).from, end);
}

/// Three results of two lines each, newest first: `Bash(three)` from row 0,
/// `Bash(two)` from row 4 and `Bash(one)` from row 9, counting the blank above
/// each but the first, so their calls' lines are rows 0, 5 and 10. Fourteen
/// rows against the six a ten-row window shows.
fn three() -> Kept {
    cut(&["Bash(one)", "Bash(two)", "Bash(three)"])
}

/// The first call's line in the window, under the rule and its blank and any
/// blank that parts the result there from the one above.
fn heading(rows: &[String]) -> Option<&str> {
    rows.get(2..)
        .unwrap_or_default()
        .iter()
        .map(|row| row.trim_end())
        .find(|row| !row.is_empty())
}

#[test]
fn right_puts_the_next_older_result_at_the_top() {
    let kept = three();
    let mut standing = Standing::default();
    standing.open(&kept);
    assert_eq!(
        heading(&frame(&kept, &mut standing, 10)),
        Some("Bash(three)")
    );

    // Its call's line on the first row of the window, where the newest
    // result's stands when the view opens, rather than the blank above it.
    assert!(standing.against(Pressed::Key(Key::Right), 3));
    assert_eq!(opened(&mut standing).from, 5);
    let rows = frame(&kept, &mut standing, 10);
    assert_eq!(rows.get(2).map(|row| row.trim_end()), Some("Bash(two)"));

    // From part way into a result too: the step is to the next result's top,
    // not by a result's worth of rows.
    opened(&mut standing).from = 6;
    frame(&kept, &mut standing, 10);
    assert!(standing.against(Pressed::Key(Key::Right), 3));
    // The oldest result's top is past the furthest the window may go, so the
    // window goes as far as it may.
    assert_eq!(opened(&mut standing).from, 8);

    // And at the oldest it moves nothing, and owes no frame.
    frame(&kept, &mut standing, 10);
    assert!(!standing.against(Pressed::Key(Key::Right), 3));
    assert_eq!(opened(&mut standing).from, 8);
}

#[test]
fn left_at_the_newest_result_moves_nothing() {
    let kept = three();
    let mut standing = Standing::default();
    standing.open(&kept);
    frame(&kept, &mut standing, 10);

    assert!(standing.against(Pressed::Key(Key::Right), 3));
    frame(&kept, &mut standing, 10);
    assert!(standing.against(Pressed::Key(Key::Left), 3));
    assert_eq!(opened(&mut standing).from, 0);
    assert_eq!(
        heading(&frame(&kept, &mut standing, 10)),
        Some("Bash(three)")
    );

    // The newest is at the top: nothing is newer, so nothing moves and no
    // frame is owed. Part way into it as well.
    assert!(!standing.against(Pressed::Key(Key::Left), 3));
    opened(&mut standing).from = 2;
    frame(&kept, &mut standing, 10);
    assert!(!standing.against(Pressed::Key(Key::Left), 3));
    assert_eq!(opened(&mut standing).from, 2);
}

#[test]
fn a_step_stops_at_a_result_still_being_read_back() {
    // Forty results, most of them let go of by the store. Each step puts the
    // next older one at the top of the window, read back from the log rather
    // than standing as the line that says it will be: a step never passes a
    // result nobody has been shown.
    let kept = forty(false);
    assert!(kept.older().count() > 0, "nothing was let go of");
    let mut standing = Standing::default();
    standing.open(&kept);
    frame(&kept, &mut standing, 40);

    for step in 1..40 {
        assert!(
            standing.against(Pressed::Key(Key::Right), 3),
            "step {step} moved nothing"
        );
        let rows = frame(&kept, &mut standing, 40);
        let at = 39 - step;
        assert_eq!(
            heading(&rows),
            Some(format!("Bash({at})").as_str()),
            "step {step}: {rows:?}"
        );
        let first = format!("call-{at:03} line 0001");
        assert!(
            rows.iter().any(|row| row.contains(&first)),
            "step {step}: {rows:?}"
        );
        assert!(read_back(opened(&mut standing)) <= BEYOND);
    }
}

#[test]
fn the_footer_counts_the_result_at_the_top() {
    let kept = three();
    let mut standing = Standing::default();
    standing.open(&kept);
    let rows = frame(&kept, &mut standing, 10);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("esc to close · ↑↓ pgup pgdn to see more · ←→ result 1 of 3")
    );

    assert!(standing.against(Pressed::Key(Key::Right), 3));
    let rows = frame(&kept, &mut standing, 10);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("esc to close · ↑↓ pgup pgdn to see more · ←→ result 2 of 3")
    );
}

#[test]
fn a_step_goes_from_the_result_the_footer_counts_when_the_oldest_is_not_read_back_yet() {
    // Twenty-one results as long as a recorded result can be, so the store
    // lets go of the oldest alone, and a log that has not placed it yet. The
    // window may go down as far as that result's top, which is past the rows
    // there are to lay, so the frame is drawn from the furthest down it opens
    // on rows: the footer counts from there, and so must the step after it.
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(Late::default())));
    for at in 0..21 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.finished(&call, said(call.as_str()).into(), at);
    }
    assert_eq!(
        kept.older().count(),
        1,
        "the store let go of other than one"
    );

    let mut standing = Standing::default();
    standing.open(&kept);
    let mut rows = frame(&kept, &mut standing, 40);
    for _ in 0..21 {
        if !standing.against(Pressed::Key(Key::Right), 3) {
            break;
        }
        rows = frame(&kept, &mut standing, 40);
    }
    assert!(rows.iter().any(|row| row.contains(LATER)), "{rows:?}");
    assert_eq!(
        rows.last().map(String::as_str),
        Some("esc to close · ↑↓ pgup pgdn to see more · ←→ result 20 of 21")
    );

    // Result 20 is at the top, so a step back puts result 19 there.
    assert!(standing.against(Pressed::Key(Key::Left), 3));
    let rows = frame(&kept, &mut standing, 40);
    assert_eq!(heading(&rows), Some("Bash(2)"), "{rows:?}");
    assert_eq!(
        rows.last().map(String::as_str),
        Some("esc to close · ↑↓ pgup pgdn to see more · ←→ result 19 of 21")
    );
}

#[test]
fn a_press_back_moves_the_picture_when_the_oldest_is_not_read_back_yet() {
    // As above: the window has gone further down than the rows there are to
    // lay, so the frame is drawn from higher up than the window stands. A
    // press back goes from where it is drawn, so the picture moves at once,
    // and by its own distance rather than by what is left of it.
    const WHEEL: usize = 3;
    for arrived in [
        Pressed::Up,
        Pressed::Scrolled { back: true },
        Pressed::PageUp,
    ] {
        let mut kept = Kept::default();
        kept.logging(Some(Box::new(Late::default())));
        for at in 0..21 {
            let call = crucible_types::ToolId::new(format!("call-{at:03}"));
            kept.calling(call.clone(), format!("Bash({at})"));
            kept.finished(&call, said(call.as_str()).into(), at);
        }

        let mut standing = Standing::default();
        standing.open(&kept);
        let mut rows = frame(&kept, &mut standing, 40);
        for _ in 0..21 {
            if !standing.against(Pressed::Key(Key::Right), 3) {
                break;
            }
            rows = frame(&kept, &mut standing, 40);
        }
        assert!(rows.iter().any(|row| row.contains(LATER)), "{rows:?}");
        let view = opened(&mut standing);
        assert!(
            view.from > view.laid,
            "the window is not past the rows laid"
        );
        let distance = match arrived {
            Pressed::PageUp => view.page - 1,
            Pressed::Scrolled { .. } => WHEEL,
            _ => 1,
        };
        let moved_to = view.laid - distance;

        assert!(standing.against(arrived.clone(), WHEEL), "{arrived:?}");
        assert_eq!(opened(&mut standing).from, moved_to, "{arrived:?}");
        let after = frame(&kept, &mut standing, 40);
        assert_ne!(after, rows, "{arrived:?} redrew the same picture");
    }
}

#[test]
fn nothing_else_moves_it() {
    let ignored = [
        Pressed::Cycle,
        Pressed::Explain,
        Pressed::Clicked { row: 2, column: 8 },
        Pressed::Ignored,
        Pressed::Key(Key::Char('q')),
    ];

    for arrived in ignored {
        let mut open = standing(3, 20);
        assert_eq!(
            moving(arrived.clone(), &mut open),
            Moved::Still,
            "{arrived:?}"
        );
        assert_eq!(open, standing(3, 20));
    }
}

/// A log that gives back, at any place, a result as long as any a log keeps,
/// every line of it naming the call it answered and its own number.
#[derive(Debug)]
struct Long;

impl crate::cli::kept::Log for Long {
    fn landed(&self) -> Vec<(crucible_types::ToolId, u64)> {
        Vec::new()
    }

    fn settled(&self) -> Vec<(crucible_types::ToolId, u64)> {
        Vec::new()
    }

    fn places(&self) -> bool {
        true
    }

    fn read(&self, call: &crucible_types::ToolId, _: u64) -> Option<Box<str>> {
        Some(said(call.as_str()).into())
    }
}

/// Lines in each result below, of sixty-four bytes: as long as a recorded
/// result may be once it is encoded, near enough.
const LINES: usize = 400;

/// What the result of `call` said.
fn said(call: &str) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(LINES * 64);
    for line in 1..=LINES {
        let _ = writeln!(text, "{:-<63}", format!("{call} line {line:04} "));
    }
    text
}

/// Forty results of the length above, the store reading back the ones it lets
/// go of, each offered by row `at` where `at` names a row per result, or all
/// of them by row 0 where they are one folded run.
fn forty(folded: bool) -> Kept {
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(Long)));
    for at in 0..40 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.placing(&call, u64::try_from(at).unwrap());
        if folded {
            kept.gathered(&call, said(call.as_str()).into(), Some(0));
        } else {
            kept.finished(&call, said(call.as_str()).into(), at);
        }
    }
    kept
}

/// What the view holds read back, in bytes.
fn read_back(view: &View) -> usize {
    view.back
        .iter()
        .map(|(_, text)| text.as_ref().map_or(0, |text| text.len()))
        .sum()
}

/// What the store holds, in bytes.
fn held(kept: &Kept) -> usize {
    kept.newest()
        .map(|whole| whole.text().len() + whole.called().len())
        .sum()
}

#[test]
fn opening_every_row_in_turn_holds_one_result_beyond_the_store_at_most() {
    // What is read back is held by the view for as long as it stands, and a
    // row opened after it takes its place: however many rows are opened, what
    // is held is what the store holds and one result more.
    let kept = forty(false);
    let mut read = 0;

    for at in 0..40 {
        let mut standing = Standing::default();
        standing.one(&kept, at);
        let rows = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
        assert!(!rows.is_empty(), "row {at} opened nothing");
        let view = opened(&mut standing);

        assert!(held(&kept) <= 512 * 1024, "row {at}: {} held", held(&kept));
        assert!(
            read_back(view) <= BEYOND,
            "row {at}: {} read back",
            read_back(view)
        );
        read += usize::from(read_back(view) > 0);
    }
    assert!(read > 0, "no row was read back; the test says nothing");
}

/// Walks `standing` from its top to its end, `step` rows at a time, and says
/// what each frame showed and how much was read back at the most.
fn walked(kept: &Kept, standing: &mut Standing, step: usize) -> (Vec<Vec<String>>, usize) {
    let mut frames = Vec::new();
    let mut most = 0;
    loop {
        let rows = laying(kept, opened(standing), Glyphs::Unicode, 80, 40);
        assert!(!rows.is_empty(), "the view closed");
        frames.push(rows.iter().map(Row::text).collect());
        most = most.max(read_back(opened(standing)));
        if !standing.against(Pressed::Scrolled { back: false }, step) {
            return (frames, most);
        }
    }
}

#[test]
fn ctrl_o_stands_every_result_and_reads_back_one_at_a_time() {
    // The key names no result, so it stands every one the rows offer, the ones
    // the store let go of among them, and reads each back as the window
    // reaches it rather than all of them when it opens.
    let kept = forty(false);
    assert!(kept.older().count() > 0, "nothing was let go of");

    let mut standing = Standing::default();
    standing.open(&kept);
    let (frames, most) = walked(&kept, &mut standing, 30);

    let shown = frames.concat().join("\n");
    for at in 0..40 {
        let line = format!("call-{at:03} line 0200");
        assert!(shown.contains(&line), "{line} was never shown");
    }
    assert!(!shown.contains(UNREAD), "a result could not be read back");
    assert!(most > 0 && most <= BEYOND, "{most} read back at once");
}

#[test]
fn a_row_counting_a_run_reads_its_results_back_one_at_a_time() {
    // One row can offer many results, and opening it reads back what the
    // window reaches of them, not all of them at once.
    let kept = forty(true);
    assert!(kept.older().count() > 1, "the run was not let go of");

    let mut standing = Standing::default();
    standing.one(&kept, 0);
    let (frames, most) = walked(&kept, &mut standing, 30);

    let shown = frames.concat().join("\n");
    for at in 0..40 {
        let line = format!("call-{at:03} line 0400");
        assert!(shown.contains(&line), "{line} was never shown");
    }
    assert!(most <= BEYOND, "{most} read back at once");
}

/// The rows of a frame between the rule and the blank above them and the
/// blank and footer below, as far as the first result not read back yet,
/// whose words are the one thing a step may change.
fn text(rows: &[String]) -> &[String] {
    let rows = rows
        .get(2..rows.len().saturating_sub(2))
        .unwrap_or_default();
    let later = rows
        .iter()
        .position(|row| row.contains(LATER))
        .unwrap_or(rows.len());
    rows.get(..later).unwrap_or_default()
}

#[test]
fn a_step_past_a_result_read_back_moves_the_rows_by_that_step() {
    // Letting go of what the window has left, and reading back what it has
    // reached, changes how long the results above the window are. The rows the
    // reader sees move by the step they asked for and no more, down across
    // the results the store let go of and back up again.
    let kept = forty(true);
    let mut standing = Standing::default();
    standing.one(&kept, 0);
    // Straight to the end of what the store holds, and on until a result let
    // go of is read back with the next one waiting under it.
    opened(&mut standing).from = kept.newest().count() * (LINES + 3) - 40;
    loop {
        let rows = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
        if rows.iter().any(|row| row.text().contains(LATER)) {
            break;
        }
        assert!(
            standing.against(Pressed::Scrolled { back: false }, 30),
            "the walk never reached a result let go of"
        );
    }

    let mut last: Vec<String> = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40)
        .iter()
        .map(Row::text)
        .collect();
    let mut read: Vec<Mark> = Vec::new();
    for back in [false, true] {
        for step in 0..60 {
            assert!(
                standing.against(Pressed::Scrolled { back }, 1),
                "stuck at {step}"
            );
            let now: Vec<String> = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40)
                .iter()
                .map(Row::text)
                .collect();
            for (mark, _) in &opened(&mut standing).back {
                if !read.contains(mark) {
                    read.push(mark.clone());
                }
            }

            let (was, is) = (text(&last), text(&now));
            let (kept_on, came) = if back {
                (was.get(..was.len().saturating_sub(1)), is.get(1..))
            } else {
                (was.get(1..), is.get(..is.len().saturating_sub(1)))
            };
            let shorter = kept_on
                .map_or(0, <[String]>::len)
                .min(came.map_or(0, <[String]>::len));
            assert_eq!(
                kept_on.and_then(|rows| rows.get(..shorter)),
                came.and_then(|rows| rows.get(..shorter)),
                "step {step} {}",
                if back { "up" } else { "down" }
            );
            last = now;
        }
    }
    assert!(
        read.len() >= 2,
        "the walk crossed {} results read back",
        read.len()
    );
}

/// A log that says where a result went only once the test says it has been
/// written, and counts every read.
#[derive(Debug, Default)]
struct Late {
    went: std::rc::Rc<std::cell::RefCell<Vec<(crucible_types::ToolId, u64)>>>,
    reads: std::rc::Rc<std::cell::Cell<usize>>,
    stopped: std::rc::Rc<std::cell::Cell<bool>>,
}

impl crate::cli::kept::Log for Late {
    fn landed(&self) -> Vec<(crucible_types::ToolId, u64)> {
        self.went.take()
    }

    fn settled(&self) -> Vec<(crucible_types::ToolId, u64)> {
        self.went.take()
    }

    fn places(&self) -> bool {
        !self.stopped.get()
    }

    fn read(&self, call: &crucible_types::ToolId, _: u64) -> Option<Box<str>> {
        self.reads.set(self.reads.get() + 1);
        Some(said(call.as_str()).into())
    }
}

#[test]
fn a_result_whose_batch_is_not_written_yet_is_read_once_it_is() {
    // A turn draws its results before it writes them, so the view can reach a
    // result the log has not placed yet. It says it will read it back, not
    // that it cannot, and reads it on the frame after the log has it.
    let late = Late::default();
    let went = std::rc::Rc::clone(&late.went);
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(late)));
    for at in 0..40 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.finished(&call, said(call.as_str()).into(), at);
    }

    let mut standing = Standing::default();
    standing.one(&kept, 0);
    let before = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
    let before: Vec<String> = before.iter().map(Row::text).collect();
    assert!(!before.iter().any(|row| row.contains(UNREAD)), "{before:?}");

    went.borrow_mut()
        .push((crucible_types::ToolId::new("call-000"), 0));
    let after = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
    assert!(
        after
            .iter()
            .any(|row| row.text().contains("call-000 line 0001")),
        "{after:?}"
    );
}

#[test]
fn a_result_left_for_later_is_not_read_again_while_the_window_stands_still() {
    // Two results let go of in one window come to more than the view reads
    // back at once, so the second waits. Drawing the same window again reads
    // nothing more from the log.
    let late = Late::default();
    let reads = std::rc::Rc::clone(&late.reads);
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(late)));
    for at in 0..40 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.placing(&call, u64::try_from(at).unwrap());
        kept.gathered(&call, said(call.as_str()).into(), Some(0));
    }

    let mut standing = Standing::default();
    standing.one(&kept, 0);
    opened(&mut standing).from = kept.newest().count() * (LINES + 3) - 40;
    loop {
        let rows = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
        if rows.iter().any(|row| row.text().contains(LATER)) {
            break;
        }
        assert!(
            standing.against(Pressed::Scrolled { back: false }, 30),
            "the walk never reached two results let go of"
        );
    }

    let read = reads.get();
    for _ in 0..5 {
        laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
    }
    assert_eq!(
        reads.get(),
        read,
        "the window stood still and the log was read again"
    );
}

#[test]
fn a_result_the_log_will_never_place_says_it_cannot_be_read_back() {
    let late = Late::default();
    late.stopped.set(true);
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(late)));
    for at in 0..40 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.finished(&call, said(call.as_str()).into(), at);
    }

    let mut standing = Standing::default();
    standing.one(&kept, 0);
    let rows = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
    assert!(
        rows.iter().any(|row| row.text().contains(UNREAD)),
        "{:?}",
        rows.iter().map(Row::text).collect::<Vec<_>>()
    );
}

#[test]
fn the_first_result_the_window_reaches_is_read_whatever_was_refused_before() {
    // A result left for later is left only beside something read back: with
    // nothing above it, as after a resize, it is read.
    let kept = forty(false);
    let mut standing = Standing::default();
    standing.one(&kept, 0);
    let mark = kept
        .older()
        .find(|placed| placed.at() == 0)
        .map(Placed::mark)
        .expect("row 0 was let go of");
    opened(&mut standing).refused = Some((0, mark));

    let rows = laying(&kept, opened(&mut standing), Glyphs::Unicode, 80, 40);
    assert!(
        rows.iter()
            .any(|row| row.text().contains("call-000 line 0001")),
        "{:?}",
        rows.iter().map(Row::text).collect::<Vec<_>>()
    );
}

#[test]
fn ctrl_o_stands_results_newest_first_when_a_short_one_stays_held() {
    // A short result costs less held than let go of, so it can outlast longer
    // ones drawn after it. The view still reads newest first.
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(Long)));
    for at in 0..40 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.placing(&call, u64::try_from(at).unwrap());
        let text = if at == 0 {
            "short".to_owned()
        } else {
            said(call.as_str())
        };
        kept.finished(&call, text.into(), at);
    }
    assert!(
        kept.newest().any(|whole| whole.at() == Some(0)),
        "the short result was let go of; the test says nothing"
    );

    let mut standing = Standing::default();
    standing.open(&kept);
    let order: Vec<usize> = entries(&kept, &opened(&mut standing).over)
        .iter()
        .map(Entry::drawn)
        .collect();
    let mut sorted = order.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(order, sorted);
    assert_eq!(order.len(), 40);
}

#[test]
fn a_folded_row_opens_on_its_results_newest_first_when_a_short_one_stays_held() {
    let mut kept = Kept::default();
    kept.logging(Some(Box::new(Long)));
    for at in 0..40_u64 {
        let call = crucible_types::ToolId::new(format!("call-{at:03}"));
        kept.calling(call.clone(), format!("Bash({at})"));
        kept.placing(&call, at);
        let text = if at == 0 {
            "short".to_owned()
        } else {
            said(call.as_str())
        };
        kept.gathered(&call, text.into(), Some(0));
    }
    assert!(kept.older().count() > 0, "nothing was let go of");

    let order: Vec<usize> = entries(&kept, &Over::One(0))
        .iter()
        .map(Entry::drawn)
        .collect();
    let mut sorted = order.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(order, sorted);
    assert_eq!(order.len(), 40);
}
