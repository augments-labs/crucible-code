//! What a crash part-way through starting a session leaves, and what the next
//! start makes of it.
//!
//! A start takes its log's name, puts the index in order, indexes the name, and
//! only then writes the header. Where the start is drawn behind the first
//! frame, the last three are paid on the session's own writer, ahead of every
//! line queued for it, so a crash can land after the start has returned and
//! before any of them, or between any two. Each point leaves its own state on
//! the disk, and each is one the next start has to take in its stride: every
//! log with a header is one the index names, the index can be built again from
//! the directory alone, and every log replays whole.

use std::path::Path;

use super::super::recent::{Reach, Roots, recent};
use super::super::{SUFFIX, index, replay, taking};
use super::*;

/// The session that was there before the crash, and must still be.
const EARLIER: &str = "01887441-0c00-7000-8000-000000000000";

/// How far a start got before the process went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crashed {
    /// The name was taken: an empty log, and its claim.
    Taken,
    /// And the index was put in order.
    Ordered,
    /// And the name went into the index.
    Indexed,
    /// And part of the header reached the log.
    Torn,
    /// And the whole header did, and the first prompt after it.
    Headed,
}

const EVERY: [Crashed; 5] = [
    Crashed::Taken,
    Crashed::Ordered,
    Crashed::Indexed,
    Crashed::Torn,
    Crashed::Headed,
];

/// Whether the directory held an index before the crash: one this build
/// vouched for, or none, as an older installation has.
#[derive(Debug, Clone, Copy)]
enum Before {
    Vouched,
    Unindexed,
}

/// A directory holding one finished session, indexed as `before` says.
fn earlier(sample: &Sample, before: Before) {
    sample.plant(
        EARLIER,
        &[
            sample.header(wire::FORMAT, EARLIER),
            wire::line(&said("before the crash")),
        ],
    );
    if matches!(before, Before::Vouched) {
        index::ensure(&sample.logs()).expect("an index");
    }
}

/// Leaves what a start that got as far as `crashed` leaves, under a name of
/// its own, and returns that name.
///
/// Built from the same steps the start takes, so what is on the disk is what a
/// process killed at that point leaves: the handles go when it does.
fn crash(sample: &Sample, crashed: Crashed) -> SessionId {
    let id = SessionId::new();
    let path = sample.logs().join(format!("{}.{SUFFIX}", id.as_str()));
    let (mut file, claim) = taking(&path).expect("a name").expect("a free name");

    let header = wire::header(&id, sample.workspace().root(), None, None);
    let reached = EVERY
        .iter()
        .position(|step| *step == crashed)
        .expect("a step");
    for step in EVERY.iter().take(reached + 1) {
        match step {
            Crashed::Taken => {}
            Crashed::Ordered => index::ensure(&sample.logs()).expect("the index in order"),
            Crashed::Indexed => index::record(&sample.logs(), &id).expect("the name indexed"),
            Crashed::Torn => {
                let half = header.len() / 2;
                file.write_all(header.as_bytes().get(..half).expect("half a header"))
                    .expect("half a header written");
            }
            Crashed::Headed => {
                let half = header.len() / 2;
                let rest = header
                    .as_bytes()
                    .get(half..)
                    .expect("the rest of the header");
                file.write_all(rest)
                    .expect("the rest of the header written");
                writeln!(file).expect("the header ended");
                writeln!(file, "{}", wire::line(&said("after the header")))
                    .expect("the first prompt written");
            }
        }
    }

    drop(file);
    drop(claim);
    id
}

/// Every log in the directory whose first line was written whole.
fn headed(logs: &Path) -> Vec<SessionId> {
    let mut found = Vec::new();
    for entry in fs::read_dir(logs).expect("the sessions directory") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|suffix| suffix.to_str()) != Some(SUFFIX) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("the log");
        if text.contains('\n') {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("a name");
            found.push(SessionId::from_str(stem).expect("a session's name"));
        }
    }
    found
}

/// The names the index holds, as the welcome screen reads them.
fn indexed(logs: &Path) -> Vec<SessionId> {
    index::written(logs, index::ENTRIES)
        .expect("an index that reads")
        .expect("an index")
        .into_iter()
        .map(|entry| entry.id)
        .collect()
}

/// What the welcome screen lists for the sample's workspace.
fn listed(sample: &Sample) -> Vec<SessionId> {
    let root = sample.workspace().root().to_path_buf();
    recent(
        &sample.logs(),
        Roots::These(&[root.as_path()]),
        Reach::Indexed,
        index::ENTRIES,
    )
    .into_iter()
    .map(|session| session.id().clone())
    .collect()
}

#[test]
fn every_state_a_crash_can_leave_a_start_in_is_one_the_next_start_recovers() {
    for before in [Before::Vouched, Before::Unindexed] {
        for crashed in EVERY {
            let sample = Sample::new("settling-crash");
            earlier(&sample, before);
            let lost = crash(&sample, crashed);
            let at = format!("{before:?}, crashed {crashed:?}");

            // `--continue` straight after the crash: the session that spoke
            // last is the one taken up, and one that never got a header is
            // passed over rather than refused.
            let (continued, transcript) = Session::resume(&sample.logs(), &sample.workspace())
                .unwrap_or_else(|problem| panic!("{at}: continue refused: {problem}"));
            let expected = match crashed {
                Crashed::Headed => (lost.clone(), said("after the header")),
                _ => (
                    SessionId::from_str(EARLIER).expect("a session's name"),
                    said("before the crash"),
                ),
            };
            assert_eq!(continued.id(), Some(&expected.0), "{at}");
            assert_eq!(transcript.messages().last(), Some(&expected.1), "{at}");
            drop(continued);

            // The next start, and its first prompt.
            let next = Session::start(&sample.logs(), &sample.workspace(), None)
                .unwrap_or_else(|problem| panic!("{at}: the next start refused: {problem}"));
            let started = next.id().expect("a recorded session").clone();
            next.append(&said("the next session"));
            drop(next);

            // Every log with a header is one the index names, so a bounded
            // lookup finds it, and the welcome screen lists each that said
            // anything.
            let names = indexed(&sample.logs());
            for id in headed(&sample.logs()) {
                assert!(names.contains(&id), "{at}: {} not indexed", id.as_str());
            }
            let shown = listed(&sample);
            assert!(
                shown.contains(&started),
                "{at}: the next session not listed"
            );
            assert!(
                shown.contains(&SessionId::from_str(EARLIER).expect("a session's name")),
                "{at}: the earlier session not listed"
            );
            assert_eq!(
                shown.contains(&lost),
                matches!(crashed, Crashed::Headed),
                "{at}: the crashed session listed wrongly"
            );

            // Rebuilt from nothing but the directory, the index names every
            // log with a header again.
            fs::remove_file(sample.logs().join("recent.sessions")).expect("the index removed");
            let _ = fs::remove_file(sample.logs().join("recent.sessions.ordered"));
            index::ensure(&sample.logs()).expect("the index rebuilt");
            let rebuilt = indexed(&sample.logs());
            for id in headed(&sample.logs()) {
                assert!(rebuilt.contains(&id), "{at}: {} not rebuilt", id.as_str());
            }

            // And every log with a header replays whole.
            for id in headed(&sample.logs()) {
                let path = sample.logs().join(format!("{}.{SUFFIX}", id.as_str()));
                assert!(
                    replay(&path).is_ok(),
                    "{at}: {} does not replay",
                    id.as_str()
                );
            }
        }
    }
}
