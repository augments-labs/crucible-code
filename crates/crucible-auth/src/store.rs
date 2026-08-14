//! The file, and what reading it answers.
//!
//! One file for every provider rather than one each, because a directory
//! listing of `openai.json` beside `moonshot.json` says which providers this
//! user has logged in to without anybody being able to read either.
//!
//! Reading is what this module answers today, and it is the half that has to
//! survive anything: a launch reads the store before it knows whether it needs
//! one, so every shape the file could be in — absent, truncated, half-written,
//! written by a version that does not exist yet — resolves to a list of keys
//! and at most one sentence, never to a stop.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crucible_core::ApiKey;

/// What the file is called, inside the home directory.
const FILE: &str = "auth.json";

/// What this version of crucible writes, and the highest it can read.
///
/// A number rather than a guess at the shape, so a file from a version that
/// does not exist yet is a case this one can recognise and decline instead of
/// a parse failure it would report as damage.
const VERSION: u64 = 1;

/// Where the keys crucible was given are written down.
#[derive(Debug, Clone)]
pub struct Store {
    /// The file itself.
    path: PathBuf,
}

impl Store {
    /// The store inside `home`, whether or not anything is there yet.
    ///
    /// A path rather than a lookup: `crucible_config::Home` is the one place
    /// that answers where crucible's files are, and a second answer here is a
    /// bug that only shows up on somebody else's machine.
    #[must_use]
    pub fn in_home(home: &Path) -> Self {
        Self {
            path: home.join(FILE),
        }
    }

    /// Every key the store holds.
    ///
    /// Infallible on purpose. Absent, unreadable, or written by a version that
    /// does not exist yet all mean the same thing to a launch — nobody is
    /// logged in — and most launches need no stored key at all. What could not
    /// be done comes back in [`Keys::trouble`] for the user to be told once.
    #[must_use]
    pub fn read(&self) -> Keys {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(trouble) if trouble.kind() == std::io::ErrorKind::NotFound => {
                return Keys::default();
            }
            // Anything else — a directory in the way, a permission denied — is
            // reported rather than passed off as "never logged in".
            Err(trouble) => {
                return Keys::nothing(&format!("{FILE} could not be opened: {trouble}"));
            }
        };

        match parse(&text) {
            Ok(keys) => Keys {
                keys,
                trouble: None,
            },
            Err(said) => Keys::nothing(&said),
        }
    }
}

/// The keys the store held, and anything that has to be said about reading it.
#[derive(Default)]
pub struct Keys {
    /// Provider name to the key written down for it.
    keys: BTreeMap<String, String>,
    /// What reading could not do, in a sentence for the user.
    trouble: Option<Box<str>>,
}

impl Keys {
    /// No keys, and a reason.
    fn nothing(said: &str) -> Self {
        Self {
            keys: BTreeMap::new(),
            trouble: Some(said.into()),
        }
    }

    /// `provider`'s key, as the type that can be applied and not read.
    #[must_use]
    pub fn get(&self, provider: &str) -> Option<ApiKey> {
        self.keys.get(provider).map(ApiKey::new)
    }

    /// Every provider with a key here, in name order.
    pub fn providers(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    /// What reading the store could not do, once, for the user to be told.
    #[must_use]
    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }
}

/// Written by hand: the derived one would print every key it holds.
impl std::fmt::Debug for Keys {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Keys")
            .field("providers", &self.keys.keys())
            .field("trouble", &self.trouble)
            .finish()
    }
}

/// The stored map, or a sentence saying why there is none.
///
/// Walked rather than derived onto a mirror struct, the way every other
/// boundary in this workspace reads JSON — and here it also means a key is
/// never handed to a derive that could put it in a `Debug`.
fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let document: serde_json::Value =
        serde_json::from_str(text).map_err(|why| format!("{FILE} could not be read: {why}"))?;

    match document.get("version").and_then(serde_json::Value::as_u64) {
        Some(VERSION) => {}
        Some(later) => {
            return Err(format!(
                "{FILE} was written by a later version of crucible (version {later}), so nobody is logged in here"
            ));
        }
        None => return Err(format!("{FILE} does not say which version wrote it")),
    }

    let Some(keys) = document.get("keys").and_then(serde_json::Value::as_object) else {
        return Err(format!("{FILE} holds no keys"));
    };

    Ok(keys
        .iter()
        .filter_map(|(provider, key)| Some((provider.clone(), key.as_str()?.to_owned())))
        .collect())
}

#[cfg(test)]
mod tests;
