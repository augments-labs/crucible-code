//! How one configuration layer's prompt-cache policy is held under the layer it inherits.
//!
//! The policy is a value a session log records, so it lives in `crucible-types`
//! with its own validation. Narrowing one layer under another is the policy
//! decision, and it is made here.

use crucible_types::{
    PromptCacheMode, PromptCachePersistentMode, PromptCachePolicy, PromptCachePolicyConflict,
    PromptCachePolicySource,
};

/// Holds a descendant layer's policy under every axis of the policy it inherits.
///
/// Each axis keeps the stricter of the two, and says which layer it came from;
/// a descendant asking for more than it inherited gets what it inherited. A
/// contradiction found on the way — an inherited requirement narrowed to
/// prohibition, or a requirement left with no mechanism — is recorded on the
/// result rather than resolved.
#[must_use]
pub fn narrow_policy(held: PromptCachePolicy, wanted: PromptCachePolicy) -> PromptCachePolicy {
    let (mode, mode_source, mode_conflict) = narrow_mode(held, wanted);
    let (isolation, isolation_source) = if held.isolation() <= wanted.isolation() {
        (held.isolation(), held.sources().isolation())
    } else {
        (wanted.isolation(), wanted.sources().isolation())
    };
    let retention = held.retention().narrowed(wanted.retention());
    let retention_source = if retention == held.retention() {
        held.sources().retention()
    } else {
        wanted.sources().retention()
    };
    let persistent_resources =
        narrow_persistent(held.persistent_resources(), wanted.persistent_resources());
    let persistent_source = if persistent_resources == held.persistent_resources() {
        held.sources().persistent_resources()
    } else {
        wanted.sources().persistent_resources()
    };
    let allowed_mechanisms = held
        .allowed_mechanisms()
        .intersection(wanted.allowed_mechanisms());
    let mechanism_source = if allowed_mechanisms == held.allowed_mechanisms() {
        held.sources().mechanisms()
    } else {
        wanted.sources().mechanisms()
    };
    let conflict = mode_conflict
        .or(held.conflict())
        .or(wanted.conflict())
        .or_else(|| {
            (mode == PromptCacheMode::Require && allowed_mechanisms.is_empty())
                .then_some(PromptCachePolicyConflict::RequiredWithoutMechanism)
        });

    let namespace_source = if held.namespace().is_some() {
        held.sources().namespace()
    } else {
        wanted.sources().namespace()
    };

    let policy = PromptCachePolicy::default()
        .with_mode_from(mode, mode_source)
        .allowing_from(allowed_mechanisms, mechanism_source)
        .with_isolation_from(isolation, isolation_source)
        .with_retention_from(retention, retention_source)
        .with_persistent_resources_from(persistent_resources, persistent_source);
    let policy = match held.namespace().or(wanted.namespace()) {
        Some(namespace) => policy.with_namespace_from(namespace, namespace_source),
        None => policy.without_namespace_from(namespace_source),
    };
    match conflict {
        Some(conflict) => policy.with_conflict(conflict),
        None => policy,
    }
}

fn narrow_mode(
    held: PromptCachePolicy,
    wanted: PromptCachePolicy,
) -> (
    PromptCacheMode,
    PromptCachePolicySource,
    Option<PromptCachePolicyConflict>,
) {
    use PromptCacheMode::{ObserveOnly, Prefer, Prohibit, Require};
    match (held.mode(), wanted.mode()) {
        (Require, Prohibit | ObserveOnly) => (
            Require,
            held.sources().mode(),
            Some(PromptCachePolicyConflict::RequiredAndProhibited),
        ),
        (Require, _) => (Require, held.sources().mode(), None),
        (Prohibit, _) => (Prohibit, held.sources().mode(), None),
        (ObserveOnly, _) => (ObserveOnly, held.sources().mode(), None),
        (Prefer, ObserveOnly | Prohibit) => (wanted.mode(), wanted.sources().mode(), None),
        (Prefer, Prefer | Require) => (Prefer, held.sources().mode(), None),
    }
}

const fn narrow_persistent(
    held: PromptCachePersistentMode,
    wanted: PromptCachePersistentMode,
) -> PromptCachePersistentMode {
    if held.authority() < wanted.authority() {
        held
    } else if wanted.authority() < held.authority() {
        wanted
    } else {
        // Equal create authority does not let a descendant turn Create into
        // Require or weaken an inherited Require into Create.
        held
    }
}

#[cfg(test)]
mod tests {
    use crucible_types::{
        PromptCacheIsolation, PromptCacheMechanism, PromptCacheMechanisms, PromptCacheMode,
        PromptCachePersistentMode, PromptCachePolicy, PromptCachePolicyConflict,
        PromptCacheRetention,
    };

    use super::narrow_policy;

    #[test]
    fn a_descendant_can_only_narrow_each_authority_axis() {
        let held = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Prefer)
            .with_isolation(PromptCacheIsolation::Workspace)
            .with_retention(PromptCacheRetention::extended(3_600).unwrap())
            .with_persistent_resources(PromptCachePersistentMode::Create)
            .allowing(PromptCacheMechanisms::ALL);
        let wanted = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::ObserveOnly)
            .with_isolation(PromptCacheIsolation::Run)
            .with_retention(PromptCacheRetention::ephemeral(300).unwrap())
            .with_persistent_resources(PromptCachePersistentMode::Reuse)
            .allowing(PromptCacheMechanisms::one(
                PromptCacheMechanism::ExplicitBreakpoints,
            ));

        let effective = narrow_policy(held, wanted);

        assert_eq!(effective.mode(), PromptCacheMode::ObserveOnly);
        assert_eq!(effective.isolation(), PromptCacheIsolation::Run);
        assert_eq!(
            effective.retention(),
            PromptCacheRetention::ephemeral(300).unwrap()
        );
        assert_eq!(
            effective.persistent_resources(),
            PromptCachePersistentMode::Reuse
        );
        assert_eq!(
            effective.allowed_mechanisms(),
            PromptCacheMechanisms::one(PromptCacheMechanism::ExplicitBreakpoints)
        );
    }

    #[test]
    fn a_descendant_cannot_turn_caching_on_or_widen_sharing() {
        let held = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::ObserveOnly)
            .with_isolation(PromptCacheIsolation::Run)
            .with_persistent_resources(PromptCachePersistentMode::Forbid);
        let wanted = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Require)
            .with_isolation(PromptCacheIsolation::User)
            .with_persistent_resources(PromptCachePersistentMode::Require);

        let effective = narrow_policy(held, wanted);

        assert_eq!(effective.mode(), PromptCacheMode::ObserveOnly);
        assert_eq!(effective.isolation(), PromptCacheIsolation::Run);
        assert_eq!(
            effective.persistent_resources(),
            PromptCachePersistentMode::Forbid
        );
    }

    #[test]
    fn narrowing_an_inherited_requirement_to_prohibit_is_an_explicit_conflict() {
        let held = PromptCachePolicy::default().with_mode(PromptCacheMode::Require);
        let wanted = PromptCachePolicy::default().with_mode(PromptCacheMode::Prohibit);

        let effective = narrow_policy(held, wanted);

        assert_eq!(effective.mode(), PromptCacheMode::Require);
        assert_eq!(
            effective.conflict(),
            Some(PromptCachePolicyConflict::RequiredAndProhibited)
        );
    }
}
