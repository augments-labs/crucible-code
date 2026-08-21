use crucible_tui::Key;

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

#[test]
fn the_list_names_every_line_and_marks_the_one_the_keys_act_on() {
    // A key's target is never a guess: the mark is drawn in the accent, and the
    // caption says both keys that change anything.
    let (queue, _) = queued(&["first", "second"]);
    let laid = rows(&queue, 1, 40, 10, Style::plain());

    let said: Vec<String> = laid.iter().map(crucible_tui::Row::text).collect();
    assert!(said.first().is_some_and(|row| row.starts_with("2 queued")));
    assert!(
        said.first()
            .is_some_and(|row| row.contains("x to take back")),
        "{said:?}"
    );
    assert!(
        said.get(2).is_some_and(|row| row.ends_with("first")),
        "{said:?}"
    );
    assert!(
        said.get(3).is_some_and(|row| row.ends_with("second")),
        "{said:?}"
    );

    // The mark leads every row, so the row the keys act on is the one carrying a
    // second run of the accent -- its own words.
    let accents = |at: usize| {
        laid.get(at)
            .expect("a row for that line")
            .kinds()
            .filter(|slot| *slot == crucible_tui::Slot::Accent)
            .count()
    };

    assert_eq!(
        accents(2),
        1,
        "the unmarked line was drawn as the marked one"
    );
    assert_eq!(accents(3), 2, "the marked line was drawn as the rest are");
}

#[test]
fn a_window_with_no_room_for_a_name_lays_nothing_out() {
    // Which both callers read as the view closing. Chrome with nothing under it
    // is a frame that took the box away and put nothing in its place.
    let (queue, _) = queued(&["first"]);

    assert!(rows(&queue, 0, 40, 3, Style::plain()).is_empty());
    assert!(!rows(&queue, 0, 40, 4, Style::plain()).is_empty());
}
