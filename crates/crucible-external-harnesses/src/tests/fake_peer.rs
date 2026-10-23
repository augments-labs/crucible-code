//! The fake peer refuses to stand in for sources it was not built from.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::Owned;
use crate::fake_peer::{
    BUILD, Reply, SCRIPT_FILES, Script, Unbuilt, ceased, install, installed, lingering,
};

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

/// A grandchild that beats, ended when the test is, however it ends.
struct Beating(Child);

impl Drop for Beating {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A reader that catches the heartbeat between its truncate and its write
/// would see an empty file, which reads as a grandchild that never beat or
/// stopped.
#[test]
fn a_heartbeat_once_read_is_never_read_empty_while_the_grandchild_beats() {
    let at = Owned::new("heartbeat");
    let peer = install(&at.0, "claude", &Script::of([])).expect("built");
    let _beating = Beating(
        Command::new(&peer)
            .arg("--linger")
            .arg(&at.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("started"),
    );
    let started = Instant::now();
    while lingering(&at.0).is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the grandchild never beat"
        );
    }

    let mut reads: u64 = 0;
    let reading = Instant::now();
    while reading.elapsed() < Duration::from_secs(3) {
        reads += 1;
        assert!(
            lingering(&at.0).is_some(),
            "the heartbeat read as none after it was read, on read {reads}"
        );
    }
}

/// A beat written beside the file and renamed over it, as the peer writes it.
fn beat(directory: &Path, number: u64) {
    let landing = directory.join("lingering.landing");
    fs::write(&landing, number.to_string()).expect("written");
    fs::rename(&landing, directory.join("lingering")).expect("renamed");
}

/// A stop can return before the grandchild's last beat lands, so a beat that
/// lands after the first look and then holds still is a grandchild that
/// ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_last_beat_that_lands_late_and_holds_still_is_a_grandchild_that_ended() {
    let at = Owned::new("ceased-late");
    beat(&at.0, 7);
    let directory = at.0.clone();
    let landing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        beat(&directory, 8);
    });

    let judged = ceased(&at.0).await;

    landing.join().expect("landed");
    assert_eq!(lingering(&at.0), Some(8));
    assert_eq!(judged, Ok(()));
}

/// A grandchild that goes on beating is found beating, and the beats seen
/// are the ones the refusal reports.
#[tokio::test(flavor = "multi_thread")]
async fn a_grandchild_that_goes_on_beating_has_not_ceased() {
    let at = Owned::new("ceased-beating");
    let peer = install(&at.0, "claude", &Script::of([])).expect("built");
    let _beating = Beating(
        Command::new(&peer)
            .arg("--linger")
            .arg(&at.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("started"),
    );
    let started = Instant::now();
    while lingering(&at.0).is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the grandchild never beat"
        );
    }

    let judged = ceased(&at.0).await;

    let beating = judged.expect_err("it goes on beating");
    assert!(beating.first < beating.last, "{beating:?}");
}
