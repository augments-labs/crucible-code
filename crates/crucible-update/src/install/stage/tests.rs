use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};
use tar::{Builder, EntryType, Header};

use super::*;
use crate::install::layout::{BROKER, CRUCIBLE, PREFIX, RECEIPT};
use crate::install::{Digest, Target};

/// The release each test install has active.
const ACTIVE: &str = "0.46.0";

/// The release each test stages.
const NEXT: &str = "0.47.0";

/// What the active unit's executable holds.
const ACTIVE_EXECUTABLE: &[u8] = b"crucible 0.46.0, as a release would ship it";

/// What the active unit's broker holds.
const ACTIVE_BROKER: &[u8] = b"crucible-sandbox-broker 0.46.0, as a release would ship it";

/// What the staged release's executable holds.
const EXECUTABLE: &[u8] = b"crucible 0.47.0, as a release would ship it";

/// What the staged release's broker holds.
const HELPER: &[u8] = b"crucible-sandbox-broker 0.47.0, as a release would ship it";

/// The installation each test install was made with.
const INSTALLATION: &str = "5c0f9d2e8a4b47e1b3d6a09f7c21e845";

/// A managed install made the way the installer lays one out, with a release
/// archive and its checksums beside it, in a directory of its own that is
/// deleted with the value.
struct Install {
    /// The directory the installer was given, canonical.
    dir: PathBuf,
}

impl Install {
    fn new(name: &str) -> Self {
        let at = std::env::temp_dir().join(format!("crucible-stage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&at);
        fs::create_dir_all(&at).expect("a temporary directory");
        let dir = at.canonicalize().expect("a canonical temporary directory");
        let install = Self { dir };
        directory(&install.prefix());
        directory(&install.releases());
        let unit = install.releases().join(ACTIVE);
        directory(&unit);
        file(&unit.join(CRUCIBLE), ACTIVE_EXECUTABLE, 0o755);
        file(&unit.join(BROKER), ACTIVE_BROKER, 0o755);
        let receipt = format!(
            "crucible-installer-receipt 1\n\
             manager=crucible-installer\n\
             installation={INSTALLATION}\n\
             target={}\n\
             layout=versioned\n\
             prefix={}\n\
             version={ACTIVE}\n\
             sha256.crucible={}\n\
             sha256.crucible-sandbox-broker={}\n",
            target(),
            install.prefix().display(),
            hex(ACTIVE_EXECUTABLE),
            hex(ACTIVE_BROKER),
        );
        file(&unit.join(RECEIPT), receipt.as_bytes(), 0o644);
        symlink(
            format!("{RELEASES}/{ACTIVE}"),
            install.prefix().join("current"),
        )
        .expect("the active-release link");
        symlink(
            format!("{PREFIX}/current/{CRUCIBLE}"),
            install.dir.join(CRUCIBLE),
        )
        .expect("the command's link");
        install
    }

    fn prefix(&self) -> PathBuf {
        self.dir.join(PREFIX)
    }

    fn releases(&self) -> PathBuf {
        self.prefix().join(RELEASES)
    }

    fn layout(&self) -> ReceiptLayout {
        ReceiptLayout::of_executable(&self.dir.join(CRUCIBLE)).expect("the active layout")
    }

    /// Stages `version` from `archive`, listed in `checksums`, under `limits`.
    fn stage_as(
        &self,
        version: &str,
        archive: &[u8],
        checksums: &str,
        limits: &Limits,
    ) -> Result<StagedUnit, StageError> {
        let downloads = self.dir.join("downloads");
        let _ = fs::remove_dir_all(&downloads);
        fs::create_dir(&downloads).expect("a download directory");
        let archive_at = downloads.join(name(version));
        let checksums_at = downloads.join("SHA256SUMS");
        fs::write(&archive_at, archive).expect("the archive");
        fs::write(&checksums_at, checksums).expect("the checksums");
        let version = Version::parse(version.as_bytes()).expect("a release number");
        StagedUnit::stage_within(&self.layout(), &version, &archive_at, &checksums_at, limits)
    }

    /// Stages the next release from `archive`, listed as it is.
    fn stage(&self, archive: &[u8]) -> Result<StagedUnit, StageError> {
        self.stage_within(archive, &Limits::RELEASE)
    }

    fn stage_within(&self, archive: &[u8], limits: &Limits) -> Result<StagedUnit, StageError> {
        self.stage_as(NEXT, archive, &listing(archive, NEXT), limits)
    }

    /// Every entry under the prefix and what it is, for comparing before and
    /// after a stage.
    fn snapshot(&self) -> Vec<(PathBuf, String)> {
        let mut seen = Vec::new();
        walk(&self.prefix(), &mut seen);
        // Staging adds its own directory under `releases/`, as the installer
        // does, which changes that directory's listing, size and times but
        // nothing it holds; its kind, mode and identity are still compared.
        let releases = self.releases();
        let metadata = fs::symlink_metadata(&releases).expect("its metadata");
        for (path, what) in &mut seen {
            if *path == releases {
                *what = format!(
                    "directory mode={:o} ino={}",
                    metadata.mode(),
                    metadata.ino()
                );
            }
        }
        seen.sort();
        seen
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Records every entry under `at`, staging directories aside, with its kind,
/// mode, identity, size, change time and content or link target.
fn walk(at: &Path, seen: &mut Vec<(PathBuf, String)>) {
    for listed in fs::read_dir(at).expect("a directory") {
        let path = listed.expect("an entry").path();
        let name = path.file_name().expect("a name").to_string_lossy();
        if name.starts_with(INCOMING) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).expect("its metadata");
        let what = if metadata.file_type().is_symlink() {
            format!(
                "link {}",
                fs::read_link(&path).expect("its target").display()
            )
        } else if metadata.is_dir() {
            walk(&path, seen);
            "directory".to_owned()
        } else {
            hex(&fs::read(&path).expect("its bytes"))
        };
        seen.push((
            path,
            format!(
                "{what} mode={:o} ino={} size={} mtime={}.{} ctime={}.{}",
                metadata.mode(),
                metadata.ino(),
                metadata.size(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        ));
    }
}

fn directory(at: &Path) {
    fs::create_dir(at).expect("a directory");
    fs::set_permissions(at, fs::Permissions::from_mode(0o755)).expect("its mode");
}

fn file(at: &Path, bytes: &[u8], mode: u32) {
    fs::write(at, bytes).expect("a file");
    fs::set_permissions(at, fs::Permissions::from_mode(mode)).expect("its mode");
}

fn hex(bytes: &[u8]) -> String {
    Digest::new(Sha256::digest(bytes).into()).to_string()
}

fn target() -> &'static str {
    Target::running().expect("a supported target").as_str()
}

fn stem(version: &str) -> String {
    format!("crucible-{version}-{}", target())
}

fn name(version: &str) -> String {
    format!("{}.tar.gz", stem(version))
}

/// A `SHA256SUMS` listing `archive` as the archive of `version`.
fn listing(archive: &[u8], version: &str) -> String {
    format!("{}  {}\n", hex(archive), name(version))
}

/// A tar archive built header by header, so that a test can write what no
/// well-behaved archiver would.
struct Archive {
    builder: Builder<Vec<u8>>,
    stem: String,
}

impl Archive {
    fn new() -> Self {
        Self::of(NEXT)
    }

    fn of(version: &str) -> Self {
        Self {
            builder: Builder::new(Vec::new()),
            stem: stem(version),
        }
    }

    /// The archive a release ships, with its broker when `broker`.
    fn release(broker: bool) -> Self {
        Self::release_of(NEXT, broker)
    }

    fn release_of(version: &str, broker: bool) -> Self {
        let archive = Self::of(version)
            .directory("")
            .file(CRUCIBLE, EXECUTABLE)
            .file("README.md", b"# crucible\n")
            .file("LICENSE", b"MIT\n")
            .file("install.sh", b"#!/bin/sh\n")
            .file("uninstall.sh", b"#!/bin/sh\n");
        if broker {
            archive.file(BROKER, HELPER)
        } else {
            archive
        }
    }

    /// The stem directory, or one under it when `name` is not empty.
    fn directory(self, name: &str) -> Self {
        let path = format!("{}/{name}", self.stem);
        self.entry(path.as_bytes(), EntryType::Directory, b"", None)
    }

    /// A file named `name` under the stem.
    fn file(self, name: &str, bytes: &[u8]) -> Self {
        let path = format!("{}/{name}", self.stem);
        self.entry(path.as_bytes(), EntryType::Regular, bytes, None)
    }

    /// An entry whose header names `path` byte for byte.
    fn entry(self, path: &[u8], kind: EntryType, data: &[u8], link: Option<&[u8]>) -> Self {
        self.entry_rewritten(path, kind, data, |header| {
            if let Some(link) = link {
                header.set_link_name_literal(link).expect("a link name");
            }
        })
    }

    /// An entry whose header names `path` byte for byte and is changed by
    /// `rewrite` before its checksum is set.
    fn entry_rewritten(
        mut self,
        path: &[u8],
        kind: EntryType,
        data: &[u8],
        rewrite: impl FnOnce(&mut Header),
    ) -> Self {
        let mut header = Header::new_gnu();
        let gnu = header.as_gnu_mut().expect("a GNU header");
        assert!(path.len() <= gnu.name.len(), "a name that fits its header");
        gnu.name
            .iter_mut()
            .zip(path)
            .for_each(|(slot, byte)| *slot = *byte);
        header.set_entry_type(kind);
        header.set_size(data.len() as u64);
        header.set_mode(if kind.is_dir() { 0o755 } else { 0o644 });
        rewrite(&mut header);
        header.set_cksum();
        self.builder.append(&header, data).expect("an entry");
        self
    }

    /// A GNU long-name record naming the entry that follows it `path`.
    fn long_name(self, path: &[u8]) -> Self {
        let mut data = path.to_vec();
        data.push(0);
        self.entry(b"././@LongLink", EntryType::GNULongName, &data, None)
    }

    /// A GNU long-link record giving the entry that follows it the link
    /// name `link`.
    fn long_link(self, link: &[u8]) -> Self {
        let mut data = link.to_vec();
        data.push(0);
        self.entry(b"././@LongLink", EntryType::GNULongLink, &data, None)
    }

    /// A PAX extended header for the entry that follows it.
    fn pax(self, records: &[(&str, &[u8])]) -> Self {
        let mut data = Vec::new();
        for (key, value) in records {
            data.extend(record(key, value));
        }
        self.entry(b"PaxHeader/entry", EntryType::XHeader, &data, None)
    }

    /// The tar stream, gzip-compressed.
    fn gzip(self) -> Vec<u8> {
        gzip(&self.tar())
    }

    fn tar(self) -> Vec<u8> {
        self.builder.into_inner().expect("a finished archive")
    }

    /// The tar stream without the two zero blocks that end an archive, so
    /// that what follows it is read as more of the same archive.
    fn unended(self) -> Vec<u8> {
        let mut tar = self.tar();
        tar.truncate(tar.len() - 1024);
        tar
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes).expect("compressed");
    encoder.finish().expect("a finished gzip stream")
}

/// One PAX record, `<length> <key>=<value>\n`, whose length counts itself.
fn record(key: &str, value: &[u8]) -> Vec<u8> {
    let body = key.len() + value.len() + 3;
    let mut length = body + 1;
    while body + length.to_string().len() > length {
        length += 1;
    }
    let mut bytes = format!("{length} {key}=").into_bytes();
    bytes.extend_from_slice(value);
    bytes.push(b'\n');
    assert_eq!(bytes.len(), length);
    bytes
}

/// The refusal a stage that must fail came to.
fn refused(result: Result<StagedUnit, StageError>) -> StageError {
    match result {
        Ok(unit) => panic!(
            "staged {} although the archive should have been refused",
            unit.path().display()
        ),
        Err(error) => error,
    }
}

/// The names in a directory, sorted.
fn names(at: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(at)
        .expect("a directory")
        .map(|listed| {
            listed
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    names.sort();
    names
}

fn mode(at: &Path) -> u32 {
    fs::symlink_metadata(at).expect("its metadata").mode() & 0o7777
}

// What a release ships is staged.

#[test]
fn a_release_is_staged_beside_the_active_unit_with_its_receipt() {
    let install = Install::new("staged");

    let staged = install
        .stage(&Archive::release(true).gzip())
        .expect("a staged unit");

    let unit = staged.path();
    assert_eq!(unit.parent(), Some(install.releases().as_path()));
    let name = unit.file_name().expect("a name").to_string_lossy();
    assert!(name.starts_with(INCOMING), "{name}");
    assert_eq!(names(unit), sorted(&[CRUCIBLE, BROKER, RECEIPT]));
    assert_eq!(fs::read(unit.join(CRUCIBLE)).expect("crucible"), EXECUTABLE);
    assert_eq!(fs::read(unit.join(BROKER)).expect("the broker"), HELPER);
    assert_eq!(mode(unit), 0o755);
    assert_eq!(mode(&unit.join(CRUCIBLE)), 0o755);
    assert_eq!(mode(&unit.join(BROKER)), 0o755);
    assert_eq!(mode(&unit.join(RECEIPT)), 0o644);

    let written = Receipt::parse(&fs::read(unit.join(RECEIPT)).expect("the receipt"))
        .expect("a valid receipt");
    assert_eq!(&written, staged.receipt());
    assert_eq!(written.version().as_str(), NEXT);
    assert_eq!(written.installation().as_str(), INSTALLATION);
    assert_eq!(written.prefix(), install.prefix());
    assert_eq!(written.target().as_str(), target());
    assert_eq!(written.crucible().to_string(), hex(EXECUTABLE));
    assert_eq!(written.broker().map(ToString::to_string), Some(hex(HELPER)));
}

#[test]
fn a_staged_unit_is_one_the_layout_takes_once_it_is_made_active() {
    let install = Install::new("taken");
    let staged = install
        .stage(&Archive::release(true).gzip())
        .expect("a staged unit");

    fs::rename(staged.path(), install.releases().join(NEXT)).expect("the unit moved in");
    fs::remove_file(install.prefix().join("current")).expect("the old link removed");
    symlink(
        format!("{RELEASES}/{NEXT}"),
        install.prefix().join("current"),
    )
    .expect("the new link");

    let layout = install.layout();
    assert_eq!(layout.receipt().version().as_str(), NEXT);
    assert_eq!(layout.receipt().crucible().to_string(), hex(EXECUTABLE));
}

#[test]
fn a_release_without_a_broker_is_staged_without_one() {
    let install = Install::new("brokerless");

    let staged = install
        .stage(&Archive::release(false).gzip())
        .expect("a staged unit");

    assert_eq!(names(staged.path()), sorted(&[CRUCIBLE, RECEIPT]));
    assert_eq!(staged.receipt().broker(), None);
}

#[test]
fn a_checksum_listed_in_binary_mode_and_upper_case_is_accepted() {
    let install = Install::new("binary-mode");
    let archive = Archive::release(true).gzip();
    let checksums = format!(
        "{}  crucible-{NEXT}-other.tar.gz\n{} *{}\n",
        hex(b"another archive"),
        hex(&archive).to_ascii_uppercase(),
        name(NEXT)
    );

    let staged = install
        .stage_as(NEXT, &archive, &checksums, &Limits::RELEASE)
        .expect("a staged unit");

    assert_eq!(
        fs::read(staged.path().join(CRUCIBLE)).expect("crucible"),
        EXECUTABLE
    );
}

#[test]
fn members_named_by_long_name_and_pax_records_are_refused() {
    let install = Install::new("extended-names");
    let stem = stem(NEXT);
    let archive = Archive::new()
        .long_name(format!("{stem}/{CRUCIBLE}").as_bytes())
        .entry(b"truncated-name", EntryType::Regular, EXECUTABLE, None)
        .pax(&[("path", format!("{stem}/{BROKER}").as_bytes())])
        .entry(b"truncated-name", EntryType::Regular, HELPER, None)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_staged_unit_dropped_unused_is_removed() {
    let install = Install::new("dropped");
    let staged = install
        .stage(&Archive::release(true).gzip())
        .expect("a staged unit");
    let unit = staged.path().to_path_buf();

    drop(staged);

    assert!(!unit.exists());
    assert_eq!(names(&install.releases()), [ACTIVE]);
}

#[test]
fn a_refused_archive_leaves_nothing_behind() {
    let install = Install::new("refused-cleanly");
    let archive = Archive::release(true).file("extra", b"").gzip();

    refused(install.stage(&archive));

    assert_eq!(names(&install.releases()), [ACTIVE]);
}

// Staging never touches the active unit.

#[test]
fn staging_a_new_release_never_touches_the_active_unit() {
    let install = Install::new("untouched-new");
    let before = install.snapshot();

    let staged = install
        .stage(&Archive::release(true).gzip())
        .expect("a staged unit");

    assert_eq!(install.snapshot(), before);
    assert!(staged.path().join(CRUCIBLE).is_file());
}

#[test]
fn staging_the_active_release_again_never_touches_the_active_unit() {
    let install = Install::new("untouched-same");
    let archive = Archive::release_of(ACTIVE, true).gzip();
    let before = install.snapshot();

    let staged = install
        .stage_as(
            ACTIVE,
            &archive,
            &listing(&archive, ACTIVE),
            &Limits::RELEASE,
        )
        .expect("a staged unit");

    assert_eq!(install.snapshot(), before);
    assert_ne!(staged.path(), install.releases().join(ACTIVE));
    assert_eq!(staged.receipt().version().as_str(), ACTIVE);
    assert_eq!(
        fs::read(staged.path().join(CRUCIBLE)).expect("crucible"),
        EXECUTABLE
    );
}

#[test]
fn a_refused_stage_never_touches_the_active_unit() {
    let install = Install::new("untouched-refused");
    let archive = Archive::release(true).file(CRUCIBLE, EXECUTABLE).gzip();
    let before = install.snapshot();

    refused(install.stage(&archive));

    assert_eq!(install.snapshot(), before);
}

// Traversal and names a release does not hold.

#[test]
fn a_member_that_climbs_out_of_the_release_is_refused() {
    let install = Install::new("climbs");
    let path = format!("{}/../../../{CRUCIBLE}", stem(NEXT));
    let archive = Archive::release(true)
        .entry(path.as_bytes(), EntryType::Regular, EXECUTABLE, None)
        .gzip();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Unexpected
    ));
}

#[test]
fn an_absolute_member_is_refused() {
    let install = Install::new("absolute");
    let archive = Archive::release(true)
        .entry(b"/tmp/crucible", EntryType::Regular, EXECUTABLE, None)
        .gzip();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Unexpected
    ));
}

#[test]
fn a_member_whose_long_name_climbs_out_is_refused() {
    let install = Install::new("long-name-climbs");
    let archive = Archive::release(true)
        .long_name(format!("{}/../../escape", stem(NEXT)).as_bytes())
        .file("README.md", b"")
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_member_whose_pax_path_climbs_out_is_refused() {
    let install = Install::new("pax-climbs");
    let archive = Archive::release(true)
        .pax(&[("path", b"../escape")])
        .file("README.md", b"")
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_member_a_release_does_not_hold_is_refused() {
    let install = Install::new("foreign");
    let archive = Archive::release(true).file("extra", b"").gzip();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Unexpected
    ));
}

#[test]
fn the_members_of_another_release_are_refused() {
    let install = Install::new("other-release");
    let archive = Archive::release_of("0.45.0", true).gzip();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Unexpected
    ));
}

// Links and entries that are not files or directories.

#[test]
fn a_symbolic_link_is_refused() {
    let install = Install::new("symlink");
    let path = format!("{}/{CRUCIBLE}", stem(NEXT));
    let archive = Archive::new()
        .directory("")
        .entry(path.as_bytes(), EntryType::Symlink, b"", Some(b"/bin/sh"))
        .gzip();

    assert!(matches!(refused(install.stage(&archive)), StageError::Link));
}

#[test]
fn a_hard_link_is_refused() {
    let install = Install::new("hardlink");
    let stem = stem(NEXT);
    let archive = Archive::new()
        .file("README.md", b"")
        .entry(
            format!("{stem}/{CRUCIBLE}").as_bytes(),
            EntryType::Link,
            b"",
            Some(format!("{stem}/README.md").as_bytes()),
        )
        .gzip();

    assert!(matches!(refused(install.stage(&archive)), StageError::Link));
}

#[test]
fn a_device_or_a_pipe_is_refused() {
    for kind in [EntryType::Char, EntryType::Block, EntryType::Fifo] {
        let install = Install::new("special");
        let path = format!("{}/{CRUCIBLE}", stem(NEXT));
        let archive = Archive::new()
            .entry(path.as_bytes(), kind, b"", None)
            .gzip();

        assert!(
            matches!(refused(install.stage(&archive)), StageError::Special),
            "{kind:?}"
        );
    }
}

// Archives larger than a release.

#[test]
fn an_archive_larger_than_a_release_is_refused() {
    let install = Install::new("large-archive");
    let archive = Archive::release(true).gzip();
    let limits = Limits {
        archive: 64,
        ..Limits::RELEASE
    };

    assert!(matches!(
        refused(install.stage_within(&archive, &limits)),
        StageError::TooLarge {
            part: StagePart::Archive
        }
    ));
}

#[test]
fn an_executable_larger_than_a_release_holds_is_refused() {
    let install = Install::new("large-executable");
    let archive = Archive::release(true).gzip();
    let limits = Limits {
        executable: 8,
        ..Limits::RELEASE
    };

    assert!(matches!(
        refused(install.stage_within(&archive, &limits)),
        StageError::TooLarge {
            part: StagePart::Member
        }
    ));
}

#[test]
fn a_document_larger_than_a_release_holds_is_refused() {
    let install = Install::new("large-document");
    let archive = Archive::release(true).gzip();
    let limits = Limits {
        document: 4,
        ..Limits::RELEASE
    };

    assert!(matches!(
        refused(install.stage_within(&archive, &limits)),
        StageError::TooLarge {
            part: StagePart::Member
        }
    ));
}

#[test]
fn a_long_name_larger_than_a_release_holds_is_refused() {
    let install = Install::new("large-extension");
    let name = vec![b'a'; 70 * 1024];
    let archive = Archive::release(true)
        .long_name(&name)
        .file("README.md", b"")
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn an_archive_that_unpacks_past_the_ceiling_is_refused() {
    let install = Install::new("large-contents");
    let archive = Archive::release(true).gzip();
    let limits = Limits {
        unpacked: 4096,
        ..Limits::RELEASE
    };

    assert!(matches!(
        refused(install.stage_within(&archive, &limits)),
        StageError::TooLarge {
            part: StagePart::Contents
        }
    ));
}

#[test]
fn an_archive_of_more_entries_than_a_release_is_refused() {
    let install = Install::new("many-entries");
    let archive = Archive::release(true).gzip();
    let limits = Limits {
        entries: 3,
        ..Limits::RELEASE
    };

    assert!(matches!(
        refused(install.stage_within(&archive, &limits)),
        StageError::TooLarge {
            part: StagePart::Contents
        }
    ));
}

#[test]
fn a_checksums_file_larger_than_the_ceiling_is_refused() {
    let install = Install::new("large-checksums");
    let archive = Archive::release(true).gzip();
    let mut checksums = listing(&archive, NEXT);
    checksums.push_str(&"#\n".repeat(40 * 1024));

    assert!(matches!(
        refused(install.stage_as(NEXT, &archive, &checksums, &Limits::RELEASE)),
        StageError::TooLarge {
            part: StagePart::Checksums
        }
    ));
}

// Archives that are not the release SHA256SUMS lists.

#[test]
fn an_archive_whose_digest_is_not_the_listed_one_is_refused() {
    let install = Install::new("mismatch");
    let archive = Archive::release(true).gzip();
    let other = Archive::release(false).gzip();

    assert!(matches!(
        refused(install.stage_as(NEXT, &archive, &listing(&other, NEXT), &Limits::RELEASE)),
        StageError::Mismatch
    ));
}

#[test]
fn an_archive_the_checksums_do_not_list_is_refused() {
    let install = Install::new("unlisted");
    let archive = Archive::release(true).gzip();
    let other_release = listing(&archive, ACTIVE);
    let listed_twice = format!("{}{}", listing(&archive, NEXT), listing(&archive, NEXT));
    let digest = hex(&archive);
    let shortened = digest.get(..63).expect("a shorter digest");
    let short = format!("{shortened}  {}\n", name(NEXT));
    let not_hex = format!("{shortened}g  {}\n", name(NEXT));
    let carriage_return = format!("{}  {}\r\n", hex(&archive), name(NEXT));

    for checksums in [
        "",
        &other_release,
        &listed_twice,
        &short,
        &not_hex,
        &carriage_return,
    ] {
        assert!(
            matches!(
                refused(install.stage_as(NEXT, &archive, checksums, &Limits::RELEASE)),
                StageError::Listing
            ),
            "{checksums:?}"
        );
    }
}

#[test]
fn an_archive_without_crucible_is_refused() {
    let install = Install::new("missing");
    let archive = Archive::new()
        .directory("")
        .file(BROKER, HELPER)
        .file("README.md", b"")
        .gzip();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Missing
    ));
}

#[test]
fn an_archive_that_ends_inside_a_member_is_refused() {
    let install = Install::new("truncated");
    let executable = vec![b'x'; 4096];
    let tar = Archive::new()
        .directory("")
        .file(CRUCIBLE, &executable)
        .tar();
    let archive = gzip(tar.get(..2048).expect("a prefix of the archive"));

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Corrupt(_)
    ));
}

#[test]
fn an_archive_that_is_not_gzip_is_refused() {
    let install = Install::new("not-gzip");
    let archive = Archive::release(true).tar();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Corrupt(_)
    ));
}

#[test]
fn a_member_that_appears_twice_is_refused() {
    let install = Install::new("duplicate");
    let archive = Archive::release(true).file(CRUCIBLE, EXECUTABLE).gzip();

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Duplicate
    ));
}

#[test]
fn a_member_of_the_wrong_kind_for_its_place_is_refused() {
    let install = Install::new("kind");
    let directory_for_a_file = Archive::new().directory(CRUCIBLE).gzip();
    let file_for_the_directory = Archive::new()
        .entry(stem(NEXT).as_bytes(), EntryType::Regular, b"", None)
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    for archive in [directory_for_a_file, file_for_the_directory] {
        assert!(matches!(refused(install.stage(&archive)), StageError::Kind));
    }
}

#[test]
fn a_member_whose_extended_header_gives_it_another_size_is_refused() {
    let install = Install::new("two-sizes");
    let archive = Archive::new()
        .directory("")
        .pax(&[("size", b"0")])
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_member_whose_extended_header_makes_it_sparse_is_refused() {
    let install = Install::new("sparse");
    let archive = Archive::new()
        .directory("")
        .pax(&[("GNU.sparse.major", b"1"), ("GNU.sparse.minor", b"0")])
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_member_whose_extended_header_gives_a_key_twice_is_refused() {
    let install = Install::new("pax-key-twice");
    let size = EXECUTABLE.len().to_string();
    let archive = Archive::new()
        .directory("")
        .pax(&[("size", size.as_bytes()), ("size", b"1024")])
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_member_whose_extended_header_names_it_otherwise_is_refused() {
    let install = Install::new("pax-path-differs");
    let stem = stem(NEXT);
    let archive = Archive::new()
        .directory("")
        .long_name(format!("{stem}/{CRUCIBLE}").as_bytes())
        .pax(&[("path", format!("{stem}/README.md").as_bytes())])
        .entry(b"truncated-name", EntryType::Regular, EXECUTABLE, None)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

// Headers no release writes, which other archivers read otherwise.

/// Writes `bytes` over the header block at `at`.
fn overwrite(block: &mut [u8; 512], at: std::ops::Range<usize>, bytes: &[u8]) {
    block
        .get_mut(at)
        .expect("a header field")
        .copy_from_slice(bytes);
}

/// A release whose `crucible` has a header that `rewrite` changes.
fn release_with_crucible(rewrite: impl FnOnce(&mut [u8; 512])) -> Vec<u8> {
    let path = format!("{}/{CRUCIBLE}", stem(NEXT));
    Archive::new()
        .directory("")
        .entry_rewritten(path.as_bytes(), EntryType::Regular, EXECUTABLE, |header| {
            rewrite(header.as_mut_bytes());
        })
        .gzip()
}

/// Asserts that `archive` is refused as one no release is, leaving nothing.
fn refused_as_unexpected(install: &Install, archive: &[u8]) {
    let error = refused(install.stage(archive));
    assert!(matches!(error, StageError::Unexpected), "{error:?}");
    assert_eq!(names(&install.releases()), [ACTIVE]);
}

#[test]
fn a_ustar_header_of_another_version_is_refused() {
    let install = Install::new("ustar-version");
    let archive = release_with_crucible(|block| {
        overwrite(block, 257..263, b"ustar\0");
        overwrite(block, 263..265, b"01");
        overwrite(block, 345..349, b"evil");
    });

    refused_as_unexpected(&install, &archive);
}

#[test]
fn an_old_style_header_is_refused() {
    let install = Install::new("old-header");
    let archive = release_with_crucible(|block| overwrite(block, 257..265, &[0; 8]));

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_member_whose_extended_header_has_a_vendor_record_is_refused() {
    let install = Install::new("pax-vendor");
    for (key, value) in [
        ("SCHILY.realsize", b"1048576".as_slice()),
        ("LIBARCHIVE.symlinktype", b"file".as_slice()),
    ] {
        let archive = Archive::new()
            .directory("")
            .pax(&[(key, value)])
            .file(CRUCIBLE, EXECUTABLE)
            .gzip();

        refused_as_unexpected(&install, &archive);
    }
}

#[test]
fn a_file_whose_extended_header_gives_it_a_link_path_is_refused() {
    let install = Install::new("pax-linkpath");
    let archive = Archive::new()
        .directory("")
        .pax(&[("linkpath", b"/bin/sh")])
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_file_whose_header_gives_it_a_link_name_is_refused() {
    let install = Install::new("ustar-linkname");
    let path = format!("{}/{CRUCIBLE}", stem(NEXT));
    let archive = Archive::new()
        .directory("")
        .entry(
            path.as_bytes(),
            EntryType::Regular,
            EXECUTABLE,
            Some(b"/bin/sh"),
        )
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_file_given_a_long_link_name_is_refused() {
    let install = Install::new("long-link");
    let archive = Archive::new()
        .directory("")
        .long_link(b"/bin/sh")
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_size_written_in_base_256_is_refused() {
    let length = u8::try_from(EXECUTABLE.len()).expect("a one-byte length");
    let positive = [0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, length];
    let negative = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, length];
    for size in [positive, negative] {
        let install = Install::new("base-256-size");
        let archive = release_with_crucible(|block| overwrite(block, 124..136, &size));

        refused_as_unexpected(&install, &archive);
    }
}

#[test]
fn an_extended_header_before_crucible_is_refused() {
    let install = Install::new("pax-before-crucible");
    let path = format!("{}/{CRUCIBLE}", stem(NEXT));
    let archive = Archive::new()
        .directory("")
        .pax(&[("path", path.as_bytes())])
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

#[test]
fn a_long_name_naming_crucible_is_refused() {
    let install = Install::new("long-name-crucible");
    let archive = Archive::new()
        .directory("")
        .long_name(format!("{}/{CRUCIBLE}", stem(NEXT)).as_bytes())
        .entry(b"truncated-name", EntryType::Regular, EXECUTABLE, None)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

/// Asserts that a release whose `crucible` has each of `sizes`, every one
/// of which this reader takes as the member's length, is refused.
fn refused_with_sizes(name: &str, sizes: &[&[u8; 12]]) {
    for size in sizes {
        let install = Install::new(name);
        let archive = release_with_crucible(|block| overwrite(block, 124..136, *size));

        refused_as_unexpected(&install, &archive);
    }
}

#[test]
fn a_size_led_by_a_space_is_refused() {
    refused_with_sizes("size-space", &[b" 0000000053\0", b"  000000053\0"]);
}

#[test]
fn a_size_led_by_a_sign_is_refused() {
    refused_with_sizes("size-sign", &[b"+0000000053\0"]);
}

#[test]
fn a_size_with_other_whitespace_is_refused() {
    refused_with_sizes(
        "size-whitespace",
        &[
            b"\t0000000053\0",
            b"\x0b0000000053\0",
            b"\n0000000053\0",
            b"\r0000000053\0",
            b"\x0c0000000053\0",
            b"0000000053\t\0",
        ],
    );
}

#[test]
fn a_size_with_a_non_octal_digit_is_refused() {
    refused_with_sizes("size-digit", &[b"0000000053\08", b"000000053\099"]);
}

#[test]
fn a_size_ended_by_spaces_or_nuls_is_staged() {
    for size in [b"0000000053 \0", b"0000000053  ", b"000000053\0\0\0"] {
        let install = Install::new("size-terminators");
        let archive = release_with_crucible(|block| overwrite(block, 124..136, size));

        let staged = install.stage(&archive).expect("a staged unit");

        assert_eq!(
            fs::read(staged.path().join(CRUCIBLE)).expect("crucible"),
            EXECUTABLE
        );
    }
}

#[test]
fn a_member_whose_extended_size_is_not_a_decimal_number_is_refused() {
    let install = Install::new("pax-size-text");
    let size = format!("{}x", EXECUTABLE.len());
    let archive = Archive::new()
        .directory("")
        .pax(&[("size", size.as_bytes())])
        .file(CRUCIBLE, EXECUTABLE)
        .gzip();

    refused_as_unexpected(&install, &archive);
}

// Archives read in part.

#[test]
fn a_second_gzip_member_is_refused() {
    let install = Install::new("two-members");
    let crucible = Archive::new().file(CRUCIBLE, b"another crucible").tar();
    let path = format!("{}/{BROKER}", stem(NEXT));
    let link = Some(b"/etc/passwd".as_slice());
    let broker = Archive::new()
        .entry(path.as_bytes(), EntryType::Symlink, b"", link)
        .tar();
    let seconds = [(true, crucible), (false, broker)];

    for (broker, second) in seconds {
        let mut archive = gzip(&Archive::release(broker).unended());
        archive.extend(gzip(&second));

        assert!(matches!(
            refused(install.stage(&archive)),
            StageError::Corrupt(_)
        ));
        assert_eq!(names(&install.releases()), [ACTIVE]);
    }
}

#[test]
fn bytes_after_the_gzip_member_are_refused() {
    let install = Install::new("trailing");
    let mut archive = Archive::release(true).gzip();
    archive.extend_from_slice(b"trailing bytes");

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Corrupt(_)
    ));
    assert_eq!(names(&install.releases()), [ACTIVE]);
}

#[test]
fn an_archive_whose_gzip_checksum_does_not_match_is_refused() {
    let install = Install::new("bad-crc");
    let mut archive = Archive::release(true).gzip();
    let crc = archive.len() - 8;
    *archive.get_mut(crc).expect("the gzip trailer") ^= 0xff;

    assert!(matches!(
        refused(install.stage(&archive)),
        StageError::Corrupt(_)
    ));
    assert_eq!(names(&install.releases()), [ACTIVE]);
}
