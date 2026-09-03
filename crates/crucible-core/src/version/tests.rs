//! Which names are read as ahead of which.

use super::later;

#[test]
fn a_release_is_compared_number_by_number_and_not_as_text() {
    // The whole reason this is not a comparison of the two strings: ten sorts
    // before nine as text, and whoever is on the newer of the two would be
    // told for ever that the older one is ahead of them.
    assert!(later("0.0.10", "0.0.9"));
    assert!(!later("0.0.9", "0.0.10"));

    assert!(later("0.1.0", "0.0.9"));
    assert!(later("1.0.0", "0.9.9"));
    assert!(!later("0.0.9", "0.0.9"));
    assert!(!later("0.0.8", "0.0.9"));
}

#[test]
fn a_pre_release_is_read_as_the_version_it_leads_to() {
    assert!(later("0.1.0-rc.1", "0.0.9"));
    assert!(!later("0.0.9-rc.1", "0.0.9"));
    // What the dropping is for. A candidate for the release already being run
    // is the same release, so a machine on `1.0.0` is not behind `1.0.0-rc.1`.
    assert!(!later("1.0.0-rc.1", "1.0.0"));
}

#[test]
fn a_name_that_is_not_a_version_is_later_than_nothing() {
    // Neither reader wrote what it is comparing, so both meet spellings that
    // are not versions at all. A part that will not parse reading as nothing
    // is what puts these behind every real release rather than ahead of one,
    // which is the safe answer in both places: nothing to announce, and no
    // bar on hosting.
    for said in ["", "latest", "v", "nightly", "..", "-"] {
        assert!(!later(said, "0.0.9"), "{said:?}");
    }
}
