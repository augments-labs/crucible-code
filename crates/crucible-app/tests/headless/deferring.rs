//! Questions the model asks without waiting, answered by a client that has no
//! terminal.
//!
//! The front end here supplies the registry and keeps a clone of it, as a
//! front end with no terminal does, and every answer it sends is a request
//! read back off the bytes it would travel as. What the tests read is what a
//! client is handed — a response, and the progress streamed beside it — and,
//! beside those, what each request to the scripted provider carried.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crucible_agents::{AgentBuilder, Model};
use crucible_app::Conversation;
use crucible_app::client::{self, Front, Shown};
use crucible_builtins::{AskUser, Ledger, Read};
use crucible_client_api::{
    AnswerOutcome, Capabilities, Capability, Command, Correlation, Decision, ErrorCode, Outcome,
    Pending, Picked, Progress, Prompt, Refusal, Replied, Request, Response, Said, Stop,
    TurnOutcome,
};
use crucible_models::Delta;
use crucible_runner::{EventEnvelope, Tools};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_session::Session;
use crucible_tools::{Deferred, Permission, Put, Rules};
use crucible_types::{AgentId, Answer, Answered, Question, QuestionId, StopReason, ToolId};

use super::client::{Counting, calling};
use super::{Failed, Script, Tree, saying};

/// How the runner opens the message that delivers an answer.
const ANSWERED: &str = "The person answered question batch";

/// Nobody at a keyboard: a question asked in the call goes unanswered.
struct Unseen;

impl Put for Unseen {
    fn put<'a>(&'a self, _questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>> {
        Box::pin(async { None })
    }
}

/// A call to `ask_user`, numbered `call`, asking which `heading` without
/// waiting for the answer.
fn deferring(call: &str, heading: &str) -> Vec<Delta> {
    let args = format!(
        r#"{{"mode":"deferred","questions":[{{"heading":"{heading}","question":"Which {heading}?","answers":[{{"answer":"Rust"}},{{"answer":"Python"}}]}}]}}"#
    );
    vec![
        Delta::ToolStarted {
            id: ToolId::new(call),
            name: "ask_user".into(),
        },
        Delta::ToolArgs(args.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// A call to `read` of the note the tree holds: work that needs no answer.
fn reading() -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("r"),
            name: "read".into(),
        },
        Delta::ToolArgs(r#"{"path":"notes.txt"}"#.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// A conversation over `script` with nobody at a keyboard, whose front end
/// holds the model's questions in `held` where it supplied a registry, and
/// which offers `tools` beside the question tool.
fn holding(
    tree: &Tree,
    script: Script,
    held: Option<&Deferred>,
    serving: Option<&'static str>,
    mut tools: Tools,
) -> Result<Conversation, Failed> {
    let asking = AskUser::new(Arc::new(Unseen));
    tools.add_builtin(match held {
        Some(held) => asking.deferring(held.clone()),
        None => asking,
    })?;
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
    let work = tree.0.join("work");
    let held = held.cloned();

    Ok(Conversation::recording(session, serving, move |session| {
        let runner = crucible_runner::Runner::new(
            Box::new(script),
            tools,
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
        .permitting(Permission::with(crucible_tools::Mode::Ask, Rules::new()));
        match held {
            Some(held) => runner.deferring(held),
            None => runner,
        }
    }))
}

/// [`holding`], with the registry and nothing offered beside the question
/// tool.
pub(super) fn held(tree: &Tree, script: Script, held: &Deferred) -> Result<Conversation, Failed> {
    holding(tree, script, Some(held), None, Tools::new())
}

/// `command` as a client claiming `claims` sends it, read back off its bytes.
fn sent(claims: Capabilities, command: Command) -> Result<Request, Failed> {
    let request = Request::new(claims, Correlation::new(7), command);
    Request::decode(&request.encode()?).map_err(|refused| refused.refusal.into())
}

/// An answer to batch `id`, choosing `chosen` for its one question.
fn choosing(id: QuestionId, chosen: &[&str]) -> Result<Command, Failed> {
    Ok(Command::Answer {
        id,
        replies: vec![Replied::Answered(Picked {
            chosen: chosen
                .iter()
                .map(|one| Said::new(one))
                .collect::<Result<_, _>>()?,
            note: Said::new("")?,
        })],
    })
}

/// [`choosing`] `Rust`, sent by a client that claims everything.
pub(super) fn rust(id: QuestionId) -> Result<Request, Failed> {
    sent(Capabilities::every(), choosing(id, &["Rust"])?)
}

/// The oldest batch waiting in `held`.
fn oldest(held: &Deferred) -> Result<QuestionId, Failed> {
    Ok(held.waiting().first().ok_or("nothing waits")?.id())
}

/// A client with no terminal. It goes back from a warned route, and while
/// anything else is pending it answers the questions waiting instead, and
/// says nothing about what is pending.
struct Client {
    held: Deferred,
    put: Vec<Pending>,
    answered: Vec<Outcome>,
    refused: Vec<Refusal>,
}

impl Client {
    fn new(held: &Deferred) -> Self {
        Self {
            held: held.clone(),
            put: Vec::new(),
            answered: Vec::new(),
            refused: Vec::new(),
        }
    }
}

impl Front for Client {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            self.put.push(pending.clone());
            if let Pending::Warning { .. } = pending {
                return Some(Decision::Declined { id: pending.id() });
            }
            for batch in self.held.waiting() {
                if let Ok(request) = rust(batch.id()) {
                    self.answered.push(client::answer(&request, &self.held));
                }
            }
            None
        })
    }

    fn refused(&mut self, refusal: Refusal) {
        self.refused.push(refusal);
    }
}

/// Takes `request` at the turn door, stopped through `cancel` where somebody
/// asks it to be, and hands back the response a client would read beside the
/// progress it would have been streamed.
fn turning(
    conversation: &mut Conversation,
    request: &Request,
    front: &mut dyn Front,
    cancel: &Cancel,
) -> Result<(Response, Vec<Progress>), Failed> {
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (steer, aside) = (Steer::new(), Aside::new());
    let ended = {
        let run = conversation.starting(&events, cancel, &steer, &aside);
        super::runtime()?.block_on(client::turn(
            conversation,
            request,
            Box::default(),
            front,
            &run,
        ))
    };
    drop(events);

    let mut streamed = Vec::new();
    let serving = conversation.serving();
    for envelope in reported.try_iter() {
        let event = envelope.into_event();
        if let Some(progress) = client::progress(request.capabilities(), &event, serving) {
            streamed.push(Progress::decode(&progress.encode()?)?);
        }
    }
    let response = ended.response(request);

    Ok((Response::decode(&response.encode()?)?, streamed))
}

/// [`turning`], with a cancel nobody asks for and a client that answers
/// nothing.
pub(super) fn turned(
    conversation: &mut Conversation,
    request: &Request,
    held: &Deferred,
) -> Result<(Response, Vec<Progress>), Failed> {
    turning(
        conversation,
        request,
        &mut Client::new(held),
        &Cancel::new(),
    )
}

/// A prompt of `words`, sent by a client that claims everything.
fn prompt(words: &str) -> Result<Request, Failed> {
    sent(Capabilities::every(), Command::Prompt(Prompt::new(words)?))
}

/// Each waiting set a client was streamed, in order, as the headings of its
/// batches' questions.
fn waits(streamed: &[Progress]) -> Vec<Vec<String>> {
    streamed
        .iter()
        .filter_map(|progress| match progress {
            Progress::Waiting { batches } => Some(
                batches
                    .iter()
                    .flat_map(|batch| &batch.questions)
                    .map(|asked| asked.heading.as_str().to_owned())
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

/// Why each turn a client was streamed stopped, in order.
pub(super) fn stops(streamed: &[Progress]) -> Vec<Stop> {
    streamed
        .iter()
        .filter_map(|progress| match progress {
            Progress::Finished { stop, .. } => Some(*stop),
            _ => None,
        })
        .collect()
}

/// How many delivered answers each request carried, in the order they were
/// made.
fn delivered(sent: &Arc<Mutex<Vec<Vec<String>>>>) -> Result<Vec<usize>, Failed> {
    Ok(sent
        .lock()
        .map_err(|_| "poisoned")?
        .iter()
        .map(|said| {
            said.iter()
                .filter(|text| text.starts_with(ANSWERED))
                .count()
        })
        .collect())
}

/// What the user had said in the request numbered `at`.
fn said_in(sent: &Arc<Mutex<Vec<Vec<String>>>>, at: usize) -> Result<Vec<String>, Failed> {
    Ok(sent
        .lock()
        .map_err(|_| "poisoned")?
        .get(at)
        .cloned()
        .ok_or("no such request")?)
}

/// Whether a response is a turn that ran and stopped for `stop`.
fn ran(response: &Response, stop: Stop) -> bool {
    matches!(&response.outcome, Outcome::Turn(TurnOutcome::Ran { stop: ended }) if *ended == stop)
}

/// The plan's acceptance, up to the answer: the model asks without waiting
/// and is handed a receipt, a second pass does other work, the client settles
/// the batch while that pass runs, and the next request carries the answer —
/// once, then and after.
#[test]
fn a_client_with_no_terminal_answers_while_the_turn_works_and_it_arrives_once() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-acceptance")?;
    std::fs::write(tree.0.join("work").join("notes.txt"), "a note\n")?;
    let held = Deferred::default();
    let answered = Arc::new(Mutex::new(Vec::new()));
    let script = Script::new(vec![
        deferring("c1", "Language"),
        reading(),
        saying("with the answer in hand"),
        saying("still once"),
    ])
    .meanwhile({
        let (held, answered) = (held.clone(), Arc::clone(&answered));
        move |before| {
            // The second pass is the other work: the batch is settled while
            // it runs, by the door a running turn leaves open.
            if before == 1
                && let Some(batch) = held.waiting().first()
                && let Ok(request) = rust(batch.id())
                && let Ok(mut answered) = answered.lock()
            {
                answered.push(client::answer(&request, &held));
            }
        }
    });
    let sent_to = Arc::clone(&script.sent);
    let mut tools = Tools::new();
    tools.add_builtin(Read::new(tree.workspace()?, Ledger::new()))?;
    let mut conversation = holding(&tree, script, Some(&held), None, tools)?;

    let (response, streamed) = turned(&mut conversation, &prompt("plan it")?, &held)?;

    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    assert_eq!(
        *answered.lock().map_err(|_| "poisoned")?,
        [Outcome::Answer(AnswerOutcome::Settled)],
        "the answer was settled while the turn ran"
    );
    assert!(
        streamed.iter().any(
            |progress| matches!(progress, Progress::ToolFinished { failed: false, call } if call.as_str() == "c1")
        ),
        "the receipt: {streamed:?}"
    );
    let waits = waits(&streamed);
    assert_eq!(
        waits.first(),
        Some(&vec!["Language".to_owned()]),
        "{waits:?}"
    );
    assert_eq!(waits.last(), Some(&Vec::new()), "nothing waits: {waits:?}");
    assert_eq!(delivered(&sent_to)?, [0, 0, 1]);

    let (response, _) = turned(&mut conversation, &prompt("and then?")?, &held)?;

    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    assert_eq!(
        delivered(&sent_to)?,
        [0, 0, 1, 1],
        "delivered once, and kept"
    );
    Ok(())
}

/// Settled while nothing runs, the answer starts a turn of its own, whose
/// first request carries it and nothing the person typed for it; and the
/// turn before it ended with the batch it left waiting in its last report.
#[test]
fn an_answer_settled_while_idle_starts_a_turn_whose_first_request_carries_it() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-idle")?;
    let held = Deferred::default();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        saying("carrying on"),
        saying("with the answer in hand"),
    ]);
    let (asked, sent_to) = (Arc::clone(&script.asked), Arc::clone(&script.sent));
    let mut conversation = self::held(&tree, script, &held)?;
    let (_, streamed) = turned(&mut conversation, &prompt("plan it")?, &held)?;
    assert_eq!(
        waits(&streamed).last(),
        Some(&vec!["Language".to_owned()]),
        "the turn ended with its question waiting"
    );
    assert_eq!(asked.load(Ordering::Relaxed), 2);

    let (response, streamed) = turned(&mut conversation, &rust(oldest(&held)?)?, &held)?;

    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    assert_eq!(asked.load(Ordering::Relaxed), 3, "one request delivered it");
    let first = said_in(&sent_to, 2)?;
    assert_eq!(first.first().map(String::as_str), Some("plan it"));
    assert!(
        matches!(first.as_slice(), [_, answer] if answer.starts_with(ANSWERED)),
        "{first:?}"
    );
    assert_eq!(waits(&streamed), [Vec::<String>::new()]);
    assert!(!held.any_settled(), "the answer was taken");
    Ok(())
}

/// A turn whose last answer was cut short ends with that stop, the answer
/// settled meanwhile still undelivered; the door starts the turn that delivers
/// it straight after, and that turn's outcome is the one response.
#[test]
fn a_cut_short_answer_ends_its_turn_and_one_that_delivers_starts_right_after() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-cut-short")?;
    let held = Deferred::default();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        vec![
            Delta::Text("half an ans".into()),
            Delta::Stopped(StopReason::OutOfTokens),
        ],
        saying("with the answer in hand"),
    ])
    .meanwhile({
        let held = held.clone();
        move |before| {
            if before == 1
                && let Some(batch) = held.waiting().first()
                && let Ok(request) = rust(batch.id())
            {
                drop(client::answer(&request, &held));
            }
        }
    });
    let (asked, sent_to) = (Arc::clone(&script.asked), Arc::clone(&script.sent));
    let mut conversation = self::held(&tree, script, &held)?;

    let (response, streamed) = turned(&mut conversation, &prompt("plan it")?, &held)?;

    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    assert_eq!(stops(&streamed), [Stop::OutOfTokens, Stop::Yielded]);
    assert_eq!(asked.load(Ordering::Relaxed), 3);
    assert_eq!(delivered(&sent_to)?, [0, 0, 1]);
    assert_eq!(
        said_in(&sent_to, 2)?
            .iter()
            .filter(|text| !text.starts_with(ANSWERED))
            .count(),
        1,
        "the delivering turn said nothing of its own"
    );
    Ok(())
}

/// A batch nobody is waiting on, and replies its questions cannot take, are
/// refused whole at either door, and the batch waits as it did.
#[test]
fn a_stale_or_unfitting_answer_is_refused_whole_and_the_batch_still_waits() -> Result<(), Failed> {
    let tree = Tree::new("deferring-refused")?;
    let held = Deferred::default();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = self::held(&tree, script, &held)?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let waiting = oldest(&held)?;
    let stale = Outcome::Refused(ErrorCode::StaleQuestions.into());

    let nobody = rust(QuestionId::numbered(waiting.number() + 40))?;
    assert_eq!(client::answer(&nobody, &held), stale);
    assert_eq!(turned(&mut conversation, &nobody, &held)?.0.outcome, stale);

    let one_of = sent(
        Capabilities::every(),
        choosing(waiting, &["Rust", "Python"])?,
    )?;
    let twice = sent(
        Capabilities::every(),
        Command::Answer {
            id: waiting,
            replies: vec![Replied::Declined, Replied::Declined],
        },
    )?;
    for unfitting in [one_of, twice] {
        for outcome in [
            client::answer(&unfitting, &held),
            turned(&mut conversation, &unfitting, &held)?.0.outcome,
        ] {
            assert!(
                matches!(
                    &outcome,
                    Outcome::Answer(AnswerOutcome::Invalid(problem))
                        if problem.code == ErrorCode::InvalidAnswer
                ),
                "{outcome:?}"
            );
        }
    }

    assert_eq!(held.waiting().len(), 1);
    assert!(!held.any_settled());
    assert_eq!(asked.load(Ordering::Relaxed), 2, "no turn was started");
    Ok(())
}

/// Words over the contract's bound, or more replies than a list may hold, are
/// refused as the frame is read, before the registry is reached; and of more
/// answers to one batch than batches wait, one settles it.
#[test]
fn an_oversized_answer_is_refused_on_the_wire_and_one_of_many_settles() -> Result<(), Failed> {
    use crucible_client_api::bounds::{ITEMS, SAID_BYTES};

    let tree = Tree::new("deferring-oversized")?;
    let held = Deferred::default();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let mut conversation = self::held(&tree, script, &held)?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let waiting = oldest(&held)?;
    let revision = held.revision();
    let frame: serde_json::Value = serde_json::from_slice(&rust(waiting)?.encode()?)?;
    let reply = frame
        .pointer("/command/replies/0")
        .cloned()
        .ok_or_else(|| format!("no reply in {frame}"))?;

    let mut noted = frame.clone();
    *noted
        .pointer_mut("/command/replies/0/note")
        .ok_or_else(|| format!("no note in {frame}"))? = "n".repeat(SAID_BYTES + 1).into();
    let mut chosen = frame.clone();
    *chosen
        .pointer_mut("/command/replies/0/chosen/0")
        .ok_or_else(|| format!("no choice in {frame}"))? = "c".repeat(SAID_BYTES + 1).into();
    let mut listed = frame.clone();
    *listed
        .pointer_mut("/command/replies")
        .ok_or_else(|| format!("no replies in {frame}"))? = vec![reply; ITEMS + 1].into();
    for oversized in [noted, chosen, listed] {
        let refused = Request::decode(&serde_json::to_vec(&oversized)?)
            .err()
            .ok_or("an oversized answer was read")?;
        assert_eq!(refused.refusal.code(), ErrorCode::TooLarge);
    }
    assert_eq!(held.revision(), revision, "the registry was reached");

    let request = rust(waiting)?;
    let outcomes: Vec<Outcome> = std::thread::scope(|scope| {
        let answering: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| client::answer(&request, &held)))
            .collect();
        answering
            .into_iter()
            .filter_map(|one| one.join().ok())
            .collect()
    });
    let settled = outcomes
        .iter()
        .filter(|outcome| **outcome == Outcome::Answer(AnswerOutcome::Settled))
        .count();
    let stale = outcomes
        .iter()
        .filter(|outcome| **outcome == Outcome::Refused(ErrorCode::StaleQuestions.into()))
        .count();
    assert_eq!((settled, stale), (1, 3), "{outcomes:?}");
    Ok(())
}

/// A client that did not claim the questions is streamed none of them, and
/// an answer from it is refused at either door.
#[test]
fn a_client_that_did_not_claim_questions_is_shown_none_and_refused_its_answer() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-unclaimed")?;
    let held = Deferred::default();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let mut conversation = self::held(&tree, script, &held)?;
    let claims = Capabilities::every().without(Capability::Questions);
    let request = sent(claims, Command::Prompt(Prompt::new("plan it")?))?;

    let (_, streamed) = turned(&mut conversation, &request, &held)?;

    assert!(waits(&streamed).is_empty(), "{streamed:?}");
    let answer = Request::new(
        claims,
        Correlation::new(8),
        choosing(oldest(&held)?, &["Rust"])?,
    );
    let needs = Outcome::Refused(ErrorCode::NeedsCapability.into());
    assert_eq!(client::answer(&answer, &held), needs);
    assert_eq!(turned(&mut conversation, &answer, &held)?.0.outcome, needs);
    assert!(!held.any_settled());
    Ok(())
}

/// With no registry from the front end, a question held for later is refused
/// to the model, and nothing waits for a client to answer.
#[test]
fn a_front_end_with_no_registry_holds_no_question_for_later() -> Result<(), Failed> {
    let tree = Tree::new("deferring-unheld")?;
    let script = Script::new(vec![deferring("c1", "Language"), saying("asked in words")]);
    let mut conversation = holding(&tree, script, None, None, Tools::new())?;
    let nowhere = Deferred::default();

    let (_, streamed) = turned(&mut conversation, &prompt("plan it")?, &nowhere)?;

    assert!(
        streamed.iter().any(
            |progress| matches!(progress, Progress::ToolFinished { failed: true, call } if call.as_str() == "c1")
        ),
        "{streamed:?}"
    );
    assert!(waits(&streamed).is_empty(), "{streamed:?}");
    let (response, _) = turned(&mut conversation, &rust(QuestionId::numbered(1))?, &nowhere)?;
    assert_eq!(
        response.outcome,
        Outcome::Refused(ErrorCode::StaleQuestions.into())
    );
    Ok(())
}

/// An answer settles questions and nothing else: sent while a permission is
/// pending it lets no tool run, and sent to a route whose vendor uses what it
/// is sent it says no yes to the route, which is put to the client first.
#[test]
fn an_answer_leaves_a_pending_permission_and_a_warned_route_unanswered() -> Result<(), Failed> {
    use crucible_app::content_use::{Consent, Routes, Serving};

    let tree = Tree::new("deferring-permission")?;
    let held = Deferred::default();
    let ran = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools.add_builtin(Counting(Arc::clone(&ran)))?;
    let script = Script::new(vec![
        deferring("c1", "Language"),
        calling(),
        saying("stopped"),
    ]);
    let mut conversation = holding(&tree, script, Some(&held), None, tools)?;
    let mut client = Client::new(&held);

    turning(
        &mut conversation,
        &prompt("plan it")?,
        &mut client,
        &Cancel::new(),
    )?;

    assert!(
        matches!(client.put.as_slice(), [Pending::Permission { .. }]),
        "{:?}",
        client.put
    );
    assert_eq!(client.answered, [Outcome::Answer(AnswerOutcome::Settled)]);
    assert_eq!(
        ran.load(Ordering::Relaxed),
        0,
        "the answer let the tool run"
    );

    let tree = Tree::new("deferring-warned")?;
    let held = Deferred::default();
    let script = Script::new(vec![saying("answered")]);
    let asked = Arc::clone(&script.asked);
    let conversation = holding(&tree, script, Some(&held), Some("google"), Tools::new())?;
    let consent = Consent::new(Routes::production());
    let file = tree.0.join("config.json");
    consent.keeps_in(file.clone());
    let mut conversation = conversation.consenting(consent.clone());
    consent.served(
        "google",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    let id = held.register(vec![Question::new(
        "Language",
        "Which language?",
        [Answer::new("Rust"), Answer::new("Python")],
    )])?;
    let mut client = Client::new(&held);

    let (response, _) = turning(&mut conversation, &rust(id)?, &mut client, &Cancel::new())?;

    assert!(
        matches!(&response.outcome, Outcome::Turn(TurnOutcome::Warned { route, .. }) if route.as_str() == "key:google"),
        "{:?}",
        response.outcome
    );
    assert!(
        matches!(client.put.as_slice(), [Pending::Warning { .. }]),
        "{:?}",
        client.put
    );
    assert_eq!(asked.load(Ordering::Relaxed), 0, "something was sent");
    assert!(!held.any_settled(), "settled without a yes to the route");
    assert_eq!(held.waiting().len(), 1);
    let written = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(!written.contains("key:google"), "{written}");
    Ok(())
}

/// Stopping a turn leaves what waits and what was settled as they were and
/// starts nothing; `/clear` leaves the session they belonged to, and empties
/// them.
#[test]
fn cancelling_a_turn_keeps_its_questions_and_clearing_empties_them() -> Result<(), Failed> {
    let tree = Tree::new("deferring-cancel")?;
    let held = Deferred::default();
    let cancel = Cancel::new();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        deferring("c2", "Storage"),
        saying("stopped mid-way"),
        saying("never asked"),
    ])
    .meanwhile({
        let (held, cancel) = (held.clone(), cancel.clone());
        move |before| {
            if before == 2
                && let Some(batch) = held.waiting().first()
                && let Ok(request) = rust(batch.id())
            {
                drop(client::answer(&request, &held));
                cancel.request();
            }
        }
    });
    let asked = Arc::clone(&script.asked);
    let mut conversation = self::held(&tree, script, &held)?;

    let (response, _) = turning(
        &mut conversation,
        &prompt("plan it")?,
        &mut Client::new(&held),
        &cancel,
    )?;

    assert!(ran(&response, Stop::Cancelled), "{:?}", response.outcome);
    assert_eq!(asked.load(Ordering::Relaxed), 3, "a turn was started");
    assert!(held.any_settled(), "the settled answer was dropped");
    assert_eq!(held.waiting().len(), 1, "the waiting batch was dropped");

    super::runtime()?.block_on(conversation.clear(&tree.sessions(), &tree.workspace()?, None))?;

    assert!(held.waiting().is_empty());
    assert!(!held.any_settled());
    Ok(())
}

/// The doors that do not deliver an answer refuse one as they refuse a
/// decision handed in on its own, and the batch waits as it did.
#[test]
fn an_answer_at_a_door_that_cannot_deliver_it_is_refused_as_stale() -> Result<(), Failed> {
    let tree = Tree::new("deferring-doors")?;
    let held = Deferred::default();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let mut conversation = self::held(&tree, script, &held)?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let request = rust(oldest(&held)?)?;
    let standing = super::Desk::new(&tree, &[])?;
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads: |_| false,
        environment: super::unset,
        notes: |_| Err(ErrorCode::InvalidArgument.into()),
    };
    let stale = Outcome::Refused(ErrorCode::StaleQuestions.into());

    let performed = super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));
    assert_eq!(performed.response(&request).outcome, stale);
    assert_eq!(
        client::keep(&request, &desk).response(&request).outcome,
        stale
    );
    let cancel = Cancel::new();
    assert_eq!(client::interrupt(&request, &cancel), stale);
    assert!(!cancel.requested());

    assert_eq!(held.waiting().len(), 1);
    assert!(!held.any_settled());
    Ok(())
}
