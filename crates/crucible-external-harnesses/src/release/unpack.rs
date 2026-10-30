//! One artifact of a catalog agent's release, unpacked into a staging
//! directory only as the catalog lists it, or surveyed so that a list can be
//! recorded for it.
//!
//! The artifact is a zip, a gzip-compressed tar or the program itself, and it
//! is read as hostile however it was pinned:
//!
//! - It is believed only once its size and SHA-256 are its pin's: the
//!   artifact's own, or the asset's that was built at review in its place.
//!   Nothing of it is read before.
//! - Every member's name is held to one set of rules before anything else is
//!   done with it, whether it is written or not: at most [`NAME_BYTES`] long,
//!   with at most [`COMPONENTS`] components of at most [`COMPONENT_BYTES`]
//!   each, which is checked before a byte of it is copied; printable ASCII
//!   alone, so it holds no control character and no two spellings of it
//!   normalise alike; relative, with no backslash, colon or character Windows
//!   refuses in a name; no empty, `.` or `..` component; and no component that
//!   ends in a dot or a space or names a device Windows reserves, with or
//!   without an extension and spaces before it. A name is refused rather than
//!   resolved, but for one leading `./` and a member that is `.` itself, which
//!   are the directory an archive was made from: the first is dropped, and the
//!   second is the staging directory.
//! - A member is written when it is kept: every member when the artifact
//!   names none, and otherwise only those it names. The rest are read through
//!   and discarded, whatever kind they are, and nothing of them is written;
//!   but a sparse tar member is refused even so, since `tar` reads what comes
//!   after its header as more of it, where this reader would see members.
//! - What is written is an ordinary file, a directory, a symbolic link or a
//!   copy of an earlier file that a hard link names. A zip's link, a device, a
//!   pipe and every other kind are refused. A file is runnable exactly when
//!   the archive's mode for it has an execute bit, which a zip member with no
//!   Unix mode has not, or when it is a program published bare; and no more of
//!   it is written than its size. On Unix a file is made with that mode and a
//!   directory owner-only, each as it is created, both less what the umask
//!   takes away.
//! - Unpacked against a list, every member written must be listed as it is,
//!   its kind, size, SHA-256, exec flag or link target, and every member
//!   listed must be written. A member the list does not name, or names
//!   otherwise, is refused before a byte of it is written; only its SHA-256
//!   waits until it has been. A survey holds the archive to every rule but
//!   that one, and records what it wrote.
//! - The names written are compared case-folded, so two that one file system
//!   takes for one are refused, and no name is written twice, or as a
//!   directory and as something else.
//! - A link is made last, with the target the archive gives it, and only when
//!   that target resolves to a file written here, inside the staging
//!   directory, through directories alone and at most [`LINK_STEPS`] links.
//! - What is read and written is bounded: no more than the artifact's
//!   `decompressed` is read out of the archive, the members discarded
//!   included, no more than its `unpacked` is written, no more names than
//!   [`MEMBERS`], each directory a member only implies counted, and no more
//!   members than its list holds by an unpack. A
//!   list's sizes are summed with checked arithmetic before the archive is
//!   opened. A tar's GNU long name or link target is held to the name's
//!   ceiling, and its extended header to [`PAX_BYTES`], before either is
//!   read. What is held of the names written grows only with their own
//!   bytes: each component of a directory is held once, in a tree, and each
//!   name written once more for what was staged.
//! - The intake and the user's machine read an archive with this one reader,
//!   and the pin fixes its bytes, so what a survey recorded is what an unpack
//!   reads.
//!
//! The staging directory is the caller's, since one version can be made of
//! several artifacts: this writes only under it, creates nothing that is
//! already there, and leaves removing it after a refusal to its owner.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Cursor, Read, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use flate2::bufread::GzDecoder;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tar::EntryType;
use zip::ZipArchive;
use zip::result::ZipError;

use super::{NAMED, hex};
use crate::catalog::{Artifact, File, FileKind, Files, Format};

/// The most links a link's target is followed through, itself included.
const LINK_STEPS: usize = 8;

/// The most members a survey writes, and the most names either writes, each
/// directory a member only implies counted; an unpack writes no more members
/// than its list holds.
pub(crate) const MEMBERS: usize = 65_536;

/// The longest name a member may have, and a link's target, in bytes.
const NAME_BYTES: usize = 4096;

/// The longest component of a name, in bytes.
const COMPONENT_BYTES: usize = 255;

/// The most components a name may have.
const COMPONENTS: usize = 64;

/// The longest extended header a tar member may be described in, in bytes.
const PAX_BYTES: u64 = 64 * 1024;

/// The devices Windows reserves a name for whatever follows it after a dot or
/// a space, besides `com0` to `com9` and `lpt0` to `lpt9`.
const DEVICES: [&str; 6] = ["con", "prn", "aux", "nul", "conin$", "conout$"];

/// The characters Windows refuses in a name, besides the backslash and the
/// colon refused as a separator and a drive.
const WINDOWS_REFUSES: [char; 6] = ['<', '>', '"', '|', '?', '*'];

/// The file-type bits of a Unix mode.
const TYPE: u32 = 0o170_000;

/// A directory's file-type bits.
const DIRECTORY: u32 = 0o040_000;

/// An ordinary file's file-type bits.
const REGULAR: u32 = 0o100_000;

/// A link's file-type bits.
const SYMLINK: u32 = 0o120_000;

/// How a catalog artifact is unpacked.
#[derive(Debug, Clone, Copy)]
pub struct Unpacker;

/// What one artifact put in a staging directory.
#[derive(Debug)]
pub struct Staged {
    directory: PathBuf,
    files: Vec<File>,
    unpacked: u64,
    decompressed: u64,
}

/// Why an artifact was not unpacked.
#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    /// Its size is not its pin's.
    #[error("the download is {held} bytes, not the {pinned} it was pinned at")]
    Size {
        /// What it holds.
        held: u64,
        /// What the pin says.
        pinned: u64,
    },
    /// Its SHA-256 is not its pin's.
    #[error("the download is not the artifact the catalog pinned")]
    Digest,
    /// It is a wheel, which has a reader of its own.
    #[error("a wheel is not unpacked by the reader for archives and programs")]
    Wheel,
    /// Its file list is a catalog asset, which is fetched and read into a
    /// list before the artifact is unpacked.
    #[error("the artifact's file list is a catalog asset that has not been read")]
    Asset,
    /// It is the program itself, and is listed as other than one file.
    #[error("a program published bare is listed as exactly one file")]
    Bare,
    /// Its list adds up to more than its unpacked size.
    #[error("the artifact's file list adds up to more than its {ceiling} unpacked bytes")]
    ListedPast {
        /// Its unpacked size.
        ceiling: u64,
    },
    /// Its list names one path twice.
    #[error("the artifact's file list names {name:?} twice")]
    ListedTwice {
        /// The path, cut short.
        name: String,
    },
    /// A name, or a component of one, is longer than its ceiling.
    #[error("the archive holds a name longer than {ceiling} bytes, {name:?}")]
    Overlong {
        /// The name, cut short.
        name: String,
        /// The most it may hold.
        ceiling: usize,
    },
    /// A name has more components than its ceiling.
    #[error("the archive holds a name deeper than {ceiling} components, {name:?}")]
    Deep {
        /// The name, cut short.
        name: String,
        /// The most it may have.
        ceiling: usize,
    },
    /// An extended header describing a member is longer than its ceiling.
    #[error("the archive describes a member in an extended header of more than {ceiling} bytes")]
    Extended {
        /// The most it may hold.
        ceiling: u64,
    },
    /// It would write more members, or more names with the directories they
    /// imply, than its ceiling.
    #[error("the archive holds more than the {ceiling} members and directories it may write")]
    Members {
        /// The most it may write.
        ceiling: usize,
    },
    /// A member's name is not one a file may be unpacked under.
    #[error("the archive holds a member named {name:?}, which {why}")]
    Name {
        /// The name, cut short.
        name: String,
        /// What is wrong with it.
        why: &'static str,
    },
    /// Two names it would write differ only in case.
    #[error("the archive holds {name:?} and another name that differs from it only in case")]
    Collision {
        /// The second, cut short.
        name: String,
    },
    /// It holds a name twice, or as a directory and as something else.
    #[error("the archive holds {name:?} twice, or as a directory and as something else")]
    Twice {
        /// The name, cut short.
        name: String,
    },
    /// A member it would write is of a kind that is not unpacked.
    #[error("{name:?} in the archive is {kind}, which is not unpacked")]
    Kind {
        /// The member, cut short.
        name: String,
        /// What it is.
        kind: &'static str,
    },
    /// It holds a member the list does not name.
    #[error("the archive holds {name:?}, which the artifact's file list does not")]
    Unlisted {
        /// The member, cut short.
        name: String,
    },
    /// A member is not as the list gives it.
    #[error("{name:?} in the archive has {what} other than the file list gives it")]
    Differs {
        /// The member, cut short.
        name: String,
        /// What differs.
        what: &'static str,
    },
    /// A member the list names, or the artifact keeps, is not in it.
    #[error("the archive holds no {name:?}, which the artifact names")]
    Missing {
        /// The member, cut short.
        name: String,
    },
    /// A link it would make does not resolve inside the version.
    #[error("the link {name:?} in the archive {why}")]
    Link {
        /// The link, cut short.
        name: String,
        /// Why it is not made.
        why: &'static str,
    },
    /// It would write more than its unpacked size.
    #[error("the archive unpacks to more than its {ceiling} unpacked bytes")]
    Unpacked {
        /// Its unpacked size.
        ceiling: u64,
    },
    /// It decompresses to more than its ceiling.
    #[error("the archive decompresses to more than its {ceiling} bytes")]
    Decompressed {
        /// The ceiling.
        ceiling: u64,
    },
    /// It is not an archive of its format that can be read.
    #[error("the archive could not be read")]
    Corrupt(#[source] io::Error),
    /// It is not a zip that can be read.
    #[error("the archive is not a zip that can be read")]
    Zip(#[from] ZipError),
    /// What it unpacks to could not be written.
    #[error("the artifact could not be written at {}", .path.display())]
    Write {
        /// Where.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: io::Error,
    },
}

impl Unpacker {
    /// Unpacks `bytes`, the download `artifact` pins, into `staging`, a
    /// directory the caller made, exactly as its file list gives it.
    ///
    /// # Errors
    ///
    /// [`UnpackError`] naming the first way the download is not the artifact
    /// listed, or could not be read or written. What was written before the
    /// refusal stays under `staging` for its owner to remove.
    pub fn unpack(
        bytes: &[u8],
        artifact: &Artifact,
        staging: &Path,
    ) -> Result<Staged, UnpackError> {
        pinned(bytes, artifact)?;
        let Files::Listed(files) = &artifact.files else {
            return Err(UnpackError::Asset);
        };
        let list = listing(files, artifact.unpacked)?;
        Walk::new(artifact, staging, Some(list)).run(Cursor::new(bytes))
    }

    /// Unpacks the download `artifact` pins, read from `file` rather than
    /// held in memory, into `staging`, exactly as [`Unpacker::unpack`] does:
    /// the file is read through once for its size and SHA-256 before anything
    /// else is done with it, and then read again as the archive.
    ///
    /// # Errors
    ///
    /// As [`Unpacker::unpack`], and [`UnpackError::Corrupt`] where the file
    /// could not be read.
    pub fn unpack_from(
        file: &mut fs::File,
        artifact: &Artifact,
        staging: &Path,
    ) -> Result<Staged, UnpackError> {
        pinned_file(file, artifact)?;
        let Files::Listed(files) = &artifact.files else {
            return Err(UnpackError::Asset);
        };
        let list = listing(files, artifact.unpacked)?;
        Walk::new(artifact, staging, Some(list)).run(io::BufReader::new(file))
    }

    /// Unpacks `bytes`, the download `artifact` pins, into `staging`, a
    /// directory the caller made, by every rule [`Unpacker::unpack`] holds an
    /// archive to but its file list, which is not read, and gives the list it
    /// would then accept.
    ///
    /// # Errors
    ///
    /// [`UnpackError`] naming the first rule the download breaks, or what could
    /// not be read or written.
    pub fn survey(
        bytes: &[u8],
        artifact: &Artifact,
        staging: &Path,
    ) -> Result<Staged, UnpackError> {
        pinned(bytes, artifact)?;
        Walk::new(artifact, staging, None).run(Cursor::new(bytes))
    }
}

impl Staged {
    /// The staging directory it is under.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// What was written, in the archive's order.
    #[must_use]
    pub fn files(&self) -> &[File] {
        &self.files
    }

    /// The bytes written.
    #[must_use]
    pub fn unpacked(&self) -> u64 {
        self.unpacked
    }

    /// The bytes read out of the archive once decompressed, the members
    /// discarded included.
    #[must_use]
    pub fn decompressed(&self) -> u64 {
        self.decompressed
    }

    /// What was staged, as the catalog records it for an artifact: its
    /// `unpacked` and `decompressed` sizes and its `files`.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({
            "unpacked": self.unpacked,
            "decompressed": self.decompressed,
            "files": self.files.iter().map(File::json).collect::<Vec<_>>(),
        })
    }
}

/// Whether `bytes` are what `artifact` pins: their size, then their SHA-256.
fn pinned(bytes: &[u8], artifact: &Artifact) -> Result<(), UnpackError> {
    let (sha256, size) = match &artifact.replaced_by {
        Some(asset) => (&asset.sha256, asset.size),
        None => (&artifact.sha256, artifact.size),
    };
    let held = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if held != size {
        return Err(UnpackError::Size { held, pinned: size });
    }
    if hex(&Sha256::digest(bytes)) != **sha256 {
        return Err(UnpackError::Digest);
    }
    Ok(())
}

/// Whether what `file` holds is what `artifact` pins, read through from its
/// start within one byte past the pinned size, and the file is left at its
/// start again.
fn pinned_file(file: &mut fs::File, artifact: &Artifact) -> Result<(), UnpackError> {
    let (sha256, size) = match &artifact.replaced_by {
        Some(asset) => (&asset.sha256, asset.size),
        None => (&artifact.sha256, artifact.size),
    };
    file.seek(io::SeekFrom::Start(0))
        .map_err(UnpackError::Corrupt)?;
    let mut digest = Sha256::new();
    let mut held: u64 = 0;
    let mut reading = Read::take(&mut *file, size.saturating_add(1));
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = reading.read(&mut chunk).map_err(UnpackError::Corrupt)?;
        if read == 0 {
            break;
        }
        digest.update(chunk.get(..read).unwrap_or_default());
        held = held.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    if held != size {
        return Err(UnpackError::Size { held, pinned: size });
    }
    if hex(&digest.finalize()) != **sha256 {
        return Err(UnpackError::Digest);
    }
    file.seek(io::SeekFrom::Start(0))
        .map_err(UnpackError::Corrupt)?;
    Ok(())
}

/// `files` by path, once no path is listed twice and their sizes add up to
/// no more than `unpacked`.
fn listing(files: &[File], unpacked: u64) -> Result<HashMap<&str, &FileKind>, UnpackError> {
    let mut list = HashMap::with_capacity(files.len());
    let mut total: u64 = 0;
    for file in files {
        if list.insert(&*file.path, &file.kind).is_some() {
            return Err(UnpackError::ListedTwice {
                name: shown(file.path.as_bytes()),
            });
        }
        if let FileKind::File { size, .. } = file.kind {
            total = total
                .checked_add(size)
                .ok_or(UnpackError::ListedPast { ceiling: unpacked })?;
        }
    }
    if total > unpacked {
        return Err(UnpackError::ListedPast { ceiling: unpacked });
    }
    Ok(list)
}

/// At most [`NAMED`] characters of `name`, for a refusal to repeat.
fn shown(name: &[u8]) -> String {
    String::from_utf8_lossy(name).chars().take(NAMED).collect()
}

/// Refuses `raw` where it is longer than a name may be, before any of it is
/// copied.
fn sized(raw: &[u8]) -> Result<(), UnpackError> {
    // A directory's name may end in one `/` more.
    if raw.len() > NAME_BYTES + 1 {
        return Err(UnpackError::Overlong {
            name: shown(raw),
            ceiling: NAME_BYTES,
        });
    }
    Ok(())
}

/// The name a member is unpacked under, from the `raw` name the archive gives
/// it, held to the rules every name is: a directory's may end in one `/`, and
/// one leading `./` is the directory the archive was made from, so it is
/// dropped.
pub(super) fn named(raw: &[u8], directory: bool) -> Result<String, UnpackError> {
    let refused = |why| UnpackError::Name {
        name: shown(raw),
        why,
    };
    sized(raw)?;
    let raw = if directory {
        raw.strip_suffix(b"/").unwrap_or(raw)
    } else {
        raw
    };
    let raw = raw.strip_prefix(b"./").unwrap_or(raw);
    if raw.len() > NAME_BYTES {
        return Err(UnpackError::Overlong {
            name: shown(raw),
            ceiling: NAME_BYTES,
        });
    }
    if raw.split(|byte| *byte == b'/').count() > COMPONENTS {
        return Err(UnpackError::Deep {
            name: shown(raw),
            ceiling: COMPONENTS,
        });
    }
    if raw
        .split(|byte| *byte == b'/')
        .any(|part| part.len() > COMPONENT_BYTES)
    {
        return Err(UnpackError::Overlong {
            name: shown(raw),
            ceiling: COMPONENT_BYTES,
        });
    }
    if let Some(byte) = raw
        .iter()
        .copied()
        .find(|byte| !matches!(byte, b' '..=b'~'))
    {
        return Err(refused(if byte.is_ascii() {
            "holds a control character"
        } else {
            "is not ASCII, and a name that is not can be spelled another way that reads alike"
        }));
    }
    // Every byte is printable ASCII, so this is the name as it was given.
    let name = String::from_utf8_lossy(raw).into_owned();
    if name.starts_with('/') {
        return Err(refused("is absolute"));
    }
    if name.contains(['\\', ':']) {
        return Err(refused("holds a backslash, a colon or a drive"));
    }
    if name.contains(WINDOWS_REFUSES) {
        return Err(refused("holds a character Windows refuses in a name"));
    }
    for part in name.split('/') {
        let why = match part {
            "" => "has an empty component",
            "." => "has a `.` component",
            ".." => "climbs out with `..`",
            _ if part.ends_with(['.', ' ']) => "has a component ending in a dot or a space",
            _ if reserved(part) => "names a device Windows reserves",
            _ => continue,
        };
        return Err(refused(why));
    }
    Ok(name)
}

/// Whether Windows takes `part` for a device, in any case and whatever
/// follows it after a dot, with or without spaces before that.
fn reserved(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end_matches(' ')
        .to_ascii_lowercase();
    DEVICES.contains(&stem.as_str())
        || ["com", "lpt"].iter().any(|device| {
            stem.strip_prefix(device)
                .is_some_and(|digit| digit.len() == 1 && digit.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// The target a link the archive gives points at, once it is relative
/// printable ASCII with no backslash, colon or empty component; it may climb
/// with `..`, and where it resolves is decided once every member is known.
fn linked(name: &str, raw: &[u8]) -> Result<String, UnpackError> {
    if raw.len() > NAME_BYTES {
        return Err(UnpackError::Overlong {
            name: shown(raw),
            ceiling: NAME_BYTES,
        });
    }
    let relative = !raw.is_empty()
        && raw.iter().all(|byte| matches!(byte, b' '..=b'~'))
        && !raw.starts_with(b"/")
        && !raw.contains(&b'\\')
        && !raw.contains(&b':')
        && raw.split(|byte| *byte == b'/').all(|part| !part.is_empty());
    if relative {
        Ok(String::from_utf8_lossy(raw).into_owned())
    } else {
        Err(UnpackError::Link {
            name: shown(name.as_bytes()),
            why: "is not a relative link",
        })
    }
}

/// The data of a file to be written, its size and whether it is runnable.
struct Incoming<'r> {
    data: &'r mut dyn Read,
    size: u64,
    exec: bool,
}

/// What a member of an archive is, as the walk is handed it.
enum Body<'r> {
    /// An ordinary file.
    File(Incoming<'r>),
    /// A directory.
    Dir,
    /// A symbolic link, to its raw target.
    Symlink(Vec<u8>),
    /// A hard link, to the raw name of an earlier member, and whether it is
    /// runnable.
    Hard { target: Vec<u8>, exec: bool },
    /// Something that is not unpacked, said as what it is.
    Refused(&'static str),
}

/// How a name written is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// A directory a member under it made.
    Implied,
    /// A directory the archive gives.
    Directory,
    /// A file or a link.
    Other,
}

/// Every name written and every directory a member under it implies, as a
/// tree of their components: each component is held once as spelled and once
/// case-folded, so what is held grows with the names' own bytes.
struct Names {
    /// The most names it holds.
    ceiling: usize,
    /// Each node, by its parent's and its component case-folded.
    children: HashMap<(usize, Box<str>), usize>,
    /// Each node's component as spelled, and how it is held; the staging
    /// directory itself is the first.
    nodes: Vec<(Box<str>, Held)>,
}

impl Names {
    fn new(ceiling: usize) -> Self {
        Self {
            ceiling,
            children: HashMap::new(),
            nodes: vec![(Box::default(), Held::Directory)],
        }
    }

    /// Holds `name` as `held`, with every directory above it, refusing a name
    /// another differs from only in case, a name taken twice, a directory
    /// that is also something else, and a name past the ceiling.
    fn claim(&mut self, name: &str, held: Held) -> Result<(), UnpackError> {
        let mut node = 0;
        let mut parts = name.split('/').peekable();
        while let Some(part) = parts.next() {
            let wanted = if parts.peek().is_some() {
                Held::Implied
            } else {
                held
            };
            node = self.hold(node, part, wanted, name)?;
        }
        Ok(())
    }

    /// Holds `part` under `parent` as `held`, for the name `name`, and gives
    /// its node.
    fn hold(
        &mut self,
        parent: usize,
        part: &str,
        held: Held,
        name: &str,
    ) -> Result<usize, UnpackError> {
        let key = (parent, part.to_ascii_lowercase().into_boxed_str());
        let Some(&node) = self.children.get(&key) else {
            // The first node is the staging directory, which is no name.
            if self.nodes.len() > self.ceiling {
                return Err(UnpackError::Members {
                    ceiling: self.ceiling,
                });
            }
            let node = self.nodes.len();
            self.nodes.push((part.into(), held));
            self.children.insert(key, node);
            return Ok(node);
        };
        let shown = || shown(name.as_bytes());
        let Some((spelled, before)) = self.nodes.get_mut(node) else {
            return Err(UnpackError::Twice { name: shown() });
        };
        if &**spelled != part {
            return Err(UnpackError::Collision { name: shown() });
        }
        match (*before, held) {
            (Held::Implied | Held::Directory, Held::Implied) => Ok(node),
            (Held::Implied, Held::Directory) => {
                *before = Held::Directory;
                Ok(node)
            }
            _ => Err(UnpackError::Twice { name: shown() }),
        }
    }

    /// Whether `name`, spelled exactly so, is a directory written or implied
    /// here; the staging directory itself is the empty name.
    fn is_directory(&self, name: &str) -> bool {
        if name.is_empty() {
            return true;
        }
        let mut node = 0;
        for part in name.split('/') {
            let key = (node, part.to_ascii_lowercase().into_boxed_str());
            let Some(&next) = self.children.get(&key) else {
                return false;
            };
            if self
                .nodes
                .get(next)
                .is_none_or(|(spelled, _)| &**spelled != part)
            {
                return false;
            }
            node = next;
        }
        self.nodes
            .get(node)
            .is_some_and(|(_, held)| *held != Held::Other)
    }
}

/// The ceiling on what is read out of an archive, and how much of it is left.
struct Budget {
    ceiling: u64,
    left: Cell<u64>,
    exceeded: Cell<bool>,
}

impl Budget {
    fn new(ceiling: u64) -> Self {
        Self {
            ceiling,
            left: Cell::new(ceiling),
            exceeded: Cell::new(false),
        }
    }

    /// What has been read so far.
    fn spent(&self) -> u64 {
        self.ceiling.saturating_sub(self.left.get())
    }

    /// The refusal for a failure to read the archive: its ceiling, where it was
    /// passed, else an archive that is not one.
    fn failed(&self, error: io::Error) -> UnpackError {
        if self.exceeded.get() {
            UnpackError::Decompressed {
                ceiling: self.ceiling,
            }
        } else {
            UnpackError::Corrupt(error)
        }
    }
}

/// A reader that fails, and says so in its budget, once more than the budget
/// has come through it.
struct Bounded<'b, R> {
    inner: R,
    budget: &'b Budget,
}

impl<R: Read> Read for Bounded<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let left = self.budget.left.get();
        let room = usize::try_from(left.saturating_add(1)).unwrap_or(usize::MAX);
        let room = room.min(buffer.len());
        let read = self
            .inner
            .read(buffer.get_mut(..room).unwrap_or_default())?;
        let Some(left) = left.checked_sub(u64::try_from(read).unwrap_or(u64::MAX)) else {
            self.budget.exceeded.set(true);
            return Err(io::Error::other(
                "the archive decompresses past its ceiling",
            ));
        };
        self.budget.left.set(left);
        Ok(read)
    }
}

/// What the entries before a tar member say of it: a GNU long name, a GNU
/// long link target and an extended header, each held only once its ceiling
/// allows it.
#[derive(Default)]
struct Ahead {
    name: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
    extended: Option<Vec<u8>>,
}

impl Ahead {
    /// Whether nothing is waiting for a member to describe.
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.link.is_none() && self.extended.is_none()
    }

    /// Takes `entry`, of `kind`, when it describes the member after it, or
    /// says something of every member after it, which this reader does not
    /// take from it; whether it was such an entry.
    fn took(
        &mut self,
        kind: EntryType,
        entry: &mut tar::Entry<'_, impl Read>,
        budget: &Budget,
    ) -> Result<bool, UnpackError> {
        let (held, ceiling) = match kind {
            EntryType::GNULongName => (&mut self.name, NAME_BYTES as u64 + 1),
            EntryType::GNULongLink => (&mut self.link, NAME_BYTES as u64 + 1),
            EntryType::XHeader => (&mut self.extended, PAX_BYTES),
            EntryType::XGlobalHeader => return Ok(true),
            _ => return Ok(false),
        };
        if entry.size() > ceiling {
            if kind == EntryType::XHeader {
                return Err(UnpackError::Extended { ceiling });
            }
            // How the name begins, which is all of it that is read.
            let mut begins = Vec::new();
            (&mut *entry)
                .take(NAMED as u64)
                .read_to_end(&mut begins)
                .map_err(|error| budget.failed(error))?;
            return Err(UnpackError::Overlong {
                name: shown(&begins),
                ceiling: NAME_BYTES,
            });
        }
        if held.is_some() {
            return Err(UnpackError::Corrupt(io::Error::other(
                "the archive describes one member twice",
            )));
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| budget.failed(error))?;
        *held = Some(data);
        Ok(true)
    }

    /// The raw name and link target of `entry`, as what came before it says
    /// they are, which it then no longer says.
    fn named(
        &mut self,
        entry: &tar::Entry<'_, impl Read>,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>), UnpackError> {
        let mut name = entry.header().path_bytes().into_owned();
        let mut target = entry
            .header()
            .link_name_bytes()
            .map(std::borrow::Cow::into_owned);
        if let Some(extended) = self.extended.take() {
            for record in tar::PaxExtensions::new(&extended) {
                let record = record.map_err(UnpackError::Corrupt)?;
                let value = record.value_bytes();
                match record.key_bytes() {
                    b"path" => {
                        sized(value)?;
                        name = value.to_vec();
                    }
                    b"linkpath" => {
                        sized(value)?;
                        target = Some(value.to_vec());
                    }
                    b"size" if parsed(value) != Some(entry.size()) => {
                        return Err(UnpackError::Corrupt(io::Error::other(
                            "an extended header gives a member another size than its header",
                        )));
                    }
                    _ => {}
                }
            }
        }
        if let Some(long) = self.name.take() {
            name = terminated(long);
        }
        if let Some(long) = self.link.take() {
            target = Some(terminated(long));
        }
        Ok((name, target))
    }
}

/// `long` without the one NUL a GNU long name may end in.
fn terminated(mut long: Vec<u8>) -> Vec<u8> {
    if long.last() == Some(&0) {
        long.pop();
    }
    long
}

/// The whole number `value` spells in decimal, where it spells one.
fn parsed(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

/// One artifact being unpacked: the rules it is held to and what it has
/// written so far.
struct Walk<'a> {
    artifact: &'a Artifact,
    staging: &'a Path,
    /// The list it is held to, by path; none for a survey.
    list: Option<HashMap<&'a str, &'a FileKind>>,
    /// The members kept, when not all of them are.
    kept: HashSet<&'a str>,
    /// Every name written and every directory a member under it implies.
    names: Names,
    /// The most members it writes.
    members: usize,
    /// What was written, in the archive's order.
    files: Vec<File>,
    /// The ordinary files written, by path, with their size and SHA-256.
    regular: HashMap<String, (u64, String)>,
    /// The links to make once every member is known, with their targets.
    links: Vec<(String, String)>,
    /// The bytes written.
    unpacked: u64,
}

impl<'a> Walk<'a> {
    fn new(
        artifact: &'a Artifact,
        staging: &'a Path,
        list: Option<HashMap<&'a str, &'a FileKind>>,
    ) -> Self {
        let members = list.as_ref().map_or(MEMBERS, HashMap::len);
        Self {
            artifact,
            staging,
            list,
            kept: artifact.kept.iter().map(|kept| &**kept).collect(),
            members,
            names: Names::new(MEMBERS),
            files: Vec::new(),
            regular: HashMap::new(),
            links: Vec::new(),
            unpacked: 0,
        }
    }

    /// Unpacks `bytes` by the artifact's format, then makes its links and
    /// checks that nothing it names is missing.
    fn run<R: io::BufRead + io::Seek>(mut self, reader: R) -> Result<Staged, UnpackError> {
        let budget = Budget::new(self.artifact.decompressed);
        match self.artifact.format {
            Format::Zip => self.zipped(reader, &budget)?,
            Format::TarGz => self.tarred(reader, &budget)?,
            Format::Bare => self.bare(reader, &budget)?,
            Format::Wheel => return Err(UnpackError::Wheel),
        }
        self.linked()?;
        self.complete()?;
        Ok(Staged {
            directory: self.staging.to_owned(),
            files: self.files,
            unpacked: self.unpacked,
            decompressed: budget.spent(),
        })
    }

    /// Walks a zip, reading every member through `budget`.
    fn zipped<R: Read + io::Seek>(
        &mut self,
        reader: R,
        budget: &Budget,
    ) -> Result<(), UnpackError> {
        let mut zip = ZipArchive::new(reader)?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            sized(entry.name_raw())?;
            let raw = entry.name_raw().to_vec();
            let size = entry.size();
            let mode = entry.unix_mode();
            let directory = raw.ends_with(b"/");
            let mut data = Bounded {
                inner: &mut entry,
                budget,
            };
            let body = match (mode.map(|mode| mode & TYPE), directory) {
                (Some(SYMLINK), _) => Body::Refused("a link, which a zip does not make"),
                (None | Some(0 | DIRECTORY), true) => Body::Dir,
                (None | Some(0 | REGULAR), false) => Body::File(Incoming {
                    data: &mut data,
                    size,
                    exec: mode.is_some_and(|mode| mode & 0o111 != 0),
                }),
                (Some(DIRECTORY | REGULAR), _) => {
                    Body::Refused("named as one kind and moded as another")
                }
                _ => Body::Refused("a device, a pipe or a socket"),
            };
            self.member(&raw, body, budget)?;
            // What a member holds past what was written, a discarded one's
            // whole, is read through, so it counts against the ceiling and
            // its checksum is checked.
            io::copy(&mut data, &mut io::sink()).map_err(|error| budget.failed(error))?;
        }
        Ok(())
    }

    /// Walks a gzip-compressed tar, decompressed through `budget`, and then
    /// the rest of its one gzip stream, refusing anything after it.
    ///
    /// Each header is read as it is written. A GNU long name or long link
    /// target, and an extended header, is held to its own ceiling before a
    /// byte of it is read, and then names the member after it as `tar` itself
    /// takes it: a long name before an extended header's `path`, and that
    /// before the header's own name. An extended header that gives a member
    /// another size than its header is refused, since the member would then
    /// be read as another.
    fn tarred<R: io::BufRead>(&mut self, reader: R, budget: &Budget) -> Result<(), UnpackError> {
        let mut archive = tar::Archive::new(Bounded {
            inner: GzDecoder::new(reader),
            budget,
        });
        let mut ahead = Ahead::default();
        let entries = archive
            .entries()
            .map_err(|error| budget.failed(error))?
            .raw(true);
        for entry in entries {
            let mut entry = entry.map_err(|error| budget.failed(error))?;
            let kind = entry.header().entry_type();
            if ahead.took(kind, &mut entry, budget)? {
                continue;
            }
            let (raw, target) = ahead.named(&entry)?;
            // What follows a sparse member's header is read by `tar` as more
            // of its map, so it is refused whether it is kept or not, before
            // anything after it is read as a member.
            if kind == EntryType::GNUSparse {
                return Err(UnpackError::Kind {
                    name: shown(&raw),
                    kind: "a sparse file",
                });
            }
            let size = entry.size();
            let exec = entry.header().mode().map_err(UnpackError::Corrupt)? & 0o111 != 0;
            let target = target.unwrap_or_default();
            let body = match kind {
                EntryType::Regular => Body::File(Incoming {
                    data: &mut entry,
                    size,
                    exec,
                }),
                EntryType::Directory => Body::Dir,
                EntryType::Symlink => Body::Symlink(target),
                EntryType::Link => Body::Hard { target, exec },
                // Says something of every member after it, which this reader
                // does not take from it.
                EntryType::XGlobalHeader => continue,
                EntryType::Char | EntryType::Block => Body::Refused("a device"),
                EntryType::Fifo => Body::Refused("a pipe"),
                _ => Body::Refused("of a kind no unpacker writes"),
            };
            self.member(&raw, body, budget)?;
        }
        if !ahead.is_empty() {
            return Err(UnpackError::Corrupt(io::Error::other(
                "the archive describes a member it does not hold",
            )));
        }
        let mut contents = archive.into_inner();
        io::copy(&mut contents, &mut io::sink()).map_err(|error| budget.failed(error))?;
        let mut rest = contents.inner.into_inner();
        if io::BufRead::fill_buf(&mut rest)
            .map_err(UnpackError::Corrupt)?
            .is_empty()
        {
            Ok(())
        } else {
            Err(UnpackError::Corrupt(io::Error::other(
                "the archive goes on past its gzip stream",
            )))
        }
    }

    /// Takes a program published bare as one member, named as its list names
    /// it, or for a survey as the last part of the address it is published
    /// at.
    fn bare<R: Read>(&mut self, reader: R, budget: &Budget) -> Result<(), UnpackError> {
        let name = if let Some(list) = &self.list {
            let mut names = list.keys();
            match (names.next(), names.next()) {
                (Some(name), None) => (*name).to_owned(),
                _ => return Err(UnpackError::Bare),
            }
        } else {
            let address = &*self.artifact.url;
            let path = address.split(['?', '#']).next().unwrap_or(address);
            path.rsplit('/').next().unwrap_or(path).to_owned()
        };
        let mut data = Bounded {
            inner: reader,
            budget,
        };
        // What it holds was held to the pinned size before it was read here.
        let size = self
            .artifact
            .replaced_by
            .as_ref()
            .map_or(self.artifact.size, |asset| asset.size);
        let body = Body::File(Incoming {
            data: &mut data,
            size,
            exec: true,
        });
        self.member(name.as_bytes(), body, budget)
    }

    /// Takes one member the archive names `raw`: refused, discarded or
    /// written.
    fn member(&mut self, raw: &[u8], body: Body<'_>, budget: &Budget) -> Result<(), UnpackError> {
        // The directory the archive was made from is the staging directory.
        if matches!(body, Body::Dir) && matches!(raw, b"." | b"./") {
            return Ok(());
        }
        let name = named(raw, matches!(body, Body::Dir))?;
        if !self.kept.is_empty() && !self.kept.contains(name.as_str()) {
            return Ok(());
        }
        if let Body::Refused(kind) = body {
            return Err(UnpackError::Kind {
                name: shown(raw),
                kind,
            });
        }
        let listed = match &self.list {
            Some(list) => Some(
                *list
                    .get(name.as_str())
                    .ok_or_else(|| UnpackError::Unlisted { name: shown(raw) })?,
            ),
            None => None,
        };
        if self.files.len() >= self.members {
            return Err(UnpackError::Members {
                ceiling: self.members,
            });
        }
        let differs = |what| UnpackError::Differs {
            name: shown(raw),
            what,
        };
        match body {
            Body::File(incoming) => {
                // Its data comes out of the archive, so none of it is written
                // where all of it could not be read.
                if incoming.size > budget.left.get() {
                    return Err(UnpackError::Decompressed {
                        ceiling: budget.ceiling,
                    });
                }
                if let Some(listed) = listed {
                    let FileKind::File { size, exec, .. } = listed else {
                        return Err(differs("a kind"));
                    };
                    if *size != incoming.size {
                        return Err(differs("a size"));
                    }
                    if *exec != incoming.exec {
                        return Err(differs("an exec flag"));
                    }
                }
                self.names.claim(&name, Held::Other)?;
                let written = self.write(&name, incoming, &|error| budget.failed(error))?;
                self.wrote(name, written, listed)
            }
            Body::Dir => {
                if listed.is_some_and(|listed| *listed != FileKind::Dir) {
                    return Err(differs("a kind"));
                }
                self.names.claim(&name, Held::Directory)?;
                self.directory(&name)?;
                self.files.push(File {
                    path: name.into(),
                    kind: FileKind::Dir,
                });
                Ok(())
            }
            Body::Symlink(target) => {
                let target = linked(&name, &target)?;
                if listed.is_some_and(|listed| {
                    !matches!(listed, FileKind::Link { target: given } if **given == *target)
                }) {
                    return Err(differs("a kind or a target"));
                }
                self.names.claim(&name, Held::Other)?;
                self.files.push(File {
                    path: name.as_str().into(),
                    kind: FileKind::Link {
                        target: target.as_str().into(),
                    },
                });
                self.links.push((name, target));
                Ok(())
            }
            Body::Hard { target, exec } => {
                let target = named(&target, false)?;
                let Some((size, _)) = self.regular.get(&target).cloned() else {
                    return Err(UnpackError::Link {
                        name: shown(raw),
                        why: "is a hard link to no file written before it",
                    });
                };
                if let Some(listed) = listed
                    && !matches!(listed, FileKind::File { size: given, exec: runs, .. }
                        if *given == size && *runs == exec)
                {
                    return Err(differs("a kind, a size or an exec flag"));
                }
                self.names.claim(&name, Held::Other)?;
                let from = self.path(&target);
                let unread = |source| UnpackError::Write {
                    path: from.clone(),
                    source,
                };
                let mut data = fs::File::open(&from).map_err(unread)?;
                let incoming = Incoming {
                    data: &mut data,
                    size,
                    exec,
                };
                let written = self.write(&name, incoming, &unread)?;
                self.wrote(name, written, listed)
            }
            Body::Refused(_) => Ok(()),
        }
    }

    /// Records the file `written` at `name`, once its SHA-256 is the one
    /// `listed` gives, where there is a list.
    fn wrote(
        &mut self,
        name: String,
        written: FileKind,
        listed: Option<&FileKind>,
    ) -> Result<(), UnpackError> {
        let FileKind::File { size, sha256, .. } = &written else {
            return Ok(());
        };
        if let Some(FileKind::File { sha256: given, .. }) = listed
            && given != sha256
        {
            return Err(UnpackError::Differs {
                name: shown(name.as_bytes()),
                what: "bytes",
            });
        }
        self.regular
            .insert(name.clone(), (*size, sha256.to_string()));
        self.files.push(File {
            path: name.into(),
            kind: written,
        });
        Ok(())
    }

    /// Where `name` is under the staging directory.
    fn path(&self, name: &str) -> PathBuf {
        name.split('/')
            .fold(self.staging.to_path_buf(), |path, part| path.join(part))
    }

    /// Makes the directory `name` and every one above it under the staging
    /// directory, where each is not there already as a directory, never
    /// through a link.
    fn directory(&self, name: &str) -> Result<PathBuf, UnpackError> {
        let mut path = self.staging.to_path_buf();
        for part in name.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(UnpackError::Write {
                        path,
                        source: io::ErrorKind::AlreadyExists.into(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    owner_only(&path).map_err(|source| UnpackError::Write {
                        path: path.clone(),
                        source,
                    })?;
                }
                Err(source) => return Err(UnpackError::Write { path, source }),
            }
        }
        Ok(path)
    }

    /// Makes the directory above `name`, as [`Walk::directory`] makes one.
    fn parent(&self, name: &str) -> Result<PathBuf, UnpackError> {
        self.directory(name.rsplit_once('/').map_or("", |(above, _)| above))
    }

    /// Writes `incoming` to a new file at `name` and gives it as the list
    /// would; `unreadable` says why its data could not be read.
    fn write(
        &mut self,
        name: &str,
        incoming: Incoming<'_>,
        unreadable: &dyn Fn(io::Error) -> UnpackError,
    ) -> Result<FileKind, UnpackError> {
        let Incoming { data, size, exec } = incoming;
        let total = self
            .unpacked
            .checked_add(size)
            .filter(|total| *total <= self.artifact.unpacked)
            .ok_or(UnpackError::Unpacked {
                ceiling: self.artifact.unpacked,
            })?;
        let path = self
            .parent(name)?
            .join(name.rsplit('/').next().unwrap_or(name));
        let written = |source| UnpackError::Write {
            path: path.clone(),
            source,
        };
        let mut out = created(&path, exec).map_err(written)?;
        let mut digest = Sha256::new();
        let mut chunk = vec![0_u8; 1 << 16];
        let mut data = data.take(size.saturating_add(1));
        let mut copied: u64 = 0;
        loop {
            let read = match data.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(unreadable(error)),
            };
            copied = copied.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if copied > size {
                return Err(UnpackError::Corrupt(io::Error::other(
                    "a member holds more than its size",
                )));
            }
            let bytes = chunk.get(..read).unwrap_or_default();
            digest.update(bytes);
            out.write_all(bytes).map_err(written)?;
        }
        if copied != size {
            return Err(UnpackError::Corrupt(io::ErrorKind::UnexpectedEof.into()));
        }
        out.sync_all().map_err(written)?;
        self.unpacked = total;
        Ok(FileKind::File {
            size,
            sha256: hex(&digest.finalize()).into(),
            exec,
        })
    }

    /// Makes every link the archive gave, once each resolves.
    fn linked(&self) -> Result<(), UnpackError> {
        let links: HashMap<&str, &str> = self
            .links
            .iter()
            .map(|(name, target)| (name.as_str(), target.as_str()))
            .collect();
        for (name, target) in &self.links {
            self.resolves(name, target, &links)?;
        }
        for (name, target) in &self.links {
            let path = self
                .parent(name)?
                .join(name.rsplit('/').next().unwrap_or(name));
            symlink(target, &path).map_err(|source| UnpackError::Write { path, source })?;
        }
        Ok(())
    }

    /// Whether the link `name` to `target` resolves to a file written here,
    /// inside the staging directory, through directories alone and at most
    /// [`LINK_STEPS`] links.
    fn resolves(
        &self,
        name: &str,
        target: &str,
        links: &HashMap<&str, &str>,
    ) -> Result<(), UnpackError> {
        link_resolves(
            name,
            target,
            links,
            &|path| self.regular.contains_key(path),
            &|path| self.names.is_directory(path),
        )
        .map_err(|why| UnpackError::Link {
            name: shown(name.as_bytes()),
            why,
        })
    }

    /// Refuses a member the list names, or the artifact keeps, that was not
    /// written.
    fn complete(&self) -> Result<(), UnpackError> {
        let written: HashSet<&str> = self.files.iter().map(|file| &*file.path).collect();
        let listed = self.list.iter().flat_map(HashMap::keys).copied();
        match listed
            .chain(self.kept.iter().copied())
            .find(|name| !written.contains(name))
        {
            Some(missing) => Err(UnpackError::Missing {
                name: shown(missing.as_bytes()),
            }),
            None => Ok(()),
        }
    }
}

/// Whether the link `name` to `target`, among `links`, resolves to what
/// `is_file` takes for a file, inside the directory the names are relative
/// to, through what `is_directory` takes for directories alone and at most
/// [`LINK_STEPS`] links; and why not, where it does not. The one rule both a
/// link unpacked and a link a version's file list names are held to.
pub(crate) fn link_resolves(
    name: &str,
    target: &str,
    links: &HashMap<&str, &str>,
    is_file: &dyn Fn(&str) -> bool,
    is_directory: &dyn Fn(&str) -> bool,
) -> Result<(), &'static str> {
    let mut from = name.to_owned();
    let mut target = target.to_owned();
    for _ in 0..LINK_STEPS {
        let mut at: Vec<&str> = from.split('/').collect();
        at.pop();
        let parts: Vec<&str> = target.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            match *part {
                "." => {}
                ".." => {
                    if at.pop().is_none() {
                        return Err("climbs out of the version");
                    }
                }
                part => at.push(part),
            }
            if index + 1 < parts.len() && !is_directory(&at.join("/")) {
                return Err("passes through what is not a directory written here");
            }
        }
        let resolved = at.join("/");
        if is_file(&resolved) {
            return Ok(());
        }
        let Some(next) = links.get(resolved.as_str()) else {
            return Err("does not resolve to a file written here");
        };
        target.clear();
        target.push_str(next);
        from = resolved;
    }
    Err("goes through more than eight links")
}

/// A new file at `path`, made on Unix with the mode it keeps, which anyone may
/// run when `exec` says and otherwise only read, less what the process's
/// umask takes away.
fn created(path: &Path, exec: bool) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, if exec { 0o755 } else { 0o644 });
    #[cfg(not(unix))]
    let _ = exec;
    options.open(path)
}

/// A new directory at `path` only its owner may reach; whatever is made of the
/// staged tree once it is committed widens that.
fn owner_only(path: &Path) -> io::Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        let mut builder = builder;
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
    };
    builder.create(path)
}

/// A symbolic link at `path` to `target`, which resolves to a file.
fn symlink(target: &str, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, path)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target.replace('/', "\\"), path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, path);
        Err(io::ErrorKind::Unsupported.into())
    }
}
