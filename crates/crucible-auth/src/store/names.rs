//! The names a provider's credential is written under.
//!
//! A provider with one row writes its credential under its own name, as every
//! release has. One with more writes each row's under a name of its own: the
//! bare provider name for the row 0.43.3 also knows, so a roll back finds its
//! credential where it left it, and the provider's name, `@`, and what tells
//! the row apart for the rest.
//!
//! The store is told the names and knows no row. It removes a provider's other
//! names when it writes one, so a provider holds one credential, and leaves
//! every name it was not told exactly where it is, since a later release may
//! have written it.
//!
//! An agent's key, typed for one of the sign-in methods it offers, is written
//! under the agent's name, `@`, and the method's id ([`signing_in`]), and that
//! name never changes once written. The store reads everything before a
//! name's `@` as its provider, so such a name is never one it is told: the
//! caller refuses an agent whose name is a provider's before it writes or
//! reads one, and no write of a provider's ever takes an agent's key out.

use std::collections::BTreeMap;
use std::sync::Arc;

/// The provider a stored name belongs to: everything before its `@`.
#[must_use]
pub fn provider_of(name: &str) -> &str {
    name.split_once('@').map_or(name, |(provider, _)| provider)
}

/// The most bytes the id of a sign-in method may have in the name a key is
/// written under.
pub const METHOD_BYTES: usize = 64;

/// The name the key `agent` signs in with by the method `method` is written
/// under, `<agent>@<method>`; nothing where either cannot be part of a name
/// that is never to change: an agent's name that is empty or holds `@`, or a
/// method's id that is empty, longer than [`METHOD_BYTES`], or holds anything
/// but ASCII letters, digits, `-`, `_` and `.`.
///
/// Whether `agent` is the name of a provider is not known here, and the
/// caller refuses that first.
#[must_use]
pub fn signing_in(agent: &str, method: &str) -> Option<String> {
    let named = !agent.is_empty() && !agent.contains('@');
    let id = !method.is_empty()
        && method.len() <= METHOD_BYTES
        && method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    (named && id).then(|| format!("{agent}@{method}"))
}

/// Every name this build writes a credential under, by provider.
#[derive(Debug, Clone, Default)]
pub struct Names {
    names: Arc<BTreeMap<String, Vec<String>>>,
}

impl Names {
    /// The names, each read for the provider before its `@`.
    #[must_use]
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut by_provider: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in names {
            let held = by_provider.entry(provider_of(name).to_owned()).or_default();
            if !held.iter().any(|known| known == name) {
                held.push(name.to_owned());
            }
        }
        Self {
            names: Arc::new(by_provider),
        }
    }

    /// Every provider it was told a name of, in the order of their names.
    pub(super) fn providers(&self) -> impl Iterator<Item = &str> {
        self.names.keys().map(String::as_str)
    }

    /// Every name this build writes `provider`'s credential under; the bare
    /// name alone where it was told none.
    ///
    /// The bare name comes first wherever it is one of them, so a caller
    /// looking for the one that serves the provider finds it first.
    pub(super) fn of<'a>(&'a self, provider: &'a str) -> Vec<&'a str> {
        let mut names: Vec<&str> = match self.names.get(provider) {
            Some(names) => names.iter().map(String::as_str).collect(),
            None => vec![provider],
        };
        names.sort_by_key(|name| *name != provider);
        names
    }
}

/// The map a credential sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A key, under `keys`.
    Key,
    /// An account's tokens, under `subscriptions`.
    Account,
}

/// A credential the store holds, by the map it is in and its name, which
/// together say which row it was given on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Held {
    /// The map it is in.
    pub kind: Kind,
    /// The name it is under.
    pub name: String,
}

impl Held {
    pub(super) fn new(kind: Kind, name: &str) -> Self {
        Self {
            kind,
            name: name.to_owned(),
        }
    }
}

/// A credential a write took out to keep a provider to one.
pub type Dropped = Held;

/// What settling the store did at a start.
///
/// An enum a start has to match, so that finding two credentials and being
/// unable to take one out cannot read as having found nothing.
#[derive(Debug)]
pub enum Settled {
    /// No provider holds a second credential.
    Nothing,
    /// Each provider's second credential was taken out.
    Removed(Vec<Dropped>),
    /// A provider holds a second credential and the write that would take it
    /// out could not be made: it stays, and the next start tries again.
    Stayed {
        /// What would have been taken out.
        found: Vec<Dropped>,
        /// Why it could not be.
        why: crate::error::AuthError,
    },
}
