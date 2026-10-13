//! Only a reviewed release of the agent is run.

use crate::claude::Manifest;

/// Whether the release that printed `said` is admitted, or why not.
fn admitted(said: &str) -> Result<String, String> {
    Manifest::REVIEWED
        .admits(said)
        .map(|version| version.as_str().to_owned())
        .map_err(|failure| failure.reason().as_str().to_owned())
}

#[test]
fn a_reviewed_release_is_admitted() {
    assert_eq!(admitted("2.1.277 (Claude Code)"), Ok("2.1.277".to_owned()));
    assert_eq!(
        admitted("2.1.296 (Claude Code)\n"),
        Ok("2.1.296".to_owned())
    );
}

/// A version outside the range is refused, naming the range.
#[test]
fn a_version_outside_the_range_is_refused_naming_it() {
    for said in [
        "2.1.276 (Claude Code)",
        "2.2.0 (Claude Code)",
        "3.0.0 (Claude Code)",
    ] {
        let refused = admitted(said).unwrap_err();
        assert!(
            refused.contains("2.1.277") && refused.contains("2.2.0"),
            "{refused}"
        );
    }
}

/// Anything but three numbers and the program's name is refused.
#[test]
fn a_version_printed_otherwise_is_refused() {
    for said in [
        "",
        "2.1.296",
        "2.1.296 (Other)",
        "2.1 (Claude Code)",
        "2.1.296.1 (Claude Code)",
        "2.1.+296 (Claude Code)",
        "v2.1.296 (Claude Code)",
        "2.1.296-beta (Claude Code)",
    ] {
        assert!(admitted(said).is_err(), "{said}");
    }
}
