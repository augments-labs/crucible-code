//! The versioned JSON shape of the API-key store.
//!
//! This boundary is walked by hand. In particular, no secret-bearing value
//! derives `Debug` or `Deserialize`, so a future parse error cannot acquire a
//! key merely because serde included the rejected value in its path.

use std::collections::BTreeMap;

use super::FILE;

/// What this version of Crucible writes, and the highest it can read.
const VERSION: u64 = 1;

/// The stored map, or a typed reason why there is none.
pub(super) fn parse(text: &str) -> Result<BTreeMap<String, String>, ParseError> {
    let document: serde_json::Value = serde_json::from_str(text).map_err(ParseError::Malformed)?;
    let version = document
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ParseError::Unversioned)?;
    if version != VERSION {
        return Err(ParseError::Unsupported { version });
    }

    let keys = document
        .get("keys")
        .and_then(serde_json::Value::as_object)
        .ok_or(ParseError::NoKeys)?;
    keys.iter()
        .map(|(provider, value)| {
            value
                .as_str()
                .filter(|key| !key.is_empty())
                .map(|key| (provider.clone(), key.to_owned()))
                .ok_or_else(|| ParseError::NonText {
                    provider: provider.clone(),
                })
        })
        .collect()
}

/// The file's whole text.
pub(super) fn render(keys: &BTreeMap<String, String>) -> String {
    let keys: serde_json::Map<_, _> = keys
        .iter()
        .map(|(provider, key)| (provider.clone(), serde_json::Value::from(key.as_str())))
        .collect();

    serde_json::Value::from(serde_json::Map::from_iter([
        ("version".to_owned(), serde_json::Value::from(VERSION)),
        ("keys".to_owned(), serde_json::Value::from(keys)),
    ]))
    .to_string()
}

/// Why the complete document could not become a complete key map.
#[derive(Debug, thiserror::Error)]
pub(super) enum ParseError {
    #[error("{FILE} could not be read: {0}")]
    Malformed(serde_json::Error),
    #[error("{FILE} uses unsupported version {version}")]
    Unsupported { version: u64 },
    #[error("{FILE} does not say which version wrote it")]
    Unversioned,
    #[error("{FILE} holds no keys map")]
    NoKeys,
    #[error("{FILE} holds a non-text key for {provider}")]
    NonText { provider: String },
}
