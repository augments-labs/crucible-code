//! Which session `--continue` picks up when the directory holds both shapes of
//! name.
//!
//! Builds before the uuid form named sessions `<millis>-<6 hex>`, and every one
//! of those sorts after every uuid as text. Picked by text, an upgraded install
//! would keep resuming its newest old-build session however much newer the
//! sessions after it were.

use super::*;
use crate::session::recent::{Reach, Roots, recent};

/// A log with this sample's workspace in its header and one prompt in it.
fn planted(sample: &Sample, id: &str, asked: &str) {
    sample.plant(
        id,
        &[sample.header(wire::FORMAT, id), wire::line(&said(asked))],
    );
}

/// The name of the log `--continue` took, and what it replayed from it.
///
/// The name is what a failure shows: message text is redacted from `Debug`.
fn continued(sample: &Sample) -> (Box<str>, Vec<Message>) {
    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("a session to continue");
    let id = session
        .id()
        .map_or_else(Box::default, |id| id.as_str().into());
    (id, transcript.messages().to_vec())
}

#[test]
fn a_newer_uuid_session_is_continued_over_an_older_legacy_one() {
    let sample = Sample::new("latest-uuid-over-legacy");
    // 2023-06-01, named the way older builds named it.
    planted(&sample, "1685577600000-abcdef", "from 2023");
    // 2025-01-01, named as a uuid.
    planted(&sample, "01941f29-7c00-7000-8000-000000000000", "from 2025");

    let (id, messages) = continued(&sample);

    assert_eq!(&*id, "01941f29-7c00-7000-8000-000000000000");
    assert_eq!(messages, [said("from 2025")]);
}

#[test]
fn a_newer_legacy_session_is_continued_over_an_older_uuid_one() {
    let sample = Sample::new("latest-legacy-over-uuid");
    // 2023-06-01, named as a uuid.
    planted(&sample, "01887441-0c00-7000-8000-000000000000", "from 2023");
    // 2025-06-01, named the way older builds named it.
    planted(&sample, "1748736000000-abcdef", "from 2025");

    let (id, messages) = continued(&sample);

    assert_eq!(&*id, "1748736000000-abcdef");
    assert_eq!(messages, [said("from 2025")]);
}

#[test]
fn continuing_and_the_listing_agree_on_which_session_is_newest() {
    let sample = Sample::new("latest-agrees-with-listing");
    planted(&sample, "1685577600000-abcdef", "from 2023");
    planted(&sample, "01941f29-7c00-7000-8000-000000000000", "from 2025");

    continued(&sample);
    // `--continue` built the index the listing reads; its first row is the
    // session `--continue` chose.
    let listed = recent(
        &sample.logs(),
        Roots::These(&[sample.workspace().root()]),
        Reach::FirstFrame,
        2,
    );
    let order: Vec<&str> = listed.iter().map(|row| row.id().as_str()).collect();

    assert_eq!(
        order,
        [
            "01941f29-7c00-7000-8000-000000000000",
            "1685577600000-abcdef"
        ]
    );
}

#[test]
fn a_name_whose_time_cannot_be_read_counts_as_the_oldest() {
    // `SessionId::started` reads such a name as the epoch, which is where the
    // listing puts it too. As text it would sort after every uuid and win.
    let sample = Sample::new("latest-unreadable-time");
    planted(&sample, "01941f29-7c00-7000-8000-000000000000", "from 2025");
    planted(&sample, "99999999999999999999-000abc", "no time at all");

    let (id, messages) = continued(&sample);

    assert_eq!(&*id, "01941f29-7c00-7000-8000-000000000000");
    assert_eq!(messages, [said("from 2025")]);
}

#[test]
fn two_sessions_started_in_the_same_millisecond_go_by_name() {
    // Same instant, so the name decides, as it does wherever start times tie:
    // a legacy name sorts after a uuid as text.
    let sample = Sample::new("latest-tie");
    planted(&sample, "01941f29-7c00-7000-8000-000000000000", "the uuid");
    planted(&sample, "1735689600000-abcdef", "the legacy name");

    let (id, messages) = continued(&sample);

    assert_eq!(&*id, "1735689600000-abcdef");
    assert_eq!(messages, [said("the legacy name")]);
}

#[test]
fn a_listing_an_earlier_build_indexed_by_name_shows_the_newest_first() {
    // Earlier builds migrated a directory into the index by name as text, so
    // an install upgraded since keeps an index with every legacy name above
    // every uuid one. The welcome and `/resume` read that index as it is.
    let sample = Sample::new("latest-listing-old-index");
    planted(&sample, "1685577600000-abcdef", "from 2023");
    planted(&sample, "01941f29-7c00-7000-8000-000000000000", "from 2025");
    fs::write(
        sample.logs().join("recent.sessions"),
        "crucible-session-index-2\n\
         1685577600000-abcdef\t1\t\n\
         01941f29-7c00-7000-8000-000000000000\t1\t\n",
    )
    .expect("a writable temporary directory");

    let listed = recent(
        &sample.logs(),
        Roots::These(&[sample.workspace().root()]),
        Reach::FirstFrame,
        2,
    );
    let order: Vec<&str> = listed.iter().map(|row| row.id().as_str()).collect();

    assert_eq!(
        order,
        [
            "01941f29-7c00-7000-8000-000000000000",
            "1685577600000-abcdef"
        ]
    );
}
