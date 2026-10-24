//! The signed catalog of agent releases crucible's own review admitted, read
//! and believed.
//!
//! The catalog is hostile text until its detached minisign signature verifies
//! under one of the public keys compiled into the binary, two of them so that
//! one can be rotated out; until that happens nothing in it is parsed. It is
//! read only when it is at most [`INDEX_BYTES`], so a catalog is bounded before
//! a byte of it is kept, and only under the one [`SCHEMA`] this reader knows.
//! A catalog fetched from the network must also carry a serial at least the one
//! already held, so an older catalog cannot be replayed over a newer one, and
//! must not have passed its `expires` by more than [`GRACE`], the allowance
//! for a clock that runs behind. The snapshot compiled into the binary is
//! checked without either, because it is the floor every fetched catalog is
//! measured against and never expires.
//!
//! The text is JSON: `schema`, `serial`, `signed_at` and `expires` in whole
//! seconds since the Unix epoch, `entries` and `revoked`. A key this reader
//! does not know is passed over, so a later catalog can add one without a new
//! schema; a change in what a key means is a new schema. The entries are
//! parsed into owned values once, here, and every refusal names the field it
//! is about. This module does no network work and decides no consent.
//!
//! What a user is offered of it is [`Trust`]'s to decide, from the highest
//! serial believed, every revocation taken in, the catalog most lately
//! believed and the snapshot [`SNAPSHOT`] holds. The snapshot is checked by
//! [`Catalog::compiled`] once there are keys to check it under; until then it
//! is built in from the one release 0.48 shipped with, and not signed.
//!
//! No key a test verifies under is written anywhere: a test signs its catalog
//! with [`fixture`], whose key is minted in memory once per process, and which
//! is compiled only under the `proof` feature that `[dev-dependencies]` alone
//! turn on.

mod entry;
#[cfg(feature = "proof")]
pub mod fixture;
mod snapshot;
mod trust;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use minisign_verify::{PublicKey, Signature};
use serde_json::Value;

use entry::Fields;

pub use snapshot::SNAPSHOT;
pub use trust::{OwnedPin, Trust, TrustError};

pub use entry::{
    ASSET_BYTES, Admits, Artifact, AssetRef, AuthMethod, ContentUse, Disposition, Entry, File,
    FileKind, Files, Format, Kind, LISTED_FILES, Launch, Lock, Method, Revoked, Route, Script,
    Target, UNPACK_RULES, VERSION_BYTES, Variable,
};

/// The largest catalog read, in bytes.
pub const INDEX_BYTES: usize = 1 << 20;

/// The largest detached signature read, in bytes.
pub const SIGNATURE_BYTES: usize = 4096;

/// The one catalog schema this reader knows.
pub const SCHEMA: u64 = 1;

/// The most of an unknown schema a refusal repeats, in characters.
const SCHEMA_SHOWN: usize = 32;

/// How long after its `expires` a fetched catalog is still believed.
pub const GRACE: Duration = Duration::from_hours(7 * 24);

/// The public keys a published catalog is signed with.
///
/// None yet: until the catalog's owner publishes theirs, every fetched catalog
/// is refused, so no key that is not the owner's is ever trusted.
pub const KEYS: Keys = Keys::new(&[]);

/// The public keys a catalog may be signed with, each as the base64 line of a
/// minisign public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keys(&'static [&'static str]);

impl Keys {
    /// The keys `keys` names.
    #[must_use]
    pub const fn new(keys: &'static [&'static str]) -> Self {
        Self(keys)
    }
}

/// The serial of the newest catalog already believed, below which a fetched
/// catalog is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Held(pub u64);

/// A catalog whose signature verified, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    serial: u64,
    signed_at: SystemTime,
    expires: SystemTime,
    entries: Vec<Entry>,
    revoked: Vec<Revoked>,
}

/// Why a catalog was not believed.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// It is larger than [`INDEX_BYTES`].
    #[error("the catalog is {0} bytes, more than the {INDEX_BYTES} one may be")]
    Size(usize),
    /// Its signature does not verify under any key it may be signed with.
    #[error("the catalog's signature does not verify under a key it may be signed with")]
    Signature,
    /// It is not JSON.
    #[error("the catalog is not JSON")]
    Json(#[from] serde_json::Error),
    /// It is written in a schema this reader does not know, given cut short.
    #[error("the catalog is in schema {0}, and this crucible reads schema {SCHEMA}")]
    Schema(Box<str>),
    /// Its serial is below the one already held.
    #[error("the catalog's serial {offered} is older than the {held} already held")]
    Serial {
        /// The catalog's.
        offered: u64,
        /// The one held.
        held: u64,
    },
    /// It expired longer ago than [`GRACE`].
    #[error("the catalog expired more than seven days ago")]
    Expired,
    /// It sets a variable a loader reads to choose what code runs.
    #[error("{field} sets {name}, which decides what code a loader runs")]
    Loader {
        /// Where.
        field: Box<str>,
        /// The variable.
        name: Box<str>,
    },
    /// A field is missing or not what it must be.
    #[error("{field} {why}")]
    Field {
        /// Which.
        field: Box<str>,
        /// What is wrong with it.
        why: &'static str,
    },
}

impl Catalog {
    /// A fetched catalog, believed: `index` with its detached minisign
    /// `signature`, verified under `keys`, at a serial at least `held`, and
    /// not expired longer ago than [`GRACE`] at `now`.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] naming the first check the catalog fails.
    pub fn verify(
        index: &[u8],
        signature: &[u8],
        keys: &Keys,
        held: Held,
        now: SystemTime,
    ) -> Result<Self, CatalogError> {
        let document = signed(index, signature, keys)?;
        let fields = header(&document)?;
        let serial = fields.number("serial")?;
        if serial < held.0 {
            return Err(CatalogError::Serial {
                offered: serial,
                held: held.0,
            });
        }
        let expires = time(&fields, "expires")?;
        if expires.checked_add(GRACE).is_some_and(|last| now > last) {
            return Err(CatalogError::Expired);
        }
        Self::read(&fields)
    }

    /// The snapshot compiled into the binary, believed: `index` with its
    /// detached minisign `signature`, verified under `keys`, with no serial
    /// to meet and no expiry, since it is the floor itself.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] naming the first check the catalog fails.
    pub fn compiled(index: &[u8], signature: &[u8], keys: &Keys) -> Result<Self, CatalogError> {
        let document = signed(index, signature, keys)?;
        Self::read(&header(&document)?)
    }

    /// The catalog whose top-level object is `fields`.
    fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        Ok(Self {
            serial: fields.number("serial")?,
            signed_at: time(fields, "signed_at")?,
            expires: time(fields, "expires")?,
            entries: fields
                .list("entries")?
                .iter()
                .map(Entry::read)
                .collect::<Result<_, _>>()?,
            revoked: fields
                .list("revoked")?
                .iter()
                .map(Revoked::read)
                .collect::<Result<_, _>>()?,
        })
    }

    /// Its serial, which only rises.
    #[must_use]
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// When it was signed.
    #[must_use]
    pub fn signed_at(&self) -> SystemTime {
        self.signed_at
    }

    /// When it expires.
    #[must_use]
    pub fn expires(&self) -> SystemTime {
        self.expires
    }

    /// The releases it offers.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The releases it withdraws.
    #[must_use]
    pub fn revoked(&self) -> &[Revoked] {
        &self.revoked
    }
}

/// The JSON `index` is, once it is within [`INDEX_BYTES`] and `signature`
/// verifies it under one of `keys`; not a byte of it is parsed before.
fn signed(index: &[u8], signature: &[u8], keys: &Keys) -> Result<Value, CatalogError> {
    if index.len() > INDEX_BYTES {
        return Err(CatalogError::Size(index.len()));
    }
    if signature.len() > SIGNATURE_BYTES {
        return Err(CatalogError::Signature);
    }
    let signature = std::str::from_utf8(signature)
        .ok()
        .and_then(|text| Signature::decode(text).ok())
        .ok_or(CatalogError::Signature)?;
    // The digest is all a minisign signature made today signs; the legacy form
    // that signs the bytes themselves is refused.
    let verified = keys.0.iter().any(|key| {
        PublicKey::from_base64(key).is_ok_and(|key| key.verify(index, &signature, false).is_ok())
    });
    if !verified {
        return Err(CatalogError::Signature);
    }
    Ok(serde_json::from_slice(index)?)
}

/// The top-level object of `document`, once its schema is the one this reader
/// knows; nothing else in it is read before.
fn header(document: &Value) -> Result<Fields<'_>, CatalogError> {
    let fields = Fields::new(document, String::new())?;
    match fields.field("schema") {
        Some(schema) if schema.as_u64() == Some(SCHEMA) => Ok(fields),
        Some(schema) => Err(CatalogError::Schema(
            schema.to_string().chars().take(SCHEMA_SHOWN).collect(),
        )),
        None => Err(CatalogError::Schema("none".into())),
    }
}

/// The instant field `name` gives in seconds since the Unix epoch.
fn time(fields: &Fields<'_>, name: &str) -> Result<SystemTime, CatalogError> {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(fields.number(name)?))
        .ok_or_else(|| fields.wrong(name, "is not a time this machine can hold"))
}

#[cfg(all(test, feature = "proof"))]
mod tests;
