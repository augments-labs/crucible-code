//! A conversation's tool calls hand their blocking work to the run's own
//! tool worker, which startup lends the runner.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crucible_models::{
    Delta, DeltaStream, PromptCacheCapabilities, PromptCacheRoute, Provider, ProviderError, Request,
};
use crucible_runtime::BoxFuture;
use crucible_types::{CredentialScopeId, Modalities, Modality, PromptCacheEncoding, StopReason};

use super::*;

/// Answers each request with the next round it was given.
struct Rounds {
    scope: CredentialScopeId,
    rounds: Mutex<std::vec::IntoIter<Vec<Delta>>>,
}

impl Provider for Rounds {
    fn name(&self) -> &'static str {
        "rounds"
    }

    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("startup-lending-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "rounds",
            endpoint: "rounds",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "startup-lending-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn stream<'a>(
        &'a self,
        _request: Request<'a>,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            let round = self
                .rounds
                .lock()
                .map_err(|_| ProviderError::Transport {
                    provider: "rounds",
                    problem: "poisoned".into(),
                })?
                .next()
                .unwrap_or_default();
            Ok(Box::new(Reading(round.into_iter())) as Box<dyn DeltaStream>)
        })
    }
}

struct Reading(std::vec::IntoIter<Delta>);

impl DeltaStream for Reading {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(async move { self.0.next().map(Ok) })
    }
}

struct Allows;

impl Ask for Allows {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Allow, Remember::Never) })
    }
}

#[test]
fn an_assembled_conversation_s_search_waits_for_room_on_the_run_s_tool_worker() {
    // Every place on the run's worker is held by the test, so a search lent
    // that worker waits for room, and one lent none runs at once on whatever
    // polls it. The turn is still out while the places are held, and ends
    // once they are given back.
    let sample = Sample::new("startup-lends-its-worker");
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let settings = Settings::default();
    let services = Services::new();
    let mut conversation = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NOTHING_TO_ASK,
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_builtins::Background::new(),
        services: &services,
        settings: &settings,
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        asking: Arc::new(Nobody),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
    })
    .expect("a session to assemble");
    conversation.runner.serve(Box::new(Rounds {
        scope: CredentialScopeId::new(),
        rounds: Mutex::new(
            vec![
                vec![
                    Delta::ToolStarted {
                        id: crucible_types::ToolId::new("walk"),
                        name: "glob".into(),
                    },
                    Delta::ToolArgs(r#"{"pattern":"*"}"#.into()),
                    Delta::Stopped(StopReason::WantsTools),
                ],
                vec![
                    Delta::Text("walked".into()),
                    Delta::Stopped(StopReason::Yielded),
                ],
            ]
            .into_iter(),
        ),
    }));

    let runtime = services.runtime().handle().expect("the run's runtime");
    let worker = services.tool_worker().expect("the run's worker").clone();
    let (release, held) = mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let holders: Vec<_> = (0..crucible_tools::ToolWorker::CAPACITY)
        .map(|_| {
            let (worker, held) = (worker.clone(), Arc::clone(&held));
            runtime.spawn(async move {
                worker
                    .run(&Cancel::new(), move |_| {
                        let _ = held.lock().map(|held| held.recv());
                    })
                    .await
            })
        })
        .collect();
    let until = Instant::now() + Duration::from_secs(10);
    while !format!("{worker:?}").contains("available: 0") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }

    let (answered, answer) = mpsc::channel();
    let turning = std::thread::spawn(move || {
        let runner = &mut conversation.runner;
        let (events, _seen) = mpsc::channel();
        let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
        let run = runner.starting(&events, &cancel, &steer, &aside);
        let turned = runner
            .turn("walk", Box::new([]), &mut Allows, &run)
            .awaited()
            .map(|_| ());
        let told: Vec<String> = runner
            .transcript()
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults(results) => Some(results),
                _ => None,
            })
            .flatten()
            .map(|result| result.output.text().to_owned())
            .collect();
        let _ = answered.send((turned.is_ok(), told));
    });

    let early = answer.recv_timeout(Duration::from_millis(500));
    for _ in 0..crucible_tools::ToolWorker::CAPACITY {
        let _ = release.send(());
    }
    let (ended, told) = match early {
        Ok(_) => panic!("the search ran while every place on the run's worker was held"),
        Err(_) => answer
            .recv_timeout(Duration::from_secs(10))
            .expect("the turn ended once the worker had room"),
    };
    let _ = turning.join();
    for holder in holders {
        let _ = runtime.block_on(holder);
    }

    assert!(ended, "the turn failed");
    assert_eq!(told.len(), 1, "{told:?}");
}
