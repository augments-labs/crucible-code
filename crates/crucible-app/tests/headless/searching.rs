//! A `tool_search` the reader stops while its work is on the worker it was
//! lent: the turn waits for that work, and tells the model what it revealed.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crucible_agents::{AgentBuilder, Model};
use crucible_builtins::{Held, ToolSearch};
use crucible_models::Delta;
use crucible_runner::{EventEnvelope, Runner, Tools, Turned};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_session::Session;
use crucible_tools::{Ask, Remember, Revealed, Sensitivity, ToolWorker, Verdict};
use crucible_types::{AgentId, Message, StopReason, ToolCall, ToolId};

use super::{Failed, Script, Tree, saying};

/// Allows every call it is asked about.
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

/// One round in which the model looks up a tool it cannot see.
fn searching() -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("search"),
            name: "tool_search".into(),
        },
        Delta::ToolArgs(r#"{"query":"web_search"}"#.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

#[test]
fn a_tool_search_stopped_while_its_work_waits_on_the_worker_is_awaited_and_reports_its_reveal()
-> Result<(), Failed> {
    let tree = Tree::new("search-stopped")?;
    // One blocking thread, held by the test, so the search's work — once
    // the worker has given it a place — is started and waits to run.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_time()
        .build()?;
    let (release, held) = mpsc::channel::<()>();
    let (occupied, busy) = mpsc::channel();
    let holding = runtime.spawn_blocking(move || {
        let _ = occupied.send(());
        let _ = held.recv();
    });
    busy.recv()?;

    let worker = ToolWorker::new(runtime.handle().clone());
    let watched = worker.clone();
    let revealed = Revealed::new();
    let mut tools = Tools::new();
    tools.add_builtin(ToolSearch::new(
        vec![Held {
            name: "web_search".into(),
            about: "Searches the web.".into(),
        }],
        revealed.clone(),
    ))?;
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let mut runner = Runner::new(
        Box::new(Script::new(vec![searching(), saying("never asked")])),
        tools,
        agent.build(),
        crucible_context::ContextInputs::new(tree.0.join("work")),
        session,
    )
    .lending(worker);

    // The reader presses the key once the search's work holds its place on
    // the worker, and only then is the work let run.
    let cancel = Cancel::new();
    let stopper = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            while !format!("{watched:?}").contains("available: 3") && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(1));
            }
            cancel.request();
            let _ = release.send(());
        })
    };
    let (events, _seen) = mpsc::channel::<EventEnvelope>();
    let (steer, aside) = (Steer::new(), Aside::new());
    let run = runner.starting(&events, &cancel, &steer, &aside);
    let turned = runtime.block_on(runner.turn("find the web", Box::new([]), &mut Allows, &run));
    let _ = stopper.join();
    runtime.block_on(holding)?;

    let Ok(Turned::Ran(result)) = turned else {
        return Err(format!("the turn did not end as a stopped run: {turned:?}").into());
    };
    assert_eq!(
        result.stop(),
        StopReason::Cancelled,
        "the stop did not end the turn"
    );
    assert!(
        revealed.holds("web_search"),
        "the search's work never ran, so nothing here was proved"
    );
    let told: Vec<&str> = runner
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults(results) => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| result.output.text())
        .collect();
    assert_eq!(told.len(), 1);
    assert!(
        told.iter()
            .all(|text| text.contains("web_search: Searches the web.")),
        "the model was not told what the search revealed: {told:?}"
    );
    Ok(())
}
