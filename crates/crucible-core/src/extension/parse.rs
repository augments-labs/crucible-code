//! Turning a manifest file's bytes into the inert record.
//!
//! The one place an extension's own words are read, and the reason the record
//! above is inert: parsing is the whole of what happens to a discovered
//! extension until somebody decides to trust it. Nothing here opens a file,
//! resolves an entrypoint or starts a process.
//!
//! Every refusal names what is wrong in the manifest rather than reporting that
//! it is invalid, because the person who has to fix it is the extension's
//! author reading their own file. A spelling crucible does not recognise is
//! refused rather than dropped: an extension whose capability line was
//! misspelled would otherwise be granted nothing, start, and fail somewhere
//! else entirely.

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::registry::SourceKind;

use super::{
    EXTENSION_MANIFEST_BYTES, ExtensionCapability, ExtensionContribution, ExtensionError,
    ExtensionIdentity, ExtensionManifest, ExtensionProtocol, ExtensionRequests,
};

/// Every key a manifest may carry.
///
/// An unrecognised one is refused rather than ignored. `capabilties` accepted
/// and skipped is an extension that asks for nothing, starts, and is refused
/// its first registration with nothing pointing at the typo.
const KEYS: &[&str] = &[
    "id",
    "version",
    "protocol",
    "entrypoint",
    "minimumCrucible",
    "capabilities",
    "contributions",
];

/// Reads one manifest.
pub(super) fn manifest(text: &str, found: SourceKind) -> Result<ExtensionManifest, ExtensionError> {
    if text.len() > EXTENSION_MANIFEST_BYTES {
        return Err(ExtensionError::TooLong {
            field: "the manifest",
            maximum: EXTENSION_MANIFEST_BYTES,
            actual: text.len(),
        });
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ExtensionError::Malformed {
        line: source.line(),
        column: source.column(),
        problem: without_position(&source.to_string()).into(),
    })?;
    let Some(object) = value.as_object() else {
        return Err(ExtensionError::WrongType {
            field: "the manifest",
            wanted: "an object",
        });
    };

    for key in object.keys() {
        if !KEYS.contains(&key.as_str()) {
            return Err(ExtensionError::UnknownKey {
                key: key.as_str().into(),
                accepted: KEYS,
            });
        }
    }

    let identity = ExtensionIdentity {
        id: spelling(object, "id")?,
        version: spelling(object, "version")?,
        entrypoint: spelling(object, "entrypoint")?,
        // Taken over the bytes as they were read rather than read out of them.
        // A manifest that stated its own digest would be a file asserting it
        // had not changed since somebody trusted it.
        digest: digest(text),
        found,
    };
    let requests = ExtensionRequests {
        protocol: protocol(object)?,
        minimum: spelling(object, "minimumCrucible")?,
        capabilities: listed(object, "capabilities", ExtensionCapability::named)?,
        contributions: listed(object, "contributions", ExtensionContribution::named)?,
    };

    ExtensionManifest::read(identity, requests)
}

/// One required string.
fn spelling(object: &Map<String, Value>, field: &'static str) -> Result<Box<str>, ExtensionError> {
    let held = object
        .get(field)
        .ok_or(ExtensionError::Missing { field })?
        .as_str()
        .ok_or(ExtensionError::WrongType {
            field,
            wanted: "a string",
        })?;
    Ok(held.into())
}

/// The protocol version, written the way its author would write it.
///
/// `"1.3"` rather than an object with two numbers: it is a version, and a
/// version is something people write on one line. A patch level is refused
/// rather than ignored, because a wire protocol that needs one has changed its
/// shape and the third number would be saying something this build cannot act
/// on.
fn protocol(object: &Map<String, Value>) -> Result<ExtensionProtocol, ExtensionError> {
    let written = spelling(object, "protocol")?;
    let halves = written.split_once('.').and_then(|(major, minor)| {
        Some(ExtensionProtocol::new(
            major.parse().ok()?,
            minor.parse().ok()?,
        ))
    });
    halves.ok_or(ExtensionError::BadProtocol { found: written })
}

/// One optional list of spellings crucible fixed the meaning of.
///
/// Absent is empty, which is the manifest that asks for nothing or promises
/// nothing — both legal. A spelling this build does not know is refused and
/// named, so an extension written against a later crucible is told which word
/// this one could not read rather than quietly getting less than it asked for.
fn listed<T>(
    object: &Map<String, Value>,
    field: &'static str,
    named: fn(&str) -> Option<T>,
) -> Result<Box<[T]>, ExtensionError> {
    let Some(held) = object.get(field) else {
        return Ok(Box::new([]));
    };
    let items = held.as_array().ok_or(ExtensionError::WrongType {
        field,
        wanted: "a list of strings",
    })?;

    items
        .iter()
        .map(|item| {
            let name = item.as_str().ok_or(ExtensionError::WrongType {
                field,
                wanted: "a list of strings",
            })?;
            named(name).ok_or_else(|| ExtensionError::Unrecognised {
                field,
                name: name.into(),
            })
        })
        .collect()
}

/// The bytes as they were read, as a spelling a trust decision can be filed
/// under.
fn digest(text: &str) -> Box<str> {
    let mut digest = Sha256::new();
    digest.update(b"crucible.extension-manifest.v1");
    digest.update(text.as_bytes());

    let mut written = String::from("sha256:");
    for byte in digest.finalize() {
        written.push(nibble(byte >> 4));
        written.push(nibble(byte & 0x0f));
    }
    written.into()
}

/// One half of a byte, as the character that spells it.
const fn nibble(half: u8) -> char {
    match half {
        0..=9 => (b'0' + half) as char,
        _ => (b'a' + half - 10) as char,
    }
}

/// What the JSON parser said, without the position crucible states itself.
///
/// The parser ends its sentence with ` at line N column M`, and the refusal
/// this becomes has already given both numbers in crucible's own words. Both,
/// punctuated differently, is the reader's first clue that nobody read the
/// message.
fn without_position(said: &str) -> &str {
    said.rsplit_once(" at line ")
        .map_or(said, |(problem, _)| problem)
}

#[cfg(test)]
mod tests;
