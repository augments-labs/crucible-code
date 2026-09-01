//! Bounded host-side decoder for the broker's terminal semantic scan.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_sandbox_broker::{
    BROKER_FAILURE_STATUS, ENTRY_DIRECTORY, ENTRY_FILE, ENTRY_SYMLINK, MAX_SCAN_ENTRIES,
    MAX_SCAN_EXTENTS, MAX_SCAN_PATH_BYTES, MAX_SCAN_ROOTS, MAX_SCAN_SYMLINK_BYTES, SCAN_END_FRAME,
    SCAN_FRAME, WAIT_STATUS_BYTES, decode_wait_status,
};

use super::{Entry, Snapshot, digest_file};

const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_SCAN_DEPTH: usize = 64;

pub(super) struct Terminal {
    pub(super) status: ExitStatus,
    pub(super) baselines: Vec<Snapshot>,
    pub(super) roots: Vec<Snapshot>,
}

pub(super) struct Receiver {
    thread: Option<JoinHandle<io::Result<Terminal>>>,
}

impl Receiver {
    pub(super) fn spawn(stream: UnixStream, stage: Option<PathBuf>) -> io::Result<Self> {
        let thread = thread::Builder::new()
            .name("crucible-sandbox-scan".into())
            .spawn(move || read_terminal(stream, stage.as_deref()))?;
        Ok(Self {
            thread: Some(thread),
        })
    }

    pub(super) fn finish(&mut self) -> io::Result<Terminal> {
        self.thread
            .take()
            .ok_or_else(|| invalid("terminal scan receiver is unavailable"))?
            .join()
            .map_err(|_| invalid("terminal scan receiver panicked"))?
    }
}

fn read_terminal(mut stream: UnixStream, stage: Option<&Path>) -> io::Result<Terminal> {
    let status = read_status(&mut stream)?;
    expect_frame(&mut stream, SCAN_FRAME, "terminal scan header")?;
    let root_count = bounded_usize(read_u32(&mut stream)?, MAX_SCAN_ROOTS, "root count")?;
    let payload_directory = if root_count == 0 {
        None
    } else {
        let directory = stage
            .ok_or_else(|| invalid("terminal scan has no host-owned payload directory"))?
            .join("payloads");
        create_private_directory(&directory)?;
        Some(directory)
    };
    let mut baselines = Vec::with_capacity(root_count);
    let mut roots = Vec::with_capacity(root_count);
    for expected_index in 0..root_count {
        let index = bounded_usize(read_u32(&mut stream)?, MAX_SCAN_ROOTS, "root index")?;
        if index != expected_index {
            return Err(invalid("terminal scan roots are not in canonical order"));
        }
        baselines.push(read_snapshot(
            &mut stream,
            payload_directory.as_deref(),
            expected_index,
            false,
        )?);
        roots.push(read_snapshot(
            &mut stream,
            payload_directory.as_deref(),
            expected_index,
            true,
        )?);
    }
    expect_frame(&mut stream, SCAN_END_FRAME, "terminal scan trailer")?;
    let mut trailing = [0_u8; 1];
    if stream.read(&mut trailing)? != 0 {
        return Err(invalid("terminal scan contains trailing bytes"));
    }
    Ok(Terminal {
        status,
        baselines,
        roots,
    })
}

fn read_snapshot(
    stream: &mut UnixStream,
    payload_directory: Option<&Path>,
    root: usize,
    allow_payload: bool,
) -> io::Result<Snapshot> {
    let entry_count = bounded_usize(read_u32(stream)?, MAX_SCAN_ENTRIES, "entry count")?;
    let mut entries = BTreeMap::new();
    let mut previous = None;
    for entry_index in 0..entry_count {
        let path = read_path(stream, MAX_SCAN_PATH_BYTES, true)?;
        if previous.as_ref().is_some_and(|prior| prior >= &path) {
            return Err(invalid(
                "terminal semantic entries are not strictly ordered",
            ));
        }
        previous = Some(path.clone());
        let kind = read_u8(stream)?;
        let entry = match kind {
            ENTRY_DIRECTORY => read_directory(stream)?,
            ENTRY_FILE => {
                let payload = payload_directory
                    .map(|directory| directory.join(format!("{root}-{entry_index}")));
                read_file(stream, payload.as_deref(), allow_payload)?
            }
            ENTRY_SYMLINK => {
                Entry::Symlink(read_path(stream, MAX_SCAN_SYMLINK_BYTES, false)?.into_os_string())
            }
            _ => return Err(invalid("terminal scan contains an unknown entry kind")),
        };
        if entries.insert(path, entry).is_some() {
            return Err(invalid("terminal scan contains a duplicate entry"));
        }
    }
    validate_links(&entries)?;
    Ok(Snapshot { entries })
}

fn read_status(stream: &mut UnixStream) -> io::Result<ExitStatus> {
    use std::os::unix::process::ExitStatusExt as _;

    let mut frame = [0_u8; WAIT_STATUS_BYTES];
    stream.read_exact(&mut frame)?;
    let raw = decode_wait_status(frame);
    if raw == BROKER_FAILURE_STATUS {
        return Err(invalid(
            "sandbox broker could not create, reap, or scan the workload",
        ));
    }
    Ok(ExitStatus::from_raw(raw))
}

fn read_directory(stream: &mut UnixStream) -> io::Result<Entry> {
    let (mode, modified) = read_metadata(stream)?;
    Ok(Entry::Directory { mode, modified })
}

fn read_file(
    stream: &mut UnixStream,
    payload_path: Option<&Path>,
    allow_payload: bool,
) -> io::Result<Entry> {
    let (mode, modified) = read_metadata(stream)?;
    let length = read_u64(stream)?;
    let mut digest = [0_u8; 32];
    stream.read_exact(&mut digest)?;
    let extents = read_extents(stream, length)?;
    let linked_to = read_optional_path(stream)?;
    let payload = match read_u8(stream)? {
        0 => None,
        1 if allow_payload && linked_to.is_none() => {
            let payload_length = read_u64(stream)?;
            if payload_length != extent_bytes(&extents)? {
                return Err(invalid("terminal payload length does not match its record"));
            }
            let path =
                payload_path.ok_or_else(|| invalid("terminal payload path is unavailable"))?;
            Some(read_payload(stream, path, length, digest, &extents)?)
        }
        1 if !allow_payload => {
            return Err(invalid("pre-release baseline cannot carry file payloads"));
        }
        1 => return Err(invalid("hard-link aliases cannot carry duplicate payloads")),
        _ => return Err(invalid("terminal payload flag is invalid")),
    };
    Ok(Entry::File {
        mode,
        modified,
        length,
        digest,
        extents,
        linked_to,
        payload,
    })
}

fn read_extents(stream: &mut UnixStream, length: u64) -> io::Result<Vec<(u64, u64)>> {
    let count = bounded_usize(read_u32(stream)?, MAX_SCAN_EXTENTS, "extent count")?;
    let mut extents = Vec::with_capacity(count);
    let mut previous_end = 0_u64;
    for _ in 0..count {
        let offset = read_u64(stream)?;
        let extent_length = read_u64(stream)?;
        if extent_length == 0 {
            return Err(invalid("terminal file contains an empty data extent"));
        }
        let end = offset
            .checked_add(extent_length)
            .ok_or_else(|| invalid("terminal file extent overflows its length"))?;
        if offset < previous_end {
            return Err(invalid("terminal file extents overlap or are out of order"));
        }
        if end > length {
            return Err(invalid("terminal file extent exceeds its logical length"));
        }
        extents.push((offset, extent_length));
        previous_end = end;
    }
    Ok(extents)
}

fn extent_bytes(extents: &[(u64, u64)]) -> io::Result<u64> {
    extents.iter().try_fold(0_u64, |total, (_, length)| {
        total
            .checked_add(*length)
            .ok_or_else(|| invalid("terminal file extent bytes overflow"))
    })
}

fn read_metadata(stream: &mut UnixStream) -> io::Result<(u32, Option<SystemTime>)> {
    let mode = read_u32(stream)?;
    if mode & !0o777 != 0 {
        return Err(invalid("terminal entry contains unsafe mode bits"));
    }
    let seconds = read_i64(stream)?;
    let nanoseconds = read_u32(stream)?;
    if nanoseconds >= 1_000_000_000 {
        return Err(invalid("terminal entry contains an invalid mtime"));
    }
    Ok((mode, Some(system_time(seconds, nanoseconds)?)))
}

fn system_time(seconds: i64, nanoseconds: u32) -> io::Result<SystemTime> {
    let subsecond = Duration::from_nanos(u64::from(nanoseconds));
    if seconds >= 0 {
        let whole = Duration::from_secs(
            u64::try_from(seconds).map_err(|_| invalid("terminal mtime is out of range"))?,
        );
        UNIX_EPOCH
            .checked_add(whole)
            .and_then(|time| time.checked_add(subsecond))
            .ok_or_else(|| invalid("terminal mtime is out of range"))
    } else {
        let whole = Duration::from_secs(seconds.unsigned_abs());
        UNIX_EPOCH
            .checked_sub(whole)
            .and_then(|time| time.checked_add(subsecond))
            .ok_or_else(|| invalid("terminal mtime is out of range"))
    }
}

fn read_payload(
    stream: &mut UnixStream,
    path: &Path,
    length: u64,
    expected_digest: [u8; 32],
    extents: &[(u64, u64)],
) -> io::Result<PathBuf> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut output = options.open(path)?;
    output.set_len(length)?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    for (offset, extent_length) in extents {
        output.seek(SeekFrom::Start(*offset))?;
        let mut remaining = *extent_length;
        while remaining > 0 {
            let wanted = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
                .map_err(|_| invalid("payload chunk length is out of range"))?;
            let bytes = buffer
                .get_mut(..wanted)
                .ok_or_else(|| invalid("payload chunk exceeded its buffer"))?;
            stream.read_exact(bytes)?;
            output.write_all(bytes)?;
            remaining = remaining.saturating_sub(
                u64::try_from(wanted).map_err(|_| invalid("payload byte count overflow"))?,
            );
        }
    }
    output.sync_all()?;
    if digest_file(path)? != expected_digest {
        return Err(invalid("terminal payload digest does not match its record"));
    }
    Ok(path.to_path_buf())
}

fn validate_links(entries: &BTreeMap<PathBuf, Entry>) -> io::Result<()> {
    for (path, entry) in entries {
        let Entry::File {
            mode,
            modified,
            length,
            digest,
            extents,
            linked_to: Some(linked_to),
            payload: _,
        } = entry
        else {
            continue;
        };
        if linked_to >= path {
            return Err(invalid("hard-link anchor does not precede its alias"));
        }
        let Some(Entry::File {
            mode: anchor_mode,
            modified: anchor_modified,
            length: anchor_length,
            digest: anchor_digest,
            extents: anchor_extents,
            linked_to: None,
            payload: _,
        }) = entries.get(linked_to)
        else {
            return Err(invalid("hard-link anchor is missing or itself an alias"));
        };
        if (mode, modified, length, digest, extents)
            != (
                anchor_mode,
                anchor_modified,
                anchor_length,
                anchor_digest,
                anchor_extents,
            )
        {
            return Err(invalid("hard-link alias metadata differs from its anchor"));
        }
    }
    Ok(())
}

fn read_optional_path(stream: &mut UnixStream) -> io::Result<Option<PathBuf>> {
    let length = read_u32(stream)?;
    if length == u32::MAX {
        return Ok(None);
    }
    let length = bounded_usize(length, MAX_SCAN_PATH_BYTES, "path length")?;
    read_path_of_length(stream, length, true).map(Some)
}

fn read_path(stream: &mut UnixStream, maximum: usize, relative: bool) -> io::Result<PathBuf> {
    let length = bounded_usize(read_u32(stream)?, maximum, "path length")?;
    read_path_of_length(stream, length, relative)
}

fn read_path_of_length(
    stream: &mut UnixStream,
    length: usize,
    relative: bool,
) -> io::Result<PathBuf> {
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes)?;
    if bytes.contains(&0) {
        return Err(invalid("terminal path contains a NUL byte"));
    }
    let path = PathBuf::from(OsString::from_vec(bytes));
    if relative {
        validate_relative(&path)?;
    }
    Ok(path)
}

fn validate_relative(path: &Path) -> io::Result<()> {
    if path.is_absolute() {
        return Err(invalid("terminal path is absolute"));
    }
    let mut depth = 0_usize;
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid("terminal path contains a non-normal component"));
        }
        depth = depth.saturating_add(1);
        if depth > MAX_SCAN_DEPTH {
            return Err(invalid("terminal path exceeds its depth bound"));
        }
    }
    Ok(())
}

fn expect_frame(stream: &mut UnixStream, expected: [u8; 8], field: &'static str) -> io::Result<()> {
    let mut actual = [0_u8; 8];
    stream.read_exact(&mut actual)?;
    if actual == expected {
        Ok(())
    } else {
        Err(invalid(match field {
            "terminal scan header" => "terminal scan header is invalid",
            _ => "terminal scan trailer is invalid",
        }))
    }
}

fn read_u8(stream: &mut UnixStream) -> io::Result<u8> {
    let mut bytes = [0_u8; 1];
    stream.read_exact(&mut bytes)?;
    bytes
        .first()
        .copied()
        .ok_or_else(|| invalid("one-byte field is unavailable"))
}

fn read_u32(stream: &mut UnixStream) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_i64(stream: &mut UnixStream) -> io::Result<i64> {
    let mut bytes = [0_u8; 8];
    stream.read_exact(&mut bytes)?;
    Ok(i64::from_le_bytes(bytes))
}

fn read_u64(stream: &mut UnixStream) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    stream.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn bounded_usize(value: u32, maximum: usize, field: &'static str) -> io::Result<usize> {
    let value = usize::try_from(value).map_err(|_| invalid("wire field is out of range"))?;
    if value > maximum {
        return Err(invalid(match field {
            "root count" => "terminal root count exceeds its bound",
            "root index" => "terminal root index exceeds its bound",
            "entry count" => "terminal entry count exceeds its bound",
            "extent count" => "terminal file extent count exceeds its bound",
            _ => "terminal path length exceeds its bound",
        }));
    }
    Ok(value)
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn invalid(problem: &'static str) -> io::Error {
    io::Error::other(problem)
}
