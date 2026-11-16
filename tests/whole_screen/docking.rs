//! The dock: questions the model asked without waiting, standing above the
//! box while the turn goes on.
//!
//! Every case runs in both screen modes, since the dock is drawn in the band
//! over the box in either and is not a screen of its own. What a submission
//! delivered is read from the session log once the process is gone, where the
//! words the model was told are written down.

use std::time::Duration;

use crate::vendor::Vendor;
use crate::watched::Watched;

/// One question asked without waiting, with two answers.
const LATER: &str = r#"{"mode":"deferred","questions":[{"heading":"Language","question":"Which language?","answers":[{"answer":"Rust"},{"answer":"Python"}]}]}"#;

/// The same question, asked the way that waits for it.
const NOW: &str = r#"{"questions":[{"heading":"Language","question":"Which language?","answers":[{"answer":"Rust"},{"answer":"Python"}]}]}"#;

/// A question taking several answers, then one with a line of the reader's own.
const MIXED: &str = r#"{"mode":"deferred","questions":[{"heading":"Checks","question":"Which checks?","several":true,"answers":[{"answer":"lint"},{"answer":"test"},{"answer":"build"}]},{"heading":"Language","question":"Which language?","answers":[{"answer":"Rust"},{"answer":"Python"}]}]}"#;

/// Four questions, each to be given a note longer than the window.
const FOUR: &str = r#"{"mode":"deferred","questions":[{"heading":"One","question":"First?","answers":[{"answer":"Yes"},{"answer":"No"}]},{"heading":"Two","question":"Second?","answers":[{"answer":"Yes"},{"answer":"No"}]},{"heading":"Three","question":"Third?","answers":[{"answer":"Yes"},{"answer":"No"}]},{"heading":"Four","question":"Fourth?","answers":[{"answer":"Yes"},{"answer":"No"}]}]}"#;

/// A question with as many answers as one may have, more than a short window
/// holds, then a second.
const MANY: &str = r#"{"mode":"deferred","questions":[{"heading":"Pick","question":"Which one?","answers":[{"answer":"Alpha"},{"answer":"Bravo"},{"answer":"Charlie"},{"answer":"Delta"},{"answer":"Echo"},{"answer":"Foxtrot"},{"answer":"Golf"},{"answer":"Hotel"}]},{"heading":"Next","question":"And then?","answers":[{"answer":"Stop"},{"answer":"Go"}]}]}"#;

/// What the held turn says once the question is asked.
const HELD: &str = "Asked. Carrying on.";

/// What a turn that ends says once the question is asked.
const ENDED: &str = "Asked.";

/// The closed row a batch of [`LATER`] stands as.
const WAITING: &str = "1 question waiting · Language · /questions";

/// The quiet row under the open dock while a question has the keys.
const CHOOSING: &str = "enter choose · n note · tab composer · esc close";

/// The same, while a line is written.
const WRITING: &str = "enter or esc done · tab composer";

/// The same, while the composer has the keys.
const ELSEWHERE: &str = "tab questions";

/// The same, while the answers are read back.
const ACTING: &str = "enter select · tab composer · esc close";

/// What the model is told when an answer reaches it.
const DELIVERED: &str = "The person answered question batch";

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const RIGHT: &str = "\x1b[C";
const ESCAPE: &str = "\x1b";
const PAGE_UP: &str = "\x1b[5~";
const PAGE_DOWN: &str = "\x1b[6~";

/// Both screen modes, by the name a capture carries.
const MODES: [(bool, &str); 2] = [(false, "fullscreen"), (true, "native")];

/// A window answered by `vendor`, on the alternate screen or in the
/// terminal's own buffer.
fn opened(case: &str, native: bool, size: (u16, u16), vendor: &Vendor) -> Watched {
    let (columns, rows) = size;
    if native {
        Watched::native(case, columns, rows, vendor)
    } else {
        Watched::answering(case, columns, rows, vendor)
    }
}

/// The screen with the running turn's spinner, verb and clock masked, so a
/// capture does not move with how long the steps before it took.
fn still(window: &Watched) -> String {
    window
        .picture()
        .lines()
        .map(masked)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One row of [`still`], the turn's own row given a face that never changes
/// and kept as wide as it was.
fn masked(row: &str) -> String {
    let (Some(open), Some(close)) = (row.find('('), row.find(')')) else {
        return row.to_owned();
    };
    if !row.contains("esc to interrupt") || close < open {
        return row.to_owned();
    }
    let inside = row.get(open + 1..close).unwrap_or_default();
    let clock = inside.trim_start_matches(|c: char| c.is_ascii_digit());
    let head = format!("|* running (#{clock})");
    let tail = row.get(close + 1..).unwrap_or_default().trim_start();
    let pad = row
        .chars()
        .count()
        .saturating_sub(head.chars().count() + tail.chars().count());
    format!("{head}{}{tail}", " ".repeat(pad))
}

/// Whether the dock stands open, its count heading the box it is drawn in.
fn open(picture: &str) -> bool {
    picture
        .lines()
        .any(|line| line.starts_with("|│  □ ") && line.contains(" waiting"))
}

/// Whether the open dock stands with no line being written in it.
fn choosing(picture: &str) -> bool {
    open(picture) && !picture.contains(WRITING)
}

/// Whether a note is being written: its quiet row says so, and where a short
/// band has given that row up, the empty line the editor opens does.
fn writing(picture: &str) -> bool {
    picture.contains(WRITING)
        || picture.lines().any(|line| {
            line.trim_end_matches(['|', '│', ' '])
                .ends_with("│    Note:")
        })
}

/// Types `keys` and waits for the open dock to have the keys again.
///
/// Its quiet row goes once a short band has no blank rows left, so a screen can
/// look the same before and after: an escape is let settle, so that the key
/// after it is not read as one with it.
fn chooses(window: &mut Watched, keys: &str) {
    window.types_and_catches_where(keys, "the open dock choosing", choosing);
    if keys == ESCAPE {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Types `keys` and waits for a line of the dock to be written.
fn writes(window: &mut Watched, keys: &str) {
    window.types_and_catches_where(keys, "a line of the dock being written", writing);
}

/// Types `keys` without waiting for the screen to go still, for a key under
/// a running turn, whose working row never lets it: the step after is what
/// waits for what the key drew.
fn presses(window: &mut Watched, keys: &str) {
    window.types_and_catches_where(keys, "a frame", |_| true);
}

/// The rows a reader can see and scroll back to: in native mode an open dock
/// pushes what the turn said off the window, into the scrollback.
fn read_back(picture: &str, scrollback: &str) -> String {
    format!("{scrollback}\n{picture}")
}

/// Waits for the turn on screen to be over.
fn idle(window: &mut Watched, step: &str) {
    window.catches_where(step, "a screen with no turn running", |picture| {
        !picture.contains("esc to interrupt")
    });
}

/// Waits for the dock to have nothing left in it.
fn emptied(window: &mut Watched, step: &str) {
    window.catches_where(step, "a screen with nothing waiting", |picture| {
        !picture.contains("question waiting") && !picture.contains("questions waiting")
    });
}

/// Waits for the session log to hold `said` `times` over, as it does once
/// what wrote it there has run.
fn logged(window: &Watched, said: &str, times: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while window.recorded().matches(said).count() < times {
        assert!(
            std::time::Instant::now() < deadline,
            "the log never held {said:?} {times} times\n{}",
            window.recorded()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits for the answers the dock sent to have reached the model, and then
/// for the dock to have nothing left in it and no turn to be running.
fn delivered(window: &mut Watched, step: &str) {
    logged(window, DELIVERED, 1);
    window.catches_where(
        step,
        "the answers delivered and the dock empty",
        |picture| {
            !picture.contains("question waiting")
                && !picture.contains("questions waiting")
                && !picture.contains("Review before you submit")
                && !picture.contains("Answers accepted")
                && !picture.contains("esc to interrupt")
        },
    );
}

/// A note `words` long, every word the same.
fn long_note(words: usize) -> String {
    "steady ".repeat(words).trim_end().to_owned()
}

/// What every session log of `window` holds, once it has ended.
fn ended_log(window: &mut Watched) -> String {
    window.ends_on("TERM");
    window.recorded()
}

#[test]
fn a_deferred_batch_shows_the_closed_row_while_the_turn_draws_on() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-row-{mode}"), native, (80, 24), &vendor);

        window.types_and_catches("go\r", WAITING);
        window.never_draws(
            "the question was asked",
            ESCAPE_CLOSE,
            Duration::from_millis(600),
        );

        let picture = window.picture();
        assert!(
            picture.contains(HELD),
            "{mode}: the turn draws on\n{picture}"
        );
        assert!(
            picture.contains("esc to interrupt"),
            "{mode}: the turn is still running\n{picture}"
        );
        insta::assert_snapshot!(format!("closed_row_{mode}"), still(&window));
        vendor.ends_the_turn();
    }
}

/// What the open dock's quiet row ends in, whichever keys it names.
const ESCAPE_CLOSE: &str = "esc close";

#[test]
fn questions_opens_the_dock_and_tab_walks_to_the_composer_and_back_with_drafts_kept() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-tab-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);

        chooses(&mut window, "/questions\r");
        insta::assert_snapshot!(format!("opened_{mode}"), still(&window));
        writes(&mut window, "n");
        window.types_and_catches("a note kept", "a note kept");
        chooses(&mut window, ESCAPE);

        // The band may give up the quiet row once the note has made the dock
        // taller and its blank rows are gone, so where the keys went is read
        // from what they did: a line in the box, then the mark moved.
        presses(&mut window, "\t");
        window.types_and_catches("half a line", "│ › half a line");
        presses(&mut window, "\t");
        window.types_and_catches(DOWN, "› 2. ( ) Python");

        let picture = window.picture();
        assert!(
            picture.contains("a note kept"),
            "{mode}: the note stays\n{picture}"
        );
        assert!(
            picture.contains("half a line"),
            "{mode}: the composer's line stays\n{picture}"
        );
        vendor.ends_the_turn();
    }
}

#[test]
fn a_line_typed_beside_the_open_dock_steers_the_turn_and_answers_nothing() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-steer-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        chooses(&mut window, "/questions\r");

        window.types_and_catches("\t", ELSEWHERE);
        window.types_and_catches("use the other one\r", "1 queued");
        let picture = window.picture();
        assert!(
            open(&picture) && !picture.contains("accepted"),
            "{mode}: the line answered nothing\n{picture}"
        );

        // The line steers the turn: it yields, and the line is the next
        // request's. That turn's last frame waits for the next thing drawn,
        // so the log says when it has run and a key is what shows it.
        vendor.ends_the_turn();
        logged(&window, HELD, 2);
        window.types_and_catches("x", "│ › x");
        let picture = window.picture();
        assert!(
            open(&picture)
                && read_back(&picture, &window.scrollback()).contains(HELD)
                && !picture.contains("esc to interrupt"),
            "{mode}: the question still waits once the line was taken\n{picture}"
        );
        let log = ended_log(&mut window);
        assert!(
            log.contains("use the other one"),
            "{mode}: the line reached the model"
        );
        assert!(
            !log.contains(DELIVERED),
            "{mode}: no answer was sent\n{log}"
        );
    }
}

#[test]
fn a_batch_submitted_under_a_running_turn_is_accepted_and_goes_once_delivered() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding("ask_user", LATER, HELD);
        let mut window = opened(
            &format!("docking-running-{mode}"),
            native,
            (80, 24),
            &vendor,
        );
        window.types_and_catches("go\r", WAITING);
        // The reply streams a word at a time; the frames below are pinned
        // with all of it drawn, not wherever the stream had got to.
        window.catches("the turn drew on", HELD);
        chooses(&mut window, "/questions\r");

        window.types_and_catches("\r", "Review before you submit");
        insta::assert_snapshot!(format!("review_{mode}"), still(&window));
        window.types_and_catches("\r", "Answers accepted");
        insta::assert_snapshot!(format!("accepted_{mode}"), still(&window));

        vendor.ends_the_turn();
        delivered(&mut window, "the turn was ended");
        let log = ended_log(&mut window);
        assert!(
            log.contains(DELIVERED),
            "{mode}: the answer was sent\n{log}"
        );
        assert!(log.contains("- Language: Rust"), "{mode}\n{log}");
    }
}

#[test]
fn a_batch_submitted_between_turns_starts_one() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling("ask_user", LATER, ENDED);
        let mut window = opened(&format!("docking-idle-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        idle(&mut window, "the question was asked");

        chooses(&mut window, "/questions\r");
        window.types_and_catches("\r", "Review before you submit");
        presses(&mut window, "\r");
        delivered(&mut window, "the batch was submitted");
        let seen = read_back(&window.picture(), &window.scrollback());
        assert!(
            seen.matches(ENDED).count() >= 2,
            "{mode}: the answer started a turn\n{seen}"
        );

        let log = ended_log(&mut window);
        assert!(
            log.contains(DELIVERED),
            "{mode}: the answer was sent\n{log}"
        );
        assert!(log.contains("- Language: Rust"), "{mode}\n{log}");
    }
}

#[test]
fn escape_leaves_the_editor_then_closes_the_dock_and_neither_declines() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-escape-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        chooses(&mut window, "/questions\r");

        writes(&mut window, "n");
        window.types_and_catches("keep me", "keep me");
        chooses(&mut window, ESCAPE);
        window.types_and_catches(ESCAPE, WAITING);
        let picture = window.picture();
        assert!(
            !picture.contains(ESCAPE_CLOSE),
            "{mode}: the dock closed\n{picture}"
        );

        chooses(&mut window, "/questions\r");
        let picture = window.picture();
        assert!(
            picture.contains("keep me"),
            "{mode}: the note stays\n{picture}"
        );
        assert!(!picture.contains("Declined"), "{mode}\n{picture}");
        insta::assert_snapshot!(format!("reopened_{mode}"), still(&window));

        window.types_and_catches("\r", "Review before you submit");
        window.types_and_catches("\r", "Answers accepted");
        vendor.ends_the_turn();
        delivered(&mut window, "the turn was ended");
        let log = ended_log(&mut window);
        assert!(log.contains("- Language: Rust"), "{mode}\n{log}");
        assert!(log.contains("they added: keep me"), "{mode}\n{log}");
    }
}

#[test]
fn a_declined_question_is_sent_declined_alone_though_it_had_a_draft() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(
            &format!("docking-decline-{mode}"),
            native,
            (80, 24),
            &vendor,
        );
        window.types_and_catches("go\r", WAITING);
        chooses(&mut window, "/questions\r");
        writes(&mut window, "n");
        window.types_and_catches("a reason", "a reason");
        chooses(&mut window, ESCAPE);
        window.types_and_catches("\r", "Review before you submit");
        window.types_and_catches(DOWN, "› Keep editing");
        chooses(&mut window, "\r");

        presses(&mut window, DOWN);
        presses(&mut window, DOWN);
        window.types_and_catches(DOWN, "› 4.");
        window.types_and_catches("\r", "enter to decline this question");
        window.types_and_catches("\r", "Declined");
        insta::assert_snapshot!(format!("declined_{mode}"), still(&window));

        window.types_and_catches("\r", "Answers accepted");
        vendor.ends_the_turn();
        delivered(&mut window, "the turn was ended");
        let log = ended_log(&mut window);
        assert!(log.contains("- Language: unresolved"), "{mode}\n{log}");
        assert!(!log.contains("- Language: Rust"), "{mode}\n{log}");
        assert!(!log.contains("they added"), "{mode}\n{log}");
    }
}

#[test]
fn undoing_a_decline_before_submitting_brings_the_draft_back() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-undo-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        chooses(&mut window, "/questions\r");
        writes(&mut window, "n");
        window.types_and_catches("a reason", "a reason");
        chooses(&mut window, ESCAPE);
        presses(&mut window, DOWN);
        presses(&mut window, DOWN);
        window.types_and_catches(DOWN, "› 4.");
        window.types_and_catches("\r", "enter to decline this question");
        window.types_and_catches("\r", "Declined");

        window.types_and_catches(DOWN, "› Keep editing");
        chooses(&mut window, "\r");
        presses(&mut window, DOWN);
        presses(&mut window, DOWN);
        window.types_and_catches(DOWN, "› 4.");
        presses(&mut window, "\r");
        window.never_draws(
            "the decline was undone",
            "enter to decline this question",
            Duration::from_millis(400),
        );
        presses(&mut window, UP);
        presses(&mut window, UP);
        window.types_and_catches(UP, "› 1.");
        window.types_and_catches("\r", "Review before you submit");
        let picture = window.picture();
        assert!(!picture.contains("Declined"), "{mode}\n{picture}");
        assert!(
            picture.contains("a reason"),
            "{mode}: the note came back\n{picture}"
        );

        window.types_and_catches("\r", "Answers accepted");
        vendor.ends_the_turn();
        delivered(&mut window, "the turn was ended");
        let log = ended_log(&mut window);
        assert!(log.contains("- Language: Rust"), "{mode}\n{log}");
        assert!(log.contains("they added: a reason"), "{mode}\n{log}");
        assert!(!log.contains("unresolved"), "{mode}\n{log}");
    }
}

#[test]
fn a_note_taller_than_the_window_pages_with_the_question_in_reach() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-tall-{mode}"), native, (60, 18), &vendor);
        window.types_and_catches("go\r", WAITING);
        chooses(&mut window, "/questions\r");
        writes(&mut window, "n");
        presses(&mut window, "start ");
        presses(&mut window, &long_note(80));
        window.types_and_catches(" finish", "finish");
        chooses(&mut window, ESCAPE);

        window.types_and_catches_where(PAGE_UP, "the question's top", |picture| {
            picture.contains("› □ Language")
        });
        insta::assert_snapshot!(format!("tall_top_{mode}"), still(&window));
        window.types_and_catches_where(PAGE_DOWN, "the window a page down", |picture| {
            !picture.contains("› □ Language")
        });
        window.types_and_catches_where(PAGE_UP, "the question's top again", |picture| {
            picture.contains("› □ Language")
        });
        vendor.ends_the_turn();
    }
}

#[test]
fn four_long_notes_page_the_review_to_submit_batch() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", FOUR, HELD);
        let mut window = opened(&format!("docking-notes-{mode}"), native, (60, 24), &vendor);
        window.types_and_catches("go\r", "4 questions waiting");
        chooses(&mut window, "/questions\r");
        for heading in ["One", "Two", "Three", "Four"] {
            writes(&mut window, "n");
            presses(&mut window, &format!("{heading} "));
            presses(&mut window, &long_note(30));
            window.types_and_catches(" over", "over");
            chooses(&mut window, "\r");
            presses(&mut window, "\r");
        }
        window.catches("the last question was answered", "Review before you submit");
        let picture = window.picture();
        assert!(
            picture.contains("✓ One") && !picture.contains("✓ Four"),
            "{mode}: the review opens at its top\n{picture}"
        );
        assert!(
            picture.contains("› Submit batch"),
            "{mode}: the actions stand under the review's window\n{picture}"
        );

        // More pages than the review is tall: the last ones go nowhere.
        for _ in 0..6 {
            presses(&mut window, PAGE_DOWN);
        }
        window.catches_where("paging down", "the last note", |picture| {
            picture.contains("Four steady")
        });
        let picture = window.picture();
        assert!(picture.contains("› Submit batch"), "{mode}\n{picture}");
        window.types_and_catches("\r", "Answers accepted");
        vendor.ends_the_turn();
        delivered(&mut window, "the turn was ended");
        let log = ended_log(&mut window);
        assert!(log.contains("they added: Four steady"), "{mode}\n{log}");
    }
}

#[test]
fn an_arrow_past_the_window_brings_the_mark_back_and_a_new_question_opens_at_its_top() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", MANY, HELD);
        let mut window = opened(&format!("docking-mark-{mode}"), native, (60, 16), &vendor);
        window.types_and_catches("go\r", "2 questions waiting");
        chooses(&mut window, "/questions\r");
        for _ in 0..6 {
            presses(&mut window, DOWN);
        }
        window.types_and_catches(DOWN, "›  8. ( ) Hotel");
        insta::assert_snapshot!(format!("marked_{mode}"), still(&window));

        window.types_and_catches(RIGHT, "And then?");
        let picture = window.picture();
        assert!(
            picture.contains("› 1. ( ) Stop"),
            "{mode}: the next question opens at its top\n{picture}"
        );
        window.types_and_catches("\r", "Review before you submit");
        vendor.ends_the_turn();
    }
}

#[test]
fn several_answers_a_written_answer_and_a_note_are_sent_together() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding_long("ask_user", MIXED, HELD);
        let mut window = opened(&format!("docking-mixed-{mode}"), native, (80, 30), &vendor);
        window.types_and_catches("go\r", "2 questions waiting");
        window.types_and_catches("/questions\r", "space toggle");
        window.types_and_catches(" ", "[✓] lint");
        presses(&mut window, DOWN);
        window.types_and_catches(" ", "[✓] test");
        window.types_and_catches("\r", "Which language?");

        presses(&mut window, DOWN);
        window.types_and_catches(DOWN, "› 3.");
        presses(&mut window, "\r");
        window.types_and_catches("Zig", "Zig");
        chooses(&mut window, "\r");
        writes(&mut window, "n");
        window.types_and_catches("it is small", "it is small");
        chooses(&mut window, "\r");
        window.types_and_catches("\r", "Review before you submit");
        insta::assert_snapshot!(format!("mixed_review_{mode}"), still(&window));

        window.types_and_catches("\r", "Answers accepted");
        vendor.ends_the_turn();
        delivered(&mut window, "the turn was ended");
        let log = ended_log(&mut window);
        assert!(log.contains("- Checks: lint, test"), "{mode}\n{log}");
        assert!(log.contains("their own answer: Zig"), "{mode}\n{log}");
        assert!(log.contains("they added: it is small"), "{mode}\n{log}");
    }
}

#[test]
fn clearing_the_session_empties_the_dock() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling("ask_user", LATER, ENDED);
        let mut window = opened(&format!("docking-clear-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        idle(&mut window, "the question was asked");

        window.types("/clear\r");
        emptied(&mut window, "the session was cleared");
        window.types_and_catches("/questions\r", "no questions are waiting");
    }
}

/// A question asked without waiting, in a session a new process resumes, is
/// not docked when the session is: nothing is drawn of it until the first turn
/// reports what waits.
#[test]
fn a_resumed_session_s_waiting_question_docks_once_its_first_turn_reports_it() {
    for (native, mode) in MODES {
        let case = format!("docking-resumed-{mode}");
        let vendor = Vendor::calling("ask_user", LATER, ENDED);
        let mut first = opened(&case, native, (80, 24), &vendor);
        first.types_and_catches("go\r", WAITING);
        idle(&mut first, "the question was asked");
        first.ends_on("TERM");
        let id = crate::native::recorded_id(&first);

        let mut window = opened(&case, native, (80, 24), &vendor);
        window.types_until(&format!("/resume {id}\r"), ENDED);
        let picture = window.picture();
        assert!(
            !picture.contains("question waiting"),
            "{mode}: nothing is docked before a turn reports it\n{picture}"
        );
        window.types_and_catches("go on\r", WAITING);
    }
}

/// A question that takes one answer, left with none, is refused in the dock
/// with the draft kept and `Retry submission` offered; retried as it stands it
/// is refused again, and once an answer is chosen it is accepted.
#[test]
fn a_refused_submission_keeps_the_draft_and_offers_retry() {
    for (native, mode) in MODES {
        // The first question takes one answer and is left with none, only a
        // note: the conversation refuses the batch, and nothing is sent.
        let vendor = Vendor::calling("ask_user", MANY, ENDED);
        let mut window = opened(
            &format!("docking-refused-{mode}"),
            native,
            (80, 30),
            &vendor,
        );
        window.types_and_catches("go\r", "2 questions waiting");
        idle(&mut window, "the questions were asked");

        chooses(&mut window, "/questions\r");
        writes(&mut window, "n");
        window.types_and_catches("still here", "still here");
        chooses(&mut window, ESCAPE);
        window.types_and_catches(RIGHT, "And then?");
        window.types_and_catches("\r", "Review before you submit");
        window.types_and_catches("\r", "Retry submission");
        let picture = window.picture();
        assert!(picture.contains("Answer not accepted"), "{mode}\n{picture}");
        assert!(picture.contains("takes one answer"), "{mode}\n{picture}");
        assert!(picture.contains("Keep editing"), "{mode}\n{picture}");
        assert!(
            picture.contains("still here"),
            "{mode}: the draft stays\n{picture}"
        );
        insta::assert_snapshot!(format!("refused_{mode}"), still(&window));

        // Retried as it stands, it is refused again, the draft still kept.
        window.types_and_catches("\r", "Retry submission");
        assert!(window.picture().contains("still here"), "{mode}");
        presses(&mut window, DOWN);
        window.types_and_catches("\r", "Which one?");
        window.types_and_catches("\r", "And then?");
        window.types_and_catches("\r", "Review before you submit");
        presses(&mut window, "\r");
        delivered(&mut window, "the batch was submitted again");
        let log = ended_log(&mut window);
        assert!(
            log.contains(DELIVERED),
            "{mode}: the answer was sent\n{log}"
        );
        assert!(log.contains("- Pick: Alpha"), "{mode}\n{log}");
        assert!(log.contains("they added: still here"), "{mode}\n{log}");
    }
}

#[test]
fn the_open_dock_fits_a_resized_window_and_one_too_small_for_it() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling("ask_user", LATER, ENDED);
        let mut window = opened(&format!("docking-resize-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        idle(&mut window, "the question was asked");
        chooses(&mut window, "/questions\r");

        window.resize(44, 24);
        window.catches_where("the window narrowed", "the dock at 44 columns", |picture| {
            picture.starts_with("44x24") && picture.contains("Which language?")
        });
        insta::assert_snapshot!(format!("narrow_{mode}"), still(&window));
        window.resize(80, 9);
        window.catches_where("the window shrank", "a window 9 rows tall", |picture| {
            picture.starts_with("80x9")
        });
        insta::assert_snapshot!(format!("cramped_{mode}"), still(&window));
        window.resize(80, 24);
        window.catches_where("the window grew back", "the open dock again", |picture| {
            picture.starts_with("80x24") && choosing(picture) && picture.contains(CHOOSING)
        });
        window.types_and_catches("\r", "Review before you submit");
    }
}

/// The row of `picture` the cursor is on, as drawn, and what is drawn on it
/// before the cursor.
fn at_cursor(picture: &str) -> (String, String) {
    let mut lines = picture.lines();
    let header = lines.next().unwrap_or_default();
    let (row, column) = header
        .split_whitespace()
        .skip_while(|word| *word != "cursor")
        .nth(1)
        .and_then(|at| at.split_once(','))
        .and_then(|(row, column)| Some((row.parse::<usize>().ok()?, column.parse::<usize>().ok()?)))
        .unwrap_or_else(|| panic!("no cursor in the picture\n{picture}"));
    let drawn = lines.nth(row).unwrap_or_default();
    // The frame's own first column stands before the window's first.
    let before = drawn.chars().skip(1).take(column).collect();
    (drawn.to_owned(), before)
}

/// Fails unless the cursor stands at the end of `written`, on a row of the
/// dock.
fn writes_at_the_cursor(window: &Watched, mode: &str, written: &str) {
    let picture = window.picture();
    let (row, before) = at_cursor(&picture);
    assert!(
        before.ends_with(written) && !row.contains("│ ›"),
        "{mode}: the cursor stands where {written:?} is being written\n{picture}"
    );
}

/// Fails unless the cursor stands on the composer's line.
fn composes_at_the_cursor(window: &Watched, mode: &str) {
    let picture = window.picture();
    let (row, _) = at_cursor(&picture);
    assert!(
        row.contains("│ ›"),
        "{mode}: the cursor stands in the composer\n{picture}"
    );
}

#[test]
fn the_cursor_stands_where_a_line_of_the_dock_is_written() {
    for (native, mode) in MODES {
        // Under a running turn.
        let vendor = Vendor::calling_then_holding_long("ask_user", LATER, HELD);
        let mut window = opened(&format!("docking-caret-{mode}"), native, (80, 24), &vendor);
        window.types_and_catches("go\r", WAITING);
        chooses(&mut window, "/questions\r");
        writes(&mut window, "n");
        window.types_and_catches("a note", "a note");
        writes_at_the_cursor(&window, mode, "Note: a note");

        presses(&mut window, "\t");
        window.types_and_catches("x", "│ › x");
        composes_at_the_cursor(&window, mode);
        vendor.ends_the_turn();

        // Between turns, writing an answer of one's own.
        let vendor = Vendor::calling("ask_user", LATER, ENDED);
        let mut window = opened(
            &format!("docking-caret-idle-{mode}"),
            native,
            (80, 24),
            &vendor,
        );
        window.types_and_catches("go\r", WAITING);
        idle(&mut window, "the question was asked");
        chooses(&mut window, "/questions\r");
        presses(&mut window, DOWN);
        window.types_and_catches(DOWN, "› 3.");
        presses(&mut window, "\r");
        window.types_and_catches("Zig", "Zig");
        writes_at_the_cursor(&window, mode, "Zig");
    }
}

#[test]
fn a_blocking_ask_still_stands_where_the_prompt_was() {
    for (native, mode) in MODES {
        let vendor = Vendor::calling_then_holding("ask_user", NOW, HELD);
        let mut window = opened(
            &format!("docking-blocking-{mode}"),
            native,
            (80, 24),
            &vendor,
        );
        window.types_and_catches("go\r", "Python");
        let picture = window.picture();
        assert!(
            !picture.contains("question waiting") && !picture.contains(ACTING),
            "{mode}: a blocking ask is not docked\n{picture}"
        );
        insta::assert_snapshot!(format!("blocking_{mode}"), still(&window));
        vendor.ends_the_turn();
    }
}

#[test]
fn a_run_reading_its_input_from_a_pipe_is_not_offered_the_question_tool() {
    // Its output is a terminal, so there is a screen, but there are no keys to
    // answer from: the dock cannot be reached and a question would only wait.
    let vendor = Vendor::calling("ask_user", LATER, ENDED);
    let log = crate::watched::piped_to_a_terminal("docking-piped", &vendor, b"go\n");

    assert!(log.contains(ENDED), "the turn ran to its end\n{log}");
    assert!(
        log.contains("no tool named ask_user"),
        "the question tool was offered with nobody at the keys\n{log}"
    );
}
