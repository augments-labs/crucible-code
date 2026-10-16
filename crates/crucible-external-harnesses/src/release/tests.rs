//! A pinned release is unpacked only as the archive it was pinned as.

use std::fs;
use std::io::{Cursor, Write as _};
use std::path::Path;

use sha2::{Digest as _, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::{Member, Pin, Unpacked, hex, unpack};
use crate::tests::Owned;

const EXECUTABLE: &[u8] = b"the agent's server";
const COMPANION: &[u8] = b"the harness it starts";

/// A zip of `members`, each a name and its bytes, deflated.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn zipped(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(*name, options).expect("a member");
        writer.write_all(bytes).expect("its bytes");
    }
    writer.finish().expect("an archive").into_inner()
}

/// The pin `archive` was published as, naming its two members.
fn pinned(archive: &[u8]) -> Pin {
    let sha256 = hex(&Sha256::digest(archive)).leak();
    Pin {
        version: "1.0.0",
        url: "https://example.invalid/agent.zip",
        sha256,
        archive: u64::try_from(archive.len()).unwrap_or(u64::MAX),
        executable: Member {
            name: "agent",
            bytes: u64::try_from(EXECUTABLE.len()).unwrap_or(u64::MAX),
        },
        companion: Member {
            name: "harness",
            bytes: u64::try_from(COMPANION.len()).unwrap_or(u64::MAX),
        },
    }
}

/// `archive` written as a download beside the release's root.
// A test with nowhere to write has nothing to report.
#[allow(clippy::expect_used)]
fn downloaded(owned: &Owned, archive: &[u8]) -> std::path::PathBuf {
    let at = owned.0.join("download.zip");
    fs::write(&at, archive).expect("the download");
    at
}

/// The names under `root`, sorted.
fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn the_pinned_archive_is_unpacked_into_its_version() {
    let owned = Owned::new("release-unpacked");
    let archive = zipped(&[("agent", EXECUTABLE), ("harness", COMPANION)]);
    let pin = pinned(&archive);
    let root = owned.0.join("programs");

    let executable = unpack(&pin, &downloaded(&owned, &archive), &root);

    assert_eq!(executable.ok(), Some(root.join("1.0.0").join("agent")));
    assert_eq!(
        fs::read(root.join("1.0.0").join("agent")).ok(),
        Some(EXECUTABLE.to_vec())
    );
    assert_eq!(
        fs::read(root.join("1.0.0").join("harness")).ok(),
        Some(COMPANION.to_vec())
    );
    assert!(pin.installed(&root));
    assert_eq!(names(&root), ["1.0.0"], "nothing staged is left behind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(root.join("1.0.0").join("agent"))
            .map_or(0, |metadata| metadata.permissions().mode());
        assert_eq!(mode & 0o111, 0o111, "the executable can be run");
    }
}

#[test]
fn a_release_already_in_place_is_kept() {
    let owned = Owned::new("release-kept");
    let archive = zipped(&[("agent", EXECUTABLE), ("harness", COMPANION)]);
    let pin = pinned(&archive);
    let root = owned.0.join("programs");
    let download = downloaded(&owned, &archive);

    assert!(unpack(&pin, &download, &root).is_ok());
    let again = unpack(&pin, &download, &root);

    assert_eq!(again.ok(), Some(root.join("1.0.0").join("agent")));
    assert_eq!(names(&root), ["1.0.0"]);
}

#[test]
fn an_archive_with_another_digest_is_refused_before_anything_is_written() {
    let owned = Owned::new("release-digest");
    let archive = zipped(&[("agent", EXECUTABLE), ("harness", COMPANION)]);
    let pin = pinned(&zipped(&[
        ("agent", b"another server"),
        ("harness", COMPANION),
    ]));
    let root = owned.0.join("programs");
    let pin = Pin {
        archive: u64::try_from(archive.len()).unwrap_or(u64::MAX),
        ..pin
    };

    let refused = unpack(&pin, &downloaded(&owned, &archive), &root);

    assert!(matches!(refused, Err(Unpacked::Digest)), "{refused:?}");
    assert!(names(&root).is_empty());
    assert!(!pin.installed(&root));
}

#[test]
fn an_archive_of_another_size_is_refused() {
    let owned = Owned::new("release-size");
    let archive = zipped(&[("agent", EXECUTABLE), ("harness", COMPANION)]);
    let pin = Pin {
        archive: 1,
        ..pinned(&archive)
    };

    let refused = unpack(
        &pin,
        &downloaded(&owned, &archive),
        &owned.0.join("programs"),
    );

    assert!(matches!(refused, Err(Unpacked::Size { .. })), "{refused:?}");
}

/// Each archive below hashes as its pin says, so only its members refuse it.
fn refused_for(owned: &Owned, members: &[(&str, &[u8])]) -> Result<std::path::PathBuf, Unpacked> {
    let archive = zipped(members);
    let pin = pinned(&archive);
    let root = owned.0.join("programs");
    let refused = unpack(&pin, &downloaded(owned, &archive), &root);
    assert!(
        names(&root).is_empty(),
        "nothing is left under {}",
        root.display()
    );
    refused
}

#[test]
fn a_third_member_is_refused() {
    let owned = Owned::new("release-third");
    let refused = refused_for(
        &owned,
        &[
            ("agent", EXECUTABLE),
            ("harness", COMPANION),
            ("extra", b"more"),
        ],
    );
    assert!(matches!(refused, Err(Unpacked::Members(3))), "{refused:?}");
}

#[test]
fn a_member_named_outside_the_pin_is_refused() {
    let owned = Owned::new("release-name");
    for name in ["../agent", "nested/agent", "nested\\agent", "other"] {
        let refused = refused_for(&owned, &[(name, EXECUTABLE), ("harness", COMPANION)]);
        assert!(
            matches!(refused, Err(Unpacked::Unpinned(_))),
            "{name}: {refused:?}"
        );
    }
}

/// The zip writer refuses a name twice, so the second name is written in
/// after it: an archive of one member twice is read as holding one file.
#[test]
fn a_member_named_twice_is_refused() {
    let owned = Owned::new("release-twice");
    let mut archive = zipped(&[("harness", COMPANION), ("harnesz", COMPANION)]);
    let mut at = 0;
    while let Some(found) = archive
        .get(at..)
        .and_then(|rest| rest.windows(7).position(|name| name == b"harnesz"))
    {
        if let Some(name) = archive.get_mut(at + found..at + found + 7) {
            name.copy_from_slice(b"harness");
        }
        at += found + 7;
    }
    let pin = pinned(&archive);
    let root = owned.0.join("programs");

    let refused = unpack(&pin, &downloaded(&owned, &archive), &root);

    assert!(matches!(refused, Err(Unpacked::Members(1))), "{refused:?}");
    assert!(!pin.installed(&root));
}

#[test]
fn a_member_of_another_size_is_refused() {
    let owned = Owned::new("release-member");
    let refused = refused_for(
        &owned,
        &[("agent", b"a longer server"), ("harness", COMPANION)],
    );
    assert!(
        matches!(refused, Err(Unpacked::Member { .. })),
        "{refused:?}"
    );
}

// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
#[test]
fn a_member_that_is_a_link_is_refused() {
    let owned = Owned::new("release-link");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    writer
        .add_symlink("agent", "/bin/sh", options)
        .expect("a link");
    writer.start_file("harness", options).expect("a member");
    writer.write_all(COMPANION).expect("its bytes");
    let archive = writer.finish().expect("an archive").into_inner();
    let pin = pinned(&archive);
    let root = owned.0.join("programs");

    let refused = unpack(&pin, &downloaded(&owned, &archive), &root);

    assert!(matches!(refused, Err(Unpacked::Kind(_))), "{refused:?}");
    assert!(names(&root).is_empty());
}
