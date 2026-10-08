//! A release unit staged from its archive, beside the active one.
//!
//! The archive and its `SHA256SUMS` are hostile until proved otherwise, and
//! nothing is unpacked from an archive that is not the one listed. The
//! checksum line is found the way the installer finds it: exactly one line
//! whose first field is hex and whose second names the archive, in text or
//! binary mode. The archive is copied into the staging directory as it is
//! hashed, and that copy, which nobody else can write, is the one read from
//! then on, so the bytes checked are the bytes unpacked.
//!
//! The copy is read twice, each time as one gzip member decompressed to its
//! end, so its checksum is checked, and a byte after that member is refused,
//! since a reader that decodes every member would go on to read it. The first
//! pass reads each header as it is written and holds it to what a release
//! writes, since a header outside that is one another archiver may read
//! otherwise: exactly ustar or exactly GNU, its size strict octal (digits, then
//! only NUL or space to the end of the field), and a file or a directory with
//! no link name. It refuses everything else, a link, a device, a pipe, a
//! sparse file, a long name, a long link name and an extended header, global
//! or not, among them, and any member larger than a release holds. So every
//! member is named and sized by its own header alone, and a release member's
//! name must fit in that header: the ustar name and prefix, or a GNU header's
//! name. The second pass takes each member by its full name, byte for byte,
//! and only a member a release ships: its own directory, `crucible`, the
//! broker and the documents beside them. A name that climbs out, an absolute
//! name, another release's names and a member given twice are refused rather
//! than resolved, since two archivers could resolve them differently, as is a
//! member this reader sizes other than its header does, so both passes walk
//! the same headers. Only the executables are written, each hashed as it is
//! written, and the receipt that names them is read back before the unit is
//! called staged.
//!
//! Every count and size is bounded before it is stored or read: the
//! checksums, the archive, its decompressed contents, its headers and each
//! member, each by the most a release holds. A unit is staged under
//! `releases/` in a directory named as the installer names its own, which is
//! removed when the value is dropped unless activation moved it into place,
//! and a refusal or a crash before activation leaves only what the installer
//! and the next update already clean up. A unit is staged only under the
//! install's lock, which activation holds, since an install holding the lock
//! removes every staging directory it finds. The active unit, the `current`
//! link and every other release are never opened for writing.

use std::cell::Cell;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::io::{self, BufRead as _, BufReader, Read, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use flate2::bufread::GzDecoder;
use sha2::{Digest as _, Sha256};
use tar::EntryType;

use super::boundary::crossed;
use super::layout::{BROKER, CRUCIBLE, EXECUTABLE_CEILING, HASH_BUFFER, RECEIPT, RELEASES};
use super::receipt::MAX_BYTES;
use super::{Digest, Receipt, ReceiptError, ReceiptLayout, Target, Version};

/// What a staging directory's name opens with, which the installer's own
/// staging directories share, so either cleans up what the other left.
pub(super) const INCOMING: &str = ".incoming.";

/// How many names are tried for a staging directory before giving up.
const ATTEMPTS: usize = 16;

/// The archive's copy inside the staging directory, removed once unpacked.
const ARCHIVE: &str = "archive";

/// The documents a release ships beside its executables, which are read past
/// and not staged.
const DOCUMENTS: [&str; 4] = ["README.md", "LICENSE", "install.sh", "uninstall.sh"];

/// A release unit staged and verified beside the active one, not yet
/// activated.
#[derive(Debug)]
pub struct StagedUnit {
    /// The staging directory, removed with the value.
    unit: Incoming,
    /// The receipt written into it.
    receipt: Receipt,
}

/// Why an archive was not staged.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    /// A part of the staging could not be read or written.
    #[error("could not read or write the {part}")]
    Io {
        /// The part.
        part: StagePart,
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
    /// A part is larger than a release ever is.
    #[error("the {part} is too large to be a release")]
    TooLarge {
        /// The part.
        part: StagePart,
    },
    /// `SHA256SUMS` does not hold exactly one SHA-256 for the archive.
    #[error("SHA256SUMS does not hold exactly one valid line for the release archive")]
    Listing,
    /// The archive is not the one `SHA256SUMS` lists.
    #[error("the release archive does not match SHA256SUMS")]
    Mismatch,
    /// The archive is not a readable gzip-compressed tar.
    #[error("the release archive is not a readable gzip-compressed tar")]
    Corrupt(#[source] io::Error),
    /// A member's headers describe it in two ways: this reader sizes it other
    /// than its header does.
    #[error("the release archive describes a member in two ways")]
    Ambiguous,
    /// A member is a symbolic or a hard link.
    #[error("the release archive holds a link")]
    Link,
    /// A member is neither a file nor a directory.
    #[error("the release archive holds a member that is neither a file nor a directory")]
    Special,
    /// A member has a name, or a header, that no release member has.
    #[error("the release archive holds a member a release does not")]
    Unexpected,
    /// A member is not the kind of entry its place holds.
    #[error("the release archive holds a member of the wrong kind for its place")]
    Kind,
    /// A member appears more than once.
    #[error("the release archive holds a member twice")]
    Duplicate,
    /// The archive holds no `crucible`.
    #[error("the release archive holds no crucible")]
    Missing,
    /// The receipt written does not follow the format.
    #[error("the staged receipt is not valid")]
    Receipt(#[source] ReceiptError),
    /// The receipt written reads back as another.
    #[error("the staged receipt does not read back as it was written")]
    Readback,
}

/// A part of staging, as an error names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagePart {
    /// The `SHA256SUMS` file.
    Checksums,
    /// The release archive, as it was downloaded.
    Archive,
    /// What the archive holds once decompressed.
    Contents,
    /// One member of the archive.
    Member,
    /// The staging directory.
    Unit,
    /// The staged `crucible`.
    Executable,
    /// The staged broker.
    Broker,
    /// The staged receipt.
    Receipt,
}

impl fmt::Display for StagePart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Checksums => "release checksums",
            Self::Archive => "release archive",
            Self::Contents => "release archive's contents",
            Self::Member => "release archive's member",
            Self::Unit => "staged release",
            Self::Executable => "staged executable",
            Self::Broker => "staged broker",
            Self::Receipt => "staged receipt",
        })
    }
}

/// The ceilings staging holds an archive to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The most `SHA256SUMS` may hold.
    pub(crate) checksums: u64,
    /// The most the archive may hold, compressed.
    pub(crate) archive: u64,
    /// The most an executable member may hold.
    executable: u64,
    /// The most any other file member may hold.
    document: u64,
    /// The most headers the archive may hold.
    entries: usize,
    /// The most the archive may hold, decompressed.
    unpacked: u64,
}

impl Limits {
    /// The ceilings of a release.
    pub(crate) const RELEASE: Self = Self {
        checksums: 64 * 1024,
        archive: 512 * 1024 * 1024,
        executable: EXECUTABLE_CEILING,
        document: 1024 * 1024,
        entries: 64,
        unpacked: 2 * EXECUTABLE_CEILING + 16 * 1024 * 1024,
    };
}

impl StagedUnit {
    /// Stages `version` from its archive and `SHA256SUMS`, under the lock
    /// the caller holds.
    pub(super) fn stage(
        layout: &ReceiptLayout,
        version: &Version,
        archive: &Path,
        checksums: &Path,
    ) -> Result<Self, StageError> {
        Self::stage_within(layout, version, archive, checksums, &Limits::RELEASE)
    }

    /// Stages `version` under `limits`.
    fn stage_within(
        layout: &ReceiptLayout,
        version: &Version,
        archive: &Path,
        checksums: &Path,
        limits: &Limits,
    ) -> Result<Self, StageError> {
        let target = layout.receipt().target();
        let expected = listed(checksums, &archive_name(version, target), limits)?;
        let unit = Incoming::create(&layout.prefix().join(RELEASES))?;
        let copy = copied(archive, &unit.path.join(ARCHIVE), expected, limits)?;
        survey(&copy, limits)?;
        rewind(&copy, StagePart::Unit)?;
        let stem = format!("crucible-{version}-{}", target.as_str());
        let unpacked = extract(&copy, &unit.path, &stem, limits)?;
        drop(copy);
        fs::remove_file(unit.path.join(ARCHIVE)).map_err(io(StagePart::Unit))?;
        crossed("removed the archive's copy");
        let receipt =
            layout
                .receipt()
                .for_release(version.clone(), unpacked.crucible, unpacked.broker);
        write_receipt(&unit.path, &receipt)?;
        seal(&unit.path)?;
        crossed("sealed the staged release");
        Ok(Self { unit, receipt })
    }

    /// Moves the unit to `to`, an absent name in the same directory, where it
    /// is no longer removed with the value, and returns its receipt.
    pub(super) fn moved(mut self, to: &Path) -> io::Result<Receipt> {
        fs::rename(&self.unit.path, to)?;
        self.unit.moved = true;
        Ok(self.receipt)
    }

    /// The staging directory.
    pub fn path(&self) -> &Path {
        &self.unit.path
    }

    /// The receipt written into it.
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }
}

/// A staging directory, removed with the value unless it was moved.
#[derive(Debug)]
struct Incoming {
    /// Where it is.
    path: PathBuf,
    /// Whether it was moved into place, so it is no longer here to remove.
    moved: bool,
}

impl Incoming {
    /// Makes an owner-only staging directory under `releases`.
    fn create(releases: &Path) -> Result<Self, StageError> {
        let mut last = io::Error::other("no name was tried");
        for _ in 0..ATTEMPTS {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            let path = releases.join(format!("{INCOMING}{:016x}", hasher.finish()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    crossed("made the staging directory");
                    return Ok(Self { path, moved: false });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = error,
                Err(source) => {
                    return Err(StageError::Io {
                        part: StagePart::Unit,
                        source,
                    });
                }
            }
        }
        Err(StageError::Io {
            part: StagePart::Unit,
            source: last,
        })
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if self.moved {
            return;
        }
        if fs::remove_dir_all(&self.path).is_ok() {
            crossed("removed the staging directory");
        }
    }
}

/// The executables a release unit holds once unpacked, by their digests.
struct Unpacked {
    /// The SHA-256 of `crucible`.
    crucible: Digest,
    /// The SHA-256 of the broker, when the release carries one.
    broker: Option<Digest>,
}

/// Where a member sits in a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// The release's own directory.
    Release,
    /// Its `crucible`.
    Crucible,
    /// Its broker.
    Broker,
    /// One of the documents beside them.
    Document,
}

impl Place {
    /// The place `name` holds in the release whose members sit under `stem`,
    /// compared byte for byte, or `None` when no release holds it.
    fn of(name: &[u8], stem: &str) -> Option<Self> {
        let rest = name.strip_prefix(stem.as_bytes())?;
        if rest.is_empty() {
            return Some(Self::Release);
        }
        let member = rest.strip_prefix(b"/")?;
        if member == CRUCIBLE.as_bytes() {
            Some(Self::Crucible)
        } else if member == BROKER.as_bytes() {
            Some(Self::Broker)
        } else if DOCUMENTS
            .iter()
            .any(|document| member == document.as_bytes())
        {
            Some(Self::Document)
        } else {
            None
        }
    }

    /// The kind of entry the place holds.
    fn kind(self) -> EntryType {
        match self {
            Self::Release => EntryType::Directory,
            Self::Crucible | Self::Broker | Self::Document => EntryType::Regular,
        }
    }
}

/// A reader that fails, and says so through `exceeded`, once more than
/// `left` bytes have come through it.
struct Bounded<'a, R> {
    /// What is read.
    inner: R,
    /// How much more may come through.
    left: u64,
    /// Set once the ceiling was passed, so a failure the archive reader
    /// passes on can be told from one in the archive.
    exceeded: &'a Cell<bool>,
}

impl<R: Read> Read for Bounded<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let room = usize::try_from(self.left.saturating_add(1)).unwrap_or(usize::MAX);
        let room = room.min(buffer.len());
        let read = self
            .inner
            .read(buffer.get_mut(..room).unwrap_or_default())?;
        let Some(left) = self
            .left
            .checked_sub(u64::try_from(read).unwrap_or(u64::MAX))
        else {
            self.exceeded.set(true);
            return Err(io::Error::other("the archive unpacks past its ceiling"));
        };
        self.left = left;
        Ok(read)
    }
}

/// What the archive holds once decompressed: its copy read as one gzip
/// member, through the ceiling on what it unpacks to.
type Contents<'a> = Bounded<'a, GzDecoder<BufReader<&'a File>>>;

/// The archive's copy as a tar stream, failing through `exceeded` once it
/// unpacks past its ceiling.
fn contents<'a>(
    archive: &'a File,
    limits: &Limits,
    exceeded: &'a Cell<bool>,
) -> tar::Archive<Contents<'a>> {
    tar::Archive::new(Bounded {
        inner: GzDecoder::new(BufReader::new(archive)),
        left: limits.unpacked,
        exceeded,
    })
}

/// Reads what is left of the archive once its members are read: the rest of
/// its gzip member, through the ceiling, so that member's checksum and length
/// are checked, and then refuses any byte after it, a second member included.
fn ended(tar: tar::Archive<Contents<'_>>, exceeded: &Cell<bool>) -> Result<(), StageError> {
    let mut contents = tar.into_inner();
    io::copy(&mut contents, &mut io::sink()).map_err(unreadable(exceeded))?;
    let mut rest = contents.inner.into_inner();
    if rest.fill_buf().map_err(io(StagePart::Unit))?.is_empty() {
        Ok(())
    } else {
        Err(StageError::Corrupt(io::Error::other(
            "the archive goes on past its gzip stream",
        )))
    }
}

/// The name a release's archive is published under.
pub(crate) fn archive_name(version: &Version, target: Target) -> String {
    format!("crucible-{version}-{}.tar.gz", target.as_str())
}

/// The SHA-256 that `SHA256SUMS` at `at` lists for the archive `name`.
fn listed(at: &Path, name: &str, limits: &Limits) -> Result<Digest, StageError> {
    let part = StagePart::Checksums;
    let file = crucible_privacy::open_read(at).map_err(|error| StageError::Io {
        part,
        source: error.into_io(),
    })?;
    if file.metadata().map_err(io(part))?.len() > limits.checksums {
        return Err(StageError::TooLarge { part });
    }
    let mut text = Vec::new();
    file.take(limits.checksums.saturating_add(1))
        .read_to_end(&mut text)
        .map_err(io(part))?;
    if u64::try_from(text.len()).map_or(true, |length| length > limits.checksums) {
        return Err(StageError::TooLarge { part });
    }
    let starred = format!("*{name}");
    let mut found = text.split(|byte| *byte == b'\n').filter_map(|line| {
        let mut fields = line
            .split(|byte| matches!(byte, b' ' | b'\t'))
            .filter(|field| !field.is_empty());
        let digest = fields.next()?;
        let listed = fields.next()?;
        let named = listed == name.as_bytes() || listed == starred.as_bytes();
        (named && digest.iter().all(u8::is_ascii_hexdigit)).then_some(digest)
    });
    match (found.next(), found.next()) {
        (Some(digest), None) => {
            Digest::parse(&digest.to_ascii_lowercase()).ok_or(StageError::Listing)
        }
        _ => Err(StageError::Listing),
    }
}

/// Copies the archive at `from` to `to`, hashing it as it goes, and returns
/// the copy once it hashes to `expected`.
fn copied(from: &Path, to: &Path, expected: Digest, limits: &Limits) -> Result<File, StageError> {
    let part = StagePart::Archive;
    let source = crucible_privacy::open_read(from).map_err(|error| StageError::Io {
        part,
        source: error.into_io(),
    })?;
    if source.metadata().map_err(io(part))?.len() > limits.archive {
        return Err(StageError::TooLarge { part });
    }
    let mut source = source.take(limits.archive.saturating_add(1));
    let mut copy = create(to, StagePart::Unit)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_BUFFER];
    let mut total: u64 = 0;
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => return Err(StageError::Io { part, source }),
        };
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > limits.archive {
            return Err(StageError::TooLarge { part });
        }
        let chunk = buffer.get(..read).unwrap_or_default();
        hasher.update(chunk);
        copy.write_all(chunk).map_err(io(StagePart::Unit))?;
    }
    if Digest::new(hasher.finalize().into()) != expected {
        return Err(StageError::Mismatch);
    }
    copy.sync_all().map_err(io(StagePart::Unit))?;
    crossed("copied the archive");
    rewind(&copy, StagePart::Unit)?;
    Ok(copy)
}

/// Reads every header of the archive as it is written, before the second
/// pass reads it, refusing every header a release does not write, a long name
/// and an extended header among them, and any member larger than a release
/// holds.
fn survey(archive: &File, limits: &Limits) -> Result<(), StageError> {
    let exceeded = Cell::new(false);
    let mut tar = contents(archive, limits, &exceeded);
    let entries = tar.entries().map_err(unreadable(&exceeded))?.raw(true);
    for (count, entry) in entries.enumerate() {
        let entry = entry.map_err(unreadable(&exceeded))?;
        if count >= limits.entries {
            return Err(StageError::TooLarge {
                part: StagePart::Contents,
            });
        }
        let header = entry.header();
        let ceiling = match header.entry_type() {
            _ if !written(header) => return Err(StageError::Unexpected),
            EntryType::Regular | EntryType::Directory if header.link_name_bytes().is_some() => {
                return Err(StageError::Unexpected);
            }
            EntryType::Regular | EntryType::Directory => limits.executable,
            EntryType::GNULongName | EntryType::XHeader | EntryType::GNULongLink => {
                return Err(StageError::Unexpected);
            }
            EntryType::Symlink | EntryType::Link => return Err(StageError::Link),
            _ => return Err(StageError::Special),
        };
        if header.entry_size().map_err(StageError::Corrupt)? > ceiling {
            return Err(StageError::TooLarge {
                part: StagePart::Member,
            });
        }
    }
    ended(tar, &exceeded)
}

/// Whether `header` is written as a release writes one: exactly ustar or
/// exactly GNU, which every archiver reads alike, where an old header or
/// another version leaves a reader to guess whether its name has a prefix;
/// and with its size in strict octal.
fn written(header: &tar::Header) -> bool {
    (header.as_ustar().is_some() || header.as_gnu().is_some()) && octal(&header.as_old().size)
}

/// Whether `field` holds a number as every archiver writes one: one or more
/// octal digits, then only NUL or space to its end. This reader also takes a
/// leading sign, whitespace around the digits, any bytes after a NUL and a
/// base-256 number, which other readers take otherwise or refuse.
fn octal(field: &[u8]) -> bool {
    let digits = field
        .iter()
        .take_while(|byte| matches!(byte, b'0'..=b'7'))
        .count();
    digits > 0
        && field
            .iter()
            .skip(digits)
            .all(|byte| matches!(byte, b'\0' | b' '))
}

/// Unpacks the executables of the release under `stem` from the archive into
/// `unit`, refusing every member a release does not hold where it holds it.
fn extract(
    archive: &File,
    unit: &Path,
    stem: &str,
    limits: &Limits,
) -> Result<Unpacked, StageError> {
    let exceeded = Cell::new(false);
    let mut tar = contents(archive, limits, &exceeded);
    let mut seen: Vec<Vec<u8>> = Vec::new();
    let mut crucible = None;
    let mut broker = None;
    for entry in tar.entries().map_err(unreadable(&exceeded))? {
        let mut entry = entry.map_err(unreadable(&exceeded))?;
        let kind = entry.header().entry_type();
        let size = entry.header().entry_size().map_err(StageError::Corrupt)?;
        if entry.size() != size {
            return Err(StageError::Ambiguous);
        }
        if matches!(kind, EntryType::Symlink | EntryType::Link) {
            return Err(StageError::Link);
        }
        let place = {
            let path = entry.path_bytes();
            let name = if kind == EntryType::Directory {
                path.strip_suffix(b"/").unwrap_or(&path)
            } else {
                &path
            };
            let place = Place::of(name, stem).ok_or(StageError::Unexpected)?;
            if seen.iter().any(|before| before.as_slice() == name) {
                return Err(StageError::Duplicate);
            }
            seen.push(name.to_vec());
            place
        };
        if kind != place.kind() {
            return Err(StageError::Kind);
        }
        let ceiling = match place {
            Place::Release => 0,
            Place::Document => limits.document,
            Place::Crucible | Place::Broker => limits.executable,
        };
        if size > ceiling {
            return Err(StageError::TooLarge {
                part: StagePart::Member,
            });
        }
        match place {
            Place::Release | Place::Document => {}
            Place::Crucible => {
                let to = unit.join(CRUCIBLE);
                let part = StagePart::Executable;
                crucible = Some(unpack(&mut entry, size, &to, part, &exceeded)?);
                crossed("unpacked the executable");
            }
            Place::Broker => {
                let to = unit.join(BROKER);
                broker = Some(unpack(&mut entry, size, &to, StagePart::Broker, &exceeded)?);
                crossed("unpacked the broker");
            }
        }
    }
    ended(tar, &exceeded)?;
    let crucible = crucible.ok_or(StageError::Missing)?;
    Ok(Unpacked { crucible, broker })
}

/// Writes the member `entry`, of `size` bytes, to `to` as an executable and
/// returns its SHA-256.
fn unpack(
    entry: &mut impl Read,
    size: u64,
    to: &Path,
    part: StagePart,
    exceeded: &Cell<bool>,
) -> Result<Digest, StageError> {
    let mut file = create(to, part)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_BUFFER];
    let mut written: u64 = 0;
    loop {
        let read = match entry.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(unreadable(exceeded)(error)),
        };
        let chunk = buffer.get(..read).unwrap_or_default();
        hasher.update(chunk);
        file.write_all(chunk).map_err(io(part))?;
        written = written.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    if written != size {
        return Err(StageError::Corrupt(io::ErrorKind::UnexpectedEof.into()));
    }
    file.set_permissions(fs::Permissions::from_mode(0o755))
        .map_err(io(part))?;
    file.sync_all().map_err(io(part))?;
    Ok(Digest::new(hasher.finalize().into()))
}

/// Writes `receipt` into `unit` and reads it back, so a unit is only called
/// staged with a receipt the layout will take.
fn write_receipt(unit: &Path, receipt: &Receipt) -> Result<(), StageError> {
    let part = StagePart::Receipt;
    let mut file = create(&unit.join(RECEIPT), part)?;
    file.write_all(&receipt.to_bytes()).map_err(io(part))?;
    file.set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(io(part))?;
    file.sync_all().map_err(io(part))?;
    crossed("wrote the receipt");
    rewind(&file, part)?;
    let mut written = Vec::new();
    let ceiling = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX);
    file.take(ceiling.saturating_add(1))
        .read_to_end(&mut written)
        .map_err(io(part))?;
    let read = Receipt::parse(&written).map_err(StageError::Receipt)?;
    if &read != receipt {
        return Err(StageError::Readback);
    }
    Ok(())
}

/// Opens the staged unit to everyone's reading, as the installer leaves one,
/// and makes it durable.
fn seal(unit: &Path) -> Result<(), StageError> {
    let part = StagePart::Unit;
    fs::set_permissions(unit, fs::Permissions::from_mode(0o755)).map_err(io(part))?;
    File::open(unit)
        .and_then(|directory| directory.sync_all())
        .map_err(io(part))?;
    crucible_privacy::sync_parent(unit).map_err(|error| StageError::Io {
        part,
        source: error.into_io(),
    })
}

/// Creates a file only its owner can reach, failing when the name exists.
fn create(at: &Path, part: StagePart) -> Result<File, StageError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(at)
        .map_err(io(part))
}

/// Moves back to the start of `file`.
fn rewind(mut file: &File, part: StagePart) -> Result<(), StageError> {
    file.seek(SeekFrom::Start(0)).map(drop).map_err(io(part))
}

/// The refusal for a failure to read or write `part`.
fn io(part: StagePart) -> impl Fn(io::Error) -> StageError {
    move |source| StageError::Io { part, source }
}

/// The refusal for a failure the archive reader reports: the ceiling, when it
/// was passed, else an archive that is not one.
fn unreadable(exceeded: &Cell<bool>) -> impl Fn(io::Error) -> StageError + '_ {
    move |error| {
        if exceeded.get() {
            StageError::TooLarge {
                part: StagePart::Contents,
            }
        } else {
            StageError::Corrupt(error)
        }
    }
}

#[cfg(test)]
mod tests;
