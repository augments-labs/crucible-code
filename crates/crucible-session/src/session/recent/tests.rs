//! Which sessions a directory offers a screen, and which it keeps to itself.

use super::*;
use crate::sample::Sample;

/// A log for this sample's own workspace, holding `prompts` as its messages.
fn planted(sample: &Sample, id: &str, prompts: &[&str]) -> String {
    let mut lines = vec![sample.header(wire::FORMAT, id)];
    lines.extend(
        prompts
            .iter()
            .map(|said| serde_json::json!({ "user": said }).to_string()),
    );

    sample.plant(id, &lines);
    id.to_owned()
}

/// A session identifier that sorts by `nth`, so a test can plant an order.
fn nth(nth: u64) -> String {
    format!("{:013}-0000{nth:02x}", 1_700_000_000_000_u64 + nth)
}

/// What the scan offers for this sample's workspace.
fn offered(sample: &Sample, wanted: usize) -> Vec<Recorded> {
    super::index::ensure(&sample.logs()).expect("the legacy sessions to be indexed");
    recent(&sample.logs(), &sample.workspace(), wanted)
}

/// What the newest of them was asked.
fn first(offered: &[Recorded]) -> &str {
    offered.first().expect("at least one session").asked()
}

#[test]
fn first_frame_does_not_enumerate_an_unindexed_legacy_directory() {
    let sample = Sample::new("recent-unindexed");
    planted(&sample, &nth(1), &["visible after migration"]);

    assert!(recent(&sample.logs(), &sample.workspace(), 4).is_empty());

    super::index::ensure(&sample.logs()).expect("migration after the first frame");
    assert_eq!(
        first(&recent(&sample.logs(), &sample.workspace(), 4)),
        "visible after migration"
    );
}

#[test]
fn a_directory_nobody_has_worked_in_offers_nothing() {
    let sample = Sample::new("recent-none");

    assert!(offered(&sample, 4).is_empty());
}

#[test]
fn a_sessions_directory_that_is_not_there_is_not_a_reason_not_to_start() {
    // The first run on a machine. Nothing has been recorded, so nothing has
    // made the directory, and the screen this feeds is drawn before anything
    // else would have.
    let sample = Sample::new("recent-missing");
    let nowhere = sample.logs().join("never-made");

    assert!(recent(&nowhere, &sample.workspace(), 4).is_empty());
}

#[test]
fn sessions_come_back_newest_first_and_say_what_was_asked() {
    // The order the list is read in, and the only order in which the numbers
    // beside it mean anything.
    let sample = Sample::new("recent-order");
    planted(&sample, &nth(1), &["the oldest thing"]);
    planted(&sample, &nth(2), &["something in between"]);
    planted(&sample, &nth(3), &["the newest thing"]);

    let offered = offered(&sample, 4);

    let asked: Vec<&str> = offered.iter().map(Recorded::asked).collect();
    assert_eq!(
        asked,
        [
            "the newest thing",
            "something in between",
            "the oldest thing"
        ]
    );
}

#[test]
fn a_session_says_when_it_started_without_the_file_being_asked() {
    // The name carries the time, so the ordering above and the date drawn
    // beside each row come from the same thirteen digits.
    let sample = Sample::new("recent-when");
    let id = planted(&sample, &nth(7), &["what time is it"]);

    let offered = offered(&sample, 4);
    let session = offered.first().expect("the one that was planted");

    assert_eq!(session.id().as_str(), id);
    assert_eq!(
        session.started(),
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_007)
    );
}

#[test]
fn another_directorys_sessions_are_not_offered_to_this_one() {
    // A session is bound to the directory it was started in. Offering one from
    // somewhere else would be offering to continue work in a directory the user
    // is not in.
    let sample = Sample::new("recent-elsewhere");
    let id = nth(1);
    let header = serde_json::json!({
        "format": wire::FORMAT,
        "session": id,
        "workspace": sample.elsewhere().root().display().to_string(),
    })
    .to_string();

    sample.plant(
        &id,
        &[header, serde_json::json!({"user": "not here"}).to_string()],
    );

    assert!(offered(&sample, 4).is_empty());
}

#[test]
fn a_log_from_a_build_that_spelled_things_differently_is_left_out() {
    // `--continue` refuses one of these outright, because continuing the wrong
    // session is worse than continuing none. Here the same log is one row that
    // does not appear: a screen drawn before anything was asked for is not
    // somewhere to fail.
    let sample = Sample::new("recent-foreign");
    let id = nth(1);
    sample.plant(
        &id,
        &[
            sample.header(wire::FORMAT + 1, &id),
            serde_json::json!({"user": "from another build"}).to_string(),
        ],
    );

    assert!(offered(&sample, 4).is_empty());
}

#[test]
fn a_session_that_was_never_asked_anything_is_not_a_row() {
    // What every run leaves behind: crucible starts, writes the header, and the
    // user leaves without typing. The heading over the list says "recent
    // sessions", and this is not one.
    let sample = Sample::new("recent-headers");
    planted(&sample, &nth(1), &[]);
    planted(&sample, &nth(2), &["a real one"]);

    let offered = offered(&sample, 4);

    assert_eq!(offered.len(), 1);
    assert_eq!(first(&offered), "a real one");
}

#[test]
fn a_log_that_stopped_inside_its_first_line_is_left_out() {
    // A process killed between opening the file and finishing the header. There
    // is no session in it to name.
    let sample = Sample::new("recent-torn");
    let id = nth(1);
    let half = sample.header(wire::FORMAT, &id);
    std::fs::write(
        sample.logs().join(format!("{id}.jsonl")),
        half.get(..half.len() / 2).unwrap_or_default(),
    )
    .expect("a writable temporary directory");

    assert!(offered(&sample, 4).is_empty());
}

#[test]
fn a_prompt_written_over_several_lines_becomes_one() {
    // The renderer counts the rows it commits so it can move the cursor back
    // over them. A title that is secretly three rows leaves it two rows too
    // high, and the next frame erases something somebody was meant to keep.
    let sample = Sample::new("recent-multiline");
    planted(
        &sample,
        &nth(1),
        &["  find the bug\n\nin the tail\r\nand fix it  "],
    );

    let offered = offered(&sample, 4);

    assert_eq!(first(&offered), "find the bug in the tail and fix it");
}

#[test]
fn nothing_a_prompt_holds_reaches_the_terminal_as_an_instruction() {
    // The text is a user's, read back out of a file, so by the time it is here
    // it is as untrusted as anything else on a disk. An escape sequence in it
    // would be moving the cursor rather than being drawn.
    let sample = Sample::new("recent-escapes");
    planted(&sample, &nth(1), &["clear \x1b[2J this \x07 and \t that"]);

    let asked = first(&offered(&sample, 4)).to_owned();

    assert!(!asked.contains('\x1b'), "{asked:?}");
    assert!(!asked.contains('\x07'), "{asked:?}");
    assert!(!asked.contains('\t'), "{asked:?}");
}

#[test]
fn a_prompt_with_a_file_pasted_into_it_gives_up_its_middle_rather_than_the_start() {
    let sample = Sample::new("recent-huge");
    let pasted = format!("look at this {}", "x".repeat(4 * TITLE));
    planted(&sample, &nth(1), &[&pasted]);

    let asked = first(&offered(&sample, 4)).to_owned();

    assert!(asked.starts_with("look at this x"), "{asked:.40?}");
    assert_eq!(asked.chars().count(), TITLE);
}

#[test]
fn the_scan_stops_once_it_has_what_it_was_asked_for() {
    let sample = Sample::new("recent-wanted");
    for count in 0..8 {
        planted(&sample, &nth(count), &["one of many"]);
    }

    assert_eq!(offered(&sample, 4).len(), 4);
    assert!(offered(&sample, 0).is_empty());
}

#[test]
fn a_directory_full_of_other_peoples_sessions_costs_a_bounded_number_of_reads() {
    // The reason there is a bound at all: this runs before the first frame, and
    // a machine that has held crucible for a year would otherwise pay for every
    // session it ever recorded. What it costs instead is the listing — names,
    // not files — and a fixed number of headers after it.
    //
    // The two sessions that would match sit under more logs than the scan will
    // open, so what is asserted is that it gave up rather than found them.
    let sample = Sample::new("recent-bounded");
    planted(&sample, &nth(0), &["older than the bound reaches"]);
    planted(&sample, &nth(1), &["older than the bound reaches"]);

    for count in 2..u64::try_from(EXAMINED + 2).unwrap_or(u64::MAX) {
        let id = nth(count);
        let header = serde_json::json!({
            "format": wire::FORMAT,
            "session": id,
            "workspace": sample.elsewhere().root().display().to_string(),
        })
        .to_string();
        sample.plant(
            &id,
            &[
                header,
                serde_json::json!({"user": "somewhere else"}).to_string(),
            ],
        );
    }

    assert!(offered(&sample, 4).is_empty());
}

#[test]
fn a_file_that_is_not_a_log_is_not_read_as_one() {
    // The directory is crucible's own, but it is still a directory, and an
    // editor's swap file or a copied-out log sits in it as easily as anywhere.
    let sample = Sample::new("recent-strangers");
    std::fs::write(sample.logs().join("notes.txt"), "not a session\n")
        .expect("a writable temporary directory");
    std::fs::write(sample.logs().join("backup.jsonl"), "{}\n")
        .expect("a writable temporary directory");
    planted(&sample, &nth(1), &["the only real one"]);

    let offered = offered(&sample, 4);

    assert_eq!(offered.len(), 1);
    assert_eq!(first(&offered), "the only real one");
}

#[test]
fn a_workspace_is_matched_whole_rather_than_by_its_start() {
    // `/w/crucible` and `/w/crucible-code` are two directories, and a comparison
    // that read one as the other would offer somebody the wrong project's work.
    let sample = Sample::new("recent-prefix");
    let id = nth(1);
    let header = serde_json::json!({
        "format": wire::FORMAT,
        "session": id,
        "workspace": format!("{}-elsewhere", sample.workspace().root().display()),
    })
    .to_string();

    sample.plant(
        &id,
        &[header, serde_json::json!({"user": "next door"}).to_string()],
    );

    assert!(offered(&sample, 4).is_empty());
}

#[test]
fn the_workspace_a_scan_is_for_is_the_one_it_answers_about() {
    // Two directories, one sessions directory. Which rows appear is the whole
    // difference between them.
    let sample = Sample::new("recent-both");
    planted(&sample, &nth(1), &["work done here"]);

    assert_eq!(offered(&sample, 4).len(), 1);
    assert!(recent(&sample.logs(), &sample.elsewhere(), 4).is_empty());
}

#[test]
fn a_session_says_the_branch_its_header_recorded() {
    let sample = Sample::new("recent-branch");
    let id = nth(1);
    let header = serde_json::json!({
        "format": wire::FORMAT,
        "session": id,
        "workspace": sample.workspace().root().display().to_string(),
        "branch": "feature/picker",
    })
    .to_string();
    sample.plant(
        &id,
        &[
            header,
            serde_json::json!({"user": "on a branch"}).to_string(),
        ],
    );
    planted(&sample, &nth(2), &["nowhere in particular"]);

    let offered = offered(&sample, 4);

    let branches: Vec<Option<&str>> = offered.iter().map(Recorded::branch).collect();
    assert_eq!(branches, [None, Some("feature/picker")]);
}

#[test]
fn the_title_is_the_saved_override_or_the_first_prompt() {
    let sample = Sample::new("recent-title");
    planted(&sample, &nth(1), &["the words that were typed"]);
    planted(&sample, &nth(2), &["about to be renamed"]);
    super::index::ensure(&sample.logs()).expect("the sessions indexed");
    let renamed: crucible_core::SessionId = nth(2).parse().expect("a session identifier");
    super::super::retitle(&sample.logs(), &renamed, "the debugging one").expect("the title kept");

    let offered = offered(&sample, 4);

    let titles: Vec<&str> = offered.iter().map(Recorded::title).collect();
    assert_eq!(titles, ["the debugging one", "the words that were typed"]);
    assert_eq!(
        offered.first().map(Recorded::asked),
        Some("about to be renamed"),
        "the first prompt stays underneath the saved title"
    );
}

#[test]
fn a_session_says_how_many_messages_the_index_counted() {
    let sample = Sample::new("recent-messages");
    planted(&sample, &nth(1), &["count me"]);
    super::index::ensure(&sample.logs()).expect("the session indexed");
    let counted: crucible_core::SessionId = nth(1).parse().expect("a session identifier");
    super::index::tally(&sample.logs(), &counted, 7).expect("the count kept");

    let offered = offered(&sample, 4);

    assert_eq!(offered.first().map(Recorded::messages), Some(7));
}

#[test]
fn a_session_that_opened_with_a_file_still_says_what_was_asked() {
    // The row is drawn from the first prompt, and format 6 writes that prompt
    // with a key beside it. A reader that stopped at the shape it knew would
    // leave a session in the list with nothing written on it.
    let sample = Sample::new("recent-attached");
    let id = nth(1);
    sample.plant(
        &id,
        &[
            sample.header(wire::FORMAT, &id),
            serde_json::json!({
                "user": "what is in this screenshot",
                "attached": [{
                    "path": "pictures/holiday.png",
                    "modality": "image",
                    "media_type": "image/png",
                    "hash": "ab".repeat(32),
                }],
            })
            .to_string(),
        ],
    );

    assert_eq!(first(&offered(&sample, 4)), "what is in this screenshot");
}
