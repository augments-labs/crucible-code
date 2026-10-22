//! Picking up a session an installed agent ran, on the route it ran on.
//!
//! The resume that continues a route is the agent's own resume of its own
//! session, so it keeps that session's history and what the log said about
//! it; every other way in still refuses a routed log.

use super::*;

/// A session `route` ran, with one prompt and a turn left unsettled, closed.
fn ran(sample: &Sample, route: &crate::RouteBinding) -> (PathBuf, crucible_types::SessionId) {
    let session = Session::start_routed(&sample.logs(), &sample.workspace(), None, route)
        .expect("a routed session");
    let path = session.path().to_owned();
    let id = session.id().expect("its id").clone();
    session.append(&said("rename the module"));
    assert!(session.external(&ExternalRef::new("work", "its-session").expect("a name")));
    assert!(session.uncertain(&crucible_storage::Uncertain::new("editing", false)));
    drop(session);
    (path, id)
}

#[test]
fn picking_a_routed_session_says_its_route_and_an_unrouted_one_none_touching_neither() {
    let sample = Sample::new("session-pick-routed");
    let route = crate::RouteBinding::new("claude", "work", None).expect("a route");
    let (path, routed) = ran(&sample, &route);
    let before = fs::read(&path).expect("the log");

    let (id, found) =
        crate::pick_routed(&sample.logs(), &sample.workspace(), Some(&routed)).expect("picked");
    assert_eq!(id, routed);
    assert!(found.is_some_and(|found| found.same_route(&route)));
    assert_eq!(fs::read(&path).expect("the log"), before);
    // Not claimed: the log is still free to be picked up.
    assert!(matches!(claim(&path), Ok(Claimed::Taken(_))));

    let plain = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    let unrouted = plain.id().expect("its id").clone();
    plain.append(&said("and the tests"));
    drop(plain);

    let (id, found) =
        crate::pick_routed(&sample.logs(), &sample.workspace(), Some(&unrouted)).expect("picked");
    assert_eq!((id, found), (unrouted.clone(), None));

    // With no id, the newest, which is the unrouted one started last.
    let (id, found) =
        crate::pick_routed(&sample.logs(), &sample.workspace(), None).expect("newest");
    assert_eq!((id, found), (unrouted, None));
}

#[test]
fn a_routed_session_continues_on_its_own_route_keeping_what_the_agent_left() {
    let sample = Sample::new("session-reopen-routed");
    let route = crate::RouteBinding::new("claude", "work", None).expect("a route");
    let (_, id) = ran(&sample, &route);
    // The same agent run again: a new version and generation are the same route.
    let again = route.clone().reporting("2.0.0", false).generation_of(7);

    let (session, transcript) =
        Session::reopen_routed(&sample.logs(), &sample.workspace(), &id, &again)
            .expect("continued on its route");

    assert_eq!(transcript.messages(), &[said("rename the module")]);
    assert_eq!(
        session.external_ref().map(ExternalRef::reference),
        Some("its-session")
    );
    assert_eq!(
        session
            .uncertain_turn()
            .map(|turn| turn.under_way().as_str()),
        Some("editing")
    );
}

#[test]
fn a_routed_resume_refuses_another_route_and_a_session_no_agent_ran() {
    let sample = Sample::new("session-reopen-elsewhere");
    let route = crate::RouteBinding::new("claude", "work", None).expect("a route");
    let (path, id) = ran(&sample, &route);
    let before = fs::read(&path).expect("the log");

    let home = crate::RouteBinding::new("claude", "home", None).expect("a route");
    let problem = Session::reopen_routed(&sample.logs(), &sample.workspace(), &id, &home)
        .expect_err("another route");
    assert!(
        matches!(&problem, SessionError::Rerouted { route, .. } if route.instance() == "work"),
        "{problem:?}"
    );
    assert_eq!(fs::read(&path).expect("the log"), before);

    let plain = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    let unrouted = plain.id().expect("its id").clone();
    plain.append(&said("and the tests"));
    drop(plain);

    let problem = Session::reopen_routed(&sample.logs(), &sample.workspace(), &unrouted, &route)
        .expect_err("no route");
    assert!(
        matches!(&problem, SessionError::Unrouted { id } if &**id == unrouted.as_str()),
        "{problem:?}"
    );
    assert!(
        problem.to_string().contains("provider's model"),
        "{problem}"
    );
}

#[test]
fn a_damaged_unsettled_turn_at_the_end_of_a_log_is_refused_rather_than_cut() {
    // Cut as a torn tail, the one record that says the files may hold changes
    // nobody was told about would be gone, and the session would carry on as
    // though the last turn had settled.
    let sample = Sample::new("session-uncertain-damaged");
    let route = crate::RouteBinding::new("claude", "work", None).expect("a route");
    let (path, id) = ran(&sample, &route);
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("the log");
    writeln!(log, r#"{{"uncertain":{{"under_way":3}}}}"#).expect("a damaged line");
    drop(log);
    let before = fs::read(&path).expect("the log");

    let problem = Session::reopen_routed(&sample.logs(), &sample.workspace(), &id, &route)
        .expect_err("refused");

    assert!(matches!(problem, SessionError::Log { .. }), "{problem:?}");
    assert_eq!(fs::read(&path).expect("the log"), before);
}

#[test]
fn a_session_the_agent_said_was_gone_reopens_naming_none_and_keeps_its_history() {
    // Replayed as still named, a resume would ask the agent again for the
    // session it said it no longer has, and fail as the turn before it did;
    // replayed as a forgetting, the conversation the reader still has would
    // be gone with it.
    let sample = Sample::new("session-gone");
    let route = crate::RouteBinding::new("claude", "work", None).expect("a route");
    let (path, id) = ran(&sample, &route);
    let (session, _) = Session::reopen_routed(&sample.logs(), &sample.workspace(), &id, &route)
        .expect("continued on its route");
    // Only the agent the session was opened on says what is gone of it.
    assert!(!session.gone(&ExternalRef::new("home", "its-session").expect("a name")));
    assert!(session.gone(&ExternalRef::new("work", "its-session").expect("a name")));
    drop(session);

    // Behind a guard the release before this one refuses by number.
    let log = fs::read_to_string(&path).expect("the log");
    let lines: Vec<&str> = log.lines().collect();
    let guard = format!(r#"{{"requires_format":{}}}"#, wire::SESSION_GONE);
    let gone: Vec<&[&str]> = lines
        .windows(2)
        .filter(|pair| {
            pair.last()
                .is_some_and(|line| line.starts_with(r#"{"gone""#))
        })
        .collect();
    assert_eq!(gone.len(), 1, "{log}");
    assert!(
        gone.iter()
            .all(|pair| pair.first() == Some(&guard.as_str())),
        "{log}"
    );
    assert!(!log.contains(r#""instance":"home""#), "{log}");

    let (session, transcript) =
        Session::reopen_routed(&sample.logs(), &sample.workspace(), &id, &route)
            .expect("continued on its route");

    assert_eq!(transcript.messages(), &[said("rename the module")]);
    assert_eq!(session.external_ref(), None);
    // The turn left unsettled is still unsettled: the files may still hold
    // changes nobody was told about.
    assert_eq!(
        session
            .uncertain_turn()
            .map(|turn| turn.under_way().as_str()),
        Some("editing")
    );
}

#[test]
fn a_session_gone_partway_down_a_log_names_only_what_the_agent_called_it_after() {
    let sample = Sample::new("session-gone-midway");
    let route = crate::RouteBinding::new("claude", "work", None).expect("a route");
    let session = Session::start_routed(&sample.logs(), &sample.workspace(), None, &route)
        .expect("a routed session");
    let path = session.path().to_owned();
    let id = session.id().expect("its id").clone();
    let earlier = ExternalRef::new("work", "earlier").expect("a name");
    session.append(&said("rename the module"));
    assert!(session.external(&earlier));
    assert!(session.gone(&earlier));
    session.append(&said("and the tests"));
    drop(session);

    let read = super::super::replay::replay(&path).expect("a log that reads");
    assert_eq!(
        read.transcript.messages(),
        &[said("rename the module"), said("and the tests")]
    );
    assert_eq!(read.ran.external, None);

    // A name the agent gives afterwards holds; and one it says is gone holds
    // nothing but itself, so a name given since is kept.
    let (session, _) = Session::reopen_routed(&sample.logs(), &sample.workspace(), &id, &route)
        .expect("continued on its route");
    let later = ExternalRef::new("work", "later").expect("a name");
    assert!(session.external(&later));
    assert!(session.gone(&earlier));
    drop(session);

    let read = super::super::replay::replay(&path).expect("a log that reads");
    assert_eq!(read.ran.external, Some(later));
    assert_eq!(read.transcript.messages().len(), 2);
}
