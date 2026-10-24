//! A pinned release is unpacked only as the archive it was pinned as, and a
//! catalog agent's release only as the catalog lists it.

use std::fs;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};
use tar::{EntryType, Header};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::{Member, Pin, Staged, UnpackError, Unpacked, Unpacker, hex, unpack};
use crate::catalog::{Artifact, AssetRef, File, FileKind, Files, Format};
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

// A catalog agent's release, unpacked by the hostile unpacker.

/// One member of a tar a test writes, its name put in the header byte for
/// byte, so that a name no archiver would write can be.
#[derive(Clone, Copy)]
enum Tarred<'a> {
    /// A file: its name, its bytes and its mode.
    File(&'a str, &'a [u8], u32),
    /// A directory.
    Dir(&'a str),
    /// A symbolic link, and its target.
    Symlink(&'a str, &'a str),
    /// A hard link to an earlier member, and its mode.
    Hard(&'a str, &'a str, u32),
    /// A member of another kind.
    Special(&'a str, EntryType),
}

/// `bytes` put at the start of a header's field.
fn put(field: &mut [u8], bytes: &[u8]) {
    if let Some(at) = field.get_mut(..bytes.len()) {
        at.copy_from_slice(bytes);
    }
}

/// `bytes` as one gzip stream.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn gzipped(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("its bytes");
    encoder.finish().expect("a gzip stream")
}

/// A gzip-compressed tar of `members`, in that order.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn tar_gz(members: &[Tarred<'_>]) -> Vec<u8> {
    // A member's name, kind, bytes, mode and link target.
    type Described<'a> = (&'a str, EntryType, &'a [u8], u32, Option<&'a str>);
    let mut builder = tar::Builder::new(Vec::new());
    for member in members {
        let (name, kind, bytes, mode, link): Described<'_> = match *member {
            Tarred::File(name, bytes, mode) => (name, EntryType::Regular, bytes, mode, None),
            Tarred::Dir(name) => (name, EntryType::Directory, &[], 0o755, None),
            Tarred::Symlink(name, target) => (name, EntryType::Symlink, &[], 0o777, Some(target)),
            Tarred::Hard(name, target, mode) => (name, EntryType::Link, &[], mode, Some(target)),
            Tarred::Special(name, kind) => (name, kind, &[], 0o644, None),
        };
        let mut header = Header::new_gnu();
        put(&mut header.as_old_mut().name, name.as_bytes());
        if let Some(link) = link {
            put(&mut header.as_old_mut().linkname, link.as_bytes());
        }
        header.set_entry_type(kind);
        header.set_size(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        header.set_mode(mode);
        header.set_mtime(0);
        header.set_cksum();
        builder.append(&header, bytes).expect("a member");
    }
    gzipped(&builder.into_inner().expect("a tar"))
}

/// One member of a zip a test writes.
#[derive(Clone, Copy)]
enum Zipped<'a> {
    /// A file: its name, its bytes and its mode.
    File(&'a str, &'a [u8], u32),
    /// A directory.
    Dir(&'a str),
    /// A symbolic link, and its target.
    Link(&'a str, &'a str),
}

/// A zip of `members`, in that order, its files deflated.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn zip_of(members: &[Zipped<'_>]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for member in members {
        match *member {
            Zipped::File(name, bytes, mode) => {
                let options = SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .unix_permissions(mode);
                writer.start_file(name, options).expect("a member");
                writer.write_all(bytes).expect("its bytes");
            }
            Zipped::Dir(name) => writer
                .add_directory(name, SimpleFileOptions::default())
                .expect("a directory"),
            Zipped::Link(name, target) => writer
                .add_symlink(name, target, SimpleFileOptions::default())
                .expect("a link"),
        }
    }
    writer.finish().expect("an archive").into_inner()
}

/// A listed file holding `bytes`.
fn listed(path: &str, bytes: &[u8], exec: bool) -> File {
    File {
        path: path.into(),
        kind: FileKind::File {
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            sha256: hex(&Sha256::digest(bytes)).into(),
            exec,
        },
    }
}

/// A listed directory.
fn listed_dir(path: &str) -> File {
    File {
        path: path.into(),
        kind: FileKind::Dir,
    }
}

/// A listed symbolic link.
fn listed_link(path: &str, target: &str) -> File {
    File {
        path: path.into(),
        kind: FileKind::Link {
            target: target.into(),
        },
    }
}

/// The artifact `archive` is pinned as, in `format` and listing `files`, its
/// unpacked size theirs and its decompressed ceiling a gibibyte.
fn pinned_as(archive: &[u8], format: Format, files: Vec<File>) -> Artifact {
    let unpacked = files
        .iter()
        .map(|file| match file.kind {
            FileKind::File { size, .. } => size,
            FileKind::Dir | FileKind::Link { .. } => 0,
        })
        .fold(0, u64::saturating_add);
    Artifact {
        url: "https://example.invalid/releases/1.0.0/agent".into(),
        mirror: None,
        sha256: hex(&Sha256::digest(archive)).into(),
        size: u64::try_from(archive.len()).unwrap_or(u64::MAX),
        format,
        unpacked,
        decompressed: 1 << 30,
        files: Files::Listed(files),
        kept: Vec::new(),
        replaced_by: None,
    }
}

/// `artifact`, keeping only `kept` of its archive.
fn keeping(artifact: Artifact, kept: &[&str]) -> Artifact {
    Artifact {
        kept: kept.iter().map(|name| (*name).into()).collect(),
        ..artifact
    }
}

/// An empty staging directory named `name` under the test's own.
// A test with nowhere to write has nothing to report.
#[allow(clippy::expect_used)]
fn staging(owned: &Owned, name: &str) -> PathBuf {
    let at = owned.0.join(name);
    fs::create_dir(&at).expect("a staging directory");
    at
}

/// Everything under `root`, sorted: a directory ends `/`, a link is followed
/// by ` -> ` and its target, and nothing is followed through a link.
fn tree(root: &Path) -> Vec<String> {
    fn walk(at: &Path, prefix: &str, found: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(at) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let target = fs::read_link(&path)
                        .map(|target| target.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    found.push(format!("{name} -> {target}"));
                }
                Ok(metadata) if metadata.is_dir() => {
                    found.push(format!("{name}/"));
                    walk(&path, &format!("{name}/"), found);
                }
                Ok(_) => found.push(name),
                Err(_) => {}
            }
        }
    }
    let mut found = Vec::new();
    walk(root, "", &mut found);
    found.sort();
    found
}

/// Whether the file at `path` can be run; on Windows every file can.
fn can_run(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| super::runnable(&metadata))
}

/// `archive`, as `artifact` describes it, unpacked against its list and
/// surveyed, each into a staging directory of its own, and what each left
/// there.
fn both_ways(
    owned: &Owned,
    archive: &[u8],
    artifact: &Artifact,
) -> [(Result<Staged, UnpackError>, Vec<String>); 2] {
    let listed = staging(owned, "listed");
    let surveyed = staging(owned, "surveyed");
    let unpacked = Unpacker::unpack(archive, artifact, &listed);
    let survey = Unpacker::survey(archive, artifact, &surveyed);
    let both = [(unpacked, tree(&listed)), (survey, tree(&surveyed))];
    let _ = fs::remove_dir_all(listed);
    let _ = fs::remove_dir_all(surveyed);
    both
}

/// `archive`, as `artifact` describes it, unpacked against its list and
/// surveyed, each into a staging directory of its own that is left empty.
fn refused_both_ways(
    owned: &Owned,
    archive: &[u8],
    artifact: &Artifact,
) -> [Result<Staged, UnpackError>; 2] {
    both_ways(owned, archive, artifact).map(|(refused, left)| {
        assert!(
            left.is_empty(),
            "nothing is written before {refused:?}: {left:?}"
        );
        refused
    })
}

#[test]
fn a_zip_is_unpacked_as_the_list_it_matches_with_each_exec_flag() {
    let owned = Owned::new("catalog-zip");
    let archive = zip_of(&[
        Zipped::Dir("bin/"),
        Zipped::File("bin/agent", EXECUTABLE, 0o755),
        Zipped::File("LICENSE", b"MIT", 0o644),
    ]);
    let files = vec![
        listed_dir("bin"),
        listed("bin/agent", EXECUTABLE, true),
        listed("LICENSE", b"MIT", false),
    ];
    let artifact = pinned_as(&archive, Format::Zip, files.clone());
    let at = staging(&owned, "staging");

    let staged = Unpacker::unpack(&archive, &artifact, &at).expect("unpacked");

    assert_eq!(staged.files(), files.as_slice());
    assert_eq!(staged.directory(), at.as_path());
    assert_eq!(staged.unpacked(), artifact.unpacked);
    assert_eq!(tree(&at), ["LICENSE", "bin/", "bin/agent"]);
    assert_eq!(
        fs::read(at.join("bin").join("agent")).ok(),
        Some(EXECUTABLE.to_vec())
    );
    assert!(can_run(&at.join("bin").join("agent")));
    #[cfg(unix)]
    assert!(
        !can_run(&at.join("LICENSE")),
        "a file is runnable only when listed so"
    );
}

#[test]
fn a_tar_gz_is_unpacked_with_the_in_tree_links_it_lists() {
    let owned = Owned::new("catalog-tar");
    let archive = tar_gz(&[
        Tarred::File("agent", EXECUTABLE, 0o755),
        Tarred::Dir("lib/"),
        Tarred::File("lib/libagent.so.1", COMPANION, 0o644),
        Tarred::Symlink("lib/libagent.so", "libagent.so.1"),
        Tarred::Dir("bin/"),
        Tarred::Symlink("bin/agent", "../agent"),
        Tarred::Symlink("bin/run", "./agent"),
    ]);
    let files = vec![
        listed("agent", EXECUTABLE, true),
        listed_dir("lib"),
        listed("lib/libagent.so.1", COMPANION, false),
        listed_link("lib/libagent.so", "libagent.so.1"),
        listed_dir("bin"),
        listed_link("bin/agent", "../agent"),
        listed_link("bin/run", "./agent"),
    ];
    let artifact = pinned_as(&archive, Format::TarGz, files.clone());
    let at = staging(&owned, "staging");

    let staged = Unpacker::unpack(&archive, &artifact, &at).expect("unpacked");

    assert_eq!(staged.files(), files.as_slice());
    assert_eq!(
        tree(&at),
        [
            "agent",
            "bin/",
            "bin/agent -> ../agent",
            "bin/run -> ./agent",
            "lib/",
            "lib/libagent.so -> libagent.so.1",
            "lib/libagent.so.1",
        ]
    );
    assert_eq!(
        fs::read(at.join("bin").join("run")).ok(),
        Some(EXECUTABLE.to_vec())
    );
    assert_eq!(
        fs::read(at.join("lib").join("libagent.so")).ok(),
        Some(COMPANION.to_vec())
    );
}

#[test]
fn a_bare_program_is_unpacked_as_its_one_listed_file() {
    let owned = Owned::new("catalog-bare");
    let files = vec![listed("agent-linux-x64", EXECUTABLE, true)];
    let artifact = pinned_as(EXECUTABLE, Format::Bare, files.clone());
    let at = staging(&owned, "staging");

    let staged = Unpacker::unpack(EXECUTABLE, &artifact, &at).expect("unpacked");

    assert_eq!(staged.files(), files.as_slice());
    assert_eq!(tree(&at), ["agent-linux-x64"]);
    assert!(can_run(&at.join("agent-linux-x64")));

    let two = pinned_as(
        EXECUTABLE,
        Format::Bare,
        vec![
            listed("agent-linux-x64", EXECUTABLE, true),
            listed("other", EXECUTABLE, true),
        ],
    );
    let refused = Unpacker::unpack(EXECUTABLE, &two, &staging(&owned, "two"));
    assert!(matches!(refused, Err(UnpackError::Bare)), "{refused:?}");
}

#[test]
fn a_hard_link_is_unpacked_as_a_copy_of_the_file_it_names() {
    let owned = Owned::new("catalog-hard");
    let archive = tar_gz(&[
        Tarred::File("agent", EXECUTABLE, 0o755),
        Tarred::Hard("agent-again", "agent", 0o755),
    ]);
    let files = vec![
        listed("agent", EXECUTABLE, true),
        listed("agent-again", EXECUTABLE, true),
    ];
    let artifact = pinned_as(&archive, Format::TarGz, files.clone());
    let at = staging(&owned, "staging");

    let staged = Unpacker::unpack(&archive, &artifact, &at).expect("unpacked");

    assert_eq!(staged.files(), files.as_slice());
    assert_eq!(tree(&at), ["agent", "agent-again"]);
    assert_eq!(
        fs::read(at.join("agent-again")).ok(),
        Some(EXECUTABLE.to_vec())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let links = fs::metadata(at.join("agent")).map_or(0, |metadata| metadata.nlink());
        assert_eq!(links, 1, "a copy, not a second name for the same file");
    }

    let later = tar_gz(&[
        Tarred::Hard("copy", "agent", 0o755),
        Tarred::File("agent", EXECUTABLE, 0o755),
    ]);
    let artifact = pinned_as(
        &later,
        Format::TarGz,
        vec![
            listed("copy", EXECUTABLE, true),
            listed("agent", EXECUTABLE, true),
        ],
    );
    for refused in refused_both_ways(&owned, &later, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Link { .. })),
            "{refused:?}"
        );
    }
}

#[test]
fn with_kept_members_only_those_are_written_and_the_rest_are_read_and_discarded() {
    let owned = Owned::new("catalog-kept");
    let npm = vec![b'n'; 64 * 1024];
    let archive = tar_gz(&[
        Tarred::Dir("node/"),
        Tarred::Dir("node/bin/"),
        Tarred::File("node/bin/node", EXECUTABLE, 0o755),
        Tarred::Symlink("node/bin/npm", "../lib/npm-cli.js"),
        Tarred::Dir("node/lib/"),
        Tarred::File("node/lib/npm-cli.js", &npm, 0o644),
        Tarred::Special("node/lib/pipe", EntryType::Fifo),
        Tarred::Hard("node/bin/nodejs", "node/bin/node", 0o755),
        Tarred::Symlink("node/escape", "../../../etc/passwd"),
        Tarred::File("node/LICENSE", b"MIT", 0o644),
    ]);
    let files = vec![
        listed("node/bin/node", EXECUTABLE, true),
        listed("node/LICENSE", b"MIT", false),
    ];
    let artifact = keeping(
        pinned_as(&archive, Format::TarGz, files.clone()),
        &["node/bin/node", "node/LICENSE"],
    );
    let at = staging(&owned, "staging");

    let staged = Unpacker::unpack(&archive, &artifact, &at).expect("unpacked");

    assert_eq!(staged.files(), files.as_slice());
    assert_eq!(
        tree(&at),
        ["node/", "node/LICENSE", "node/bin/", "node/bin/node"],
        "a discarded member is never written"
    );
    assert!(
        staged.decompressed() > 64 * 1024,
        "the discarded members were read: {}",
        staged.decompressed()
    );

    let surveyed = staging(&owned, "surveyed");
    let survey = Unpacker::survey(&archive, &artifact, &surveyed).expect("surveyed");
    assert_eq!(survey.files(), files.as_slice());
    assert_eq!(tree(&surveyed), tree(&at));

    let absent = keeping(artifact, &["node/bin/node", "node/README"]);
    let refused = Unpacker::survey(&archive, &absent, &staging(&owned, "absent"));
    assert!(
        matches!(refused, Err(UnpackError::Missing { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_discarded_member_is_held_to_the_name_rules() {
    let owned = Owned::new("catalog-discarded-name");
    let archive = tar_gz(&[
        Tarred::File("../outside", b"discarded", 0o644),
        Tarred::File("agent", EXECUTABLE, 0o755),
    ]);
    let artifact = keeping(
        pinned_as(
            &archive,
            Format::TarGz,
            vec![listed("agent", EXECUTABLE, true)],
        ),
        &["agent"],
    );
    for refused in refused_both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Name { .. })),
            "{refused:?}"
        );
    }
}

#[test]
fn the_pin_is_checked_before_a_byte_of_the_archive_is_read() {
    let owned = Owned::new("catalog-pin");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let files = vec![listed("agent", EXECUTABLE, true)];
    let other = Artifact {
        sha256: hex(&Sha256::digest(b"another release")).into(),
        ..pinned_as(&archive, Format::TarGz, files.clone())
    };
    for refused in refused_both_ways(&owned, &archive, &other) {
        assert!(matches!(refused, Err(UnpackError::Digest)), "{refused:?}");
    }
    let shorter = Artifact {
        size: 1,
        ..pinned_as(&archive, Format::TarGz, files.clone())
    };
    for refused in refused_both_ways(&owned, &archive, &shorter) {
        assert!(
            matches!(refused, Err(UnpackError::Size { .. })),
            "{refused:?}"
        );
    }
    let garbage = b"not an archive at all";
    let unread = Artifact {
        format: Format::TarGz,
        ..pinned_as(garbage, Format::TarGz, files.clone())
    };
    let wrong = Artifact {
        sha256: hex(&Sha256::digest(b"something else")).into(),
        ..unread.clone()
    };
    for refused in refused_both_ways(&owned, garbage, &wrong) {
        assert!(
            matches!(refused, Err(UnpackError::Digest)),
            "a download that is not the pin's is refused as that, before it is read: {refused:?}"
        );
    }
    for refused in refused_both_ways(&owned, garbage, &unread) {
        assert!(
            matches!(refused, Err(UnpackError::Corrupt(_))),
            "{refused:?}"
        );
    }
}

#[test]
fn a_replacement_built_at_review_is_held_to_its_own_pin() {
    let owned = Owned::new("catalog-replaced");
    let vendor = b"the vendor's archive, which is not installed";
    let built = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let files = vec![listed("agent", EXECUTABLE, true)];
    let artifact = Artifact {
        replaced_by: Some(AssetRef {
            sha256: hex(&Sha256::digest(&built)).into(),
            size: u64::try_from(built.len()).unwrap_or(u64::MAX),
        }),
        ..pinned_as(vendor, Format::TarGz, files.clone())
    };

    let staged = Unpacker::unpack(&built, &artifact, &staging(&owned, "staging"));
    assert_eq!(
        staged.ok().map(|staged| staged.files().to_vec()),
        Some(files)
    );

    let refused = Unpacker::unpack(vendor, &artifact, &staging(&owned, "vendor"));
    assert!(
        matches!(refused, Err(UnpackError::Size { .. } | UnpackError::Digest)),
        "{refused:?}"
    );
}

#[test]
fn a_name_that_climbs_out_or_is_not_relative_is_refused() {
    let owned = Owned::new("catalog-climbs");
    for name in [
        "../agent",
        "lib/../../agent",
        "lib/..",
        "/agent",
        "/",
        "C:agent",
        "c:/agent",
        "lib\\agent",
        "./agent",
        "lib//agent",
        "lib/./agent",
        "",
    ] {
        let files = vec![listed(name, EXECUTABLE, true)];
        let tarred = tar_gz(&[Tarred::File(name, EXECUTABLE, 0o755)]);
        let artifact = pinned_as(&tarred, Format::TarGz, files.clone());
        for refused in refused_both_ways(&owned, &tarred, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Name { .. })),
                "{name:?} in a tar: {refused:?}"
            );
        }
        if name.is_empty() || name.ends_with('/') {
            continue;
        }
        let zipped = zip_of(&[Zipped::File(name, EXECUTABLE, 0o755)]);
        let artifact = pinned_as(&zipped, Format::Zip, files);
        for refused in refused_both_ways(&owned, &zipped, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Name { .. })),
                "{name:?} in a zip: {refused:?}"
            );
        }
    }
}

#[test]
fn a_name_one_platform_would_take_otherwise_is_refused() {
    let owned = Owned::new("catalog-names");
    for name in [
        "agent\u{1}",
        "tab\there",
        "agent\u{7f}",
        "caf\u{e9}",
        "e\u{301}",
        "con",
        "CON",
        "Con.txt",
        "lib/aux.so",
        "nul",
        "prn.",
        "com0",
        "COM9.dll",
        "lpt1",
        "lib/LPT9",
        "agent.",
        "agent ",
        "lib./agent",
        "a<b",
        "a>b",
        "a\"b",
        "a|b",
        "a?b",
        "a*b",
    ] {
        let files = vec![listed(name, EXECUTABLE, true)];
        let tarred = tar_gz(&[Tarred::File(name, EXECUTABLE, 0o755)]);
        let artifact = pinned_as(&tarred, Format::TarGz, files.clone());
        for refused in refused_both_ways(&owned, &tarred, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Name { .. })),
                "{name:?} in a tar: {refused:?}"
            );
        }
        let zipped = zip_of(&[Zipped::File(name, EXECUTABLE, 0o755)]);
        let artifact = pinned_as(&zipped, Format::Zip, files);
        for refused in refused_both_ways(&owned, &zipped, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Name { .. })),
                "{name:?} in a zip: {refused:?}"
            );
        }
    }
    let near = [
        "console",
        "com10",
        "lpt",
        "auxiliary.so",
        "agent.d/run",
        "a b",
        "v1.0.0",
    ];
    for name in near {
        let tarred = tar_gz(&[Tarred::File(name, EXECUTABLE, 0o755)]);
        let artifact = pinned_as(&tarred, Format::TarGz, vec![listed(name, EXECUTABLE, true)]);
        let at = staging(&owned, "near");
        let staged = Unpacker::unpack(&tarred, &artifact, &at);
        assert!(
            staged.is_ok(),
            "{name:?} is a name a file may have: {staged:?}"
        );
        let _ = fs::remove_dir_all(at);
    }
}

#[test]
fn names_that_differ_only_in_case_or_are_given_twice_are_refused() {
    let owned = Owned::new("catalog-twice");
    let cases: [(&[Tarred<'_>], &str); 5] = [
        (
            &[
                Tarred::File("agent", EXECUTABLE, 0o755),
                Tarred::File("Agent", EXECUTABLE, 0o755),
            ],
            "case",
        ),
        (
            &[
                Tarred::File("Lib/one", EXECUTABLE, 0o644),
                Tarred::File("lib/two", EXECUTABLE, 0o644),
            ],
            "case",
        ),
        (
            &[
                Tarred::File("agent", EXECUTABLE, 0o755),
                Tarred::File("agent", EXECUTABLE, 0o755),
            ],
            "twice",
        ),
        (
            &[
                Tarred::File("agent", EXECUTABLE, 0o755),
                Tarred::File("agent/inside", EXECUTABLE, 0o644),
            ],
            "twice",
        ),
        (
            &[
                Tarred::File("lib/agent", EXECUTABLE, 0o644),
                Tarred::File("lib", EXECUTABLE, 0o644),
            ],
            "twice",
        ),
    ];
    for (members, why) in cases {
        let archive = tar_gz(members);
        let artifact = Artifact {
            unpacked: 1 << 20,
            ..pinned_as(&archive, Format::TarGz, Vec::new())
        };
        let at = staging(&owned, "staging");
        let refused = Unpacker::survey(&archive, &artifact, &at);
        let _ = fs::remove_dir_all(at);
        match why {
            "case" => assert!(
                matches!(refused, Err(UnpackError::Collision { .. })),
                "{refused:?}"
            ),
            _ => assert!(
                matches!(refused, Err(UnpackError::Twice { .. })),
                "{refused:?}"
            ),
        }
    }
}

#[test]
fn a_zip_link_is_refused_where_it_would_be_written() {
    let owned = Owned::new("catalog-zip-link");
    let archive = zip_of(&[
        Zipped::File("agent", EXECUTABLE, 0o755),
        Zipped::Link("run", "agent"),
    ]);
    let artifact = pinned_as(
        &archive,
        Format::Zip,
        vec![
            listed("agent", EXECUTABLE, true),
            listed_link("run", "agent"),
        ],
    );
    let refused = Unpacker::unpack(&archive, &artifact, &staging(&owned, "listed"));
    assert!(
        matches!(refused, Err(UnpackError::Kind { .. })),
        "{refused:?}"
    );
    let refused = Unpacker::survey(&archive, &artifact, &staging(&owned, "surveyed"));
    assert!(
        matches!(refused, Err(UnpackError::Kind { .. })),
        "{refused:?}"
    );

    let discarded = keeping(
        pinned_as(
            &archive,
            Format::Zip,
            vec![listed("agent", EXECUTABLE, true)],
        ),
        &["agent"],
    );
    let at = staging(&owned, "discarded");
    let staged = Unpacker::unpack(&archive, &discarded, &at);
    assert!(staged.is_ok(), "a discarded link is read past: {staged:?}");
    assert_eq!(tree(&at), ["agent"]);
}

#[test]
fn a_device_or_a_pipe_is_refused() {
    let owned = Owned::new("catalog-device");
    for kind in [
        EntryType::Char,
        EntryType::Block,
        EntryType::Fifo,
        EntryType::Continuous,
    ] {
        let archive = tar_gz(&[Tarred::Special("agent", kind)]);
        let artifact = pinned_as(&archive, Format::TarGz, vec![listed("agent", b"", false)]);
        for refused in refused_both_ways(&owned, &archive, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Kind { .. })),
                "{kind:?}: {refused:?}"
            );
        }
    }
}

#[test]
fn a_member_the_list_does_not_name_is_refused_before_it_is_written() {
    let owned = Owned::new("catalog-extra");
    let archive = tar_gz(&[
        Tarred::File("extra", b"more", 0o644),
        Tarred::File("agent", EXECUTABLE, 0o755),
    ]);
    let artifact = pinned_as(
        &archive,
        Format::TarGz,
        vec![listed("agent", EXECUTABLE, true)],
    );
    let at = staging(&owned, "staging");

    let refused = Unpacker::unpack(&archive, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Unlisted { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));
}

#[test]
fn a_member_unlike_its_listing_is_refused_before_it_is_written() {
    let owned = Owned::new("catalog-differs");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let longer = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(
            &archive,
            Format::TarGz,
            vec![listed("agent", b"a longer server", true)],
        )
    };
    let cases = [
        ("a size", longer),
        (
            "an exec flag",
            pinned_as(
                &archive,
                Format::TarGz,
                vec![listed("agent", EXECUTABLE, false)],
            ),
        ),
        (
            "a kind",
            pinned_as(&archive, Format::TarGz, vec![listed_dir("agent")]),
        ),
    ];
    for (what, artifact) in cases {
        let at = staging(&owned, "staging");
        let refused = Unpacker::unpack(&archive, &artifact, &at);
        assert!(
            matches!(refused, Err(UnpackError::Differs { .. })),
            "{what}: {refused:?}"
        );
        assert!(tree(&at).is_empty(), "{what}: {:?}", tree(&at));
        let _ = fs::remove_dir_all(at);
    }

    let plain = tar_gz(&[Tarred::File("LICENSE", b"MIT", 0o644)]);
    let artifact = pinned_as(&plain, Format::TarGz, vec![listed("LICENSE", b"MIT", true)]);
    let refused = Unpacker::unpack(&plain, &artifact, &staging(&owned, "plain"));
    assert!(
        matches!(refused, Err(UnpackError::Differs { .. })),
        "listed runnable, archived not: {refused:?}"
    );

    let other = pinned_as(
        &archive,
        Format::TarGz,
        vec![listed("agent", b"the agent's server", true)],
    );
    let tampered = tar_gz(&[Tarred::File("agent", b"the agent's sermon", 0o755)]);
    let artifact = Artifact {
        sha256: hex(&Sha256::digest(&tampered)).into(),
        size: u64::try_from(tampered.len()).unwrap_or(u64::MAX),
        ..other
    };
    let refused = Unpacker::unpack(&tampered, &artifact, &staging(&owned, "bytes"));
    assert!(
        matches!(refused, Err(UnpackError::Differs { .. })),
        "same size, other bytes: {refused:?}"
    );
}

#[test]
fn a_listed_file_the_archive_lacks_is_refused() {
    let owned = Owned::new("catalog-missing");
    let archive = zip_of(&[Zipped::File("agent", EXECUTABLE, 0o755)]);
    let artifact = pinned_as(
        &archive,
        Format::Zip,
        vec![
            listed("agent", EXECUTABLE, true),
            listed("LICENSE", b"MIT", false),
        ],
    );
    let refused = Unpacker::unpack(&archive, &artifact, &staging(&owned, "staging"));
    assert!(
        matches!(refused, Err(UnpackError::Missing { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_list_that_adds_up_past_its_unpacked_size_or_names_a_path_twice_is_refused() {
    let owned = Owned::new("catalog-list");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let files = vec![listed("agent", EXECUTABLE, true)];
    let past = Artifact {
        unpacked: 1,
        ..pinned_as(&archive, Format::TarGz, files.clone())
    };
    let huge = File {
        path: "huge".into(),
        kind: FileKind::File {
            size: u64::MAX,
            sha256: hex(&Sha256::digest(b"")).into(),
            exec: false,
        },
    };
    let overflowing = Artifact {
        unpacked: u64::MAX,
        ..pinned_as(
            &archive,
            Format::TarGz,
            vec![
                huge.clone(),
                File {
                    path: "huger".into(),
                    ..huge
                },
            ],
        )
    };
    for artifact in [past, overflowing] {
        let at = staging(&owned, "staging");
        let refused = Unpacker::unpack(&archive, &artifact, &at);
        assert!(
            matches!(refused, Err(UnpackError::ListedPast { .. })),
            "{refused:?}"
        );
        assert!(tree(&at).is_empty());
        let _ = fs::remove_dir_all(at);
    }
    let twice = pinned_as(
        &archive,
        Format::TarGz,
        vec![listed("agent", EXECUTABLE, true), listed_dir("agent")],
    );
    let refused = Unpacker::unpack(&archive, &twice, &staging(&owned, "twice"));
    assert!(
        matches!(refused, Err(UnpackError::ListedTwice { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_list_kept_as_an_asset_or_a_wheel_is_not_unpacked_here() {
    let owned = Owned::new("catalog-elsewhere");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let asset = Artifact {
        files: Files::Asset(AssetRef {
            sha256: hex(&Sha256::digest(b"the list")).into(),
            size: 8,
        }),
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let refused = Unpacker::unpack(&archive, &asset, &staging(&owned, "asset"));
    assert!(matches!(refused, Err(UnpackError::Asset)), "{refused:?}");
    let wheel = pinned_as(
        &archive,
        Format::Wheel,
        vec![listed("agent", EXECUTABLE, true)],
    );
    for refused in refused_both_ways(&owned, &archive, &wheel) {
        assert!(matches!(refused, Err(UnpackError::Wheel)), "{refused:?}");
    }
}

#[test]
fn a_link_that_escapes_the_version_or_is_not_relative_is_refused() {
    let owned = Owned::new("catalog-escape");
    for (name, target) in [
        ("lib/agent", "../../agent"),
        ("run", "../agent"),
        ("run", "/bin/sh"),
        ("run", "C:agent"),
        ("run", "lib\\agent"),
        ("run", ""),
        ("run", "lib//agent"),
        ("run", "agent\u{1}"),
    ] {
        let archive = tar_gz(&[
            Tarred::File("agent", EXECUTABLE, 0o755),
            Tarred::Dir("lib/"),
            Tarred::Symlink(name, target),
        ]);
        let artifact = pinned_as(
            &archive,
            Format::TarGz,
            vec![
                listed("agent", EXECUTABLE, true),
                listed_dir("lib"),
                listed_link(name, target),
            ],
        );
        let at = staging(&owned, "listed");
        let refused = Unpacker::unpack(&archive, &artifact, &at);
        assert!(
            matches!(refused, Err(UnpackError::Link { .. })),
            "{name} -> {target:?}: {refused:?}"
        );
        assert!(
            !tree(&at).iter().any(|found| found.contains(" -> ")),
            "no link is made: {:?}",
            tree(&at)
        );
        let _ = fs::remove_dir_all(at);
        let at = staging(&owned, "surveyed");
        let refused = Unpacker::survey(&archive, &artifact, &at);
        assert!(
            matches!(refused, Err(UnpackError::Link { .. })),
            "{name} -> {target:?}: {refused:?}"
        );
        let _ = fs::remove_dir_all(at);
    }
}

/// A tar of `agent` and a chain of `links` links to it, `l1` the first, each
/// naming the next and the last naming `agent`.
fn chained(links: usize) -> (Vec<u8>, Vec<File>) {
    let names: Vec<String> = (1..=links).map(|at| format!("l{at}")).collect();
    let targets: Vec<String> = (2..=links)
        .map(|at| format!("l{at}"))
        .chain(["agent".to_owned()])
        .collect();
    let mut members = vec![Tarred::File("agent", EXECUTABLE, 0o755)];
    let mut files = vec![listed("agent", EXECUTABLE, true)];
    for (name, target) in names.iter().zip(&targets) {
        members.push(Tarred::Symlink(name, target));
        files.push(listed_link(name, target));
    }
    (tar_gz(&members), files)
}

#[test]
fn a_chain_of_links_resolves_within_eight_steps_and_no_further() {
    let owned = Owned::new("catalog-chain");
    let (archive, files) = chained(8);
    let artifact = pinned_as(&archive, Format::TarGz, files);
    let at = staging(&owned, "eight");
    let staged = Unpacker::unpack(&archive, &artifact, &at);
    assert!(staged.is_ok(), "{staged:?}");
    assert_eq!(fs::read(at.join("l1")).ok(), Some(EXECUTABLE.to_vec()));

    let (archive, files) = chained(9);
    let artifact = pinned_as(&archive, Format::TarGz, files);
    for (refused, left) in both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Link { .. })),
            "{refused:?}"
        );
        assert_eq!(left, ["agent"], "no link is made");
    }

    let archive = tar_gz(&[Tarred::Symlink("a", "b"), Tarred::Symlink("b", "a")]);
    let artifact = pinned_as(
        &archive,
        Format::TarGz,
        vec![listed_link("a", "b"), listed_link("b", "a")],
    );
    for refused in refused_both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Link { .. })),
            "a cycle: {refused:?}"
        );
    }
}

#[test]
fn a_dangling_link_or_one_to_what_is_not_a_file_is_refused() {
    let owned = Owned::new("catalog-dangling");
    for (target, why) in [
        ("missing", "names nothing"),
        ("lib", "names a directory"),
        ("lib/missing", "names nothing in a directory"),
        ("agent/../agent", "passes through a file"),
        ("dir-link/agent", "passes through a link"),
    ] {
        let archive = tar_gz(&[
            Tarred::File("agent", EXECUTABLE, 0o755),
            Tarred::Dir("lib/"),
            Tarred::File("lib/agent", EXECUTABLE, 0o755),
            Tarred::Symlink("dir-link", "lib"),
            Tarred::Symlink("run", target),
        ]);
        let artifact = keeping(
            pinned_as(
                &archive,
                Format::TarGz,
                vec![
                    listed("agent", EXECUTABLE, true),
                    listed_dir("lib"),
                    listed("lib/agent", EXECUTABLE, true),
                    listed_link("run", target),
                ],
            ),
            &["agent", "lib", "lib/agent", "run"],
        );
        let at = staging(&owned, "staging");
        let refused = Unpacker::unpack(&archive, &artifact, &at);
        assert!(
            matches!(refused, Err(UnpackError::Link { .. })),
            "{why}: {refused:?}"
        );
        let _ = fs::remove_dir_all(at);
    }
}

#[test]
fn an_archive_that_unpacks_past_its_unpacked_size_is_refused_before_it_is_written() {
    let owned = Owned::new("catalog-unpacked");
    let archive = tar_gz(&[
        Tarred::File("LICENSE", b"MIT", 0o644),
        Tarred::File("agent", EXECUTABLE, 0o755),
    ]);
    let artifact = Artifact {
        unpacked: 10,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let at = staging(&owned, "staging");

    let refused = Unpacker::survey(&archive, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Unpacked { .. })),
        "{refused:?}"
    );
    assert_eq!(
        tree(&at),
        ["LICENSE"],
        "nothing past the ceiling is written"
    );
}

#[test]
fn an_archive_that_decompresses_past_its_ceiling_is_refused() {
    let owned = Owned::new("catalog-decompressed");
    let large = vec![0_u8; 256 * 1024];
    let tarred = tar_gz(&[Tarred::File("agent", &large, 0o755)]);
    let zipped = zip_of(&[
        Zipped::File("discarded", &large, 0o644),
        Zipped::File("agent", EXECUTABLE, 0o755),
    ]);
    let cases = [
        Artifact {
            decompressed: 64 * 1024,
            ..pinned_as(&tarred, Format::TarGz, vec![listed("agent", &large, true)])
        },
        keeping(
            Artifact {
                decompressed: 64 * 1024,
                ..pinned_as(
                    &zipped,
                    Format::Zip,
                    vec![listed("agent", EXECUTABLE, true)],
                )
            },
            &["agent"],
        ),
    ];
    for (archive, artifact) in [&tarred, &zipped].into_iter().zip(cases) {
        let at = staging(&owned, "staging");
        let refused = Unpacker::unpack(archive, &artifact, &at);
        assert!(
            matches!(refused, Err(UnpackError::Decompressed { .. })),
            "{refused:?}"
        );
        let _ = fs::remove_dir_all(at);
    }
    let bare = Artifact {
        decompressed: 4,
        ..pinned_as(
            EXECUTABLE,
            Format::Bare,
            vec![listed("agent", EXECUTABLE, true)],
        )
    };
    for refused in refused_both_ways(&owned, EXECUTABLE, &bare) {
        assert!(
            matches!(refused, Err(UnpackError::Decompressed { .. })),
            "{refused:?}"
        );
    }
}

#[test]
fn a_byte_after_the_gzip_stream_is_refused() {
    let owned = Owned::new("catalog-trailing");
    let mut archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    archive.extend_from_slice(&gzipped(b"a second member"));
    let artifact = pinned_as(
        &archive,
        Format::TarGz,
        vec![listed("agent", EXECUTABLE, true)],
    );
    let refused = Unpacker::unpack(&archive, &artifact, &staging(&owned, "staging"));
    assert!(
        matches!(refused, Err(UnpackError::Corrupt(_))),
        "{refused:?}"
    );
}

#[test]
fn a_survey_records_the_list_an_unpack_then_accepts() {
    let owned = Owned::new("catalog-round-trip");
    let archive = tar_gz(&[
        Tarred::Dir("bin/"),
        Tarred::File("bin/agent", EXECUTABLE, 0o755),
        Tarred::File("LICENSE", b"MIT", 0o644),
        Tarred::Hard("bin/agent-again", "bin/agent", 0o750),
        Tarred::Dir("lib/"),
        Tarred::File("lib/harness", COMPANION, 0o700),
        Tarred::Symlink("lib/agent", "../bin/agent"),
    ]);
    let described = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };

    let surveyed =
        Unpacker::survey(&archive, &described, &staging(&owned, "surveyed")).expect("surveyed");

    assert_eq!(
        surveyed.files(),
        [
            listed_dir("bin"),
            listed("bin/agent", EXECUTABLE, true),
            listed("LICENSE", b"MIT", false),
            listed("bin/agent-again", EXECUTABLE, true),
            listed_dir("lib"),
            listed("lib/harness", COMPANION, true),
            listed_link("lib/agent", "../bin/agent"),
        ]
    );
    let sizes = [EXECUTABLE.len(), 3, EXECUTABLE.len(), COMPANION.len()];
    assert_eq!(
        surveyed.unpacked(),
        u64::try_from(sizes.iter().sum::<usize>()).unwrap_or(u64::MAX)
    );
    let recorded = Artifact {
        unpacked: surveyed.unpacked(),
        decompressed: surveyed.decompressed(),
        files: Files::Listed(surveyed.files().to_vec()),
        ..described
    };
    let at = staging(&owned, "unpacked");
    let unpacked = Unpacker::unpack(&archive, &recorded, &at).expect("unpacked");
    assert_eq!(unpacked.files(), surveyed.files());
    assert_eq!(tree(&at), tree(&owned.0.join("surveyed")));

    let tighter = Artifact {
        decompressed: surveyed.decompressed() - 1,
        ..recorded
    };
    let refused = Unpacker::unpack(&archive, &tighter, &staging(&owned, "tighter"));
    assert!(
        matches!(refused, Err(UnpackError::Decompressed { .. })),
        "the survey's count is the one an unpack holds it to: {refused:?}"
    );
}

#[test]
fn a_surveyed_list_reads_back_as_the_catalog_spells_it() {
    let owned = Owned::new("catalog-spelled");
    let archive = tar_gz(&[
        Tarred::File("agent", EXECUTABLE, 0o755),
        Tarred::Dir("lib/"),
        Tarred::Symlink("lib/agent", "../agent"),
    ]);
    let described = pinned_as(&archive, Format::TarGz, Vec::new());
    let mut spelled = serde_json::json!({
        "url": &*described.url,
        "sha256": &*described.sha256,
        "size": described.size,
        "format": "tar.gz",
        "unpacked": 1 << 20,
        "decompressed": 1 << 20
    });

    let unlisted = Artifact::unsigned(spelled.to_string().as_bytes()).expect("an artifact");
    assert_eq!(unlisted.files, Files::Listed(Vec::new()));
    let refused = Unpacker::unpack(&archive, &unlisted, &staging(&owned, "unlisted"));
    assert!(
        matches!(refused, Err(UnpackError::Unlisted { .. })),
        "an artifact with no list is surveyed, never unpacked: {refused:?}"
    );

    let surveyed =
        Unpacker::survey(&archive, &unlisted, &staging(&owned, "surveyed")).expect("surveyed");
    let json = surveyed.json();
    assert_eq!(
        json.get("unpacked").and_then(serde_json::Value::as_u64),
        Some(surveyed.unpacked())
    );
    assert_eq!(
        json.get("decompressed").and_then(serde_json::Value::as_u64),
        Some(surveyed.decompressed())
    );
    let files = json.get("files").cloned().unwrap_or_default();
    if let Some(object) = spelled.as_object_mut() {
        object.insert("files".to_owned(), files);
    }
    let read = Artifact::unsigned(spelled.to_string().as_bytes()).expect("an artifact");
    assert_eq!(read.files, Files::Listed(surveyed.files().to_vec()));
}

#[test]
fn a_bare_program_surveyed_is_named_after_the_address_it_is_published_at() {
    let owned = Owned::new("catalog-bare-survey");
    let artifact = Artifact {
        url: "https://example.invalid/releases/download/v1/agent-linux-x64?raw=1".into(),
        unpacked: 1 << 20,
        ..pinned_as(EXECUTABLE, Format::Bare, Vec::new())
    };
    let at = staging(&owned, "staging");
    let surveyed = Unpacker::survey(EXECUTABLE, &artifact, &at).expect("surveyed");
    assert_eq!(
        surveyed.files(),
        [listed("agent-linux-x64", EXECUTABLE, true)]
    );
    assert!(can_run(&at.join("agent-linux-x64")));

    let nameless = Artifact {
        url: "https://example.invalid/releases/".into(),
        ..artifact
    };
    for refused in refused_both_ways(&owned, EXECUTABLE, &nameless) {
        assert!(
            matches!(refused, Err(UnpackError::Name { .. } | UnpackError::Bare)),
            "{refused:?}"
        );
    }
}

/// The release a test publishes for `crucible agents unpack-check` surveys to
/// the list written out here, which the command line's own test holds its
/// committed output to.
#[cfg(feature = "proof")]
#[test]
fn the_release_a_test_publishes_for_the_intake_surveys_to_its_list() {
    let owned = Owned::new("catalog-proof");
    let (archive, described) = super::proof::catalog_release();
    let artifact = Artifact::unsigned(&described).expect("an artifact");

    let surveyed =
        Unpacker::survey(&archive, &artifact, &staging(&owned, "staging")).expect("surveyed");

    assert_eq!(
        surveyed.files(),
        [
            listed("agent", super::proof::EXECUTABLE, true),
            listed("LICENSE", super::proof::LICENSE, false),
            listed_dir("lib"),
            listed("lib/harness", super::proof::COMPANION, true),
            listed_link("lib/current", "harness"),
        ]
    );
    // Five headers, three blocks of data and the two blocks that end a tar.
    assert_eq!(surveyed.decompressed(), 10 * 512);
}
