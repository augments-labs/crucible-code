//! Durable writable-transaction admission and the closed R8 lifecycle grammar.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crucible_core::{SandboxError, SandboxFilesystemAccess, SandboxId, SandboxRequest};
use rustix::fs::{FlockOperation, Mode, OFlags};
use sha2::{Digest as _, Sha256};

const WAL_MAGIC: &[u8; 8] = b"CRSBWAL1";
const WAL_VERSION: u16 = 1;
const SANDBOX_ID_BYTES: usize = 36;
const WAL_PREFIX_BYTES: usize = 8 + 2 + 2 + 8 + 32 + SANDBOX_ID_BYTES;
const WAL_CHECKSUM_BYTES: usize = 32;
const MAX_WAL_PAYLOAD_BYTES: usize = 5;
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
    CallAccepted,
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
    _lease: Lease,
    journal: File,
    machine: Machine,
    sequence: u64,
    previous: [u8; 32],
    identity: [u8; SANDBOX_ID_BYTES],
}

impl Transaction {
    pub(super) fn start(
        lease: Lease,
        directory: &Path,
        sandbox: SandboxId,
        mode: InvocationMode,
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
            sequence: 0,
            previous: [0_u8; 32],
            identity,
        };
        transaction.append(Record::Initialized(mode))?;
        Ok(transaction)
    }

    pub(super) fn append(&mut self, record: Record) -> io::Result<()> {
        let mut next = self.machine.clone();
        next.push(record)?;
        let payload = encode_record(record);
        let sequence = self
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
                + self.previous.len()
                + self.identity.len()
                + payload.len()
                + 32,
        );
        frame.extend_from_slice(WAL_MAGIC);
        frame.extend_from_slice(&WAL_VERSION.to_le_bytes());
        frame.extend_from_slice(&payload_length.to_le_bytes());
        frame.extend_from_slice(&sequence.to_le_bytes());
        frame.extend_from_slice(&self.previous);
        frame.extend_from_slice(&self.identity);
        frame.extend_from_slice(&payload);
        let digest: [u8; 32] = Sha256::digest(&frame).into();
        frame.extend_from_slice(&digest);
        self.journal.write_all(&frame)?;
        self.journal.sync_all()?;
        self.machine = next;
        self.sequence = sequence;
        self.previous = digest;
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

impl std::fmt::Debug for Transaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transaction")
            .field("sequence", &self.sequence)
            .field("terminal", &self.machine.is_terminal())
            .finish_non_exhaustive()
    }
}

fn encode_record(record: Record) -> Vec<u8> {
    let (kind, value) = match record {
        Record::Initialized(InvocationMode::Foreground) => (1, Some(0)),
        Record::Initialized(InvocationMode::Background) => (1, Some(1)),
        Record::Prepared => (2, None),
        Record::ReleaseIntent => (3, None),
        Record::OwnerTransferred => (4, None),
        Record::GoSentOrAmbiguous => (5, None),
        Record::CallAcceptIntent => (6, None),
        Record::CallAccepted => (7, None),
        Record::RefusalObserved => (8, None),
        Record::PreparationCleanupIntent => (9, None),
        Record::PreparationCleanupProved => (10, None),
        Record::PreparationCleanupUnproved => (11, None),
        Record::Refused => (12, None),
        Record::CommandExited => (13, None),
        Record::WorkloadReapIntent => (14, None),
        Record::WorkloadReaped => (15, None),
        Record::ScanIntent => (16, None),
        Record::ScanTransferred => (17, None),
        Record::StageIntent(index) => (18, Some(index)),
        Record::Staged(index) => (19, Some(index)),
        Record::PublicationStaged => (20, None),
        Record::ScopeReapIntent => (21, None),
        Record::ScopeReapProved => (22, None),
        Record::ScopeReapUnproved => (23, None),
        Record::ApplyIntent(index) => (24, Some(index)),
        Record::Applied(index) => (25, Some(index)),
        Record::AbortObserved => (26, None),
        Record::QuarantineObserved => (27, None),
        Record::Committed => (28, None),
        Record::RolledBack => (29, None),
        Record::Quarantined => (30, None),
        Record::RollbackIntent(index) => (31, Some(index)),
        Record::RollbackApplied(index) => (32, Some(index)),
        Record::DiscardIntent(index) => (33, Some(index)),
        Record::Discarded(index) => (34, Some(index)),
    };
    let mut payload = vec![kind];
    if let Some(value) = value {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload
}

struct Recovered {
    machine: Machine,
    #[cfg(test)]
    records: Vec<Record>,
    #[cfg(test)]
    torn_tail: bool,
    identity: [u8; SANDBOX_ID_BYTES],
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
            .get(52..WAL_PREFIX_BYTES)
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
    if torn_tail {
        file.set_len(
            u64::try_from(offset)
                .map_err(|_| invalid("sandbox transaction verified length overflow"))?,
        )?;
        file.sync_all()?;
    }
    Ok(Recovered {
        machine,
        #[cfg(test)]
        records,
        #[cfg(test)]
        torn_tail,
        identity: identity.ok_or_else(|| invalid("sandbox transaction identity is unavailable"))?,
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
        7 if payload.len() == 1 => Record::CallAccepted,
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
                mode == Some(InvocationMode::Background) && matches!(next, Record::CallAccepted)
            }
            Record::CallAccepted => matches!(
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
        reconcile_stale_transactions(Path::new("/var/tmp")).map_err(|_| {
            SandboxError::BackendUnavailable {
                reason: "stale writable transaction requires recovery or quarantine review".into(),
            }
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
        let recovered = recover_wal_at(&directory)?;
        if recovered.identity.as_slice() != sandbox.to_string().as_bytes() {
            return Err(invalid("stale sandbox journal names another transaction"));
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
    use std::io::{Seek as _, SeekFrom};

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
            Record::CallAccepted,
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
    fn nonterminal_stale_transactions_block_and_retain_their_evidence() {
        let sample = crate::sample::Sample::new("sandbox-nonterminal-recovery");
        let base = sample.root().join("recovery");
        create_private_test_directory(&base);
        let stage = stale_journal(&sample, &base, false);

        assert!(reconcile_stale_transactions(&base).is_err());
        assert!(stage.join("transaction.wal").exists());
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
            lease,
            &projection_root,
            SandboxId::new(),
            InvocationMode::Foreground,
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
        let sandbox = SandboxId::new();
        let stage = base.join(format!("crucible-projection-{sandbox}"));
        create_private_test_directory(&stage);
        let lease = Lease::acquire_at(&sample.root().join("state")).expect("transaction lease");
        let mut transaction =
            Transaction::start(lease, &stage, sandbox, InvocationMode::Foreground)
                .expect("transaction journal");
        transaction.append(Record::Prepared).expect("prepared");
        if terminal {
            for record in [
                Record::RefusalObserved,
                Record::PreparationCleanupIntent,
                Record::PreparationCleanupProved,
                Record::Refused,
            ] {
                transaction.append(record).expect("terminal record");
            }
        }
        drop(transaction);
        stage
    }

    fn create_private_test_directory(path: &Path) {
        use std::os::unix::fs::DirBuilderExt as _;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path).expect("private directory");
    }
}
