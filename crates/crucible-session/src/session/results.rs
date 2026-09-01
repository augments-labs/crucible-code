//! Durable, create-once call results beside an append-only session log.
//!
//! A background sandbox may cross its caller-visible acceptance boundary only
//! after its result is durable. The ordinary log writer is asynchronous, so
//! that boundary cannot be the queue. One owner-private sidecar per
//! source-qualified call gives the sandbox a synchronous `put_if_absent`: an
//! identical retry gets the original receipt, while different content under
//! the same key is refused. Replay later folds the record into the ordinary
//! transcript and removes the sidecar only after that transcript line is
//! durable.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use crucible_core::{
    CallResultKey, CallResultReceipt, CallResultStoreError, MAX_RUN_ITEM_BYTES, ToolResult,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{privacy, wire};

const RECEIPT_DOMAIN: &[u8] = b"crucible:session-call-result:v1\0";
const RESULT_VERSION: u32 = 1;
const MAX_RESULT_FILES: usize = 4_096;

pub(super) struct StoredResult {
    pub(super) key: CallResultKey,
    pub(super) receipt: CallResultReceipt,
    pub(super) result: ToolResult,
    pub(super) path: PathBuf,
}

pub(super) fn put(
    log: &Path,
    key: CallResultKey,
    result: &ToolResult,
) -> Result<CallResultReceipt, CallResultStoreError> {
    let directory = result_directory(log).ok_or(CallResultStoreError::Invalid)?;
    prepare_directory(&directory).map_err(|_| CallResultStoreError::Storage)?;
    if entries(&directory).map_err(|_| CallResultStoreError::Storage)? >= MAX_RESULT_FILES {
        return Err(CallResultStoreError::Storage);
    }

    let (line, receipt) = encode(key, result).ok_or(CallResultStoreError::Invalid)?;
    let path = directory.join(format!("{}.json", hex(&key.bytes())));
    match privacy::fresh(&path) {
        Ok(mut file) => {
            file.write_all(line.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .and_then(|()| file.sync_all())
                .map_err(|_| CallResultStoreError::Storage)?;
            sync_directory(&directory).map_err(|_| CallResultStoreError::Storage)?;
            Ok(receipt)
        }
        Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {
            let existing = read_one(&path).map_err(|_| CallResultStoreError::Conflict)?;
            if existing.key == key && existing.receipt == receipt {
                Ok(receipt)
            } else {
                Err(CallResultStoreError::Conflict)
            }
        }
        Err(_) => Err(CallResultStoreError::Storage),
    }
}

pub(super) fn load(log: &Path) -> Result<Vec<StoredResult>, io::Error> {
    let Some(directory) = result_directory(log) else {
        return Ok(Vec::new());
    };
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(problem) => return Err(problem),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "call-result path is not a private directory",
        ));
    }

    let mut stored = Vec::new();
    for entry in fs::read_dir(&directory)? {
        if stored.len() == MAX_RESULT_FILES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many durable call-result records",
            ));
        }
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(invalid_record());
        };
        if Path::new(name).extension() != Some(OsStr::new("json"))
            || name.len() != 64 + ".json".len()
        {
            return Err(invalid_record());
        }
        let record = read_one(&path)?;
        if name != format!("{}.json", hex(&record.key.bytes())) {
            return Err(invalid_record());
        }
        stored.push(record);
    }
    stored.sort_unstable_by_key(|record| record.key.bytes());
    Ok(stored)
}

/// Removes records only after their ordinary transcript line has been synced.
pub(super) fn settle(records: Vec<StoredResult>) -> Result<(), io::Error> {
    let Some(directory) = records
        .first()
        .and_then(|record| record.path.parent())
        .map(Path::to_owned)
    else {
        return Ok(());
    };
    if records
        .iter()
        .any(|record| record.path.parent() != Some(directory.as_path()))
    {
        return Err(invalid_record());
    }
    for record in records {
        fs::remove_file(record.path)?;
    }
    sync_directory(&directory)?;

    if fs::read_dir(&directory)?.next().is_none() {
        fs::remove_dir(&directory)?;
        if let Some(parent) = directory.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

fn encode(key: CallResultKey, result: &ToolResult) -> Option<(String, CallResultReceipt)> {
    let canonical = json!({
        "key": hex(&key.bytes()),
        "result": wire::answered(result),
        "version": RESULT_VERSION,
    });
    let canonical = canonical.to_string();
    let receipt = receipt(canonical.as_bytes());
    let line = json!({
        "call_result": {
            "key": hex(&key.bytes()),
            "receipt": hex(&receipt.bytes()),
            "result": wire::answered(result),
            "version": RESULT_VERSION,
        }
    })
    .to_string();
    (line.len() <= MAX_RUN_ITEM_BYTES).then_some((line, receipt))
}

fn decode(line: &str, path: PathBuf) -> Option<StoredResult> {
    let value: Value = serde_json::from_str(line).ok()?;
    let outer = value.as_object()?;
    if outer.len() != 1 {
        return None;
    }
    let body = outer.get("call_result")?.as_object()?;
    if body.len() != 4 || body.get("version")?.as_u64()? != u64::from(RESULT_VERSION) {
        return None;
    }
    let key = CallResultKey::from_digest(wire::hash(body.get("key")?)?);
    let claimed = CallResultReceipt::from_digest(wire::hash(body.get("receipt")?)?);
    let result = wire::result(body.get("result")?)?;
    let canonical = json!({
        "key": hex(&key.bytes()),
        "result": wire::answered(&result),
        "version": RESULT_VERSION,
    })
    .to_string();
    let actual = receipt(canonical.as_bytes());
    (claimed == actual).then_some(StoredResult {
        key,
        receipt: claimed,
        result,
        path,
    })
}

fn read_one(path: &Path) -> Result<StoredResult, io::Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_RUN_ITEM_BYTES as u64 + 1
    {
        return Err(invalid_record());
    }
    let mut line = String::new();
    File::open(path)?
        .take(MAX_RUN_ITEM_BYTES as u64 + 2)
        .read_to_string(&mut line)?;
    let line = line.strip_suffix('\n').ok_or_else(invalid_record)?;
    if line.contains('\n') {
        return Err(invalid_record());
    }
    decode(line, path.to_owned()).ok_or_else(invalid_record)
}

fn receipt(canonical: &[u8]) -> CallResultReceipt {
    let mut digest = Sha256::new();
    digest.update(RECEIPT_DOMAIN);
    digest.update(canonical);
    CallResultReceipt::from_digest(digest.finalize().into())
}

fn result_directory(log: &Path) -> Option<PathBuf> {
    let stem = log.file_stem()?.to_str()?;
    let parent = log.parent()?;
    Some(parent.join(format!("{stem}.results")))
}

fn prepare_directory(path: &Path) -> Result<(), io::Error> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(invalid_record());
    }
    privacy::directory(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(invalid_record())
    }
}

fn entries(path: &Path) -> Result<usize, io::Error> {
    let mut count = 0_usize;
    for entry in fs::read_dir(path)? {
        entry?;
        count = count.saturating_add(1);
        if count >= MAX_RESULT_FILES {
            break;
        }
    }
    Ok(count)
}

fn sync_directory(path: &Path) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn invalid_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid durable call-result record",
    )
}
