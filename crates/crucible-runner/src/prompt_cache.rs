//! Scope-safe prompt-cache identity derivation for provider attempts.
//!
//! The provider-visible prefix has its own canonical fingerprint in
//! `crucible-core`. This module binds that fingerprint to the live owners the
//! runner can see: route, credential instance, model, selected sharing scope,
//! authority, instructions, and tool generation. Only the final opaque digest
//! can leave this boundary as a provider routing key.

use std::time::Instant;

use crucible_core::{
    Effort, PromptCacheCapabilities, PromptCacheFingerprint, PromptCacheIdentity,
    PromptCacheIneligibleReason, PromptCacheIsolation, PromptCacheKey, PromptCacheMechanism,
    PromptCacheMode, PromptCachePersistentMode, PromptCachePlan, PromptCachePolicy,
    PromptCachePolicyDigest, PromptCachePolicySource, PromptCacheProjection, PromptCacheRequest,
    PromptCacheResourceBinding, PromptCacheResourceCreate, PromptCacheResourceDeadline,
    PromptCacheResourceError, PromptCacheResourceFact, PromptCacheResourceLifecycle,
    PromptCacheResourceOperation, PromptCacheResourceOwner, PromptCacheResourceRecord,
    PromptCacheResourceReference, PromptCacheResourceState, PromptCacheResourceStore,
    PromptCacheRoute, PromptCacheSelection, ProviderAttemptId, Request, RunId, TurnError,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub(super) struct ScopeInputs<'a> {
    pub route: PromptCacheRoute<'a>,
    pub policy: PromptCachePolicy,
    pub model: &'a str,
    pub model_revision: Option<&'a str>,
    pub max_tokens: u32,
    pub effort: Option<Effort>,
    pub run: RunId,
    pub session: Option<&'a str>,
    pub workspace: &'a [u8],
    pub user: &'a [u8],
    pub trust: &'a [u8],
    pub authority: &'a [u8],
    pub instructions: &'a [u8],
    pub tool_generation: &'a str,
}

pub(super) struct Prepared {
    pub attempt: ProviderAttemptId,
    pub capabilities: PromptCacheCapabilities,
    pub policy: PromptCachePolicy,
    pub plan: PromptCachePlan,
    pub identity: PromptCacheIdentity,
    pub selection: PromptCacheSelection,
    pub routing_key: Option<PromptCacheKey>,
    pub resource: Option<PromptCacheResourceRecord>,
}

pub(super) struct ResourceInputs<'a> {
    pub store: &'a mut dyn PromptCacheResourceStore,
    pub lifecycle: &'a dyn PromptCacheResourceLifecycle,
    pub cancel: &'a crucible_core::Cancel,
    pub now: u64,
    pub deadline: Instant,
}

pub(super) fn prepare(
    request: &Request<'_>,
    capabilities: PromptCacheCapabilities,
    inputs: &ScopeInputs<'_>,
) -> Result<Prepared, TurnError> {
    prepare_inner(request, capabilities, inputs, None)
}

#[cfg(test)]
pub(super) fn prepare_with_resources(
    request: &Request<'_>,
    capabilities: PromptCacheCapabilities,
    inputs: &ScopeInputs<'_>,
    resources: ResourceInputs<'_>,
) -> Result<Prepared, TurnError> {
    let mut ignored = Vec::new();
    prepare_with_resource_facts(request, capabilities, inputs, resources, &mut ignored)
}

pub(super) fn prepare_with_resource_facts(
    request: &Request<'_>,
    capabilities: PromptCacheCapabilities,
    inputs: &ScopeInputs<'_>,
    resources: ResourceInputs<'_>,
    facts: &mut Vec<PromptCacheResourceFact>,
) -> Result<Prepared, TurnError> {
    facts.clear();
    prepare_inner(request, capabilities, inputs, Some((resources, facts)))
}

fn prepare_inner(
    request: &Request<'_>,
    capabilities: PromptCacheCapabilities,
    inputs: &ScopeInputs<'_>,
    resources: Option<(ResourceInputs<'_>, &mut Vec<PromptCacheResourceFact>)>,
) -> Result<Prepared, TurnError> {
    let attempt = ProviderAttemptId::new();
    let projection = PromptCacheProjection::inspect(request)?;
    let mut hash = Sha256::new();
    field(&mut hash, 0, b"crucible.prompt-cache.stable-prefix.v1");
    projection.write_stable(request, |bytes| hash.update(bytes))?;
    let fingerprint = PromptCacheFingerprint::new(hash.finalize().into());
    let plan = PromptCachePlan::new(&projection, fingerprint);
    let identity = identity(inputs, fingerprint);
    let ready_selection = PromptCacheSelection::prepare(inputs.policy, &capabilities, &plan, true);
    let wants_persistent = ready_selection
        .as_ref()
        .ok()
        .and_then(|selection| selection.selected())
        .is_some_and(|selected| selected.mechanism() == PromptCacheMechanism::PersistentContent);
    let must_persist = inputs.policy.persistent_resources() == PromptCachePersistentMode::Require;

    let (selection, resource) = if wants_persistent {
        let binding = PromptCacheResourceBinding::new(
            identity.scope(),
            provider_scope(inputs.route),
            owner_scope(inputs),
            identity.prefix(),
            policy_digest(inputs.policy),
            PromptCacheResourceOwner::new(
                inputs.policy.isolation(),
                matches!(
                    inputs.policy.isolation(),
                    PromptCacheIsolation::Run | PromptCacheIsolation::Session
                ),
            ),
            inputs.route.protocol,
            inputs.model,
            inputs.model_revision,
        )
        .map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
        let prepared = resources
            .ok_or(PromptCacheResourceError::Unsupported)
            .and_then(|(resources, facts)| {
                prepare_resource(request, inputs.policy, &binding, resources, attempt, facts)
            });
        match prepared {
            Ok(Some(record)) => (ready_selection?, Some(record)),
            Ok(None) => {
                if must_persist {
                    return Err(PromptCacheResourceError::Unsupported.into());
                }
                (
                    PromptCacheSelection::prepare(inputs.policy, &capabilities, &plan, false)?,
                    None,
                )
            }
            Err(problem @ PromptCacheResourceError::Cancelled) => return Err(problem.into()),
            Err(_problem) if !must_persist => {
                let fallback =
                    PromptCacheSelection::prepare(inputs.policy, &capabilities, &plan, false)?;
                (fallback, None)
            }
            Err(problem) => return Err(problem.into()),
        }
    } else {
        if must_persist {
            return Err(PromptCacheResourceError::Unsupported.into());
        }
        (
            PromptCacheSelection::prepare(inputs.policy, &capabilities, &plan, false)?,
            None,
        )
    };
    let routing_key = selection
        .selected()
        .and_then(|selected| {
            capabilities
                .mechanisms()
                .iter()
                .find(|candidate| candidate.mechanism() == selected.mechanism())
        })
        .filter(|candidate| candidate.supports_routing_key())
        .map(|_| routing_key(identity, 64));

    Ok(Prepared {
        attempt,
        capabilities,
        policy: inputs.policy,
        plan,
        identity,
        selection,
        routing_key,
        resource,
    })
}

impl Prepared {
    pub(super) fn request(&self) -> PromptCacheRequest<'_> {
        PromptCacheRequest {
            attempt: self.attempt,
            policy: self.policy(),
            capabilities: &self.capabilities,
            plan: &self.plan,
            identity: self.identity,
            selection: self.selection,
            routing_key: self.routing_key,
            resource: self.resource.as_ref().and_then(|record| {
                record
                    .handle()
                    .map(|handle| PromptCacheResourceReference::new(record.id(), handle))
            }),
        }
    }

    /// Replaces an optional control the adapter could not lower with an
    /// unchanged-request attempt. Required use and privacy prohibition never
    /// take this path.
    pub(super) fn fallback_request(
        &self,
        reason: PromptCacheIneligibleReason,
    ) -> Option<PromptCacheRequest<'_>> {
        (self.policy.mode() == PromptCacheMode::Prefer).then(|| PromptCacheRequest {
            selection: PromptCacheSelection::ineligible(reason),
            routing_key: None,
            resource: None,
            ..self.request()
        })
    }

    fn policy(&self) -> PromptCachePolicy {
        // The selected policy is identity-bound and supplied explicitly during
        // preparation. Kept beside the plan so a retry cannot accidentally
        // borrow a subsequently changed run policy.
        self.policy
    }
}

// Request, policy, binding, lifecycle inputs, attempt identity, and fact sink
// are independent boundaries for one state-machine transition.
#[allow(clippy::too_many_arguments)]
fn prepare_resource(
    request: &Request<'_>,
    policy: PromptCachePolicy,
    binding: &PromptCacheResourceBinding,
    resources: ResourceInputs<'_>,
    attempt: ProviderAttemptId,
    facts: &mut Vec<PromptCacheResourceFact>,
) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError> {
    let ResourceInputs {
        store,
        lifecycle,
        cancel,
        now,
        deadline,
    } = resources;
    if cancel.requested() {
        return Err(PromptCacheResourceError::Cancelled);
    }
    let deadline = PromptCacheResourceDeadline::new(deadline);
    if deadline.expired() {
        return Err(PromptCacheResourceError::Deadline);
    }

    if let Some(mut record) = store.matching(binding)? {
        let newly_expired = record.state() == PromptCacheResourceState::Ready
            && record.expires_at().is_some_and(|expiry| now >= expiry);
        if newly_expired || record.state() == PromptCacheResourceState::Expired {
            if newly_expired {
                record.set_state(PromptCacheResourceState::Expired, now);
                store.put(&record)?;
                push_resource_fact(facts, attempt, &record, None);
            }

            if record.binding().owner().exclusive() {
                record.set_state(PromptCacheResourceState::Deleting, now);
                store.put(&record)?;
                push_resource_fact(
                    facts,
                    attempt,
                    &record,
                    Some(PromptCacheResourceOperation::Delete),
                );
                match lifecycle.delete(&record, deadline, cancel) {
                    Ok(remote) if remote.state == PromptCacheResourceState::Deleted => {
                        record.set_state(PromptCacheResourceState::Deleted, now);
                        push_resource_fact(
                            facts,
                            attempt,
                            &record,
                            Some(PromptCacheResourceOperation::Delete),
                        );
                        store.remove(record.id())?;
                    }
                    Err(
                        problem @ (PromptCacheResourceError::Ambiguous(_)
                        | PromptCacheResourceError::Cancelled
                        | PromptCacheResourceError::Deadline),
                    ) => {
                        record.ambiguous(PromptCacheResourceOperation::Delete, now);
                        store.put(&record)?;
                        push_resource_fact(
                            facts,
                            attempt,
                            &record,
                            Some(PromptCacheResourceOperation::Delete),
                        );
                        return Err(problem);
                    }
                    Ok(_) | Err(_) => {
                        record.set_state(PromptCacheResourceState::Orphaned, now);
                        store.put(&record)?;
                        push_resource_fact(
                            facts,
                            attempt,
                            &record,
                            Some(PromptCacheResourceOperation::Delete),
                        );
                    }
                }
            } else {
                // A shared resource cannot be deleted on behalf of one local
                // reference. Once its provider expiry has passed, dropping
                // only this stale local reference is safe.
                store.remove(record.id())?;
            }

            return create_if_authorized(
                request, policy, binding, store, lifecycle, cancel, now, deadline, attempt, facts,
            );
        }

        let may_mutate = matches!(
            policy.persistent_resources(),
            PromptCachePersistentMode::Create | PromptCachePersistentMode::Require
        );
        let mut action = match record.state() {
            PromptCacheResourceState::Ready => ResourceAction::Resolve,
            PromptCacheResourceState::Expiring if may_mutate => ResourceAction::Renew,
            PromptCacheResourceState::Creating
            | PromptCacheResourceState::Deleting
            | PromptCacheResourceState::Ambiguous => ResourceAction::Reconcile(
                record
                    .pending()
                    .ok_or(PromptCacheResourceError::InvalidMetadata)?,
            ),
            PromptCacheResourceState::Expiring
            | PromptCacheResourceState::Deleted
            | PromptCacheResourceState::Expired
            | PromptCacheResourceState::Orphaned => {
                return create_if_authorized(
                    request, policy, binding, store, lifecycle, cancel, now, deadline, attempt,
                    facts,
                );
            }
        };

        // One resolve/reconcile may discover an expiring resource. Give it one
        // immediate renewal before this request is allowed to reference it.
        for _ in 0..2 {
            let before = resource_fact_state(&record);
            let operation = action.operation();
            let observed = match action {
                ResourceAction::Resolve => lifecycle.resolve(&record, deadline, cancel),
                ResourceAction::Renew => {
                    lifecycle.renew(&record, policy.retention(), deadline, cancel)
                }
                ResourceAction::Reconcile(_) => lifecycle.reconcile(&record, deadline, cancel),
            };

            match observed {
                Ok(remote) => {
                    if let Err(problem) = apply_remote(&mut record, remote, policy, now) {
                        // `apply_remote` deliberately turns a remotely accepted
                        // but over-ceiling resource into an orphan. That state
                        // must survive restart so cleanup can recover it.
                        store.put(&record)?;
                        push_resource_fact_if_changed(facts, attempt, &record, operation, before);
                        return Err(problem);
                    }
                    match record.state() {
                        PromptCacheResourceState::Deleted | PromptCacheResourceState::Expired => {
                            push_resource_fact_if_changed(
                                facts, attempt, &record, operation, before,
                            );
                            store.remove(record.id())?;
                            return create_if_authorized(
                                request, policy, binding, store, lifecycle, cancel, now, deadline,
                                attempt, facts,
                            );
                        }
                        PromptCacheResourceState::Orphaned => {
                            store.put(&record)?;
                            push_resource_fact_if_changed(
                                facts, attempt, &record, operation, before,
                            );
                            return create_if_authorized(
                                request, policy, binding, store, lifecycle, cancel, now, deadline,
                                attempt, facts,
                            );
                        }
                        PromptCacheResourceState::Expiring if may_mutate => {
                            store.put(&record)?;
                            push_resource_fact_if_changed(
                                facts, attempt, &record, operation, before,
                            );
                            action = ResourceAction::Renew;
                        }
                        PromptCacheResourceState::Ready => {
                            store.put(&record)?;
                            push_resource_fact_if_changed(
                                facts, attempt, &record, operation, before,
                            );
                            return Ok(record.can_reuse(binding, now).then_some(record));
                        }
                        PromptCacheResourceState::Creating
                        | PromptCacheResourceState::Expiring
                        | PromptCacheResourceState::Deleting
                        | PromptCacheResourceState::Ambiguous => {
                            store.put(&record)?;
                            push_resource_fact_if_changed(
                                facts, attempt, &record, operation, before,
                            );
                            return Ok(None);
                        }
                    }
                }
                Err(
                    problem @ (PromptCacheResourceError::Ambiguous(_)
                    | PromptCacheResourceError::Cancelled
                    | PromptCacheResourceError::Deadline),
                ) => {
                    // A failed resolve is a failed read, not an ambiguous
                    // renewal. Only an operation that may have changed remote
                    // state needs durable reconciliation.
                    if let Some(operation) = action.operation() {
                        record.ambiguous(operation, now);
                        store.put(&record)?;
                        push_resource_fact_if_changed(
                            facts,
                            attempt,
                            &record,
                            Some(operation),
                            before,
                        );
                    }
                    return Err(problem);
                }
                Err(problem) => return Err(problem),
            }
        }

        store.put(&record)?;
        return Ok(None);
    }

    create_if_authorized(
        request, policy, binding, store, lifecycle, cancel, now, deadline, attempt, facts,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_if_authorized(
    request: &Request<'_>,
    policy: PromptCachePolicy,
    binding: &PromptCacheResourceBinding,
    store: &mut dyn PromptCacheResourceStore,
    lifecycle: &dyn PromptCacheResourceLifecycle,
    cancel: &crucible_core::Cancel,
    now: u64,
    deadline: PromptCacheResourceDeadline,
    attempt: ProviderAttemptId,
    facts: &mut Vec<PromptCacheResourceFact>,
) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError> {
    if !matches!(
        policy.persistent_resources(),
        PromptCachePersistentMode::Create | PromptCachePersistentMode::Require
    ) {
        return Ok(None);
    }
    let mut record = PromptCacheResourceRecord::creating(
        crucible_core::PromptCacheResourceId::new(),
        binding.clone(),
        now,
    );
    store.put(&record)?;
    push_resource_fact(
        facts,
        attempt,
        &record,
        Some(PromptCacheResourceOperation::Create),
    );
    let created = lifecycle.create(
        PromptCacheResourceCreate {
            id: record.id(),
            request,
            binding,
            retention: policy.retention(),
            deadline,
        },
        cancel,
    );
    match created {
        Ok(created) => {
            if !expiry_allowed(policy, now, created.expires_at) {
                record.ready(created.handle, created.expires_at, now);
                record.set_state(PromptCacheResourceState::Orphaned, now);
                store.put(&record)?;
                push_resource_fact(
                    facts,
                    attempt,
                    &record,
                    Some(PromptCacheResourceOperation::Create),
                );
                return Err(PromptCacheResourceError::Rejected);
            }
            record.ready(created.handle, created.expires_at, now);
            store.put(&record)?;
            push_resource_fact(
                facts,
                attempt,
                &record,
                Some(PromptCacheResourceOperation::Create),
            );
            Ok(Some(record))
        }
        Err(
            problem @ (PromptCacheResourceError::Ambiguous(_)
            | PromptCacheResourceError::Cancelled
            | PromptCacheResourceError::Deadline),
        ) => {
            record.ambiguous(PromptCacheResourceOperation::Create, now);
            store.put(&record)?;
            push_resource_fact(
                facts,
                attempt,
                &record,
                Some(PromptCacheResourceOperation::Create),
            );
            Err(problem)
        }
        Err(problem) => {
            store.remove(record.id())?;
            Err(problem)
        }
    }
}

fn push_resource_fact(
    facts: &mut Vec<PromptCacheResourceFact>,
    attempt: ProviderAttemptId,
    record: &PromptCacheResourceRecord,
    operation: Option<PromptCacheResourceOperation>,
) {
    facts.push(PromptCacheResourceFact {
        attempt: Some(attempt),
        resource: record.id().clone(),
        operation,
        state: record.state(),
        expires_at: record.expires_at(),
        owner: record.binding().owner(),
    });
}

type ResourceFactState = (
    PromptCacheResourceState,
    Option<PromptCacheResourceOperation>,
    Option<u64>,
);

fn resource_fact_state(record: &PromptCacheResourceRecord) -> ResourceFactState {
    (record.state(), record.pending(), record.expires_at())
}

fn push_resource_fact_if_changed(
    facts: &mut Vec<PromptCacheResourceFact>,
    attempt: ProviderAttemptId,
    record: &PromptCacheResourceRecord,
    operation: Option<PromptCacheResourceOperation>,
    before: ResourceFactState,
) {
    if before != resource_fact_state(record) {
        push_resource_fact(facts, attempt, record, operation);
    }
}

pub(super) fn apply_remote(
    record: &mut PromptCacheResourceRecord,
    remote: crucible_core::PromptCacheResourceRemote,
    policy: PromptCachePolicy,
    now: u64,
) -> Result<(), PromptCacheResourceError> {
    match remote.state {
        PromptCacheResourceState::Ready => {
            let handle = remote
                .handle
                .or_else(|| record.handle().cloned())
                .ok_or(PromptCacheResourceError::InvalidMetadata)?;
            let expiry = remote
                .expires_at
                .ok_or(PromptCacheResourceError::InvalidMetadata)?;
            if !expiry_allowed(policy, now, expiry) {
                record.ready(handle, expiry, now);
                record.set_state(PromptCacheResourceState::Orphaned, now);
                return Err(PromptCacheResourceError::Rejected);
            }
            record.ready(handle, expiry, now);
        }
        state => record.set_state(state, now),
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ResourceAction {
    Resolve,
    Renew,
    Reconcile(PromptCacheResourceOperation),
}

impl ResourceAction {
    const fn operation(self) -> Option<PromptCacheResourceOperation> {
        match self {
            Self::Resolve => None,
            Self::Renew => Some(PromptCacheResourceOperation::Renew),
            Self::Reconcile(operation) => Some(operation),
        }
    }
}

fn expiry_allowed(policy: PromptCachePolicy, now: u64, expires_at: u64) -> bool {
    expires_at > now
        && policy
            .retention()
            .maximum_seconds()
            .is_none_or(|maximum| expires_at <= now.saturating_add(u64::from(maximum)))
}

pub(super) fn identity(
    inputs: &ScopeInputs<'_>,
    prefix: PromptCacheFingerprint,
) -> PromptCacheIdentity {
    let mut hash = Sha256::new();
    field(&mut hash, 0, b"crucible.prompt-cache.scope.v1");
    field(&mut hash, 1, inputs.route.protocol.as_bytes());
    field(&mut hash, 2, inputs.route.endpoint.as_bytes());
    field(&mut hash, 3, &[u8::from(inputs.route.custom_endpoint)]);
    field(&mut hash, 4, &inputs.route.credential_scope.bytes());
    optional(&mut hash, 5, inputs.route.account);
    optional(&mut hash, 6, inputs.route.project);
    field(&mut hash, 7, inputs.route.request_shape_version.as_bytes());
    field(&mut hash, 8, inputs.model.as_bytes());
    optional(&mut hash, 9, inputs.model_revision);
    field(&mut hash, 10, &inputs.max_tokens.to_be_bytes());
    optional(&mut hash, 11, inputs.effort.map(Effort::as_str));
    policy(&mut hash, inputs.policy);

    match inputs.policy.isolation() {
        PromptCacheIsolation::Run => field(&mut hash, 30, inputs.run.to_string().as_bytes()),
        PromptCacheIsolation::Session => {
            if let Some(session) = inputs.session {
                field(&mut hash, 31, session.as_bytes());
            } else {
                // A run without a durable conversation cannot safely claim
                // session sharing; its effective scope narrows to this run.
                field(&mut hash, 30, inputs.run.to_string().as_bytes());
            }
        }
        PromptCacheIsolation::Workspace => field(&mut hash, 32, inputs.workspace),
        PromptCacheIsolation::User => field(&mut hash, 33, inputs.user),
    }

    field(&mut hash, 34, inputs.trust);
    field(&mut hash, 35, inputs.authority);
    field(&mut hash, 36, inputs.instructions);
    field(&mut hash, 37, inputs.tool_generation.as_bytes());

    PromptCacheIdentity::new(
        crucible_core::PromptCacheScopeDigest::new(hash.finalize().into()),
        prefix,
        inputs.route.request_shape_version,
    )
}

/// Route and credential identity that bounds lifecycle authority.
pub(super) fn provider_scope(route: PromptCacheRoute<'_>) -> crucible_core::PromptCacheScopeDigest {
    let mut hash = Sha256::new();
    field(&mut hash, 0, b"crucible.prompt-cache.provider-scope.v1");
    field(&mut hash, 1, route.protocol.as_bytes());
    field(&mut hash, 2, route.endpoint.as_bytes());
    field(&mut hash, 3, &[u8::from(route.custom_endpoint)]);
    field(&mut hash, 4, &route.credential_scope.bytes());
    optional(&mut hash, 5, route.account);
    optional(&mut hash, 6, route.project);
    crucible_core::PromptCacheScopeDigest::new(hash.finalize().into())
}

/// Exact owner allowed to retire an exclusive persistent resource.
pub(super) fn owner_scope(inputs: &ScopeInputs<'_>) -> crucible_core::PromptCacheScopeDigest {
    let mut hash = Sha256::new();
    field(&mut hash, 0, b"crucible.prompt-cache.owner-scope.v1");
    field(&mut hash, 1, &provider_scope(inputs.route).bytes());
    field(&mut hash, 2, inputs.policy.isolation().as_str().as_bytes());
    match inputs.policy.namespace() {
        Some(namespace) => field(&mut hash, 3, namespace.as_str().as_bytes()),
        None => field(&mut hash, 3, &[]),
    }
    match inputs.policy.isolation() {
        PromptCacheIsolation::Run => field(&mut hash, 10, inputs.run.to_string().as_bytes()),
        PromptCacheIsolation::Session => {
            if let Some(session) = inputs.session {
                field(&mut hash, 11, session.as_bytes());
            } else {
                field(&mut hash, 10, inputs.run.to_string().as_bytes());
            }
        }
        PromptCacheIsolation::Workspace => field(&mut hash, 12, inputs.workspace),
        PromptCacheIsolation::User => field(&mut hash, 13, inputs.user),
    }
    field(&mut hash, 14, inputs.trust);
    field(&mut hash, 15, inputs.authority);
    crucible_core::PromptCacheScopeDigest::new(hash.finalize().into())
}

pub(super) fn routing_key(identity: PromptCacheIdentity, maximum: usize) -> PromptCacheKey {
    let mut hash = Sha256::new();
    field(&mut hash, 0, b"crucible.prompt-cache.routing-key.v1");
    field(&mut hash, 1, &identity.scope().bytes());
    field(&mut hash, 2, identity.request_shape_version().as_bytes());
    PromptCacheKey::from_digest(hash.finalize().into(), maximum)
}

fn policy_digest(value: PromptCachePolicy) -> PromptCachePolicyDigest {
    let mut hash = Sha256::new();
    field(&mut hash, 0, b"crucible.prompt-cache.policy.v1");
    policy(&mut hash, value);
    PromptCachePolicyDigest::new(hash.finalize().into())
}

fn policy(hash: &mut Sha256, policy: PromptCachePolicy) {
    field(hash, 12, policy.version().as_str().as_bytes());
    field(hash, 13, policy.mode().as_str().as_bytes());
    field(hash, 14, policy.isolation().as_str().as_bytes());
    field(hash, 15, policy.retention().class().as_str().as_bytes());
    optional_u32(hash, 16, policy.retention().maximum_seconds());
    field(hash, 17, policy.persistent_resources().as_str().as_bytes());
    match policy.namespace() {
        Some(namespace) => field(hash, 18, namespace.as_str().as_bytes()),
        None => field(hash, 18, &[]),
    }

    for (tag, mechanism) in [19_u8, 20, 21, 22].into_iter().zip([
        PromptCacheMechanism::ProviderManagedUsageOnly,
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheMechanism::PersistentContent,
    ]) {
        field(
            hash,
            tag,
            &[u8::from(policy.allowed_mechanisms().contains(mechanism))],
        );
    }

    let sources = policy.sources();
    for (tag, source) in [23_u8, 24, 25, 26, 27, 28].into_iter().zip([
        sources.mode(),
        sources.mechanisms(),
        sources.isolation(),
        sources.retention(),
        sources.persistent_resources(),
        sources.namespace(),
    ]) {
        field(hash, tag, &[source_code(source)]);
    }
}

const fn source_code(source: PromptCachePolicySource) -> u8 {
    match source {
        PromptCachePolicySource::Default => 0,
        PromptCachePolicySource::User => 1,
        PromptCachePolicySource::Workspace => 2,
        PromptCachePolicySource::Run => 3,
    }
}

fn optional(hash: &mut Sha256, tag: u8, value: Option<&str>) {
    match value {
        Some(value) => field(hash, tag, value.as_bytes()),
        None => field(hash, tag, &[]),
    }
}

fn optional_u32(hash: &mut Sha256, tag: u8, value: Option<u32>) {
    match value {
        Some(value) => field(hash, tag, &value.to_be_bytes()),
        None => field(hash, tag, &[]),
    }
}

fn field(hash: &mut Sha256, tag: u8, value: &[u8]) {
    hash.update([tag]);
    hash.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hash.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_core::{
        CredentialScopeId, Message, PromptCacheContent, PromptCacheIsolation,
        PromptCacheMechanismCapability, PromptCachePersistentMode, PromptCacheProvenance,
        PromptCacheResourceCreate, PromptCacheResourceCreated, PromptCacheResourceDeadline,
        PromptCacheResourceError, PromptCacheResourceLifecycle, PromptCacheResourceRecord,
        PromptCacheResourceRemote, PromptCacheResourceState, PromptCacheResourceStore,
        PromptCacheUsageReporting, StatefulTransportCapability, Transcript,
    };
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn inputs(session: Option<&str>) -> ScopeInputs<'_> {
        ScopeInputs {
            route: PromptCacheRoute {
                protocol: "fixture",
                endpoint: "https://provider.invalid/v1",
                custom_endpoint: false,
                credential_scope: CredentialScopeId::new(),
                account: Some("account-a"),
                project: Some("project-a"),
                request_shape_version: "fixture-request-v1",
            },
            policy: PromptCachePolicy::default(),
            model: "model-a",
            model_revision: Some("revision-a"),
            max_tokens: 2048,
            effort: Some(Effort::High),
            run: RunId::new(),
            session,
            workspace: b"workspace-a",
            user: b"user-a",
            trust: b"trusted",
            authority: b"authority-a",
            instructions: b"instructions-a",
            tool_generation: "tools-a",
        }
    }

    fn changed(base: &ScopeInputs<'_>, change: impl FnOnce(&mut ScopeInputs<'_>)) {
        let prefix = PromptCacheFingerprint::new([0x42; 32]);
        let before = identity(base, prefix);
        let mut after = *base;
        change(&mut after);
        assert_ne!(before, identity(&after, prefix));
    }

    #[test]
    fn every_provider_request_and_authority_axis_forks_identity() {
        let base = inputs(Some("session-a"));
        changed(&base, |one| one.route.protocol = "other");
        changed(&base, |one| one.route.endpoint = "https://other.invalid/v1");
        changed(&base, |one| one.route.custom_endpoint = true);
        changed(&base, |one| {
            one.route.credential_scope = CredentialScopeId::new();
        });
        changed(&base, |one| one.route.account = Some("account-b"));
        changed(&base, |one| one.route.project = Some("project-b"));
        changed(&base, |one| {
            one.route.request_shape_version = "fixture-request-v2";
        });
        changed(&base, |one| one.model = "model-b");
        changed(&base, |one| one.model_revision = Some("revision-b"));
        changed(&base, |one| one.max_tokens = 4096);
        changed(&base, |one| one.effort = Some(Effort::Low));
        changed(&base, |one| one.authority = b"authority-b");
        changed(&base, |one| one.instructions = b"instructions-b");
        changed(&base, |one| one.tool_generation = "tools-b");
        changed(&base, |one| {
            one.policy = one
                .policy
                .with_namespace(crucible_core::PromptCacheNamespace::new("separate").unwrap());
        });
    }

    #[test]
    fn selected_isolation_axis_is_exact_and_a_missing_session_falls_back_to_run() {
        let prefix = PromptCacheFingerprint::new([0x24; 32]);

        let session = inputs(Some("session-a"));
        let mut other_run = session;
        other_run.run = RunId::new();
        assert_eq!(identity(&session, prefix), identity(&other_run, prefix));
        changed(&session, |one| one.session = Some("session-b"));

        let mut run = session;
        run.policy = run.policy.with_isolation(PromptCacheIsolation::Run);
        changed(&run, |one| one.run = RunId::new());

        let mut workspace = session;
        workspace.policy = workspace
            .policy
            .with_isolation(PromptCacheIsolation::Workspace);
        changed(&workspace, |one| one.workspace = b"workspace-b");

        let mut user = session;
        user.policy = user.policy.with_isolation(PromptCacheIsolation::User);
        changed(&user, |one| one.user = b"user-b");

        let no_session = inputs(None);
        changed(&no_session, |one| one.run = RunId::new());
    }

    #[test]
    fn stable_prefix_and_trust_domain_fork_identity_but_not_the_session_routing_key() {
        let inputs = inputs(Some("session-a"));
        let first = identity(&inputs, PromptCacheFingerprint::new([1; 32]));
        let second = identity(&inputs, PromptCacheFingerprint::new([2; 32]));
        assert_ne!(first, second);
        assert_eq!(routing_key(first, 32), routing_key(second, 32));
        changed(&inputs, |one| one.trust = b"untrusted");

        let key = routing_key(first, 32);
        assert_eq!(key.as_str().len(), 32);
        assert!(key.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(format!("{key:?}"), "PromptCacheKey([redacted])");
    }

    #[test]
    fn default_prefer_derives_a_session_scoped_routing_key_when_supported() {
        let mut transcript = Transcript::new();
        transcript.push(Message::said("hello"));
        let request = Request {
            model: "model-a",
            transcript: &transcript,
            tools: &[],
            attached: &[],
            max_tokens: 2_048,
            system: Some("stable instructions"),
            effort: None,
            prompt_cache: None,
        };
        let capabilities = PromptCacheCapabilities::supported(
            "fixture-v1",
            Some("model-a"),
            PromptCacheProvenance::new(
                "https://provider.invalid/prompt-cache",
                "2026-08-31",
                "fixture-v1",
            ),
            StatefulTransportCapability::Unsupported,
            &[PromptCacheMechanismCapability::automatic_prefix(
                0,
                true,
                false,
                &[PromptCacheContent::Text],
            )],
            PromptCacheUsageReporting::ReadTokens,
        );

        let prepared = prepare(&request, capabilities.clone(), &inputs(Some("session-a"))).unwrap();

        let mut explicit = inputs(Some("session-a"));
        explicit.policy = explicit
            .policy
            .with_namespace(crucible_core::PromptCacheNamespace::new("shared-agent").unwrap());
        let explicitly_scoped = prepare(&request, capabilities, &explicit).unwrap();

        let Some(default_key) = prepared.routing_key else {
            panic!("default session isolation should derive an opaque routing key");
        };
        let Some(explicit_key) = explicitly_scoped.routing_key else {
            panic!("an explicit namespace should derive an opaque routing key");
        };
        assert_ne!(default_key, explicit_key);
    }

    #[derive(Debug, Default)]
    struct MemoryStore(Vec<PromptCacheResourceRecord>);

    fn only_record(store: &MemoryStore) -> &PromptCacheResourceRecord {
        let [record] = store.0.as_slice() else {
            panic!("expected exactly one resource record");
        };
        record
    }

    impl PromptCacheResourceStore for MemoryStore {
        fn matching(
            &mut self,
            binding: &crucible_core::PromptCacheResourceBinding,
        ) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError> {
            Ok(self
                .0
                .iter()
                .rev()
                .find(|record| record.binding() == binding)
                .cloned())
        }

        fn put(
            &mut self,
            record: &PromptCacheResourceRecord,
        ) -> Result<(), PromptCacheResourceError> {
            if let Some(found) = self.0.iter_mut().find(|found| found.id() == record.id()) {
                *found = record.clone();
            } else {
                self.0.push(record.clone());
            }
            Ok(())
        }

        fn remove(
            &mut self,
            id: &crucible_core::PromptCacheResourceId,
        ) -> Result<(), PromptCacheResourceError> {
            self.0.retain(|record| record.id() != id);
            Ok(())
        }

        fn inspect(
            &mut self,
            maximum: usize,
        ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
            Ok(self.0.iter().take(maximum).cloned().collect())
        }
    }

    struct PersistentFixture;

    impl PromptCacheResourceLifecycle for PersistentFixture {
        fn create(
            &self,
            request: PromptCacheResourceCreate<'_>,
            cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceCreated, PromptCacheResourceError> {
            if cancel.requested() {
                return Err(PromptCacheResourceError::Cancelled);
            }
            assert!(!request.deadline.expired());
            Ok(PromptCacheResourceCreated {
                handle: crucible_core::PromptCacheResourceHandle::new("remote-fixture").unwrap(),
                expires_at: 1_600,
            })
        }

        fn resolve(
            &self,
            record: &PromptCacheResourceRecord,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            Ok(PromptCacheResourceRemote {
                handle: record.handle().cloned(),
                state: PromptCacheResourceState::Ready,
                expires_at: record.expires_at(),
            })
        }

        fn renew(
            &self,
            record: &PromptCacheResourceRecord,
            _retention: crucible_core::PromptCacheRetention,
            deadline: PromptCacheResourceDeadline,
            cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            self.resolve(record, deadline, cancel)
        }

        fn delete(
            &self,
            _record: &PromptCacheResourceRecord,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            Ok(PromptCacheResourceRemote {
                handle: None,
                state: PromptCacheResourceState::Deleted,
                expires_at: None,
            })
        }

        fn reconcile(
            &self,
            record: &PromptCacheResourceRecord,
            deadline: PromptCacheResourceDeadline,
            cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            self.resolve(record, deadline, cancel)
        }

        fn inspect(
            &self,
            record: &PromptCacheResourceRecord,
            deadline: PromptCacheResourceDeadline,
            cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            self.resolve(record, deadline, cancel)
        }
    }

    #[test]
    fn authorized_persistent_content_is_created_before_it_is_referenced() {
        let mut transcript = Transcript::new();
        transcript.push(Message::said("a stable prefix long enough"));
        let request = Request {
            model: "model-a",
            transcript: &transcript,
            tools: &[],
            attached: &[],
            max_tokens: 2_048,
            system: Some("stable instructions"),
            effort: None,
            prompt_cache: None,
        };
        let capabilities = PromptCacheCapabilities::supported(
            "persistent-fixture-v1",
            Some("revision-a"),
            PromptCacheProvenance::new(
                "https://provider.invalid/persistent-cache",
                "2026-08-31",
                "persistent-fixture-v1",
            ),
            StatefulTransportCapability::Unsupported,
            &[PromptCacheMechanismCapability::persistent_content(
                1,
                &[PromptCacheContent::Text],
            )],
            PromptCacheUsageReporting::ReadAndWriteTokens,
        );
        let mut scope = inputs(Some("session-a"));
        scope.policy = scope
            .policy
            .with_persistent_resources(PromptCachePersistentMode::Create);
        let mut store = MemoryStore::default();
        let cancel = crucible_core::Cancel::new();

        let prepared = prepare_with_resources(
            &request,
            capabilities,
            &scope,
            ResourceInputs {
                store: &mut store,
                lifecycle: &PersistentFixture,
                cancel: &cancel,
                now: 1_000,
                deadline: Instant::now() + Duration::from_secs(1),
            },
        )
        .unwrap();

        assert_eq!(
            prepared
                .selection
                .selected()
                .map(crucible_core::PromptCacheSelected::mechanism),
            Some(PromptCacheMechanism::PersistentContent)
        );
        assert!(prepared.request().resource.is_some());
        assert_eq!(store.0.len(), 1);
        assert_eq!(only_record(&store).state(), PromptCacheResourceState::Ready);
    }

    #[derive(Debug, Clone, Copy)]
    enum CreateReply {
        Ready(u64),
        Rejected,
        Cancelled,
    }

    #[derive(Debug, Clone, Copy)]
    enum RemoteReply {
        Ready(u64),
        Expiring(u64),
        Deleted,
        Cancelled,
    }

    #[derive(Debug)]
    struct LifecycleFixture {
        create: CreateReply,
        resolve: RemoteReply,
        renew: RemoteReply,
        reconcile: RemoteReply,
        calls: Mutex<Vec<PromptCacheResourceOperation>>,
    }

    impl LifecycleFixture {
        fn new(create: CreateReply, resolve: RemoteReply, renew: RemoteReply) -> Self {
            Self {
                create,
                resolve,
                renew,
                reconcile: RemoteReply::Ready(1_600),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn reconciling(mut self, reply: RemoteReply) -> Self {
            self.reconcile = reply;
            self
        }

        fn calls(&self) -> Vec<PromptCacheResourceOperation> {
            self.calls.lock().unwrap().clone()
        }

        fn remote(
            reply: RemoteReply,
            record: &PromptCacheResourceRecord,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            match reply {
                RemoteReply::Ready(expires_at) => Ok(PromptCacheResourceRemote {
                    handle: record.handle().cloned().or_else(|| {
                        Some(
                            crucible_core::PromptCacheResourceHandle::new("reconciled-fixture")
                                .unwrap(),
                        )
                    }),
                    state: PromptCacheResourceState::Ready,
                    expires_at: Some(expires_at),
                }),
                RemoteReply::Expiring(expires_at) => Ok(PromptCacheResourceRemote {
                    handle: record.handle().cloned(),
                    state: PromptCacheResourceState::Expiring,
                    expires_at: Some(expires_at),
                }),
                RemoteReply::Deleted => Ok(PromptCacheResourceRemote {
                    handle: None,
                    state: PromptCacheResourceState::Deleted,
                    expires_at: None,
                }),
                RemoteReply::Cancelled => Err(PromptCacheResourceError::Cancelled),
            }
        }
    }

    impl PromptCacheResourceLifecycle for LifecycleFixture {
        fn create(
            &self,
            _request: PromptCacheResourceCreate<'_>,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceCreated, PromptCacheResourceError> {
            self.calls
                .lock()
                .unwrap()
                .push(PromptCacheResourceOperation::Create);
            match self.create {
                CreateReply::Ready(expires_at) => Ok(PromptCacheResourceCreated {
                    handle: crucible_core::PromptCacheResourceHandle::new("created-fixture")
                        .unwrap(),
                    expires_at,
                }),
                CreateReply::Rejected => Err(PromptCacheResourceError::Rejected),
                CreateReply::Cancelled => Err(PromptCacheResourceError::Cancelled),
            }
        }

        fn resolve(
            &self,
            record: &PromptCacheResourceRecord,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            Self::remote(self.resolve, record)
        }

        fn renew(
            &self,
            record: &PromptCacheResourceRecord,
            _retention: crucible_core::PromptCacheRetention,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            self.calls
                .lock()
                .unwrap()
                .push(PromptCacheResourceOperation::Renew);
            Self::remote(self.renew, record)
        }

        fn delete(
            &self,
            record: &PromptCacheResourceRecord,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            self.calls
                .lock()
                .unwrap()
                .push(PromptCacheResourceOperation::Delete);
            Self::remote(RemoteReply::Deleted, record)
        }

        fn reconcile(
            &self,
            record: &PromptCacheResourceRecord,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            if let Some(operation) = record.pending() {
                self.calls.lock().unwrap().push(operation);
            }
            Self::remote(self.reconcile, record)
        }

        fn inspect(
            &self,
            record: &PromptCacheResourceRecord,
            _deadline: PromptCacheResourceDeadline,
            _cancel: &crucible_core::Cancel,
        ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
            Self::remote(self.resolve, record)
        }
    }

    fn with_persistent_request<T>(run: impl FnOnce(&Request<'_>) -> T) -> T {
        let mut transcript = Transcript::new();
        transcript.push(Message::said("a stable prefix long enough"));
        let request = Request {
            model: "model-a",
            transcript: &transcript,
            tools: &[],
            attached: &[],
            max_tokens: 2_048,
            system: Some("stable instructions"),
            effort: None,
            prompt_cache: None,
        };
        run(&request)
    }

    fn persistent_capabilities() -> PromptCacheCapabilities {
        PromptCacheCapabilities::supported(
            "persistent-fixture-v1",
            Some("revision-a"),
            PromptCacheProvenance::new(
                "https://provider.invalid/persistent-cache",
                "2026-08-31",
                "persistent-fixture-v1",
            ),
            StatefulTransportCapability::Unsupported,
            &[PromptCacheMechanismCapability::persistent_content(
                1,
                &[PromptCacheContent::Text],
            )],
            PromptCacheUsageReporting::ReadAndWriteTokens,
        )
    }

    fn creating_scope(session: Option<&str>) -> ScopeInputs<'_> {
        let mut scope = inputs(session);
        scope.policy = scope
            .policy
            .with_persistent_resources(PromptCachePersistentMode::Create);
        scope
    }

    fn prepared_with(
        request: &Request<'_>,
        scope: &ScopeInputs<'_>,
        store: &mut MemoryStore,
        lifecycle: &dyn PromptCacheResourceLifecycle,
        now: u64,
    ) -> Result<Prepared, TurnError> {
        prepare_with_resources(
            request,
            persistent_capabilities(),
            scope,
            ResourceInputs {
                store,
                lifecycle,
                cancel: &crucible_core::Cancel::new(),
                now,
                deadline: Instant::now() + Duration::from_secs(1),
            },
        )
    }

    #[test]
    fn an_exact_ready_resource_is_resolved_and_reused_without_another_create() {
        with_persistent_request(|request| {
            let scope = creating_scope(Some("session-a"));
            let lifecycle = LifecycleFixture::new(
                CreateReply::Ready(1_600),
                RemoteReply::Ready(1_600),
                RemoteReply::Ready(1_700),
            );
            let mut store = MemoryStore::default();

            let first = prepared_with(request, &scope, &mut store, &lifecycle, 1_000).unwrap();
            let second = prepared_with(request, &scope, &mut store, &lifecycle, 1_100).unwrap();

            assert_eq!(
                first.resource.as_ref().unwrap().id(),
                second.resource.as_ref().unwrap().id()
            );
            assert_eq!(lifecycle.calls(), [PromptCacheResourceOperation::Create]);
            assert_eq!(store.0.len(), 1);
        });
    }

    #[test]
    fn model_scope_and_prefix_mismatches_never_reuse_a_resource() {
        with_persistent_request(|request| {
            let scope = creating_scope(Some("session-a"));
            let lifecycle = LifecycleFixture::new(
                CreateReply::Ready(1_600),
                RemoteReply::Ready(1_600),
                RemoteReply::Ready(1_700),
            );
            let mut store = MemoryStore::default();
            prepared_with(request, &scope, &mut store, &lifecycle, 1_000).unwrap();

            let mut other_model = scope;
            other_model.model = "model-b";
            prepared_with(request, &other_model, &mut store, &lifecycle, 1_010).unwrap();

            let mut other_scope = scope;
            other_scope.route.credential_scope = CredentialScopeId::new();
            prepared_with(request, &other_scope, &mut store, &lifecycle, 1_020).unwrap();

            let mut other_transcript = Transcript::new();
            other_transcript.push(Message::said("a different stable prefix long enough"));
            let other_request = Request {
                transcript: &other_transcript,
                system: Some("different stable instructions"),
                ..*request
            };
            prepared_with(&other_request, &scope, &mut store, &lifecycle, 1_030).unwrap();

            assert_eq!(store.0.len(), 4);
            assert_eq!(
                lifecycle.calls(),
                [
                    PromptCacheResourceOperation::Create,
                    PromptCacheResourceOperation::Create,
                    PromptCacheResourceOperation::Create,
                    PromptCacheResourceOperation::Create,
                ]
            );
        });
    }

    #[test]
    fn prefer_falls_back_after_a_rejected_create_but_require_fails_closed() {
        with_persistent_request(|request| {
            let lifecycle = LifecycleFixture::new(
                CreateReply::Rejected,
                RemoteReply::Ready(1_600),
                RemoteReply::Ready(1_700),
            );
            let mut store = MemoryStore::default();
            let preferred = prepared_with(
                request,
                &creating_scope(Some("session-a")),
                &mut store,
                &lifecycle,
                1_000,
            )
            .unwrap();
            assert!(preferred.resource.is_none());
            assert!(preferred.selection.selected().is_none());
            assert!(store.0.is_empty());

            let mut required = creating_scope(Some("session-a"));
            required.policy = required
                .policy
                .with_persistent_resources(PromptCachePersistentMode::Require);
            assert!(prepared_with(request, &required, &mut store, &lifecycle, 1_000).is_err());
            assert!(store.0.is_empty());
        });
    }

    #[test]
    fn an_expiring_resource_is_renewed_before_the_same_request_references_it() {
        with_persistent_request(|request| {
            let scope = creating_scope(Some("session-a"));
            let lifecycle = LifecycleFixture::new(
                CreateReply::Ready(1_600),
                RemoteReply::Expiring(1_200),
                RemoteReply::Ready(1_700),
            );
            let mut store = MemoryStore::default();
            prepared_with(request, &scope, &mut store, &lifecycle, 1_000).unwrap();

            let renewed = prepared_with(request, &scope, &mut store, &lifecycle, 1_100).unwrap();

            assert!(renewed.request().resource.is_some());
            assert_eq!(renewed.resource.unwrap().expires_at(), Some(1_700));
            assert_eq!(
                lifecycle.calls(),
                [
                    PromptCacheResourceOperation::Create,
                    PromptCacheResourceOperation::Renew,
                ]
            );
        });
    }

    #[test]
    fn an_expired_exclusive_resource_is_deleted_before_its_replacement() {
        with_persistent_request(|request| {
            let scope = creating_scope(Some("session-a"));
            let creator = LifecycleFixture::new(
                CreateReply::Ready(1_200),
                RemoteReply::Ready(1_200),
                RemoteReply::Ready(1_300),
            );
            let mut store = MemoryStore::default();
            prepared_with(request, &scope, &mut store, &creator, 1_000).unwrap();

            let replacement = LifecycleFixture::new(
                CreateReply::Ready(1_700),
                RemoteReply::Ready(1_700),
                RemoteReply::Ready(1_800),
            );
            let prepared = prepared_with(request, &scope, &mut store, &replacement, 1_200).unwrap();

            assert!(prepared.request().resource.is_some());
            assert_eq!(
                replacement.calls(),
                [
                    PromptCacheResourceOperation::Delete,
                    PromptCacheResourceOperation::Create,
                ]
            );
            assert_eq!(store.0.len(), 1);
            assert_eq!(only_record(&store).state(), PromptCacheResourceState::Ready);
            assert_eq!(only_record(&store).expires_at(), Some(1_700));
        });
    }

    #[test]
    fn a_renewal_beyond_the_user_ceiling_is_durably_orphaned_and_not_reused() {
        with_persistent_request(|request| {
            let mut scope = creating_scope(Some("session-a"));
            scope.policy = scope
                .policy
                .with_retention(crucible_core::PromptCacheRetention::extended(300).unwrap());
            let lifecycle = LifecycleFixture::new(
                CreateReply::Ready(1_250),
                RemoteReply::Expiring(1_200),
                RemoteReply::Ready(2_000),
            );
            let mut store = MemoryStore::default();
            prepared_with(request, &scope, &mut store, &lifecycle, 1_000).unwrap();

            let fallback = prepared_with(request, &scope, &mut store, &lifecycle, 1_100).unwrap();

            assert!(fallback.resource.is_none());
            assert_eq!(store.0.len(), 1);
            assert_eq!(
                only_record(&store).state(),
                PromptCacheResourceState::Orphaned
            );
        });
    }

    #[test]
    fn a_replacement_after_an_orphan_is_the_record_reused_on_later_requests() {
        with_persistent_request(|request| {
            let mut scope = creating_scope(Some("session-a"));
            scope.policy = scope
                .policy
                .with_retention(crucible_core::PromptCacheRetention::extended(300).unwrap());
            let first = LifecycleFixture::new(
                CreateReply::Ready(1_250),
                RemoteReply::Expiring(1_200),
                RemoteReply::Ready(2_000),
            );
            let mut store = MemoryStore::default();
            prepared_with(request, &scope, &mut store, &first, 1_000).unwrap();
            prepared_with(request, &scope, &mut store, &first, 1_100).unwrap();
            assert_eq!(
                only_record(&store).state(),
                PromptCacheResourceState::Orphaned
            );

            let replacement = LifecycleFixture::new(
                CreateReply::Ready(1_400),
                RemoteReply::Ready(1_400),
                RemoteReply::Ready(1_500),
            );
            let created = prepared_with(request, &scope, &mut store, &replacement, 1_110).unwrap();
            let reused = prepared_with(request, &scope, &mut store, &replacement, 1_120).unwrap();

            assert_eq!(store.0.len(), 2);
            assert_eq!(
                store.0.first().map(PromptCacheResourceRecord::state),
                Some(PromptCacheResourceState::Orphaned)
            );
            assert_eq!(
                created.resource.as_ref().unwrap().id(),
                reused.resource.as_ref().unwrap().id()
            );
            assert_eq!(replacement.calls(), [PromptCacheResourceOperation::Create]);
        });
    }

    #[test]
    fn an_ambiguous_create_is_reconciled_after_restart_before_any_new_create() {
        with_persistent_request(|request| {
            let scope = creating_scope(Some("session-a"));
            let cancelled = LifecycleFixture::new(
                CreateReply::Cancelled,
                RemoteReply::Ready(1_600),
                RemoteReply::Ready(1_700),
            );
            let mut store = MemoryStore::default();
            assert!(matches!(
                prepared_with(request, &scope, &mut store, &cancelled, 1_000),
                Err(TurnError::PromptCacheResource(
                    PromptCacheResourceError::Cancelled
                ))
            ));
            assert_eq!(
                only_record(&store).state(),
                PromptCacheResourceState::Ambiguous
            );

            let resumed = LifecycleFixture::new(
                CreateReply::Rejected,
                RemoteReply::Ready(1_600),
                RemoteReply::Ready(1_700),
            )
            .reconciling(RemoteReply::Ready(1_600));
            let prepared = prepared_with(request, &scope, &mut store, &resumed, 1_010).unwrap();

            assert!(prepared.request().resource.is_some());
            assert_eq!(resumed.calls(), [PromptCacheResourceOperation::Create]);
            assert_eq!(only_record(&store).state(), PromptCacheResourceState::Ready);
        });
    }

    #[test]
    fn a_cancelled_read_only_resolve_does_not_relabel_a_ready_resource_as_ambiguous_renewal() {
        with_persistent_request(|request| {
            let scope = creating_scope(Some("session-a"));
            let creator = LifecycleFixture::new(
                CreateReply::Ready(1_600),
                RemoteReply::Ready(1_600),
                RemoteReply::Ready(1_700),
            );
            let mut store = MemoryStore::default();
            prepared_with(request, &scope, &mut store, &creator, 1_000).unwrap();
            let resolver = LifecycleFixture::new(
                CreateReply::Rejected,
                RemoteReply::Cancelled,
                RemoteReply::Ready(1_700),
            );

            assert!(matches!(
                prepared_with(request, &scope, &mut store, &resolver, 1_100),
                Err(TurnError::PromptCacheResource(
                    PromptCacheResourceError::Cancelled
                ))
            ));
            assert_eq!(only_record(&store).state(), PromptCacheResourceState::Ready);
            assert_eq!(only_record(&store).pending(), None);
        });
    }
}
