use crucible_tui::{Glyphs, Key};

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

/// The rows of the panel as plain text, one string each.
fn said(laid: &[Row]) -> Vec<String> {
    laid.iter().map(Row::text).collect()
}

/// The lines still waiting, oldest first.
fn waiting(queue: &Prompts) -> Vec<&str> {
    queue.waiting_all().collect()
}

const FIVE: [&str; 5] = ["first", "second", "third", "fourth", "fifth"];

#[test]
fn nothing_waiting_lays_no_panel_out() {
    assert!(panel(&Prompts::default(), 80, 40, Style::plain()).is_empty());
}

#[test]
fn the_panel_names_three_lines_under_a_title_that_counts_them_all() {
    // A rule, the count and the key that sends them all, three lines with the
    // highlighted one marked, and a footer naming every key that works.
    let (queue, _) = queued(&FIVE);
    let rule = "─".repeat(80);

    assert_eq!(
        said(&panel(&queue, 80, 40, Style::plain())),
        vec![
            rule.as_str(),
            "",
            "5 queued · ctrl+enter to send all now",
            "",
            "› first",
            "",
            "  second",
            "",
            "  third",
            "",
            "↑↓ to walk · ctrl+e to edit · ctrl+x to delete · ctrl+s to send now",
            "",
        ]
    );
}

#[test]
fn the_panel_in_ascii_says_the_same_in_the_glyphs_every_terminal_has() {
    let (queue, _) = queued(&FIVE);
    let rule = "-".repeat(80);

    assert_eq!(
        said(&panel(&queue, 80, 40, Style::drawn(Glyphs::Ascii))),
        vec![
            rule.as_str(),
            "",
            "5 queued - ctrl+enter to send all now",
            "",
            "> first",
            "",
            "  second",
            "",
            "  third",
            "",
            "^v to walk - ctrl+e to edit - ctrl+x to delete - ctrl+s to send now",
            "",
        ]
    );
}

#[test]
fn a_narrow_window_folds_the_footer_rather_than_cutting_a_key_off_it() {
    let (queue, _) = queued(&FIVE);
    let laid = said(&panel(&queue, 40, 40, Style::plain()));

    assert!(laid.iter().all(|row| crucible_tui::columns(row) <= 40));
    assert_eq!(
        laid.get(laid.len() - 3..),
        Some(
            &[
                "↑↓ to walk · ctrl+e to edit · ctrl+x to".to_owned(),
                "delete · ctrl+s to send now".to_owned(),
                String::new(),
            ][..]
        )
    );
}

#[test]
fn the_arrows_walk_the_highlight_and_stop_at_either_end() {
    // Stopping rather than going round leaves the highlight at the end it was
    // walked to; the arrow past it goes nowhere else while a prompt waits.
    let (mut queue, _) = queued(&["first", "second"]);

    assert!(!queue.walk(true), "nothing is before the first");
    assert!(queue.walk(false));
    assert_eq!(queue.highlighted(), 1);
    assert!(!queue.walk(false), "nothing is after the last");
    assert!(queue.walk(true));
    assert_eq!(queue.highlighted(), 0);
}

#[test]
fn a_line_queued_behind_the_highlight_leaves_it_on_the_line_it_was_on() {
    // A line typed while the reader is partway down the queue joins it at the
    // end. The keys go on acting on the line the reader walked to.
    let (mut queue, _) = queued(&["first", "second", "third"]);
    assert!(queue.walk(false));
    assert!(queue.walk(false));

    let mut editor = Editor::new();
    for key in "fourth".chars() {
        editor.press(Key::Char(key));
    }
    assert_eq!(queue.accept(&mut editor), Retained::Accepted);

    assert_eq!(queue.highlighted(), 2, "the highlight moved off the third");
}

#[test]
fn the_three_named_follow_the_highlight_down_the_queue() {
    let (mut queue, _) = queued(&FIVE);
    for _ in 0..3 {
        queue.walk(false);
    }

    let laid = said(&panel(&queue, 80, 40, Style::plain()));
    assert_eq!(
        laid.get(4..9),
        Some(
            &[
                "  second".to_owned(),
                String::new(),
                "  third".to_owned(),
                String::new(),
                "› fourth".to_owned(),
            ][..]
        )
    );
}

#[test]
fn ctrl_x_deletes_the_highlighted_line_from_the_queue_and_the_turn() {
    // The panel and the turn's own offer hold the same line. One dropped from
    // the panel alone is a prompt the reader deleted that the turn goes on to
    // work in anyway.
    let (mut queue, steer) = queued(&["first", "second", "third"]);
    queue.walk(false);

    assert!(queue.delete(Offer::Turn(&steer)));

    assert_eq!(waiting(&queue), vec!["first", "third"]);
    assert_eq!(
        queue.highlighted(),
        1,
        "on the line that came up into its place"
    );
    assert_eq!(queue.bytes, "first".len() + "third".len());
    assert_eq!(steer.take(), vec!["first".to_owned(), "third".to_owned()]);
}

#[test]
fn deleting_the_last_line_in_the_queue_leaves_the_highlight_on_the_new_last() {
    let (mut queue, steer) = queued(&["first", "second"]);
    queue.walk(false);

    assert!(queue.delete(Offer::Turn(&steer)));
    assert_eq!(queue.highlighted(), 0);

    assert!(queue.delete(Offer::Turn(&steer)));
    assert_eq!(queue.waiting_count(), 0);
    assert!(
        panel(&queue, 80, 40, Style::plain()).is_empty(),
        "the panel goes"
    );
    assert!(!steer.any(), "a deleted line was sent anyway");
    assert!(
        !queue.delete(Offer::Turn(&steer)),
        "nothing is left to delete"
    );
}

#[test]
fn a_line_the_turn_has_already_taken_is_past_deleting_or_taking_back() {
    // The turn takes its whole offer at a pass boundary and says which lines
    // it took a moment later. A key in between finds the line still named here
    // but already the turn's: deleted, it would be sent anyway, and taken back
    // it would be sent twice. It stays named until the turn says it took it.
    let (mut queue, steer) = queued(&["first", "second"]);
    let taken = steer.take();
    let mut editor = Editor::new();

    assert!(
        !queue.delete(Offer::Turn(&steer)),
        "a line the turn took was shown deleted"
    );
    assert!(
        !queue.take_back(&mut editor, Offer::Turn(&steer)),
        "a line the turn took was put back in the box"
    );
    assert!(editor.is_empty(), "the box holds a line the turn will send");
    assert_eq!(waiting(&queue), vec!["first", "second"]);
    assert_eq!(queue.bytes, "first".len() + "second".len());

    for line in &taken {
        assert!(
            queue.steered(line),
            "{line} was not waiting when the turn said it took it"
        );
    }
    assert_eq!(queue.waiting_count(), 0);
}

#[test]
fn ctrl_e_takes_the_highlighted_line_back_into_the_box() {
    // At the cursor, out of the queue, and out of what the turn will read: a
    // line being edited is not one the reader has sent.
    let (mut queue, steer) = queued(&["first", "second", "third"]);
    let mut editor = Editor::new();
    queue.walk(false);

    assert!(queue.take_back(&mut editor, Offer::Turn(&steer)));

    assert_eq!(editor.text(), "second");
    editor.press(Key::Char('!'));
    assert_eq!(editor.text(), "second!", "the cursor is after the line");
    assert_eq!(waiting(&queue), vec!["first", "third"]);
    assert_eq!(queue.highlighted(), 1);
    assert_eq!(steer.take(), vec!["first".to_owned(), "third".to_owned()]);
}

#[test]
fn between_turns_the_panel_holds_the_only_copy_and_both_keys_reach_it() {
    // A used-up plan held these lines with no turn running, so no steer has
    // them: the keys act on the panel alone, and nothing else is asked.
    let (mut queue, _) = queued(&["first", "second", "third"]);
    let mut editor = Editor::new();

    assert!(queue.take_back(&mut editor, Offer::Nowhere));
    assert_eq!(editor.text(), "first");
    assert!(queue.delete(Offer::Nowhere));

    assert_eq!(waiting(&queue), vec!["third"]);
}

#[test]
fn a_line_the_box_has_no_room_for_stays_queued_and_the_panel_says_so() {
    // The box already holds a draft, and the highlighted line is too long to
    // go in beside it. Taken out of the queue before the box refused it, the
    // line would be in neither place.
    let (mut queue, steer) = queued(&["first"]);
    let long = "y".repeat(Editor::MAX_BYTES - 16);
    let mut typing = Editor::new();
    assert_eq!(typing.paste(&long), Typed::Changed);
    steer.say(long.clone());
    assert_eq!(queue.accept(&mut typing), Retained::Accepted);

    let draft = "a draft still being written";
    let mut editor = Editor::new();
    assert_eq!(editor.paste(draft), Typed::Changed);
    queue.walk(false);

    assert!(
        queue.take_back(&mut editor, Offer::Turn(&steer)),
        "the panel owes a frame"
    );

    // Compared by length, so a failure does not print a megabyte.
    assert_eq!(
        queue.waiting_all().map(str::len).collect::<Vec<_>>(),
        vec!["first".len(), long.len()]
    );
    assert_eq!(queue.bytes, "first".len() + long.len());
    assert_eq!(queue.highlighted(), 1, "the highlight is still on it");
    assert_eq!(editor.text(), draft, "the draft is as it was");
    assert_eq!(
        steer.take().iter().map(String::len).collect::<Vec<_>>(),
        vec!["first".len(), long.len()],
        "the turn still reads it"
    );

    let laid = said(&panel(&queue, 80, 40, Style::plain()));
    assert_eq!(
        laid.get(2).map(String::as_str),
        Some("2 queued · ctrl+enter to send all now · no room in the box · line stays queued"),
        "beside the title, where the row holds both"
    );

    assert!(queue.settle(), "the next key clears it");
    assert!(!queue.settle(), "and only once");
    let laid = said(&panel(&queue, 80, 40, Style::plain()));
    assert_eq!(
        laid.get(2).map(String::as_str),
        Some("2 queued · ctrl+enter to send all now")
    );
}

#[test]
fn a_narrow_window_says_the_box_had_no_room_under_the_title() {
    let (mut queue, steer) = queued(&["first"]);
    let mut editor = Editor::new();
    assert_eq!(
        editor.paste(&"y".repeat(Editor::MAX_BYTES - 2)),
        Typed::Changed
    );
    queue.take_back(&mut editor, Offer::Turn(&steer));

    let laid = said(&panel(&queue, 40, 40, Style::plain()));
    assert_eq!(
        laid.get(2..5),
        Some(
            &[
                "1 queued · ctrl+enter to send all now".to_owned(),
                "no room in the box · line stays queued".to_owned(),
                "› first".to_owned(),
            ][..]
        ),
        "in the blank that parted the title from the lines"
    );

    let laid = said(&panel(&queue, 24, 40, Style::plain()));
    assert!(laid.iter().all(|row| crucible_tui::columns(row) <= 24));
    assert!(
        laid.iter().any(|row| row == "line stays queued"),
        "folded, not cut: {laid:?}"
    );
}

#[test]
fn a_line_leaving_from_before_the_highlight_keeps_it_on_the_same_line() {
    // What the turn taking the oldest line does while the reader has walked
    // further down: the line they were on is still the one the keys act on.
    let (mut queue, _) = queued(&["first", "second", "third"]);
    queue.walk(false);
    queue.walk(false);

    assert!(queue.steered("first"));

    assert_eq!(waiting(&queue), vec!["second", "third"]);
    assert_eq!(queue.highlighted(), 1);
    assert_eq!(queue.waiting_all().nth(queue.highlighted()), Some("third"));
}

#[test]
fn a_short_window_names_fewer_lines_and_none_below_one() {
    // At 80 columns the rows around the lines are seven: the rule and its
    // blank, the title and its blank, the blank over the footer, the footer
    // and the blank under it. Each line past the first costs a blank too.
    let (queue, _) = queued(&FIVE);
    let named = |room| {
        said(&panel(&queue, 80, room, Style::plain()))
            .iter()
            .filter(|row| {
                row.ends_with("first") || row.ends_with("second") || row.ends_with("third")
            })
            .count()
    };

    assert_eq!(named(7), 0);
    assert!(panel(&queue, 80, 7, Style::plain()).is_empty());
    assert_eq!(named(8), 1);
    assert_eq!(named(10), 2);
    assert_eq!(named(12), 3);
    assert_eq!(named(40), 3);
    for room in 0..40 {
        assert!(panel(&queue, 80, room, Style::plain()).len() <= room);
    }
}

#[test]
fn a_megabyte_line_is_cut_to_its_row() {
    let long = "y".repeat(Editor::MAX_BYTES - 1);
    let (queue, _) = queued(&[long.as_str()]);
    let style = Style::plain();

    let laid = said(&panel(&queue, 80, 40, style));
    let named = laid.get(4).map(String::as_str).unwrap_or_default();
    assert_eq!(crucible_tui::columns(named), 80);
    assert!(named.starts_with("› yyy"));
    assert!(named.ends_with(style.glyphs().ellipsis()));
}

#[test]
fn deleting_a_queued_line_gives_back_the_bytes_it_held() {
    // The ceiling is on what is waiting, so a deleted line is room for another.
    let (mut queue, steer) = queued(&["first", "second"]);
    queue.delete(Offer::Turn(&steer));

    assert_eq!(queue.bytes, "second".len());
}

#[test]
fn the_panel_follows_the_colour_rule() {
    // The highlighted line is the accent from its mark to its last word; the
    // rule is the one other accent, and every other row reads plain or quiet.
    let (mut queue, steer) = queued(&[
        "first",
        "a second line long enough to be cut in a narrow window, and then some",
        "third",
    ]);
    let mut full = Editor::new();
    assert_eq!(
        full.paste(&"y".repeat(Editor::MAX_BYTES - 2)),
        Typed::Changed
    );

    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        for columns in [80, 40, 24] {
            for at in 0..3 {
                while queue.walk(true) {}
                for _ in 0..at {
                    queue.walk(false);
                }
                for refused in [false, true] {
                    queue.settle();
                    if refused {
                        queue.take_back(&mut full, Offer::Turn(&steer));
                    }
                    let laid = panel(&queue, columns, 40, Style::drawn(glyphs));

                    crate::cli::colour_rule::holds(
                        &format!("queue panel at {columns}, {at}, {refused}"),
                        &laid,
                        crate::cli::colour_rule::marked,
                    );
                }
            }
        }
    }
}

#[test]
fn a_window_too_narrow_for_a_line_beside_its_mark_lays_no_panel_out() {
    // Two columns are the mark and the space after it, so a window that wide
    // has nowhere to put any of the line; one wider has, and every row it
    // lays out stays inside the window in either glyph set.
    let (queue, _) = queued(&FIVE);
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let style = Style::drawn(glyphs);
        for columns in 0..=MARKED {
            assert!(
                panel(&queue, columns, 40, style).is_empty(),
                "{columns} {glyphs:?}"
            );
        }
        for columns in MARKED + 1..=12 {
            let laid = panel(&queue, columns, 80, style);
            assert!(!laid.is_empty(), "{columns} {glyphs:?}");
            for row in said(&laid) {
                assert!(
                    crucible_tui::columns(&row) <= columns,
                    "{columns} {glyphs:?}: {row:?}"
                );
            }
        }
    }
}

#[test]
fn every_state_of_the_panel_fits_every_window_it_is_given() {
    // The panel's own fit sweep, since it is laid out here and not among the
    // components the crate's sweep walks: fresh, walked into the middle,
    // walked to the end, and saying the box had no room — each at every width
    // and every room it decides anything at, in either glyph set.
    let fresh = queued(&FIVE).0;
    let mut middle = queued(&FIVE).0;
    middle.walk(false);
    middle.walk(false);
    let mut end = queued(&FIVE).0;
    for _ in 0..FIVE.len() {
        end.walk(false);
    }
    let (mut refused, steer) = queued(&FIVE);
    let mut editor = Editor::new();
    assert_eq!(
        editor.paste(&"y".repeat(Editor::MAX_BYTES - 2)),
        Typed::Changed
    );
    refused.take_back(&mut editor, Offer::Turn(&steer));

    for (state, queue) in [
        ("fresh", &fresh),
        ("middle", &middle),
        ("end", &end),
        ("refused", &refused),
    ] {
        let marked = queue.waiting_all().nth(queue.highlighted()).unwrap();
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let style = Style::drawn(glyphs);
            for columns in 1..=200 {
                for room in 0..=24 {
                    let laid = said(&panel(queue, columns, room, style));
                    let at = format!("{state} {glyphs:?} {columns}x{room}");
                    assert!(laid.len() <= room, "{at}: {laid:?}");
                    for row in &laid {
                        assert!(crucible_tui::columns(row) <= columns, "{at}: {row:?}");
                    }
                    if columns >= 12 && !laid.is_empty() {
                        assert!(
                            laid.iter().any(|row| row.contains(marked)),
                            "{at}: the highlighted line is not named in {laid:?}"
                        );
                    }
                }
            }
        }
    }
}
