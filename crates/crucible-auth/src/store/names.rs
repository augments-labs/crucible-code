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

use std::collections::BTreeMap;
use std::sync::Arc;

/// The provider a stored name belongs to: everything before its `@`.
#[must_use]
pub fn provider_of(name: &str) -> &str {
    name.split_once('@').map_or(name, |(provider, _)| provider)
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Default)]
pub struct Settled {
    /// What was taken out, or would have been had the write been made.
    pub dropped: Vec<Dropped>,
    /// Why the write could not be made, where it could not; what was found
    /// stays, and the next start tries again.
    pub unwritten: Option<crate::error::AuthError>,
}
