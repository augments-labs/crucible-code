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
    AgentId, CredentialScopeId, Modalities, Modality, PromptCacheEncoding, StopReason,
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
