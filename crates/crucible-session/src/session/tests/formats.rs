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
    let mut opening: Value = serde_json::from_str(header).expect("a header that reads");
    let id = opening["session"]
        .as_str()
        .expect("a session id")
        .to_owned();
    opening["workspace"] = Value::String(sample.workspace().root().display().to_string());
    if let Some(format) = format {
        opening["format"] = Value::from(format);
    }
    *header = opening.to_string();
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
    planted(&sample, &fixture("format-14.jsonl"), Some(wire::FORMAT + 1));

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
    let long = format!(r#""instance":"{}""#, "w".repeat(BINDING_NAME_BYTES + 1));
    for (case, damaged) in [
        ("too long", log.replacen(r#""instance":"work""#, &long, 1)),
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
fn no_fixture_holds_what_the_agent_backends_are_found_and_removed_by() {
    for name in ["format-13.jsonl", "format-14.jsonl"] {
        assert_eq!(foreign(&fixture(name)), None, "{name}");
    }
}
