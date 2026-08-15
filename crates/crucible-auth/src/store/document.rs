//! The versioned JSON shape of the credential store.
//!
//! This boundary is walked by hand. In particular, no secret-bearing value
//! derives `Debug` or `Deserialize`, so a future parse error cannot acquire a
//! token merely because serde included the rejected value in its path.

use std::collections::BTreeMap;

use crate::renewable::Tokens;

use super::{FILE, VERSION};

const MAX_DETAILS: usize = 16;
const MAX_DETAIL_NAME: usize = 64;
const MAX_DETAIL_VALUE: usize = 4 * 1024;

/// Every credential kind the current store holds.
#[derive(Default)]
pub(super) struct Document {
    pub(super) keys: BTreeMap<String, String>,
    pub(super) subscriptions: BTreeMap<String, Tokens>,
    pub(super) identities: BTreeMap<String, String>,
}

/// The stored document, or a sentence saying why there is none.
pub(super) fn parse(text: &str) -> Result<Document, ParseError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(ParseError::Malformed)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ParseError::Unversioned)?;
    if version > VERSION {
        return Err(ParseError::Later { later: version });
    }
    if !matches!(version, 1 | VERSION) {
        return Err(ParseError::Unsupported { version });
    }

    let keys = text_map(&value, "keys")?;
    let subscriptions = if version == 1 {
        BTreeMap::new()
    } else {
        subscriptions(&value)?
    };
    let identities = if version == 1 || value.get("identities").is_none() {
        BTreeMap::new()
    } else {
        text_map(&value, "identities")?
    };
    if let Some(provider) = keys.keys().find(|name| subscriptions.contains_key(*name)) {
        return Err(ParseError::Duplicate {
            provider: provider.clone(),
        });
    }
    Ok(Document {
        keys,
        subscriptions,
        identities,
    })
}

fn text_map(
    value: &serde_json::Value,
    field: &'static str,
) -> Result<BTreeMap<String, String>, ParseError> {
    let object = value
        .get(field)
        .and_then(serde_json::Value::as_object)
        .ok_or(ParseError::NoMap { field })?;
    object
        .iter()
        .map(|(provider, value)| {
            value
                .as_str()
                .filter(|secret| !secret.is_empty())
                .map(|secret| (provider.clone(), secret.to_owned()))
                .ok_or_else(|| ParseError::NonText {
                    field,
                    provider: provider.clone(),
                })
        })
        .collect()
}

fn subscriptions(value: &serde_json::Value) -> Result<BTreeMap<String, Tokens>, ParseError> {
    let object = value
        .get("subscriptions")
        .and_then(serde_json::Value::as_object)
        .ok_or(ParseError::NoMap {
            field: "subscriptions",
        })?;
    object
        .iter()
        .map(|(provider, value)| Ok((provider.clone(), tokens(value, provider)?)))
        .collect()
}

fn tokens(value: &serde_json::Value, provider: &str) -> Result<Tokens, ParseError> {
    let object = value
        .as_object()
        .ok_or_else(|| ParseError::InvalidSubscription {
            provider: provider.to_owned(),
        })?;
    let secret = |field| {
        object
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|secret| !secret.is_empty() && secret.len() <= 32 * 1024)
            .map(Box::from)
            .ok_or_else(|| ParseError::InvalidSubscription {
                provider: provider.to_owned(),
            })
    };
    let number = |field| {
        object
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ParseError::InvalidSubscription {
                provider: provider.to_owned(),
            })
    };
    let mut details = details(object.get("details"), provider)?;

    // Development builds briefly wrote these OpenAI-specific fields before
    // the provider-neutral v2 document settled. Accept them so an upgrade does
    // not strand that credential; the next write drops the unused ID token.
    if let Some(value) = object.get("id_token") {
        value
            .as_str()
            .filter(|secret| !secret.is_empty() && secret.len() <= 32 * 1024)
            .ok_or_else(|| ParseError::InvalidSubscription {
                provider: provider.to_owned(),
            })?;
    }
    if let Some(value) = object.get("account_id") {
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::String(account)
                if !account.is_empty() && account.len() <= MAX_DETAIL_VALUE =>
            {
                details
                    .entry("account_id".to_owned())
                    .or_insert_with(|| account.clone());
            }
            _ => {
                return Err(ParseError::InvalidSubscription {
                    provider: provider.to_owned(),
                });
            }
        }
    }

    let mut tokens = Tokens::new(
        secret("access_token")?,
        secret("refresh_token")?,
        number("expires_at")?,
        number("refreshed_at")?,
    );
    tokens.replace_details(details);
    Ok(tokens)
}

fn details(
    value: Option<&serde_json::Value>,
    provider: &str,
) -> Result<BTreeMap<String, String>, ParseError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let object = value
        .as_object()
        .filter(|object| object.len() <= MAX_DETAILS)
        .ok_or_else(|| ParseError::InvalidSubscription {
            provider: provider.to_owned(),
        })?;
    object
        .iter()
        .map(|(name, value)| {
            let detail = value
                .as_str()
                .filter(|detail| !detail.is_empty() && detail.len() <= MAX_DETAIL_VALUE)
                .ok_or_else(|| ParseError::InvalidSubscription {
                    provider: provider.to_owned(),
                })?;
            if name.is_empty() || name.len() > MAX_DETAIL_NAME {
                return Err(ParseError::InvalidSubscription {
                    provider: provider.to_owned(),
                });
            }
            Ok((name.clone(), detail.to_owned()))
        })
        .collect()
}

/// The file's whole text.
pub(super) fn render(document: &Document) -> String {
    let keys: serde_json::Map<_, _> = document
        .keys
        .iter()
        .map(|(provider, key)| (provider.clone(), serde_json::Value::from(key.as_str())))
        .collect();
    let subscriptions: serde_json::Map<_, _> = document
        .subscriptions
        .iter()
        .map(|(provider, tokens)| (provider.clone(), token_value(tokens)))
        .collect();
    let identities: serde_json::Map<_, _> = document
        .identities
        .iter()
        .map(|(provider, identity)| (provider.clone(), serde_json::Value::from(identity.as_str())))
        .collect();
    serde_json::Value::from(serde_json::Map::from_iter([
        ("version".to_owned(), serde_json::Value::from(VERSION)),
        ("keys".to_owned(), serde_json::Value::from(keys)),
        (
            "subscriptions".to_owned(),
            serde_json::Value::from(subscriptions),
        ),
        ("identities".to_owned(), serde_json::Value::from(identities)),
    ]))
    .to_string()
}

fn token_value(tokens: &Tokens) -> serde_json::Value {
    let (expires_at, refreshed_at) = tokens.times();
    let details: serde_json::Map<_, _> = tokens
        .details()
        .iter()
        .map(|(name, value)| (name.clone(), serde_json::Value::from(value.as_str())))
        .collect();
    serde_json::Value::from(serde_json::Map::from_iter([
        (
            "access_token".to_owned(),
            serde_json::Value::from(tokens.access()),
        ),
        (
            "refresh_token".to_owned(),
            serde_json::Value::from(tokens.refresh()),
        ),
        ("details".to_owned(), serde_json::Value::from(details)),
        ("expires_at".to_owned(), serde_json::Value::from(expires_at)),
        (
            "refreshed_at".to_owned(),
            serde_json::Value::from(refreshed_at),
        ),
    ]))
}

/// Why the complete document could not become a complete credential map.
#[derive(Debug, thiserror::Error)]
pub(super) enum ParseError {
    #[error("{FILE} could not be read: {0}")]
    Malformed(serde_json::Error),
    #[error(
        "{FILE} was written by a later version of crucible (version {later}), so no stored credential is used"
    )]
    Later { later: u64 },
    #[error("{FILE} uses unsupported version {version}")]
    Unsupported { version: u64 },
    #[error("{FILE} does not say which version wrote it")]
    Unversioned,
    #[error("{FILE} holds no {field} map")]
    NoMap { field: &'static str },
    #[error("{FILE} holds a non-text {field} credential for {provider}")]
    NonText {
        field: &'static str,
        provider: String,
    },
    #[error("{FILE} holds an invalid subscription for {provider}")]
    InvalidSubscription { provider: String },
    #[error("{FILE} holds both an API key and a subscription for {provider}")]
    Duplicate { provider: String },
}
