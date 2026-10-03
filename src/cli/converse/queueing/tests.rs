use crucible_runner::Breakdown;
use crucible_tui::{Key, Recording};

use super::super::Retained;
use super::*;

/// A queue with these lines waiting, and the offer the turn reads holding the
/// same ones — which is the state a session is in the moment a line is typed
/// under a running turn.
fn queued(lines: &[&str]) -> (Prompts, Steer) {
    let mut queue = Prompts::default();
    let steer = Steer::new();

    for line in lines {
        let mut editor = Editor::new();
        for key in line.chars() {
            editor.press(Key::Char(key));
        }

        steer.say((*line).to_owned());
        assert_eq!(queue.accept(&mut editor), Retained::Accepted);
    }

    (queue, steer)
}

/// One key against a standing view, with the three things it acts on.
fn against(standing: &mut Standing, arrived: &Pressed, queue: &mut Prompts, steer: &Steer) -> bool {
    let mut editor = Editor::new();
    standing.against(
        arrived,
        Reading {
            queue,
            editor: &mut editor,
            steer,
        },
    )
}

#[test]
fn nothing_opens_on_an_empty_queue() {
    // The key is offered by the panel that names what is waiting, so a session
    // with nothing waiting has made no offer -- and a frame put up for a press
    // nobody meant is one that took the box away for no reason.
    let (queue, steer) = queued(&[]);
    let mut standing = Standing::default();

    standing.open(&queue, &steer);

    assert!(!standing.is_open());
    assert!(
        !steer.any(),
        "an empty queue was held for a view nobody saw"
    );
}

#[test]
fn the_turn_takes_nothing_while_the_queue_stands_open() {
    // The whole of what the view is for. A line the reader is still going over
    // is not one the agent should be reading, and one taken mid-edit is in the
    // transcript, where it cannot be taken back.
    let (mut queue, steer) = queued(&["first", "second"]);
    let mut standing = Standing::default();

    standing.open(&queue, &steer);
    assert!(standing.is_open());

    assert!(
        !steer.any(),
        "the turn was told there was something to take"
    );
    assert!(steer.take().is_empty(), "the turn took a line mid-edit");

    // And the walk over it changes nothing about that: every key but the way
    // out leaves the queue where it is.
    against(&mut standing, &Pressed::Down, &mut queue, &steer);

    assert!(steer.take().is_empty());
}

#[test]
fn closing_it_gives_the_whole_batch_back_at_once() {
    // Edited or not, together: what the reader closes the queue on is one
    // course-correction, and the turn works it in at one pass boundary.
    let (mut queue, steer) = queued(&["first", "second"]);
    let mut standing = Standing::default();

    standing.open(&queue, &steer);
    against(&mut standing, &Pressed::Escape, &mut queue, &steer);

    assert!(!standing.is_open());
    assert!(steer.any());
    assert_eq!(steer.take(), vec!["first".to_owned(), "second".to_owned()]);
}

#[test]
fn a_line_taken_back_leaves_the_queue_the_turn_reads_as_well() {
    // The panel and the turn's own offer hold the same line. One dropped from
    // the panel alone is a prompt the reader deleted that the turn goes on to
    // work in anyway -- which is the one thing holding the queue cannot save
    // them from on its own.
    let (mut queue, steer) = queued(&["first", "second", "third"]);
    let mut editor = Editor::new();
    let mut standing = Standing::default();

    standing.open(&queue, &steer);
    against(&mut standing, &Pressed::Down, &mut queue, &steer);
    standing.against(
        &Pressed::Key(Key::Char('x')),
        Reading {
            queue: &mut queue,
            editor: &mut editor,
            steer: &steer,
        },
    );

    assert_eq!(
        queue.waiting_all().collect::<Vec<_>>(),
        vec!["first", "third"]
    );
    assert_eq!(editor.text(), "second", "it went back into the box");

    against(&mut standing, &Pressed::Escape, &mut queue, &steer);

    assert_eq!(steer.take(), vec!["first".to_owned(), "third".to_owned()]);
}

#[test]
fn taking_the_last_line_back_closes_it_and_gives_the_queue_back() {
    // The list it was read from is then empty, so the way out is the same key
    // that emptied it -- and a view left standing over nothing would go on
    // holding a queue with nothing in it.
    let (mut queue, steer) = queued(&["only"]);
    let mut standing = Standing::default();

    standing.open(&queue, &steer);
    against(
        &mut standing,
        &Pressed::Key(Key::Char('x')),
        &mut queue,
        &steer,
    );

    assert!(!standing.is_open());
    assert_eq!(queue.waiting_count(), 0);
    assert!(steer.take().is_empty(), "the line was taken back, not sent");
    assert!(!steer.any());
}

#[test]
fn a_line_typed_while_it_stands_goes_out_with_the_rest() {
    // The box is still live under the view's own keys, and a line finished in
    // it is still queued. What arrives while the queue is held is held with it
    // rather than reaching the turn on its own.
    let (queue, steer) = queued(&["first"]);
    let mut standing = Standing::default();

    standing.open(&queue, &steer);
    steer.say("second".to_owned());
    assert!(!steer.any());

    steer.release();
    assert_eq!(steer.take(), vec!["first".to_owned(), "second".to_owned()]);
}

/// The rows of the view as plain text, one string each.
fn said(laid: &[crucible_tui::Row]) -> Vec<String> {
    laid.iter().map(crucible_tui::Row::text).collect()
}

/// Takes the key against a standing view of `lines` with the mark on `at`, and
/// hands back what is left: the queue, the box, and the offer the turn reads.
fn after(lines: &[&str], at: usize, key: Key) -> (Standing, Prompts, Editor, Steer) {
    let (mut queue, steer) = queued(lines);
    let mut editor = Editor::new();
    let mut standing = Standing::default();

    standing.open(&queue, &steer);
    for _ in 0..at {
        against(&mut standing, &Pressed::Down, &mut queue, &steer);
    }
    standing.against(
        &Pressed::Key(key),
        Reading {
            queue: &mut queue,
            editor: &mut editor,
            steer: &steer,
        },
    );

    (standing, queue, editor, steer)
}

#[test]
fn the_queue_view_stands_as_a_panel_with_the_keys_named_in_its_footer() {
    // The panel every other list follows: a rule, a title, the rows, and a
    // footer naming every key that is offered. The marked line leads with the
    // mark and the rest stand two columns in under it.
    let (queue, _) = queued(&["first", "second", "third"]);
    let laid = rows(&queue, 1, 80, 20, Style::plain());

    assert_eq!(
        said(&laid),
        vec![
            "\u{2500}".repeat(80),
            String::new(),
            "3 queued".to_owned(),
            String::new(),
            "  first".to_owned(),
            "\u{203a} second".to_owned(),
            "  third".to_owned(),
            String::new(),
            "\u{2191}\u{2193} to walk \u{b7} e edit \u{b7} d delete \u{b7} esc to close".to_owned(),
        ]
    );
}

#[test]
fn the_queue_view_marks_the_line_the_keys_act_on_in_the_accent() {
    // A key's target is never a guess: the mark and the words of the marked
    // line are the accent, the rest read plain, and the footer is quiet.
    use crucible_tui::Slot;

    let (queue, _) = queued(&["first", "second"]);
    let laid = rows(&queue, 1, 40, 20, Style::plain());
    let slots = |at: usize| {
        laid.get(at)
            .expect("a row there")
            .spans()
            .map(|(slot, _)| slot)
            .filter(|slot| *slot != Slot::Plain)
            .collect::<Vec<_>>()
    };

    assert_eq!(slots(0), vec![Slot::Accent], "the rule");
    assert_eq!(slots(2), vec![Slot::Strong], "the title");
    assert!(slots(4).is_empty(), "the unmarked line: {:?}", said(&laid));
    assert_eq!(
        slots(5),
        vec![Slot::Accent, Slot::Accent],
        "the marked line"
    );
    assert!(
        slots(8).iter().all(|slot| *slot == Slot::Quiet),
        "the footer"
    );
}

#[test]
fn the_queue_view_wraps_a_line_and_its_footer_in_a_narrow_window() {
    // Lines are cut to a row in the box and wrap here, where they are read
    // whole; a wrapped line hangs under its own first word. Nothing is wider
    // than the window, which is the whole of what narrow asks.
    let (queue, _) = queued(&[
        "and add a test for the windows path",
        "keep the old error text",
    ]);
    let laid = rows(&queue, 0, 40, 20, Style::plain());
    let rows = said(&laid);

    assert_eq!(
        rows.get(4).map(String::as_str),
        Some("\u{203a} and add a test for the windows path")
    );
    assert!(
        rows.iter().all(|row| crucible_tui::columns(row) <= 40),
        "{rows:?}"
    );
    assert_eq!(
        rows.get(laid.len() - 2).map(String::as_str),
        Some("\u{2191}\u{2193} to walk \u{b7} e edit \u{b7} d delete \u{b7} esc to"),
        "{rows:?}"
    );
    assert_eq!(rows.last().map(String::as_str), Some("close"), "{rows:?}");

    let narrow = said(&self::rows(&queue, 0, 24, 20, Style::plain()));
    assert!(
        narrow.iter().all(|row| crucible_tui::columns(row) <= 24),
        "{narrow:?}"
    );
    assert!(narrow.contains(&"  windows path".to_owned()), "{narrow:?}");
}

#[test]
fn a_window_with_no_room_for_a_name_lays_nothing_out() {
    // Which both callers read as the view closing. Chrome with nothing under it
    // is a frame that took the box away and put nothing in its place.
    let (queue, _) = queued(&["first"]);

    assert!(rows(&queue, 0, 80, 6, Style::plain()).is_empty());
    assert!(!rows(&queue, 0, 80, 7, Style::plain()).is_empty());

    // The footer is a row more where it wraps, and that row is chrome too.
    assert!(rows(&queue, 0, 40, 7, Style::plain()).is_empty());
    assert!(!rows(&queue, 0, 40, 8, Style::plain()).is_empty());
}

#[test]
fn walking_past_the_last_drawn_line_scrolls_so_the_marked_line_is_always_drawn() {
    // The defect this pins: the list was always drawn from its first line, so in
    // a window short of the whole queue the mark could stand on a line that was
    // not on screen, and `d` would delete words the reader had never seen.
    let lines = ["one", "two", "three", "four", "five", "six"];
    let (queue, _) = queued(&lines);

    // Two rows for lines: the footer is one row at 80 columns.
    for (at, line) in lines.iter().enumerate() {
        let drawn = said(&rows(&queue, at, 80, CHROME + 1 + 2, Style::plain()));

        assert!(
            drawn.contains(&format!("\u{203a} {line}")),
            "{at}: {line} is marked and not drawn in {drawn:?}"
        );
        assert_eq!(
            drawn
                .iter()
                .filter(|row| row.starts_with('\u{203a}'))
                .count(),
            1,
            "{at}: {drawn:?}"
        );
    }

    // And what `d` removes there is exactly the line the mark stood on.
    let (_, queue, editor, _) = after(&lines, 5, Key::Char('d'));
    assert_eq!(
        queue.waiting_all().collect::<Vec<_>>(),
        vec!["one", "two", "three", "four", "five"]
    );
    assert_eq!(editor.text(), "");
}

#[test]
fn the_view_scrolls_by_wrapped_rows_and_keeps_a_tall_line_drawn_from_its_start() {
    let first = "alpha beta gamma delta epsilon zeta eta theta iota";
    let second = "kappa lambda mu nu xi omicron pi rho sigma tau";
    let (queue, _) = queued(&[first, "short", second]);

    // Forty columns fold each long line over two rows, so the third is the
    // marked one and the room for lines is three rows: it takes two of them
    // and the line before it takes the third.
    let drawn = said(&rows(&queue, 2, 40, CHROME + 2 + 3, Style::plain()));
    let whole = drawn.join("\n");
    assert!(
        whole.contains("\u{203a} kappa lambda mu nu xi omicron pi"),
        "{whole}"
    );
    assert!(whole.contains("sigma tau"), "{whole}");
    assert!(whole.contains("short"), "{whole}");
    assert!(!whole.contains("alpha"), "{whole}");

    // A line taller than all the room is drawn from its first row.
    let drawn = said(&rows(&queue, 0, 40, CHROME + 2 + 1, Style::plain())).join("\n");
    assert!(drawn.contains("\u{203a} alpha beta gamma delta"), "{drawn}");
}

#[test]
fn a_window_that_holds_the_list_only_without_the_working_row_draws_the_whole_list() {
    // The row that says a turn is running is the first to give way: the list is
    // what the reader opened, and a window one row short of both draws the list
    // whole rather than the row and no list (which would close the view).
    let (queue, steer) = queued(&["first"]);
    let turning = Turning::started(Breakdown::default());
    let working = turning.working(80, Style::plain()).text();

    // Seven rows is the least the list takes at this width, and one row of the
    // window always stays with the transcript.
    let drawn = |window: usize| {
        let mut standing = Standing::default();
        standing.open(&queue, &steer);
        let mut render = Renderer::new(Recording::new(80, window));
        let stood = under(
            &mut render,
            Style::plain(),
            &queue,
            &mut standing,
            &steer,
            &turning,
        )
        .expect("drawn");
        (stood, render.terminal().written().to_owned())
    };

    let (stood, tight) = drawn(8);
    assert!(stood);
    assert!(tight.contains("first"), "{tight:?}");
    assert!(tight.contains("esc to close"), "{tight:?}");
    assert!(!tight.contains(working.trim()), "{tight:?}");

    // One row more and the row stands over the rule as well.
    let (stood, roomy) = drawn(9);
    assert!(stood);
    assert!(roomy.contains(working.trim()), "{roomy:?}");
    assert!(roomy.contains("first"), "{roomy:?}");
}

#[test]
fn editing_a_queued_line_moves_its_words_to_the_box_and_leaves_the_rest() {
    // `e` is the key the footer names, and it does what `x` always did: the
    // line leaves the queue, in both places it is held, and the box has it with
    // the cursor after it.
    let (standing, queue, mut editor, steer) =
        after(&["first", "second", "third"], 1, Key::Char('e'));

    assert_eq!(
        queue.waiting_all().collect::<Vec<_>>(),
        vec!["first", "third"]
    );
    assert_eq!(editor.text(), "second");
    editor.press(Key::Char('!'));
    assert_eq!(editor.text(), "second!", "the cursor is after the line");
    assert!(standing.is_open(), "two lines are still being read");

    assert!(!steer.any(), "the view holds what is left");
    steer.release();
    assert_eq!(steer.take(), vec!["first".to_owned(), "third".to_owned()]);
}

#[test]
fn x_still_edits_what_e_edits() {
    // Kept for the hands that learned it; the footer names the one key.
    let (_, queue, editor, _) = after(&["first", "second"], 0, Key::Char('x'));

    assert_eq!(queue.waiting_all().collect::<Vec<_>>(), vec!["second"]);
    assert_eq!(editor.text(), "first");
}

#[test]
fn editing_a_queued_line_the_box_cannot_take_keeps_it_queued() {
    // The box already holds a draft, and the marked line is too long to go in
    // beside it. Taken out of the queue before the box refused it, the line was
    // in neither place: something the reader typed, gone without a word.
    use crucible_tui::Typed;

    let (mut queue, steer) = queued(&["first"]);
    let long = "y".repeat(Editor::MAX_BYTES - 16);
    let mut typing = Editor::new();
    assert_eq!(typing.paste(&long), Typed::Changed);
    steer.say(long.clone());
    assert_eq!(queue.accept(&mut typing), Retained::Accepted);

    let draft = "a draft still being written";
    let mut editor = Editor::new();
    assert_eq!(editor.paste(draft), Typed::Changed);

    let mut standing = Standing::default();
    standing.open(&queue, &steer);
    against(&mut standing, &Pressed::Down, &mut queue, &steer);

    for key in ['e', 'x'] {
        standing.against(
            &Pressed::Key(Key::Char(key)),
            Reading {
                queue: &mut queue,
                editor: &mut editor,
                steer: &steer,
            },
        );

        // Compared by length, so a failure does not print a megabyte.
        assert_eq!(
            queue.waiting_all().map(str::len).collect::<Vec<_>>(),
            vec!["first".len(), long.len()],
            "{key} lost a line the box could not take"
        );
        assert_eq!(queue.waiting_all().nth(1), Some(long.as_str()));
        assert_eq!(queue.bytes, "first".len() + long.len());
        assert_eq!(editor.text(), draft, "the draft is as it was");
        assert_eq!(standing, Standing::Open(1), "the mark is still on it");
    }

    steer.release();
    let taken = steer.take();
    assert_eq!(
        taken.iter().map(String::len).collect::<Vec<_>>(),
        vec!["first".len(), long.len()],
        "the turn still reads it"
    );
    assert!(taken.last().is_some_and(|last| *last == long));
}

#[test]
fn deleting_a_queued_line_removes_it_without_taking_it_back() {
    // The line is gone from the queue and from what the turn reads, and the box
    // is exactly as it was: nothing was put there to be sent by accident.
    for key in [Key::Char('d'), Key::Delete] {
        let (standing, queue, editor, steer) = after(&["first", "second", "third"], 1, key);

        assert_eq!(
            queue.waiting_all().collect::<Vec<_>>(),
            vec!["first", "third"]
        );
        assert_eq!(queue.waiting_count(), 2);
        assert_eq!(editor.text(), "", "the line was deleted, not taken back");
        assert!(standing.is_open());

        steer.release();
        assert_eq!(steer.take(), vec!["first".to_owned(), "third".to_owned()]);
    }
}

#[test]
fn deleting_the_last_queued_line_closes_the_view_and_sends_nothing() {
    let (standing, queue, editor, steer) = after(&["only"], 0, Key::Char('d'));

    assert!(!standing.is_open());
    assert_eq!(queue.waiting_count(), 0);
    assert_eq!(editor.text(), "");
    assert!(!steer.any());
    assert!(steer.take().is_empty(), "the deleted line was sent anyway");
}

#[test]
fn deleting_a_queued_line_gives_back_the_bytes_it_held() {
    // The ceiling is on what is waiting, so a deleted line is room for another.
    let (_, queue, _, _) = after(&["first", "second"], 0, Key::Char('d'));

    assert_eq!(queue.bytes, "second".len());
}

#[test]
fn the_queue_view_follows_the_colour_rule() {
    // The marked line is the accent from its mark to its last word; the rest
    // read plain and the footer quiet.
    let (queue, _) = queued(&[
        "first",
        "a second line long enough to wrap in a narrow window, and then some",
        "third",
    ]);
    for (columns, glyphs) in [(80, Style::plain()), (40, Style::plain())] {
        for at in 0..3 {
            let laid = rows(&queue, at, columns, 20, glyphs);

            crate::cli::colour_rule::holds(&format!("queue view at {columns}"), &laid, |row| {
                crate::cli::colour_rule::marked(row)
            });
        }
    }
}
