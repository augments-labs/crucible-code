//! A pinned release is unpacked only as the archive it was pinned as, and a
//! catalog agent's release only as the catalog lists it.

use std::fs;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};
use tar::{EntryType, Header};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crucible_types::AgentKey;

use super::{
    LAUNCHER, LaunchScript, Member, OwnedPin, Pin, Staged, UnpackError, Unpacked, Unpacker, Wheels,
    hex, unpack,
};
use crate::catalog::{Admits, Artifact, AssetRef, File, FileKind, Files, Format, Launch, Target};
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
fn pinned(archive: &[u8]) -> Pin<'static> {
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
    /// An entry of `kind` holding `bytes`, such as one that describes the
    /// member after it.
    Data(&'a str, EntryType, &'a [u8]),
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

/// `bytes` as one gzip stream of stored blocks, which is quick to write and
/// to read back however much it holds.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn stored(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::none());
    encoder.write_all(bytes).expect("its bytes");
    encoder.finish().expect("a gzip stream")
}

/// A gzip-compressed tar of `members`, in that order.
fn tar_gz(members: &[Tarred<'_>]) -> Vec<u8> {
    gzipped(&tarred(members))
}

/// A tar of `members`, in that order, not compressed.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn tarred(members: &[Tarred<'_>]) -> Vec<u8> {
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
            Tarred::Data(name, kind, bytes) => (name, kind, bytes, 0o644, None),
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
    builder.into_inner().expect("a tar")
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

/// A zip of `members`, in that order, its files deflated, each recorded as
/// made on Unix with its mode. The writer records a member made on Windows
/// as made on DOS unless told otherwise, and a DOS member carries no Unix
/// mode, so without this the same test would read another archive there.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn zip_of(members: &[Zipped<'_>]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let unix = SimpleFileOptions::default().system(zip::System::Unix);
    for member in members {
        match *member {
            Zipped::File(name, bytes, mode) => {
                let options = unix
                    .compression_method(CompressionMethod::Deflated)
                    .unix_permissions(mode);
                writer.start_file(name, options).expect("a member");
                writer.write_all(bytes).expect("its bytes");
            }
            Zipped::Dir(name) => writer.add_directory(name, unix).expect("a directory"),
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
/// by ` -> ` and its target, and nothing is followed through a link. Walked
/// without recursion, so a tree as deep as a hostile name makes it is walked
/// whole.
fn tree(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((at, prefix)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&at) else {
            continue;
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
                    pending.push((path, format!("{name}/")));
                }
                Ok(_) => found.push(name),
                Err(_) => {}
            }
        }
    }
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
        "lib//agent",
        "lib/./agent",
        "././agent",
        "./../agent",
        "/./agent",
        ".",
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
        "con .txt",
        "CON .txt",
        "com1 .x",
        "aux .tar.gz",
        "lpt1 .x",
        "lib/prn .so",
        "nul .d/f",
        "conin$",
        "CONOUT$",
        "conin$.txt",
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
        "con x.txt",
        "comx",
        "com10",
        "lpt",
        "auxiliary.so",
        "a.con",
        "conin",
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

/// `artifact`, as the catalog spells it, read back through a catalog signed
/// for the test and believed, as an install reads one.
#[cfg(feature = "proof")]
// A test whose catalog is not believed has nothing to report.
#[allow(clippy::expect_used)]
fn signed_back(artifact: &serde_json::Value) -> Artifact {
    use std::time::SystemTime;

    use crate::catalog::{Catalog, Held, fixture};

    let catalog = serde_json::json!({
        "schema": 1,
        "serial": 1,
        "entries": [{
            "key": "agent",
            "registry_id": "agent",
            "version": "1.0.0",
            "kind": "binary",
            "license": "MIT",
            "review": "https://example.invalid/review/1",
            "admits": {"name": "agent", "version": "1.0.0"},
            "targets": [{
                "platform": "linux-x86_64",
                "cmd": "agent",
                "artifacts": [artifact],
            }],
        }],
    });
    let signed = fixture::signed(catalog.to_string().as_bytes());
    let believed = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        SystemTime::now(),
    )
    .expect("a believed catalog");
    believed
        .entries()
        .first()
        .and_then(|entry| entry.targets.first())
        .and_then(|target| target.artifacts.first())
        .cloned()
        .expect("its artifact")
}

#[test]
fn an_unsigned_artifact_carries_no_file_list() {
    let owned = Owned::new("catalog-unsigned");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
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

    let listed = File::json(&listed("agent", EXECUTABLE, true));
    if let Some(object) = spelled.as_object_mut() {
        object.insert("files".to_owned(), serde_json::json!([listed]));
    }
    let refused = Artifact::unsigned(spelled.to_string().as_bytes());
    assert!(
        matches!(&refused, Err(crate::catalog::CatalogError::Field { field, .. }) if &**field == "files"),
        "only a signed catalog lists an artifact's files: {refused:?}"
    );
}

#[cfg(feature = "proof")]
#[test]
fn a_surveyed_list_reads_back_through_a_signed_catalog_as_one_an_unpack_accepts() {
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
    if let Some(object) = spelled.as_object_mut() {
        for field in ["unpacked", "decompressed", "files"] {
            object.insert(
                field.to_owned(),
                json.get(field).cloned().unwrap_or_default(),
            );
        }
    }

    let read = signed_back(&spelled);

    assert_eq!(read.files, Files::Listed(surveyed.files().to_vec()));
    let at = staging(&owned, "unpacked");
    let unpacked = Unpacker::unpack(&archive, &read, &at).expect("unpacked");
    assert_eq!(unpacked.files(), surveyed.files());
    assert_eq!(tree(&at), tree(&owned.0.join("surveyed")));
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

#[test]
fn a_tree_archived_from_its_own_directory_unpacks_as_its_relative_names() {
    let owned = Owned::new("catalog-dot");
    let files = vec![
        listed("agent", EXECUTABLE, true),
        listed_dir("lib"),
        listed("lib/harness", COMPANION, true),
        listed_link("lib/current", "harness"),
    ];
    for root in ["./", "."] {
        let archive = tar_gz(&[
            Tarred::Dir(root),
            Tarred::File("./agent", EXECUTABLE, 0o755),
            Tarred::Dir("./lib/"),
            Tarred::File("./lib/harness", COMPANION, 0o755),
            Tarred::Symlink("./lib/current", "harness"),
        ]);
        let artifact = pinned_as(&archive, Format::TarGz, files.clone());
        let at = staging(&owned, "listed");
        let staged = Unpacker::unpack(&archive, &artifact, &at).expect("unpacked");
        assert_eq!(staged.files(), files.as_slice(), "{root:?}");
        assert_eq!(
            tree(&at),
            ["agent", "lib/", "lib/current -> harness", "lib/harness"]
        );
        let _ = fs::remove_dir_all(at);
        let at = staging(&owned, "surveyed");
        let surveyed = Unpacker::survey(&archive, &artifact, &at).expect("surveyed");
        assert_eq!(surveyed.files(), files.as_slice(), "{root:?}");
        let _ = fs::remove_dir_all(at);
    }

    let zipped = zip_of(&[
        Zipped::Dir("./"),
        Zipped::File("./agent", EXECUTABLE, 0o755),
    ]);
    let artifact = pinned_as(
        &zipped,
        Format::Zip,
        vec![listed("agent", EXECUTABLE, true)],
    );
    let staged = Unpacker::unpack(&zipped, &artifact, &staging(&owned, "zipped"));
    assert_eq!(
        staged.ok().map(|staged| staged.files().to_vec()),
        Some(vec![listed("agent", EXECUTABLE, true)])
    );

    let twice = tar_gz(&[
        Tarred::File("./agent", EXECUTABLE, 0o755),
        Tarred::File("agent", EXECUTABLE, 0o755),
    ]);
    let artifact = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(&twice, Format::TarGz, Vec::new())
    };
    let refused = Unpacker::survey(&twice, &artifact, &staging(&owned, "twice"));
    assert!(
        matches!(refused, Err(UnpackError::Twice { .. })),
        "`./agent` is `agent`: {refused:?}"
    );
}

/// A tar of one member named `name`, as `tar` writes a name too long for its
/// header: in a GNU long name before it.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn long_named(name: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(1);
    header.set_mode(0o644);
    header.set_mtime(0);
    builder
        .append_data(&mut header, name, &b"x"[..])
        .expect("a member");
    gzipped(&builder.into_inner().expect("a tar"))
}

/// A tar of one member whose name is given as `path` in an extended header
/// before it.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn pax_named(path: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_pax_extensions([("path", path.as_bytes())])
        .expect("an extended header");
    let mut header = Header::new_ustar();
    put(&mut header.as_old_mut().name, b"short");
    header.set_entry_type(EntryType::Regular);
    header.set_size(1);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, &b"x"[..]).expect("a member");
    gzipped(&builder.into_inner().expect("a tar"))
}

#[test]
fn a_name_past_its_ceilings_is_refused_before_it_is_held() {
    let owned = Owned::new("catalog-ceilings");
    let slashed = |components: usize| "a/".repeat(components - 1) + "f";
    let mut parts = vec!["c".repeat(255); 15];
    parts.extend(["d".repeat(254), "e".to_owned()]);
    let whole = parts.join("/");
    assert_eq!(whole.len(), 4096);
    let cases: [(String, &str); 5] = [
        (slashed(32_000), "long"),
        ("c".repeat(256), "long"),
        (format!("{whole}e"), "long"),
        (slashed(65), "deep"),
        (format!("lib/{}", "c".repeat(300)), "long"),
    ];
    for (name, why) in &cases {
        let files = vec![listed(name, b"x", false)];
        let archives = [
            (long_named(name), Format::TarGz),
            (pax_named(name), Format::TarGz),
            (zip_of(&[Zipped::File(name, b"x", 0o644)]), Format::Zip),
        ];
        for (archive, format) in archives {
            let artifact = pinned_as(&archive, format, files.clone());
            for refused in refused_both_ways(&owned, &archive, &artifact) {
                let refused_as = match why {
                    &"deep" => matches!(refused, Err(UnpackError::Deep { .. })),
                    _ => matches!(
                        refused,
                        Err(UnpackError::Overlong { .. } | UnpackError::Extended { .. })
                    ),
                };
                assert!(refused_as, "{why} {} bytes: {refused:?}", name.len());
            }
        }
    }

    for name in [slashed(64), "c".repeat(255), whole.clone()] {
        let archive = long_named(&name);
        let artifact = pinned_as(&archive, Format::TarGz, vec![listed(&name, b"x", false)]);
        let at = staging(&owned, "near");
        let staged = Unpacker::unpack(&archive, &artifact, &at);
        let _ = fs::remove_dir_all(at);
        // A file system with a shorter limit than these refuses the write,
        // not the name.
        assert!(
            !matches!(
                staged,
                Err(UnpackError::Overlong { .. } | UnpackError::Deep { .. })
            ),
            "{} bytes is a name a file may have: {staged:?}",
            name.len()
        );
    }
}

#[test]
fn a_link_target_past_its_ceiling_is_refused() {
    let owned = Owned::new("catalog-link-ceiling");
    let target = "../".repeat(1400) + "agent";
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_mtime(0);
    let linked = builder.append_link(&mut header, "run", &target);
    assert!(linked.is_ok(), "{linked:?}");
    let archive = gzipped(&builder.into_inner().unwrap_or_default());
    let artifact = pinned_as(&archive, Format::TarGz, vec![listed_link("run", &target)]);
    for refused in refused_both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Overlong { .. })),
            "{refused:?}"
        );
    }
}

#[test]
fn a_survey_writes_no_more_members_than_its_ceiling() {
    let owned = Owned::new("catalog-members");
    let names: Vec<String> = (0..=super::unpack::MEMBERS)
        .map(|at| format!("d{at}/"))
        .collect();
    let members: Vec<Tarred<'_>> = names.iter().map(|name| Tarred::Dir(name)).collect();
    let over = stored(&tarred(&members));
    let artifact = pinned_as(&over, Format::TarGz, Vec::new());
    let refused = Unpacker::survey(&over, &artifact, &staging(&owned, "over"));
    assert!(
        matches!(refused, Err(UnpackError::Members { .. })),
        "{} members: {:?}",
        members.len(),
        refused.map(|staged| staged.files().len())
    );

    let at = members.len() - 1;
    let within = stored(&tarred(members.get(..at).unwrap_or_default()));
    let artifact = pinned_as(&within, Format::TarGz, Vec::new());
    let surveyed = Unpacker::survey(&within, &artifact, &staging(&owned, "within"));
    assert_eq!(surveyed.map(|staged| staged.files().len()).ok(), Some(at));
}

#[test]
fn a_discarded_member_that_reads_past_the_ceiling_is_refused_as_it_streams() {
    let owned = Owned::new("catalog-streamed");
    let large = vec![0_u8; 256 * 1024];
    let archive = tar_gz(&[
        Tarred::File("agent", EXECUTABLE, 0o755),
        Tarred::File("discarded", &large, 0o644),
    ]);
    let artifact = keeping(
        Artifact {
            decompressed: 64 * 1024,
            ..pinned_as(
                &archive,
                Format::TarGz,
                vec![listed("agent", EXECUTABLE, true)],
            )
        },
        &["agent"],
    );
    for (refused, left) in both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Decompressed { .. })),
            "{refused:?}"
        );
        assert_eq!(left, ["agent"], "only what was kept was written");
    }
}

/// `zip` with its one member's Unix mode, as its central directory gives it,
/// changed from `from` to `to`.
fn moded(mut zip: Vec<u8>, from: u32, to: u32) -> Vec<u8> {
    let central = zip
        .windows(4)
        .rposition(|window| window == b"PK\x01\x02")
        .unwrap_or_default();
    let at = central + 38..central + 42;
    if let Some(attributes) = zip.get_mut(at) {
        assert_eq!(attributes, &(from << 16).to_le_bytes(), "the mode written");
        attributes.copy_from_slice(&(to << 16).to_le_bytes());
    }
    zip
}

#[test]
fn a_zip_member_moded_as_a_device_is_refused() {
    let owned = Owned::new("catalog-zip-device");
    for mode in [0o020_644, 0o060_644, 0o010_644, 0o140_644] {
        let archive = moded(
            zip_of(&[Zipped::File("agent", EXECUTABLE, 0o644)]),
            0o100_644,
            mode,
        );
        let artifact = pinned_as(
            &archive,
            Format::Zip,
            vec![listed("agent", EXECUTABLE, false)],
        );
        for refused in refused_both_ways(&owned, &archive, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Kind { .. })),
                "{mode:o}: {refused:?}"
            );
        }
    }
}

#[test]
fn a_link_that_passes_through_a_link_is_refused() {
    let owned = Owned::new("catalog-through");
    let archive = tar_gz(&[
        Tarred::Dir("lib/"),
        Tarred::File("lib/agent", EXECUTABLE, 0o755),
        Tarred::Symlink("run", "dir-link/agent"),
        Tarred::Symlink("dir-link", "lib"),
    ]);
    let artifact = pinned_as(
        &archive,
        Format::TarGz,
        vec![
            listed_dir("lib"),
            listed("lib/agent", EXECUTABLE, true),
            listed_link("run", "dir-link/agent"),
            listed_link("dir-link", "lib"),
        ],
    );
    let refused = Unpacker::unpack(&archive, &artifact, &staging(&owned, "staging"));
    assert!(
        matches!(&refused, Err(UnpackError::Link { name, why })
            if name == "run" && why.contains("passes through")),
        "{refused:?}"
    );
}

#[cfg(unix)]
#[test]
fn what_is_staged_is_made_owner_only_and_each_file_with_its_mode() {
    use std::os::unix::fs::PermissionsExt as _;

    let owned = Owned::new("catalog-modes");
    let archive = tar_gz(&[
        Tarred::Dir("lib/"),
        Tarred::File("lib/agent", EXECUTABLE, 0o6777),
        Tarred::File("bin/run", EXECUTABLE, 0o700),
        Tarred::File("LICENSE", b"MIT", 0o666),
    ]);
    let artifact = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let at = staging(&owned, "staging");
    let staged = Unpacker::survey(&archive, &artifact, &at);
    assert!(staged.is_ok(), "{staged:?}");
    let mode = |name: &str| {
        fs::symlink_metadata(at.join(name))
            .map_or(0, |metadata| metadata.permissions().mode() & 0o7777)
    };
    assert_eq!(mode("lib"), 0o700, "a directory the archive gives");
    assert_eq!(mode("bin"), 0o700, "a directory a member implies");
    for (file, exec) in [("lib/agent", true), ("bin/run", true), ("LICENSE", false)] {
        let mode = mode(file);
        assert_eq!(
            mode & 0o7022,
            0,
            "{file}: {mode:o} sets no id, sticky or write bit for others"
        );
        assert_eq!(mode & 0o100 != 0, exec, "{file}: {mode:o}");
    }
}

/// The extended-header records `records` spell, each `<length> <key>=<value>`
/// and a line feed, its length counting itself.
fn pax(records: &[(&str, &[u8])]) -> Vec<u8> {
    let mut spelled = Vec::new();
    for (key, value) in records {
        let body = key.len() + value.len() + 3;
        let mut length = body + 1;
        loop {
            let next = body + length.to_string().len();
            if next == length {
                break;
            }
            length = next;
        }
        spelled.extend_from_slice(format!("{length} {key}=").as_bytes());
        spelled.extend_from_slice(value);
        spelled.push(b'\n');
    }
    spelled
}

/// Whether `refused` is the archive refused as unreadable for a cause that
/// says `why`.
fn corrupt(refused: &Result<Staged, UnpackError>, why: &str) -> bool {
    matches!(refused, Err(UnpackError::Corrupt(cause)) if cause.to_string().contains(why))
}

#[test]
fn what_comes_before_a_tar_member_is_held_to_its_own_rules() {
    /// A GNU long name naming the member after it `name`.
    fn named(name: &str) -> Tarred<'_> {
        Tarred::Data("././@LongLink", EntryType::GNULongName, name.as_bytes())
    }
    let owned = Owned::new("catalog-ahead");
    let one = vec![listed("agent", b"x", false)];

    let sized = pax(&[("size", b"999")]);
    let unspelled = pax(&[("size", b"abc")]);
    for records in [&sized, &unspelled] {
        let archive = tar_gz(&[
            Tarred::Data("PaxHeaders/agent", EntryType::XHeader, records),
            Tarred::File("agent", b"x", 0o644),
        ]);
        let artifact = pinned_as(&archive, Format::TarGz, one.clone());
        for refused in refused_both_ways(&owned, &archive, &artifact) {
            assert!(corrupt(&refused, "another size"), "{refused:?}");
        }
    }

    let comment = vec![b'c'; 70_000];
    let long = pax(&[("comment", &comment)]);
    let archive = tar_gz(&[
        Tarred::Data("PaxHeaders/agent", EntryType::XHeader, &long),
        Tarred::File("agent", b"x", 0o644),
    ]);
    let artifact = pinned_as(&archive, Format::TarGz, one.clone());
    for refused in refused_both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Extended { .. })),
            "{refused:?}"
        );
    }

    let other = pax(&[("path", b"other")]);
    for orphan in [
        Tarred::Data("PaxHeaders/other", EntryType::XHeader, &other),
        named("other\0"),
    ] {
        let archive = tar_gz(&[Tarred::File("agent", b"x", 0o644), orphan]);
        let artifact = pinned_as(&archive, Format::TarGz, one.clone());
        for (refused, _) in both_ways(&owned, &archive, &artifact) {
            assert!(corrupt(&refused, "does not hold"), "{refused:?}");
        }
    }

    let renamed = pax(&[("path", b"agent")]);
    for twice in [
        [named("agent\0"), named("agent\0")],
        [
            Tarred::Data("PaxHeaders/agent", EntryType::XHeader, &renamed),
            Tarred::Data("PaxHeaders/agent", EntryType::XHeader, &renamed),
        ],
    ] {
        let mut members = twice.to_vec();
        members.push(Tarred::File("short", b"x", 0o644));
        let archive = tar_gz(&members);
        let artifact = pinned_as(&archive, Format::TarGz, one.clone());
        for refused in refused_both_ways(&owned, &archive, &artifact) {
            assert!(corrupt(&refused, "twice"), "{refused:?}");
        }
    }

    let target = "../".repeat(1700) + "agent";
    let linked = pax(&[("linkpath", target.as_bytes())]);
    let archive = tar_gz(&[
        Tarred::Data("PaxHeaders/run", EntryType::XHeader, &linked),
        Tarred::Symlink("run", "agent"),
    ]);
    let artifact = pinned_as(&archive, Format::TarGz, vec![listed_link("run", &target)]);
    for refused in refused_both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Overlong { .. })),
            "{refused:?}"
        );
    }
}

#[test]
fn a_tar_member_is_named_as_the_headers_before_it_say() {
    let owned = Owned::new("catalog-ahead-read");
    let global = pax(&[("path", b"evil"), ("comment", b"made by a tool")]);
    let renamed = pax(&[("mtime", b"1700000000.5"), ("path", b"lib/agent")]);
    let cases = [
        (
            vec![
                Tarred::Data("pax_global_header", EntryType::XGlobalHeader, &global),
                Tarred::File("agent", b"x", 0o644),
            ],
            "agent",
        ),
        (
            vec![
                Tarred::Data("PaxHeaders/short", EntryType::XHeader, &renamed),
                Tarred::File("short", b"x", 0o644),
            ],
            "lib/agent",
        ),
        (
            vec![
                Tarred::Data("PaxHeaders/short", EntryType::XHeader, &renamed),
                Tarred::Data("././@LongLink", EntryType::GNULongName, b"long/agent\0"),
                Tarred::File("short", b"x", 0o644),
            ],
            "long/agent",
        ),
    ];
    for (members, name) in cases {
        let archive = tar_gz(&members);
        let artifact = pinned_as(&archive, Format::TarGz, vec![listed(name, b"x", false)]);
        let at = staging(&owned, "staging");
        let staged = Unpacker::unpack(&archive, &artifact, &at);
        let _ = fs::remove_dir_all(at);
        assert_eq!(
            staged.ok().map(|staged| staged.files().to_vec()),
            Some(vec![listed(name, b"x", false)]),
            "{name}"
        );
    }
}

#[test]
fn a_long_name_past_its_ceiling_is_refused_by_how_it_begins() {
    let owned = Owned::new("catalog-long-said");
    let archive = long_named(&("a/".repeat(2500) + "f"));
    let artifact = pinned_as(&archive, Format::TarGz, Vec::new());
    for refused in refused_both_ways(&owned, &archive, &artifact) {
        assert!(
            matches!(&refused, Err(UnpackError::Overlong { name, .. })
                if name.starts_with("a/a/a/") && !name.contains("@LongLink")),
            "{refused:?}"
        );
    }
}

#[test]
fn a_sparse_member_is_refused_whether_it_is_kept_or_not() {
    let owned = Owned::new("catalog-sparse");
    let mut builder = tar::Builder::new(Vec::new());
    let mut sparse = Header::new_gnu();
    sparse.set_entry_type(EntryType::GNUSparse);
    sparse.set_size(0);
    sparse.set_mode(0o644);
    put(&mut sparse.as_old_mut().name, b"sparse");
    if let Some(gnu) = sparse.as_gnu_mut() {
        gnu.isextended[0] = 1;
    }
    sparse.set_cksum();
    let appended = builder.append(&sparse, &b""[..]);
    assert!(appended.is_ok(), "{appended:?}");
    let mut hidden = Header::new_gnu();
    hidden.set_entry_type(EntryType::Regular);
    hidden.set_size(4);
    hidden.set_mode(0o755);
    put(&mut hidden.as_old_mut().name, b"evil");
    hidden.set_cksum();
    let appended = builder.append(&hidden, &b"evil"[..]);
    assert!(appended.is_ok(), "{appended:?}");
    let archive = gzipped(&builder.into_inner().unwrap_or_default());
    for kept in [&[][..], &["evil"][..], &["sparse"][..]] {
        let artifact = keeping(
            Artifact {
                unpacked: 1 << 20,
                ..pinned_as(&archive, Format::TarGz, vec![listed("evil", b"evil", true)])
            },
            kept,
        );
        for refused in refused_both_ways(&owned, &archive, &artifact) {
            assert!(
                matches!(refused, Err(UnpackError::Kind { .. })),
                "kept {kept:?}: {refused:?}"
            );
        }
    }
}

/// A tar of `members` directories, each `COMPONENTS` deep and none sharing a
/// directory with another.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn deep_directories(members: usize) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for at in 0..members {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_mtime(0);
        let name = format!("m{at}/{}", ["a"; 63].join("/"));
        builder
            .append_data(&mut header, name, &b""[..])
            .expect("a directory");
    }
    stored(&builder.into_inner().expect("a tar"))
}

#[test]
fn every_directory_a_member_implies_is_charged_to_the_ceiling() {
    let owned = Owned::new("catalog-implied");
    // Each member names 64 directories of its own.
    let within = super::unpack::MEMBERS / 64;
    let over = deep_directories(within + 1);
    let artifact = pinned_as(&over, Format::TarGz, Vec::new());
    let refused = Unpacker::survey(&over, &artifact, &staging(&owned, "over"));
    assert!(
        matches!(refused, Err(UnpackError::Members { .. })),
        "{:?}",
        refused.map(|staged| staged.files().len())
    );

    let near = deep_directories(within);
    let artifact = pinned_as(&near, Format::TarGz, Vec::new());
    let surveyed = Unpacker::survey(&near, &artifact, &staging(&owned, "within"));
    assert_eq!(
        surveyed.map(|staged| staged.files().len()).ok(),
        Some(within)
    );
}

#[test]
fn a_zip_member_made_on_dos_has_no_unix_mode_so_it_is_not_runnable() {
    let owned = Owned::new("catalog-dos");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .system(zip::System::Dos)
        .unix_permissions(0o755);
    let started = writer.start_file("agent.exe", options);
    assert!(started.is_ok(), "{started:?}");
    let written = writer.write_all(EXECUTABLE);
    assert!(written.is_ok(), "{written:?}");
    let archive = writer.finish().map(Cursor::into_inner).unwrap_or_default();
    let artifact = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(&archive, Format::Zip, Vec::new())
    };

    let surveyed = Unpacker::survey(&archive, &artifact, &staging(&owned, "staging"));

    assert_eq!(
        surveyed.ok().map(|staged| staged.files().to_vec()),
        Some(vec![listed("agent.exe", EXECUTABLE, false)]),
        "the mode a DOS member's high bits seem to give is not read, on any platform"
    );
}

/// A file of a release's list, executable or not.
fn file(path: &str, size: u64, exec: bool) -> File {
    File {
        path: path.into(),
        kind: FileKind::File {
            size,
            sha256: Box::default(),
            exec,
        },
    }
}

/// A release the catalog offers as one zip of the two files [`EXECUTABLE`]
/// and [`COMPANION`], started as `agent`.
fn offered() -> OwnedPin {
    OwnedPin {
        key: AgentKey::new("agent").expect("a key"),
        version: "2.0.0".into(),
        lock_revision: None,
        admits: Admits {
            name: "agent".into(),
            version: "2.0.0".into(),
        },
        target: Target {
            platform: "linux-x86_64".into(),
            cmd: "agent".into(),
            args: Vec::new(),
            env: Vec::new(),
            hosts: vec!["example.invalid".into()],
            artifacts: vec![Artifact {
                url: "https://example.invalid/agent-2.0.0.zip".into(),
                mirror: None,
                sha256: "ab".repeat(32).into(),
                size: 5000,
                format: Format::Zip,
                unpacked: 39,
                decompressed: 39,
                files: Files::Listed(vec![file("agent", 18, true), file("harness", 21, true)]),
                kept: Vec::new(),
                replaced_by: None,
            }],
            launch: Launch::Binary,
        },
    }
}

/// `offered`, with its one archive changed by `change`.
fn archived(change: impl FnOnce(&mut Artifact)) -> OwnedPin {
    let mut pin = offered();
    if let Some(archive) = pin.target.artifacts.first_mut() {
        change(archive);
    }
    pin
}

#[test]
fn a_release_of_one_zip_of_two_files_is_lent_as_a_pin() {
    let held = offered();

    let lent = held.pin();

    assert_eq!(
        lent,
        Some(Pin {
            version: "2.0.0",
            url: "https://example.invalid/agent-2.0.0.zip",
            sha256: &"ab".repeat(32),
            archive: 5000,
            executable: Member {
                name: "agent",
                bytes: 18,
            },
            companion: Member {
                name: "harness",
                bytes: 21,
            },
        })
    );
}

#[test]
fn the_program_is_the_file_the_target_starts_whichever_the_list_gives_first() {
    let held = archived(|archive| {
        archive.files = Files::Listed(vec![file("harness", 21, true), file("agent", 18, true)]);
    });

    let lent = held.pin();

    assert_eq!(
        lent.map(|pin| (pin.executable.name, pin.companion.name)),
        Some(("agent", "harness"))
    );
}

#[test]
fn a_release_of_any_other_shape_is_not_a_pin() {
    let one = || file("agent", 18, true);
    let two = || file("harness", 21, true);
    let shapes: Vec<(&str, OwnedPin)> = vec![
        (
            "a tarball",
            archived(|archive| archive.format = Format::TarGz),
        ),
        (
            "a wheel",
            archived(|archive| archive.format = Format::Wheel),
        ),
        (
            "a bare program",
            archived(|archive| archive.format = Format::Bare),
        ),
        (
            "an archive built at review",
            archived(|archive| {
                archive.replaced_by = Some(AssetRef {
                    sha256: "cd".repeat(32).into(),
                    size: 9,
                });
            }),
        ),
        (
            "a list kept as an asset",
            archived(|archive| {
                archive.files = Files::Asset(AssetRef {
                    sha256: "cd".repeat(32).into(),
                    size: 9,
                });
            }),
        ),
        (
            "a single file",
            archived(|archive| archive.files = Files::Listed(vec![one()])),
        ),
        (
            "three files",
            archived(|archive| {
                archive.files = Files::Listed(vec![one(), two(), file("third", 1, false)]);
            }),
        ),
        (
            "a directory in place of the companion",
            archived(|archive| {
                archive.files = Files::Listed(vec![
                    one(),
                    File {
                        path: "harness".into(),
                        kind: FileKind::Dir,
                    },
                ]);
            }),
        ),
        (
            "a link in place of the companion",
            archived(|archive| {
                archive.files = Files::Listed(vec![
                    one(),
                    File {
                        path: "harness".into(),
                        kind: FileKind::Link {
                            target: "agent".into(),
                        },
                    },
                ]);
            }),
        ),
        (
            "a program the list does not name",
            archived(|archive| {
                archive.files = Files::Listed(vec![two(), file("third", 1, true)]);
            }),
        ),
        (
            "a file named twice",
            archived(|archive| archive.files = Files::Listed(vec![one(), one()])),
        ),
        (
            "a file below a directory",
            archived(|archive| {
                archive.files = Files::Listed(vec![one(), file("lib/harness", 21, true)]);
            }),
        ),
        ("no archive", {
            let mut held = offered();
            held.target.artifacts.clear();
            held
        }),
        ("two archives", {
            let mut held = offered();
            let again = held.target.artifacts.clone();
            held.target.artifacts.extend(again);
            held
        }),
        ("an interpreter that runs the agent", {
            let mut held = offered();
            held.target.launch = Launch::Uvx {
                interpreter_args: Vec::new(),
                entry_point: "agent:main".into(),
            };
            held
        }),
    ];

    for (what, held) in shapes {
        assert_eq!(held.pin(), None, "{what}");
    }
}

/// `offered`, with its archive's address changed to `url`.
fn at(url: &str) -> OwnedPin {
    archived(|archive| archive.url = url.into())
}

/// A release is downloaded only from a host its target lists, so an address in
/// a catalog cannot send the installer anywhere the target did not name.
#[test]
fn a_release_is_a_pin_only_from_an_address_on_a_host_its_target_lists() {
    for (what, url) in [
        ("another host", "https://elsewhere.invalid/agent.zip"),
        (
            "a host that only ends like it",
            "https://notexample.invalid/agent.zip",
        ),
        (
            "a user in front of the host",
            "https://example.invalid@elsewhere.invalid/agent.zip",
        ),
        ("a port", "https://example.invalid:8443/agent.zip"),
        ("the loopback", "https://127.0.0.1/agent.zip"),
        (
            "the metadata address",
            "https://169.254.169.254/latest/meta-data",
        ),
        ("a plain address", "http://example.invalid/agent.zip"),
        ("no host", "https:///agent.zip"),
    ] {
        assert_eq!(at(url).pin(), None, "{what}");
    }
    assert!(
        at("https://EXAMPLE.invalid/agent.zip").pin().is_some(),
        "case"
    );
    assert!(at("https://example.invalid").pin().is_some(), "no path");
    let mut unlisted = offered();
    unlisted.target.hosts.clear();
    assert_eq!(unlisted.pin(), None, "a target listing no host");
}

/// Sizes are held to the ceiling a release may have before anything is
/// downloaded or written for them.
#[test]
fn a_release_over_the_ceiling_is_not_a_pin() {
    let huge = || file("agent", super::RELEASE_BYTES, true);
    for (what, held) in [
        ("an archive", archived(|archive| archive.size = u64::MAX)),
        (
            "an archive over it",
            archived(|archive| archive.size = super::RELEASE_BYTES + 1),
        ),
        (
            "a member",
            archived(|archive| {
                archive.files = Files::Listed(vec![
                    file("agent", u64::MAX, true),
                    file("harness", 1, true),
                ]);
            }),
        ),
        (
            "members that sum over it",
            archived(|archive| {
                archive.files = Files::Listed(vec![huge(), file("harness", 1, true)]);
            }),
        ),
    ] {
        assert_eq!(held.pin(), None, "{what}");
    }
    let at_it = archived(|archive| {
        archive.size = super::RELEASE_BYTES;
        archive.files = Files::Listed(vec![
            file("agent", super::RELEASE_BYTES - 1, true),
            file("harness", 1, true),
        ]);
    });
    assert!(at_it.pin().is_some());
}

/// A member's name is held to the rules the unpacker holds every name to.
#[test]
fn a_release_whose_member_names_break_the_unpackers_rules_is_not_a_pin() {
    for (what, program) in [
        ("an escape sequence", "agent\u{1b}[2J"),
        ("a newline", "agent\nx"),
        ("a nul", "agent\0"),
        ("a trailing dot", "agent."),
        ("a trailing space", "agent "),
        ("a device name", "con"),
        ("a device name with an extension", "NUL.txt"),
        ("a character Windows refuses", "ag*nt"),
        ("a non-ASCII letter", "agént"),
        ("a dot", "."),
        ("two dots", ".."),
        ("an overlong name", &"a".repeat(300)),
    ] {
        let held = {
            let mut held = offered();
            held.target.cmd = program.into();
            if let Some(archive) = held.target.artifacts.first_mut() {
                archive.files =
                    Files::Listed(vec![file(program, 18, true), file("harness", 21, true)]);
            }
            held
        };
        assert_eq!(held.pin(), None, "{what}");
    }
    let colliding = archived(|archive| {
        archive.files = Files::Listed(vec![file("agent", 18, true), file("AGENT", 21, true)]);
    });
    assert_eq!(colliding.pin(), None, "names that differ only in case");
}

/// `bytes` kept in a file of their own under `owned`, opened to be read.
fn kept(owned: &Owned, name: &str, bytes: &[u8]) -> fs::File {
    let path = owned.0.join(name);
    fs::write(&path, bytes).expect("the download");
    fs::File::open(&path).expect("the download opened")
}

#[test]
fn an_artifact_read_from_its_file_unpacks_as_its_bytes_do_and_is_held_to_its_pin() {
    let owned = Owned::new("catalog-from-file");
    let zip = zip_of(&[
        Zipped::Dir("bin/"),
        Zipped::File("bin/agent", EXECUTABLE, 0o755),
    ]);
    let zip_files = vec![listed_dir("bin"), listed("bin/agent", EXECUTABLE, true)];
    let tarred = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let tar_files = vec![listed("agent", EXECUTABLE, true)];
    let bare_files = vec![listed("agent-linux-x64", EXECUTABLE, true)];
    for (index, (bytes, format, files)) in [
        (zip.clone(), Format::Zip, zip_files),
        (tarred.clone(), Format::TarGz, tar_files),
        (EXECUTABLE.to_vec(), Format::Bare, bare_files),
    ]
    .into_iter()
    .enumerate()
    {
        let artifact = pinned_as(&bytes, format, files.clone());
        let from_bytes = staging(&owned, &format!("bytes-{index}"));
        let from_file = staging(&owned, &format!("file-{index}"));
        let mut file = kept(&owned, &format!("download-{index}"), &bytes);

        let read = Unpacker::unpack_from(&mut file, &artifact, &from_file).expect("unpacked");
        let held = Unpacker::unpack(&bytes, &artifact, &from_bytes).expect("unpacked");

        assert_eq!(read.files(), held.files(), "{format:?}");
        assert_eq!(read.files(), files.as_slice(), "{format:?}");
        assert_eq!(tree(&from_file), tree(&from_bytes), "{format:?}");
    }

    // Another byte, a byte more, or a gzip stream with something after it.
    let artifact = pinned_as(&zip, Format::Zip, vec![]);
    let mut other = zip.clone();
    if let Some(last) = other.last_mut() {
        *last ^= 1;
    }
    let refused = Unpacker::unpack_from(
        &mut kept(&owned, "other", &other),
        &artifact,
        &staging(&owned, "other-out"),
    );
    assert!(matches!(refused, Err(UnpackError::Digest)), "{refused:?}");
    let mut longer = zip.clone();
    longer.push(0);
    let refused = Unpacker::unpack_from(
        &mut kept(&owned, "longer", &longer),
        &artifact,
        &staging(&owned, "longer-out"),
    );
    assert!(
        matches!(refused, Err(UnpackError::Size { .. })),
        "{refused:?}"
    );
    let mut trailing = tarred.clone();
    trailing.extend_from_slice(b"more");
    let artifact = pinned_as(
        &trailing,
        Format::TarGz,
        vec![listed("agent", EXECUTABLE, true)],
    );
    let refused = Unpacker::unpack_from(
        &mut kept(&owned, "trailing", &trailing),
        &artifact,
        &staging(&owned, "trailing-out"),
    );
    assert!(
        matches!(refused, Err(UnpackError::Corrupt(_))),
        "{refused:?}"
    );
}

/// The `.dist-info` directory every wheel a test writes describes itself in.
const DIST_INFO: &str = "fast_agent-1.0.dist-info";

/// A module of the agent a wheel installs.
const MODULE: &[u8] = b"def main():\n    return 0\n";

/// The members of a wheel that installs its root, its `purelib` and its
/// `platlib`, before its `RECORD`.
const WHEEL: [(&str, &[u8]); 5] = [
    ("fast_agent/__init__.py", b""),
    ("fast_agent/cli.py", MODULE),
    (
        "fast_agent-1.0.data/purelib/fast_agent/acp.py",
        b"ACP = 1\n",
    ),
    (
        "fast_agent-1.0.data/platlib/fast_agent/_speed.so",
        b"\x7fELF",
    ),
    ("fast_agent-1.0.dist-info/METADATA", b"Name: fast-agent\n"),
];

/// `name`'s row in a wheel's `RECORD`, for `bytes`.
fn row(name: &str, bytes: &[u8]) -> String {
    format!(
        "{name},sha256={},{}\n",
        URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)),
        bytes.len()
    )
}

/// The `RECORD` of a wheel of `members`: a row for each, then its own, which
/// gives no hash and no size.
fn record_of(members: &[(&str, &[u8])]) -> String {
    let mut record: String = members
        .iter()
        .map(|(name, bytes)| row(name, bytes))
        .collect();
    record.push_str(DIST_INFO);
    record.push_str("/RECORD,,\n");
    record
}

/// A wheel of `members`, each a file made with mode `0644`, and then
/// `record` as its `RECORD`.
fn wheel_with(members: &[(&str, &[u8])], record: &str) -> Vec<u8> {
    let name = format!("{DIST_INFO}/RECORD");
    let mut zipped: Vec<Zipped<'_>> = members
        .iter()
        .map(|(name, bytes)| Zipped::File(name, bytes, 0o644))
        .collect();
    zipped.push(Zipped::File(&name, record.as_bytes(), 0o644));
    zip_of(&zipped)
}

/// A wheel of `members` with the `RECORD` that agrees with them.
fn wheel_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    wheel_with(members, &record_of(members))
}

/// The list a wheel of `members` at its root and `record` installs as.
fn installed(members: &[(&str, &[u8])], record: &str) -> Vec<File> {
    members
        .iter()
        .map(|(name, bytes)| listed(&format!("site/{name}"), bytes, false))
        .chain([listed(
            &format!("site/{DIST_INFO}/RECORD"),
            record.as_bytes(),
            false,
        )])
        .collect()
}

/// The wheel `wheel` is pinned as for a survey, which may write a mebibyte.
fn surveyed_as(wheel: &[u8]) -> Artifact {
    Artifact {
        unpacked: 1 << 20,
        ..pinned_as(wheel, Format::Wheel, Vec::new())
    }
}

/// `wheel`, as `artifact` describes it, unpacked against its list and
/// surveyed, each into a staging directory of its own, and what each left
/// there.
fn wheel_both_ways(
    owned: &Owned,
    wheel: &[u8],
    artifact: &Artifact,
) -> [(Result<Staged, UnpackError>, Vec<String>); 2] {
    let listed = staging(owned, "listed");
    let surveyed = staging(owned, "surveyed");
    let unpacked = Wheels::unpack(wheel, artifact, &listed);
    let survey = Wheels::survey(wheel, artifact, &surveyed);
    let both = [(unpacked, tree(&listed)), (survey, tree(&surveyed))];
    let _ = fs::remove_dir_all(listed);
    let _ = fs::remove_dir_all(surveyed);
    both
}

#[test]
fn a_wheel_installs_its_root_purelib_and_platlib_under_site_as_its_survey_records() {
    let owned = Owned::new("wheel-site");
    let wheel = wheel_of(&WHEEL);
    let record = record_of(&WHEEL);
    let described = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(&wheel, Format::Wheel, Vec::new())
    };

    let surveyed =
        Wheels::survey(&wheel, &described, &staging(&owned, "surveyed")).expect("surveyed");

    assert_eq!(
        surveyed.files(),
        [
            listed("site/fast_agent/__init__.py", b"", false),
            listed("site/fast_agent/cli.py", MODULE, false),
            listed("site/fast_agent/acp.py", b"ACP = 1\n", false),
            listed("site/fast_agent/_speed.so", b"\x7fELF", false),
            listed(
                "site/fast_agent-1.0.dist-info/METADATA",
                b"Name: fast-agent\n",
                false
            ),
            listed(
                "site/fast_agent-1.0.dist-info/RECORD",
                record.as_bytes(),
                false
            ),
        ]
    );
    let recorded = Artifact {
        unpacked: surveyed.unpacked(),
        decompressed: surveyed.decompressed(),
        files: Files::Listed(surveyed.files().to_vec()),
        ..described
    };
    let at = staging(&owned, "unpacked");
    let unpacked = Wheels::unpack(&wheel, &recorded, &at).expect("unpacked");
    assert_eq!(unpacked.files(), surveyed.files());
    assert_eq!(
        tree(&at),
        [
            "site/",
            "site/fast_agent-1.0.dist-info/",
            "site/fast_agent-1.0.dist-info/METADATA",
            "site/fast_agent-1.0.dist-info/RECORD",
            "site/fast_agent/",
            "site/fast_agent/__init__.py",
            "site/fast_agent/_speed.so",
            "site/fast_agent/acp.py",
            "site/fast_agent/cli.py",
        ],
        "nothing of the .data directory is written as it is named"
    );
    let from = staging(&owned, "from-file");
    let read = Wheels::unpack_from(&mut kept(&owned, "wheel", &wheel), &recorded, &from)
        .expect("unpacked from its file");
    assert_eq!(read.files(), surveyed.files());

    let refused = Unpacker::unpack(&wheel, &recorded, &staging(&owned, "archive"));
    assert!(matches!(refused, Err(UnpackError::Wheel)), "{refused:?}");
    let zip = Artifact {
        format: Format::Zip,
        ..recorded
    };
    let refused = Wheels::unpack(&wheel, &zip, &staging(&owned, "zip"));
    assert!(matches!(refused, Err(UnpackError::NotWheel)), "{refused:?}");
}

#[test]
fn a_wheel_is_held_to_its_pin_before_a_byte_of_it_is_read() {
    let owned = Owned::new("wheel-pin");
    let wheel = wheel_of(&WHEEL[..2]);
    let artifact = pinned_as(&wheel, Format::Wheel, Vec::new());
    let mut other = wheel.clone();
    if let Some(last) = other.last_mut() {
        *last ^= 1;
    }
    let refused = Wheels::unpack(&other, &artifact, &staging(&owned, "bytes"));
    assert!(matches!(refused, Err(UnpackError::Digest)), "{refused:?}");
    let refused = Wheels::survey(&other, &artifact, &staging(&owned, "survey"));
    assert!(matches!(refused, Err(UnpackError::Digest)), "{refused:?}");
    let refused = Wheels::unpack_from(
        &mut kept(&owned, "other", &other),
        &artifact,
        &staging(&owned, "file"),
    );
    assert!(matches!(refused, Err(UnpackError::Digest)), "{refused:?}");
    assert!(tree(&owned.0.join("bytes")).is_empty());
    assert!(tree(&owned.0.join("file")).is_empty());
}

#[test]
fn a_wheel_with_another_part_of_its_data_directory_is_refused_naming_the_part() {
    let owned = Owned::new("wheel-data");
    for part in ["scripts", "headers", "data"] {
        let name = format!("fast_agent-1.0.data/{part}/fast-agent");
        let members: [(&str, &[u8]); 2] = [(&name, b"#!python\n"), ("fast_agent/__init__.py", b"")];
        let record = record_of(&members);
        let wheel = wheel_with(&members, &record);
        let artifact = pinned_as(&wheel, Format::Wheel, installed(&members[1..], &record));
        for (refused, left) in wheel_both_ways(&owned, &wheel, &artifact) {
            assert!(
                matches!(&refused, Err(UnpackError::Data { part: named, .. }) if named == part),
                "{part}: {refused:?}"
            );
            assert!(left.is_empty(), "nothing is written before it: {left:?}");
            let said = refused
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert!(said.contains(part), "{said}");
        }
    }
}

#[test]
fn two_members_a_wheel_installs_on_one_path_are_refused() {
    let owned = Owned::new("wheel-twice");
    let members: [(&str, &[u8]); 2] = [
        ("fast_agent/acp.py", b"ACP = 1\n"),
        (
            "fast_agent-1.0.data/purelib/fast_agent/acp.py",
            b"ACP = 2\n",
        ),
    ];
    let record = record_of(&members);
    let wheel = wheel_with(&members, &record);
    let artifact = pinned_as(&wheel, Format::Wheel, installed(&members[..1], &record));
    for (refused, _) in wheel_both_ways(&owned, &wheel, &artifact) {
        assert!(
            matches!(refused, Err(UnpackError::Twice { .. })),
            "{refused:?}"
        );
    }
    let members: [(&str, &[u8]); 2] = [
        ("fast_agent/ACP.py", b"ACP = 1\n"),
        (
            "fast_agent-1.0.data/platlib/fast_agent/acp.py",
            b"ACP = 2\n",
        ),
    ];
    let wheel = wheel_of(&members);
    let artifact = surveyed_as(&wheel);
    let refused = Wheels::survey(&wheel, &artifact, &staging(&owned, "case"));
    assert!(
        matches!(refused, Err(UnpackError::Collision { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_wheel_whose_record_disagrees_with_what_it_holds_is_refused() {
    let owned = Owned::new("wheel-record");
    let members = &WHEEL[..2];
    let good = record_of(members);
    let own = format!("{DIST_INFO}/RECORD,,\n");
    let cli = row("fast_agent/cli.py", MODULE);
    let init = row("fast_agent/__init__.py", b"");
    let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(MODULE));
    let cases: [(&str, String, Option<&str>); 9] = [
        (
            "a member with no row",
            format!("{init}{own}"),
            Some("fast_agent/cli.py"),
        ),
        (
            "a row for no member",
            format!("{good}{}", row("fast_agent/gone.py", b"gone")),
            Some("fast_agent/gone.py"),
        ),
        (
            "a member's row twice",
            format!("{init}{cli}{cli}{own}"),
            Some("fast_agent/cli.py"),
        ),
        (
            "another algorithm",
            format!("{init}fast_agent/cli.py,md5=AAAA,{}\n{own}", MODULE.len()),
            Some("fast_agent/cli.py"),
        ),
        (
            "another member's hash",
            format!("{init}{}{own}", row("fast_agent/cli.py", b"other")).replace(
                &format!(",{}\n{own}", b"other".len()),
                &format!(",{}\n{own}", MODULE.len()),
            ),
            Some("fast_agent/cli.py"),
        ),
        (
            "a hash that is not base64",
            format!("{init}fast_agent/cli.py,sha256=!!,{}\n{own}", MODULE.len()),
            Some("fast_agent/cli.py"),
        ),
        (
            "another size",
            format!(
                "{init}fast_agent/cli.py,sha256={digest},{}\n{own}",
                MODULE.len() + 1
            ),
            Some("fast_agent/cli.py"),
        ),
        (
            "a row that is not three fields",
            format!("{good}a row\n"),
            None,
        ),
        (
            "a row longer than a row may be",
            format!("{good}{},,\n", "a".repeat(8192)),
            None,
        ),
    ];
    for (case, record, named) in cases {
        let wheel = wheel_with(members, &record);
        let artifact = pinned_as(&wheel, Format::Wheel, installed(members, &record));
        for (refused, _) in wheel_both_ways(&owned, &wheel, &artifact) {
            match named {
                Some(named) => assert!(
                    matches!(&refused, Err(UnpackError::Recorded { name, .. }) if name == named),
                    "{case}: {refused:?}"
                ),
                None => assert!(
                    matches!(refused, Err(UnpackError::Record { .. })),
                    "{case}: {refused:?}"
                ),
            }
        }
    }
    let unrecorded = zip_of(&[Zipped::File("fast_agent/cli.py", MODULE, 0o644)]);
    let artifact = surveyed_as(&unrecorded);
    let refused = Wheels::survey(&unrecorded, &artifact, &staging(&owned, "unrecorded"));
    assert!(
        matches!(refused, Err(UnpackError::Record { .. })),
        "no RECORD: {refused:?}"
    );
    let twice = zip_of(&[
        Zipped::File("fast_agent/cli.py", MODULE, 0o644),
        Zipped::File("one-1.0.dist-info/RECORD", good.as_bytes(), 0o644),
        Zipped::File("two-1.0.dist-info/RECORD", good.as_bytes(), 0o644),
    ]);
    let artifact = surveyed_as(&twice);
    let refused = Wheels::survey(&twice, &artifact, &staging(&owned, "twice"));
    assert!(
        matches!(refused, Err(UnpackError::Record { .. })),
        "two RECORDs: {refused:?}"
    );
}

#[test]
fn a_wheel_member_on_a_path_another_artifact_wrote_is_refused() {
    let owned = Owned::new("wheel-overlap");
    let at = staging(&owned, "version");
    let interpreter = tar_gz(&[
        Tarred::Dir("site/"),
        Tarred::File("site/sitecustomize.py", b"import os\n", 0o644),
    ]);
    let artifact = pinned_as(
        &interpreter,
        Format::TarGz,
        vec![
            listed_dir("site"),
            listed("site/sitecustomize.py", b"import os\n", false),
        ],
    );
    Unpacker::unpack(&interpreter, &artifact, &at).expect("the interpreter");
    let members: [(&str, &[u8]); 1] = [("sitecustomize.py", b"import sys\n")];
    let record = record_of(&members);
    let wheel = wheel_with(&members, &record);
    let artifact = pinned_as(&wheel, Format::Wheel, installed(&members, &record));

    let refused = Wheels::unpack(&wheel, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Write { .. })),
        "{refused:?}"
    );
    assert_eq!(
        fs::read(at.join("site").join("sitecustomize.py")).ok(),
        Some(b"import os\n".to_vec()),
        "the interpreter's file is as it wrote it"
    );
}

/// The launcher a `uvx` release named `fast-agent` with the entry point
/// `fast_agent.cli:main` is started through, to the byte.
const LAUNCH_PY: &str = r#"# Written by crucible: the pinned interpreter runs this file, which starts
# the agent from the wheels unpacked into site/ beside it.
import importlib
import os
import site
import sys

AGENT = "fast-agent"
ENTRY_POINT = "fast_agent.cli:main"

here = os.path.dirname(os.path.abspath(__file__))
site.addsitedir(os.path.join(here, "site"))
sys.argv = [AGENT] + sys.argv[1:]
module, _, attribute = ENTRY_POINT.partition(":")
target = importlib.import_module(module)
for part in attribute.split("."):
    target = getattr(target, part)
sys.exit(target())
"#;

/// What the catalog gives the interpreter a launcher is written for.
fn isolating() -> Vec<Box<str>> {
    vec!["-I".into()]
}

#[test]
fn the_launcher_is_a_fixed_template_holding_only_the_key_and_the_entry_point() {
    let key = AgentKey::new("fast-agent").expect("a key");
    let script = LaunchScript::new(&key, &isolating(), "fast_agent.cli:main").expect("a launcher");
    assert_eq!(std::str::from_utf8(script.bytes()), Ok(LAUNCH_PY));
    assert_eq!(script.file(), listed(LAUNCHER, LAUNCH_PY.as_bytes(), false));
    assert_eq!(LAUNCHER, "launch.py");

    let owned = Owned::new("wheel-launcher");
    let at = staging(&owned, "version");
    assert_eq!(script.write(&at).ok(), Some(script.file()));
    assert_eq!(
        fs::read(at.join(LAUNCHER)).ok(),
        Some(LAUNCH_PY.as_bytes().to_vec())
    );
    #[cfg(unix)]
    assert!(!can_run(&at.join(LAUNCHER)), "the launcher is not runnable");
    let refused = script.write(&at);
    assert!(
        matches!(refused, Err(UnpackError::Write { .. })),
        "a launcher already there, or a file an artifact put there, is never written over: \
         {refused:?}"
    );
}

#[test]
fn an_entry_point_that_is_not_dotted_identifiers_on_each_side_is_refused() {
    let key = AgentKey::new("agent").expect("a key");
    for accepted in ["main:run", "fast_agent.cli:main", "_a.b9:C.d_", "A:b.c.d"] {
        let launcher = LaunchScript::new(&key, &isolating(), accepted);
        assert!(launcher.is_ok(), "{accepted}: {launcher:?}");
    }
    for refused in [
        "", "main", ":main", "main:", "a::b", "a:b:c", "a..b:c", ".a:b", "a.:b", "a:b.", "9a:b",
        "a:9b", "a-b:c", "a b:c", "a:b\"", "a:b\\", "a:b\n", "a:b'", "a:b()", "a:b # c",
        "\u{e9}:b",
    ] {
        let launcher = LaunchScript::new(&key, &isolating(), refused);
        assert!(
            matches!(launcher, Err(UnpackError::EntryPoint)),
            "{refused:?}: {launcher:?}"
        );
    }
    let said = LaunchScript::new(&key, &isolating(), "evil\"value:x")
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        said.contains("entry_point") && !said.contains("evil"),
        "the refusal names the field and not its value: {said}"
    );
}

/// A launcher is written only for an interpreter the catalog gives exactly
/// `-I`, which starts it isolated with the launcher the first thing it runs.
/// Anything beside it is refused: an argument after which `-I` is not read
/// as an option (`-c`, `-m`, a file to run), one it is followed by, a flag
/// added, a flag run together with it, or none at all.
#[test]
fn a_launcher_is_written_only_for_an_interpreter_given_exactly_dash_i() {
    let key = AgentKey::new("agent").expect("a key");
    let given = |arguments: &[&str]| -> Vec<Box<str>> {
        arguments
            .iter()
            .map(|argument| (*argument).into())
            .collect()
    };
    let accepted: [&[&str]; 1] = [&["-I"]];
    let refused: [&[&str]; 15] = [
        &["-c", "x", "-I"],
        &["-m", "x", "-I"],
        &["acp", "-I"],
        &["-I", "acp"],
        &["-I", "-c", "x"],
        &["-B", "-I"],
        &["-I", "-u"],
        &["-I", "-I"],
        &[],
        &["-i"],
        &["-E", "-s"],
        &["-Is"],
        &["-I "],
        &["--I"],
        &["I"],
    ];

    let written: Vec<bool> = accepted
        .iter()
        .chain(&refused)
        .map(
            |arguments| match LaunchScript::new(&key, &given(arguments), "main:run") {
                Ok(_) => true,
                Err(UnpackError::Unisolated) => false,
                Err(other) => panic!("{arguments:?}: {other:?}"),
            },
        )
        .collect();
    let said = LaunchScript::new(&key, &given(&["-evil"]), "main:run")
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();

    let expected: Vec<bool> = accepted
        .iter()
        .map(|_| true)
        .chain(refused.iter().map(|_| false))
        .collect();
    assert_eq!(written, expected);
    assert!(
        said.contains("interpreter_args") && !said.contains("evil"),
        "the refusal names the field and not its value: {said}"
    );
}

#[test]
fn every_character_an_agent_key_may_hold_needs_no_escaping_in_a_python_string() {
    let mut allowed = 0;
    for byte in 0_u8..=0xff {
        let character = char::from(byte);
        if AgentKey::new(&format!("a{character}")).is_ok() {
            allowed += 1;
            assert!(
                byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\'' | b'\\'),
                "{byte:#04x} would need escaping in the launcher's string"
            );
        }
    }
    assert_eq!(
        allowed,
        26 + 10 + 1,
        "lowercase letters, digits and a hyphen"
    );
}
