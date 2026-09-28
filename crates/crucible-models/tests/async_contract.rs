//! A provider written outside this crate, driven the way a turn drives one.
//!
//! This file is a crate of its own and sees only what `crucible-models`
//! exports, so what compiles here is what an adapter outside the tree can
//! write: a request that starts a stream, the deltas the stream answers with,
//! and the cache resources the adapter may keep, each reached through a trait
//! object and each handing back a future.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crucible_models::{
    Delta, DeltaStream, PromptCacheCapabilities, PromptCacheResourceCreate,
    PromptCacheResourceCreated, PromptCacheResourceDeadline, PromptCacheResourceLifecycle,
    PromptCacheResourceRemote, PromptCacheRoute, Provider, ProviderError, Request, RequestPurpose,
};
use crucible_runtime::{BoxFuture, Cancel, answered};
use crucible_types::{
    CredentialScopeId, Modalities, Modality, PromptCacheEncoding, PromptCacheFingerprint,
    PromptCacheIsolation, PromptCachePolicyDigest, PromptCacheResourceBinding,
    PromptCacheResourceError, PromptCacheResourceHandle, PromptCacheResourceId,
    PromptCacheResourceOwner, PromptCacheResourceRecord, PromptCacheResourceState,
    PromptCacheRetention, PromptCacheScopeDigest, StopReason, Transcript,
};

/// Answers every request with the name of the model it was asked for, and
/// refuses one whose cancel was asked first.
struct Parrot {
    scope: CredentialScopeId,
    kept: Kept,
}

impl Parrot {
    fn new() -> Self {
        Self {
            scope: CredentialScopeId::new(),
            kept: Kept::default(),
        }
    }
}

impl Provider for Parrot {
    fn name(&self) -> &'static str {
        "parrot"
    }

    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("an unreviewed adapter")
    }

    fn prompt_cache_resources(&self) -> Option<&dyn PromptCacheResourceLifecycle> {
        Some(&self.kept)
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "parrot",
            endpoint: "parrot",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "parrot-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            if cancel.requested() {
                return Err(ProviderError::Cancelled("parrot"));
            }
            // What the stream keeps is copied out of the request: it may keep
            // the response, and nothing the request lent it.
            let reply: Box<dyn DeltaStream> = Box::new(Reply {
                pending: vec![
                    Delta::Stopped(StopReason::Yielded),
                    Delta::Text(request.model.into()),
                ],
            });
            Ok(reply)
        })
    }
}

/// What the parrot still has to say, last delta first.
struct Reply {
    pending: Vec<Delta>,
}

impl DeltaStream for Reply {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(async move { self.pending.pop().map(Ok) })
    }
}

/// A resource lifecycle counting every operation it carries out.
#[derive(Default)]
struct Kept {
    answered: AtomicUsize,
}

impl Kept {
    fn remote<'a>(
        &'a self,
        cancel: &'a Cancel,
        state: PromptCacheResourceState,
    ) -> BoxFuture<'a, Result<PromptCacheResourceRemote, PromptCacheResourceError>> {
        Box::pin(async move {
            if cancel.requested() {
                return Err(PromptCacheResourceError::Cancelled);
            }
            self.answered.fetch_add(1, Ordering::SeqCst);
            Ok(PromptCacheResourceRemote {
                handle: None,
                state,
                expires_at: None,
            })
        })
    }
}

impl PromptCacheResourceLifecycle for Kept {
    fn create<'a>(
        &'a self,
        request: PromptCacheResourceCreate<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PromptCacheResourceCreated, PromptCacheResourceError>> {
        Box::pin(async move {
            if cancel.requested() {
                return Err(PromptCacheResourceError::Cancelled);
            }
            let handle = PromptCacheResourceHandle::new(format!("kept-{}", request.request.model))
                .map_err(|_| PromptCacheResourceError::InvalidMetadata)?;
            self.answered.fetch_add(1, Ordering::SeqCst);
            Ok(PromptCacheResourceCreated {
                handle,
                expires_at: 300,
            })
        })
    }

    fn resolve<'a>(
        &'a self,
        _record: &'a PromptCacheResourceRecord,
        _deadline: PromptCacheResourceDeadline,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PromptCacheResourceRemote, PromptCacheResourceError>> {
        self.remote(cancel, PromptCacheResourceState::Ready)
    }

    fn renew<'a>(
        &'a self,
        _record: &'a PromptCacheResourceRecord,
        _retention: PromptCacheRetention,
        _deadline: PromptCacheResourceDeadline,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PromptCacheResourceRemote, PromptCacheResourceError>> {
        self.remote(cancel, PromptCacheResourceState::Ready)
    }

    fn delete<'a>(
        &'a self,
        _record: &'a PromptCacheResourceRecord,
        _deadline: PromptCacheResourceDeadline,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PromptCacheResourceRemote, PromptCacheResourceError>> {
        self.remote(cancel, PromptCacheResourceState::Deleted)
    }

    fn reconcile<'a>(
        &'a self,
        _record: &'a PromptCacheResourceRecord,
        _deadline: PromptCacheResourceDeadline,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PromptCacheResourceRemote, PromptCacheResourceError>> {
        self.remote(cancel, PromptCacheResourceState::Ready)
    }

    fn inspect<'a>(
        &'a self,
        _record: &'a PromptCacheResourceRecord,
        _deadline: PromptCacheResourceDeadline,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PromptCacheResourceRemote, PromptCacheResourceError>> {
        self.remote(cancel, PromptCacheResourceState::Ready)
    }
}

/// A turn request with nothing in it but the model it names.
fn asking<'a>(model: &'a str, transcript: &'a Transcript) -> Request<'a> {
    Request {
        purpose: RequestPurpose::Turn,
        model,
        transcript,
        tools: &[],
        max_tokens: 16,
        system: None,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

/// Every delta a stream has left, read one at a time.
#[allow(clippy::expect_used)] // The parrot fails nothing, so a failure is the test's.
fn drained(stream: &mut dyn DeltaStream) -> Vec<Delta> {
    let mut heard = Vec::new();
    while let Some(delta) = answered!(stream.next()) {
        heard.push(delta.expect("the parrot fails nothing"));
    }
    heard
}

fn said_and_stopped(heard: &[Delta], model: &str) -> bool {
    matches!(
        heard,
        [Delta::Text(text), Delta::Stopped(StopReason::Yielded)] if &**text == model
    )
}

#[test]
fn an_external_provider_answers_through_trait_objects() {
    let provider: Box<dyn Provider> = Box::new(Parrot::new());
    let cancel = Cancel::new();

    let mut stream = {
        let transcript = Transcript::new();
        answered!(provider.stream(asking("model-a", &transcript), &cancel))
            .expect("the parrot answers")
    };

    // The transcript the request borrowed is gone; the stream it started is not.
    assert!(said_and_stopped(&drained(stream.as_mut()), "model-a"));

    cancel.request();
    let transcript = Transcript::new();
    let refused = answered!(provider.stream(asking("model-a", &transcript), &cancel));
    assert!(matches!(refused, Err(ProviderError::Cancelled("parrot"))));
}

#[test]
fn a_request_is_started_on_one_thread_and_read_on_another() {
    let provider: Box<dyn Provider> = Box::new(Parrot::new());
    let cancel = Cancel::new();
    let transcript = Transcript::new();
    let starting = provider.stream(asking("model-b", &transcript), &cancel);

    let started = std::thread::scope(|scope| {
        scope
            .spawn(move || answered!(starting))
            .join()
            .expect("the poll ends")
    });

    let mut stream = started.expect("the parrot answers");
    assert!(said_and_stopped(&drained(stream.as_mut()), "model-b"));
}

#[test]
fn an_external_cache_lifecycle_is_reached_through_its_provider() {
    let parrot = Parrot::new();
    let provider: &dyn Provider = &parrot;
    let lifecycle = provider
        .prompt_cache_resources()
        .expect("the parrot keeps resources");
    let cancel = Cancel::new();
    let deadline = PromptCacheResourceDeadline::new(Instant::now() + Duration::from_mins(1));
    let transcript = Transcript::new();
    let request = asking("model-c", &transcript);
    let id = PromptCacheResourceId::new();
    let binding = PromptCacheResourceBinding::new(
        PromptCacheScopeDigest::new([1; 32]),
        PromptCacheScopeDigest::new([4; 32]),
        PromptCacheScopeDigest::new([5; 32]),
        PromptCacheFingerprint::new([2; 32]),
        PromptCachePolicyDigest::new([3; 32]),
        PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
        "parrot",
        "model-c",
        None,
    )
    .expect("a valid binding");

    let created = answered!(lifecycle.create(
        PromptCacheResourceCreate {
            id: &id,
            request: &request,
            binding: &binding,
            retention: PromptCacheRetention::provider_default(),
            deadline,
        },
        &cancel,
    ))
    .expect("the resource is created");
    let mut record = PromptCacheResourceRecord::creating(id, binding, 100);
    record.ready(created.handle, created.expires_at, 110);

    let answers = [
        answered!(lifecycle.resolve(&record, deadline, &cancel)),
        answered!(lifecycle.renew(
            &record,
            PromptCacheRetention::provider_default(),
            deadline,
            &cancel,
        )),
        answered!(lifecycle.reconcile(&record, deadline, &cancel)),
        answered!(lifecycle.inspect(&record, deadline, &cancel)),
    ];
    for answer in answers {
        assert_eq!(
            answer.expect("the resource answers").state,
            PromptCacheResourceState::Ready
        );
    }
    let deleted = answered!(lifecycle.delete(&record, deadline, &cancel));
    assert_eq!(
        deleted.expect("the resource is deleted").state,
        PromptCacheResourceState::Deleted
    );

    cancel.request();
    let refused = answered!(lifecycle.inspect(&record, deadline, &cancel));
    assert!(matches!(refused, Err(PromptCacheResourceError::Cancelled)));
    assert_eq!(parrot.kept.answered.load(Ordering::SeqCst), 6);
}
