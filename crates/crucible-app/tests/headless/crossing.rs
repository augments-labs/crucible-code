//! Waiting for a turn: on the thread that asks for it, never spawned, and
//! refused where it cannot wait.
//!
//! The runner's turn is asynchronous and a conversation's is not, so a
//! conversation waits for each turn on the thread that asked for it, entered
//! into the application's runtime. These drive a conversation over the
//! application's own runtime, from [`serving`], and read where the model was
//! asked from, what the runtime was running meanwhile, and how the turn ended.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread;

use crucible_agents::{AgentBuilder, Model};
use crucible_app::Conversation;
use crucible_app::services::serving;
use crucible_models::{
    Delta, DeltaStream, PromptCacheCapabilities, PromptCacheRoute, Provider, ProviderError, Request,
};
use crucible_runner::{Event, EventEnvelope, Runner, Tools, TurnError, Turned};
use crucible_runtime::{Aside, BoxFuture, Bridge, Cancel, Steer, Unwaited};
use crucible_session::Session;
use crucible_types::{
    AgentId, CredentialScopeId, Message, Modalities, Modality, PromptCacheEncoding,
    RecordedToolOutput, ResultProvenance, StopReason, ToolArgs, ToolCall, ToolId, ToolResult,
    Transcript,
};
use tokio::runtime::Handle;

use super::{Failed, Nobody, Tree};

/// Where each poll of the model's steps ran: the thread's name, and how many
/// tasks the runtime entered there had alive at that moment.
#[derive(Debug, Default)]
struct Seen(Mutex<Vec<(Option<String>, Option<usize>)>>);

impl Seen {
    fn polled(&self) {
        let here = (
            thread::current().name().map(str::to_owned),
            Handle::try_current()
                .ok()
                .map(|runtime| runtime.metrics().num_alive_tasks()),
        );
        if let Ok(mut seen) = self.0.lock() {
            seen.push(here);
        }
    }

    fn all(&self) -> Vec<(Option<String>, Option<usize>)> {
        self.0.lock().map(|seen| seen.clone()).unwrap_or_default()
    }
}

/// `future`, noting where it is polled, after saying once that it is not
/// ready and waking whoever asked.
struct Noted<F> {
    seen: Arc<Seen>,
    asked: bool,
    future: Pin<Box<F>>,
}

impl<F> Noted<F> {
    fn new(seen: &Arc<Seen>, future: F) -> Self {
        Self {
            seen: Arc::clone(seen),
            asked: false,
            future: Box::pin(future),
        }
    }
}

impl<F: Future> Future for Noted<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        self.seen.polled();
        if !self.asked {
            self.asked = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.future.as_mut().poll(cx)
    }
}

/// How a [`Watched`] provider's response goes.
#[derive(Debug, Clone, Copy)]
enum Answering {
    /// It says `text` and yields.
    Saying(&'static str),
    /// It says nothing until the turn is stopped, and then that it was. The
    /// first read tells whoever holds the receiving end of `waiting` each
    /// time it waits unstopped, and drops its end once it has answered.
    UntilStopped,
}

/// A provider whose every step waits once before it answers, and notes where
/// it was polled.
struct Watched {
    seen: Arc<Seen>,
    asked: Arc<AtomicUsize>,
    answering: Answering,
    scope: CredentialScopeId,
    /// Where a response that waits for the stop says it is waiting; taken by
    /// the first response that waits.
    waiting: Mutex<Option<mpsc::SyncSender<()>>>,
}

impl Watched {
    fn new(answering: Answering) -> Self {
        Self {
            seen: Arc::default(),
            asked: Arc::default(),
            answering,
            scope: CredentialScopeId::new(),
            waiting: Mutex::new(None),
        }
    }

    /// The same provider, saying on `waiting` whenever its response waits
    /// for the stop.
    fn telling(self, waiting: mpsc::SyncSender<()>) -> Self {
        Self {
            waiting: Mutex::new(Some(waiting)),
            ..self
        }
    }
}

impl Provider for Watched {
    fn name(&self) -> &'static str {
        "watched"
    }

    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("headless-watched-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "watched",
            endpoint: "watched",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "headless-watched-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn stream<'a>(
        &'a self,
        _request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        let deltas = match self.answering {
            Answering::Saying(text) => vec![
                Delta::Text(text.into()),
                Delta::Stopped(StopReason::Yielded),
            ],
            Answering::UntilStopped => vec![Delta::Stopped(StopReason::Cancelled)],
        };
        let stream = Replying {
            seen: Arc::clone(&self.seen),
            deltas: deltas.into_iter(),
            until: matches!(self.answering, Answering::UntilStopped).then(|| {
                let waiting = self.waiting.lock().ok().and_then(|mut held| held.take());
                (cancel.clone(), waiting)
            }),
        };
        Box::pin(Noted::new(&self.seen, async move {
            Ok(Box::new(stream) as Box<dyn DeltaStream>)
        }))
    }
}

/// A response whose every read waits once, and whose first read, where it
/// holds a cancel, waits until that cancel is raised.
struct Replying {
    seen: Arc<Seen>,
    deltas: std::vec::IntoIter<Delta>,
    until: Option<(Cancel, Option<mpsc::SyncSender<()>>)>,
}

impl DeltaStream for Replying {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        let next = self.deltas.next().map(Ok);
        let until = self.until.take();
        Box::pin(Noted::new(&self.seen, async move {
            if let Some((cancel, waiting)) = until {
                Stopped { cancel, waiting }.await;
            }
            next
        }))
    }
}

/// Waits until its cancel is raised, looking each time it is woken, saying
/// on `waiting` each time it is still waiting, and waking itself so that it
/// is asked again. Its end of `waiting` goes with it once it has answered.
struct Stopped {
    cancel: Cancel,
    waiting: Option<mpsc::SyncSender<()>>,
}

impl Future for Stopped {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.cancel.requested() {
            self.waiting = None;
            return Poll::Ready(());
        }
        if let Some(waiting) = &self.waiting {
            let _ = waiting.try_send(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// A conversation asking `provider`, recording into a session of its own in
/// `tree`, handed no runtime.
fn asking(tree: &Tree, provider: Watched) -> Result<Conversation, Failed> {
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "watched".into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let work = tree.0.join("work");
    Ok(Conversation::recording(session, None, |session| {
        Runner::new(
            Box::new(provider),
            Tools::new(),
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
    }))
}

/// Takes one turn of `conversation` under `cancel`, handing back how it ended
/// and every event it posted.
fn turned(
    conversation: &mut Conversation,
    cancel: &Cancel,
) -> (Result<Turned, TurnError>, Vec<Event>) {
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (steer, aside) = (Steer::new(), Aside::new());
    let turned = {
        let run = conversation
            .runner()
            .starting(&events, cancel, &steer, &aside);
        conversation.turn("go", Box::default(), &mut Nobody, &run)
    };
    drop(events);
    (
        turned,
        reported.try_iter().map(EventEnvelope::into_event).collect(),
    )
}

#[test]
fn a_turn_is_polled_on_the_thread_that_asks_for_it_and_spawns_nothing() -> Result<(), Failed> {
    let tree = Tree::new("crossing-thread")?;
    let provider = Watched::new(Answering::Saying("from the turn thread"));
    let seen = Arc::clone(&provider.seen);
    let conversation = asking(&tree, provider)?;

    let (ran, stopped) = serving(|services| -> Result<Option<StopReason>, String> {
        let mut conversation =
            conversation.on(services.runtime().handle().map_err(|e| e.to_string())?);
        thread::Builder::new()
            .name("turn".to_owned())
            .spawn(move || {
                let (turned, _) = turned(&mut conversation, &Cancel::new());
                turned
                    .map(|turned| turned.stop())
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "the turn thread came apart".to_owned())?
    });

    assert_eq!(ran?, Some(StopReason::Yielded));
    assert!(stopped.is_ok(), "{stopped:?}");
    let seen = seen.all();
    assert!(
        seen.len() >= 4,
        "the model's steps were polled {} times, fewer than waiting once each takes",
        seen.len()
    );
    assert!(
        seen.iter()
            .all(|(thread, _)| thread.as_deref() == Some("turn")),
        "a step of the turn was polled off the thread that asked for it: {seen:?}"
    );
    assert!(
        seen.iter().all(|(_, alive)| *alive == Some(0)),
        "the runtime had a task alive while the turn was polled, or was not entered: {seen:?}"
    );
    Ok(())
}

#[test]
fn a_turn_asked_for_on_a_runtime_s_thread_is_refused_and_never_asked() -> Result<(), Failed> {
    let tree = Tree::new("crossing-inside")?;
    let provider = Watched::new(Answering::Saying("never said"));
    let asked = Arc::clone(&provider.asked);
    let conversation = asking(&tree, provider)?;

    let (refused, stopped) = serving(|services| -> Result<String, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let mut conversation = conversation.on(runtime.clone());
        let _entered = runtime.enter();
        match turned(&mut conversation, &Cancel::new()).0 {
            Err(TurnError::Unwaited(Unwaited::InsideRuntime(Bridge::AppTurn))) => {
                Ok("refused".to_owned())
            }
            other => Err(format!("{other:?}")),
        }
    });

    assert_eq!(refused?, "refused");
    assert!(stopped.is_ok(), "{stopped:?}");
    assert_eq!(asked.load(Ordering::Relaxed), 0, "the model was asked");
    Ok(())
}

#[test]
fn a_conversation_handed_no_runtime_refuses_its_turns_and_compactions() -> Result<(), Failed> {
    let tree = Tree::new("crossing-none")?;
    let provider = Watched::new(Answering::Saying("never said"));
    let asked = Arc::clone(&provider.asked);
    let mut conversation = asking(&tree, provider)?;

    let turn = turned(&mut conversation, &Cancel::new()).0;
    let room = {
        let (events, _reported) = mpsc::channel::<EventEnvelope>();
        let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
        let run = conversation
            .runner()
            .starting(&events, &cancel, &steer, &aside);
        conversation.compact(
            crucible_types::Compacting::Asked,
            &run,
            &mut crucible_types::Spend::default(),
        )
    };

    assert!(
        matches!(
            turn,
            Err(TurnError::Unwaited(Unwaited::NoRuntime(Bridge::AppTurn)))
        ),
        "{turn:?}"
    );
    assert!(
        matches!(
            room,
            Err(TurnError::Unwaited(Unwaited::NoRuntime(Bridge::AppTurn)))
        ),
        "{room:?}"
    );
    assert_eq!(asked.load(Ordering::Relaxed), 0, "the model was asked");
    Ok(())
}

/// The sentence a search result another vendor restricts is cleared to.
const RESTRICTED: &str = "[cleared - restricted to the vendor that produced it]";

/// A conversation over `provider`, picked up from a session whose one search
/// result was answered by a vendor that restricts it to itself.
fn resuming_a_restricted_result(tree: &Tree, provider: Watched) -> Result<Conversation, Failed> {
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let mut transcript = Transcript::new();
    transcript.push(Message::said("search for rust"))?;
    transcript.push(Message::Agent {
        continuation: None,
        text: "searching".into(),
        calls: vec![ToolCall {
            id: ToolId::new("search"),
            name: "web_search".into(),
            args: ToolArgs::new("{}"),
        }],
        stop: Some(StopReason::WantsTools),
    })?;
    transcript.push(Message::ToolResults(vec![ToolResult {
        id: ToolId::new("search"),
        output: RecordedToolOutput::ok("restricted canary")
            .answered_by(ResultProvenance::answered("google", Some(RESTRICTED))?),
    }]))?;
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "watched".into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let work = tree.0.join("work");
    Ok(Conversation::recording(session, None, |session| {
        Runner::new(
            Box::new(provider),
            Tools::new(),
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
        .resuming(transcript)
    }))
}

#[test]
fn a_clearing_line_the_conversation_could_not_wait_for_is_reported_until_a_turn_writes_it()
-> Result<(), Failed> {
    let tree = Tree::new("crossing-owed")?;
    let conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("after")))?;

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        // Handed its runtime on a thread entered into it, so the wait for
        // what picking the session up owes is refused.
        let entered = runtime.enter();
        let mut conversation = conversation.on(runtime.clone());
        drop(entered);
        let refused = conversation.session().missed();

        turned(&mut conversation, &Cancel::new())
            .0
            .map_err(|e| e.to_string())?;
        let session = Arc::clone(conversation.session());
        let written = session.missed();
        drop(conversation);
        let trouble = session.finish();
        let log = std::fs::read_to_string(session.path()).map_err(|e| e.to_string())?;
        Ok((refused, written, trouble, log))
    });
    let (refused, written, trouble, log) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(
        refused.is_some(),
        "a clearing line nobody waited for went unreported"
    );
    assert_eq!(
        written, None,
        "the report outlived the turn that wrote the line"
    );
    assert_eq!(trouble, None, "the log was said to have stopped recording");
    let cleared = log.lines().position(|line| line.contains("\"restricted\""));
    let asked = log.lines().position(|line| line == r#"{"user":"go"}"#);
    assert!(
        cleared.is_some() && cleared < asked,
        "the turn did not write the owed line before its own: {log}"
    );
    Ok(())
}

#[test]
fn a_clearing_line_dropped_with_the_conversation_stays_reported() -> Result<(), Failed> {
    let tree = Tree::new("crossing-owed-dropped")?;
    let conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("never")))?;

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let entered = runtime.enter();
        let conversation = conversation.on(runtime.clone());
        drop(entered);
        let session = Arc::clone(conversation.session());
        drop(conversation);
        Ok((session.missed(), session.finish()))
    });
    let (missed, trouble) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(
        missed.is_some(),
        "clearing lines lost with the runner went unreported"
    );
    assert_eq!(trouble, None, "the log was said to have stopped recording");
    Ok(())
}

#[test]
fn a_conversation_that_owes_nothing_reports_nothing_where_it_could_not_wait() -> Result<(), Failed>
{
    // Nothing is owed, so there is nothing to wait for and nothing to report,
    // even where waiting would have been refused.
    let tree = Tree::new("crossing-owes-nothing")?;
    let conversation = asking(&tree, Watched::new(Answering::Saying("never")))?;

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let entered = runtime.enter();
        let conversation = conversation.on(runtime.clone());
        drop(entered);
        Ok((
            conversation.session().missed(),
            conversation.session().trouble(),
        ))
    });
    let (missed, trouble) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert_eq!(missed, None, "a wait with nothing to write was reported");
    assert_eq!(trouble, None, "a wait with nothing to write was trouble");
    Ok(())
}

/// Whether `session`'s log holds a line clearing a restricted result.
fn cleared_in(session: &Session) -> Result<bool, String> {
    let log = std::fs::read_to_string(session.path()).map_err(|e| e.to_string())?;
    Ok(log.lines().any(|line| line.contains("\"restricted\"")))
}

#[test]
fn a_report_stays_through_a_refused_turn_and_names_only_the_session_owed() -> Result<(), Failed> {
    // The line is owed to the session picked up first. A turn that could not
    // be waited for writes nothing, so the report stays; and a new session
    // started where the wait is refused again is owed nothing, so it is not
    // said to be missing anything.
    let tree = Tree::new("crossing-owed-refused-turn")?;
    let conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("never")))?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let entered = runtime.enter();
        let mut conversation = conversation.on(runtime.clone());
        let first = Arc::clone(conversation.session());
        let refused = turned(&mut conversation, &Cancel::new()).0.is_err();
        let after_turn = first.missed();
        conversation
            .clear(&sessions, &workspace, None)
            .map_err(|e| e.to_string())?;
        let started = conversation.session().missed();
        drop(entered);
        Ok((refused, after_turn, first.missed(), started))
    });
    let (refused, after_turn, first, started) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(refused, "the turn was not refused");
    assert!(
        after_turn.is_some(),
        "a turn that wrote nothing withdrew the report"
    );
    assert!(
        first.is_some(),
        "the session still owed its line stopped being reported"
    );
    assert_eq!(
        started, None,
        "a session owed nothing was said to be missing lines"
    );
    Ok(())
}

#[test]
fn a_refused_pick_up_reports_the_session_left_that_is_owed_and_not_the_new_one()
-> Result<(), Failed> {
    // With no runtime, neither resuming nor the pick-up after it can wait. The
    // line is owed to the session left behind, and the one started is owed
    // nothing, so only the first is said to be missing it.
    let tree = Tree::new("crossing-owed-pick-up")?;
    let mut conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("never")))?;
    let first = Arc::clone(conversation.session());

    let left = conversation.clear(&tree.sessions(), &tree.workspace()?, None)?;

    assert!(Arc::ptr_eq(&left, &first), "a different session was left");
    assert!(
        first.missed().is_some(),
        "the session left owing a line was not reported"
    );
    assert_eq!(
        conversation.session().missed(),
        None,
        "the session started, owed nothing, was said to be missing lines"
    );
    Ok(())
}

#[test]
fn a_waited_pick_up_clears_the_report_of_the_session_it_wrote_to() -> Result<(), Failed> {
    // Refused twice — the first session is owed its line, and a second one is
    // started without it — then a pick-up that is waited for writes the line
    // to the first session, which is two sessions back by then.
    let tree = Tree::new("crossing-owed-left")?;
    let conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("never")))?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let entered = runtime.enter();
        let mut conversation = conversation.on(runtime.clone());
        let first = Arc::clone(conversation.session());
        let second = conversation
            .clear(&sessions, &workspace, None)
            .map(|_| Arc::clone(conversation.session()))
            .map_err(|e| e.to_string())?;
        drop(entered);
        conversation
            .clear(&sessions, &workspace, None)
            .map_err(|e| e.to_string())?;
        Ok((
            cleared_in(&first)?,
            first.missed(),
            second.missed(),
            conversation.session().missed(),
        ))
    });
    let (written, first, second, third) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(written, "the waited pick-up did not write the owed line");
    assert_eq!(first, None, "the report outlived the line it named");
    assert_eq!(second, None, "a session owed nothing was reported");
    assert_eq!(third, None, "a session owed nothing was reported");
    Ok(())
}

#[test]
fn a_report_clears_when_a_conversation_handed_its_runtime_writes_the_line() -> Result<(), Failed> {
    // With no runtime yet, a pick-up cannot wait, and the session left behind
    // is the one owed its line. Handing the runtime over writes it there.
    let tree = Tree::new("crossing-owed-on")?;
    let mut conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("never")))?;
    let first = Arc::clone(conversation.session());
    conversation.clear(&tree.sessions(), &tree.workspace()?, None)?;
    let second = Arc::clone(conversation.session());
    let refused = (first.missed(), second.missed());

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let conversation = conversation.on(runtime);
        Ok((
            cleared_in(&first)?,
            first.missed(),
            conversation.session().missed(),
        ))
    });
    let (written, first_after, second_after) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(
        refused.0.is_some(),
        "the session owed the line was not reported"
    );
    assert_eq!(refused.1, None, "the session owed nothing was reported");
    assert!(written, "handing the runtime over did not write the line");
    assert_eq!(first_after, None, "the report outlived the line it named");
    assert_eq!(second_after, None, "a session owed nothing was reported");
    Ok(())
}

#[test]
fn a_waited_compaction_clears_the_report_once_it_writes_the_line() -> Result<(), Failed> {
    let tree = Tree::new("crossing-owed-compaction")?;
    let conversation =
        resuming_a_restricted_result(&tree, Watched::new(Answering::Saying("never")))?;

    let (seen, stopped) = serving(|services| -> Result<_, String> {
        let runtime = services.runtime().handle().map_err(|e| e.to_string())?;
        let entered = runtime.enter();
        let mut conversation = conversation.on(runtime.clone());
        drop(entered);
        let refused = conversation.session().missed();
        {
            let (events, _reported) = mpsc::channel::<EventEnvelope>();
            let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
            let run = conversation
                .runner()
                .starting(&events, &cancel, &steer, &aside);
            conversation
                .compact(
                    crucible_types::Compacting::Asked,
                    &run,
                    &mut crucible_types::Spend::default(),
                )
                .map_err(|e| e.to_string())?;
        }
        Ok((
            refused,
            cleared_in(conversation.session())?,
            conversation.session().missed(),
        ))
    });
    let (refused, written, after) = seen?;

    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(refused.is_some(), "the refused wait went unreported");
    assert!(written, "the compaction did not write the owed line");
    assert_eq!(after, None, "the report outlived the line it named");
    Ok(())
}

#[test]
fn a_turn_stopped_while_the_model_waits_ends_as_a_stopped_turn() -> Result<(), Failed> {
    // The crossing is not what stops the turn: the turn sees its own cancel
    // through the step it awaits, ends the way a stopped turn ends, and says
    // so. A turn dropped at the waiting step would post no ending at all.
    // The stop is raised only once the model's response has said it is
    // waiting, so it lands on a step that is pending, whatever the timing.
    let tree = Tree::new("crossing-stopped")?;
    let (waiting, heard) = mpsc::sync_channel(1);
    let conversation = asking(
        &tree,
        Watched::new(Answering::UntilStopped).telling(waiting),
    )?;
    let cancel = Cancel::new();
    let raising = cancel.clone();

    let (ended, stopped) = serving(
        |services| -> Result<(Option<StopReason>, Vec<Event>, bool), String> {
            let mut conversation =
                conversation.on(services.runtime().handle().map_err(|e| e.to_string())?);
            // Raises the stop on hearing the response wait, and says whether
            // it ever did: the response drops its end once it has answered,
            // and the conversation drops the rest below.
            let raiser = thread::spawn(move || {
                let waited = heard.recv().is_ok();
                if waited {
                    raising.request();
                }
                waited
            });
            let (turned, events) = turned(&mut conversation, &cancel);
            drop(conversation);
            let waited = raiser
                .join()
                .map_err(|_| "the raiser came apart".to_owned())?;
            Ok((turned.map_err(|e| e.to_string())?.stop(), events, waited))
        },
    );

    let (stop, events, waited) = ended?;
    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(
        waited,
        "the model's response never waited, so the stop landed on no pending step"
    );
    assert_eq!(stop, Some(StopReason::Cancelled));
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::TurnFinished {
                stop: StopReason::Cancelled,
                ..
            }
        )),
        "the stopped turn did not say it finished: {events:?}"
    );
    Ok(())
}
