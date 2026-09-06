//! Opt-in confinement resolved without letting workspace layers weaken it.

use serde_json::Value;

use crate::document::{Document, Origin};
use crate::error::{At, ConfigError};

/// The one confinement choice a document actually stated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SandboxLayer {
    origin: Origin,
    enabled: bool,
}

/// Reads one typed layer after the general shape walk.
pub(crate) fn read(
    value: &Value,
    file: &str,
    text: &str,
    origin: Origin,
) -> Result<Option<SandboxLayer>, ConfigError> {
    let Some(enabled) = value
        .get("sandbox")
        .and_then(|block| block.get("enabled"))
        .and_then(Value::as_bool)
    else {
        return Ok(None);
    };
    if origin.in_the_workspace() && !enabled {
        return Err(ConfigError::Widening {
            file: file.into(),
            path: "sandbox.enabled".into(),
            at: At::of("enabled", text),
        });
    }
    Ok(Some(SandboxLayer { origin, enabled }))
}

/// Resolves the user choice, then applies only project strengthening.
pub(crate) fn resolve(documents: &[Document]) -> bool {
    let mut enabled = documents
        .iter()
        .filter_map(Document::sandbox)
        .find(|layer| layer.origin == Origin::User)
        .is_some_and(|layer| layer.enabled);
    if documents
        .iter()
        .filter_map(Document::sandbox)
        .any(|layer| layer.origin.in_the_workspace())
    {
        enabled = true;
    }
    enabled
}

#[cfg(test)]
mod tests;
