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

/// What the first frame's scan offers for `workspace`.
fn here(
    sample: &Sample,
    workspace: &crucible_workspace::Workspace,
    wanted: usize,
) -> Vec<Recorded> {
    recent(
        &sample.logs(),
        Roots::These(&[workspace.root()]),
        Reach::FirstFrame,
        wanted,
    )
}

/// What the scan offers for this sample's workspace.
fn offered(sample: &Sample, wanted: usize) -> Vec<Recorded> {
    super::index::ensure(&sample.logs()).expect("the legacy sessions to be indexed");
    here(sample, &sample.workspace(), wanted)
}

/// What the newest of them was asked.
fn first(offered: &[Recorded]) -> &str {
    offered.first().expect("at least one session").asked()
}

#[test]
fn first_frame_does_not_enumerate_an_unindexed_legacy_directory() {
    let sample = Sample::new("recent-unindexed");
    planted(&sample, &nth(1), &["visible after migration"]);

    assert!(here(&sample, &sample.workspace(), 4).is_empty());

    super::index::ensure(&sample.logs()).expect("migration after the first frame");
    assert_eq!(
        first(&here(&sample, &sample.workspace(), 4)),
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

    assert!(
        recent(
            &nowhere,
            Roots::These(&[sample.workspace().root()]),
            Reach::FirstFrame,
            4,
        )
        .is_empty()
    );
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
    assert!(here(&sample, &sample.elsewhere(), 4).is_empty());
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
    let renamed: crucible_types::SessionId = nth(2).parse().expect("a session identifier");
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
    let counted: crucible_types::SessionId = nth(1).parse().expect("a session identifier");
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

/// A log recorded in `workspace`, holding `prompts` as its messages.
fn planted_in(
    sample: &Sample,
    workspace: &crucible_workspace::Workspace,
    id: &str,
    prompts: &[&str],
) -> String {
    let mut lines = vec![
        serde_json::json!({
            "format": wire::FORMAT,
            "session": id,
            "workspace": workspace.root().display().to_string(),
        })
        .to_string(),
    ];
    lines.extend(
        prompts
            .iter()
            .map(|said| serde_json::json!({ "user": said }).to_string()),
    );

    sample.plant(id, &lines);
    id.to_owned()
}

#[test]
fn the_whole_index_reaches_past_what_the_first_frame_opens() {
    // The same directory as the bound test above, asked for by a listing
    // somebody opened after the first frame: it can afford every header the
    // index names, so the two sessions under the others are found.
    let sample = Sample::new("recent-indexed");
    planted(&sample, &nth(0), &["under the others"]);
    planted(&sample, &nth(1), &["under the others"]);
    for count in 2..u64::try_from(EXAMINED + 2).unwrap_or(u64::MAX) {
        planted_in(
            &sample,
            &sample.elsewhere(),
            &nth(count),
            &["somewhere else"],
        );
    }
    super::index::ensure(&sample.logs()).expect("the sessions indexed");

    let reached = recent(
        &sample.logs(),
        Roots::These(&[sample.workspace().root()]),
        Reach::Indexed,
        usize::MAX,
    );

    assert_eq!(reached.len(), 2);
    assert!(
        offered(&sample, 4).is_empty(),
        "the first frame still gives up"
    );
}

#[test]
fn any_directory_lists_another_directorys_session_and_says_where_it_was() {
    let sample = Sample::new("recent-any");
    planted(&sample, &nth(1), &["work done here"]);
    planted_in(
        &sample,
        &sample.elsewhere(),
        &nth(2),
        &["work done elsewhere"],
    );
    super::index::ensure(&sample.logs()).expect("the sessions indexed");

    let listed = recent(&sample.logs(), Roots::Any, Reach::Indexed, usize::MAX);

    let said: Vec<(&str, &Path)> = listed
        .iter()
        .map(|session| (session.asked(), session.workspace()))
        .collect();
    assert_eq!(
        said,
        [
            ("work done elsewhere", sample.elsewhere().root()),
            ("work done here", sample.workspace().root()),
        ]
    );
}

#[test]
fn every_root_named_is_admitted_and_no_other() {
    // A repository's worktrees are several directories, and each of them is a
    // whole match like the one directory is.
    let sample = Sample::new("recent-roots");
    std::fs::create_dir_all(sample.home()).expect("a third directory");
    let third = crucible_workspace::Workspace::open(sample.home()).expect("the third exists");
    planted(&sample, &nth(1), &["here"]);
    planted_in(&sample, &sample.elsewhere(), &nth(2), &["a worktree"]);
    planted_in(&sample, &third, &nth(3), &["another project"]);
    super::index::ensure(&sample.logs()).expect("the sessions indexed");

    let listed = recent(
        &sample.logs(),
        Roots::These(&[sample.workspace().root(), sample.elsewhere().root()]),
        Reach::Indexed,
        usize::MAX,
    );

    let asked: Vec<&str> = listed.iter().map(Recorded::asked).collect();
    assert_eq!(asked, ["a worktree", "here"]);
}

#[test]
fn a_session_never_asked_anything_is_no_row_whichever_directories_are_admitted() {
    let sample = Sample::new("recent-any-headers");
    planted(&sample, &nth(1), &[]);
    planted_in(&sample, &sample.elsewhere(), &nth(2), &[]);
    planted_in(&sample, &sample.elsewhere(), &nth(3), &["a real one"]);
    super::index::ensure(&sample.logs()).expect("the sessions indexed");

    let listed = recent(&sample.logs(), Roots::Any, Reach::Indexed, usize::MAX);

    let asked: Vec<&str> = listed.iter().map(Recorded::asked).collect();
    assert_eq!(asked, ["a real one"]);
}

/// What the first prompt of every planted session says, so a listing that
/// read past a header would be caught carrying it.
const PROMPT: &str = "prompt-sentinel-never-listed";

/// A log for `workspace` whose header names `branch`, holding one prompt.
fn headed(
    sample: &Sample,
    workspace: &crucible_workspace::Workspace,
    id: &str,
    branch: Option<&str>,
) {
    let mut header = serde_json::json!({
        "format": wire::FORMAT,
        "session": id,
        "workspace": workspace.root().display().to_string(),
    });
    if let (Some(branch), Some(fields)) = (branch, header.as_object_mut()) {
        fields.insert("branch".to_owned(), branch.into());
    }
    sample.plant(
        id,
        &[
            header.to_string(),
            serde_json::json!({ "user": PROMPT }).to_string(),
        ],
    );
}

/// The index, written by hand: each id with its count and saved title.
fn indexed(sample: &Sample, entries: &[(&str, usize, Option<&str>)]) {
    let mut text = "crucible-session-index-2\n".to_owned();
    for (id, messages, title) in entries {
        let title = title.map(|title| format!("\t{title}")).unwrap_or_default();
        text.push_str(&[id, "\t", &messages.to_string(), &title, "\n"].concat());
    }
    std::fs::write(sample.logs().join("recent.sessions"), text).expect("an index");
}

/// What a listing for this sample's workspace finds.
fn discovering(sample: &Sample, wanted: usize) -> Discovery {
    discovered(
        &sample.logs(),
        Roots::These(&[sample.workspace().root()]),
        wanted,
    )
    .expect("an index that reads")
}

/// Every entry under `root`, with its bytes where it is a file.
fn tree(root: &Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
    let mut seen = Vec::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        for entry in std::fs::read_dir(&directory).expect("a directory the test made") {
            let at = entry.expect("an entry").path();
            if at.is_dir() {
                seen.push((at.clone(), None));
                left.push(at);
            } else {
                seen.push((at.clone(), Some(std::fs::read(&at).expect("its bytes"))));
            }
        }
    }
    seen.sort();
    seen
}

#[test]
fn a_listing_says_what_the_index_and_each_header_say_and_nothing_a_session_recorded() {
    let sample = Sample::new("discovered-listed");
    headed(&sample, &sample.workspace(), &nth(1), None);
    headed(
        &sample,
        &sample.workspace(),
        &nth(2),
        Some("feature/picker"),
    );
    indexed(
        &sample,
        &[(&nth(1), 4, None), (&nth(2), 9, Some("the debugging one"))],
    );

    let found = discovering(&sample, 8);

    let listed: Vec<(&str, Option<&str>, usize, Option<&str>)> = found
        .sessions()
        .iter()
        .map(|session| {
            (
                session.id().as_str(),
                session.branch(),
                session.messages(),
                session.title(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                nth(2).as_str(),
                Some("feature/picker"),
                9,
                Some("the debugging one")
            ),
            (nth(1).as_str(), None, 4, None),
        ]
    );
    assert!(!format!("{found:?}").contains(PROMPT), "{found:?}");
    assert_eq!(
        (
            found.omitted(),
            found.unreadable(),
            found.full(),
            found.unindexed()
        ),
        (0, 0, false, false)
    );
}

#[test]
fn a_listing_writes_nothing_where_it_reads() {
    let sample = Sample::new("discovered-unwritten");
    headed(&sample, &sample.workspace(), &nth(1), None);
    indexed(&sample, &[(&nth(1), 1, None)]);
    let before = tree(&sample.logs());

    let found = discovering(&sample, 8);

    assert_eq!(found.sessions().len(), 1);
    assert_eq!(
        tree(&sample.logs()),
        before,
        "a listing made, changed or removed a file"
    );
}

#[test]
fn another_directorys_sessions_are_neither_listed_nor_counted() {
    let sample = Sample::new("discovered-elsewhere");
    headed(&sample, &sample.workspace(), &nth(1), None);
    headed(&sample, &sample.elsewhere(), &nth(2), None);
    indexed(&sample, &[(&nth(1), 1, None), (&nth(2), 1, None)]);

    let found = discovering(&sample, 8);

    let ids: Vec<&str> = found
        .sessions()
        .iter()
        .map(|one| one.id().as_str())
        .collect();
    assert_eq!(ids, [nth(1).as_str()]);
    assert_eq!((found.omitted(), found.unreadable()), (0, 0));
}

#[test]
fn more_sessions_than_were_wanted_are_counted_rather_than_listed() {
    let sample = Sample::new("discovered-omitted");
    for nth_one in 1..=5 {
        headed(&sample, &sample.workspace(), &nth(nth_one), None);
    }
    let ids: Vec<String> = (1..=5).map(nth).collect();
    indexed(
        &sample,
        &ids.iter()
            .map(|id| (id.as_str(), 0, None))
            .collect::<Vec<_>>(),
    );

    let found = discovering(&sample, 2);

    let listed: Vec<&str> = found
        .sessions()
        .iter()
        .map(|one| one.id().as_str())
        .collect();
    assert_eq!(listed, [nth(5).as_str(), nth(4).as_str()]);
    assert_eq!(found.omitted(), 3);
}

#[test]
fn a_log_whose_first_line_does_not_read_is_counted_and_said_nothing_of() {
    let sample = Sample::new("discovered-unreadable");
    headed(&sample, &sample.workspace(), &nth(1), None);
    sample.plant(&nth(2), &[format!("{{\"format\": 1, {PROMPT}")]);
    sample.plant(
        &nth(3),
        &[serde_json::json!({
            "format": wire::FORMAT + 1,
            "session": nth(3),
            "workspace": sample.workspace().root().display().to_string(),
        })
        .to_string()],
    );
    std::fs::write(
        sample.logs().join(format!("{}.jsonl", nth(4))),
        "{\"format\":",
    )
    .expect("a torn log");
    indexed(
        &sample,
        &[
            (&nth(1), 1, None),
            (&nth(2), 1, None),
            (&nth(3), 1, None),
            (&nth(4), 1, None),
        ],
    );

    let found = discovering(&sample, 8);

    let ids: Vec<&str> = found
        .sessions()
        .iter()
        .map(|one| one.id().as_str())
        .collect();
    assert_eq!(ids, [nth(1).as_str()]);
    assert_eq!(found.unreadable(), 3);
    assert!(!format!("{found:?}").contains(PROMPT), "{found:?}");
}

#[test]
fn a_session_indexed_with_no_log_yet_is_left_out_and_not_counted() {
    // A session starting this instant is indexed before its header exists.
    let sample = Sample::new("discovered-unlogged");
    headed(&sample, &sample.workspace(), &nth(1), None);
    indexed(&sample, &[(&nth(1), 1, None), (&nth(2), 0, None)]);

    let found = discovering(&sample, 8);

    assert_eq!(found.sessions().len(), 1);
    assert_eq!((found.omitted(), found.unreadable()), (0, 0));
}

#[test]
fn an_index_that_holds_all_it_keeps_says_older_sessions_may_be_left_out() {
    let sample = Sample::new("discovered-full");
    // `nth` spells two hex digits, so the window is numbered from zero.
    let ids: Vec<String> = (0..u64::try_from(index::ENTRIES).expect("a small number"))
        .map(nth)
        .collect();
    indexed(
        &sample,
        &ids.iter()
            .map(|id| (id.as_str(), 0, None))
            .collect::<Vec<_>>(),
    );

    assert!(discovering(&sample, 8).full());

    indexed(
        &sample,
        &ids.iter()
            .skip(1)
            .map(|id| (id.as_str(), 0, None))
            .collect::<Vec<_>>(),
    );
    assert!(!discovering(&sample, 8).full());
}

#[test]
fn a_directory_with_no_index_yet_says_so_rather_than_that_nothing_was_recorded() {
    let sample = Sample::new("discovered-unindexed");
    headed(&sample, &sample.workspace(), &nth(1), None);

    let found = discovering(&sample, 8);
    assert!(found.unindexed(), "{found:?}");
    assert!(found.sessions().is_empty());
    assert!(
        !sample.logs().join("recent.sessions").exists(),
        "a listing built the index"
    );

    let nowhere = discovered(
        &sample.logs().join("never-made"),
        Roots::These(&[sample.workspace().root()]),
        8,
    )
    .expect("no directory is nothing recorded");
    assert_eq!(nowhere, Discovery::default());
}

#[test]
fn an_index_that_does_not_read_is_refused_without_quoting_it() {
    let sample = Sample::new("discovered-malformed");
    std::fs::write(
        sample.logs().join("recent.sessions"),
        format!("crucible-session-index-2\nnot an entry {PROMPT}\n"),
    )
    .expect("an index");

    let refused = discovered(
        &sample.logs(),
        Roots::These(&[sample.workspace().root()]),
        8,
    )
    .expect_err("an index with a line that is not an entry");

    assert!(matches!(refused, SessionError::Index { .. }), "{refused:?}");
    assert!(!refused.to_string().contains(PROMPT), "{refused}");
}
