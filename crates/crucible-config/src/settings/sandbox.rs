//! Confinement mode resolved without letting workspace layers weaken it.

use crucible_core::SandboxMode;
use serde_json::Value;

use crate::document::{Document, Origin};
use crate::error::{At, ConfigError};

/// The one confinement choice a document actually stated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SandboxLayer {
    origin: Origin,
    mode: SandboxMode,
}

/// Reads one typed layer after the general shape walk.
pub(crate) fn read(
    value: &Value,
    file: &str,
    text: &str,
    origin: Origin,
) -> Result<Option<SandboxLayer>, ConfigError> {
    let Some(written) = value
        .get("sandbox")
        .and_then(|block| block.get("mode"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let mode = match written {
        "required" => SandboxMode::Required,
        "degraded" => SandboxMode::Degraded,
        "off" => SandboxMode::Off,
        // The shape walk already proves this cannot occur.
        _ => return Ok(None),
    };
    if origin.in_the_workspace() && mode != SandboxMode::Required {
        return Err(ConfigError::Widening {
            file: file.into(),
            path: "sandbox.mode".into(),
            at: At::of("mode", text),
        });
    }
    Ok(Some(SandboxLayer { origin, mode }))
}

/// Resolves the user choice, then applies only project strengthening.
pub(crate) fn resolve(documents: &[Document]) -> SandboxMode {
    let mut mode = documents
        .iter()
        .filter_map(Document::sandbox)
        .find(|layer| layer.origin == Origin::User)
        .map_or(SandboxMode::Required, |layer| layer.mode);
    if documents
        .iter()
        .filter_map(Document::sandbox)
        .any(|layer| layer.origin.in_the_workspace())
    {
        mode = SandboxMode::Required;
    }
    mode
}
