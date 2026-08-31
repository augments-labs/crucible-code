//! Prompt-cache policy resolved across authority-bearing configuration layers.

use crucible_core::{
    PromptCacheIsolation, PromptCacheMechanism, PromptCacheMechanisms, PromptCacheMode,
    PromptCacheNamespace, PromptCachePersistentMode, PromptCachePolicy, PromptCachePolicySource,
    PromptCacheRetention, PromptCacheRetentionClass,
};
use serde_json::Value;

use crate::document::{Document, Origin};
use crate::error::{At, ConfigError};

/// Only fields one document actually stated.
#[derive(Clone)]
pub(crate) struct PromptCacheLayer {
    file: Box<str>,
    mode: Option<PromptCacheMode>,
    mechanisms: Option<PromptCacheMechanisms>,
    isolation: Option<PromptCacheIsolation>,
    retention: Option<PromptCacheRetention>,
    persistent_resources: Option<PromptCachePersistentMode>,
    namespace: Option<PromptCacheNamespace>,
}

/// Reads semantic policy constraints after the general shape walk succeeded.
pub(crate) fn read(
    value: &Value,
    file: &str,
    text: &str,
    _origin: Origin,
) -> Result<Option<PromptCacheLayer>, ConfigError> {
    let Some(block) = value.get("promptCaching") else {
        return Ok(None);
    };

    let mode = word::<PromptCacheMode>(block, "mode");

    let mechanisms = block
        .get("allowedMechanisms")
        .and_then(Value::as_array)
        .map(|written| {
            written.iter().filter_map(Value::as_str).fold(
                PromptCacheMechanisms::NONE,
                |held, value| {
                    value
                        .parse::<PromptCacheMechanism>()
                        .map_or(held, |mechanism| held.with(mechanism))
                },
            )
        });

    let isolation = word::<PromptCacheIsolation>(block, "isolationScope");

    let retention = retention(block, file, text)?;

    let persistent_resources = block
        .get("persistentResources")
        .and_then(|resource| word::<PromptCachePersistentMode>(resource, "mode"));

    let namespace = block
        .get("namespace")
        .and_then(Value::as_str)
        .map(PromptCacheNamespace::new)
        .transpose()
        .map_err(|_| {
            semantic(
                file,
                text,
                "promptCaching.namespace",
                "namespace",
                "namespace must be 1 to 64 ASCII letters, digits, '.', '-' or '_'",
            )
        })?;

    if mode == Some(PromptCacheMode::Prohibit)
        && persistent_resources.is_some_and(|mode| mode != PromptCachePersistentMode::Forbid)
    {
        return Err(semantic(
            file,
            text,
            "promptCaching",
            "promptCaching",
            "prohibit mode cannot authorize a persistent cache resource",
        ));
    }

    Ok(Some(PromptCacheLayer {
        file: file.into(),
        mode,
        mechanisms,
        isolation,
        retention,
        persistent_resources,
        namespace,
    }))
}

fn word<T: std::str::FromStr>(block: &Value, key: &str) -> Option<T> {
    block.get(key)?.as_str()?.parse().ok()
}

fn retention(
    block: &Value,
    file: &str,
    text: &str,
) -> Result<Option<PromptCacheRetention>, ConfigError> {
    let Some(retention) = block.get("requestedRetention") else {
        return Ok(None);
    };
    let class = word::<PromptCacheRetentionClass>(retention, "class").ok_or_else(|| {
        semantic(
            file,
            text,
            "promptCaching.requestedRetention.class",
            "class",
            "requested retention must name its class",
        )
    })?;
    let seconds = retention.get("maxSeconds").and_then(Value::as_u64);

    match class {
        PromptCacheRetentionClass::ProviderDefault if seconds.is_none() => {
            Ok(Some(PromptCacheRetention::provider_default()))
        }
        PromptCacheRetentionClass::ProviderDefault => Err(semantic(
            file,
            text,
            "promptCaching.requestedRetention.maxSeconds",
            "maxSeconds",
            "providerDefault does not accept a retention override",
        )),
        PromptCacheRetentionClass::Ephemeral | PromptCacheRetentionClass::Extended => {
            let seconds = seconds.and_then(|seconds| u32::try_from(seconds).ok());
            let parsed = seconds.and_then(|seconds| match class {
                PromptCacheRetentionClass::Ephemeral => {
                    PromptCacheRetention::ephemeral(seconds).ok()
                }
                PromptCacheRetentionClass::Extended => PromptCacheRetention::extended(seconds).ok(),
                PromptCacheRetentionClass::ProviderDefault => None,
            });
            parsed.map(Some).ok_or_else(|| {
                semantic(
                    file,
                    text,
                    "promptCaching.requestedRetention.maxSeconds",
                    "maxSeconds",
                    "ephemeral and extended retention need a bounded positive maxSeconds",
                )
            })
        }
    }
}

fn semantic(file: &str, text: &str, path: &str, key: &str, problem: &'static str) -> ConfigError {
    ConfigError::PromptCaching {
        file: file.into(),
        path: path.into(),
        at: At::of(key, text),
        problem,
    }
}

/// Applies user replacement once, then intersects every workspace layer.
pub(crate) fn resolve(documents: &[Document]) -> Result<PromptCachePolicy, ConfigError> {
    let mut effective = PromptCachePolicy::default();

    for document in documents {
        let Some(layer) = document.prompt_cache() else {
            continue;
        };
        let source = if document.origin() == Origin::User {
            PromptCachePolicySource::User
        } else {
            PromptCachePolicySource::Workspace
        };
        let wanted = apply(effective, layer, source);

        if source == PromptCachePolicySource::User {
            effective = wanted.validate().map_err(|_| ConfigError::PromptCaching {
                file: layer.file.clone(),
                path: "promptCaching".into(),
                at: At::Ambiguous,
                problem: "prompt-cache policy is contradictory",
            })?;
            continue;
        }

        let narrowed = effective.narrowed(wanted);
        reject_broader(layer, wanted, narrowed)?;
        effective = narrowed
            .validate()
            .map_err(|_| ConfigError::PromptCaching {
                file: layer.file.clone(),
                path: "promptCaching".into(),
                at: At::Ambiguous,
                problem: "workspace narrowing conflicts with the inherited prompt-cache policy",
            })?;
    }

    Ok(effective)
}

fn apply(
    mut policy: PromptCachePolicy,
    layer: &PromptCacheLayer,
    source: PromptCachePolicySource,
) -> PromptCachePolicy {
    if let Some(mode) = layer.mode {
        policy = policy.with_mode_from(mode, source);
    }
    if let Some(mechanisms) = layer.mechanisms {
        policy = policy.allowing_from(mechanisms, source);
    }
    if let Some(isolation) = layer.isolation {
        policy = policy.with_isolation_from(isolation, source);
    }
    if let Some(retention) = layer.retention {
        policy = policy.with_retention_from(retention, source);
    }
    if let Some(resources) = layer.persistent_resources {
        policy = policy.with_persistent_resources_from(resources, source);
    }
    if let Some(namespace) = layer.namespace {
        policy = policy.with_namespace_from(namespace, source);
    }
    policy
}

fn reject_broader(
    layer: &PromptCacheLayer,
    wanted: PromptCachePolicy,
    narrowed: PromptCachePolicy,
) -> Result<(), ConfigError> {
    let broader = (layer.mode.is_some() && wanted.mode() != narrowed.mode())
        .then_some("promptCaching.mode")
        .or_else(|| {
            (layer.mechanisms.is_some()
                && wanted.allowed_mechanisms() != narrowed.allowed_mechanisms())
            .then_some("promptCaching.allowedMechanisms")
        })
        .or_else(|| {
            (layer.isolation.is_some() && wanted.isolation() != narrowed.isolation())
                .then_some("promptCaching.isolationScope")
        })
        .or_else(|| {
            (layer.retention.is_some() && wanted.retention() != narrowed.retention())
                .then_some("promptCaching.requestedRetention")
        })
        .or_else(|| {
            (layer.persistent_resources.is_some()
                && wanted.persistent_resources() != narrowed.persistent_resources())
            .then_some("promptCaching.persistentResources.mode")
        });

    match broader {
        None => Ok(()),
        Some(path) => Err(ConfigError::PromptCaching {
            file: layer.file.clone(),
            path: path.into(),
            at: At::Ambiguous,
            problem: "a workspace layer may only narrow the inherited prompt-cache ceiling",
        }),
    }
}

impl super::Settings {
    /// Fully merged typed prompt-cache policy.
    #[must_use]
    pub const fn prompt_cache(&self) -> PromptCachePolicy {
        self.prompt_cache
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        PromptCacheIsolation, PromptCacheMechanism, PromptCacheMechanisms, PromptCacheMode,
        PromptCacheNamespace, PromptCachePersistentMode, PromptCachePolicy, PromptCacheRetention,
    };

    use crate::document::{Document, Origin};
    use crate::error::ConfigError;
    use crate::settings::Settings;

    #[test]
    fn absence_is_the_same_default_as_typed_sdk_construction() {
        assert_eq!(
            Settings::resolve(Vec::new()).prompt_cache(),
            PromptCachePolicy::default()
        );
    }

    #[test]
    fn canonical_json_resolves_to_the_same_typed_policy() {
        let namespace = PromptCacheNamespace::new("personal").unwrap();
        let expected = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Require)
            .with_isolation(PromptCacheIsolation::User)
            .with_retention(PromptCacheRetention::extended(3_600).unwrap())
            .with_persistent_resources(PromptCachePersistentMode::Create)
            .allowing(
                PromptCacheMechanisms::one(PromptCacheMechanism::ExplicitBreakpoints)
                    .with(PromptCacheMechanism::PersistentContent),
            )
            .with_namespace(namespace);
        let document = Document::sample(
            r#"{
              "promptCaching": {
                "mode": "require",
                "allowedMechanisms": ["explicitBreakpoints", "persistentContent"],
                "isolationScope": "user",
                "requestedRetention": {"class": "extended", "maxSeconds": 3600},
                "persistentResources": {"mode": "create"},
                "namespace": "personal"
              }
            }"#,
            Origin::User,
        );

        let actual = Settings::resolve(vec![document]).prompt_cache();

        assert_eq!(actual.mode(), expected.mode());
        assert_eq!(actual.isolation(), expected.isolation());
        assert_eq!(actual.retention(), expected.retention());
        assert_eq!(
            actual.persistent_resources(),
            expected.persistent_resources()
        );
        assert_eq!(actual.allowed_mechanisms(), expected.allowed_mechanisms());
        assert_eq!(actual.namespace(), expected.namespace());
    }

    #[test]
    fn workspace_fields_intersect_the_user_ceiling_instead_of_replacing_the_object() {
        let user = Document::sample(
            r#"{"promptCaching":{"mode":"require","allowedMechanisms":["explicitBreakpoints","persistentContent"],"isolationScope":"user","requestedRetention":{"class":"extended","maxSeconds":3600},"persistentResources":{"mode":"create"},"namespace":"personal"}}"#,
            Origin::User,
        );
        let project = Document::sample(
            r#"{"promptCaching":{"allowedMechanisms":["explicitBreakpoints"],"isolationScope":"run","requestedRetention":{"class":"ephemeral","maxSeconds":300},"persistentResources":{"mode":"reuse"}}}"#,
            Origin::Project,
        );

        let policy = Settings::resolve(vec![project, user]).prompt_cache();

        assert_eq!(policy.mode(), PromptCacheMode::Require);
        assert_eq!(policy.isolation(), PromptCacheIsolation::Run);
        assert_eq!(
            policy.retention(),
            PromptCacheRetention::ephemeral(300).unwrap()
        );
        assert_eq!(
            policy.persistent_resources(),
            PromptCachePersistentMode::Reuse
        );
        assert_eq!(
            policy.allowed_mechanisms(),
            PromptCacheMechanisms::one(PromptCacheMechanism::ExplicitBreakpoints)
        );
        assert_eq!(policy.namespace().unwrap().as_str(), "personal");
    }

    #[test]
    fn a_workspace_may_repeat_or_narrow_user_authority_but_never_widen_it() {
        let user = Document::sample(
            r#"{"promptCaching":{"mode":"require","isolationScope":"user","requestedRetention":{"class":"extended","maxSeconds":3600},"persistentResources":{"mode":"create"}}}"#,
            Origin::User,
        );
        let narrower = Document::parse(
            r#"{"promptCaching":{"mode":"require","isolationScope":"workspace","requestedRetention":{"class":"extended","maxSeconds":1800},"persistentResources":{"mode":"create"}}}"#,
            ".crucible/config.json",
            Origin::Project,
        )
        .expect("a workspace request beneath the user ceiling");

        let policy = Settings::resolve_checked(vec![narrower, user])
            .unwrap()
            .prompt_cache();
        assert_eq!(policy.mode(), PromptCacheMode::Require);
        assert_eq!(policy.isolation(), PromptCacheIsolation::Workspace);
        assert_eq!(
            policy.retention(),
            PromptCacheRetention::extended(1_800).unwrap()
        );
        assert_eq!(
            policy.persistent_resources(),
            PromptCachePersistentMode::Create
        );

        for (user, project) in [
            (
                r#"{"promptCaching":{"mode":"observeOnly"}}"#,
                r#"{"promptCaching":{"mode":"prefer"}}"#,
            ),
            (
                r#"{"promptCaching":{"isolationScope":"session"}}"#,
                r#"{"promptCaching":{"isolationScope":"workspace"}}"#,
            ),
            (
                r#"{"promptCaching":{"requestedRetention":{"class":"ephemeral","maxSeconds":300}}}"#,
                r#"{"promptCaching":{"requestedRetention":{"class":"extended","maxSeconds":600}}}"#,
            ),
            (
                r#"{"promptCaching":{"persistentResources":{"mode":"reuse"}}}"#,
                r#"{"promptCaching":{"persistentResources":{"mode":"create"}}}"#,
            ),
        ] {
            let documents = vec![
                Document::sample(user, Origin::User),
                Document::parse(project, ".crucible/config.json", Origin::Project)
                    .expect("the field shape is valid before cross-layer resolution"),
            ];
            assert!(
                Settings::resolve_checked(documents).is_err(),
                "accepted project widening {project}"
            );
        }
    }

    #[test]
    fn a_workspace_cannot_supply_a_cache_namespace() {
        let error = Document::parse(
            r#"{"promptCaching":{"namespace":"project-key"}}"#,
            ".crucible/config.json",
            Origin::Project,
        )
        .expect_err("project-owned external identity must be refused");
        assert!(matches!(error, ConfigError::Widening { .. }), "{error:?}");
    }

    #[test]
    fn contradictory_or_unbounded_retention_is_rejected_with_its_path() {
        for (text, path) in [
            (
                r#"{"promptCaching":{"mode":"prohibit","persistentResources":{"mode":"create"}}}"#,
                "promptCaching",
            ),
            (
                r#"{"promptCaching":{"requestedRetention":{"class":"extended"}}}"#,
                "promptCaching.requestedRetention.maxSeconds",
            ),
            (
                r#"{"promptCaching":{"requestedRetention":{"class":"ephemeral","maxSeconds":0}}}"#,
                "promptCaching.requestedRetention.maxSeconds",
            ),
        ] {
            let error = Document::parse(text, "config.json", Origin::User)
                .expect_err("invalid prompt-cache policy");
            let shown = error.to_string();
            assert!(
                matches!(error, ConfigError::PromptCaching { .. }),
                "{error:?}"
            );
            assert!(shown.contains(path), "{shown}");
        }
    }
}
