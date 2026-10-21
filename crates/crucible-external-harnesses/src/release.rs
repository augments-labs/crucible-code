//! A release of an agent that crucible keeps for the user, unpacked from the
//! one archive it was pinned as.
//!
//! The archive is believed only once its size and SHA-256 are the pin's, and
//! even then it is read as hostile: it must hold exactly the two files the pin
//! names, each an ordinary file, stored or deflated and unencrypted, under its
//! file name alone and of its pinned size, and no more of a member than that
//! size is ever written. The two are written into a directory of this unpack's
//! own beside the release's, owner-only, then renamed into place as one, so a
//! release is either whole under its version or absent; what an unpack leaves
//! unfinished is removed with it.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

/// The most of a name the archive gives that a refusal repeats.
const NAMED: usize = 64;

/// One release, as the archive it is published in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    /// The release's version, which names the directory it is unpacked into.
    pub version: &'static str,
    /// Where the archive is published.
    pub url: &'static str,
    /// The archive's SHA-256, in lowercase hexadecimal.
    pub sha256: &'static str,
    /// The archive's size in bytes.
    pub archive: u64,
    /// The program that is started.
    pub executable: Member,
    /// The one other file the archive holds, which the program starts.
    pub companion: Member,
}

/// One file a pinned archive holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    /// Its name, which is a file name alone.
    pub name: &'static str,
    /// Its size in bytes once unpacked.
    pub bytes: u64,
}

/// Why a downloaded archive was not unpacked.
#[derive(Debug, thiserror::Error)]
pub enum Unpacked {
    /// Its SHA-256 is not the pin's.
    #[error("the archive is not the pinned release")]
    Digest,
    /// Its size is not the pin's.
    #[error("the archive is {held} bytes, not the {pinned} the release was pinned at")]
    Size {
        /// What it holds.
        held: u64,
        /// What the pin says.
        pinned: u64,
    },
    /// It holds another number of files than the pin's two.
    #[error("the archive holds {0} files, not the 2 the release was pinned with")]
    Members(usize),
    /// It holds a file the pin does not name, given cut short.
    #[error("the archive holds a file the release was not pinned with: {0:?}")]
    Unpinned(String),
    /// A pinned file in it is a directory, a link, encrypted, or compressed
    /// another way than stored or deflated.
    #[error("{0} in the archive is not an ordinary file")]
    Kind(&'static str),
    /// A pinned file in it is not its pinned size.
    #[error("{name} in the archive is not {pinned} bytes")]
    Member {
        /// The file.
        name: &'static str,
        /// Its pinned size.
        pinned: u64,
    },
    /// The archive could not be read.
    #[error("the archive could not be read")]
    Read(#[source] io::Error),
    /// The archive is not a zip, or not one this reader reads.
    #[error("the archive is not a zip that can be read")]
    Zip(#[from] ZipError),
    /// The release could not be written.
    #[error("the release could not be written under {}", .path.display())]
    Write {
        /// The directory it was being written under.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: io::Error,
    },
    /// Something other than this release is already where it goes.
    #[error("{} holds something other than this release", .0.display())]
    InTheWay(PathBuf),
}

impl Pin {
    /// The executable's path once unpacked under `root`.
    #[must_use]
    pub fn executable(&self, root: &Path) -> PathBuf {
        root.join(self.version).join(self.executable.name)
    }

    /// Whether the release is unpacked under `root`: both its files there as
    /// ordinary files of their pinned sizes, and on Unix the executable one
    /// that can be run.
    #[must_use]
    pub fn installed(&self, root: &Path) -> bool {
        let directory = root.join(self.version);
        [self.executable, self.companion].iter().all(|member| {
            fs::symlink_metadata(directory.join(member.name)).is_ok_and(|metadata| {
                metadata.is_file() && metadata.len() == member.bytes && runnable(&metadata)
            })
        })
    }

    /// The member the archive names `name`, where the pin names it.
    fn member(&self, name: &str) -> Option<Member> {
        [self.executable, self.companion]
            .into_iter()
            .find(|member| member.name == name)
    }
}

/// Unpacks `archive`, the release `pin` names, into its version under `root`,
/// and gives the executable's path.
///
/// A release already whole there is kept as it is. `root` is made owner-only
/// where it is missing.
///
/// # Errors
///
/// [`Unpacked`] naming the first way the archive is not the one pinned, or
/// could not be read or written.
pub fn unpack(pin: &Pin, archive: &Path, root: &Path) -> Result<PathBuf, Unpacked> {
    let mut file = File::open(archive).map_err(Unpacked::Read)?;
    believed(pin, &mut file)?;
    let mut zip = ZipArchive::new(file)?;
    if zip.len() != 2 {
        return Err(Unpacked::Members(zip.len()));
    }
    let written = |source| Unpacked::Write {
        path: root.to_owned(),
        source,
    };
    crucible_privacy::directory(root).map_err(|error| written(error.into_io()))?;
    let staging = Staging::new(root).map_err(written)?;
    // The reader keeps one entry for a name the archive gives twice, and a
    // file is created only where none is, so neither member is written twice.
    for index in 0..2 {
        let mut entry = zip.by_index(index)?;
        let name = entry.name()?.into_owned();
        let member = pin
            .member(&name)
            .ok_or_else(|| Unpacked::Unpinned(name.chars().take(NAMED).collect()))?;
        let ordinary = entry.is_file()
            && !entry.is_symlink()
            && !entry.encrypted()
            && matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            );
        if !ordinary {
            return Err(Unpacked::Kind(member.name));
        }
        if entry.size() != member.bytes {
            return Err(Unpacked::Member {
                name: member.name,
                pinned: member.bytes,
            });
        }
        let path = staging.0.join(member.name);
        let mut out = created(&path).map_err(written)?;
        let copied = io::copy(
            &mut (&mut entry).take(member.bytes.saturating_add(1)),
            &mut out,
        )
        .map_err(Unpacked::Read)?;
        if copied != member.bytes {
            return Err(Unpacked::Member {
                name: member.name,
                pinned: member.bytes,
            });
        }
        out.sync_all().map_err(written)?;
    }
    let directory = root.join(pin.version);
    match fs::rename(&staging.0, &directory) {
        Ok(()) => {
            crucible_privacy::sync_parent(&directory).map_err(|error| written(error.into_io()))?;
        }
        // Another unpack may have put the same release there first.
        Err(_) if pin.installed(root) => {}
        Err(_) if directory.exists() => return Err(Unpacked::InTheWay(directory)),
        Err(error) => return Err(written(error)),
    }
    Ok(pin.executable(root))
}

/// Whether `file` is the archive `pin` names: its size, then its SHA-256.
fn believed(pin: &Pin, file: &mut File) -> Result<(), Unpacked> {
    let held = file.metadata().map_err(Unpacked::Read)?.len();
    if held != pin.archive {
        return Err(Unpacked::Size {
            held,
            pinned: pin.archive,
        });
    }
    let mut digest = Sha256::new();
    let mut chunk = vec![0_u8; 1 << 16];
    let mut read: u64 = 0;
    loop {
        let got = match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(got) => got,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Unpacked::Read(error)),
        };
        read = read.saturating_add(u64::try_from(got).unwrap_or(u64::MAX));
        if read > pin.archive {
            return Err(Unpacked::Size {
                held: read,
                pinned: pin.archive,
            });
        }
        digest.update(chunk.get(..got).unwrap_or_default());
    }
    if hex(&digest.finalize()) != pin.sha256 {
        return Err(Unpacked::Digest);
    }
    io::Seek::rewind(file).map_err(Unpacked::Read)
}

/// A directory of this unpack's own under the release's root, removed with
/// the value unless it was renamed into place.
struct Staging(PathBuf);

impl Staging {
    fn new(root: &Path) -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
        for attempt in 0..16_u32 {
            let at = root.join(format!(
                ".staging-{}-{nanos:x}-{attempt}",
                std::process::id()
            ));
            // Made, not found: a name somebody else made first is passed by.
            match fs::create_dir(&at) {
                Ok(()) => {
                    let staging = Self(at);
                    crucible_privacy::directory(&staging.0)
                        .map_err(crucible_privacy::PrivacyError::into_io)?;
                    return Ok(staging);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = error,
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A new file at `path`, which on Unix can be run.
fn created(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
    options.open(path)
}

/// Whether a file with `metadata` can be run; on Windows every file can.
#[cfg(unix)]
fn runnable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn runnable(_metadata: &fs::Metadata) -> bool {
    true
}

/// `bytes` as lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// A release a test can publish: an archive of two small members and the pin
/// it was published as.
#[cfg(feature = "proof")]
pub mod proof {
    use std::io::{Cursor, Write as _};

    use sha2::{Digest as _, Sha256};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::{Member, Pin, hex};

    /// The bytes of the release's executable.
    pub const EXECUTABLE: &[u8] = b"the agent's server";

    /// The bytes of the program it starts beside it.
    pub const COMPANION: &[u8] = b"the harness it starts";

    /// Release 1.0.0's archive, of an `agent` and a `harness`, and its pin,
    /// published at `url`.
    ///
    /// # Panics
    ///
    /// When the archive cannot be written in memory, which leaves the test
    /// asking for it nothing to report.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn released(url: &'static str) -> (Vec<u8>, Pin) {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in [("agent", EXECUTABLE), ("harness", COMPANION)] {
            writer.start_file(name, options).expect("a member");
            writer.write_all(bytes).expect("its bytes");
        }
        let archive = writer.finish().expect("an archive").into_inner();
        let pin = Pin {
            version: "1.0.0",
            url,
            sha256: hex(&Sha256::digest(&archive)).leak(),
            archive: u64::try_from(archive.len()).unwrap_or(u64::MAX),
            executable: Member {
                name: "agent",
                bytes: u64::try_from(EXECUTABLE.len()).unwrap_or(u64::MAX),
            },
            companion: Member {
                name: "harness",
                bytes: u64::try_from(COMPANION.len()).unwrap_or(u64::MAX),
            },
        };
        (archive, pin)
    }
}

#[cfg(test)]
mod tests;
