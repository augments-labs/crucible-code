//! Pictures of diagrams and display math, kept on disk so a session put back
//! shows them again without drawing them.

use std::collections::BinaryHeap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use crucible_raster::{Slot, decode};
use crucible_tui::drawing::Svg;
use crucible_tui::graphics::{Form, Payload, PictureKey, Protocol};
use sha2::{Digest as _, Sha256};

/// The most bytes of entries the cache holds once a write has landed.
pub(crate) const MOST_HELD: u64 = 32 << 20;

/// The longest an entry is kept after it was written.
pub(crate) const MOST_AGE: Duration = Duration::from_hours(14 * 24);

/// The most entries one write holds the names of while it chooses which to
/// take out: the oldest of them, so a directory of any number of names costs
/// a write no more than these, and what they free short of the bound the
/// next write goes on freeing.
const MOST_LISTED: usize = 4096;

/// How far ahead of now an entry may say it was written and still be kept: a
/// clock a little out, and not a date set so it never ages.
const SKEW: Duration = Duration::from_hours(24);

/// The most bytes of entries waiting for the writer, and so the largest one
/// entry may be.
pub(crate) const MOST_QUEUED: usize = 512 << 10;

/// The directory under crucible's home the entries are kept in.
const DIRECTORY: &str = "pictures";

/// What every entry opens with, so a file that is not one is told apart
/// before its digests are worked out.
const MAGIC: [u8; 8] = *b"CRPICT\0\x01";

/// The bytes before an entry's body: what it opens with, its key's digest and
/// its body's.
const HEADER: usize = MAGIC.len() + 32 + 32;

/// The bytes of a body before its picture: its protocol, its pixels' width
/// and height, and its form's length.
const BODY: usize = 1 + 4 + 4 + 4;

/// The bounds one cache is held to.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    held: u64,
    age: Duration,
    names: usize,
}

/// The pictures kept under one home.
#[derive(Debug, Clone)]
pub(crate) struct Cache {
    directory: PathBuf,
    bounds: Bounds,
    /// Whether the directory was made its owner's alone before it was first
    /// read, settled once for every copy of this cache.
    narrowed: Arc<OnceLock<bool>>,
}

impl Cache {
    /// The cache kept under `home`. Nothing is read or made until it is used.
    pub(crate) fn open(home: &Path) -> Self {
        Self {
            directory: home.join(DIRECTORY),
            bounds: Bounds {
                held: MOST_HELD,
                age: MOST_AGE,
                names: MOST_LISTED,
            },
            narrowed: Arc::new(OnceLock::new()),
        }
    }

    /// The form and payload kept for `source` under `key`, where an entry
    /// for them is here and reads back whole; anything else is a miss.
    pub(crate) fn get(&self, source: &str, key: &PictureKey) -> Option<(Form, Payload)> {
        let digest = keyed(source, key);
        let bytes = self.read(&named(&digest))?;
        let (magic, rest) = bytes.split_first_chunk::<8>()?;
        let (named_by, rest) = rest.split_first_chunk::<32>()?;
        let (sealed_as, body) = rest.split_first_chunk::<32>()?;
        // An entry under another key's name, or changed since it was sealed,
        // is not this picture.
        if *magic != MAGIC
            || *named_by != digest
            || *sealed_as != <[u8; 32]>::from(Sha256::digest(body))
        {
            return None;
        }
        let read = Read::of(body)?;
        if read.protocol != tag(key.protocol) || !shown(key.protocol, read.payload, read.pixels) {
            return None;
        }
        let svg = Svg::new(String::from_utf8(read.svg.to_vec()).ok()?).ok()?;
        Some((
            Form::Svg(svg),
            Payload::new(key.protocol, read.payload.to_vec(), read.pixels, key.cell),
        ))
    }

    /// Writes `entry`, making room for it first. A cache that cannot be
    /// written is left as it is, and the picture was drawn either way.
    pub(crate) fn put(&self, entry: &Entry) {
        self.make_room(entry);
        self.land(entry);
    }

    /// Removes drafts left half written and every entry past the bounds.
    pub(crate) fn trim(&self) {
        if !self.is_directory() {
            return;
        }
        if let Ok(listed) = fs::read_dir(&self.directory) {
            for found in listed.flatten() {
                if is_draft(&found.file_name()) {
                    let _ = fs::remove_file(found.path());
                }
            }
        }
        self.prune(None, 0);
    }

    /// Makes the directory, owner-only, and removes the entries past the
    /// bounds with room made for `entry`.
    fn make_room(&self, entry: &Entry) {
        if crucible_privacy::directory(&self.directory).is_ok() {
            self.prune(Some(&entry.name), entry.length());
        }
    }

    /// Puts `entry` in place: written to a draft and renamed over its name.
    fn land(&self, entry: &Entry) {
        let _ = crucible_privacy::replace_in(
            &self.directory,
            std::ffi::OsStr::new(&entry.name),
            &entry.bytes,
        );
    }

    /// The entry `name`, where the directory and the entry are this
    /// account's alone and the entry is no larger than one is written.
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        if !self.is_directory() {
            return None;
        }
        // A directory an earlier build or a hand left open is narrowed
        // before anything in it is read; one that cannot be is passed over.
        if !*self
            .narrowed
            .get_or_init(|| crucible_privacy::directory(&self.directory).is_ok())
        {
            return None;
        }
        let directory = fs::symlink_metadata(&self.directory).ok()?;
        if !directory.is_dir() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            // The home the directory is in, whose owner this account is.
            let home = fs::metadata(self.directory.parent()?).ok()?;
            if !kept_alone(directory.mode(), directory.uid(), home.uid()) {
                return None;
            }
        }
        // Not a link, not a pipe and not a file of two names.
        let file = crucible_privacy::open_read(&self.directory.join(name)).ok()?;
        let found = file.metadata().ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if !kept_alone(found.mode(), found.uid(), directory.uid()) {
                return None;
            }
        }
        // Past its age, or dated so it never ages, whether or not a write
        // has come by to take it out since.
        if !found
            .modified()
            .is_ok_and(|written| fresh(written, SystemTime::now(), self.bounds.age))
        {
            return None;
        }
        let most = u64::try_from(MOST_QUEUED).ok()?;
        if found.len() > most {
            return None;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, most + 1), &mut bytes).ok()?;
        (bytes.len() <= MOST_QUEUED).then_some(bytes)
    }

    /// Whether the directory is one, and not a link to one.
    fn is_directory(&self) -> bool {
        fs::symlink_metadata(&self.directory).is_ok_and(|found| found.is_dir())
    }

    /// Removes every entry past its age or dated more than [`SKEW`] ahead,
    /// then the oldest, of the [`MOST_LISTED`] oldest, until those left and
    /// `incoming` more bytes are within the bound. The entry `replacing`
    /// names is not counted, since the write replaces it.
    fn prune(&self, replacing: Option<&str>, incoming: u64) {
        let Ok(listed) = fs::read_dir(&self.directory) else {
            return;
        };
        let now = SystemTime::now();
        // The oldest names alone, newest on top so it is the one let go.
        let mut oldest: BinaryHeap<(SystemTime, String, u64)> = BinaryHeap::new();
        let mut held = incoming;
        for found in listed.flatten() {
            let Ok(name) = found.file_name().into_string() else {
                continue;
            };
            if !is_entry(&name) {
                continue;
            }
            let path = found.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            // Nothing but a file is an entry: a link here is never followed,
            // and is taken out so the write can land in its place.
            if !metadata.is_file() {
                let _ = fs::remove_file(&path);
                continue;
            }
            if replacing == Some(name.as_str()) {
                continue;
            }
            let written = metadata.modified().unwrap_or(now);
            if !fresh(written, now, self.bounds.age) {
                let _ = fs::remove_file(&path);
                continue;
            }
            held = held.saturating_add(metadata.len());
            oldest.push((written, name, metadata.len()));
            if oldest.len() > self.bounds.names {
                oldest.pop();
            }
        }
        for (_, name, length) in oldest.into_sorted_vec() {
            if held <= self.bounds.held {
                break;
            }
            if fs::remove_file(self.directory.join(name)).is_ok() {
                held = held.saturating_sub(length);
            }
        }
    }
}

/// Whether an entry written at `written` is still kept at `now`: no older
/// than `age`, and dated no more than [`SKEW`] ahead.
fn fresh(written: SystemTime, now: SystemTime, age: Duration) -> bool {
    match now.duration_since(written) {
        Ok(since) => since <= age,
        Err(ahead) => ahead.duration() <= SKEW,
    }
}

/// Whether `name` is an entry's: its key's digest in lowercase hex.
fn is_entry(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `name` is a draft a write of this cache made, as
/// `crucible_privacy::replace_in` names them.
fn is_draft(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(".crucible-") && name.ends_with(".writing"))
}

/// Whether `payload` is what `protocol` is sent, `pixels` in size: a PNG of
/// those pixels for Kitty and iTerm2, and for Sixel what the terminal is sent
/// as Sixel and no other sequence, painting no more than those pixels.
fn shown(protocol: Protocol, payload: &[u8], pixels: (u32, u32)) -> bool {
    match protocol {
        Protocol::Kitty | Protocol::Iterm2 => {
            // Taken here for no longer than a frame waits, since the decode
            // takes the slot for far longer otherwise.
            let Ok(_slot) = Slot::take(super::pictures::WAIT) else {
                return false;
            };
            decode(payload).is_ok_and(|read| (read.width(), read.height()) == pixels)
        }
        Protocol::Sixel => crucible_tui::graphics::sixel_body(payload)
            .is_some_and(|body| sixel_within(body.as_bytes(), pixels)),
    }
}

/// Whether the Sixel `body`, inside its device control string, says it is
/// `pixels` in its raster attributes and paints nothing outside them: a
/// payload that paints more than it says would draw past the cells it was
/// given. Anything but colours, sixels, repeats, returns and new bands is
/// refused, as is a body that does not say its size.
fn sixel_within(body: &[u8], (width, height): (u32, u32)) -> bool {
    let opened = body
        .iter()
        .position(|byte| *byte == b'q')
        .and_then(|at| body.get(at + 1..));
    let Some(rest) = opened.and_then(|rest| rest.strip_prefix(b"\"")) else {
        return false;
    };
    let mut said = [0_u64; 4];
    let mut rest = rest;
    for (at, slot) in said.iter_mut().enumerate() {
        let Some((number, after)) = number(rest) else {
            return false;
        };
        *slot = number;
        rest = after;
        if at < 3 {
            let Some(after) = rest.strip_prefix(b";") else {
                return false;
            };
            rest = after;
        }
    }
    let [_, _, across, down] = said;
    if (across, down) != (u64::from(width), u64::from(height)) {
        return false;
    }
    let bands = down.div_ceil(6);
    let (mut band, mut column) = (0_u64, 0_u64);
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        let painted = match byte {
            b'#' => {
                let taken = rest
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit() || **byte == b';')
                    .count();
                rest = rest.get(taken..).unwrap_or_default();
                0
            }
            b'!' => {
                let Some((count, after)) = number(rest) else {
                    return false;
                };
                let Some((sixel, after)) = after.split_first() else {
                    return false;
                };
                if !(0x3f..=0x7e).contains(sixel) {
                    return false;
                }
                rest = after;
                count.max(1)
            }
            0x3f..=0x7e => 1,
            b'$' => {
                column = 0;
                0
            }
            b'-' => {
                band += 1;
                column = 0;
                0
            }
            _ => return false,
        };
        if painted > 0 {
            column = column.saturating_add(painted);
            if band >= bands || column > across {
                return false;
            }
        }
    }
    true
}

/// The decimal number `bytes` opens with, and what follows it; nothing where
/// it opens with no digit or the number is past any picture's size.
fn number(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let (taken, rest) = bytes.split_at_checked(digits)?;
    if taken.is_empty() || taken.len() > 9 {
        return None;
    }
    let number = taken
        .iter()
        .fold(0_u64, |number, digit| number * 10 + u64::from(digit - b'0'));
    Some((number, rest))
}

/// An entry's body, read apart.
struct Read<'a> {
    protocol: u8,
    pixels: (u32, u32),
    svg: &'a [u8],
    payload: &'a [u8],
}

impl<'a> Read<'a> {
    /// `body` read apart, or nothing where it is too short for what it says.
    fn of(body: &'a [u8]) -> Option<Self> {
        let (&protocol, rest) = body.split_first()?;
        let (width, rest) = word(rest)?;
        let (height, rest) = word(rest)?;
        let (length, rest) = word(rest)?;
        let (svg, payload) = rest.split_at_checked(usize::try_from(length).ok()?)?;
        Some(Self {
            protocol,
            pixels: (width, height),
            svg,
            payload,
        })
    }
}

/// The little-endian word `bytes` opens with, and what follows it.
fn word(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let (word, rest) = bytes.split_first_chunk::<4>()?;
    Some((u32::from_le_bytes(*word), rest))
}

/// One picture's entry, named and sealed, on its way to the writer.
pub(crate) struct Entry {
    name: String,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.name)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl Entry {
    /// The entry for `source`'s picture under `key`, or nothing where it
    /// would be larger than the queue takes.
    fn of(source: &str, key: &PictureKey, form: &Form, payload: &Payload) -> Option<Self> {
        // An image is a file a turn named: what it shows is the file's, and
        // it is read again from the file rather than kept here.
        let svg: &[u8] = match form {
            Form::Svg(svg) => svg.as_str().as_bytes(),
            Form::Png(_) => return None,
        };
        if HEADER + BODY + svg.len() + payload.bytes().len() > MOST_QUEUED {
            return None;
        }
        let digest = keyed(source, key);
        let bytes = sealed(
            &digest,
            payload.protocol(),
            payload.pixels(),
            svg,
            payload.bytes(),
        )?;
        Some(Self {
            name: named(&digest),
            bytes,
        })
    }

    /// How many bytes it takes on disk.
    fn length(&self) -> u64 {
        u64::try_from(self.bytes.len()).unwrap_or(u64::MAX)
    }
}

/// The digest an entry for `source` under `key` is named by.
fn keyed(source: &str, key: &PictureKey) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"crucible picture\0");
    digest.update(env!("CARGO_PKG_VERSION").as_bytes());
    digest.update([0]);
    digest.update(format!("{key:?}").as_bytes());
    digest.update([0]);
    digest.update(Sha256::digest(source.as_bytes()));
    digest.finalize().into()
}

/// `digest` as the name of a file.
fn named(digest: &[u8; 32]) -> String {
    digest
        .iter()
        .fold(String::with_capacity(64), |mut name, byte| {
            use std::fmt::Write as _;
            let _ = write!(name, "{byte:02x}");
            name
        })
}

/// An entry's bytes.
fn sealed(
    digest: &[u8; 32],
    protocol: Protocol,
    (width, height): (u32, u32),
    svg: &[u8],
    payload: &[u8],
) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    body.push(tag(protocol));
    body.extend_from_slice(&width.to_le_bytes());
    body.extend_from_slice(&height.to_le_bytes());
    body.extend_from_slice(&u32::try_from(svg.len()).ok()?.to_le_bytes());
    body.extend_from_slice(svg);
    body.extend_from_slice(payload);
    let mut bytes = Vec::with_capacity(HEADER + body.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(digest);
    bytes.extend_from_slice(&<[u8; 32]>::from(Sha256::digest(&body)));
    bytes.extend_from_slice(&body);
    Some(bytes)
}

/// The byte an entry names `protocol` by.
const fn tag(protocol: Protocol) -> u8 {
    match protocol {
        Protocol::Kitty => 1,
        Protocol::Iterm2 => 2,
        Protocol::Sixel => 3,
    }
}

/// Whether a file or directory is reached by its owner alone.
#[cfg(unix)]
const fn kept_alone(mode: u32, owner: u32, expected: u32) -> bool {
    mode.trailing_zeros() >= 6 && owner == expected
}

/// The drawing thread's end of the writer's queue.
#[derive(Debug)]
pub(crate) struct Queue {
    sender: mpsc::Sender<Entry>,
    queued: Arc<AtomicUsize>,
}

impl Queue {
    /// Hands `source`'s picture under `key` to the writer, or drops it where
    /// it is not kept or the queue has no room for it; never waits.
    pub(crate) fn put(
        &self,
        source: &str,
        key: &PictureKey,
        form: &Form,
        payload: &Payload,
    ) -> bool {
        let Some(entry) = Entry::of(source, key, form, payload) else {
            return false;
        };
        let length = entry.bytes.len();
        let reserved = self
            .queued
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |queued| {
                queued
                    .checked_add(length)
                    .filter(|queued| *queued <= MOST_QUEUED)
            });
        if reserved.is_err() {
            return false;
        }
        if self.sender.send(entry).is_err() {
            self.queued.fetch_sub(length, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// The one thread's end of the queue, which writes what arrives.
#[derive(Debug)]
pub(crate) struct Writer {
    cache: Cache,
    receiver: mpsc::Receiver<Entry>,
    queued: Arc<AtomicUsize>,
}

impl Writer {
    /// Trims the cache, then writes each entry as it arrives until the queue
    /// is let go of.
    pub(crate) fn run(self) {
        self.cache.trim();
        for entry in &self.receiver {
            self.write(&entry);
        }
    }

    /// Writes every entry waiting now.
    #[cfg(test)]
    pub(crate) fn drain(&self) {
        while let Ok(entry) = self.receiver.try_recv() {
            self.write(&entry);
        }
    }

    /// Writes `entry` and frees the room it held in the queue, written or not.
    fn write(&self, entry: &Entry) {
        self.cache.put(entry);
        self.queued.fetch_sub(entry.bytes.len(), Ordering::SeqCst);
    }
}

/// A queue to the one writer for `cache`, and that writer.
pub(crate) fn writing(cache: Cache) -> (Queue, Writer) {
    let (sender, receiver) = mpsc::channel();
    let queued = Arc::new(AtomicUsize::new(0));
    (
        Queue {
            sender,
            queued: Arc::clone(&queued),
        },
        Writer {
            cache,
            receiver,
            queued,
        },
    )
}

#[cfg(test)]
mod tests;
