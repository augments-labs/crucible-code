//! Explicit passes over the private prompt-cache resource store.
//!
//! Separate from the turn loop because it is not part of one. A cleanup is
//! something the user asks for, or something a change of model, provider or
//! credential requires before it can go ahead; either way it walks the whole
//! store once and stops, where a turn asks and answers and comes back.
//!
//! What every pass here holds to is that a resource is only forgotten locally
//! once the provider has said it is gone. A deletion the provider could not
//! confirm leaves the record behind marked for what it is — ambiguous, or
//! orphaned — because a handle to something that may still exist remotely is
//! worth more than a tidy store.

use crucible_core::{
    Cancel, PromptCacheResourceDeadline, PromptCacheResourceError, PromptCacheResourceOperation,
    PromptCacheResourceRecord, PromptCacheResourceState, PromptCacheScopeDigest,
};

use crate::prompt_cache;

use super::{PROMPT_CACHE_RESOURCE_DEADLINE, Runner, unix_now};

/// Result of one explicit bounded persistent-resource cleanup pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromptCacheCleanup {
    /// Records owned by the current provider lifecycle that were considered.
    pub inspected: usize,
    /// Remote deletions confirmed and removed locally.
    pub deleted: usize,
    /// Operations whose provider result remains ambiguous.
    pub ambiguous: usize,
    /// Records retained because remote deletion could not be confirmed.
    pub orphaned: usize,
    changes: Vec<crucible_core::PromptCacheResourceFact>,
}

#[derive(Clone, Copy)]
enum ResourceCleanupScope {
    Provider(PromptCacheScopeDigest),
    ExclusiveOwner(PromptCacheScopeDigest),
}

impl ResourceCleanupScope {
    fn includes(self, record: &PromptCacheResourceRecord) -> bool {
        match self {
            Self::Provider(scope) => record.binding().provider_scope() == scope,
            Self::ExclusiveOwner(scope) => {
                record.binding().owner_scope() == scope && record.binding().owner().exclusive()
            }
        }
    }
}

impl PromptCacheCleanup {
    /// Bounded immutable lifecycle changes from this explicit cleanup pass.
    #[must_use]
    pub fn changes(&self) -> &[crucible_core::PromptCacheResourceFact] {
        &self.changes
    }
}

impl Runner {
    /// Explicitly deletes every resource owned by the current provider
    /// lifecycle, retaining ambiguous or orphaned records for later recovery.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] when local metadata cannot be read or
    /// durably updated. Individual provider cleanup failures are represented in
    /// the returned counts and retained states.
    pub fn clean_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        let scope = ResourceCleanupScope::Provider(prompt_cache::provider_scope(
            self.provider.prompt_cache_route(),
        ));
        self.clean_prompt_cache_in(scope, cancel)
    }

    /// Retires exclusive persistent resources owned by this active run/session.
    ///
    /// This is the bounded transition used before changing model, provider, or
    /// credential. Workspace- and user-shared resources are deliberately left
    /// for explicit cleanup or expiry.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] when local metadata cannot be read or
    /// durably updated.
    pub fn retire_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        let Some(owner) = self.prompt_cache_owner_scope else {
            return Ok(PromptCacheCleanup::default());
        };
        let result =
            self.clean_prompt_cache_in(ResourceCleanupScope::ExclusiveOwner(owner), cancel);
        if result.is_ok() {
            self.prompt_cache_owner_scope = None;
        }
        result
    }

    fn clean_prompt_cache_in(
        &mut self,
        scope: ResourceCleanupScope,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        let Some(store) = self.prompt_cache_store.as_deref_mut() else {
            return Ok(PromptCacheCleanup::default());
        };
        let records = store.inspect(crucible_core::MAX_PROMPT_CACHE_RESOURCES)?;
        let Some(lifecycle) = self.provider.prompt_cache_resources() else {
            return if records.iter().any(|record| scope.includes(record)) {
                Err(PromptCacheResourceError::Unsupported)
            } else {
                Ok(PromptCacheCleanup::default())
            };
        };
        let mut result = PromptCacheCleanup::default();
        for mut record in records {
            if !scope.includes(&record) {
                continue;
            }
            result.inspected += 1;
            if cancel.requested() {
                return Err(PromptCacheResourceError::Cancelled);
            }
            let now = unix_now();

            if matches!(
                record.state(),
                PromptCacheResourceState::Creating
                    | PromptCacheResourceState::Expiring
                    | PromptCacheResourceState::Deleting
                    | PromptCacheResourceState::Ambiguous
            ) {
                let operation = record.pending();
                let deadline = PromptCacheResourceDeadline::new(
                    std::time::Instant::now() + PROMPT_CACHE_RESOURCE_DEADLINE,
                );
                match lifecycle.reconcile(&record, deadline, cancel) {
                    Ok(remote) => {
                        let _ = prompt_cache::apply_remote(
                            &mut record,
                            remote,
                            self.policy.prompt_cache,
                            now,
                        );
                        cleanup_change(&mut result, &record, operation);
                        if record.state() == PromptCacheResourceState::Deleted {
                            store.remove(record.id())?;
                            result.deleted += 1;
                            continue;
                        }
                        if !matches!(
                            record.state(),
                            PromptCacheResourceState::Ready
                                | PromptCacheResourceState::Expired
                                | PromptCacheResourceState::Orphaned
                        ) {
                            record.set_state(PromptCacheResourceState::Orphaned, now);
                            cleanup_change(&mut result, &record, operation);
                        }
                        store.put(&record)?;
                    }
                    Err(
                        PromptCacheResourceError::Ambiguous(_)
                        | PromptCacheResourceError::Cancelled
                        | PromptCacheResourceError::Deadline,
                    ) => {
                        if let Some(operation) = operation {
                            record.ambiguous(operation, now);
                        }
                        store.put(&record)?;
                        cleanup_change(&mut result, &record, operation);
                        result.ambiguous += 1;
                        continue;
                    }
                    Err(_) => {
                        record.set_state(PromptCacheResourceState::Orphaned, now);
                        store.put(&record)?;
                        cleanup_change(&mut result, &record, operation);
                        result.orphaned += 1;
                        continue;
                    }
                }
            }

            if record.state() == PromptCacheResourceState::Deleted {
                store.remove(record.id())?;
                result.deleted += 1;
                continue;
            }
            record.set_state(PromptCacheResourceState::Deleting, now);
            store.put(&record)?;
            cleanup_change(
                &mut result,
                &record,
                Some(PromptCacheResourceOperation::Delete),
            );
            let deadline = PromptCacheResourceDeadline::new(
                std::time::Instant::now() + PROMPT_CACHE_RESOURCE_DEADLINE,
            );
            match lifecycle.delete(&record, deadline, cancel) {
                Ok(remote) if remote.state == PromptCacheResourceState::Deleted => {
                    record.set_state(PromptCacheResourceState::Deleted, now);
                    cleanup_change(
                        &mut result,
                        &record,
                        Some(PromptCacheResourceOperation::Delete),
                    );
                    store.remove(record.id())?;
                    result.deleted += 1;
                }
                Ok(_remote) => {
                    // A bounded delete pass that conclusively leaves the
                    // resource present has exhausted its one attempt. Keep the
                    // handle privately for later recovery, but report the
                    // durable state honestly as orphaned.
                    record.set_state(PromptCacheResourceState::Orphaned, now);
                    store.put(&record)?;
                    cleanup_change(
                        &mut result,
                        &record,
                        Some(PromptCacheResourceOperation::Delete),
                    );
                    result.orphaned += 1;
                }
                Err(
                    PromptCacheResourceError::Ambiguous(_)
                    | PromptCacheResourceError::Cancelled
                    | PromptCacheResourceError::Deadline,
                ) => {
                    record.ambiguous(PromptCacheResourceOperation::Delete, now);
                    store.put(&record)?;
                    cleanup_change(
                        &mut result,
                        &record,
                        Some(PromptCacheResourceOperation::Delete),
                    );
                    result.ambiguous += 1;
                }
                Err(_) => {
                    record.set_state(PromptCacheResourceState::Orphaned, now);
                    store.put(&record)?;
                    cleanup_change(
                        &mut result,
                        &record,
                        Some(PromptCacheResourceOperation::Delete),
                    );
                    result.orphaned += 1;
                }
            }
        }
        Ok(result)
    }
}

fn cleanup_change(
    cleanup: &mut PromptCacheCleanup,
    record: &PromptCacheResourceRecord,
    operation: Option<PromptCacheResourceOperation>,
) {
    cleanup
        .changes
        .push(crucible_core::PromptCacheResourceFact {
            attempt: None,
            resource: record.id().clone(),
            operation,
            state: record.state(),
            expires_at: record.expires_at(),
            owner: record.binding().owner(),
        });
}
