//! Resolve domain and local socket grants once, preserving project restrictions.
//!
//! User configuration introduces authority. Project allowlists and socket lists
//! replace inherited lists only after proving they are subsets; denies accumulate.

use crucible_core::{
    MAX_SANDBOX_NETWORK_RULES, SandboxDomainPattern, SandboxDomainPolicy, SandboxNetworkPolicy,
    SandboxNetworkProvenance,
};
use serde_json::Value;

use super::{ConfigError, Origin, Path, PathBuf, Source, Workspace};

#[derive(Clone)]
struct Stated<T> {
    value: T,
    source: Source,
}

#[derive(Clone, Default)]
pub(super) struct NetworkLayer {
    allowed: Option<Stated<Vec<SandboxDomainPattern>>>,
    denied: Option<Stated<Vec<SandboxDomainPattern>>>,
    binding: Option<Stated<bool>>,
    sockets: Option<Stated<Vec<PathBuf>>>,
}

#[derive(Clone, Default)]
pub(super) struct NetworkSettings {
    allowed: Vec<SandboxDomainPattern>,
    denied: Vec<SandboxDomainPattern>,
    binding: bool,
    sockets: Vec<PathBuf>,
}

impl std::fmt::Debug for NetworkSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkSettings")
            .field("allowed", &self.allowed.len())
            .field("denied", &self.denied.len())
            .field("binding", &self.binding)
            .field("sockets", &self.sockets.len())
            .finish()
    }
}

pub(super) fn read(
    block: &Value,
    source: &impl Fn(&'static str) -> Source,
) -> Result<NetworkLayer, ConfigError> {
    let Some(block) = block.get("network") else {
        return Ok(NetworkLayer::default());
    };
    let domains = |key: &'static str| -> Result<_, ConfigError> {
        let Some(values) = block
            .get(key.trim_start_matches("network."))
            .and_then(Value::as_array)
        else {
            return Ok(None);
        };
        let source = source(key);
        let value = values
            .iter()
            .map(|value| {
                SandboxDomainPattern::new(value.as_str().unwrap_or_default()).map_err(|_| {
                    source.error(
                        "use a hostname, IP literal, *.domain pattern or * without a URL or port",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Stated { value, source }))
    };
    let sockets = block.get("allowUnixSockets").and_then(Value::as_array).map(|values| {
        let source = source("network.allowUnixSockets");
        let value = values.iter().map(|value| {
            let path = Path::new(value.as_str().unwrap_or_default());
            if !super::valid_path(path) { return Err(source.error("Unix sockets require nonempty native paths without NUL or parent traversal")); }
            Ok(path.components().collect())
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(Stated { value, source })
    }).transpose()?;
    Ok(NetworkLayer {
        allowed: domains("network.allowedDomains")?,
        denied: domains("network.deniedDomains")?,
        binding: block
            .get("allowLocalBinding")
            .and_then(Value::as_bool)
            .map(|value| Stated {
                value,
                source: source("network.allowLocalBinding"),
            }),
        sockets,
    })
}

impl NetworkSettings {
    pub(super) fn apply(
        &mut self,
        layer: &NetworkLayer,
        origin: Origin,
    ) -> Result<(), ConfigError> {
        let project = origin.in_the_workspace();
        if let Some(stated) = &layer.allowed {
            if project
                && stated.value.iter().any(|rule| {
                    !self
                        .allowed
                        .iter()
                        .any(|parent| rule.is_no_wider_than(parent))
                })
            {
                return Err(stated.source.error(
                    "a project domain allowlist must be a subset of inherited user grants",
                ));
            }
            self.allowed.clone_from(&stated.value);
        }
        if let Some(stated) = &layer.denied {
            self.denied.extend(stated.value.iter().cloned());
            self.denied.sort();
            self.denied.dedup();
            if self.denied.len() > MAX_SANDBOX_NETWORK_RULES {
                return Err(stated
                    .source
                    .error("effective denied domains exceed their bound"));
            }
        }
        if let Some(stated) = &layer.binding {
            if project && stated.value && !self.binding {
                return Err(stated
                    .source
                    .error("only user configuration may grant local binding"));
            }
            self.binding = stated.value;
        }
        if let Some(stated) = &layer.sockets {
            if project && stated.value.iter().any(|path| !self.sockets.contains(path)) {
                return Err(stated
                    .source
                    .error("a project Unix socket list must be a subset of inherited user paths"));
            }
            self.sockets.clone_from(&stated.value);
        }
        Ok(())
    }

    pub(super) fn policy(
        &self,
        workspace: &Workspace,
    ) -> Result<SandboxNetworkPolicy, ConfigError> {
        if self.allowed.is_empty()
            && self.denied.is_empty()
            && !self.binding
            && self.sockets.is_empty()
        {
            return Ok(SandboxNetworkPolicy::Closed);
        }
        SandboxDomainPolicy::new(
            self.allowed.iter().cloned(),
            self.denied.iter().cloned(),
            self.binding,
            self.sockets
                .iter()
                .map(|path| workspace.root().join(path).components().collect()),
            SandboxNetworkProvenance::User,
        )
        .map(SandboxNetworkPolicy::Domains)
        .map_err(|_| ConfigError::Sandbox {
            file: "effective configuration".into(),
            path: "sandbox.network".into(),
            at: super::At::Ambiguous,
            problem: "network paths or rule counts are invalid",
        })
    }
}
