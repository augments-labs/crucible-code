//! The fake peer refuses to stand in for sources it was not built from.

use std::fs;
use std::path::Path;

use super::Owned;
use crate::fake_peer::{BUILD, Reply, SCRIPT_FILES, Script, Unbuilt, installed};

/// A script set added without its file in the stamp would let a stale peer
/// play by rules its caller no longer knows.
#[test]
fn every_script_file_is_in_the_stamp() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fake_peer/scripts");
    let mut found: Vec<String> = fs::read_dir(directory)
        .expect("the scripts directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    let mut stamped: Vec<&str> = SCRIPT_FILES.to_vec();
    stamped.sort_unstable();
    assert_eq!(found, stamped);
}

#[test]
fn a_missing_peer_is_refused_with_the_command_that_builds_it() {
    let at = Owned::new("absent");
    let refused = installed(&at.0.join("nothing"), &at.0, "claude", &Script::of([]))
        .expect_err("nothing to install");
    assert!(matches!(refused, Unbuilt::Absent(_)));
    assert!(refused.to_string().contains(BUILD));
}

#[cfg(unix)]
#[test]
fn a_peer_with_another_stamp_is_refused_with_the_command_that_builds_it() {
    use std::os::unix::fs::PermissionsExt as _;

    let at = Owned::new("stale");
    let peer = at.0.join("old-peer");
    fs::write(&peer, "#!/bin/sh\necho 0000000000000000\n").expect("written");
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o755)).expect("executable");

    let refused = installed(&peer, &at.0.join("installed"), "claude", &Script::of([]))
        .expect_err("another build");
    assert!(matches!(refused, Unbuilt::Stale(_)));
    assert!(refused.to_string().contains(BUILD));
}

#[test]
fn a_script_reads_back_as_it_was_written() {
    let script = Script::of([
        Reply::lines(["one", "two"]),
        Reply::new()
            .hear(2)
            .mutter("why")
            .flood(crate::fake_peer::Stream::Err, 3)
            .pause(std::time::Duration::from_millis(5))
            .linger()
            .exit(4),
    ]);
    assert_eq!(Script::decoded(&script.encoded()), Some(script));
}
