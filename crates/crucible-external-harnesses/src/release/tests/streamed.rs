//! A survey read from the file an archive was downloaded into records what
//! the survey in memory records, and is held to the same ceilings and the
//! same refusals.

use super::*;

/// A gzip-compressed tar whose one member declares `declared` bytes and holds
/// `holds`, so a stream that ends before what it declared can be made.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn lying(declared: u64, holds: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = Header::new_gnu();
    put(&mut header.as_old_mut().name, b"agent");
    header.set_entry_type(EntryType::Regular);
    header.set_size(declared);
    header.set_mode(0o755);
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, holds).expect("a member");
    gzipped(&builder.into_inner().expect("a tar"))
}

/// The registry's two bounds, and the release's, at the sizes the release
/// ships them at.
#[test]
fn a_registry_archive_is_bounded_at_512_mib_compressed_and_2_gib_unpacked() {
    assert_eq!(crate::release::ARCHIVE_BYTES, 512 << 20);
    assert_eq!(crate::release::UNPACKED_BYTES, 2 << 30);
    assert_eq!(crate::release::RELEASE_BYTES, 4 << 30);
}

/// Every format a registry archive may be is surveyed from a file as it is
/// from the bytes held in memory, and the two record the same list and sizes.
#[test]
fn a_streamed_survey_records_what_the_survey_in_memory_records() {
    let owned = Owned::new("catalog-streamed");
    let zip = zip_of(&[
        Zipped::File("agent", EXECUTABLE, 0o755),
        Zipped::File("LICENSE", b"MIT", 0o644),
    ]);
    let tar = tar_gz(&[
        Tarred::File("agent", EXECUTABLE, 0o755),
        Tarred::File("LICENSE", b"MIT", 0o644),
    ]);
    let formats = [
        (
            Format::Zip,
            zip,
            vec![
                listed("agent", EXECUTABLE, true),
                listed("LICENSE", b"MIT", false),
            ],
        ),
        (
            Format::TarGz,
            tar,
            vec![
                listed("agent", EXECUTABLE, true),
                listed("LICENSE", b"MIT", false),
            ],
        ),
        (Format::Bare, EXECUTABLE.to_vec(), Vec::new()),
    ];
    for (index, (format, archive, files)) in formats.into_iter().enumerate() {
        let mut artifact = pinned_as(&archive, format, files);
        artifact.unpacked = 1 << 20;
        let path = owned.0.join(format!("archive-{index}"));
        fs::write(&path, &archive).expect("the archive");
        let in_memory = staging(&owned, &format!("memory-{index}"));
        let streamed = staging(&owned, &format!("streamed-{index}"));

        let held = Unpacker::survey(&archive, &artifact, &in_memory).expect("surveyed in memory");
        let read = Unpacker::survey_from(
            &mut fs::File::open(&path).expect("the archive"),
            &artifact,
            &streamed,
        )
        .expect("surveyed from its file");

        assert_eq!(read.files(), held.files(), "{format:?}");
        assert_eq!(read.unpacked(), held.unpacked(), "{format:?}");
        assert_eq!(read.decompressed(), held.decompressed(), "{format:?}");
        assert_eq!(tree(&streamed), tree(&in_memory), "{format:?}");
    }
}

/// A file that ends before the bytes it was pinned at are all there is
/// refused, and nothing of it is written.
#[test]
fn a_streamed_survey_refuses_a_file_cut_short() {
    let owned = Owned::new("catalog-streamed-short");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let artifact = pinned_as(
        &archive,
        Format::TarGz,
        vec![listed("agent", EXECUTABLE, true)],
    );
    let path = owned.0.join("archive");
    fs::write(&path, archive.get(..archive.len() / 2).expect("half")).expect("the archive");
    let at = staging(&owned, "streamed");

    let refused = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the archive"),
        &artifact,
        &at,
    );

    assert!(
        matches!(refused, Err(UnpackError::Size { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));
}

/// A member that declares more than the stream holds is refused as it is
/// read, and what it read before the end is left under staging for its owner
/// to remove, as an unpack's refusal leaves it.
#[test]
fn a_streamed_survey_refuses_a_member_that_declares_more_than_the_stream_holds() {
    let owned = Owned::new("catalog-streamed-lie");
    let archive = lying(
        u64::try_from(EXECUTABLE.len() + 4096).expect("a size"),
        EXECUTABLE,
    );
    let artifact = Artifact {
        unpacked: 1 << 20,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let path = owned.0.join("archive");
    fs::write(&path, &archive).expect("the archive");
    let at = staging(&owned, "streamed");

    let refused = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the archive"),
        &artifact,
        &at,
    );

    assert!(
        matches!(
            refused,
            Err(UnpackError::Corrupt(ref error)) if error.kind() == std::io::ErrorKind::UnexpectedEof
        ),
        "{refused:?}"
    );
    // What it read before the stream ended is left under the staging
    // directory, for its owner to remove as an unpack's is.
    assert_eq!(tree(&at), ["agent"], "{:?}", tree(&at));
}

/// An artifact larger than the 512 MiB compressed cap is refused before its
/// file is read, as the download path refuses one.
#[test]
fn a_streamed_survey_refuses_an_archive_past_the_compressed_cap() {
    let owned = Owned::new("catalog-streamed-cap");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let artifact = Artifact {
        size: crate::release::ARCHIVE_BYTES + 1,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let path = owned.0.join("archive");
    fs::write(&path, &archive).expect("the archive");

    let refused = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the archive"),
        &artifact,
        &staging(&owned, "streamed"),
    );

    assert!(
        matches!(
            refused,
            Err(UnpackError::Archive { ceiling }) if ceiling == crate::release::ARCHIVE_BYTES
        ),
        "{refused:?}"
    );
}

/// An archive whose member would take what it unpacks to past its ceiling is
/// refused before the member is read, and so is one that would decompress
/// past its ceiling.
#[test]
fn a_streamed_survey_refuses_an_archive_past_its_unpacked_or_decompressed_ceiling() {
    let owned = Owned::new("catalog-streamed-ceiling");
    let archive = tar_gz(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let path = owned.0.join("archive");
    fs::write(&path, &archive).expect("the archive");
    let unpacked = u64::try_from(EXECUTABLE.len()).expect("a size");

    let past_unpacked = Artifact {
        unpacked: unpacked - 1,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let at = staging(&owned, "unpacked");
    let refused = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the archive"),
        &past_unpacked,
        &at,
    );
    assert!(
        matches!(refused, Err(UnpackError::Unpacked { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));

    let past_decompressed = Artifact {
        unpacked: 1 << 20,
        decompressed: 10,
        ..pinned_as(&archive, Format::TarGz, Vec::new())
    };
    let at = staging(&owned, "decompressed");
    let refused = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the archive"),
        &past_decompressed,
        &at,
    );
    assert!(
        matches!(refused, Err(UnpackError::Decompressed { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));
}
