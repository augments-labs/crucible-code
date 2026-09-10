//! What may be attached to a request, what says a file is one, and how its
//! bytes are taken.
//!
//! A closed table rather than a guess from an extension, and the bytes are read
//! back against it: the cost of being wrong is a request the user paid for and
//! a provider refused. It lives here because two callers ask the same question
//! about the same files — the prompt, where a person names one, and the `read`
//! tool, where a model does — and one of them getting a different answer would
//! mean a file that can be typed and cannot be read, or the reverse.
//!
//! What this does *not* decide is whether the request being built can carry the
//! kind it names. That is the model's half and the provider's, settled per
//! request, and it is not a property of the file.
//!
//! There is one way in, in two halves. [`Opened`] is the descriptor — reached
//! through the workspace where a proof was held, opened by name where there was
//! none to walk down — and [`Taken`] is what a bounded read of it came to,
//! bytes and the digest that identifies them together. The halves stay
//! apart because a caller may have cheap reasons to refuse a file after it is
//! standing at the descriptor and before its bytes are worth reading; they are
//! one pipeline because a second spelling of read-then-digest is a second answer
//! to "is this the file that was attached":
//!
//! ```
//! use crucible_attachments::{Opened, kind};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let at = std::env::temp_dir().join(format!("crucible-doc-{}.png", std::process::id()));
//! std::fs::write(&at, [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])?;
//!
//! let named = kind("shot.png").expect("png is attachable");
//! let taken = Opened::named(&at)?.taken()?;
//!
//! assert!(taken.is(named));
//! assert_eq!(taken.bytes().len(), 8);
//! # std::fs::remove_file(&at)?;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use crucible_types::Modality;
// Public because this crate's own surface is written in them: `Opened::reached`
// takes a `WorkspacePath` and `AttachmentError::Unreached` carries a
// `PathError`, so a caller that depends on this crate alone can name both.
pub use crucible_workspace::{PathError, WorkspacePath};
use sha2::{Digest as _, Sha256};

/// The most raw attachment bytes one request may carry.
///
/// Not a vendor's limit — this one binds first. What a request peaks at is
/// measured rather than derived: `scripts/sh/bench.sh mem` runs a session at this
/// ceiling every time it runs, and reads about three times this figure on top
/// of what the session was already holding. The bytes, their base64 form and
/// the serialized body are alive at once, and the last two each hold the
/// encoding whole.
///
/// The figure this replaced was half as much again, from a reading of the code
/// that counted one of those copies and not the other. At that size the same
/// measurement swung between three times and four from run to run, which is the
/// other half of why this one is lower — a reading that moves by seven
/// megabytes needs somewhere to move to.
///
/// The worst a session is otherwise holding is its record full, at about 14 MB.
/// This is deliberately below what the rest of the 35 MB in
/// `performance-budgets.md` allows. A budget spent to its last megabyte is not
/// a budget.
///
/// A single file larger than this can never be carried whatever else a request
/// holds, which is what lets a caller refuse one before it has read the bytes.
/// [`Opened::taken`] is where that refusal is made, from the descriptor.
pub const CEILING: usize = 4 * 1024 * 1024;

/// Why a named file did not become attachable bytes.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    /// Larger than [`CEILING`], settled from the descriptor rather than from
    /// bytes already in hand.
    #[error("larger than the {} MB one attachment may be", CEILING / (1024 * 1024))]
    TooLarge,
    /// Opened, and what opened is a directory, a device or a pipe.
    #[error("not a regular file")]
    NotFile,
    /// The open or the read did not finish.
    #[error("could not be read: {0}")]
    Unread(#[from] io::Error),
    /// The descriptor walk refused the path.
    ///
    /// Only [`Opened::reached`] produces this, and only by naming it: a path
    /// the workspace proved can still be swapped under the walk, and what the
    /// walk refuses is a path question rather than an attachment one. There is
    /// no `From` conversion, so a later `?` on a `PathError` cannot quietly
    /// route [`PathError::NotFile`] back here after `reached` has sorted it
    /// into [`AttachmentError::NotFile`].
    ///
    /// `#[source]` is not `#[from]`: what it keeps is the refusal underneath
    /// reachable, which is the rest of this enum's habit, without minting the
    /// conversion the paragraph above is about.
    #[error("{0}")]
    Unreached(#[source] PathError),
}

/// Opens a file whose path did not come from the workspace, for its bytes.
///
/// What it answers is the pair that a name cannot: a pipe or a device standing
/// where a file stood is refused on the opened descriptor rather than trusted
/// by name. On Unix the open itself is non-blocking, so reaching that refusal
/// does not wait on a writer who is not coming.
///
/// # Errors
///
/// [`AttachmentError::NotFile`] where what opened is not a regular file, and
/// [`AttachmentError::Unread`] where the open itself failed.
fn opened(path: &Path) -> Result<File, AttachmentError> {
    let mut options = File::options();
    options.read(true);

    // Opening a pipe for reading blocks until somebody writes, so on Unix the
    // descriptor is asked for without waiting; the check below is what then
    // refuses it. Other platforms reach the same refusal, having waited.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed());
    }

    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(AttachmentError::NotFile);
    }
    Ok(file)
}

/// The bytes of an opened file, where there are few enough of them to carry.
///
/// The size is asked of the descriptor, so a file over [`CEILING`] is refused
/// before a byte of it is allocated — that is the sentence in `CEILING`'s
/// documentation, made true rather than assumed. The read still stops one past
/// the ceiling and checks the length again, because the file may grow between
/// the two questions and a descriptor already open would follow it.
///
/// # Errors
///
/// [`AttachmentError::TooLarge`] where the file is over the ceiling at either
/// question, and [`AttachmentError::Unread`] where the read did not finish.
fn carried(file: &mut File) -> Result<Vec<u8>, AttachmentError> {
    let size = file.metadata()?.len();
    if size > CEILING as u64 {
        return Err(AttachmentError::TooLarge);
    }

    // The size is at most the ceiling by the check above, so this asks for
    // exactly what the file holds and no more.
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(CEILING));
    file.by_ref()
        .take(CEILING as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > CEILING {
        return Err(AttachmentError::TooLarge);
    }
    Ok(bytes)
}

/// A descriptor standing at a file that is going to be attached.
///
/// Which constructor made it is the whole of the authority question. A path a
/// model reached is opened by walking the tree the workspace proved, one
/// component at a time against descriptors already held; a path with no proof
/// behind it has no containment to answer and is opened by name. Mixing the two
/// would hand one ingress the other's authority, which is why there is no way
/// to build this from a `File` a caller opened itself. The error code is
/// what that fails with today and not a gate, since `compile_fail` accepts any
/// compile error:
///
/// ```compile_fail,E0423
/// use crucible_attachments::Opened;
///
/// fn anywhere(file: std::fs::File) -> Opened {
///     Opened(file)
/// }
/// ```
pub struct Opened(File);

/// The descriptor stays; the file it stands at goes.
///
/// A `File`'s own `Debug` prints the path the operating system resolved it to,
/// which is the user content [`Attachment`](crucible_types::Attachment) hand-
/// writes its own `Debug` to keep out of a panic payload. That there is a
/// descriptor at all is the fact worth rendering.
impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened").finish_non_exhaustive()
    }
}

impl Opened {
    /// Opens a path the workspace proved, through the descriptor walk.
    ///
    /// # Errors
    ///
    /// [`AttachmentError::NotFile`] where what the walk reached is not a
    /// regular file, and [`AttachmentError::Unreached`] where the walk refused
    /// the path — most often because something replaced a component of it
    /// since it was proved.
    pub fn reached(path: &WorkspacePath) -> Result<Self, AttachmentError> {
        match path.open_regular() {
            Ok(file) => Ok(Self(file)),
            // One condition, one variant. The walk refuses a pipe or a device
            // for the same reason a named open does, and a caller deciding
            // what to tell the user must not have to know which ingress asked
            // in order to recognise the same answer.
            Err(PathError::NotFile { .. }) => Err(AttachmentError::NotFile),
            Err(refused) => Err(AttachmentError::Unreached(refused)),
        }
    }

    /// Opens a path by name, where there is no proof to walk down.
    ///
    /// # Errors
    ///
    /// [`AttachmentError::NotFile`] where what opened is not a regular file,
    /// and [`AttachmentError::Unread`] where the open itself failed.
    pub fn named(path: &Path) -> Result<Self, AttachmentError> {
        Ok(Self(opened(path)?))
    }

    /// The bytes, bounded by [`CEILING`], and the digest that identifies them.
    ///
    /// # Errors
    ///
    /// [`AttachmentError::TooLarge`] where the file is over the ceiling at
    /// either question, and [`AttachmentError::Unread`] where the read did not
    /// finish.
    pub fn taken(mut self) -> Result<Taken, AttachmentError> {
        let bytes = carried(&mut self.0)?;
        let hash = <[u8; 32]>::from(Sha256::digest(&bytes));
        Ok(Taken { bytes, hash })
    }
}

/// One file's bytes, and the digest they are identified by.
///
/// The two travel together because every caller that holds the bytes also has
/// to answer whether they are still the file that was attached, and a digest
/// taken somewhere else is a second answer to that question. Nothing here can
/// be built from bytes a caller already had: the digest is over what
/// [`Opened::taken`] read, and the ceiling had already been applied to it.
/// That is what lets a caller compare this digest against a recorded one and
/// conclude something about the file, so it is checked rather than asserted —
/// the error code is what this fails with today and not a gate, since
/// `compile_fail` accepts any compile error:
///
/// ```compile_fail,E0451
/// use crucible_attachments::Taken;
///
/// fn anywhere(bytes: Vec<u8>) -> Taken {
///     Taken { bytes, hash: [0; 32] }
/// }
/// ```
pub struct Taken {
    bytes: Vec<u8>,
    hash: [u8; 32],
}

/// The file goes; how much of it there was stays.
///
/// This holds a whole file somebody attached, and the digest that identifies
/// it. [`Attachment`](crucible_types::Attachment) redacts that same digest from
/// its own `Debug` for a reason this type does not escape by holding the bytes
/// as well: a hash is not reversible, but it confirms which file somebody was
/// holding to anyone who already has a copy. How many bytes arrived is a fact
/// about the shape of the turn and names nobody.
impl fmt::Debug for Taken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Taken")
            .field("read", &self.bytes.len())
            .field("hash", &"[redacted]")
            .finish()
    }
}

impl Taken {
    /// What was read.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The digest of what was read.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }

    /// Whether the bytes are what a kind says they should be.
    ///
    /// The name said what somebody meant; this says whether the file agrees.
    #[must_use]
    pub fn is(&self, kind: &Kind) -> bool {
        (kind.confirms)(&self.bytes)
    }

    /// The bytes alone, for a caller handing them on.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// The kind a path's extension names, where it names one this build attaches.
///
/// The name is being asked what somebody meant by it. Whether the bytes agree
/// is [`Kind::confirms`], asked separately and after, because the two failures
/// are different sentences.
#[must_use]
pub fn kind(word: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.names(word))
}

/// One kind of file that may be attached, under the name it goes by.
#[derive(Debug)]
pub struct Kind {
    /// The extension, without its dot, as a prompt would spell it.
    pub extension: &'static str,
    /// What the model would be asked to do with it.
    pub modality: Modality,
    /// What the provider labels the bytes with.
    pub media_type: &'static str,
    /// Whether the bytes are what the extension claims.
    pub confirms: fn(&[u8]) -> bool,
}

impl Kind {
    /// Whether a word in a prompt is a path spelled with this extension.
    ///
    /// Case-insensitive on the extension alone: a camera writes `IMG_0001.JPG`
    /// and a person types what the camera wrote.
    #[must_use]
    pub fn names(&self, word: &str) -> bool {
        word.rsplit_once('.')
            .is_some_and(|(_, tail)| tail.eq_ignore_ascii_case(self.extension))
    }

    /// The kind as it appears mid-sentence, with the article English wants.
    #[must_use]
    pub fn spoken(&self) -> String {
        let article = match self.modality {
            Modality::Image | Modality::Audio => "an",
            Modality::Text | Modality::Pdf | Modality::Video => "a",
        };

        format!("{article} {}", self.modality.as_str())
    }
}

/// Every kind crucible will attach: the picture formats all three vendors
/// document accepting, the document format any of them reads, and the video
/// container Moonshot documents carrying as a base64 data URL.
///
/// A closed list rather than a guess from the extension, because the cost of
/// being wrong is a refused request the user paid for. Anything not here is
/// text, and the `read` tool already opens it.
pub const KINDS: &[Kind] = &[
    Kind {
        extension: "png",
        modality: Modality::Image,
        media_type: "image/png",
        confirms: png,
    },
    Kind {
        extension: "jpg",
        modality: Modality::Image,
        media_type: "image/jpeg",
        confirms: jpeg,
    },
    Kind {
        extension: "jpeg",
        modality: Modality::Image,
        media_type: "image/jpeg",
        confirms: jpeg,
    },
    Kind {
        extension: "gif",
        modality: Modality::Image,
        media_type: "image/gif",
        confirms: gif,
    },
    Kind {
        extension: "webp",
        modality: Modality::Image,
        media_type: "image/webp",
        confirms: webp,
    },
    Kind {
        extension: "pdf",
        modality: Modality::Pdf,
        media_type: "application/pdf",
        confirms: pdf,
    },
    Kind {
        extension: "mp4",
        modality: Modality::Video,
        media_type: "video/mp4",
        confirms: mp4,
    },
];

/// The eight bytes a PNG starts with, of which the last four catch a file a
/// transfer has rewritten the line endings of.
fn png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// Every JPEG starts with a start-of-image marker and the next marker's
/// introducer. What follows differs by encoder, so three bytes is the whole of
/// what is common to all of them.
fn jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

/// The two GIF versions, both still written by something.
fn gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

/// A WebP is a RIFF container, and the four bytes saying which kind sit after
/// the length rather than beside the tag.
fn webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..])
}

/// The header a PDF opens with, version and all.
fn pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// An MP4-family file whose first box declares a bounded `ftyp` payload.
///
/// A four-byte major brand and minor version are mandatory; compatible brands
/// are extensible but must each be complete. An `ftyp` box consuming the whole
/// file is not a usable video because it leaves no room for media boxes.
fn mp4(bytes: &[u8]) -> bool {
    if bytes.get(4..8) != Some(&b"ftyp"[..]) {
        return false;
    }

    let Some(size) = bytes
        .get(..4)
        .and_then(|size| <[u8; 4]>::try_from(size).ok())
        .map(u32::from_be_bytes)
    else {
        return false;
    };
    let (header, size) = match size {
        0 => return false,
        1 => {
            let Some(size) = bytes
                .get(8..16)
                .and_then(|size| <[u8; 8]>::try_from(size).ok())
                .map(u64::from_be_bytes)
                .and_then(|size| usize::try_from(size).ok())
            else {
                return false;
            };
            (16, size)
        }
        size => (8, size as usize),
    };

    size >= header + 8 && size <= bytes.len() && (size - header - 8).is_multiple_of(4)
}

#[cfg(test)]
mod tests {
    use std::io::Seek as _;

    #[cfg(unix)]
    use crucible_workspace::Workspace;

    use super::*;

    /// A directory this test owns, emptied first so a rerun starts clean.
    fn base(name: &str) -> std::path::PathBuf {
        let base =
            std::env::temp_dir().join(format!("crucible-attach-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("a writable temporary directory");
        base
    }

    #[test]
    fn a_file_over_the_ceiling_is_refused_from_the_descriptor() {
        // The size comes from the descriptor, so this holds without the test
        // ever writing four megabytes: the file is that large and no page of
        // it is touched, by the check or by the assertion.
        let base = base("over");
        let at = base.join("huge.png");
        let file = File::create(&at).expect("a writable temporary directory");
        file.set_len(CEILING as u64 + 1).expect("a sparse file");
        drop(file);

        let mut file = opened(&at).expect("a regular file opens");
        // Matched rather than unwrapped: an `expect_err` here formats the four
        // megabytes it was handed when this regresses, and a failure nobody can
        // read is a failure nobody acts on. The error side still says what it
        // got, which is the half that fits on a line.
        let refused = carried(&mut file);

        match refused {
            Err(AttachmentError::TooLarge) => {}
            Ok(bytes) => panic!("carried {} bytes", bytes.len()),
            Err(other) => panic!("{other}"),
        }
        // What the size check buys, made observable. `carried` never seeks, so
        // an untouched offset is proof the refusal came from asking the
        // descriptor rather than from reading the file and measuring after.
        assert_eq!(
            file.stream_position().expect("an open file has an offset"),
            0,
            "the refusal read the file instead of asking its size"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_descriptor_yielding_more_than_it_said_it_held_is_refused() {
        // The growth case, made deterministic. A file that grows between the
        // two questions is a race no test can schedule; a character device is
        // the same shape standing still — it reports no size and then yields
        // bytes without end, so it reaches the second check exactly as a grown
        // file does. Opened directly because `Opened::named` refuses a device,
        // which is the point: this is about the guard behind that refusal.
        let mut file = File::open("/dev/zero").expect("every unix has one");

        let refused = carried(&mut file);

        match refused {
            Err(AttachmentError::TooLarge) => {}
            Ok(bytes) => panic!("carried {} bytes", bytes.len()),
            Err(other) => panic!("{other}"),
        }
    }

    #[test]
    fn the_digest_travels_with_the_bytes_it_was_taken_over() {
        let base = base("digest");
        let at = base.join("shot.png");
        std::fs::write(&at, [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
            .expect("a writable temporary directory");

        let taken = Opened::named(&at)
            .expect("a regular file opens")
            .taken()
            .expect("under the ceiling");

        assert!(taken.is(kind("shot.png").expect("png is attachable")));
        // The published SHA-256 of those eight bytes, written down rather than
        // recomputed here: an oracle that runs the same algorithm over the same
        // input agrees with the code even when both are wrong.
        assert_eq!(
            taken.hash(),
            [
                0x4c, 0x4b, 0x6a, 0x3b, 0xe1, 0x31, 0x4a, 0xb8, 0x61, 0x38, 0xbe, 0xf4, 0x31, 0x4d,
                0xde, 0x02, 0x2e, 0x60, 0x09, 0x60, 0xd8, 0x68, 0x9a, 0x2c, 0x8f, 0x86, 0x31, 0x80,
                0x2d, 0x20, 0xda, 0xb6
            ]
        );
    }

    #[test]
    fn a_file_of_exactly_the_ceiling_is_carried_whole() {
        // The largest file the ceiling promises, which is the value both guards
        // are written against. Without this, either `>` may become `>=` and
        // nothing goes red: the file below is 64 bytes, and every other case is
        // far enough from the edge to survive the wrong comparison.
        let base = base("edge");
        let at = base.join("exact.png");
        let file = File::create(&at).expect("a writable temporary directory");
        file.set_len(CEILING as u64).expect("a sparse file");
        drop(file);

        let mut file = opened(&at).expect("a regular file opens");
        let bytes = carried(&mut file).expect("the ceiling is a file that fits");

        assert_eq!(bytes.len(), CEILING);
    }

    #[test]
    fn a_file_under_the_ceiling_is_carried_whole() {
        let base = base("under");
        let at = base.join("edge.png");
        std::fs::write(&at, vec![7; 64]).expect("a writable temporary directory");

        let mut file = opened(&at).expect("a regular file opens");
        let bytes = carried(&mut file).expect("under the ceiling");

        assert_eq!(bytes, vec![7; 64]);
    }

    #[test]
    fn a_directory_is_not_a_file_to_attach() {
        let base = base("directory");

        let refused = opened(&base).expect_err("a directory is not attachable");

        // Unix opens a directory and the kind check is what refuses it; Windows
        // refuses the open itself, without the flag that would let a directory
        // through. Either way no caller is handed a directory to read, which is
        // the sentence being held here.
        #[cfg(unix)]
        assert!(matches!(refused, AttachmentError::NotFile), "{refused}");
        #[cfg(not(unix))]
        assert!(matches!(refused, AttachmentError::Unread(_)), "{refused}");
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_refused_without_waiting_for_a_writer() {
        let base = base("pipe");
        let at = base.join("waiting.png");
        let made = std::process::Command::new("mkfifo")
            .arg(&at)
            .status()
            .expect("mkfifo runs");
        assert!(made.success());

        let refused = opened(&at).expect_err("a pipe is not attachable");

        assert!(matches!(refused, AttachmentError::NotFile), "{refused}");
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_the_workspace_proved_is_refused_as_not_a_file() {
        let base = base("reached-pipe");
        let at = base.join("waiting.png");
        let made = std::process::Command::new("mkfifo")
            .arg(&at)
            .status()
            .expect("mkfifo runs");
        assert!(made.success());
        let workspace = Workspace::open(&base).expect("a directory opens as a workspace");
        let proven = workspace
            .existing("waiting.png")
            .expect("it is there, and inside");

        let refused = Opened::reached(&proven).expect_err("a pipe is not attachable");

        // One condition, one variant. The walk refuses a pipe for the same
        // reason a named open does, and a caller deciding what to tell the
        // user must not have to know which ingress asked.
        assert!(matches!(refused, AttachmentError::NotFile), "{refused}");
    }

    #[test]
    fn neither_the_bytes_nor_the_digest_reach_a_debug_rendering() {
        // `Attachment` hand-writes its `Debug` to keep the path and the hash
        // out of a panic payload. These hold the file itself and the same
        // digest, so deriving one here would put back what that removed.
        let base = base("debug");
        let at = base.join("private.png");
        std::fs::write(&at, [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
            .expect("a writable temporary directory");

        let open = Opened::named(&at).expect("a regular file opens");
        // Pinned exactly, like the rendering below: asserting only that the
        // filename is absent would pass a future field that carries the path
        // under another name.
        assert_eq!(format!("{open:?}"), "Opened { .. }");

        let taken = open.taken().expect("under the ceiling");

        assert_eq!(
            format!("{taken:?}"),
            "Taken { read: 8, hash: \"[redacted]\" }"
        );
    }

    /// A minimal whole `ftyp` box with one compatible brand.
    fn video(major: [u8; 4], compatible: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::from(&20_u32.to_be_bytes()[..]);
        bytes.extend_from_slice(b"ftyp");
        bytes.extend_from_slice(&major);
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&compatible);
        bytes
    }

    /// Rewrites the ordinary box size in a fixture known to contain it.
    fn sized(bytes: &mut [u8], size: u32) {
        bytes
            .get_mut(..4)
            .expect("the fixture starts with a four-byte size")
            .copy_from_slice(&size.to_be_bytes());
    }

    #[test]
    fn mp4_names_are_case_insensitive_and_name_video() {
        let kind = kind("clips/demo.MP4").expect("mp4 is attachable");

        assert_eq!(kind.modality, Modality::Video);
        assert_eq!(kind.media_type, "video/mp4");
        assert!((kind.confirms)(&video(*b"isom", *b"mp42")));
    }

    #[test]
    fn moonshots_documented_quicktime_branded_mp4_is_accepted() {
        assert!(mp4(&video(*b"qt  ", *b"qt  ")));
    }

    #[test]
    fn mp4_brands_are_extensible() {
        assert!(mp4(&video(*b"vend", *b"more")));
    }

    #[test]
    fn an_mp4_name_does_not_make_unrelated_bytes_a_video() {
        assert!(!mp4(b"this is not an ISO base media file"));
    }

    #[test]
    fn a_truncated_or_malformed_ftyp_box_is_refused() {
        let mut truncated = video(*b"isom", *b"mp42");
        truncated.pop();
        assert!(!mp4(&truncated));

        let mut partial_brand = video(*b"isom", *b"mp42");
        sized(&mut partial_brand, 19);
        assert!(!mp4(&partial_brand));

        let mut too_small = video(*b"isom", *b"mp42");
        sized(&mut too_small, 12);
        assert!(!mp4(&too_small));

        let mut unbounded = video(*b"isom", *b"mp42");
        sized(&mut unbounded, 0);
        assert!(!mp4(&unbounded));
    }
}
