//! Provider-neutral secret state for renewable account credentials.
//!
//! The store owns the versioned representation while each login implementation
//! owns how the values are obtained, renewed and applied. Keeping that common
//! state here means neither side needs a closed provider enum.

use std::collections::BTreeMap;
use std::fmt;

/// The secret and bounded metadata one renewable credential needs.
#[derive(Clone)]
pub(crate) struct Tokens {
    access: Box<str>,
    refresh: Box<str>,
    details: BTreeMap<String, String>,
    expires_at: u64,
    refreshed_at: u64,
}

impl Tokens {
    pub(crate) fn new(
        access: Box<str>,
        refresh: Box<str>,
        expires_at: u64,
        refreshed_at: u64,
    ) -> Self {
        Self {
            access,
            refresh,
            details: BTreeMap::new(),
            expires_at,
            refreshed_at,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_detail(mut self, name: &str, value: impl Into<String>) -> Self {
        self.details.insert(name.to_owned(), value.into());
        self
    }

    pub(crate) fn access(&self) -> &str {
        &self.access
    }

    pub(crate) fn refresh(&self) -> &str {
        &self.refresh
    }

    pub(crate) fn details(&self) -> &BTreeMap<String, String> {
        &self.details
    }

    pub(crate) fn replace_details(&mut self, details: BTreeMap<String, String>) {
        self.details = details;
    }

    pub(crate) fn times(&self) -> (u64, u64) {
        (self.expires_at, self.refreshed_at)
    }
}

impl fmt::Debug for Tokens {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("Tokens(<redacted>)")
    }
}
