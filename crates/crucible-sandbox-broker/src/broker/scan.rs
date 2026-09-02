//! Complete merged-view scanning and bounded terminal transfer.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Seek as _, SeekFrom, Write};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crucible_sandbox_broker::{
    ENTRY_DIRECTORY, ENTRY_FILE, ENTRY_SYMLINK, MAX_SCAN_ENTRIES, MAX_SCAN_EXTENTS,
    MAX_SCAN_FILE_BYTES, MAX_SCAN_PATH_BYTES, MAX_SCAN_ROOTS, MAX_SCAN_SYMLINK_BYTES,
    SCAN_END_FRAME, SCAN_FRAME,
};
use sha2::{Digest as _, Sha256};

const MAX_SCAN_DEPTH: usize = 64;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const RESERVED_PREFIX: &[u8] = b".crucible-sandbox-";

type ContentKey = (u64, [u8; 32], Vec<(u64, u64)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    entries: BTreeMap<Vec<u8>, Entry>,
    hard_links: Vec<Vec<PathBuf>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Directory {
        mode: u32,
        mtime_seconds: i64,
        mtime_nanoseconds: u32,
    },
    File {
        mode: u32,
        mtime_seconds: i64,
        mtime_nanoseconds: u32,
        length: u64,
        digest: [u8; 32],
        extents: Vec<(u64, u64)>,
        linked_to: Option<Vec<u8>>,
    },
    Symlink(Vec<u8>),
}

pub(crate) fn prepare(roots: &[PathBuf], exclusions: &[PathBuf]) -> io::Result<Vec<Snapshot>> {
    if roots.len() > MAX_SCAN_ROOTS {
        return Err(invalid("writable root count exceeds the protocol bound"));
    }
    let mut baselines = Vec::with_capacity(roots.len());
    for root in roots {
        let baseline = scan(root, exclusions)?;
        normalize_hard_links(root, &baseline)?;
        restore_directory_metadata(root, &baseline)?;
        let normalized = scan(root, exclusions)?;
        if baseline.entries != normalized.entries {
            return Err(invalid(
                "hard-link normalization changed the projected semantic view",
            ));
        }
        baselines.push(normalized);
    }
    Ok(baselines)
}

fn restore_directory_metadata(root: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let mut directories = snapshot
        .entries
        .iter()
        .filter_map(|(path, entry)| match entry {
            Entry::Directory {
                mode,
                mtime_seconds,
                mtime_nanoseconds,
            } => Some((
                path,
                raw_path(path).components().count(),
                *mode,
                *mtime_seconds,
                *mtime_nanoseconds,
            )),
            Entry::File { .. } | Entry::Symlink(_) => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by(|(left, left_depth, ..), (right, right_depth, ..)| {
        right_depth.cmp(left_depth).then_with(|| right.cmp(left))
    });
    for (relative, _, mode, seconds, nanoseconds) in directories {
        let path = root.join(raw_path(relative));
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        let modified = system_time(seconds, nanoseconds)?;
        File::open(path)?.set_times(FileTimes::new().set_modified(modified))?;
    }
    Ok(())
}

fn system_time(seconds: i64, nanoseconds: u32) -> io::Result<std::time::SystemTime> {
    use std::time::{Duration, UNIX_EPOCH};

    let subsecond = Duration::from_nanos(u64::from(nanoseconds));
    if seconds >= 0 {
        let whole = Duration::from_secs(
            u64::try_from(seconds).map_err(|_| invalid("file mtime is out of range"))?,
        );
        UNIX_EPOCH
            .checked_add(whole)
            .and_then(|time| time.checked_add(subsecond))
            .ok_or_else(|| invalid("file mtime is out of range"))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .and_then(|time| time.checked_add(subsecond))
            .ok_or_else(|| invalid("file mtime is out of range"))
    }
}

pub(crate) fn write_terminal(
    channel: &mut File,
    roots: &[PathBuf],
    exclusions: &[PathBuf],
    baselines: &[Snapshot],
) -> io::Result<()> {
    if roots.len() != baselines.len() || roots.len() > MAX_SCAN_ROOTS {
        return Err(invalid("terminal scan root set does not match preparation"));
    }
    write_bytes(channel, &SCAN_FRAME)?;
    write_u32(channel, bounded_u32(roots.len(), "root count")?)?;
    for (index, (root, baseline)) in roots.iter().zip(baselines).enumerate() {
        let final_snapshot = scan(root, exclusions)?;
        write_u32(channel, bounded_u32(index, "root index")?)?;
        let baseline_content = baseline_content(baseline);
        write_snapshot(channel, root, baseline, &baseline_content, false)?;
        write_snapshot(channel, root, &final_snapshot, &baseline_content, true)?;
    }
    write_bytes(channel, &SCAN_END_FRAME)?;
    channel.flush()
}

fn write_snapshot(
    channel: &mut File,
    root: &Path,
    snapshot: &Snapshot,
    baseline_content: &BTreeSet<ContentKey>,
    payloads: bool,
) -> io::Result<()> {
    write_u32(channel, bounded_u32(snapshot.entries.len(), "entry count")?)?;
    for (path, entry) in &snapshot.entries {
        write_path(channel, path)?;
        match entry {
            Entry::Directory {
                mode,
                mtime_seconds,
                mtime_nanoseconds,
            } => {
                write_u8(channel, ENTRY_DIRECTORY)?;
                write_metadata(channel, *mode, *mtime_seconds, *mtime_nanoseconds)?;
            }
            Entry::File {
                mode,
                mtime_seconds,
                mtime_nanoseconds,
                length,
                digest,
                extents,
                linked_to,
            } => {
                write_u8(channel, ENTRY_FILE)?;
                write_metadata(channel, *mode, *mtime_seconds, *mtime_nanoseconds)?;
                write_u64(channel, *length)?;
                write_bytes(channel, digest)?;
                write_extents(channel, extents)?;
                write_optional_path(channel, linked_to.as_deref())?;
                let payload = payloads
                    && linked_to.is_none()
                    && !baseline_content.contains(&(*length, *digest, extents.clone()));
                write_u8(channel, u8::from(payload))?;
                if payload {
                    write_u64(channel, extent_bytes(extents)?)?;
                    stream_file(channel, &root_path(root, path), *length, *digest, extents)?;
                }
            }
            Entry::Symlink(target) => {
                write_u8(channel, ENTRY_SYMLINK)?;
                write_path_bound(channel, target, MAX_SCAN_SYMLINK_BYTES, "symlink target")?;
            }
        }
    }
    Ok(())
}

fn baseline_content(snapshot: &Snapshot) -> BTreeSet<ContentKey> {
    snapshot
        .entries
        .values()
        .filter_map(|entry| match entry {
            Entry::File {
                length,
                digest,
                extents,
                ..
            } => Some((*length, *digest, extents.clone())),
            Entry::Directory { .. } | Entry::Symlink(_) => None,
        })
        .collect()
}

fn scan(root: &Path, exclusions: &[PathBuf]) -> io::Result<Snapshot> {
    let mut builder = Builder {
        exclusions: exclusions.to_vec(),
        ..Builder::default()
    };
    builder.visit(root, Path::new(""), 0)?;
    let mut hard_links = Vec::new();
    for (expected, paths) in builder.hard_links.into_values() {
        if u64::try_from(paths.len()) != Ok(expected) {
            return Err(invalid(
                "a projected hard-link group escapes the writable root",
            ));
        }
        if paths.len() > 1 {
            hard_links.push(paths);
        }
    }
    Ok(Snapshot {
        entries: builder.entries,
        hard_links,
    })
}

#[derive(Default)]
struct Builder {
    entries: BTreeMap<Vec<u8>, Entry>,
    first_links: BTreeMap<(u64, u64), Vec<u8>>,
    hard_links: BTreeMap<(u64, u64), (u64, Vec<PathBuf>)>,
    retained: usize,
    exclusions: Vec<PathBuf>,
}

impl Builder {
    fn visit(&mut self, root: &Path, relative: &Path, depth: usize) -> io::Result<()> {
        if depth > MAX_SCAN_DEPTH {
            return Err(invalid("projected tree exceeds its depth bound"));
        }
        self.retained = self.retained.saturating_add(1);
        if self.retained > MAX_SCAN_ENTRIES {
            return Err(invalid("projected tree exceeds its entry bound"));
        }
        let path = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(relative)
        };
        let metadata = if relative.as_os_str().is_empty() {
            fs::metadata(&path)?
        } else {
            fs::symlink_metadata(&path)?
        };
        validate_publishable_metadata(&path, &metadata)?;
        let raw_relative = relative.as_os_str().as_bytes().to_vec();
        if raw_relative.len() > MAX_SCAN_PATH_BYTES {
            return Err(invalid("projected path exceeds its byte bound"));
        }
        let entry = if metadata.is_dir() {
            let (seconds, nanoseconds) = mtime(&metadata)?;
            Entry::Directory {
                mode: safe_mode(&metadata),
                mtime_seconds: seconds,
                mtime_nanoseconds: nanoseconds,
            }
        } else if metadata.is_file() {
            if metadata.len() > MAX_SCAN_FILE_BYTES {
                return Err(invalid("projected file exceeds its byte bound"));
            }
            let identity = (metadata.dev(), metadata.ino());
            let linked_to = if metadata.nlink() > 1 {
                let group = self
                    .hard_links
                    .entry(identity)
                    .or_insert_with(|| (metadata.nlink(), Vec::new()));
                if group.0 != metadata.nlink() {
                    return Err(invalid("projected hard-link identity changed"));
                }
                group.1.push(relative.to_path_buf());
                if let Some(first) = self.first_links.get(&identity) {
                    Some(first.clone())
                } else {
                    self.first_links.insert(identity, raw_relative.clone());
                    None
                }
            } else {
                None
            };
            let (seconds, nanoseconds) = mtime(&metadata)?;
            Entry::File {
                mode: safe_mode(&metadata),
                mtime_seconds: seconds,
                mtime_nanoseconds: nanoseconds,
                length: metadata.len(),
                digest: digest_file(&path)?,
                extents: sparse_extents(&path, metadata.len())?,
                linked_to,
            }
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?.as_os_str().as_bytes().to_vec();
            if target.len() > MAX_SCAN_SYMLINK_BYTES {
                return Err(invalid("projected symlink target exceeds its byte bound"));
            }
            Entry::Symlink(target)
        } else {
            return Err(invalid("projected tree contains a special file"));
        };
        if self.entries.insert(raw_relative, entry).is_some() {
            return Err(invalid("projected tree contains a duplicate path"));
        }

        if metadata.is_dir() {
            let children = bounded_children(&path, self.retained, MAX_SCAN_ENTRIES)?;
            for child in children {
                let name = child.file_name();
                if protected_name(&name) {
                    continue;
                }
                if reserved_name(&name) {
                    return Err(invalid("projected tree contains a reserved name"));
                }
                let child_path = path.join(&name);
                if self
                    .exclusions
                    .iter()
                    .any(|excluded| child_path == *excluded || child_path.starts_with(excluded))
                {
                    continue;
                }
                self.visit(root, &relative.join(name), depth.saturating_add(1))?;
            }
        }
        Ok(())
    }
}

fn normalize_hard_links(root: &Path, snapshot: &Snapshot) -> io::Result<()> {
    for (index, paths) in snapshot.hard_links.iter().enumerate() {
        let source_relative = paths
            .first()
            .ok_or_else(|| invalid("hard-link group has no source"))?;
        let source = root.join(source_relative);
        let temporary = root.join(format!(".crucible-sandbox-hardlink-{index}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut output = options.open(&temporary)?;
        copy_sparse(&source, &mut output)?;
        copy_metadata(&source, &temporary)?;
        for relative in paths {
            fs::remove_file(root.join(relative))?;
        }
        for relative in paths {
            fs::hard_link(&temporary, root.join(relative))?;
        }
        fs::remove_file(&temporary)?;
    }
    Ok(())
}

fn copy_metadata(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(safe_mode(&metadata)),
    )?;
    let mut times = FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    OpenOptions::new()
        .write(true)
        .open(destination)?
        .set_times(times)
}

fn sparse_extents(path: &Path, length: u64) -> io::Result<Vec<(u64, u64)>> {
    use rustix::fs::SeekFrom as RustixSeekFrom;
    use rustix::io::Errno;

    let file = File::open(path)?;
    let mut extents = Vec::new();
    let mut cursor = 0_u64;
    while cursor < length {
        let data = match rustix::fs::seek(&file, RustixSeekFrom::Data(cursor)) {
            Ok(data) => data,
            Err(Errno::NXIO) => break,
            Err(problem) => return Err(problem.into()),
        };
        if data >= length {
            break;
        }
        let hole = rustix::fs::seek(&file, RustixSeekFrom::Hole(data))?.min(length);
        if hole <= data {
            return Err(invalid("file extent map did not advance"));
        }
        extents.push((data, hole.saturating_sub(data)));
        if extents.len() > MAX_SCAN_EXTENTS {
            return Err(invalid("file extent count exceeds its bound"));
        }
        cursor = hole;
    }
    Ok(extents)
}

fn copy_sparse(source: &Path, destination: &mut File) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    let extents = sparse_extents(source, metadata.len())?;
    destination.set_len(metadata.len())?;
    let mut input = File::open(source)?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    for (offset, length) in extents {
        input.seek(SeekFrom::Start(offset))?;
        destination.seek(SeekFrom::Start(offset))?;
        copy_exact(&mut input, destination, length, &mut buffer)?;
    }
    destination.sync_all()
}

fn copy_exact(
    source: &mut impl Read,
    destination: &mut impl Write,
    mut remaining: u64,
    buffer: &mut [u8],
) -> io::Result<()> {
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(
            u64::try_from(buffer.len()).map_err(|_| invalid("copy buffer length is invalid"))?,
        ))
        .map_err(|_| invalid("copy chunk length is invalid"))?;
        let chunk = buffer
            .get_mut(..wanted)
            .ok_or_else(|| invalid("copy chunk exceeded its buffer"))?;
        source.read_exact(chunk)?;
        destination.write_all(chunk)?;
        remaining = remaining.saturating_sub(
            u64::try_from(wanted).map_err(|_| invalid("copy byte count overflow"))?,
        );
    }
    Ok(())
}

fn extent_bytes(extents: &[(u64, u64)]) -> io::Result<u64> {
    extents.iter().try_fold(0_u64, |total, (_, length)| {
        total
            .checked_add(*length)
            .ok_or_else(|| invalid("file extent bytes overflow"))
    })
}

/// Reads one directory's children in name order without buffering more of them
/// than the entry bound still allows: a workload chooses how many names one
/// directory holds, and the bound must apply before they are all in memory.
fn bounded_children(path: &Path, retained: usize, bound: usize) -> io::Result<Vec<fs::DirEntry>> {
    let mut children = Vec::new();
    for child in fs::read_dir(path)? {
        if retained.saturating_add(children.len()) >= bound {
            return Err(invalid("projected tree exceeds its entry bound"));
        }
        children.push(child?);
    }
    children.sort_by_key(fs::DirEntry::file_name);
    Ok(children)
}

fn digest_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let bytes = buffer
            .get(..read)
            .ok_or_else(|| invalid("file read exceeded its buffer"))?;
        digest.update(bytes);
    }
    Ok(digest.finalize().into())
}

fn stream_file(
    channel: &mut File,
    path: &Path,
    expected_length: u64,
    expected_digest: [u8; 32],
    extents: &[(u64, u64)],
) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    for (offset, length) in extents {
        file.seek(SeekFrom::Start(*offset))?;
        copy_exact(&mut file, channel, *length, &mut buffer)?;
    }
    let metadata = file.metadata()?;
    if metadata.len() != expected_length || digest_file(path)? != expected_digest {
        return Err(invalid("payload changed after the terminal semantic scan"));
    }
    Ok(())
}

fn write_extents(channel: &mut File, extents: &[(u64, u64)]) -> io::Result<()> {
    if extents.len() > MAX_SCAN_EXTENTS {
        return Err(invalid("file extent count exceeds its bound"));
    }
    write_u32(channel, bounded_u32(extents.len(), "extent count")?)?;
    for (offset, length) in extents {
        write_u64(channel, *offset)?;
        write_u64(channel, *length)?;
    }
    Ok(())
}

fn write_metadata(
    channel: &mut File,
    mode: u32,
    mtime_seconds: i64,
    mtime_nanoseconds: u32,
) -> io::Result<()> {
    write_u32(channel, mode)?;
    write_i64(channel, mtime_seconds)?;
    write_u32(channel, mtime_nanoseconds)
}

fn write_optional_path(channel: &mut File, path: Option<&[u8]>) -> io::Result<()> {
    if let Some(path) = path {
        write_path(channel, path)
    } else {
        write_u32(channel, u32::MAX)
    }
}

fn write_path(channel: &mut File, bytes: &[u8]) -> io::Result<()> {
    write_path_bound(channel, bytes, MAX_SCAN_PATH_BYTES, "path")
}

fn write_path_bound(
    channel: &mut File,
    bytes: &[u8],
    maximum: usize,
    field: &'static str,
) -> io::Result<()> {
    if bytes.len() > maximum {
        return Err(invalid(match field {
            "symlink target" => "symlink target exceeds its protocol bound",
            _ => "path exceeds its protocol bound",
        }));
    }
    write_u32(channel, bounded_u32(bytes.len(), field)?)?;
    write_bytes(channel, bytes)
}

fn write_u8(channel: &mut File, value: u8) -> io::Result<()> {
    write_bytes(channel, &[value])
}

fn write_u32(channel: &mut File, value: u32) -> io::Result<()> {
    write_bytes(channel, &value.to_le_bytes())
}

fn write_i64(channel: &mut File, value: i64) -> io::Result<()> {
    write_bytes(channel, &value.to_le_bytes())
}

fn write_u64(channel: &mut File, value: u64) -> io::Result<()> {
    write_bytes(channel, &value.to_le_bytes())
}

fn write_bytes(channel: &mut File, bytes: &[u8]) -> io::Result<()> {
    channel.write_all(bytes)
}

fn bounded_u32(value: usize, field: &'static str) -> io::Result<u32> {
    u32::try_from(value).map_err(|_| {
        invalid(match field {
            "root count" => "root count exceeds its wire representation",
            "root index" => "root index exceeds its wire representation",
            "entry count" => "entry count exceeds its wire representation",
            _ => "field length exceeds its wire representation",
        })
    })
}

fn mtime(metadata: &fs::Metadata) -> io::Result<(i64, u32)> {
    let nanoseconds = u32::try_from(metadata.mtime_nsec())
        .ok()
        .filter(|value| *value < 1_000_000_000)
        .ok_or_else(|| invalid("file mtime nanoseconds are invalid"))?;
    Ok((metadata.mtime(), nanoseconds))
}

fn raw_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

fn root_path(root: &Path, relative: &[u8]) -> PathBuf {
    if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(raw_path(relative))
    }
}

fn safe_mode(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode() & 0o777
}

fn validate_publishable_metadata(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use rustix::io::Errno;

    if metadata.mode() & 0o7000 != 0 {
        return Err(invalid(
            "projected tree contains special mode bits the publisher cannot preserve",
        ));
    }
    let mut names = [0_u8; 4096];
    match rustix::fs::llistxattr(path, &mut names) {
        Ok(0) | Err(Errno::NOTSUP) => Ok(()),
        Ok(_) | Err(Errno::RANGE) => Err(invalid(
            "projected tree contains extended metadata the publisher cannot preserve",
        )),
        Err(problem) => Err(problem.into()),
    }
}

fn protected_name(name: &OsStr) -> bool {
    matches!(
        name.as_bytes(),
        b".git" | b".agents" | b".codex" | b".crucible"
    )
}

fn reserved_name(name: &OsStr) -> bool {
    name.as_bytes().starts_with(RESERVED_PREFIX)
}

fn invalid(problem: &'static str) -> io::Error {
    io::Error::other(problem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_beyond_the_byte_bound_is_refused_before_it_is_digested() {
        let root =
            std::env::temp_dir().join(format!("crucible-scan-byte-bound-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scan root");
        let sparse = fs::File::create(root.join("sparse.bin")).expect("sparse fixture");
        sparse
            .set_len(MAX_SCAN_FILE_BYTES + 1)
            .expect("a sparse file beyond the bound");
        drop(sparse);

        let refused = scan(&root, &[]).expect_err("an oversized file was scanned");
        assert!(
            refused.to_string().contains("byte bound"),
            "unexpected refusal: {refused}"
        );
        fs::remove_dir_all(&root).expect("fixture removed");
    }
}
