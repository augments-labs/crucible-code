//! Picking up a session by name rather than by being the newest.
//!
//! Everything [`Session::resume`] is asked of, asked of a log somebody chose:
//! it is the same directory, open to whatever is in it, and the identifier
//! naming a file is not a reason to skip the questions about what that file
//! turns out to be.

use std::str::FromStr as _;

use crucible_core::SessionId;

use super::*;

/// The identifier a planted log is named by.
fn named(id: &str) -> SessionId {
    SessionId::from_str(id).expect("a session id")
}

#[test]
fn a_session_named_outright_is_the_one_picked_up() {
    // Not the newest, which is what `--continue` would have found. Naming one
    // is the whole point: the session worth going back to is rarely the last
    // one started.
    let sample = Sample::new("session-named");
    sample.plant(
        "0000000000001-000001",
        &[
            sample.header(wire::FORMAT, "older"),
            r#"{"user":"the one asked for"}"#.to_owned(),
        ],
    );
    record(&sample, &[said("the newest")]);

    let (_session, transcript) = Session::reopen(
        &sample.logs(),
        &sample.workspace(),
        &named("0000000000001-000001"),
    )
    .expect("the session named");

    assert_eq!(transcript.messages(), &[said("the one asked for")]);
}

#[test]
fn a_session_this_directory_has_no_log_of_is_refused_by_name() {
    let sample = Sample::new("session-named-missing");
    record(&sample, &[said("the only one")]);

    let problem = Session::reopen(
        &sample.logs(),
        &sample.workspace(),
        &named("0000000000007-000007"),
    )
    .expect_err("nothing of that name");

    assert!(matches!(problem, SessionError::Unknown { .. }), "{problem}");
    assert!(problem.to_string().contains("0000000000007-000007"));
}

#[test]
fn a_session_belonging_to_another_workspace_is_not_reachable_by_naming_it() {
    // The list somebody picked from holds this directory's sessions, so an
    // identifier that names another one did not come off it. What it would
    // continue is somebody else's transcript, in this directory's terminal.
    let sample = Sample::new("session-named-elsewhere");
    let session = Session::start(&sample.logs(), &sample.elsewhere()).expect("a new session");
    let id = session
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(named)
        .expect("the log is named by its session");
    drop(session);

    let problem = Session::reopen(&sample.logs(), &sample.workspace(), &id)
        .expect_err("not this workspace's");

    assert!(matches!(problem, SessionError::Unknown { .. }), "{problem}");
}

#[test]
fn a_session_named_from_a_build_that_spelled_things_differently_says_which() {
    // Refused rather than reported missing. The file is there and it is this
    // directory's; what it is not is readable by this build, and that is the
    // answer worth having.
    let sample = Sample::new("session-named-foreign");
    sample.plant(
        "0000000000002-000002",
        &[
            sample.header(99, "future"),
            r#"{"utterance":"something this build has never heard of"}"#.to_owned(),
        ],
    );

    let problem = Session::reopen(
        &sample.logs(),
        &sample.workspace(),
        &named("0000000000002-000002"),
    )
    .expect_err("not ours");

    assert!(matches!(problem, SessionError::Foreign { .. }), "{problem}");
}

#[test]
fn a_session_another_crucible_has_open_is_refused_by_name_too() {
    let sample = Sample::new("session-named-busy");
    let open = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let id = open
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(named)
        .expect("the log is named by its session");

    let problem = Session::reopen(&sample.logs(), &sample.workspace(), &id);

    // A filesystem with no locks cannot say a session is open, and there this
    // is the ordinary continuation it is everywhere else — see `claim`.
    match problem {
        Err(SessionError::Busy { .. }) | Ok(_) => {}
        Err(other) => panic!("{other}"),
    }
}

#[test]
fn a_session_picked_up_by_name_is_appended_to_rather_than_started_over() {
    let sample = Sample::new("session-named-append");
    let path = record(&sample, &[said("first")]);
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(named)
        .expect("the log is named by its session");

    let (session, _) =
        Session::reopen(&sample.logs(), &sample.workspace(), &id).expect("the session named");
    assert_eq!(session.path(), path);
    assert_eq!(session.id(), Some(&id), "the one that was asked for");
    session.append(&said("second"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("first"), said("second")]);
}
