//! Durable writable-transaction admission and the closed R8 lifecycle grammar.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crucible_core::{
    CallResultKey, SandboxError, SandboxFilesystemAccess, SandboxId, SandboxRequest,
};
use rustix::fs::{FlockOperation, Mode, OFlags};
use sha2::{Digest as _, Sha256};

const WAL_MAGIC: &[u8; 8] = b"CRSBWAL1";
const WAL_VERSION: u16 = 2;
const SANDBOX_ID_BYTES: usize = 36;
const OWNER_PID_BYTES: usize = 4;
const OWNER_START_BYTES: usize = 8;
const OWNER_BOOT_BYTES: usize = 32;
const IDENTITY_OFFSET: usize = 52;
const OWNER_PID_OFFSET: usize = IDENTITY_OFFSET + SANDBOX_ID_BYTES;
const OWNER_START_OFFSET: usize = OWNER_PID_OFFSET + OWNER_PID_BYTES;
const OWNER_BOOT_OFFSET: usize = OWNER_START_OFFSET + OWNER_START_BYTES;
const CALL_RESULT_KEY_BYTES: usize = 32;
const CALL_RESULT_KEY_OFFSET: usize = OWNER_BOOT_OFFSET + OWNER_BOOT_BYTES;
const WAL_PREFIX_BYTES: usize = 8
    + 2
    + 2
    + 8
    + 32
    + SANDBOX_ID_BYTES
    + OWNER_PID_BYTES
    + OWNER_START_BYTES
    + OWNER_BOOT_BYTES
    + CALL_RESULT_KEY_BYTES;
const WAL_CHECKSUM_BYTES: usize = 32;
const MAX_WAL_PAYLOAD_BYTES: usize = 1 + 32;
const MAX_WAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_STALE_TRANSACTIONS: usize = 128;
const STAGE_PREFIX: &str = "crucible-projection-";

/// Whether the original call waits for the terminal result or receives one
/// accepted result after the one-shot release boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InvocationMode {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Invocation {
    mode: InvocationMode,
    call_result_key: [u8; CALL_RESULT_KEY_BYTES],
}

impl Invocation {
    fn new(mode: InvocationMode, call_result_key: Option<CallResultKey>) -> io::Result<Self> {
        let call_result_key = match (mode, call_result_key) {
            (InvocationMode::Foreground, None) => [0_u8; CALL_RESULT_KEY_BYTES],
            (InvocationMode::Background, Some(key)) => key.bytes(),
            _ => {
                return Err(invalid(
                    "sandbox transaction result identity does not match invocation mode",
                ));
            }
        };
        Ok(Self {
            mode,
            call_result_key,
        })
    }
}

/// Closed transaction records. Index-bearing records use contiguous indices
/// beginning at zero; records after a terminal are always invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Record {
    Initialized(InvocationMode),
    Prepared,
    ReleaseIntent,
    OwnerTransferred,
    GoSentOrAmbiguous,
    CallAcceptIntent,
    CallAccepted([u8; 32]),
    RefusalObserved,
    PreparationCleanupIntent,
    PreparationCleanupProved,
    PreparationCleanupUnproved,
    Refused,
    CommandExited,
    WorkloadReapIntent,
    WorkloadReaped,
    ScanIntent,
    ScanTransferred,
    StageIntent(u32),
    Staged(u32),
    PublicationStaged,
    ScopeReapIntent,
    ScopeReapProved,
    ScopeReapUnproved,
    ApplyIntent(u32),
    Applied(u32),
    AbortObserved,
    QuarantineObserved,
    RollbackIntent(u32),
    RollbackApplied(u32),
    DiscardIntent(u32),
    Discarded(u32),
    Committed,
    RolledBack,
    Quarantined,
}

/// Incremental validator used by execution and, later, WAL recovery.
#[derive(Debug, Default, Clone)]
pub(super) struct Machine {
    records: Vec<Record>,
}

/// One append-only, fsynced transaction record plus the descriptor-held global
/// lease that gives it exclusive writable authority.
pub(super) struct Transaction {
    _lease: Option<Lease>,
    journal: File,
    machine: Machine,
    frame: FrameState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameState {
    sequence: u64,
    previous: [u8; 32],
    identity: [u8; SANDBOX_ID_BYTES],
    owner: OwnerIdentity,
    call_result_key: [u8; CALL_RESULT_KEY_BYTES],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnerIdentity {
    pid: u32,
    start: u64,
    boot: [u8; OWNER_BOOT_BYTES],
}

impl Transaction {
    pub(super) fn start(
        lease: Option<Lease>,
        directory: &Path,
        sandbox: SandboxId,
        mode: InvocationMode,
        call_result_key: Option<CallResultKey>,
    ) -> io::Result<Self> {
        Self::start_owned(
            lease,
            directory,
            sandbox,
            Invocation::new(mode, call_result_key)?,
            OwnerIdentity::current()?,
        )
    }

    fn start_owned(
        lease: Option<Lease>,
        directory: &Path,
        sandbox: SandboxId,
        invocation: Invocation,
        owner: OwnerIdentity,
    ) -> io::Result<Self> {
        let path = directory.join("transaction.wal");
        let journal = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        journal.set_permissions(fs::Permissions::from_mode(0o600))?;
        journal.sync_all()?;
        File::open(directory)?.sync_all()?;
        let text = sandbox.to_string();
        let identity: [u8; SANDBOX_ID_BYTES] = text
            .as_bytes()
            .try_into()
            .map_err(|_| invalid("sandbox transaction identity is not canonical"))?;
        let mut transaction = Self {
            _lease: lease,
            journal,
            machine: Machine::new(),
            frame: FrameState {
                sequence: 0,
                previous: [0_u8; 32],
                identity,
                owner,
                call_result_key: invocation.call_result_key,
            },
        };
        transaction.append(Record::Initialized(invocation.mode))?;
        Ok(transaction)
    }

    pub(super) fn append(&mut self, record: Record) -> io::Result<()> {
        let mut next = self.machine.clone();
        next.push(record)?;
        append_frame(&mut self.journal, &mut self.frame, record)?;
        self.machine = next;
        Ok(())
    }

    pub(super) fn finish_abort(&mut self, quarantine: bool) -> io::Result<()> {
        self.begin_abort(quarantine)?;
        self.append(if quarantine {
            Record::Quarantined
        } else {
            Record::RolledBack
        })
    }

    pub(super) fn begin_abort(&mut self, quarantine: bool) -> io::Result<()> {
        self.append(Record::AbortObserved)?;
        if quarantine {
            self.append(Record::QuarantineObserved)?;
        }
        if !self.machine.scope_reaped() {
            if !self.machine.scope_reap_started() {
                self.append(Record::ScopeReapIntent)?;
            }
            self.append(Record::ScopeReapProved)?;
        }
        Ok(())
    }
}

fn append_frame(journal: &mut File, state: &mut FrameState, record: Record) -> io::Result<()> {
    let payload = encode_record(record);
    let next_sequence = state
        .sequence
        .checked_add(1)
        .ok_or_else(|| invalid("sandbox transaction sequence overflow"))?;
    let payload_length = u16::try_from(payload.len())
        .map_err(|_| invalid("sandbox transaction frame exceeds its bound"))?;
    let mut frame = Vec::with_capacity(
        WAL_MAGIC.len()
            + 2
            + 2
            + 8
            + state.previous.len()
            + state.identity.len()
            + OWNER_PID_BYTES
            + OWNER_START_BYTES
            + OWNER_BOOT_BYTES
            + CALL_RESULT_KEY_BYTES
            + payload.len()
            + WAL_CHECKSUM_BYTES,
    );
    frame.extend_from_slice(WAL_MAGIC);
    frame.extend_from_slice(&WAL_VERSION.to_le_bytes());
    frame.extend_from_slice(&payload_length.to_le_bytes());
    frame.extend_from_slice(&next_sequence.to_le_bytes());
    frame.extend_from_slice(&state.previous);
    frame.extend_from_slice(&state.identity);
    frame.extend_from_slice(&state.owner.pid.to_le_bytes());
    frame.extend_from_slice(&state.owner.start.to_le_bytes());
    frame.extend_from_slice(&state.owner.boot);
    frame.extend_from_slice(&state.call_result_key);
    frame.extend_from_slice(&payload);
    let digest: [u8; 32] = Sha256::digest(&frame).into();
    frame.extend_from_slice(&digest);
    journal.write_all(&frame)?;
    journal.sync_all()?;
    state.sequence = next_sequence;
    state.previous = digest;
    Ok(())
}

impl std::fmt::Debug for Transaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transaction")
            .field("sequence", &self.frame.sequence)
            .field("terminal", &self.machine.is_terminal())
            .finish_non_exhaustive()
    }
}

impl OwnerIdentity {
    fn current() -> io::Result<Self> {
        let pid = std::process::id();
        Ok(Self {
            pid,
            start: process_start(pid)?,
            boot: boot_identity()?,
        })
    }

    fn owner_is_dead(self) -> io::Result<bool> {
        if self.boot != boot_identity()? {
            return Ok(true);
        }
        match process_start(self.pid) {
            Ok(start) => Ok(start != self.start),
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(problem) => Err(problem),
        }
    }
}

fn boot_identity() -> io::Result<[u8; OWNER_BOOT_BYTES]> {
    let boot = fs::read("/proc/sys/kernel/random/boot_id")?;
    let boot = boot.strip_suffix(b"\n").unwrap_or(&boot);
    let boot = boot.strip_suffix(b"\r").unwrap_or(boot);
    if boot.len() != SANDBOX_ID_BYTES
        || !boot.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && *byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        })
    {
        return Err(invalid("host boot identity is invalid"));
    }
    let mut digest = Sha256::new();
    digest.update(b"crucible-sandbox-owner-boot-v1\0");
    digest.update(boot);
    Ok(digest.finalize().into())
}

fn process_start(pid: u32) -> io::Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = stat
        .rfind(')')
        .ok_or_else(|| invalid("host process identity record is invalid"))?;
    stat.get(close.saturating_add(1)..)
        .and_then(|fields| fields.split_whitespace().nth(19))
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(|| invalid("host process start identity is invalid"))
}

fn encode_record(record: Record) -> Vec<u8> {
    enum Value {
        Empty,
        Number(u32),
        Receipt([u8; 32]),
    }
    let (kind, value) = match record {
        Record::Initialized(InvocationMode::Foreground) => (1, Value::Number(0)),
        Record::Initialized(InvocationMode::Background) => (1, Value::Number(1)),
        Record::Prepared => (2, Value::Empty),
        Record::ReleaseIntent => (3, Value::Empty),
        Record::OwnerTransferred => (4, Value::Empty),
        Record::GoSentOrAmbiguous => (5, Value::Empty),
        Record::CallAcceptIntent => (6, Value::Empty),
        Record::CallAccepted(receipt) => (7, Value::Receipt(receipt)),
        Record::RefusalObserved => (8, Value::Empty),
        Record::PreparationCleanupIntent => (9, Value::Empty),
        Record::PreparationCleanupProved => (10, Value::Empty),
        Record::PreparationCleanupUnproved => (11, Value::Empty),
        Record::Refused => (12, Value::Empty),
        Record::CommandExited => (13, Value::Empty),
        Record::WorkloadReapIntent => (14, Value::Empty),
        Record::WorkloadReaped => (15, Value::Empty),
        Record::ScanIntent => (16, Value::Empty),
        Record::ScanTransferred => (17, Value::Empty),
        Record::StageIntent(index) => (18, Value::Number(index)),
        Record::Staged(index) => (19, Value::Number(index)),
        Record::PublicationStaged => (20, Value::Empty),
        Record::ScopeReapIntent => (21, Value::Empty),
        Record::ScopeReapProved => (22, Value::Empty),
        Record::ScopeReapUnproved => (23, Value::Empty),
        Record::ApplyIntent(index) => (24, Value::Number(index)),
        Record::Applied(index) => (25, Value::Number(index)),
        Record::AbortObserved => (26, Value::Empty),
        Record::QuarantineObserved => (27, Value::Empty),
        Record::Committed => (28, Value::Empty),
        Record::RolledBack => (29, Value::Empty),
        Record::Quarantined => (30, Value::Empty),
        Record::RollbackIntent(index) => (31, Value::Number(index)),
        Record::RollbackApplied(index) => (32, Value::Number(index)),
        Record::DiscardIntent(index) => (33, Value::Number(index)),
        Record::Discarded(index) => (34, Value::Number(index)),
    };
    let mut payload = vec![kind];
    match value {
        Value::Empty => {}
        Value::Number(value) => payload.extend_from_slice(&value.to_le_bytes()),
        Value::Receipt(receipt) => payload.extend_from_slice(&receipt),
    }
    payload
}

struct Recovered {
    machine: Machine,
    #[cfg(test)]
    records: Vec<Record>,
    #[cfg(test)]
    torn_tail: bool,
    journal: File,
    frame: FrameState,
}

impl Recovered {
    fn append(&mut self, record: Record) -> io::Result<()> {
        let mut next = self.machine.clone();
        next.push(record)?;
        append_frame(&mut self.journal, &mut self.frame, record)?;
        self.machine = next;
        #[cfg(test)]
        self.records.push(record);
        Ok(())
    }
}

#[cfg(test)]
fn recover_wal(path: &Path) -> io::Result<Recovered> {
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    recover_wal_file(File::from(descriptor))
}

fn recover_wal_at(directory: &File) -> io::Result<Recovered> {
    let descriptor = rustix::fs::openat(
        directory,
        "transaction.wal",
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    recover_wal_file(File::from(descriptor))
}

fn recover_wal_file(mut file: File) -> io::Result<Recovered> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.gid() != rustix::process::getgid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > MAX_WAL_BYTES
    {
        return Err(invalid("sandbox transaction journal authority is invalid"));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| invalid("sandbox transaction journal exceeds addressable memory"))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)?;

    let mut offset = 0_usize;
    let mut sequence = 0_u64;
    let mut previous = [0_u8; 32];
    let mut identity: Option<[u8; SANDBOX_ID_BYTES]> = None;
    let mut owner: Option<OwnerIdentity> = None;
    let mut call_result_key: Option<[u8; CALL_RESULT_KEY_BYTES]> = None;
    let mut machine = Machine::new();
    let mut records = Vec::new();
    let mut torn_tail = false;
    while offset < bytes.len() {
        let remaining = bytes.len().saturating_sub(offset);
        if remaining < WAL_PREFIX_BYTES {
            torn_tail = true;
            break;
        }
        let prefix_end = offset
            .checked_add(WAL_PREFIX_BYTES)
            .ok_or_else(|| invalid("sandbox transaction frame offset overflow"))?;
        let prefix = bytes
            .get(offset..prefix_end)
            .ok_or_else(|| invalid("sandbox transaction frame prefix is unavailable"))?;
        if prefix.get(..WAL_MAGIC.len()) != Some(WAL_MAGIC) {
            return Err(invalid("sandbox transaction journal magic is invalid"));
        }
        let version = u16::from_le_bytes(
            prefix
                .get(8..10)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| invalid("sandbox transaction journal version is missing"))?,
        );
        if version != WAL_VERSION {
            return Err(invalid(
                "sandbox transaction journal version is unsupported",
            ));
        }
        let payload_length = usize::from(u16::from_le_bytes(
            prefix
                .get(10..12)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| invalid("sandbox transaction payload length is missing"))?,
        ));
        if payload_length == 0 || payload_length > MAX_WAL_PAYLOAD_BYTES {
            return Err(invalid("sandbox transaction payload length is invalid"));
        }
        let frame_without_checksum = WAL_PREFIX_BYTES
            .checked_add(payload_length)
            .ok_or_else(|| invalid("sandbox transaction frame length overflow"))?;
        let frame_length = frame_without_checksum
            .checked_add(WAL_CHECKSUM_BYTES)
            .ok_or_else(|| invalid("sandbox transaction frame length overflow"))?;
        if remaining < frame_length {
            torn_tail = true;
            break;
        }
        let frame_end = offset
            .checked_add(frame_length)
            .ok_or_else(|| invalid("sandbox transaction frame offset overflow"))?;
        let checksum_start = offset
            .checked_add(frame_without_checksum)
            .ok_or_else(|| invalid("sandbox transaction checksum offset overflow"))?;
        let expected: [u8; 32] = bytes
            .get(checksum_start..frame_end)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| invalid("sandbox transaction checksum is missing"))?;
        let actual: [u8; 32] = Sha256::digest(
            bytes
                .get(offset..checksum_start)
                .ok_or_else(|| invalid("sandbox transaction frame is unavailable"))?,
        )
        .into();
        if expected != actual {
            return Err(invalid("sandbox transaction checksum is invalid"));
        }

        let frame_sequence = u64::from_le_bytes(
            prefix
                .get(12..20)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| invalid("sandbox transaction sequence is missing"))?,
        );
        if frame_sequence != sequence.saturating_add(1) {
            return Err(invalid("sandbox transaction sequence is noncontiguous"));
        }
        if prefix.get(20..52) != Some(previous.as_slice()) {
            return Err(invalid("sandbox transaction digest chain is invalid"));
        }
        let frame_identity: [u8; SANDBOX_ID_BYTES] = prefix
            .get(IDENTITY_OFFSET..OWNER_PID_OFFSET)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| invalid("sandbox transaction identity is missing"))?;
        let identity_text = std::str::from_utf8(&frame_identity)
            .map_err(|_| invalid("sandbox transaction identity is not UTF-8"))?;
        SandboxId::parse(identity_text)
            .map_err(|_| invalid("sandbox transaction identity is not canonical"))?;
        if identity.is_some_and(|known| known != frame_identity) {
            return Err(invalid("sandbox transaction identity changed"));
        }
        identity = Some(frame_identity);
        let frame_owner = OwnerIdentity {
            pid: u32::from_le_bytes(
                prefix
                    .get(OWNER_PID_OFFSET..OWNER_START_OFFSET)
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| invalid("sandbox transaction owner PID is missing"))?,
            ),
            start: u64::from_le_bytes(
                prefix
                    .get(OWNER_START_OFFSET..OWNER_BOOT_OFFSET)
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| invalid("sandbox transaction owner start is missing"))?,
            ),
            boot: prefix
                .get(OWNER_BOOT_OFFSET..CALL_RESULT_KEY_OFFSET)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| invalid("sandbox transaction owner boot is missing"))?,
        };
        if frame_owner.pid == 0 || frame_owner.start == 0 {
            return Err(invalid("sandbox transaction owner identity is invalid"));
        }
        if owner.is_some_and(|known| known != frame_owner) {
            return Err(invalid("sandbox transaction owner identity changed"));
        }
        owner = Some(frame_owner);
        let frame_call_result_key: [u8; CALL_RESULT_KEY_BYTES] = prefix
            .get(CALL_RESULT_KEY_OFFSET..WAL_PREFIX_BYTES)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| invalid("sandbox transaction call-result key is missing"))?;
        if call_result_key.is_some_and(|known| known != frame_call_result_key) {
            return Err(invalid("sandbox transaction call-result key changed"));
        }
        call_result_key = Some(frame_call_result_key);

        let payload = bytes
            .get(prefix_end..checksum_start)
            .ok_or_else(|| invalid("sandbox transaction payload is unavailable"))?;
        let record = decode_record(payload)?;
        machine.push(record)?;
        records.push(record);
        offset = frame_end;
        sequence = frame_sequence;
        previous = actual;
    }
    if records.is_empty() {
        return Err(invalid(
            "sandbox transaction journal has no initialized frame",
        ));
    }
    let recovered_key = call_result_key
        .ok_or_else(|| invalid("sandbox transaction call-result key is unavailable"))?;
    match records.first() {
        Some(Record::Initialized(InvocationMode::Foreground)) if recovered_key == [0_u8; 32] => {}
        Some(Record::Initialized(InvocationMode::Background)) => {}
        _ => {
            return Err(invalid(
                "sandbox transaction call-result key does not match invocation mode",
            ));
        }
    }
    if torn_tail {
        file.set_len(
            u64::try_from(offset)
                .map_err(|_| invalid("sandbox transaction verified length overflow"))?,
        )?;
        file.sync_all()?;
    }
    file.seek(SeekFrom::Start(u64::try_from(offset).map_err(|_| {
        invalid("sandbox transaction verified length overflow")
    })?))?;
    Ok(Recovered {
        machine,
        #[cfg(test)]
        records,
        #[cfg(test)]
        torn_tail,
        journal: file,
        frame: FrameState {
            sequence,
            previous,
            identity: identity
                .ok_or_else(|| invalid("sandbox transaction identity is unavailable"))?,
            owner: owner.ok_or_else(|| invalid("sandbox transaction owner is unavailable"))?,
            call_result_key: recovered_key,
        },
    })
}

fn decode_record(payload: &[u8]) -> io::Result<Record> {
    let kind = payload
        .first()
        .copied()
        .ok_or_else(|| invalid("sandbox transaction record kind is missing"))?;
    let value = || {
        payload
            .get(1..5)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| invalid("sandbox transaction record value is missing"))
    };
    let record = match kind {
        1 if payload.len() == 5 => match value()? {
            0 => Record::Initialized(InvocationMode::Foreground),
            1 => Record::Initialized(InvocationMode::Background),
            _ => return Err(invalid("sandbox transaction invocation mode is invalid")),
        },
        2 if payload.len() == 1 => Record::Prepared,
        3 if payload.len() == 1 => Record::ReleaseIntent,
        4 if payload.len() == 1 => Record::OwnerTransferred,
        5 if payload.len() == 1 => Record::GoSentOrAmbiguous,
        6 if payload.len() == 1 => Record::CallAcceptIntent,
        7 if payload.len() == 33 => Record::CallAccepted(
            payload
                .get(1..33)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| invalid("sandbox call-result receipt is missing"))?,
        ),
        8 if payload.len() == 1 => Record::RefusalObserved,
        9 if payload.len() == 1 => Record::PreparationCleanupIntent,
        10 if payload.len() == 1 => Record::PreparationCleanupProved,
        11 if payload.len() == 1 => Record::PreparationCleanupUnproved,
        12 if payload.len() == 1 => Record::Refused,
        13 if payload.len() == 1 => Record::CommandExited,
        14 if payload.len() == 1 => Record::WorkloadReapIntent,
        15 if payload.len() == 1 => Record::WorkloadReaped,
        16 if payload.len() == 1 => Record::ScanIntent,
        17 if payload.len() == 1 => Record::ScanTransferred,
        18 if payload.len() == 5 => Record::StageIntent(value()?),
        19 if payload.len() == 5 => Record::Staged(value()?),
        20 if payload.len() == 1 => Record::PublicationStaged,
        21 if payload.len() == 1 => Record::ScopeReapIntent,
        22 if payload.len() == 1 => Record::ScopeReapProved,
        23 if payload.len() == 1 => Record::ScopeReapUnproved,
        24 if payload.len() == 5 => Record::ApplyIntent(value()?),
        25 if payload.len() == 5 => Record::Applied(value()?),
        26 if payload.len() == 1 => Record::AbortObserved,
        27 if payload.len() == 1 => Record::QuarantineObserved,
        28 if payload.len() == 1 => Record::Committed,
        29 if payload.len() == 1 => Record::RolledBack,
        30 if payload.len() == 1 => Record::Quarantined,
        31 if payload.len() == 5 => Record::RollbackIntent(value()?),
        32 if payload.len() == 5 => Record::RollbackApplied(value()?),
        33 if payload.len() == 5 => Record::DiscardIntent(value()?),
        34 if payload.len() == 5 => Record::Discarded(value()?),
        _ => {
            return Err(invalid(
                "sandbox transaction record is unknown or malformed",
            ));
        }
    };
    Ok(record)
}

impl Machine {
    pub(super) const fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, record: Record) -> io::Result<()> {
        if self.is_terminal() {
            return Err(invalid("transaction record follows its terminal"));
        }
        if !self.accepts(record) {
            return Err(invalid("transaction record violates the R8 grammar"));
        }
        self.records.push(record);
        Ok(())
    }

    pub(super) fn is_terminal(&self) -> bool {
        matches!(
            self.records.last(),
            Some(Record::Refused | Record::Committed | Record::RolledBack | Record::Quarantined)
        )
    }

    fn terminal(&self) -> Option<Record> {
        self.is_terminal()
            .then(|| self.records.last().copied())
            .flatten()
    }

    fn accepts(&self, next: Record) -> bool {
        let Some(last) = self.records.last().copied() else {
            return matches!(next, Record::Initialized(_));
        };
        let mode = self.records.first().and_then(|record| match record {
            Record::Initialized(mode) => Some(*mode),
            _ => None,
        });
        match last {
            Record::Initialized(_) => matches!(next, Record::Prepared | Record::RefusalObserved),
            Record::Prepared => matches!(next, Record::ReleaseIntent | Record::RefusalObserved),
            Record::ReleaseIntent => match mode {
                Some(InvocationMode::Foreground) => {
                    matches!(next, Record::GoSentOrAmbiguous | Record::RefusalObserved)
                }
                Some(InvocationMode::Background) => {
                    matches!(next, Record::OwnerTransferred | Record::RefusalObserved)
                }
                None => false,
            },
            Record::OwnerTransferred => {
                mode == Some(InvocationMode::Background)
                    && matches!(next, Record::GoSentOrAmbiguous | Record::RefusalObserved)
            }
            Record::GoSentOrAmbiguous => match mode {
                Some(InvocationMode::Foreground) => matches!(
                    next,
                    Record::CommandExited | Record::AbortObserved | Record::QuarantineObserved
                ),
                Some(InvocationMode::Background) => matches!(next, Record::CallAcceptIntent),
                None => false,
            },
            Record::CallAcceptIntent => {
                mode == Some(InvocationMode::Background)
                    && matches!(
                        next,
                        Record::CallAccepted(_)
                            | Record::AbortObserved
                            | Record::QuarantineObserved
                    )
            }
            Record::CallAccepted(_) => matches!(
                next,
                Record::CommandExited | Record::AbortObserved | Record::QuarantineObserved
            ),
            Record::RefusalObserved => {
                matches!(next, Record::Refused | Record::PreparationCleanupIntent)
            }
            Record::PreparationCleanupIntent => matches!(
                next,
                Record::PreparationCleanupProved | Record::PreparationCleanupUnproved
            ),
            Record::PreparationCleanupProved => matches!(next, Record::Refused),
            Record::PreparationCleanupUnproved | Record::ScopeReapUnproved => {
                matches!(next, Record::Quarantined)
            }
            Record::CommandExited => matches!(
                next,
                Record::WorkloadReapIntent | Record::AbortObserved | Record::QuarantineObserved
            ),
            Record::WorkloadReapIntent => matches!(
                next,
                Record::WorkloadReaped | Record::AbortObserved | Record::QuarantineObserved
            ),
            Record::WorkloadReaped => matches!(
                next,
                Record::ScanIntent | Record::AbortObserved | Record::QuarantineObserved
            ),
            Record::ScanIntent => matches!(
                next,
                Record::ScanTransferred | Record::AbortObserved | Record::QuarantineObserved
            ),
            Record::ScanTransferred => matches!(
                next,
                Record::StageIntent(0)
                    | Record::PublicationStaged
                    | Record::AbortObserved
                    | Record::QuarantineObserved
            ),
            Record::StageIntent(index) => {
                matches!(
                    next,
                    Record::Staged(next_index) if next_index == index
                ) || matches!(next, Record::AbortObserved | Record::QuarantineObserved)
            }
            Record::Staged(index) => {
                matches!(
                    next,
                    Record::StageIntent(next_index) if next_index == index.saturating_add(1)
                ) || matches!(
                    next,
                    Record::PublicationStaged | Record::AbortObserved | Record::QuarantineObserved
                )
            }
            Record::PublicationStaged => matches!(
                next,
                Record::ScopeReapIntent | Record::AbortObserved | Record::QuarantineObserved
            ),
            Record::ScopeReapIntent => matches!(
                next,
                Record::ScopeReapProved
                    | Record::ScopeReapUnproved
                    | Record::AbortObserved
                    | Record::QuarantineObserved
            ),
            Record::ScopeReapProved if self.failure_observed() => self.accepts_recovery(next),
            Record::ScopeReapProved => matches!(
                next,
                Record::ApplyIntent(0)
                    | Record::Committed
                    | Record::AbortObserved
                    | Record::QuarantineObserved
            ),
            Record::ApplyIntent(index) => {
                matches!(
                    next,
                    Record::Applied(next_index) if next_index == index
                ) || matches!(next, Record::AbortObserved | Record::QuarantineObserved)
            }
            Record::Applied(index) => {
                matches!(
                    next,
                    Record::ApplyIntent(next_index) if next_index == index.saturating_add(1)
                ) || matches!(
                    next,
                    Record::Committed | Record::AbortObserved | Record::QuarantineObserved
                )
            }
            Record::AbortObserved => {
                if self.scope_reaped() {
                    self.accepts_recovery(next)
                } else if self.scope_reap_started() {
                    matches!(next, Record::ScopeReapProved | Record::ScopeReapUnproved)
                } else {
                    matches!(next, Record::ScopeReapIntent | Record::QuarantineObserved)
                }
            }
            Record::QuarantineObserved => {
                if self.scope_reaped() {
                    matches!(next, Record::Quarantined)
                } else if self.scope_reap_started() {
                    matches!(next, Record::ScopeReapProved | Record::ScopeReapUnproved)
                } else {
                    matches!(next, Record::ScopeReapIntent)
                }
            }
            Record::RollbackIntent(index) => {
                matches!(next, Record::RollbackApplied(next_index) if next_index == index)
                    || matches!(next, Record::QuarantineObserved)
            }
            Record::RollbackApplied(_) | Record::Discarded(_) => self.accepts_recovery(next),
            Record::DiscardIntent(index) => {
                matches!(next, Record::Discarded(next_index) if next_index == index)
                    || matches!(next, Record::QuarantineObserved)
            }
            Record::Refused | Record::Committed | Record::RolledBack | Record::Quarantined => false,
        }
    }

    fn scope_reaped(&self) -> bool {
        self.records.contains(&Record::ScopeReapProved)
    }

    fn scope_reap_started(&self) -> bool {
        self.records.contains(&Record::ScopeReapIntent)
    }

    fn failure_observed(&self) -> bool {
        self.records
            .iter()
            .any(|record| matches!(record, Record::AbortObserved | Record::QuarantineObserved))
    }

    fn accepts_recovery(&self, next: Record) -> bool {
        if self
            .records
            .iter()
            .any(|record| matches!(record, Record::QuarantineObserved))
        {
            return matches!(next, Record::Quarantined);
        }
        if let Some(index) = self.pending_rollback() {
            return matches!(next, Record::RollbackIntent(next_index) if next_index == index)
                || matches!(next, Record::QuarantineObserved);
        }
        if let Some(index) = self.pending_discard() {
            return matches!(next, Record::DiscardIntent(next_index) if next_index == index)
                || matches!(next, Record::QuarantineObserved);
        }
        matches!(next, Record::RolledBack | Record::QuarantineObserved)
    }

    fn pending_rollback(&self) -> Option<u32> {
        self.records.iter().rev().find_map(|record| match record {
            Record::ApplyIntent(index)
                if !self.records.contains(&Record::RollbackApplied(*index)) =>
            {
                Some(*index)
            }
            _ => None,
        })
    }

    fn pending_discard(&self) -> Option<u32> {
        self.records.iter().rev().find_map(|record| match record {
            Record::StageIntent(index) if !self.records.contains(&Record::Discarded(*index)) => {
                Some(*index)
            }
            _ => None,
        })
    }
}

/// The descriptor-held global writable-transaction lock.
pub(super) struct Lease {
    _state: File,
    _lock: File,
    #[cfg(test)]
    test_serial: Option<TestSerialLease>,
}

impl Lease {
    pub(super) fn acquire(request: &SandboxRequest) -> Result<Option<Self>, SandboxError> {
        let writable = request
            .policy()
            .filesystem()
            .iter()
            .any(|rule| rule.access() == SandboxFilesystemAccess::ReadWrite)
            || request
                .manifest()
                .entries()
                .iter()
                .any(|entry| entry.access() == Some(SandboxFilesystemAccess::ReadWrite));
        if !writable {
            return Ok(None);
        }
        #[cfg(test)]
        let test_serial =
            TestSerialLease::acquire().map_err(|_| SandboxError::BackendUnavailable {
                reason: "sandbox test writer coordination is unavailable".into(),
            })?;
        let state = state_directory(request)?;
        let lease = Self::acquire_at(&state).map_err(|source| {
            if source.raw_os_error() == Some(rustix::io::Errno::WOULDBLOCK.raw_os_error()) {
                SandboxError::Concurrency
            } else {
                SandboxError::BackendUnavailable {
                    reason: "writable transaction state or global lock is unavailable".into(),
                }
            }
        })?;
        #[cfg(test)]
        let lease = {
            let mut lease = lease;
            lease.test_serial = Some(test_serial);
            lease
        };
        reconcile_host_transactions().map_err(|_| SandboxError::BackendUnavailable {
            reason: "stale writable transaction requires recovery or quarantine review".into(),
        })?;
        Ok(Some(lease))
    }

    fn acquire_at(path: &Path) -> io::Result<Self> {
        create_state_directory(path)?;
        let state = open_state_directory(path)?;
        let lock = open_lock(&state)?;
        rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        validate_state(path, &state)?;
        validate_lock(path, &lock)?;
        Ok(Self {
            _state: state,
            _lock: lock,
            #[cfg(test)]
            test_serial: None,
        })
    }
}

#[cfg(test)]
struct TestSerialLease {
    held: bool,
}

#[cfg(test)]
static TEST_SERIAL_OWNER: std::sync::LazyLock<(
    std::sync::Mutex<Option<std::thread::ThreadId>>,
    std::sync::Condvar,
)> = std::sync::LazyLock::new(|| (std::sync::Mutex::new(None), std::sync::Condvar::new()));

#[cfg(test)]
impl TestSerialLease {
    fn acquire() -> io::Result<Self> {
        let current = std::thread::current().id();
        let (owner, available) = &*TEST_SERIAL_OWNER;
        let mut owner = owner
            .lock()
            .map_err(|_| invalid("sandbox test writer coordination was poisoned"))?;
        loop {
            match *owner {
                None => {
                    *owner = Some(current);
                    return Ok(Self { held: true });
                }
                Some(active) if active == current => return Ok(Self { held: false }),
                Some(_) => {
                    owner = available
                        .wait(owner)
                        .map_err(|_| invalid("sandbox test writer coordination was poisoned"))?;
                }
            }
        }
    }
}

#[cfg(test)]
impl Drop for TestSerialLease {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let (owner, available) = &*TEST_SERIAL_OWNER;
        if let Ok(mut owner) = owner.lock() {
            *owner = None;
            available.notify_all();
        }
    }
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Lease").finish_non_exhaustive()
    }
}

fn create_state_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(problem) => Err(problem),
    }
}

fn open_state_directory(path: &Path) -> io::Result<File> {
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let state = File::from(descriptor);
    let metadata = state.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.gid() != rustix::process::getgid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(invalid("sandbox state directory authority is invalid"));
    }
    state.sync_all()?;
    Ok(state)
}

fn open_lock(state: &File) -> io::Result<File> {
    let (descriptor, created) = match rustix::fs::openat(
        state,
        "writable.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(descriptor) => (descriptor, true),
        Err(rustix::io::Errno::EXIST) => (
            rustix::fs::openat(
                state,
                "writable.lock",
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?,
            false,
        ),
        Err(problem) => return Err(problem.into()),
    };
    let lock = File::from(descriptor);
    if created {
        use std::os::unix::fs::PermissionsExt as _;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.gid() != rustix::process::getgid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(invalid("sandbox writable lock authority is invalid"));
    }
    if created {
        lock.sync_all()?;
        state.sync_all()?;
    }
    Ok(lock)
}

fn validate_state(path: &Path, state: &File) -> io::Result<()> {
    let named = fs::symlink_metadata(path)?;
    let opened = state.metadata()?;
    if named.dev() != opened.dev() || named.ino() != opened.ino() || !named.is_dir() {
        return Err(invalid("sandbox state directory identity changed"));
    }
    Ok(())
}

fn validate_lock(state_path: &Path, lock: &File) -> io::Result<()> {
    let named = fs::symlink_metadata(state_path.join("writable.lock"))?;
    let opened = lock.metadata()?;
    if named.dev() != opened.dev()
        || named.ino() != opened.ino()
        || named.nlink() != 1
        || !named.is_file()
    {
        return Err(invalid("sandbox writable lock identity changed"));
    }
    Ok(())
}

pub(super) fn reconcile_host_transactions() -> io::Result<()> {
    for base in [Path::new("/var/tmp"), Path::new("/tmp")] {
        let Ok(canonical) = base.canonicalize() else {
            continue;
        };
        if canonical == base {
            reconcile_stale_transactions(base)?;
        }
    }
    Ok(())
}

fn reconcile_stale_transactions(base: &Path) -> io::Result<()> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        if !entry
            .file_name()
            .as_bytes()
            .starts_with(STAGE_PREFIX.as_bytes())
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.gid() != rustix::process::getgid().as_raw()
        {
            continue;
        }
        candidates.push(entry.path());
        if candidates.len() > MAX_STALE_TRANSACTIONS {
            return Err(invalid("stale sandbox transaction count exceeds its bound"));
        }
    }
    candidates.sort();
    let mut removed = false;
    for candidate in candidates {
        let name = candidate
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("stale sandbox transaction name is invalid"))?;
        let sandbox = name
            .strip_prefix(STAGE_PREFIX)
            .ok_or_else(|| invalid("stale sandbox transaction prefix is invalid"))
            .and_then(|identity| {
                SandboxId::parse(identity)
                    .map_err(|_| invalid("stale sandbox transaction identity is invalid"))
            })?;
        let named = fs::symlink_metadata(&candidate)?;
        if !named.is_dir()
            || named.uid() != rustix::process::getuid().as_raw()
            || named.gid() != rustix::process::getgid().as_raw()
            || named.mode() & 0o7777 != 0o700
        {
            return Err(invalid("stale sandbox transaction authority is invalid"));
        }
        let descriptor = rustix::fs::open(
            &candidate,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let directory = File::from(descriptor);
        let opened = directory.metadata()?;
        if opened.dev() != named.dev() || opened.ino() != named.ino() {
            return Err(invalid("stale sandbox transaction identity changed"));
        }
        let mut recovered = recover_wal_at(&directory)?;
        if recovered.frame.identity.as_slice() != sandbox.to_string().as_bytes() {
            return Err(invalid("stale sandbox journal names another transaction"));
        }
        if !recovered.machine.is_terminal() && !recovered.frame.owner.owner_is_dead()? {
            continue;
        }
        if !recovered.machine.is_terminal() {
            recover_stale_transaction(&candidate, &mut recovered)?;
        }
        if !matches!(
            recovered.machine.terminal(),
            Some(Record::Refused | Record::Committed | Record::RolledBack)
        ) {
            return Err(invalid(
                "stale sandbox transaction is nonterminal or quarantined",
            ));
        }
        drop(recovered);
        drop(directory);
        let current = fs::symlink_metadata(&candidate)?;
        if current.dev() != named.dev() || current.ino() != named.ino() || !current.is_dir() {
            return Err(invalid("stale sandbox transaction changed before cleanup"));
        }
        fs::remove_dir_all(&candidate)?;
        if candidate.exists() {
            return Err(invalid("stale sandbox transaction cleanup is incomplete"));
        }
        removed = true;
    }
    if removed {
        File::open(base)?.sync_all()?;
    }
    Ok(())
}

fn recover_stale_transaction(candidate: &Path, recovered: &mut Recovered) -> io::Result<()> {
    if !recovered.frame.owner.owner_is_dead()? {
        return Err(invalid(
            "nonterminal sandbox transaction owner is still alive",
        ));
    }
    if !recovered
        .machine
        .records
        .contains(&Record::GoSentOrAmbiguous)
    {
        return recover_pre_release(recovered);
    }
    recover_post_release(candidate, recovered)
}

fn recover_pre_release(recovered: &mut Recovered) -> io::Result<()> {
    loop {
        let next = match recovered.machine.records.last().copied() {
            Some(
                Record::Initialized(_)
                | Record::Prepared
                | Record::ReleaseIntent
                | Record::OwnerTransferred,
            ) => Record::RefusalObserved,
            Some(Record::RefusalObserved) => Record::PreparationCleanupIntent,
            Some(Record::PreparationCleanupIntent) => Record::PreparationCleanupProved,
            Some(Record::PreparationCleanupProved) => Record::Refused,
            Some(Record::PreparationCleanupUnproved) => Record::Quarantined,
            Some(Record::Refused | Record::Quarantined) => return Ok(()),
            _ => {
                return Err(invalid(
                    "pre-release sandbox transaction has an impossible recovery state",
                ));
            }
        };
        recovered.append(next)?;
    }
}

fn recover_post_release(candidate: &Path, recovered: &mut Recovered) -> io::Result<()> {
    if matches!(
        recovered.machine.records.last(),
        Some(Record::ScopeReapUnproved)
    ) {
        return recovered.append(Record::Quarantined);
    }
    if recovered
        .machine
        .records
        .contains(&Record::QuarantineObserved)
    {
        prove_recovered_scope(recovered)?;
        return recovered.append(Record::Quarantined);
    }
    if !recovered.machine.failure_observed() {
        recovered.append(Record::AbortObserved)?;
    }
    prove_recovered_scope(recovered)?;

    if recovered.machine.pending_rollback().is_some() {
        recovered.append(Record::QuarantineObserved)?;
        return recovered.append(Record::Quarantined);
    }
    if let Err(problem) = discard_recovered_staging(candidate, recovered) {
        let _ = recovered.append(Record::QuarantineObserved);
        let _ = recovered.append(Record::Quarantined);
        return Err(problem);
    }
    recovered.append(Record::RolledBack)
}

fn prove_recovered_scope(recovered: &mut Recovered) -> io::Result<()> {
    if recovered.machine.scope_reaped() {
        return Ok(());
    }
    if !recovered.machine.scope_reap_started() {
        recovered.append(Record::ScopeReapIntent)?;
    }
    recovered.append(Record::ScopeReapProved)
}

fn discard_recovered_staging(candidate: &Path, recovered: &mut Recovered) -> io::Result<()> {
    let publication = candidate.join("publication");
    while let Some(index) = recovered.machine.pending_discard() {
        if !matches!(
            recovered.machine.records.last(),
            Some(Record::DiscardIntent(current)) if *current == index
        ) {
            recovered.append(Record::DiscardIntent(index))?;
        }
        let directory = publication.join(index.to_string());
        match fs::remove_dir_all(&directory) {
            Ok(()) => {}
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => {}
            Err(problem) => return Err(problem),
        }
        if publication.exists() {
            File::open(&publication)?.sync_all()?;
        } else {
            File::open(candidate)?.sync_all()?;
        }
        recovered.append(Record::Discarded(index))?;
    }
    match fs::remove_dir(&publication) {
        Ok(()) => File::open(candidate)?.sync_all(),
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(problem) => Err(problem),
    }
}

fn state_directory(request: &SandboxRequest) -> Result<PathBuf, SandboxError> {
    let base = Path::new("/var/tmp");
    if base.canonicalize().ok().as_deref() != Some(base) {
        return Err(SandboxError::BackendUnavailable {
            reason: "canonical host transaction state base is unavailable".into(),
        });
    }
    let state = base.join(format!(
        "crucible-code-sandbox-{}-v1",
        rustix::process::getuid().as_raw()
    ));
    let overlaps_policy = request
        .policy()
        .filesystem()
        .iter()
        .any(|rule| state.starts_with(rule.path()) || rule.path().starts_with(&state));
    let overlaps_manifest = request.manifest().entries().iter().any(|entry| {
        entry
            .source()
            .is_some_and(|source| state.starts_with(source) || source.starts_with(&state))
    });
    if overlaps_policy || overlaps_manifest {
        return Err(SandboxError::BackendUnavailable {
            reason: "transaction state overlaps the requested sandbox filesystem view".into(),
        });
    }
    Ok(state)
}

fn invalid(problem: &'static str) -> io::Error {
    io::Error::other(problem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::SeekFrom;

    #[test]
    fn r8_background_acceptance_follows_owner_transfer_and_go() {
        let mut machine = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Background),
            Record::Prepared,
            Record::ReleaseIntent,
            Record::OwnerTransferred,
            Record::GoSentOrAmbiguous,
            Record::CallAcceptIntent,
            Record::CallAccepted([0x5a; 32]),
        ] {
            machine.push(record).expect("valid R8 prefix");
        }
        assert!(!machine.is_terminal());
    }

    #[test]
    fn r8_rejects_background_acceptance_before_go_or_without_an_owner() {
        let mut before_go = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Background),
            Record::Prepared,
            Record::ReleaseIntent,
            Record::OwnerTransferred,
        ] {
            before_go.push(record).expect("valid prefix");
        }
        assert!(before_go.push(Record::CallAcceptIntent).is_err());

        let mut no_owner = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Background),
            Record::Prepared,
            Record::ReleaseIntent,
        ] {
            no_owner.push(record).expect("valid prefix");
        }
        assert!(no_owner.push(Record::GoSentOrAmbiguous).is_err());
    }

    #[test]
    fn positive_publication_requires_contiguous_stage_and_apply_records() {
        let mut machine = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Foreground),
            Record::Prepared,
            Record::ReleaseIntent,
            Record::GoSentOrAmbiguous,
            Record::CommandExited,
            Record::WorkloadReapIntent,
            Record::WorkloadReaped,
            Record::ScanIntent,
            Record::ScanTransferred,
            Record::StageIntent(0),
            Record::Staged(0),
            Record::StageIntent(1),
            Record::Staged(1),
            Record::PublicationStaged,
            Record::ScopeReapIntent,
            Record::ScopeReapProved,
            Record::ApplyIntent(0),
            Record::Applied(0),
            Record::ApplyIntent(1),
            Record::Applied(1),
            Record::Committed,
        ] {
            machine.push(record).expect("positive R8 history");
        }
        assert!(machine.is_terminal());
        assert!(machine.push(Record::AbortObserved).is_err());
    }

    #[test]
    fn rollback_requires_reverse_apply_and_stage_resolution() {
        let mut machine = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Foreground),
            Record::Prepared,
            Record::ReleaseIntent,
            Record::GoSentOrAmbiguous,
            Record::CommandExited,
            Record::WorkloadReapIntent,
            Record::WorkloadReaped,
            Record::ScanIntent,
            Record::ScanTransferred,
            Record::StageIntent(0),
            Record::Staged(0),
            Record::PublicationStaged,
            Record::ScopeReapIntent,
            Record::ScopeReapProved,
            Record::ApplyIntent(0),
            Record::AbortObserved,
        ] {
            machine.push(record).expect("abort prefix");
        }
        assert!(machine.push(Record::RolledBack).is_err());
        for record in [
            Record::RollbackIntent(0),
            Record::RollbackApplied(0),
            Record::DiscardIntent(0),
            Record::Discarded(0),
            Record::RolledBack,
        ] {
            machine.push(record).expect("resolved rollback");
        }
        assert!(machine.is_terminal());
    }

    #[test]
    fn proved_pre_release_cleanup_can_refuse_but_unproved_cleanup_quarantines() {
        let mut proved = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Foreground),
            Record::Prepared,
            Record::RefusalObserved,
            Record::PreparationCleanupIntent,
            Record::PreparationCleanupProved,
            Record::Refused,
        ] {
            proved.push(record).expect("proved refusal history");
        }
        assert!(proved.is_terminal());

        let mut unproved = Machine::new();
        for record in [
            Record::Initialized(InvocationMode::Foreground),
            Record::Prepared,
            Record::RefusalObserved,
            Record::PreparationCleanupIntent,
            Record::PreparationCleanupUnproved,
        ] {
            unproved.push(record).expect("unproved cleanup prefix");
        }
        assert!(unproved.push(Record::Refused).is_err());
        unproved
            .push(Record::Quarantined)
            .expect("unproved cleanup quarantines");
    }

    #[test]
    fn writable_lease_is_exclusive_and_released_with_its_descriptor() {
        let sample = crate::sample::Sample::new("sandbox-transaction-lock");
        let state = sample.root().join("state");
        let first = Lease::acquire_at(&state).expect("first lease");
        assert!(Lease::acquire_at(&state).is_err());
        drop(first);
        Lease::acquire_at(&state).expect("lease after descriptor close");
    }

    #[test]
    fn durable_frames_replay_through_the_same_closed_validator() {
        let (sample, journal) = journal_with(&[
            Record::Prepared,
            Record::RefusalObserved,
            Record::PreparationCleanupIntent,
            Record::PreparationCleanupProved,
            Record::Refused,
        ]);
        let recovered = recover_wal(&journal).expect("valid journal");
        assert_eq!(recovered.records.len(), 6);
        assert!(recovered.machine.is_terminal());
        assert!(!recovered.torn_tail);
        drop(sample);
    }

    #[test]
    fn durable_background_frames_bind_the_result_key_and_acceptance_receipt() {
        use std::os::unix::fs::DirBuilderExt as _;

        let sample = crate::sample::Sample::new("sandbox-background-transaction-journal");
        let state_root = sample.root().join("state");
        let projection_root = sample.root().join("stage");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&projection_root).expect("stage directory");
        let lease = Lease::acquire_at(&state_root).expect("transaction lease");
        let key = CallResultKey::from_digest([0x3c; 32]);
        let receipt = [0x5a; 32];
        let mut transaction = Transaction::start(
            Some(lease),
            &projection_root,
            SandboxId::new(),
            InvocationMode::Background,
            Some(key),
        )
        .expect("background transaction journal");
        for record in [
            Record::Prepared,
            Record::ReleaseIntent,
            Record::OwnerTransferred,
            Record::GoSentOrAmbiguous,
            Record::CallAcceptIntent,
            Record::CallAccepted(receipt),
        ] {
            transaction.append(record).expect("journal record");
        }
        drop(transaction);

        let recovered = recover_wal(&projection_root.join("transaction.wal"))
            .expect("valid background journal");
        assert_eq!(recovered.frame.call_result_key, key.bytes());
        assert_eq!(
            recovered.records.last(),
            Some(&Record::CallAccepted(receipt))
        );
    }

    #[test]
    fn invocation_mode_and_durable_result_identity_cannot_disagree() {
        assert!(Invocation::new(InvocationMode::Foreground, None).is_ok());
        assert!(
            Invocation::new(
                InvocationMode::Background,
                Some(CallResultKey::from_digest([1; 32]))
            )
            .is_ok()
        );
        assert!(
            Invocation::new(
                InvocationMode::Foreground,
                Some(CallResultKey::from_digest([2; 32]))
            )
            .is_err()
        );
        assert!(Invocation::new(InvocationMode::Background, None).is_err());
    }

    #[test]
    fn a_truncated_tail_is_removed_to_the_last_verified_frame() {
        let (sample, journal) = journal_with(&[Record::Prepared]);
        let complete = fs::metadata(&journal).expect("journal metadata").len();
        OpenOptions::new()
            .append(true)
            .open(&journal)
            .expect("append journal")
            .write_all(b"partial")
            .expect("torn tail fixture");

        let recovered = recover_wal(&journal).expect("recover torn tail");
        assert!(recovered.torn_tail);
        assert_eq!(
            fs::metadata(&journal).expect("recovered metadata").len(),
            complete
        );
        drop(sample);
    }

    #[test]
    fn recovery_appends_after_the_verified_boundary_of_a_torn_tail() {
        let (sample, journal) = journal_with(&[]);
        OpenOptions::new()
            .append(true)
            .open(&journal)
            .expect("append journal")
            .write_all(b"partial")
            .expect("torn tail fixture");

        let mut recovered = recover_wal(&journal).expect("recover torn tail");
        recovered.append(Record::Prepared).expect("recovery frame");
        drop(recovered);
        let replayed = recover_wal(&journal).expect("replay recovered WAL");
        assert_eq!(
            replayed.records,
            vec![
                Record::Initialized(InvocationMode::Foreground),
                Record::Prepared
            ]
        );
        drop(sample);
    }

    #[test]
    fn boot_pid_and_start_time_distinguish_live_and_dead_owners() {
        assert!(
            !OwnerIdentity::current()
                .expect("current identity")
                .owner_is_dead()
                .expect("live owner check")
        );
        assert!(dead_owner().owner_is_dead().expect("dead owner check"));
    }

    #[test]
    fn checksum_corruption_is_never_treated_as_a_torn_tail() {
        let (sample, journal) = journal_with(&[Record::Prepared]);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&journal)
            .expect("open journal");
        let end = file.seek(SeekFrom::End(-1)).expect("last checksum byte");
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).expect("checksum byte");
        file.seek(SeekFrom::Start(end)).expect("rewind checksum");
        byte[0] ^= 0xff;
        file.write_all(&byte).expect("corrupt checksum");
        file.sync_all().expect("sync corruption");

        assert!(recover_wal(&journal).is_err());
        drop(sample);
    }

    #[test]
    fn terminal_stale_transactions_are_cleaned_idempotently() {
        let sample = crate::sample::Sample::new("sandbox-terminal-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal(&sample, &base, true);

        reconcile_stale_transactions(&base).expect("terminal cleanup");
        assert!(!stage.exists());
        reconcile_stale_transactions(&base).expect("idempotent terminal cleanup");
    }

    #[test]
    fn live_nonterminal_transactions_are_skipped_and_retain_their_evidence() {
        let sample = crate::sample::Sample::new("sandbox-nonterminal-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal(&sample, &base, false);

        reconcile_stale_transactions(&base).expect("live transaction is not stale");
        assert!(stage.join("transaction.wal").exists());
    }

    #[test]
    fn read_only_lifecycles_have_a_durable_transaction_without_a_writer_lease() {
        use std::os::unix::fs::DirBuilderExt as _;

        let sample = crate::sample::Sample::new("sandbox-read-only-transaction");
        let stage = sample.root().join("stage");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&stage).expect("stage directory");
        let mut transaction = Transaction::start(
            None,
            &stage,
            SandboxId::new(),
            InvocationMode::Foreground,
            None,
        )
        .expect("read-only lifecycle journal");
        for record in [
            Record::Prepared,
            Record::RefusalObserved,
            Record::PreparationCleanupIntent,
            Record::PreparationCleanupProved,
            Record::Refused,
        ] {
            transaction.append(record).expect("lifecycle record");
        }
        drop(transaction);

        let recovered = recover_wal(&stage.join("transaction.wal")).expect("read-only WAL");
        assert_eq!(recovered.machine.terminal(), Some(Record::Refused));
    }

    #[test]
    fn dead_pre_release_owners_are_refused_and_cleaned() {
        let sample = crate::sample::Sample::new("sandbox-dead-pre-release-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal_with(&sample, &base, dead_owner(), &[Record::Prepared]);

        reconcile_stale_transactions(&base).expect("pre-release recovery");
        assert!(!stage.exists());
    }

    #[test]
    fn dead_post_release_owners_roll_back_when_no_apply_was_authorized() {
        let sample = crate::sample::Sample::new("sandbox-dead-post-release-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal_with(
            &sample,
            &base,
            dead_owner(),
            &[
                Record::Prepared,
                Record::ReleaseIntent,
                Record::GoSentOrAmbiguous,
            ],
        );

        reconcile_stale_transactions(&base).expect("post-release rollback");
        assert!(!stage.exists());
    }

    #[test]
    fn dead_owners_discard_durable_staging_in_reverse_before_rollback() {
        let sample = crate::sample::Sample::new("sandbox-dead-staging-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal_with(
            &sample,
            &base,
            dead_owner(),
            &[
                Record::Prepared,
                Record::ReleaseIntent,
                Record::GoSentOrAmbiguous,
                Record::CommandExited,
                Record::WorkloadReapIntent,
                Record::WorkloadReaped,
                Record::ScanIntent,
                Record::ScanTransferred,
                Record::StageIntent(0),
                Record::Staged(0),
                Record::StageIntent(1),
                Record::Staged(1),
                Record::PublicationStaged,
                Record::ScopeReapIntent,
                Record::ScopeReapProved,
            ],
        );
        create_private_test_directory(&stage.join("publication"));
        create_private_test_directory(&stage.join("publication/0"));
        create_private_test_directory(&stage.join("publication/1"));

        reconcile_stale_transactions(&base).expect("staging recovery");
        assert!(!stage.exists());
    }

    #[test]
    fn dead_owners_with_an_ambiguous_apply_are_quarantined() {
        let sample = crate::sample::Sample::new("sandbox-dead-apply-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal_with(
            &sample,
            &base,
            dead_owner(),
            &[
                Record::Prepared,
                Record::ReleaseIntent,
                Record::GoSentOrAmbiguous,
                Record::CommandExited,
                Record::WorkloadReapIntent,
                Record::WorkloadReaped,
                Record::ScanIntent,
                Record::ScanTransferred,
                Record::StageIntent(0),
                Record::Staged(0),
                Record::PublicationStaged,
                Record::ScopeReapIntent,
                Record::ScopeReapProved,
                Record::ApplyIntent(0),
            ],
        );
        create_private_test_directory(&stage.join("publication"));
        create_private_test_directory(&stage.join("publication/0"));

        assert!(reconcile_stale_transactions(&base).is_err());
        let recovered = recover_wal(&stage.join("transaction.wal")).expect("quarantine WAL");
        assert_eq!(recovered.machine.terminal(), Some(Record::Quarantined));
        assert!(stage.join("publication/0").exists());
    }

    fn journal_with(records: &[Record]) -> (crate::sample::Sample, PathBuf) {
        use std::os::unix::fs::DirBuilderExt as _;

        let sample = crate::sample::Sample::new("sandbox-transaction-journal");
        let state_root = sample.root().join("state");
        let projection_root = sample.root().join("stage");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&projection_root).expect("stage directory");
        let lease = Lease::acquire_at(&state_root).expect("transaction lease");
        let mut transaction = Transaction::start(
            Some(lease),
            &projection_root,
            SandboxId::new(),
            InvocationMode::Foreground,
            None,
        )
        .expect("transaction journal");
        for record in records {
            transaction.append(*record).expect("journal record");
        }
        drop(transaction);
        let journal = projection_root.join("transaction.wal");
        (sample, journal)
    }

    fn stale_journal(sample: &crate::sample::Sample, base: &Path, terminal: bool) -> PathBuf {
        let mut records = vec![Record::Prepared];
        if terminal {
            records.extend([
                Record::RefusalObserved,
                Record::PreparationCleanupIntent,
                Record::PreparationCleanupProved,
                Record::Refused,
            ]);
        }
        stale_journal_with(
            sample,
            base,
            OwnerIdentity::current().expect("current owner"),
            &records,
        )
    }

    fn stale_journal_with(
        sample: &crate::sample::Sample,
        base: &Path,
        owner: OwnerIdentity,
        records: &[Record],
    ) -> PathBuf {
        let sandbox = SandboxId::new();
        let stage = base.join(format!("crucible-projection-{sandbox}"));
        create_private_test_directory(&stage);
        let lease = Lease::acquire_at(&sample.root().join("state")).expect("transaction lease");
        let mut transaction = Transaction::start_owned(
            Some(lease),
            &stage,
            sandbox,
            Invocation::new(InvocationMode::Foreground, None).expect("foreground identity"),
            owner,
        )
        .expect("transaction journal");
        for record in records {
            transaction.append(*record).expect("transaction record");
        }
        drop(transaction);
        stage
    }

    fn dead_owner() -> OwnerIdentity {
        OwnerIdentity {
            pid: u32::MAX,
            start: 1,
            boot: boot_identity().expect("boot identity"),
        }
    }

    fn create_private_test_directory(path: &Path) {
        use std::os::unix::fs::DirBuilderExt as _;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path).expect("private directory");
    }
}
