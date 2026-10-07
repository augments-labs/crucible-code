//! What `crucible sessions list` says about what was recorded here, and what
//! it never says: anything a session recorded, or where a failure was.

use std::fs;
use std::str::FromStr as _;
use std::time::SystemTime;

use crucible_client_api::Text;
use crucible_client_api::sessions::{Listing, Report, Session as Listed};
use crucible_session::Session;
use crucible_types::{Message, SessionId};

use super::{Unmade, failure, human, listing};
use crate::AppError;
use crate::sample::Sample;

/// What a session was asked, which no list may say.
const PROMPT: &str = "prompt-sentinel-never-listed";

/// A clock that says every session started a while ago.
fn ago(_: SystemTime) -> String {
    "a while ago".to_owned()
}

/// A session recorded in `sample`'s workspace, asked [`PROMPT`], with
/// `title` saved over it where there is one.
fn recorded(sample: &Sample, branch: Option<&str>, title: Option<&str>) -> SessionId {
    let session =
        Session::start(&sample.logs(), &sample.workspace(), branch).expect("a new session");
    session.append(&Message::said(PROMPT));
    let id = session.id().expect("a recorded session").clone();
    drop(session);
    if let Some(title) = title {
        crucible_session::retitle(&sample.logs(), &id, title).expect("a saved title");
    }
    id
}

fn id(nth: u64) -> SessionId {
    SessionId::from_str(&format!(
        "{:013}-0000{nth:02x}",
        1_700_000_000_000_u64 + nth
    ))
    .expect("a legacy session id")
}

fn whole(sessions: Vec<Listed>) -> Listing {
    Listing {
        sessions,
        omitted: 0,
        unreadable: 0,
        index_full: false,
        unindexed: false,
    }
}

#[test]
fn each_session_recorded_here_is_listed_with_what_its_index_and_header_say() {
    let sample = Sample::new("sessions-listed");
    let titled = recorded(&sample, Some("main"), Some("fix the parser"));
    let plain = recorded(&sample, None, None);

    let listed = listing(&sample.root(), &sample.logs()).expect("a listing");

    let Report::Listed(said) = listed.contract() else {
        panic!("a list: {:?}", listed.contract());
    };
    assert_eq!(said.omitted + said.unreadable, 0);
    assert!(!said.index_full && !said.unindexed);
    let ids: Vec<&SessionId> = said.sessions.iter().map(|one| &one.id).collect();
    assert_eq!(ids, [&plain, &titled], "newest first");
    let first = said
        .sessions
        .iter()
        .find(|one| one.id == titled)
        .expect("the titled one");
    assert_eq!(first.branch.as_ref().map(Text::as_str), Some("main"));
    assert_eq!(
        first.title.as_ref().map(Text::as_str),
        Some("fix the parser")
    );
    assert_eq!(first.messages, 1);
    let second = said
        .sessions
        .iter()
        .find(|one| one.id == plain)
        .expect("the plain one");
    assert_eq!(
        (second.branch.as_ref(), second.title.as_ref()),
        (None, None)
    );

    let json = String::from_utf8(listed.json().expect("a document")).expect("UTF-8");
    let text = listed.human(&ago);
    for out in [&json, &text] {
        assert!(!out.contains(PROMPT), "{out}");
    }
    assert!(text.starts_with("2 sessions recorded for "), "{text}");
    assert!(
        text.contains(&format!(
            "  {titled}  a while ago  1 message  on main  fix the parser\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("  {plain}  a while ago  1 message  untitled\n")),
        "{text}"
    );
    assert!(text.contains("crucible --resume"), "{text}");
}

#[test]
fn a_session_recorded_in_another_directory_is_left_out() {
    let here = Sample::new("sessions-here");
    let there = Sample::new("sessions-there");
    recorded(&there, None, None);
    // The same session directory, so only where each was recorded tells them
    // apart.
    let mine = {
        let session =
            Session::start(&there.logs(), &here.workspace(), None).expect("a new session");
        let id = session.id().expect("a recorded session").clone();
        drop(session);
        id
    };

    let listed = listing(&here.root(), &there.logs()).expect("a listing");

    let Report::Listed(said) = listed.contract() else {
        panic!("a list");
    };
    let ids: Vec<&SessionId> = said.sessions.iter().map(|one| &one.id).collect();
    assert_eq!(ids, [&mine]);
    assert_eq!(listed.contract().status(), "complete");
}

#[test]
fn nothing_recorded_says_so_and_names_where_it_looked() {
    let sample = Sample::new("sessions-none");

    let listed = listing(&sample.root(), &sample.logs()).expect("a listing");

    assert_eq!(listed.contract(), Report::Listed(whole(Vec::new())));
    let text = listed.human(&ago);
    assert!(text.starts_with("no sessions recorded for "), "{text}");
    assert!(!sample.logs().exists(), "a list made the session directory");
}

#[test]
fn a_directory_with_sessions_and_no_index_says_they_were_not_listed() {
    let sample = Sample::new("sessions-unindexed");
    fs::create_dir_all(sample.logs()).expect("a session directory");

    let listed = listing(&sample.root(), &sample.logs()).expect("a listing");

    let report = listed.contract();
    assert_eq!(report.status(), "incomplete");
    assert!(matches!(&report, Report::Listed(said) if said.unindexed));
    let text = listed.human(&ago);
    assert!(text.contains("no session index"), "{text}");
}

#[test]
fn everything_that_kept_a_list_from_being_whole_is_said_under_it() {
    let at = std::env::temp_dir();
    let listing = Listing {
        sessions: vec![Listed {
            id: id(1),
            branch: None,
            messages: 3,
            title: None,
        }],
        omitted: 4,
        unreadable: 2,
        index_full: true,
        unindexed: false,
    };

    let text = human(&at, &listing, &ago);

    for line in [
        "4 older sessions recorded here are not listed\n",
        "2 session logs could not be read and are not listed\n",
        "the session index holds as many sessions as it keeps, so older ones may be missing\n",
        "3 messages",
    ] {
        assert!(text.contains(line), "no {line:?} in\n{text}");
    }
    assert!(!text.contains("no session index"), "{text}");
}

#[test]
fn what_a_terminal_would_act_on_in_a_title_or_branch_is_shown_as_its_escape() {
    let listing = whole(vec![Listed {
        id: id(1),
        branch: Some(Text::cut("ma\u{1b}[2Jin")),
        messages: 1,
        title: Some(Text::cut("\u{202e}gnirts\u{7}")),
    }]);

    let text = human(&std::env::temp_dir(), &listing, &ago);

    for raw in ['\u{1b}', '\u{202e}', '\u{7}'] {
        assert!(!text.contains(raw), "{text:?}");
    }
    assert!(
        text.contains(r"on ma\u{1b}[2Jin  \u{202e}gnirts\u{7}"),
        "{text}"
    );
}

#[test]
fn an_index_that_does_not_read_fails_with_a_document_that_names_no_path() {
    let sample = Sample::new("sessions-malformed");
    fs::create_dir_all(sample.logs()).expect("a session directory");
    fs::write(sample.logs().join("recent.sessions"), "not an index\n").expect("an index");

    let refused = listing(&sample.root(), &sample.logs()).expect_err("an unreadable index");

    assert!(matches!(refused, AppError::Session(_)), "{refused:?}");
    let written = failure(Unmade::Listing(&refused)).expect("a failed document");
    let report = Report::decode(&written).expect("a document that reads");
    assert_eq!(report.status(), "failed");
    assert_eq!(report.exit(), 1);
    let Report::Failed(problem) = report else {
        panic!("a failure");
    };
    assert_eq!(
        problem.as_str(),
        "the session index could not be read; standard error says why"
    );
    let written = String::from_utf8(written).expect("UTF-8");
    assert!(
        !written.contains(&*sample.logs().to_string_lossy()),
        "{written}"
    );
    assert!(!written.contains("not an index"), "{written}");
}

#[test]
fn a_directory_that_cannot_be_worked_in_fails_naming_the_step_and_no_path() {
    let sample = Sample::new("sessions-nowhere");
    let missing = sample.root().join("not-here");

    let refused = listing(&missing, &sample.logs()).expect_err("no such directory");

    let written = failure(Unmade::Listing(&refused)).expect("a failed document");
    let Report::Failed(problem) = Report::decode(&written).expect("a document") else {
        panic!("a failure");
    };
    assert_eq!(
        problem.as_str(),
        "this directory is not one crucible can work in; standard error says why"
    );
    let here = failure(Unmade::Here).expect("a failed document");
    let Report::Failed(problem) = Report::decode(&here).expect("a document") else {
        panic!("a failure");
    };
    assert_eq!(
        problem.as_str(),
        "the directory crucible was started in could not be read"
    );
}

#[test]
fn a_branch_too_long_to_show_whole_is_cut_where_it_says_so_and_leaves_the_list_incomplete() {
    let sample = Sample::new("sessions-long-branch");
    let long = "b".repeat(crucible_client_api::bounds::TEXT_BYTES + 64);
    recorded(&sample, Some(&long), None);

    let listed = listing(&sample.root(), &sample.logs()).expect("a listing");

    assert_eq!(listed.contract().status(), "incomplete");
    let text = listed.human(&ago);
    let shown = format!(
        "on {}… (cut)  untitled\n",
        "b".repeat(crucible_client_api::bounds::TEXT_BYTES)
    );
    assert!(text.contains(&shown), "{text}");
}
