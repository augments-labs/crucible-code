use std::io;

use crucible_app::update::Refused;

use super::*;

#[test]
fn a_refused_archive_is_written_without_what_its_members_are_named() {
    let corrupt = crucible_update::StageError::Corrupt(io::Error::other(
        "numeric field was not a number when getting size for \u{1b}]0;owned\u{7}crucible",
    ));

    let written = refusal(&Failed::Refused(Refused::Stage(corrupt)));

    assert!(
        !written
            .iter()
            .any(|byte| byte.is_ascii_control() && *byte != b'\n'),
        "{}",
        String::from_utf8_lossy(&written)
    );
    assert_eq!(
        written,
        b"crucible: the release was not installed: the release archive is not a readable \
          gzip-compressed tar\n"
    );
}
