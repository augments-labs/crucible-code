use crucible_client_api::{Command, Era, Picked, Replied, Said};
use crucible_tui::{Glyphs, Key, Phase, Pressed};
use crucible_types::{Answer, Batch, Question, QuestionId};

use super::*;

/// The era every case asks in, unless it is about another.
fn era() -> Era {
    Era::Stint(1)
}

/// One question taking a single answer, numbered `number`.
fn single(number: u64) -> Batch {
    Batch::new(
        QuestionId::numbered(number),
        [Question::new(
            "Language",
            "Which language?",
            [Answer::new("Rust"), Answer::new("Python")],
        )],
    )
}

/// One question taking several answers.
fn several() -> Batch {
    Batch::new(
        QuestionId::numbered(1),
        [Question::new(
            "Support",
            "Which of these?",
            [Answer::new("Images"), Answer::new("PDFs")],
        )
        .several()],
    )
}

/// Two questions, each taking a single answer.
fn pair() -> Batch {
    Batch::new(
        QuestionId::numbered(1),
        [
            Question::new(
                "Language",
                "Which language?",
                [Answer::new("Rust"), Answer::new("Python")],
            ),
            Question::new(
                "Shell",
                "Which shell?",
                [Answer::new("bash"), Answer::new("zsh")],
            ),
        ],
    )
}

/// A dock showing `batch`, opened and drawn once, as `/questions` leaves it.
fn opened(batch: Batch) -> Docking {
    let mut docking = Docking::default();
    docking.waiting(era(), &[batch]);
    assert_eq!(docking.toggled(), Toggled::Opened);
    let _ = docking.rows(80, 40, Glyphs::default());
    docking
}

/// Presses each of `keys` in turn, keeping what the last one did.
fn press(docking: &mut Docking, keys: &[Pressed]) -> Keyed {
    let mut last = Keyed::Passed;
    for key in keys {
        last = docking.key(key);
    }
    last
}

/// Types `text` a character at a time.
fn typed(text: &str) -> Vec<Pressed> {
    text.chars()
        .map(|typed| Pressed::Key(Key::Char(typed)))
        .collect()
}

fn enter() -> Pressed {
    Pressed::Key(Key::Enter)
}

/// What a submission asks for: the replies, where it asked for an answer.
fn replies(keyed: Keyed) -> Vec<Replied> {
    match keyed {
        Keyed::Submit(Command::Answer {
            era: asked,
            id,
            replies,
        }) => {
            assert_eq!(asked, era());
            assert_eq!(id, QuestionId::numbered(1));
            replies
        }
        other => panic!("expected a submission, got {other:?}"),
    }
}

fn said(text: &str) -> Said {
    Said::new(text).expect("short enough")
}

fn answered(chosen: &[&str], note: &str) -> Replied {
    Replied::Answered(Picked {
        chosen: chosen.iter().map(|one| said(one)).collect(),
        note: said(note),
    })
}

#[test]
fn a_batch_waiting_shows_the_closed_row_and_the_dock_does_not_open() {
    let mut docking = Docking::default();
    docking.waiting(era(), &[single(1)]);

    let (rows, caret) = docking.rows(80, 40, Glyphs::default());

    assert!(!docking.is_open());
    assert!(!docking.has_focus());
    assert_eq!(rows.len(), 1, "one closed row: {rows:?}");
    assert!(caret.is_none());
}

#[test]
fn the_command_opens_and_closes_the_dock_and_says_when_nothing_waits() {
    let mut docking = Docking::default();
    assert_eq!(docking.toggled(), Toggled::Empty);

    docking.waiting(era(), &[single(1)]);
    assert_eq!(docking.toggled(), Toggled::Opened);
    assert!(docking.is_open() && docking.has_focus());
    assert!(docking.rows(80, 40, Glyphs::default()).0.len() > 1);

    assert_eq!(docking.toggled(), Toggled::Closed);
    assert!(!docking.is_open() && !docking.has_focus());
}

#[test]
fn escape_closes_the_dock_with_the_draft_kept_and_reopening_shows_it() {
    let mut docking = opened(pair());
    press(&mut docking, &[enter()]);
    assert_eq!(press(&mut docking, &[Pressed::Escape]), Keyed::Closed);
    assert!(!docking.is_open());

    assert_eq!(docking.toggled(), Toggled::Opened);
    let shown = press(&mut docking, &[Pressed::Key(Key::Left)]);
    assert_eq!(shown, Keyed::Moved);
    press(&mut docking, &[Pressed::Key(Key::Right), enter()]);
    let sent = replies(press(&mut docking, &[enter()]));
    assert_eq!(sent, vec![answered(&["Rust"], ""), answered(&["bash"], "")]);
}

#[test]
fn escape_in_the_editor_leaves_the_editor_only_and_keeps_the_line() {
    let mut docking = opened(single(1));
    press(&mut docking, &[Pressed::Down, Pressed::Down, enter()]);
    press(&mut docking, &typed("Go"));
    assert!(
        docking.rows(80, 40, Glyphs::default()).1.is_some(),
        "a caret while writing"
    );

    assert_eq!(press(&mut docking, &[Pressed::Escape]), Keyed::Moved);
    assert!(docking.is_open() && docking.has_focus());
    assert!(docking.rows(80, 40, Glyphs::default()).1.is_none());

    let sent = replies(press(&mut docking, &[enter(), enter()]));
    assert_eq!(sent, vec![answered(&["Go"], "")]);
}

#[test]
fn a_decline_asks_once_and_is_sent_alone_though_the_member_had_a_draft() {
    let mut docking = opened(single(1));
    press(&mut docking, &[Pressed::Key(Key::Char('n'))]);
    press(&mut docking, &typed("why"));
    press(
        &mut docking,
        &[Pressed::Escape, Pressed::Down, Pressed::Down, Pressed::Down],
    );

    press(&mut docking, &[enter()]);
    assert_eq!(
        docking.dock.phase,
        Phase::Answering,
        "asked to confirm first"
    );
    press(&mut docking, &[enter()]);
    assert_eq!(docking.dock.phase, Phase::Reviewing);

    let sent = replies(press(&mut docking, &[enter()]));
    assert_eq!(sent, vec![Replied::Declined]);
}

#[test]
fn a_decline_asks_its_second_press_in_a_room_too_short_for_the_row_of_keys() {
    let mut docking = opened(single(1));
    press(&mut docking, &[Pressed::Down, Pressed::Down, Pressed::Down]);
    press(&mut docking, &[enter()]);
    let whole = docking.rows(80, 40, Glyphs::default()).0.len();

    let (rows, _) = docking.rows(80, whole - 1, Glyphs::default());
    let drawn: Vec<String> = rows.iter().map(Row::text).collect();
    assert!(rows.len() < whole, "{drawn:#?}");
    assert!(
        drawn.iter().any(|row| row.contains(CONFIRMING)),
        "the second press is asked for\n{drawn:#?}"
    );
}

#[test]
fn undoing_a_decline_before_submitting_restores_the_draft() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);
    // Keep editing, then decline.
    press(&mut docking, &[Pressed::Down, enter()]);
    press(
        &mut docking,
        &[
            Pressed::Down,
            Pressed::Down,
            Pressed::Down,
            enter(),
            enter(),
        ],
    );
    assert_eq!(docking.dock.phase, Phase::Reviewing);

    // Keep editing again, and undo it on the row that declined it.
    press(&mut docking, &[Pressed::Down, enter()]);
    press(
        &mut docking,
        &[Pressed::Down, Pressed::Down, Pressed::Down, enter()],
    );
    press(
        &mut docking,
        &[Pressed::Up, Pressed::Up, Pressed::Up, enter()],
    );

    let sent = replies(press(&mut docking, &[enter()]));
    assert_eq!(sent, vec![answered(&["Rust"], "")]);
}

#[test]
fn a_several_answer_question_takes_space_a_written_answer_last_and_a_note() {
    let mut docking = opened(several());
    press(&mut docking, &[Pressed::Key(Key::Char(' '))]);
    press(&mut docking, &[Pressed::Down, Pressed::Down, enter()]);
    press(&mut docking, &typed("Video"));
    press(&mut docking, &[enter(), Pressed::Key(Key::Char('n'))]);
    press(&mut docking, &typed("soon"));
    press(&mut docking, &[Pressed::Escape, enter()]);
    assert_eq!(docking.dock.phase, Phase::Reviewing);

    let sent = replies(press(&mut docking, &[enter()]));
    assert_eq!(sent, vec![answered(&["Images", "Video"], "soon")]);
}

#[test]
fn a_second_submit_while_one_is_in_flight_does_nothing() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);
    let _ = replies(press(&mut docking, &[enter()]));
    assert_eq!(docking.dock.phase, Phase::Submitting);

    let again = press(&mut docking, &[enter()]);
    assert!(!matches!(again, Keyed::Submit(_)), "{again:?}");
}

#[test]
fn a_refused_submission_keeps_the_draft_and_offers_a_retry() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);
    let _ = replies(press(&mut docking, &[enter()]));

    docking.answered(
        QuestionId::numbered(1),
        Err("the questions moved on".to_owned()),
    );
    assert_eq!(
        docking.dock.phase,
        Phase::Refused("the questions moved on".to_owned())
    );
    assert_eq!(
        docking.dock.marked_action(),
        Some(crucible_tui::Action::Retry)
    );

    let sent = replies(press(&mut docking, &[enter()]));
    assert_eq!(sent, vec![answered(&["Rust"], "")]);
}

#[test]
fn an_accepted_batch_shows_accepted_until_a_listing_leaves_it_out() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);
    let _ = replies(press(&mut docking, &[enter()]));

    docking.answered(QuestionId::numbered(1), Ok(()));
    assert_eq!(docking.dock.phase, Phase::Accepted);
    assert!(docking.rows(80, 40, Glyphs::default()).0.len() > 1);

    docking.waiting(era(), &[]);
    assert!(!docking.is_open() && !docking.has_focus());
    assert!(docking.rows(80, 40, Glyphs::default()).0.is_empty());
}

#[test]
fn a_listing_keeps_the_draft_of_a_batch_with_the_same_era_and_id() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);
    docking.waiting(era(), &[single(1), single(2)]);

    assert_eq!(
        docking.dock.phase,
        Phase::Reviewing,
        "progress moved nothing"
    );
    let sent = replies(press(&mut docking, &[enter()]));
    assert_eq!(sent, vec![answered(&["Rust"], "")]);
}

#[test]
fn a_listing_of_another_era_drops_every_draft() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);
    docking.waiting(Era::Stint(2), &[single(1)]);

    assert!(!docking.is_open());
    assert_eq!(docking.toggled(), Toggled::Opened);
    assert_eq!(docking.dock.phase, Phase::Answering);
    assert!(!docking.dock.batches.iter().any(|batch| {
        batch
            .members
            .iter()
            .any(|member| member.draft.chosen.takes(0))
    }));
}

#[test]
fn moving_to_another_era_drops_every_batch_and_the_same_era_keeps_them() {
    let mut docking = opened(single(1));
    press(&mut docking, &[enter()]);

    docking.moved(&era());
    assert!(docking.is_open());
    assert_eq!(docking.dock.batches.len(), 1);

    docking.moved(&Era::Stint(2));
    assert!(!docking.is_open());
    assert!(docking.dock.batches.is_empty());
    assert_eq!(docking.dock.scrolled, crucible_tui::Scrolled::default());
    assert_eq!(docking.toggled(), Toggled::Empty);
}

#[test]
fn tab_reaches_the_composer_and_back_with_the_draft_intact() {
    let mut docking = opened(single(1));
    press(&mut docking, &[Pressed::Down, Pressed::Down, enter()]);
    press(&mut docking, &typed("Go"));

    assert_eq!(press(&mut docking, &[Pressed::Tab]), Keyed::Composer);
    assert!(docking.is_open() && !docking.has_focus());
    assert!(docking.rows(80, 40, Glyphs::default()).1.is_none());

    assert!(docking.focused());
    let sent = replies(press(&mut docking, &[enter(), enter()]));
    assert_eq!(sent, vec![answered(&["Go"], "")]);
}

#[test]
fn the_keys_that_are_not_the_docks_pass_through() {
    let mut docking = opened(single(1));
    for passed in [
        Pressed::Key(Key::Interrupt),
        Pressed::Resized,
        Pressed::Expand,
    ] {
        assert_eq!(docking.key(&passed), Keyed::Passed, "{passed:?}");
    }
}

#[test]
fn page_keys_scroll_a_note_taller_than_the_window_and_come_back() {
    let mut docking = opened(single(1));
    press(&mut docking, &[Pressed::Key(Key::Char('n'))]);
    press(&mut docking, &typed(&"a long note ".repeat(60)));
    press(&mut docking, &[Pressed::Escape]);

    let top = docking.rows(30, 14, Glyphs::default()).0;
    press(&mut docking, &[Pressed::PageDown]);
    let down = docking.rows(30, 14, Glyphs::default()).0;
    assert_ne!(top, down, "the window moved");

    press(
        &mut docking,
        &[Pressed::PageUp, Pressed::PageUp, Pressed::PageUp],
    );
    assert_eq!(docking.rows(30, 14, Glyphs::default()).0, top);
}

#[test]
fn moving_to_the_next_question_opens_it_with_the_mark_at_its_top() {
    let mut docking = opened(pair());
    press(&mut docking, &[Pressed::Down]);
    press(&mut docking, &[Pressed::Key(Key::Right)]);
    assert_eq!(docking.dock.focus.member, 1);
    assert_eq!(docking.dock.focus.marked, 0);
}
