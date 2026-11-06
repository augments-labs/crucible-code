//! Questions the model asks without waiting, answered by a client that has no
//! terminal.
//!
//! The front end here supplies a `Questions` handle and keeps a clone of it,
//! as a front end with no terminal does, and every answer it sends is a
//! request read back off the bytes it would travel as. A batch waits only
//! because the scripted model called `ask_user` without waiting; the handle is
//! read for what waits and answered through the client door, and nothing here
//! registers or settles a batch any other way. What the tests read is what a
//! client is handed — a response, and the progress streamed beside it — and,
//! beside those, what each request to the scripted provider carried.
//!
//! A conversation started twice on one tree stands in for a host started
//! again: each has a handle of its own, numbering batches afresh, so a
//! session picked up brings back ids another session also handed out.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crucible_agents::{AgentBuilder, Model};
use crucible_app::Conversation;
use crucible_app::client::{self, Front, Questions, Shown};
use crucible_builtins::{AskUser, Ledger, Read};
use crucible_client_api::{
    AnswerOutcome, Capabilities, Capability, Command, Correlation, Decision, Era, ErrorCode,
    Outcome, Pending, Picked, Progress, Prompt, Refusal, Replied, Request, Response, Said, Stop,
    TurnOutcome,
};
use crucible_models::Delta;
use crucible_runner::{Event, EventEnvelope, Post, Tools};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_session::Session;
use crucible_tools::{Permission, Put, Rules};
use crucible_types::{AgentId, Answered, Question, QuestionId, StopReason, ToolId};

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
pub(super) fn deferring(call: &str, heading: &str) -> Vec<Delta> {
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
/// holds the model's questions in `held` where it supplied a handle, and
/// which offers `tools` beside the question tool.
fn holding(
    tree: &Tree,
    script: Script,
    held: Option<&Questions>,
    serving: Option<&'static str>,
    mut tools: Tools,
) -> Result<Conversation, Failed> {
    let asking = AskUser::new(Arc::new(Unseen));
    tools.add_builtin(match held {
        Some(held) => held.asking_for_proof(asking),
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

    let conversation = Conversation::recording(session, serving, move |session| {
        crucible_runner::Runner::new(
            Box::new(script),
            tools,
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
        .permitting(Permission::with(crucible_tools::Mode::Ask, Rules::new()))
    });
    Ok(match held {
        Some(held) => conversation.holding_for_proof(held.clone())?,
        None => conversation,
    })
}

/// [`holding`], with the handle and nothing offered beside the question
/// tool.
pub(super) fn held(tree: &Tree, script: Script, held: &Questions) -> Result<Conversation, Failed> {
    holding(tree, script, Some(held), None, Tools::new())
}

/// `command` as a client claiming `claims` sends it, read back off its bytes.
fn sent(claims: Capabilities, command: Command) -> Result<Request, Failed> {
    let request = Request::new(claims, Correlation::new(7), command);
    Request::decode(&request.encode()?).map_err(|refused| refused.refusal.into())
}

/// An answer to the batch of `era` numbered `id`, choosing `chosen` for its
/// one question.
fn choosing(era: &Era, id: QuestionId, chosen: &[&str]) -> Result<Command, Failed> {
    Ok(Command::Answer {
        era: era.clone(),
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
pub(super) fn rust(era: &Era, id: QuestionId) -> Result<Request, Failed> {
    sent(Capabilities::every(), choosing(era, id, &["Rust"])?)
}

/// The era of what waits in `held`, and the oldest batch waiting there.
pub(super) fn oldest(held: &Questions) -> Result<(Era, QuestionId), Failed> {
    let (era, waiting) = held.waiting();
    Ok((era, waiting.first().ok_or("nothing waits")?.id()))
}

/// The era of `held`, which must have nothing waiting.
pub(super) fn emptied(held: &Questions) -> Result<Era, Failed> {
    let (era, waiting) = held.waiting();
    if waiting.is_empty() {
        Ok(era)
    } else {
        Err(format!("{} batches wait in {era:?}", waiting.len()).into())
    }
}

/// [`rust`] to the oldest batch waiting in `held`, where one waits.
fn rust_to_oldest(held: &Questions) -> Option<Request> {
    let (era, id) = oldest(held).ok()?;
    rust(&era, id).ok()
}

/// The era of the last waiting set a client was streamed, and the oldest
/// batch in it.
fn last_shown(streamed: &[Progress]) -> Result<(Era, QuestionId), Failed> {
    streamed
        .iter()
        .rev()
        .find_map(|progress| match progress {
            Progress::Waiting { era, batches } => {
                Some(batches.first().map(|batch| (era.clone(), batch.id)))
            }
            _ => None,
        })
        .flatten()
        .ok_or_else(|| format!("no batch was shown: {streamed:?}").into())
}

/// A client with no terminal. It goes back from a warned route, and while
/// anything else is pending it answers the questions waiting instead, and
/// says nothing about what is pending.
struct Client {
    held: Questions,
    put: Vec<Pending>,
    answered: Vec<Outcome>,
    refused: Vec<Refusal>,
}

impl Client {
    fn new(held: &Questions) -> Self {
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
            let (era, waiting) = self.held.waiting();
            for batch in waiting {
                if let Ok(request) = rust(&era, batch.id()) {
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
    let (response, streaming) = reporting(conversation, request, front, cancel)?;
    let streamed = streaming
        .streamed
        .into_inner()
        .map_err(|_| "poisoned")?
        .into_iter()
        .collect::<Result<_, _>>()?;
    Ok((response, streamed))
}

/// Where a turn's events go: each is converted as it is posted, reading what
/// waits then, as a client streamed it would be shown it, and kept as it was
/// posted beside that.
struct Streaming {
    claims: Capabilities,
    serving: Option<&'static str>,
    held: Option<Questions>,
    streamed: Mutex<Vec<Result<Progress, String>>>,
    posted: Mutex<Vec<Event>>,
}

impl Post for Streaming {
    fn post(&self, reported: EventEnvelope) {
        let event = reported.into_event();
        if let Some(progress) =
            client::progress(self.claims, &event, self.serving, self.held.as_ref())
        {
            let read = progress
                .encode()
                .map_err(|error| error.to_string())
                .and_then(|bytes| Progress::decode(&bytes).map_err(|error| error.to_string()));
            if let Ok(mut streamed) = self.streamed.lock() {
                streamed.push(read);
            }
        }
        if let Ok(mut posted) = self.posted.lock() {
            posted.push(event);
        }
    }
}

/// [`turning`], handing back where the turn's events went: what a client was
/// streamed, and the events themselves as they were posted.
fn reporting(
    conversation: &mut Conversation,
    request: &Request,
    front: &mut dyn Front,
    cancel: &Cancel,
) -> Result<(Response, Streaming), Failed> {
    let streaming = Streaming {
        claims: request.capabilities(),
        serving: conversation.serving(),
        held: conversation.questions().cloned(),
        streamed: Mutex::new(Vec::new()),
        posted: Mutex::new(Vec::new()),
    };
    let (steer, aside) = (Steer::new(), Aside::new());
    let ended = {
        let run = conversation.starting(&streaming, cancel, &steer, &aside);
        super::runtime()?.block_on(client::turn(
            conversation,
            request,
            Box::default(),
            front,
            &run,
        ))
    };
    let response = ended.response(request);

    Ok((Response::decode(&response.encode()?)?, streaming))
}

/// [`turning`], with a cancel nobody asks for and a client that answers
/// nothing.
pub(super) fn turned(
    conversation: &mut Conversation,
    request: &Request,
    held: &Questions,
) -> Result<(Response, Vec<Progress>), Failed> {
    turning(
        conversation,
        request,
        &mut Client::new(held),
        &Cancel::new(),
    )
}

/// A prompt of `words`, sent by a client that claims everything.
pub(super) fn prompt(words: &str) -> Result<Request, Failed> {
    sent(Capabilities::every(), Command::Prompt(Prompt::new(words)?))
}

/// Each waiting set a client was streamed, in order, as the headings of its
/// batches' questions.
fn waits(streamed: &[Progress]) -> Vec<Vec<String>> {
    streamed
        .iter()
        .filter_map(|progress| match progress {
            Progress::Waiting { batches, .. } => Some(
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
    let held = Questions::new();
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
                && let Some(request) = rust_to_oldest(&held)
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
    let held = Questions::new();
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
    let (era, id) = last_shown(&streamed)?;

    let (response, streamed) = turned(&mut conversation, &rust(&era, id)?, &held)?;

    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    assert_eq!(asked.load(Ordering::Relaxed), 3, "one request delivered it");
    let first = said_in(&sent_to, 2)?;
    assert_eq!(first.first().map(String::as_str), Some("plan it"));
    assert!(
        matches!(first.as_slice(), [_, answer] if answer.starts_with(ANSWERED)),
        "{first:?}"
    );
    assert_eq!(waits(&streamed), [Vec::<String>::new()]);
    assert_eq!(
        client::answer(&rust(&era, id)?, &held),
        Outcome::Refused(ErrorCode::StaleQuestions.into()),
        "the answer was taken, and cannot be given twice"
    );
    Ok(())
}

/// A turn whose last answer was cut short ends with that stop, the answer
/// settled meanwhile still undelivered; the door starts the turn that delivers
/// it straight after, and that turn's outcome is the one response.
#[test]
fn a_cut_short_answer_ends_its_turn_and_one_that_delivers_starts_right_after() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-cut-short")?;
    let held = Questions::new();
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
                && let Some(request) = rust_to_oldest(&held)
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
    let held = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = self::held(&tree, script, &held)?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let (era, waiting) = oldest(&held)?;
    let stale = Outcome::Refused(ErrorCode::StaleQuestions.into());

    let nobody = rust(&era, QuestionId::numbered(waiting.number() + 40))?;
    assert_eq!(client::answer(&nobody, &held), stale);
    assert_eq!(turned(&mut conversation, &nobody, &held)?.0.outcome, stale);

    let one_of = sent(
        Capabilities::every(),
        choosing(&era, waiting, &["Rust", "Python"])?,
    )?;
    let twice = sent(
        Capabilities::every(),
        Command::Answer {
            era: era.clone(),
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
                matches!(&outcome, Outcome::Answer(AnswerOutcome::Invalid { .. })),
                "{outcome:?}"
            );
        }
    }

    assert_eq!(held.waiting().1.len(), 1, "a refused answer settled it");
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
    let held = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let mut conversation = self::held(&tree, script, &held)?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let (era, waiting) = oldest(&held)?;
    let frame: serde_json::Value = serde_json::from_slice(&rust(&era, waiting)?.encode()?)?;
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
    assert_eq!(held.waiting().1.len(), 1, "the registry was reached");

    let request = rust(&era, waiting)?;
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
    let held = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let mut conversation = self::held(&tree, script, &held)?;
    let claims = Capabilities::every().without(Capability::Questions);
    let request = sent(claims, Command::Prompt(Prompt::new("plan it")?))?;

    let (_, streamed) = turned(&mut conversation, &request, &held)?;

    assert!(waits(&streamed).is_empty(), "{streamed:?}");
    let (era, id) = oldest(&held)?;
    let answer = Request::new(claims, Correlation::new(8), choosing(&era, id, &["Rust"])?);
    let needs = Outcome::Refused(ErrorCode::NeedsCapability.into());
    assert_eq!(client::answer(&answer, &held), needs);
    assert_eq!(turned(&mut conversation, &answer, &held)?.0.outcome, needs);
    assert_eq!(held.waiting().1.len(), 1, "a refused answer settled it");
    Ok(())
}

/// With no handle from the front end, a question held for later is refused
/// to the model, and nothing waits for a client to answer.
#[test]
fn a_front_end_with_no_registry_holds_no_question_for_later() -> Result<(), Failed> {
    let tree = Tree::new("deferring-unheld")?;
    let script = Script::new(vec![deferring("c1", "Language"), saying("asked in words")]);
    let mut conversation = holding(&tree, script, None, None, Tools::new())?;
    let nowhere = Questions::new();

    let (_, streamed) = turned(&mut conversation, &prompt("plan it")?, &nowhere)?;

    assert!(
        streamed.iter().any(
            |progress| matches!(progress, Progress::ToolFinished { failed: true, call } if call.as_str() == "c1")
        ),
        "{streamed:?}"
    );
    assert!(waits(&streamed).is_empty(), "{streamed:?}");
    let (era, _) = nowhere.waiting();
    let (response, _) = turned(
        &mut conversation,
        &rust(&era, QuestionId::numbered(1))?,
        &nowhere,
    )?;
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
    let held = Questions::new();
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

    // The batch is asked before the route warns, so the answer is one that
    // could be taken.
    let tree = Tree::new("deferring-warned")?;
    let held = Questions::new();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        saying("asked"),
        saying("answered"),
    ]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = holding(&tree, script, Some(&held), Some("google"), Tools::new())?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let (era, id) = oldest(&held)?;
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
    let mut client = Client::new(&held);

    let (response, _) = turning(
        &mut conversation,
        &rust(&era, id)?,
        &mut client,
        &Cancel::new(),
    )?;

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
    assert_eq!(asked.load(Ordering::Relaxed), 2, "something was sent");
    assert_eq!(
        held.waiting().1.len(),
        1,
        "settled without a yes to the route"
    );
    let written = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(!written.contains("key:google"), "{written}");
    Ok(())
}

/// With nothing settled there is no turn to deliver it, and none is said to
/// have run.
#[test]
fn delivering_with_nothing_settled_runs_no_turn_and_says_none_ran() -> Result<(), Failed> {
    let tree = Tree::new("deferring-nothing")?;
    let held = Questions::new();
    let script = Script::new(vec![saying("never asked")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = self::held(&tree, script, &held)?;
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());

    let delivered = {
        let run = conversation.starting(&events, &cancel, &steer, &aside);
        super::runtime()?.block_on(conversation.answering(&mut super::Nobody, &run))
    };
    drop(events);

    assert!(delivered.is_none(), "{delivered:?}");
    assert_eq!(asked.load(Ordering::Relaxed), 0);
    assert_eq!(reported.try_iter().count(), 0, "a turn was reported");
    Ok(())
}

/// An answer that could settle nothing is refused before the route is put:
/// nobody is asked to agree to a send that cannot happen, and no yes is
/// written for it.
#[test]
fn an_answer_that_cannot_be_taken_is_refused_before_a_warned_route_is_put() -> Result<(), Failed> {
    use crucible_app::content_use::{Consent, Routes, Serving};

    let tree = Tree::new("deferring-warned-first")?;
    let held = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("asked")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = holding(&tree, script, Some(&held), Some("google"), Tools::new())?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let (era, id) = oldest(&held)?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(tree.0.join("config.json"));
    let mut conversation = conversation.consenting(consent.clone());
    consent.served(
        "google",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    let unclaimed = sent(
        Capabilities::every().without(Capability::Questions),
        choosing(&era, id, &["Rust"])?,
    )?;
    let stale = rust(&era, QuestionId::numbered(id.number() + 5))?;

    for (request, refused) in [
        (unclaimed, ErrorCode::NeedsCapability),
        (stale, ErrorCode::StaleQuestions),
    ] {
        let mut client = Client::new(&held);
        let (response, _) = turning(&mut conversation, &request, &mut client, &Cancel::new())?;

        assert_eq!(response.outcome, Outcome::Refused(refused.into()));
        assert!(client.put.is_empty(), "{:?}", client.put);
    }
    assert_eq!(asked.load(Ordering::Relaxed), 2);
    assert_eq!(held.waiting().1.len(), 1);
    Ok(())
}

/// Stopping a turn leaves what waits and what was settled as they were and
/// starts nothing; `/clear` leaves the session they belonged to, and empties
/// them.
#[test]
fn cancelling_a_turn_keeps_its_questions_and_clearing_empties_them() -> Result<(), Failed> {
    let tree = Tree::new("deferring-cancel")?;
    let held = Questions::new();
    let cancel = Cancel::new();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        deferring("c2", "Storage"),
        saying("stopped mid-way"),
        saying("with the answer kept"),
    ])
    .meanwhile({
        let (held, cancel) = (held.clone(), cancel.clone());
        move |before| {
            if before == 2
                && let Some(request) = rust_to_oldest(&held)
            {
                drop(client::answer(&request, &held));
                cancel.request();
            }
        }
    });
    let (asked, sent_to) = (Arc::clone(&script.asked), Arc::clone(&script.sent));
    let mut conversation = self::held(&tree, script, &held)?;

    let (response, streamed) = turning(
        &mut conversation,
        &prompt("plan it")?,
        &mut Client::new(&held),
        &cancel,
    )?;

    assert!(ran(&response, Stop::Cancelled), "{:?}", response.outcome);
    assert_eq!(stops(&streamed), [Stop::Cancelled], "a turn was started");
    assert_eq!(asked.load(Ordering::Relaxed), 3, "a request was made");
    assert_eq!(delivered(&sent_to)?, [0, 0, 0]);
    assert_eq!(held.waiting().1.len(), 1, "the waiting batch was dropped");
    let (left, storage) = oldest(&held)?;

    turned(&mut conversation, &prompt("and then?")?, &held)?;
    assert_eq!(
        delivered(&sent_to)?,
        [0, 0, 0, 1],
        "the settled answer was dropped"
    );

    super::runtime()?.block_on(conversation.clear(&tree.sessions(), &tree.workspace()?, None))?;

    let (era, waiting) = held.waiting();
    assert!(waiting.is_empty(), "{waiting:?}");
    assert_ne!(era, left);
    let (response, _) = turned(&mut conversation, &rust(&left, storage)?, &held)?;
    assert_eq!(
        response.outcome,
        Outcome::Refused(ErrorCode::StaleQuestions.into())
    );
    assert_eq!(asked.load(Ordering::Relaxed), 4, "a turn was started");
    Ok(())
}

/// Two sessions number their batches alike, so a batch of one can share its
/// id with a batch of the other: the era tells them apart. Session B is left
/// by a host with a batch waiting; a host started again asks its own first
/// batch in a new session A and shows it, then `/clear`s and resumes B, whose
/// batch comes back under the same id. A's answer settles nothing of B's, and
/// the same replies under B's era do.
#[test]
fn an_answer_shown_in_one_session_settles_nothing_of_another_with_the_same_id() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-two-sessions")?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let before = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("left waiting")]);
    let mut left = self::held(&tree, script, &before)?;
    turned(&mut left, &prompt("plan B")?, &before)?;
    let b = left.session().id().cloned().ok_or("an id")?;
    drop(left);

    let held = Questions::new();
    let script = Script::new(vec![
        deferring("c2", "Storage"),
        saying("asked in A"),
        saying("back in B"),
        saying("with B's answer"),
    ]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = self::held(&tree, script, &held)?;
    let (_, streamed) = turned(&mut conversation, &prompt("plan A")?, &held)?;
    let (in_a, shown) = last_shown(&streamed)?;
    super::runtime()?.block_on(conversation.clear(&sessions, &workspace, None))?;
    let _left = super::runtime()?.block_on(conversation.resume(&sessions, &workspace, &b, &[]))?;
    let (_, streamed) = turned(&mut conversation, &prompt("in B again")?, &held)?;
    assert_eq!(waits(&streamed).last(), Some(&vec!["Language".to_owned()]));
    let (in_b, restored) = last_shown(&streamed)?;
    assert_eq!(restored, shown, "the two sessions' batches share no id");
    assert_eq!(in_b, Era::Session(b));
    assert_ne!(in_a, in_b);
    let counted = asked.load(Ordering::Relaxed);

    let stale = Outcome::Refused(ErrorCode::StaleQuestions.into());
    assert_eq!(client::answer(&rust(&in_a, shown)?, &held), stale);
    assert_eq!(
        turned(&mut conversation, &rust(&in_a, shown)?, &held)?
            .0
            .outcome,
        stale
    );
    assert_eq!(asked.load(Ordering::Relaxed), counted, "a turn was started");
    assert_eq!(oldest(&held)?, (in_b.clone(), restored), "B's batch moved");

    assert_eq!(
        client::answer(&rust(&in_b, restored)?, &held),
        Outcome::Answer(AnswerOutcome::Settled)
    );
    Ok(())
}

/// The same session left and picked up again is the same era: an answer it
/// was shown is refused while its batch has not come back, starting nothing,
/// and is taken once the session's first turn has brought it back, to be
/// delivered once.
#[test]
fn an_answer_to_a_session_picked_up_again_is_taken_once_its_batch_is_back() -> Result<(), Failed> {
    let tree = Tree::new("deferring-same-session")?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let held = Questions::new();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        saying("asked in A"),
        saying("in B"),
        saying("back in A"),
        saying("with the answer in hand"),
    ]);
    let (asked, sent_to) = (Arc::clone(&script.asked), Arc::clone(&script.sent));
    let mut conversation = self::held(&tree, script, &held)?;
    let a = conversation.session().id().cloned().ok_or("an id")?;
    let (_, streamed) = turned(&mut conversation, &prompt("plan A")?, &held)?;
    let (era, shown) = last_shown(&streamed)?;
    assert_eq!(era, Era::Session(a.clone()));
    super::runtime()?.block_on(conversation.clear(&sessions, &workspace, None))?;
    turned(&mut conversation, &prompt("in B")?, &held)?;
    let _left = super::runtime()?.block_on(conversation.resume(&sessions, &workspace, &a, &[]))?;
    assert_eq!(emptied(&held)?, era);

    let (response, _) = turned(&mut conversation, &rust(&era, shown)?, &held)?;

    assert_eq!(
        response.outcome,
        Outcome::Refused(ErrorCode::StaleQuestions.into())
    );
    assert_eq!(asked.load(Ordering::Relaxed), 3, "a turn was started");

    let (_, streamed) = turned(&mut conversation, &prompt("back")?, &held)?;
    assert_eq!(last_shown(&streamed)?, (era.clone(), shown));

    let (response, _) = turned(&mut conversation, &rust(&era, shown)?, &held)?;

    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    assert_eq!(delivered(&sent_to)?, [0, 0, 0, 0, 1], "delivered once");
    Ok(())
}

/// A batch delivered before its session was left does not come back with
/// it: an answer to it is refused in the session picked up again, before and
/// after its first turn, and a batch the session asks next takes a new id,
/// even from a host started again. The session's log lists a delivered batch
/// as waiting until its next request records it gone, so the session here
/// makes one more before it is left.
#[test]
fn a_batch_delivered_before_its_session_was_left_stays_answered() -> Result<(), Failed> {
    let tree = Tree::new("deferring-delivered")?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let before = Questions::new();
    let script = Script::new(vec![
        deferring("c1", "Language"),
        saying("asked"),
        saying("with the answer in hand"),
        saying("and after"),
    ]);
    let mut left = self::held(&tree, script, &before)?;
    let a = left.session().id().cloned().ok_or("an id")?;
    let (_, streamed) = turned(&mut left, &prompt("plan A")?, &before)?;
    let (era, shown) = last_shown(&streamed)?;
    let (response, _) = turned(&mut left, &rust(&era, shown)?, &before)?;
    assert!(ran(&response, Stop::Yielded), "{:?}", response.outcome);
    turned(&mut left, &prompt("and then?")?, &before)?;
    drop(left);

    let held = Questions::new();
    let script = Script::new(vec![deferring("c2", "Storage"), saying("asked again")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = self::held(&tree, script, &held)?;
    let _left = super::runtime()?.block_on(conversation.resume(&sessions, &workspace, &a, &[]))?;
    let stale = Outcome::Refused(ErrorCode::StaleQuestions.into());

    assert_eq!(
        turned(&mut conversation, &rust(&era, shown)?, &held)?
            .0
            .outcome,
        stale
    );
    assert_eq!(asked.load(Ordering::Relaxed), 0, "a turn was started");
    let (_, streamed) = turned(&mut conversation, &prompt("again")?, &held)?;
    let (again, asked_next) = last_shown(&streamed)?;
    assert_eq!(again, era);
    assert_ne!(asked_next, shown, "an id was handed out twice");
    assert_eq!(waits(&streamed).last(), Some(&vec!["Storage".to_owned()]));
    assert_eq!(client::answer(&rust(&era, shown)?, &held), stale);
    Ok(())
}

/// What waits is read when it is shown, not when it was reported: a report
/// posted in session A and converted after `/clear` and a resume of B shows
/// B's era with B's batches, and an answer built from it settles only B's.
#[test]
fn a_report_converted_after_the_session_moved_shows_the_session_it_moved_to() -> Result<(), Failed>
{
    let tree = Tree::new("deferring-late-report")?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let held = Questions::new();
    let script = Script::new(vec![
        deferring("c1", "Storage"),
        saying("asked in B"),
        deferring("c2", "Language"),
        saying("asked in A"),
        saying("back in B"),
    ]);
    let mut conversation = self::held(&tree, script, &held)?;
    let b = conversation.session().id().cloned().ok_or("an id")?;
    turned(&mut conversation, &prompt("plan B")?, &held)?;
    super::runtime()?.block_on(conversation.clear(&sessions, &workspace, None))?;
    let request = prompt("plan A")?;
    let (_, streaming) = reporting(
        &mut conversation,
        &request,
        &mut Client::new(&held),
        &Cancel::new(),
    )?;
    let posted = streaming.posted.into_inner().map_err(|_| "poisoned")?;
    let late = posted
        .iter()
        .rev()
        .find(|event| matches!(event, Event::Waiting { .. }))
        .ok_or("nothing waited in A")?;
    let _left = super::runtime()?.block_on(conversation.resume(&sessions, &workspace, &b, &[]))?;
    turned(&mut conversation, &prompt("in B again")?, &held)?;

    let shown = client::progress(Capabilities::every(), late, None, Some(&held))
        .ok_or("the report was not shown")?;

    let shown = Progress::decode(&shown.encode()?)?;
    assert_eq!(
        waits(std::slice::from_ref(&shown)),
        [vec!["Storage".to_owned()]]
    );
    let (era, id) = last_shown(&[shown])?;
    assert_eq!(era, Era::Session(b));
    assert_eq!(
        client::answer(&rust(&era, id)?, &held),
        Outcome::Answer(AnswerOutcome::Settled)
    );
    assert!(held.waiting().1.is_empty(), "{:?}", held.waiting());
    Ok(())
}

/// The doors that do not deliver an answer refuse one as they refuse a
/// decision handed in on its own, and the batch waits as it did.
#[test]
fn an_answer_at_a_door_that_cannot_deliver_it_is_refused_as_stale() -> Result<(), Failed> {
    let tree = Tree::new("deferring-doors")?;
    let held = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("carrying on")]);
    let mut conversation = self::held(&tree, script, &held)?;
    turned(&mut conversation, &prompt("plan it")?, &held)?;
    let (era, id) = oldest(&held)?;
    let request = rust(&era, id)?;
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

    assert_eq!(held.waiting().1.len(), 1);
    Ok(())
}
