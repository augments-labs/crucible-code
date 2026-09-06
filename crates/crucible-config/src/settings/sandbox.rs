//! Opt-in confinement and resource settings, resolved by authority.
//!
//! Configuration paths are parsed once and retain their source. Only a user
//! document grants filesystem access; workspace documents can require
//! confinement, restrict existing access, and lower command ceilings. The
//! effective policy is built for the same workspace for every process host.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crucible_core::{
    SandboxEnablement, SandboxFilesystemAccess as Access,
    SandboxFilesystemProvenance as Provenance, SandboxFilesystemRule, SandboxPolicy,
    SandboxResourceLimits, Workspace,
};
use serde_json::Value;

use crate::document::{Document, Origin};
use crate::error::{At, ConfigError};

mod filesystem;
mod network;

/// One stated path with its authority and location retained for diagnostics.
#[derive(Clone)]
struct PathSetting {
    path: PathBuf,
    access: Access,
    source: Source,
}

/// Configuration coordinates; the configured value is deliberately absent.
#[derive(Debug, Clone)]
struct Source {
    file: Box<str>,
    key: &'static str,
    at: At,
    origin: Origin,
}

impl Source {
    fn error(&self, problem: &'static str) -> ConfigError {
        ConfigError::Sandbox {
            file: self.file.clone(),
            path: format!("sandbox.{}", self.key).into(),
            at: self.at,
            problem,
        }
    }

    fn provenance(&self) -> Provenance {
        match self.origin {
            Origin::User => Provenance::UserConfiguration,
            Origin::Project => Provenance::ProjectConfiguration,
            Origin::ProjectLocal => Provenance::ProjectLocalConfiguration,
        }
    }
}

/// A command ceiling a document actually stated.
#[derive(Debug, Clone)]
struct Limit {
    value: u64,
    source: Source,
}

/// The confinement settings one document actually stated.
#[derive(Clone)]
pub(crate) struct SandboxLayer {
    network: network::NetworkLayer,
    origin: Origin,
    enabled: Option<bool>,
    filesystem: Vec<PathSetting>,
    command_seconds: Option<Limit>,
    output_bytes: Option<Limit>,
    concurrent_commands: Option<Limit>,
}

impl std::fmt::Debug for SandboxLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxLayer")
            .field("origin", &self.origin)
            .field("enabled", &self.enabled)
            .field("filesystem_rules", &self.filesystem.len())
            .finish_non_exhaustive()
    }
}

/// Resolved settings applied to every sandboxed command host.
///
/// Paths retain authority separately from ordinary JSON layering. Debug output
/// exposes counts and limits, never the private path spellings.
#[derive(Clone)]
pub struct SandboxSettings {
    network: network::NetworkSettings,
    enablement: Arc<SandboxEnablement>,
    filesystem: Vec<PathSetting>,
    command_seconds: u64,
    output_bytes: u64,
    concurrent_commands: u64,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            enablement: Arc::new(SandboxEnablement::new(false, false)),
            filesystem: Vec::new(),
            network: network::NetworkSettings::default(),
            command_seconds: 1200,
            output_bytes: 10 * 1024 * 1024,
            concurrent_commands: 4,
        }
    }
}

impl std::fmt::Debug for SandboxSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxSettings")
            .field("network", &self.network)
            .field("enablement", &self.enablement)
            .field("filesystem_rules", &self.filesystem.len())
            .field("command_seconds", &self.command_seconds)
            .field("output_bytes", &self.output_bytes)
            .field("concurrent_commands", &self.concurrent_commands)
            .finish()
    }
}

impl SandboxSettings {
    /// Whether the resolved configuration requires OS confinement.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enablement.enabled()
    }

    /// Whether a project document prevents an interactive opt-out.
    #[must_use]
    pub fn required_by_project(&self) -> bool {
        self.enablement.required()
    }

    /// Shared host control. Changes affect new preparations, never active processes.
    #[must_use]
    pub fn enablement(&self) -> Arc<SandboxEnablement> {
        Arc::clone(&self.enablement)
    }

    /// Builds the effective filesystem view and command ceilings.
    ///
    /// Relative paths are based at the workspace root, including paths from
    /// the user configuration. No environment, home or glob expansion occurs.
    ///
    /// # Errors
    ///
    /// Refuses invalid native paths, contradictory access, excessive rules,
    /// and project restrictions that would grant previously absent access.
    pub fn policy(&self, workspace: &Workspace) -> Result<SandboxPolicy, ConfigError> {
        Ok(self
            .enforcing_policy(workspace)?
            .with_enabled(self.enabled()))
    }

    /// The configured enforcing policy before the host applies its enabled choice.
    ///
    /// Process hosts keep this immutable template and sample [`Self::enablement`]
    /// at preparation, so re-enabling restores the same kernel ceilings.
    ///
    /// # Errors
    ///
    /// The same invalid path and authority cases as [`Self::policy`].
    pub fn enforcing_policy(&self, workspace: &Workspace) -> Result<SandboxPolicy, ConfigError> {
        let base = SandboxPolicy::standard(workspace).map_err(|_| policy_error())?;
        let filesystem = filesystem::resolve(base.filesystem(), &self.filesystem, workspace)?;
        SandboxPolicy::new(
            true,
            filesystem,
            workspace.root(),
            self.network.policy(workspace)?,
            SandboxResourceLimits {
                command_time: Some(Duration::from_secs(self.command_seconds)),
                output_bytes: Some(self.output_bytes),
                concurrent_commands: Some(self.concurrent_commands),
                ..base.limits()
            },
        )
        .map_err(|_| policy_error())
    }
}

fn policy_error() -> ConfigError {
    ConfigError::Sandbox {
        file: "effective configuration".into(),
        path: "sandbox.filesystem".into(),
        at: At::Ambiguous,
        problem: "filesystem rules conflict, exceed their bound, or exclude the working directory",
    }
}

fn valid_path(path: &Path) -> bool {
    !path.as_os_str().as_encoded_bytes().contains(&0)
        && path.components().count() > 0
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
}

/// Reads one typed layer after the general shape walk.
pub(crate) fn read(
    value: &Value,
    file: &str,
    text: &str,
    origin: Origin,
) -> Result<Option<SandboxLayer>, ConfigError> {
    let Some(block) = value.get("sandbox") else {
        return Ok(None);
    };
    let source = |key: &'static str| Source {
        file: file.into(),
        key,
        origin,
        at: At::of(key.rsplit('.').next().unwrap_or(key), text),
    };
    let enabled = block.get("enabled").and_then(Value::as_bool);
    if origin.in_the_workspace() && enabled == Some(false) {
        return Err(ConfigError::Widening {
            file: file.into(),
            path: "sandbox.enabled".into(),
            at: At::of("enabled", text),
        });
    }
    let mut filesystem = Vec::new();
    for (key, access) in [
        ("filesystem.readOnly", Access::ReadOnly),
        ("filesystem.writable", Access::ReadWrite),
        ("filesystem.protected", Access::Protected),
        ("filesystem.unreadable", Access::Unreadable),
    ] {
        let Some(paths) = block
            .get("filesystem")
            .and_then(|fs| fs.get(key.trim_start_matches("filesystem.")))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for value in paths {
            let source = source(key);
            let path = Path::new(value.as_str().unwrap_or_default());
            if !valid_path(path) {
                return Err(source.error("paths must be nonempty, contain no NUL or parent traversal, and use native path syntax"));
            }
            if origin.in_the_workspace() && access == Access::ReadWrite {
                return Err(source.error("only user configuration may grant writable access"));
            }
            filesystem.push(PathSetting {
                path: path.into(),
                access,
                source,
            });
        }
    }
    if filesystem.len() > crucible_core::MAX_SANDBOX_FILESYSTEM_RULES {
        return Err(source("filesystem").error("too many filesystem rules"));
    }
    let limit = |key: &'static str| {
        block
            .get("limits")
            .and_then(|limits| limits.get(key.trim_start_matches("limits.")))
            .and_then(Value::as_u64)
            .map(|value| Limit {
                value,
                source: source(key),
            })
    };
    Ok(Some(SandboxLayer {
        network: network::read(block, &source)?,
        origin,
        enabled,
        filesystem,
        command_seconds: limit("limits.commandSeconds"),
        output_bytes: limit("limits.outputBytes"),
        concurrent_commands: limit("limits.concurrentCommands"),
    }))
}

/// Resolves user grants followed by project narrowing, in authority order.
pub(crate) fn resolve(documents: &[Document]) -> Result<SandboxSettings, ConfigError> {
    let mut settings = SandboxSettings::default();
    let mut enabled = false;
    let mut required = false;
    let mut layers: Vec<_> = documents.iter().filter_map(Document::sandbox).collect();
    layers.sort_by_key(|layer| layer.origin.nearness());
    for layer in layers {
        settings.network.apply(&layer.network, layer.origin)?;
        if let Some(stated) = layer.enabled {
            enabled = stated;
            required |= layer.origin.in_the_workspace() && stated;
        }
        settings.filesystem.extend(layer.filesystem.iter().cloned());
        if settings.filesystem.len() > crucible_core::MAX_SANDBOX_FILESYSTEM_RULES {
            return Err(policy_error());
        }
        for (effective, stated) in [
            (&mut settings.command_seconds, &layer.command_seconds),
            (&mut settings.output_bytes, &layer.output_bytes),
            (
                &mut settings.concurrent_commands,
                &layer.concurrent_commands,
            ),
        ] {
            if let Some(stated) = stated {
                if layer.origin.in_the_workspace() && stated.value > *effective {
                    return Err(stated.source.error(
                        "project configuration may only lower an inherited command ceiling",
                    ));
                }
                *effective = stated.value;
            }
        }
    }
    settings.enablement = Arc::new(SandboxEnablement::new(enabled, required));
    Ok(settings)
}

#[cfg(test)]
mod tests;
