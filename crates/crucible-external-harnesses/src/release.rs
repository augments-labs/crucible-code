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
//!
//! A release the agent catalog reviewed is unpacked by [`Unpacker`] instead,
//! one artifact at a time into a staging directory its caller owns, only as
//! the catalog lists it; the rules it holds an archive to are written above
//! it, in its own module. Its [`Unpacker::survey`] holds an archive to the
//! same rules with no list, so the list the catalog's intake records is one
//! an unpack accepts. A wheel of a release started by a pinned interpreter
//! is unpacked by [`Wheels`] into the same staging directory, by the same
//! rules, under `site/`, and [`LaunchScript`] is the file that starts it. A
//! release that Node runs is a package tree, which [`NpmTree`] reads from the
//! lock the catalog keeps for it and unpacks from its packages' tarballs by
//! npm's rules where they differ from these; [`Preload`] is the file that
//! confines Node to that tree, and each [`Stub`] a program of its `.bin` that
//! runs the pinned Node under it.
//!
//! A [`Pin`] borrows what it names: the build's own table lends `'static`
//! text, and an [`OwnedPin`], a release the agent catalog offers, lends text it
//! holds. Only a release that is one archive of exactly two ordinary files is a
//! pin; [`OwnedPin::pin`] says none for any other.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_types::AgentKey;
use sha2::{Digest as _, Sha256};
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

use crate::catalog::{Admits, FileKind, Files, Format, Launch, Target};

mod npm;
mod npm_launch;
mod unpack;
mod wheel;

pub use npm::{NpmError, NpmPackage, NpmTarball, NpmTree};
pub use npm_launch::{BIN, PRELOAD, Preload, Stub, StubError};
pub(crate) use unpack::{MEMBERS, link_resolves};
pub use unpack::{Staged, UnpackError, Unpacker};
pub use wheel::{LAUNCHER, LaunchScript, Wheels};

/// The most a release may be: its archive, and what it unpacks to.
pub const RELEASE_BYTES: u64 = 4 << 30;

/// The most of a name the archive gives that a refusal repeats.
const NAMED: usize = 64;

/// One release, as the archive it is published in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin<'a> {
    /// The release's version, which names the directory it is unpacked into.
    pub version: &'a str,
    /// Where the archive is published.
    pub url: &'a str,
    /// The archive's SHA-256, in lowercase hexadecimal.
    pub sha256: &'a str,
    /// The archive's size in bytes.
    pub archive: u64,
    /// The program that is started.
    pub executable: Member<'a>,
    /// The one other file the archive holds, which the program starts.
    pub companion: Member<'a>,
}

/// One file a pinned archive holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member<'a> {
    /// Its name, which is a file name alone.
    pub name: &'a str,
    /// Its size in bytes once unpacked.
    pub bytes: u64,
}

/// One release the trust state offers, for the platform this build runs on.
///
/// It is owned, so it outlives the catalog it was read from, and it holds what
/// an install needs of one entry and one of its targets: which release it is,
/// what the agent must call itself when it is run, and the target with its
/// archives, files and launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPin {
    /// The agent.
    pub key: AgentKey,
    /// The version.
    pub version: Box<str>,
    /// The revision of the version's package tree, for an agent whose tree is
    /// resolved again under the same version.
    pub lock_revision: Option<u64>,
    /// The name and version the agent must answer initializing with.
    pub admits: Admits,
    /// The agent as published for this platform.
    pub target: Target,
}

impl OwnedPin {
    /// The release as the one archive of two files [`unpack()`] handles, lent
    /// from what this holds; nothing for a release of any other shape: a
    /// launch that is not the agent itself, more or fewer than one archive,
    /// one that is not a zip or was built at review, a list of files that is
    /// not exactly two ordinary files, or a program the list does not name.
    #[must_use]
    pub fn pin(&self) -> Option<Pin<'_>> {
        if self.target.launch != Launch::Binary {
            return None;
        }
        let [archive] = self.target.artifacts.as_slice() else {
            return None;
        };
        if archive.format != Format::Zip || archive.replaced_by.is_some() {
            return None;
        }
        let Files::Listed(listed) = &archive.files else {
            return None;
        };
        let [first, second] = listed.as_slice() else {
            return None;
        };
        let (executable, companion) = if *first.path == *self.target.cmd {
            (first, second)
        } else {
            (second, first)
        };
        if *executable.path != *self.target.cmd || executable.path == companion.path {
            return None;
        }
        let bytes = archive
            .size
            .max(executable_bytes(executable)?.checked_add(executable_bytes(companion)?)?);
        if bytes > RELEASE_BYTES
            || !listed_host(&archive.url, &self.target.hosts)
            || executable.path.eq_ignore_ascii_case(&companion.path)
        {
            return None;
        }
        Some(Pin {
            version: &self.version,
            url: &archive.url,
            sha256: &archive.sha256,
            archive: archive.size,
            executable: member(executable)?,
            companion: member(companion)?,
        })
    }
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
    Kind(Box<str>),
    /// A pinned file in it is not its pinned size.
    #[error("{name} in the archive is not {pinned} bytes")]
    Member {
        /// The file.
        name: Box<str>,
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

impl<'a> Pin<'a> {
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
    fn member(&self, name: &str) -> Option<Member<'a>> {
        [self.executable, self.companion]
            .into_iter()
            .find(|member| member.name == name)
    }
}

/// `file` as a member of a pinned archive, where it is an ordinary file named
/// by a file name alone, which the unpacker would take as written, and of a
/// size a release may have.
fn member(file: &crate::catalog::File) -> Option<Member<'_>> {
    let FileKind::File { size, .. } = file.kind else {
        return None;
    };
    let plain = !file.path.contains('/')
        && unpack::named(file.path.as_bytes(), false).is_ok_and(|name| name == *file.path);
    (plain && size <= RELEASE_BYTES).then_some(Member {
        name: &file.path,
        bytes: size,
    })
}

/// The size of the ordinary file `file` is.
fn executable_bytes(file: &crate::catalog::File) -> Option<u64> {
    match file.kind {
        FileKind::File { size, .. } => Some(size),
        FileKind::Dir | FileKind::Link { .. } => None,
    }
}

/// Whether the host of `url`, which is an `https` address with no user and no
/// port, is one of `hosts`, compared as host names are.
fn listed_host(url: &str, hosts: &[Box<str>]) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
        && !host.contains(['@', ':', '[', ']'])
        && hosts.iter().any(|listed| listed.eq_ignore_ascii_case(host))
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
pub fn unpack(pin: &Pin<'_>, archive: &Path, root: &Path) -> Result<PathBuf, Unpacked> {
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
            return Err(Unpacked::Kind(member.name.into()));
        }
        if entry.size() != member.bytes {
            return Err(Unpacked::Member {
                name: member.name.into(),
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
                name: member.name.into(),
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
fn believed(pin: &Pin<'_>, file: &mut File) -> Result<(), Unpacked> {
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
/// it was published as, a catalog agent's archive, a wheel, and a package
/// tree's lock and tarball.
#[cfg(feature = "proof")]
pub mod proof {
    use std::fmt::Write as _;
    use std::io::{Cursor, Write as _};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::{Digest as _, Sha256, Sha512};
    use tar::{EntryType, Header};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::{Member, Pin, hex};

    /// The bytes of the release's executable.
    pub const EXECUTABLE: &[u8] = b"the agent's server";

    /// The bytes of the program it starts beside it.
    pub const COMPANION: &[u8] = b"the harness it starts";

    /// The licence a catalog agent's release ships beside them.
    pub const LICENSE: &[u8] = b"MIT License\n";

    /// The bytes of the one package of [`npm_tree`]'s `index.js`.
    pub const NPM_INDEX: &[u8] = b"module.exports = 1;\n";

    /// The bytes of the one package of [`npm_tree`]'s `bin/peer.js`.
    pub const NPM_BIN: &[u8] = b"#!/usr/bin/env node\n";

    /// A catalog agent's release for the intake to survey, and the artifact
    /// the catalog describes it as before a file list is recorded for it, as
    /// the JSON `crucible agents unpack-check` reads.
    ///
    /// The release is a gzip-compressed tar of the executable `agent`, its
    /// `LICENSE`, and a `lib` directory of the executable `harness` and a link
    /// `current` to it. Every header and the gzip stream are written with no
    /// time, owner or name in them, so the same bytes come out wherever it is
    /// made.
    ///
    /// # Panics
    ///
    /// When the archive cannot be written in memory, which leaves the test
    /// asking for it nothing to report.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn catalog_release() -> (Vec<u8>, Vec<u8>) {
        // Each member's name, kind, bytes, mode and link target.
        type Member<'a> = (&'a str, EntryType, &'a [u8], u32, Option<&'a str>);
        let mut builder = tar::Builder::new(Vec::new());
        let members: [Member<'_>; 5] = [
            ("agent", EntryType::Regular, EXECUTABLE, 0o755, None),
            ("LICENSE", EntryType::Regular, LICENSE, 0o644, None),
            ("lib/", EntryType::Directory, b"", 0o755, None),
            ("lib/harness", EntryType::Regular, COMPANION, 0o755, None),
            (
                "lib/current",
                EntryType::Symlink,
                b"",
                0o777,
                Some("harness"),
            ),
        ];
        for (name, kind, bytes, mode, link) in members {
            let mut header = Header::new_gnu();
            header.set_path(name).expect("a name");
            if let Some(link) = link {
                header.set_link_name(link).expect("a target");
            }
            header.set_entry_type(kind);
            header.set_size(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            header.set_mode(mode);
            header.set_mtime(0);
            header.set_cksum();
            builder.append(&header, bytes).expect("a member");
        }
        let tar = builder.into_inner().expect("a tar");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).expect("its bytes");
        let archive = encoder.finish().expect("a gzip stream");
        let described = serde_json::json!({
            "url": "https://example.invalid/releases/download/v1.0.0/agent.tar.gz",
            "sha256": hex(&Sha256::digest(&archive)),
            "size": archive.len(),
            "format": "tar.gz",
            "unpacked": 1 << 20,
            "decompressed": 1 << 20,
        });
        (archive, described.to_string().into_bytes())
    }

    /// A package tree's lock, as the JSON `crucible agents unpack-check`
    /// reads, the SHA-512 its one tarball is pinned by and kept under, and
    /// the tarball.
    ///
    /// The tree is one package, `peer` 1.0.0, installed at
    /// `node_modules/peer`. Its tarball is a gzip-compressed tar of
    /// `package/index.js`, holding [`NPM_INDEX`], and `package/bin/peer.js`,
    /// holding [`NPM_BIN`] and given a mode that runs it, written with no
    /// time or owner in them.
    ///
    /// # Panics
    ///
    /// When the tarball cannot be written in memory, which leaves the test
    /// asking for it nothing to report.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn npm_tree() -> (Vec<u8>, String, Vec<u8>) {
        let mut builder = tar::Builder::new(Vec::new());
        let members = [
            ("package/index.js", NPM_INDEX, 0o644),
            ("package/bin/peer.js", NPM_BIN, 0o755),
        ];
        for (name, bytes, mode) in members {
            let mut header = Header::new_gnu();
            header.set_path(name).expect("a name");
            header.set_entry_type(EntryType::Regular);
            header.set_size(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            header.set_mode(mode);
            header.set_mtime(0);
            header.set_cksum();
            builder.append(&header, bytes).expect("a member");
        }
        let tar = builder.into_inner().expect("a tar");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).expect("its bytes");
        let tarball = encoder.finish().expect("a gzip stream");
        let sha512 = hex(&Sha512::digest(&tarball));
        let lock = serde_json::json!({
            "unpack_rules": 1,
            "packages": [{
                "location": "node_modules/peer",
                "name": "peer",
                "version": "1.0.0",
                "url": "https://registry.npmjs.org/peer/-/peer-1.0.0.tgz",
                "size": tarball.len(),
                "sha512": sha512,
                "files": members.len(),
                "unpacked": NPM_INDEX.len() + NPM_BIN.len(),
            }],
        });
        (lock.to_string().into_bytes(), sha512, tarball)
    }

    /// Release 1.0.0's archive, of an `agent` and a `harness`, and its pin,
    /// published at `url`.
    ///
    /// # Panics
    ///
    /// When the archive cannot be written in memory, which leaves the test
    /// asking for it nothing to report.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn released(url: &'static str) -> (Vec<u8>, Pin<'static>) {
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

    /// A wheel of `members`, each a file made with mode `0644` and named as
    /// the wheel names it, then the `RECORD` of the `.dist-info` directory
    /// `dist_info` that agrees with them; and each file it installs, under
    /// `site/`, with its bytes, as a catalog lists them.
    ///
    /// # Panics
    ///
    /// When the archive cannot be written in memory, which leaves the test
    /// asking for it nothing to report.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn wheel(dist_info: &str, members: &[(&str, &[u8])]) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
        let record_name = format!("{dist_info}/RECORD");
        let mut record = String::new();
        for (name, bytes) in members {
            let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(bytes));
            let _ = writeln!(record, "{name},sha256={hash},{}", bytes.len());
        }
        let _ = writeln!(record, "{record_name},,");
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .system(zip::System::Unix)
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let mut installed = Vec::new();
        let all = members
            .iter()
            .copied()
            .chain([(record_name.as_str(), record.as_bytes())]);
        for (name, bytes) in all {
            writer.start_file(name, options).expect("a member");
            writer.write_all(bytes).expect("its bytes");
            installed.push((format!("site/{name}"), bytes.to_vec()));
        }
        (writer.finish().expect("a wheel").into_inner(), installed)
    }
}

#[cfg(test)]
mod tests;
