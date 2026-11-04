//! A bzip2-compressed tar is unpacked and surveyed by the same rules as its
//! gzip-compressed sibling: the same members, paths, size and decompression
//! ceilings, and the same refusals for a stream cut short, a member that
//! climbs out, or a stream that runs past a ceiling.

use super::*;

/// `bytes` as one bzip2 stream.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn bzipped(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
    encoder.write_all(bytes).expect("its bytes");
    encoder.finish().expect("a bzip2 stream")
}

/// A bzip2-compressed tar of `members`, in that order.
fn tar_bz2(members: &[Tarred<'_>]) -> Vec<u8> {
    bzipped(&tarred(members))
}

/// A `.tar.bz2` unpacks and surveys to the same files as its `.tar.gz`
/// sibling, through the streamed survey as well as from memory.
#[test]
fn a_tar_bz2_unpacks_and_surveys_as_its_tar_gz_sibling() {
    let owned = Owned::new("catalog-bz2");
    let members = [
        Tarred::File("agent", EXECUTABLE, 0o755),
        Tarred::Dir("lib/"),
        Tarred::File("lib/harness", COMPANION, 0o644),
        Tarred::Symlink("lib/current", "harness"),
    ];
    let files = vec![
        listed("agent", EXECUTABLE, true),
        listed_dir("lib"),
        listed("lib/harness", COMPANION, false),
        listed_link("lib/current", "harness"),
    ];
    let gzip = tar_gz(&members);
    let bzip = tar_bz2(&members);
    let gzip_artifact = pinned_as(&gzip, Format::TarGz, files.clone());
    let bzip_artifact = pinned_as(&bzip, Format::TarBz2, files.clone());

    let gzip_at = staging(&owned, "gzip");
    let bzip_at = staging(&owned, "bzip");
    let gzipped_staged =
        Unpacker::unpack(&gzip, &gzip_artifact, &gzip_at).expect("the gzip sibling");
    let bzipped_staged =
        Unpacker::unpack(&bzip, &bzip_artifact, &bzip_at).expect("the bzip2 archive");

    assert_eq!(bzipped_staged.files(), gzipped_staged.files());
    assert_eq!(bzipped_staged.files(), files.as_slice());
    assert_eq!(bzipped_staged.unpacked(), gzipped_staged.unpacked());
    assert_eq!(bzipped_staged.decompressed(), gzipped_staged.decompressed());
    assert_eq!(tree(&bzip_at), tree(&gzip_at));

    let path = owned.0.join("agent.tar.bz2");
    fs::write(&path, &bzip).expect("the archive");
    let streamed_at = staging(&owned, "streamed");
    let streamed = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the archive"),
        &bzip_artifact,
        &streamed_at,
    )
    .expect("surveyed from its file");
    let memory_at = staging(&owned, "memory");
    let memory = Unpacker::survey(&bzip, &bzip_artifact, &memory_at).expect("surveyed in memory");

    assert_eq!(streamed.files(), memory.files());
    assert_eq!(streamed.unpacked(), memory.unpacked());
    assert_eq!(streamed.decompressed(), memory.decompressed());
    assert_eq!(tree(&streamed_at), tree(&memory_at));
}

/// A bzip2 stream that ends before its archive does is refused, and nothing of
/// it is written.
#[test]
fn a_truncated_tar_bz2_is_refused_and_writes_nothing() {
    let owned = Owned::new("catalog-bz2-truncated");
    let archive = tar_bz2(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let cut = archive.get(..4).expect("the bzip2 header alone");
    let artifact = pinned_as(cut, Format::TarBz2, vec![listed("agent", EXECUTABLE, true)]);
    let at = staging(&owned, "staging");

    let refused = Unpacker::survey(cut, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Corrupt(_))),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));
}

/// A member whose name climbs out of the staging directory is refused before
/// anything is written.
#[test]
fn a_tar_bz2_member_that_climbs_out_is_refused() {
    let owned = Owned::new("catalog-bz2-escape");
    let archive = tar_bz2(&[Tarred::File("../agent", EXECUTABLE, 0o755)]);
    let artifact = pinned_as(&archive, Format::TarBz2, Vec::new());
    let at = staging(&owned, "staging");

    let refused = Unpacker::survey(&archive, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Name { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));
}

/// A bzip2 stream whose member would take what it unpacks to past its ceiling
/// is refused before the member is read, and so is one that would decompress
/// past its ceiling.
#[test]
fn a_tar_bz2_stream_past_its_unpacked_or_decompressed_ceiling_is_refused() {
    let owned = Owned::new("catalog-bz2-ceiling");
    let archive = tar_bz2(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let unpacked = u64::try_from(EXECUTABLE.len()).expect("a size");

    let past_unpacked = Artifact {
        unpacked: unpacked - 1,
        ..pinned_as(&archive, Format::TarBz2, Vec::new())
    };
    let at = staging(&owned, "unpacked");
    let refused = Unpacker::survey(&archive, &past_unpacked, &at);
    assert!(
        matches!(refused, Err(UnpackError::Unpacked { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));

    let past_decompressed = Artifact {
        unpacked: 1 << 20,
        decompressed: 10,
        ..pinned_as(&archive, Format::TarBz2, Vec::new())
    };
    let at = staging(&owned, "decompressed");
    let refused = Unpacker::survey(&archive, &past_decompressed, &at);
    assert!(
        matches!(refused, Err(UnpackError::Decompressed { .. })),
        "{refused:?}"
    );
    assert!(tree(&at).is_empty(), "{:?}", tree(&at));
}

/// A bzip2 archive larger than the 512 MiB compressed cap is refused before
/// its file is read, as the download path refuses one.
#[test]
fn a_streamed_survey_refuses_a_tar_bz2_archive_past_the_compressed_cap() {
    let owned = Owned::new("catalog-bz2-cap");
    let archive = tar_bz2(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    let artifact = Artifact {
        size: crate::release::ARCHIVE_BYTES + 1,
        ..pinned_as(&archive, Format::TarBz2, Vec::new())
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

/// Bytes after the one bzip2 stream are refused, as they are after a gzip
/// stream.
#[test]
fn a_tar_bz2_with_something_after_its_stream_is_refused() {
    let owned = Owned::new("catalog-bz2-trailing");
    let mut archive = tar_bz2(&[Tarred::File("agent", EXECUTABLE, 0o755)]);
    archive.extend_from_slice(b"more");
    let artifact = pinned_as(
        &archive,
        Format::TarBz2,
        vec![listed("agent", EXECUTABLE, true)],
    );
    let at = staging(&owned, "staging");

    let refused = Unpacker::survey(&archive, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Corrupt(_))),
        "{refused:?}"
    );
}

/// A `tar.bz2` artifact is the format a catalog spells, read back from the
/// same text the registry entry will give it.
#[test]
fn a_tar_bz2_artifact_is_the_format_the_catalog_spells() {
    let sha256 = "0".repeat(64);
    let text = format!(
        r#"{{"url":"https://example.invalid/goose.tar.bz2","sha256":"{sha256}","size":1,"format":"tar.bz2","unpacked":1,"decompressed":1}}"#
    );

    let artifact = Artifact::unsigned(text.as_bytes()).expect("a tar.bz2 artifact");

    assert_eq!(artifact.format, Format::TarBz2);
}
