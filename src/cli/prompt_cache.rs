//! Private user-home metadata for persistent provider prompt-cache resources.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_core::{
    MAX_PROMPT_CACHE_RESOURCES, PromptCacheFingerprint, PromptCacheIsolation,
    PromptCachePolicyDigest, PromptCacheResourceBinding, PromptCacheResourceError,
    PromptCacheResourceHandle, PromptCacheResourceId, PromptCacheResourceOperation,
    PromptCacheResourceOwner, PromptCacheResourceRecord, PromptCacheResourceState,
    PromptCacheResourceStore, PromptCacheScopeDigest,
};
use serde_json::{Map, Value, json};

/// Directory created only after persistent-resource policy authorizes a write.
const DIRECTORY: &str = "prompt-cache";
/// Versioned whole-file metadata name.
const FILE: &str = "resources-v1.json";
/// Concurrent mutations contend on this private identity.
const LOCK: &str = "resources.lock";
/// Bounded even if every record carries its maximum provider handle.
const MAX_FILE_BYTES: usize = 256 * 1024;
/// Current private metadata format.
const FORMAT: u64 = 1;

/// Lazy file-backed resource metadata under Crucible's resolved user home.
#[derive(Debug)]
pub(crate) struct MetadataStore {
    directory: PathBuf,
    file: PathBuf,
}

impl MetadataStore {
    /// Names the store without touching the filesystem.
    #[must_use]
    pub(crate) fn in_home(home: &Path) -> Self {
        let directory = home.join(DIRECTORY);
        let file = directory.join(FILE);
        Self { directory, file }
    }

    fn with_lock<T>(
        &self,
        change: impl FnOnce(&Self) -> Result<T, PromptCacheResourceError>,
    ) -> Result<T, PromptCacheResourceError> {
        crucible_privacy::directory(&self.directory)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(|source| local("create the private directory", source))?;
        let lock = crucible_privacy::lock(&self.directory.join(LOCK))
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(|source| local("open the metadata lock", source))?;
        if !crucible_privacy::try_lock_identity(&lock)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(|source| local("lock the metadata store", source))?
        {
            return Err(local(
                "lock the metadata store",
                io::Error::new(io::ErrorKind::WouldBlock, "metadata is busy"),
            ));
        }
        let held = Held(lock);
        let answer = change(self);
        drop(held);
        answer
    }

    fn read(&self) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        match crucible_privacy::tighten(&self.file) {
            Ok(_) => {}
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(problem) => return Err(local("protect the metadata file", problem.into_io())),
        }
        let opened = crucible_privacy::open_read(&self.file)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(|source| local("open the metadata file", source))?;
        let mut bytes = Vec::new();
        opened
            .take(u64::try_from(MAX_FILE_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .map_err(|source| local("read the metadata file", source))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(PromptCacheResourceError::InvalidMetadata);
        }
        decode(&bytes)
    }

    fn replace(
        &self,
        records: &[PromptCacheResourceRecord],
    ) -> Result<(), PromptCacheResourceError> {
        let bytes = encode(records)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(PromptCacheResourceError::StoreFull);
        }
        let mut beside = Temporary::new(&self.directory)?;
        let file = beside
            .file
            .as_mut()
            .ok_or(PromptCacheResourceError::InvalidMetadata)?;
        file.write_all(&bytes)
            .map_err(|source| local("write replacement metadata", source))?;
        file.sync_all()
            .map_err(|source| local("sync replacement metadata", source))?;
        drop(beside.file.take());
        crucible_privacy::replace(&beside.path, &self.file)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(|source| local("replace the metadata file", source))?;
        beside.landed = true;
        Ok(())
    }
}

impl PromptCacheResourceStore for MetadataStore {
    fn matching(
        &mut self,
        binding: &PromptCacheResourceBinding,
    ) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError> {
        let records = self.read()?;
        Ok(records
            .into_iter()
            .rev()
            .find(|record| record.binding() == binding))
    }

    fn put(&mut self, record: &PromptCacheResourceRecord) -> Result<(), PromptCacheResourceError> {
        self.with_lock(|store| {
            let mut records = store.read()?;
            match records.iter().position(|found| found.id() == record.id()) {
                Some(index) => {
                    let slot = records
                        .get_mut(index)
                        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
                    *slot = record.clone();
                }
                None if records.len() < MAX_PROMPT_CACHE_RESOURCES => records.push(record.clone()),
                None => return Err(PromptCacheResourceError::StoreFull),
            }
            store.replace(&records)
        })
    }

    fn remove(&mut self, id: &PromptCacheResourceId) -> Result<(), PromptCacheResourceError> {
        self.with_lock(|store| {
            let mut records = store.read()?;
            records.retain(|record| record.id() != id);
            store.replace(&records)
        })
    }

    fn inspect(
        &mut self,
        maximum: usize,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        let mut records = self.read()?;
        records.truncate(maximum.min(MAX_PROMPT_CACHE_RESOURCES));
        Ok(records)
    }
}

/// Releases an operating-system identity lock on every return path.
struct Held(File);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = crucible_privacy::unlock_identity(&self.0);
    }
}

/// One owner-only whole-file replacement beside the destination.
#[derive(Debug)]
struct Temporary {
    path: PathBuf,
    file: Option<File>,
    landed: bool,
}

impl Temporary {
    fn new(directory: &Path) -> Result<Self, PromptCacheResourceError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let next = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                ".resources.{}.{}.writing",
                std::process::id(),
                next
            ));
            match crucible_privacy::create_write(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        landed: false,
                    });
                }
                Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
                Err(problem) => {
                    return Err(local("create replacement metadata", problem.into_io()));
                }
            }
        }
        Err(local(
            "create replacement metadata",
            io::Error::new(io::ErrorKind::AlreadyExists, "no free sibling name"),
        ))
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.landed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn encode(records: &[PromptCacheResourceRecord]) -> Result<Vec<u8>, PromptCacheResourceError> {
    if records.len() > MAX_PROMPT_CACHE_RESOURCES {
        return Err(PromptCacheResourceError::StoreFull);
    }
    let records = records.iter().map(encode_record).collect::<Vec<_>>();
    serde_json::to_vec(&json!({ "format": FORMAT, "resources": records }))
        .map_err(|_| PromptCacheResourceError::InvalidMetadata)
}

fn encode_record(record: &PromptCacheResourceRecord) -> Value {
    let binding = record.binding();
    json!({
        "id": record.id().as_str(),
        "scope": hex(binding.scope().bytes()),
        "providerScope": hex(binding.provider_scope().bytes()),
        "ownerScope": hex(binding.owner_scope().bytes()),
        "prefix": hex(binding.prefix().bytes()),
        "policy": hex(binding.policy().bytes()),
        "owner": binding.owner().isolation().as_str(),
        "exclusive": binding.owner().exclusive(),
        "protocol": binding.protocol(),
        "model": binding.model(),
        "revision": binding.revision(),
        "handle": record.handle().map(PromptCacheResourceHandle::expose),
        "state": record.state().as_str(),
        "pending": record.pending().map(PromptCacheResourceOperation::as_str),
        "createdAt": record.created_at(),
        "expiresAt": record.expires_at(),
        "lastReconciledAt": record.last_reconciled_at(),
    })
}

fn decode(bytes: &[u8]) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
    let root = value
        .as_object()
        .filter(|root| exact(root, &["format", "resources"]))
        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
    if root.get("format").and_then(Value::as_u64) != Some(FORMAT) {
        return Err(PromptCacheResourceError::InvalidMetadata);
    }
    let values = root
        .get("resources")
        .and_then(Value::as_array)
        .filter(|values| values.len() <= MAX_PROMPT_CACHE_RESOURCES)
        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
    let records = values
        .iter()
        .map(decode_record)
        .collect::<Result<Vec<_>, _>>()?;
    for (index, record) in records.iter().enumerate() {
        if records
            .get(..index)
            .unwrap_or_default()
            .iter()
            .any(|earlier| earlier.id() == record.id())
        {
            return Err(PromptCacheResourceError::InvalidMetadata);
        }
    }
    Ok(records)
}

fn decode_record(value: &Value) -> Result<PromptCacheResourceRecord, PromptCacheResourceError> {
    const KEYS: &[&str] = &[
        "id",
        "scope",
        "providerScope",
        "ownerScope",
        "prefix",
        "policy",
        "owner",
        "exclusive",
        "protocol",
        "model",
        "revision",
        "handle",
        "state",
        "pending",
        "createdAt",
        "expiresAt",
        "lastReconciledAt",
    ];
    let record = value
        .as_object()
        .filter(|record| exact(record, KEYS))
        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
    let text = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .ok_or(PromptCacheResourceError::InvalidMetadata)
    };
    let optional_text = |key: &str| match record.get(key) {
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or(PromptCacheResourceError::InvalidMetadata),
        None => Err(PromptCacheResourceError::InvalidMetadata),
    };
    let optional_u64 = |key: &str| match record.get(key) {
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or(PromptCacheResourceError::InvalidMetadata),
        None => Err(PromptCacheResourceError::InvalidMetadata),
    };
    let id = PromptCacheResourceId::parse(text("id")?)
        .map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
    let scope = PromptCacheScopeDigest::new(unhex(text("scope")?)?);
    let provider_scope = PromptCacheScopeDigest::new(unhex(text("providerScope")?)?);
    let owner_scope = PromptCacheScopeDigest::new(unhex(text("ownerScope")?)?);
    let prefix = PromptCacheFingerprint::new(unhex(text("prefix")?)?);
    let policy = PromptCachePolicyDigest::new(unhex(text("policy")?)?);
    let isolation = PromptCacheIsolation::from_str(text("owner")?)
        .map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
    let exclusive = record
        .get("exclusive")
        .and_then(Value::as_bool)
        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
    let binding = PromptCacheResourceBinding::new(
        scope,
        provider_scope,
        owner_scope,
        prefix,
        policy,
        PromptCacheResourceOwner::new(isolation, exclusive),
        text("protocol")?,
        text("model")?,
        optional_text("revision")?,
    )
    .map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
    let handle = optional_text("handle")?
        .map(PromptCacheResourceHandle::new)
        .transpose()
        .map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
    let state = PromptCacheResourceState::parse(text("state")?)
        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
    let pending = optional_text("pending")?
        .map(|value| {
            PromptCacheResourceOperation::parse(value)
                .ok_or(PromptCacheResourceError::InvalidMetadata)
        })
        .transpose()?;
    let created_at = record
        .get("createdAt")
        .and_then(Value::as_u64)
        .ok_or(PromptCacheResourceError::InvalidMetadata)?;
    let expires_at = optional_u64("expiresAt")?;
    let last_reconciled_at = optional_u64("lastReconciledAt")?;
    PromptCacheResourceRecord::restored(
        id,
        binding,
        handle,
        state,
        pending,
        created_at,
        expires_at,
        last_reconciled_at,
    )
}

fn exact(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn hex(bytes: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(hex_digit(byte >> 4)));
        encoded.push(char::from(hex_digit(byte & 0x0f)));
    }
    encoded
}

const fn hex_digit(value: u8) -> u8 {
    if value < 10 {
        b'0' + value
    } else {
        b'a' + value - 10
    }
}

fn unhex(value: &str) -> Result<[u8; 32], PromptCacheResourceError> {
    if value.len() != 64 {
        return Err(PromptCacheResourceError::InvalidMetadata);
    }
    let mut decoded = [0; 32];
    for (slot, pair) in decoded.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let [high, low] = pair else {
            return Err(PromptCacheResourceError::InvalidMetadata);
        };
        let high = nybble(*high).ok_or(PromptCacheResourceError::InvalidMetadata)?;
        let low = nybble(*low).ok_or(PromptCacheResourceError::InvalidMetadata)?;
        *slot = high << 4 | low;
    }
    Ok(decoded)
}

const fn nybble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn local(operation: &'static str, source: io::Error) -> PromptCacheResourceError {
    PromptCacheResourceError::Local { operation, source }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crucible_core::{
        PromptCacheFingerprint, PromptCacheIsolation, PromptCachePolicyDigest,
        PromptCacheResourceBinding, PromptCacheResourceHandle, PromptCacheResourceId,
        PromptCacheResourceOwner, PromptCacheResourceRecord, PromptCacheResourceState,
        PromptCacheResourceStore, PromptCacheScopeDigest,
    };

    use super::{MetadataStore, decode, encode};
    use crate::cli::sample::Sample;

    fn record() -> PromptCacheResourceRecord {
        let binding = PromptCacheResourceBinding::new(
            PromptCacheScopeDigest::new([1; 32]),
            PromptCacheScopeDigest::new([4; 32]),
            PromptCacheScopeDigest::new([5; 32]),
            PromptCacheFingerprint::new([2; 32]),
            PromptCachePolicyDigest::new([3; 32]),
            PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
            "fixture",
            "model-a",
            Some("revision-a"),
        )
        .unwrap();
        let mut record =
            PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), binding, 100);
        record.ready(
            PromptCacheResourceHandle::new("provider-handle").unwrap(),
            200,
            110,
        );
        record
    }

    #[test]
    fn construction_is_lazy_and_an_authorized_write_is_private_and_round_trips() {
        let sample = Sample::new("prompt-cache-store");
        let mut store = MetadataStore::in_home(&sample.home());
        let directory = sample.home().join("prompt-cache");
        assert!(!directory.exists());

        let record = record();
        store.put(&record).unwrap();
        let read = store.inspect(10).unwrap();

        let Some(read) = read.first() else {
            panic!("written metadata record must round-trip");
        };
        assert_eq!(read.id(), record.id());
        assert_eq!(read.state(), PromptCacheResourceState::Ready);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(directory.join("resources-v1.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn reopening_selects_the_newest_record_for_an_exact_binding() {
        let sample = Sample::new("prompt-cache-restart-selection");
        let mut first_process = MetadataStore::in_home(&sample.home());
        let older = record();
        let mut newer = PromptCacheResourceRecord::creating(
            PromptCacheResourceId::new(),
            older.binding().clone(),
            120,
        );
        newer.ready(
            PromptCacheResourceHandle::new("new-provider-handle").unwrap(),
            300,
            130,
        );
        first_process.put(&older).unwrap();
        first_process.put(&newer).unwrap();
        drop(first_process);

        let mut restarted = MetadataStore::in_home(&sample.home());
        let selected = restarted.matching(older.binding()).unwrap().unwrap();

        assert_eq!(selected.id(), newer.id());
        assert_eq!(selected.state(), PromptCacheResourceState::Ready);
    }

    #[test]
    fn the_metadata_shape_has_no_place_for_prompt_credentials_or_provider_responses() {
        let sample = Sample::new("prompt-cache-private-shape");
        let mut store = MetadataStore::in_home(&sample.home());
        store.put(&record()).unwrap();

        let text = fs::read_to_string(sample.home().join("prompt-cache").join("resources-v1.json"))
            .unwrap();
        for absent in ["prompt-canary", "credential-canary", "response-canary"] {
            assert!(!text.contains(absent), "{text}");
        }
    }

    #[test]
    fn impossible_lifecycle_state_and_pending_operation_pairs_are_rejected() {
        let encoded = encode(&[record()]).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let invalid = [
            ("ready", Some("delete")),
            ("creating", None),
            ("creating", Some("create")),
            ("expiring", None),
            ("deleting", Some("renew")),
            ("deleted", Some("delete")),
            ("expired", Some("renew")),
            ("ambiguous", None),
            ("orphaned", Some("create")),
        ];

        for (state, pending) in invalid {
            let mut candidate = original.clone();
            let resource = candidate
                .get_mut("resources")
                .and_then(serde_json::Value::as_array_mut)
                .and_then(|resources| resources.first_mut())
                .and_then(serde_json::Value::as_object_mut)
                .unwrap();
            resource.insert("state".into(), serde_json::Value::String(state.into()));
            resource.insert(
                "pending".into(),
                pending.map_or(serde_json::Value::Null, |operation| {
                    serde_json::Value::String(operation.into())
                }),
            );
            let bytes = serde_json::to_vec(&candidate).unwrap();
            assert!(decode(&bytes).is_err(), "accepted {state}/{pending:?}");
        }
    }
}
