//! A bzip2-compressed tar is unpacked and surveyed by the same rules as its
//! gzip-compressed sibling: the same members, paths, size and decompression
//! ceilings, and the same refusals for a stream cut short, a member that
//! climbs out, or a stream that runs past a ceiling.

use super::*;

/// A tar of [`fixture_files`], compressed once by the reference `bzip2` 1.0.8
/// implementation rather than by this crate's encoder, so the decoder is
/// held to a stream it did not write.
const COMMITTED_BZ2: &[u8] = include_bytes!("bz2/agent.tar.bz2");

/// The same tar [`COMMITTED_BZ2`] holds, compressed once by the reference
/// `gzip` implementation as its sibling.
const COMMITTED_GZ: &[u8] = include_bytes!("bz2/agent.tar.gz");

/// The four members both committed fixtures hold.
fn fixture_files() -> Vec<File> {
    vec![
        listed("agent", EXECUTABLE, true),
        listed_dir("lib"),
        listed("lib/harness", COMPANION, false),
        listed_link("lib/current", "harness"),
    ]
}

/// `bytes` as one bzip2 stream.
// A test that cannot build its archive has nothing to report.
#[allow(clippy::expect_used)]
fn bzipped(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
    encoder.write_all(bytes).expect("its bytes");
    encoder.finish().expect("a bzip2 stream")
}

/// A bzip2-compressed tar of `members`, in that order, for the hostile cases
/// that mutate its bytes.
fn tar_bz2(members: &[Tarred<'_>]) -> Vec<u8> {
    bzipped(&tarred(members))
}

/// The committed `.tar.bz2` unpacks and surveys to the same files as its
/// committed `.tar.gz` sibling, through the streamed survey as well as from
/// memory.
#[test]
fn a_committed_tar_bz2_unpacks_and_surveys_as_its_tar_gz_sibling() {
    let owned = Owned::new("catalog-bz2-committed");
    let files = fixture_files();
    let bzip_artifact = pinned_as(COMMITTED_BZ2, Format::TarBz2, files.clone());
    let gzip_artifact = pinned_as(COMMITTED_GZ, Format::TarGz, files.clone());

    let bzip_at = staging(&owned, "bzip");
    let gzip_at = staging(&owned, "gzip");
    let bzip_staged = Unpacker::unpack(COMMITTED_BZ2, &bzip_artifact, &bzip_at)
        .expect("the committed bzip2 fixture");
    let gzip_staged = Unpacker::unpack(COMMITTED_GZ, &gzip_artifact, &gzip_at)
        .expect("the committed gzip sibling");

    assert_eq!(bzip_staged.files(), gzip_staged.files());
    assert_eq!(bzip_staged.files(), files.as_slice());
    assert_eq!(bzip_staged.unpacked(), gzip_staged.unpacked());
    assert_eq!(bzip_staged.decompressed(), gzip_staged.decompressed());
    assert_eq!(tree(&bzip_at), tree(&gzip_at));

    let path = owned.0.join("committed.tar.bz2");
    fs::write(&path, COMMITTED_BZ2).expect("the fixture");
    let streamed_at = staging(&owned, "streamed");
    let streamed = Unpacker::survey_from(
        &mut fs::File::open(&path).expect("the fixture"),
        &bzip_artifact,
        &streamed_at,
    )
    .expect("surveyed from its file");
    let memory_at = staging(&owned, "memory");
    let memory =
        Unpacker::survey(COMMITTED_BZ2, &bzip_artifact, &memory_at).expect("surveyed in memory");

    assert_eq!(streamed.files(), memory.files());
    assert_eq!(streamed.unpacked(), memory.unpacked());
    assert_eq!(streamed.decompressed(), memory.decompressed());
    assert_eq!(tree(&streamed_at), tree(&memory_at));
}

/// A bzip2 stream that ends at its header is refused, and nothing of it is
/// written.
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

/// A bzip2 stream cut after the tar's last member and its end blocks, before
/// the stream's own footer, is refused: the decoder has already produced the
/// archive when it meets the cut.
#[test]
fn a_tar_bz2_cut_after_its_members_is_refused() {
    let owned = Owned::new("catalog-bz2-cut-after-members");
    let cut = COMMITTED_BZ2
        .get(..COMMITTED_BZ2.len() - 4)
        .expect("all but the stream footer");
    let artifact = pinned_as(cut, Format::TarBz2, fixture_files());
    let at = staging(&owned, "staging");

    let refused = Unpacker::survey(cut, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Corrupt(_))),
        "{refused:?}"
    );
    assert_eq!(
        tree(&at),
        ["agent", "lib/", "lib/harness"],
        "{:?}",
        tree(&at)
    );
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

/// A stream whose members all declare sizes within the ceiling still refuses
/// when its decompressed bytes pass that ceiling: the streamed budget stops
/// the read, not a member's own pre-check.
#[test]
fn a_tar_bz2_bomb_past_its_decompressed_ceiling_is_refused_by_the_stream_budget() {
    let owned = Owned::new("catalog-bz2-bomb");
    let artifact = Artifact {
        unpacked: 1 << 20,
        decompressed: 4096,
        ..pinned_as(COMMITTED_BZ2, Format::TarBz2, fixture_files())
    };
    let at = staging(&owned, "staging");

    let refused = Unpacker::survey(COMMITTED_BZ2, &artifact, &at);

    assert!(
        matches!(refused, Err(UnpackError::Decompressed { ceiling }) if ceiling == 4096),
        "{refused:?}"
    );
    assert!(
        tree(&at).iter().any(|name| name == "agent"),
        "the member within the ceiling was written before the stream was stopped: {:?}",
        tree(&at)
    );
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

/// Bytes after the one bzip2 stream are refused, and the refusal names the
/// bzip2 stream as the gzip one does.
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

    let refused = Unpacker::survey(&archive, &artifact, &at).expect_err("bytes after the stream");

    let UnpackError::Corrupt(error) = refused else {
        panic!("not a corrupt stream: {refused:?}");
    };
    assert!(
        error.to_string().contains("past its bzip2 stream"),
        "{error}"
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
