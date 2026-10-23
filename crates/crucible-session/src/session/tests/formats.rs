//! The logs under `tests/fixtures/sessions/`: one written before an installed
//! agent could run a session, and one an agent ran. Each is planted under the
//! id its header names with only the header's workspace moved to the
//! sample's, so what is read is what a release wrote.

use std::path::Path;

use crucible_storage::{BINDING_NAME_BYTES, ExternalRef, RouteBinding};

use super::super::replay::{replay, route_of};
use super::super::{Roots, discovered, index};
use super::*;
use crate::sample::foreign;

/// The fixture called `name`, as its bytes say.
fn fixture(name: &str) -> String {
    let at = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sessions")
        .join(name);
    fs::read_to_string(&at).expect("a session fixture")
}

/// `log` planted in `sample` under the id its header names, the header
/// moved to the sample's workspace and, where one is given, to `format`.
fn planted(sample: &Sample, log: &str, format: Option<u32>) -> PathBuf {
    let mut lines: Vec<String> = log.lines().map(str::to_owned).collect();
    let header = lines.first_mut().expect("a header");
    let mut opening: serde_json::Map<String, Value> =
        serde_json::from_str(header).expect("a header that reads");
    let id = opening
        .get("session")
        .and_then(Value::as_str)
        .expect("a session id")
        .to_owned();
    opening.insert(
        "workspace".to_owned(),
        Value::String(sample.workspace().root().display().to_string()),
    );
    if let Some(format) = format {
        opening.insert("format".to_owned(), Value::from(format));
    }
    *header = Value::Object(opening).to_string();
    sample.plant(&id, &lines)
}

/// The route the agent-run fixture was opened on.
fn work() -> RouteBinding {
    RouteBinding::new("claude", "work", Some("/home/me/.claude-work"))
        .expect("a route")
        .resolved(Path::new("/usr/local/bin/claude"))
        .reporting("2.1.0", false)
}

#[test]
fn a_log_from_before_an_agent_could_run_a_session_reads_and_continues_as_it_did() {
    let sample = Sample::new("format-13");
    let path = planted(&sample, &fixture("format-13.jsonl"), None);

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages().len(), 4);
    assert!(session.route().is_none() && session.uncertain_turn().is_none());
    session.append(&said("and the docs"));
    drop(session);
    // Continued here, it is still a log the release that wrote it reads: the
    // number on it stays, and nothing written under it needs a newer one.
    let log = fs::read_to_string(&path).expect("the log");
    assert!(log.starts_with(r#"{"branch":"main","format":13,"#), "{log}");
    assert!(!log.contains("requires_format"), "{log}");
}

#[test]
fn only_a_session_an_agent_runs_is_headed_with_a_number_the_release_before_it_refuses() {
    // A session a provider's model runs writes nothing the format before this
    // one lacks, so it carries that number and the release before this one
    // continues it; one an agent runs carries this number and is refused there.
    let sample = Sample::new("format-by-route");
    let heading = |session: &Session| {
        let log = fs::read_to_string(session.path()).expect("the log");
        let header = log.lines().next().expect("a header").to_owned();
        wire::opening(&header).map(|opening| opening.format)
    };

    let native = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    let routed = Session::start_routed(&sample.logs(), &sample.workspace(), None, &work())
        .expect("a session");

    assert_eq!(heading(&native), Some(wire::PROVIDER_LINES));
    assert_eq!(heading(&routed), Some(wire::FORMAT));
}

#[test]
fn a_log_an_agent_ran_reads_back_its_route_the_agents_name_for_it_and_its_open_turn() {
    let sample = Sample::new("format-14");
    let log = fixture("format-14.jsonl");
    let path = planted(&sample, &log, None);

    assert_eq!(route_of(&path).expect("a log that opens"), Some(work()));
    let read = replay(&path).expect("a log that reads");
    assert_eq!(read.transcript.messages().len(), 3);
    assert_eq!(read.ran.route, Some(work()));
    assert_eq!(
        read.ran.external,
        Some(ExternalRef::new("work", "3f2a9c1e-5b7d-4e2a-9c1f-0a8b6d4e2f10").expect("a name")),
    );
    assert_eq!(
        read.ran
            .uncertain
            .as_ref()
            .map(|turn| turn.under_way().as_str()),
        Some("running cargo test"),
    );

    // Each line that is not a message is written again exactly as it was.
    let mut rewritten = 0;
    for line in log.lines() {
        let again = wire::routed(line)
            .map(|route| wire::route(&route))
            .or_else(|| wire::named(line).map(|named| wire::external(&named)))
            .or_else(|| wire::uncertainty(line).map(|turn| wire::uncertain(&turn)));
        if let Some(again) = again {
            assert_eq!(again, line);
            rewritten += 1;
        }
    }
    assert_eq!(rewritten, 3);
}

#[test]
fn a_log_an_agent_ran_is_refused_by_its_number_where_that_number_is_not_read() {
    // A release that reads up to the format before this one meets this log as
    // this build meets the one after it: by the number on the header, before
    // any line it has no word for.
    let sample = Sample::new("format-14-unread");
    planted(
        &sample,
        &fixture("format-14.jsonl"),
        Some(wire::SESSION_GONE + 1),
    );

    let refused = Session::resume(&sample.logs(), &sample.workspace()).expect_err("refused");

    assert!(
        matches!(refused, SessionError::Foreign { .. }),
        "{refused:?}"
    );
    assert_eq!(
        refused
            .to_string()
            .split_once(" was ")
            .map(|(_, said)| said),
        Some("written by a different version of crucible"),
    );
}

#[test]
fn a_route_line_that_does_not_read_is_refused_rather_than_continued_without_it() {
    // Read as no route, the session would be continued by a provider's model,
    // and a line that does not read at the end of a log is a torn tail that
    // continuing cuts: the agent's session would become a provider's.
    let log = fixture("format-14.jsonl");
    let (opening, rest) = log.split_once('\n').expect("a header");
    let (route, _) = rest.split_once('\n').expect("a route line");
    let past_the_ceiling = format!(r#""instance":"{}""#, "w".repeat(BINDING_NAME_BYTES + 1));
    for (case, damaged) in [
        (
            "too long",
            log.replacen(r#""instance":"work""#, &past_the_ceiling, 1),
        ),
        ("torn", format!("{opening}\n{}", &route[..route.len() / 2])),
        ("alone", format!("{opening}\n{route}\n")),
    ] {
        let sample = Sample::new("format-14-damaged");
        let path = planted(&sample, &damaged, None);
        let before = fs::read(&path).expect("the log");

        let refused = Session::resume(&sample.logs(), &sample.workspace()).expect_err(case);

        if case == "alone" {
            assert!(matches!(refused, SessionError::Rerouted { .. }), "{case}");
        } else {
            assert!(
                matches!(refused, SessionError::Log { .. }),
                "{case}: {refused:?}"
            );
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("an id");
        let id = SessionId::from_str(id).expect("an id");
        assert!(crate::picked(&sample.logs(), &sample.workspace(), Some(&id)).is_err());
        // Nor is it listed as a session a provider's model ran.
        index::record(&sample.logs(), &id).expect("an index");
        let root = sample.workspace().root().to_path_buf();
        let listed =
            discovered(&sample.logs(), Roots::These(&[root.as_path()]), 8).expect("a listing");
        let routes: Vec<Option<&str>> = listed
            .sessions()
            .iter()
            .map(|one| one.route().map(RouteBinding::instance))
            .collect();
        if case == "alone" {
            assert_eq!(routes, [Some("work")]);
        } else {
            assert_eq!((routes.len(), listed.unreadable()), (0, 1), "{case}");
        }
        assert_eq!(fs::read(&path).expect("the log"), before, "{case}");
    }
}

#[test]
fn a_log_continued_here_with_a_reasoning_models_state_is_still_one_its_release_reads() {
    // The release that wrote format 13 reads every line this one guards with
    // 13; a guard naming the format after it would refuse its own log.
    let sample = Sample::new("format-13-reasoned");
    let path = planted(&sample, &fixture("format-13.jsonl"), None);
    let (session, _) = Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    session.append(&super::reasoned());
    drop(session);

    let log = fs::read_to_string(&path).expect("the log");
    let guards: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("requires_format"))
        .collect();
    assert_eq!(guards, [r#"{"requires_format":13}"#], "{log}");
}

#[test]
fn a_route_line_anywhere_but_after_the_header_or_spelled_otherwise_is_no_route() {
    // Resume and the list read the route from the line after the header, as
    // it is written; a reader that took one from anywhere else would continue
    // on a provider's model a session that says an agent ran it.
    let log = fixture("format-14.jsonl");
    let (opening, after) = log.split_once('\n').expect("a header");
    let (route, after) = after.split_once('\n').expect("a route line");
    let (asked, rest) = after.split_once('\n').expect("a first message");
    let moved = format!("{opening}\n{asked}\n{route}\n{rest}");
    let spaced = log.replacen(r#"{"route":"#, r#"{ "route":"#, 1);
    for (case, damaged) in [("moved", moved), ("spaced", spaced)] {
        let sample = Sample::new("format-14-misplaced");
        let path = planted(&sample, &damaged, None);
        let before = fs::read(&path).expect("the log");

        assert_eq!(route_of(&path).expect("a log that opens"), None, "{case}");
        // Neither is a route, nor a line of any other kind: the log does not
        // read, and is left as it was.
        let unread = |refused: &SessionError| matches!(refused, SessionError::Log { .. });
        assert!(replay(&path).err().as_ref().is_some_and(unread), "{case}");
        assert!(
            Session::resume(&sample.logs(), &sample.workspace())
                .err()
                .as_ref()
                .is_some_and(unread),
            "{case}"
        );
        assert_eq!(fs::read(&path).expect("the log"), before, "{case}");
    }
}

#[test]
fn the_agents_name_for_a_session_is_read_only_under_the_route_it_was_opened_on() {
    let log = fixture("format-14.jsonl").replacen(
        r#"{"external":{"instance":"work""#,
        r#"{"external":{"instance":"home""#,
        1,
    );
    assert!(log.contains(r#""instance":"home""#), "{log}");
    let sample = Sample::new("format-14-other-instance");
    let path = planted(&sample, &log, None);

    // Read past rather than refused: the rest of the log is the session's.
    let read = replay(&path).expect("a log that reads");
    assert_eq!(
        read.ran.external, None,
        "a session opened on work kept a name home gave"
    );
    assert_eq!(read.transcript.messages().len(), 3);
    assert!(read.ran.uncertain.is_some());
}

#[test]
fn a_session_an_agent_ran_is_listed_with_its_route_wherever_its_header_ends() {
    // The header's length is the branch's, which the checked-out repository
    // chooses; a list that looked for the route only in what one read of the
    // header happened to bring in would call some of these native.
    let log = fixture("format-14.jsonl");
    for ends in 8180..=8200 {
        // Each sample's workspace is its own, and the header holds its path.
        let sample = Sample::new("format-14-header-length");
        let header = fs::read_to_string(planted(&sample, &log, None)).expect("the log");
        let opening = header.lines().next().expect("a header").len();
        let branch = format!("main{}", "b".repeat(ends - 1 - opening));
        let path = planted(
            &sample,
            &log.replacen(r#""branch":"main""#, &format!(r#""branch":"{branch}""#), 1),
            None,
        );
        assert_eq!(
            fs::read_to_string(&path).expect("the log").find('\n'),
            Some(ends - 1)
        );
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("an id");
        index::record(&sample.logs(), &SessionId::from_str(id).expect("an id")).expect("an index");
        let root = sample.workspace().root().to_path_buf();
        let listed =
            discovered(&sample.logs(), Roots::These(&[root.as_path()]), 8).expect("a listing");
        let routes: Vec<Option<&str>> = listed
            .sessions()
            .iter()
            .map(|one| one.route().map(RouteBinding::instance))
            .collect();
        assert_eq!(routes, [Some("work")], "a header ending at byte {ends}");
    }
}

#[test]
fn a_log_from_before_an_agent_could_run_a_session_is_listed_as_no_agents() {
    let sample = Sample::new("format-13-listed");
    let path = planted(&sample, &fixture("format-13.jsonl"), None);
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("an id");
    index::record(&sample.logs(), &SessionId::from_str(id).expect("an id")).expect("an index");
    let root = sample.workspace().root().to_path_buf();

    let listed = discovered(&sample.logs(), Roots::These(&[root.as_path()]), 8).expect("a listing");

    assert_eq!(listed.sessions().len(), 1);
    assert!(listed.sessions().iter().all(|one| one.route().is_none()));
}

#[test]
fn no_fixture_holds_what_the_agent_backends_are_found_and_removed_by() {
    for name in ["format-13.jsonl", "format-14.jsonl"] {
        assert_eq!(foreign(&fixture(name)), None, "{name}");
    }
}
