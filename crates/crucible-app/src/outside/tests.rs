use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use crucible_execution::{
    Capabilities, Currency, Description, Effects, Fault, Observation, Scope, SessionRef,
};
use crucible_runner::EventEnvelope;
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_tools::{Remember, Sensitivity, Verdict};
use crucible_types::ToolCall;

use super::*;

/// What one priced turn costs, in millionths of a dollar.
const PRICE: u64 = 250_000;

/// One turn as an agent tells it: what it posts, and how it ends.
type Told = (Vec<Event>, Result<Outcome, Error>);

/// An agent that tells each turn as it was scripted.
struct Scripted {
    turns: Mutex<VecDeque<Told>>,
}

impl Scripted {
    fn playing(turns: impl IntoIterator<Item = Told>) -> Box<dyn Backend> {
        Box::new(Self {
            turns: Mutex::new(turns.into_iter().collect()),
        })
    }
}

impl Backend for Scripted {
    fn describe(&self) -> Description {
        Description::new("claude", "work", Capabilities::unknown())
    }

    fn open(&self, _: Option<SessionRef>) -> BoxFuture<'_, Result<SessionRef, Error>> {
        Box::pin(async { Err(Failure::new("not opened here").into()) })
    }

    fn prompt<'a>(
        &'a self,
        _: TurnRequest,
        events: &'a dyn Sink,
    ) -> BoxFuture<'a, Result<Outcome, Error>> {
        let told = self.turns.lock().expect("the script").pop_front();
        Box::pin(async move {
            let (posted, ended) = told.expect("a turn scripted");
            for event in posted {
                events.post(event);
            }
            ended
        })
    }

    fn decide(&self, _: &PendingRef, _: Decision) -> Result<(), Error> {
        Ok(())
    }

    fn interrupt(&self) -> Result<(), Error> {
        Ok(())
    }

    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}

/// Launches the backends it holds in order, refusing full access, as an agent
/// with no launch for a mode would, and any launch past the last, and
/// remembers each resume it was asked for.
struct Again {
    backends: Mutex<VecDeque<Result<Box<dyn Backend>, Failure>>>,
    resumed: Resumed,
    /// What each ask for the models is answered, in order; none past the
    /// last.
    listing: Mutex<VecDeque<Listing>>,
    /// The model each launch was started on, in order.
    models_launched: Launched,
}

/// The model each launch was started on, in order.
type Launched = Arc<Mutex<Vec<Option<String>>>>;

/// The agent's own session each launch was asked to resume, in order.
type Resumed = Arc<Mutex<Vec<Option<String>>>>;

impl Again {
    fn launching(
        backends: impl IntoIterator<Item = Result<Box<dyn Backend>, Failure>>,
    ) -> (Box<dyn Relaunch>, Resumed) {
        let resumed = Arc::default();
        let again = Self {
            backends: Mutex::new(backends.into_iter().collect()),
            resumed: Arc::clone(&resumed),
            listing: Mutex::default(),
            models_launched: Arc::default(),
        };
        (Box::new(again), resumed)
    }

    /// [`Again::launching`], answering each ask for the models with the next
    /// of `listings`; with the model each launch was started on.
    fn listing(
        backends: impl IntoIterator<Item = Result<Box<dyn Backend>, Failure>>,
        listings: impl IntoIterator<Item = Listing>,
    ) -> (Box<dyn Relaunch>, Launched) {
        let models_launched = Arc::default();
        let again = Self {
            backends: Mutex::new(backends.into_iter().collect()),
            resumed: Arc::default(),
            listing: Mutex::new(listings.into_iter().collect()),
            models_launched: Arc::clone(&models_launched),
        };
        (Box::new(again), models_launched)
    }
}

impl Relaunch for Again {
    fn admits(&self, mode: Mode) -> Result<(), Failure> {
        match mode {
            Mode::FullAccess => Err(Failure::new("full access is not offered")),
            Mode::Ask | Mode::AllowEdits => Ok(()),
        }
    }

    fn model(&self) -> Option<&str> {
        None
    }

    fn models(&self) -> BoxFuture<'_, Listing> {
        let listing = self.listing.lock().expect("the listings").pop_front();
        Box::pin(async move { listing.unwrap_or(Listing::Unoffered) })
    }

    fn launch<'a>(
        &'a self,
        _: Mode,
        model: Option<&'a str>,
        resume: Option<&'a ExternalRef>,
    ) -> BoxFuture<'a, Result<Box<dyn Backend>, Failure>> {
        self.models_launched
            .lock()
            .expect("the record")
            .push(model.map(Into::into));
        self.resumed
            .lock()
            .expect("the record")
            .push(resume.map(|named| named.reference().to_owned()));
        let next = self.backends.lock().expect("the launches").pop_front();
        Box::pin(async move { next.unwrap_or_else(|| Err(Failure::new("nothing left"))) })
    }
}

/// Nobody to ask: every leave asked for is refused.
struct Nobody;

impl crucible_tools::Ask for Nobody {
    fn ask<'a>(
        &'a mut self,
        _: &'a ToolCall,
        _: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Deny, Remember::Never) })
    }
}

impl Asker for Nobody {}

/// A conversation on the agent `backend`, relaunched by `relaunch`.
fn outside(backend: Box<dyn Backend>, relaunch: Box<dyn Relaunch>) -> Outside {
    Outside::new(
        Installed {
            backend: Some(backend),
            relaunch,
            route: RouteBinding::new("claude", "work", None).expect("a route"),
        },
        Transcript::new(),
        Mode::Ask,
        None,
    )
}

/// A conversation whose agent tells `turns` in order.
fn scripted(turns: impl IntoIterator<Item = Told>) -> Outside {
    outside(Scripted::playing(turns), Again::launching([]).0)
}

/// Takes one turn of `outside`, answering with how it ended and what it
/// posted.
async fn turn(outside: &mut Outside) -> (Result<Turned, TurnError>, Vec<RunEvent>) {
    let (events, heard): (Sender<EventEnvelope>, Receiver<EventEnvelope>) = channel();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let run = RunContext::outside(&events, &cancel, &steer, &aside);
    let session = Session::nowhere();
    let turned = outside.turn("go", &mut Nobody, &run, &session).await;
    drop(events);
    let posted = heard.try_iter().map(EventEnvelope::into_event).collect();
    (turned, posted)
}

fn dollars() -> Currency {
    Currency::new("USD").expect("a currency")
}

/// A turn's figures: those of a priced turn, costing `cost` where it says.
fn usage(turn: TurnId, cost: Option<Cost>) -> Event {
    let usage = Usage::of(turn)
        .input(125)
        .cache_read(20)
        .cache_write(5)
        .output(40);
    let usage = match cost {
        Some(cost) => usage.cost(cost),
        None => usage,
    };
    Event::Usage(Observation::live(usage, Scope::Session))
}

/// A turn answered, costing `millionths` of a dollar where it says.
fn answered(turn: TurnId, millionths: Option<u64>) -> Told {
    let cost = millionths.map(|millionths| Cost::new(millionths, dollars()));
    (
        vec![
            Event::Started { turn },
            Event::Delta(Text::new("Done.")),
            usage(turn, cost),
            Event::Finished(Outcome::Answered),
        ],
        Ok(Outcome::Answered),
    )
}

/// A turn stopped with nothing reported of it.
fn stopped(turn: TurnId) -> Told {
    (
        vec![Event::Started { turn }, Event::Finished(Outcome::Cancelled)],
        Ok(Outcome::Cancelled),
    )
}

/// A turn that failed with nothing reported of it, its stream lost where
/// `lost`.
fn failed(turn: TurnId, lost: bool) -> Told {
    let failure = Failure::new("the agent stopped answering");
    let mut posted = vec![Event::Started { turn }];
    if lost {
        posted.push(Event::Process(Fault::new(Text::new("exit 9"))));
    }
    posted.push(Event::Failed(failure.clone()));
    (posted, Err(failure.into()))
}

/// `millionths` of a dollar, as the session's figures hold it.
fn amount(millionths: u64) -> CostAmount {
    CostAmount::new(
        DOLLARS,
        PricingUnit::Mixed,
        u128::from(millionths) * 1_000_000_000,
    )
}

/// The turns in order, one script each.
fn turns() -> impl Iterator<Item = TurnId> {
    std::iter::successors(Some(TurnId::FIRST), |turn| Some(turn.next()))
}

/// One turn's script, given the turn it is.
type Script = fn(TurnId) -> Told;

/// The cost `scripts` leave the session at, taking one turn of each.
async fn cost_after(scripts: impl IntoIterator<Item = Script>) -> SessionCost {
    let told: Vec<Told> = scripts
        .into_iter()
        .zip(turns())
        .map(|(f, t)| f(t))
        .collect();
    let count = told.len();
    let mut outside = scripted(told);
    for _ in 0..count {
        let _turned = turn(&mut outside).await;
    }
    outside.totals().cost()
}

fn priced(turn: TurnId) -> Told {
    answered(turn, Some(PRICE))
}

fn priced_twice(turn: TurnId) -> Told {
    answered(turn, Some(2 * PRICE))
}

fn unpriced(turn: TurnId) -> Told {
    answered(turn, None)
}

fn lost_failed(turn: TurnId) -> Told {
    failed(turn, true)
}

fn plain_failed(turn: TurnId) -> Told {
    failed(turn, false)
}

/// The seven sequences a session's cost is read from, over the events an
/// agent posts.
#[tokio::test]
async fn the_seven_cost_sequences_read_as_the_native_path_does() {
    let cases: [(&[Script], SessionCost); 7] = [
        (&[priced], SessionCost::Priced(amount(PRICE))),
        (&[unpriced], SessionCost::NotPriced),
        (&[priced, unpriced], SessionCost::NotPriced),
        (&[priced, priced], SessionCost::Priced(amount(2 * PRICE))),
        (&[priced, stopped], SessionCost::AtLeast(amount(PRICE))),
        (&[stopped, priced], SessionCost::NotPriced),
        (
            &[priced, stopped, priced_twice],
            SessionCost::AtLeast(amount(3 * PRICE)),
        ),
    ];
    for (scripts, expected) in cases {
        assert_eq!(cost_after(scripts.iter().copied()).await, expected);
    }
}

/// A turn that failed after a priced one may have spent and did not say.
#[tokio::test]
async fn a_failed_turn_after_a_priced_one_reads_at_least_its_sum() {
    assert_eq!(
        cost_after([priced as Script, plain_failed]).await,
        SessionCost::AtLeast(amount(PRICE))
    );
}

/// A lost stream posts its fault and then the failure that ends the turn,
/// and the turn is counted once: the floor is posted once, and the failure
/// is not posted at all, its caller posting it from the error.
#[tokio::test]
async fn a_fault_then_a_failure_is_counted_once() {
    let mut outside = scripted([priced(TurnId::FIRST), lost_failed(TurnId::FIRST.next())]);
    let _first = turn(&mut outside).await;

    let (turned, posted) = turn(&mut outside).await;

    assert!(turned.is_err());
    let floors: Vec<_> = posted
        .iter()
        .filter_map(|event| match event {
            RunEvent::Used { totals } => Some(totals.cost()),
            _ => None,
        })
        .collect();
    assert_eq!(floors, [SessionCost::AtLeast(amount(PRICE))]);
    assert!(
        !posted
            .iter()
            .any(|event| matches!(event, RunEvent::Failed { .. }))
    );
    assert_eq!(outside.totals().cost(), SessionCost::AtLeast(amount(PRICE)));
}

/// A launch refused before the agent's turn began leaves the figures as they
/// were, and posts nothing.
#[tokio::test]
async fn a_refused_launch_leaves_the_tally_as_it_was() {
    let (again, _) = Again::launching([Err(Failure::new("the sandbox would not start"))]);
    let mut outside = outside(Scripted::playing([priced(TurnId::FIRST)]), again);
    let _first = turn(&mut outside).await;
    let before = outside.totals();
    outside.switch(Mode::AllowEdits);

    let (turned, posted) = turn(&mut outside).await;

    let Err(TurnError::Provider(ProviderError::Transport { problem, .. })) = turned else {
        panic!("{turned:?}");
    };
    assert_eq!(&*problem, "the sandbox would not start");
    assert!(posted.is_empty(), "{posted:?}");
    assert_eq!(outside.totals().cost(), before.cost());
    assert_eq!(outside.totals().input(), before.input());
}

/// A sum too large to state in millionths, or a cost in another currency,
/// leaves the session not priced for good.
#[tokio::test]
async fn a_sum_that_cannot_be_stated_is_not_priced() {
    fn most(turn: TurnId) -> Told {
        answered(turn, Some(u64::MAX))
    }
    assert_eq!(
        cost_after([most as Script]).await,
        SessionCost::Priced(amount(u64::MAX))
    );
    assert_eq!(
        cost_after([most as Script, most]).await,
        SessionCost::NotPriced
    );

    let euros = Cost::new(PRICE, Currency::new("EUR").expect("a currency"));
    let mut tally = Tally::new();
    tally.begin();
    tally.used(&Usage::of(TurnId::FIRST).cost(euros));
    tally.end(true);
    tally.begin();
    tally.used(&Usage::of(TurnId::FIRST.next()).cost(Cost::new(PRICE, dollars())));
    tally.end(true);
    assert_eq!(tally.totals().cost(), SessionCost::NotPriced);
}

/// What a turn read stands until the next turn says its own, adds up within
/// a turn that says it twice, and stays where a turn says nothing of it.
#[test]
fn what_a_turn_read_stands_until_the_next_says_its_own() {
    let mut tally = Tally::new();
    assert_eq!(tally.read, 0);
    tally.begin();
    tally.used(&Usage::of(TurnId::FIRST).input(125));
    tally.used(&Usage::of(TurnId::FIRST).input(200));
    tally.end(true);
    assert_eq!(tally.read, 325);

    let second = TurnId::FIRST.next();
    tally.begin();
    assert_eq!(tally.read, 325, "the turn under way has said nothing");
    tally.used(&Usage::of(second).output(40));
    assert_eq!(tally.read, 325, "the turn said nothing of what it read");
    tally.used(&Usage::of(second).input(300));
    assert_eq!(tally.read, 300);
    tally.used(&Usage::of(second).input(u64::MAX));
    tally.end(true);
    assert_eq!(tally.read, u64::MAX);
}

/// The figures reported add up across turns, and a session that has begun
/// no turn reads as having used nothing.
#[tokio::test]
async fn tokens_add_up_across_turns() {
    assert_eq!(Tally::new().totals().cost(), SessionCost::Unspent);
    let mut outside = scripted([priced(TurnId::FIRST), priced_twice(TurnId::FIRST.next())]);
    let _first = turn(&mut outside).await;
    let _second = turn(&mut outside).await;

    let totals = outside.totals();
    assert_eq!(
        (
            totals.input(),
            totals.output(),
            totals.cache_read(),
            totals.cache_write()
        ),
        (250, 80, 40, 10)
    );
}

/// What a person sees of a turn is kept: the prompt and the answer, the
/// answer stopped as the turn was.
#[tokio::test]
async fn a_turn_keeps_its_prompt_and_its_answer() {
    let mut outside = scripted([priced(TurnId::FIRST)]);

    let (turned, posted) = turn(&mut outside).await;

    assert!(matches!(turned, Ok(Turned::Ran(_))), "{turned:?}");
    let messages = outside.transcript().messages();
    assert!(
        matches!(messages, [Message::User { text, .. }, Message::Agent { text: answer, stop: Some(StopReason::Yielded), .. }]
            if &**text == "go" && &**answer == "Done."),
        "{messages:?}"
    );
    assert!(posted.iter().any(|event| matches!(
        event,
        RunEvent::TurnFinished {
            stop: StopReason::Yielded,
            ..
        }
    )));
}

/// Each way an agent ends a turn is the stop the runner reports for it.
#[tokio::test]
async fn each_end_is_the_runner_s_stop() {
    for (outcome, stop) in [
        (Outcome::Answered, StopReason::Yielded),
        (Outcome::Cancelled, StopReason::Cancelled),
        (Outcome::CutShort, StopReason::Unknown),
    ] {
        let mut outside = scripted([(
            vec![
                Event::Started {
                    turn: TurnId::FIRST,
                },
                Event::Finished(outcome),
            ],
            Ok(outcome),
        )]);

        let (turned, posted) = turn(&mut outside).await;

        let Ok(Turned::Ran(result)) = turned else {
            panic!("{outcome:?}: {turned:?}");
        };
        assert_eq!(result.stop(), stop, "{outcome:?}");
        assert!(
            posted.iter().any(|event| matches!(
                event,
                RunEvent::TurnFinished { stop: posted, .. } if *posted == stop
            )),
            "{outcome:?}: {posted:?}"
        );
    }
}

/// A turn stopped with something under way may or may not have done it: it
/// answers as uncertain, and a lost one says so in its failure; either way
/// the next turn launches the agent again, resuming its own session.
#[tokio::test]
async fn an_uncertain_turn_says_so_and_its_run_is_replaced() {
    let under_way = || Event::Uncertain(Effects::new(Text::new("Edit src/lib.rs")));
    let opened = Event::Opened(SessionRef::new("own-session").expect("a reference"));
    let first = Scripted::playing([(
        vec![
            Event::Started {
                turn: TurnId::FIRST,
            },
            under_way(),
            Event::Finished(Outcome::Cancelled),
            opened,
        ],
        Ok(Outcome::Cancelled),
    )]);
    let failure = Failure::new("the agent stopped answering");
    let second = Scripted::playing([(
        vec![
            Event::Started {
                turn: TurnId::FIRST.next(),
            },
            under_way(),
            Event::Process(Fault::new(Text::new("exit 9"))),
            Event::Failed(failure.clone()),
        ],
        Err(failure.into()),
    )]);
    let (again, resumed) = Again::launching([Ok(second)]);
    let mut outside = outside(first, again);

    let (stopped, _) = turn(&mut outside).await;
    let (lost, _) = turn(&mut outside).await;

    assert!(
        matches!(&stopped, Ok(Turned::Uncertain(result)) if result.stop() == StopReason::Cancelled),
        "{stopped:?}"
    );
    let Err(TurnError::Provider(ProviderError::Transport { problem, .. })) = lost else {
        panic!("{lost:?}");
    };
    assert_eq!(
        &*problem,
        "the agent stopped answering What the agent was doing may or may not have happened."
    );
    assert_eq!(
        *resumed.lock().expect("the record"),
        [Some("own-session".to_owned())]
    );
}

/// Nothing is attached to an agent's turn, and a mode the agent has no launch
/// for, full access in this fake, is not taken: the mode stays, and a cycle
/// steps past it.
#[test]
fn attachments_and_unlaunchable_modes_are_refused() {
    let mut outside = scripted([]);
    let attached = [Attachment {
        path: "shot.png".into(),
        modality: crucible_types::Modality::Image,
        media_type: "image/png".into(),
        hash: [0; 32],
    }];

    let refused = unattached(&attached);

    assert!(matches!(refused, Err(TurnError::Refused(_))), "{refused:?}");
    assert!(unattached(&[]).is_ok());
    assert_eq!(outside.switch(Mode::FullAccess), Mode::Ask);
    assert_eq!(outside.cycle(), Mode::AllowEdits);
    assert_eq!(outside.cycle(), Mode::Ask);
}

/// What an agent lists: each model's value, `None` for its own choice.
fn offering(models: &[Option<&str>]) -> Listing {
    Listing::Listed(
        models
            .iter()
            .map(|model| Offered {
                model: model.map(Text::new),
                shown: Text::new(model.unwrap_or("Default")),
            })
            .collect(),
    )
}

/// A model is chosen only from what the agent listed, asked once, and is
/// what the next turn's run is started on; a listed value its program would
/// read as an option is refused like one never listed.
#[tokio::test]
async fn a_model_is_chosen_from_what_the_agent_lists_for_the_next_launch() {
    let second = Scripted::playing([priced(TurnId::FIRST.next())]);
    let (again, started) = Again::listing(
        [Ok(second)],
        [offering(&[None, Some("opus"), Some("--resume")])],
    );
    let mut outside = outside(Scripted::playing([priced(TurnId::FIRST)]), again);
    let _first = turn(&mut outside).await;

    assert_eq!(
        outside.choose(Some("sonnet")).await,
        Err(Unchosen::Unlisted)
    );
    assert_eq!(
        outside.choose(Some("--resume")).await,
        Err(Unchosen::Unlisted)
    );
    assert_eq!(outside.model(), None);
    assert_eq!(outside.choose(Some("opus")).await, Ok(()));
    assert_eq!(outside.model(), Some("opus"));
    let (second, _) = turn(&mut outside).await;

    assert!(second.is_ok(), "{second:?}");
    assert_eq!(
        *started.lock().expect("the record"),
        [Some("opus".to_owned())]
    );
    // Asked once: the agent's own choice is found in what it already said.
    assert_eq!(outside.choose(None).await, Ok(()));
    assert_eq!(outside.model(), None);
}

/// An agent with no way to list its models has none chosen for it, and its
/// run goes on as it was.
#[tokio::test]
async fn an_agent_that_lists_no_models_has_none_chosen() {
    let (again, started) = Again::listing([], [Listing::Unoffered]);
    let mut outside = outside(Scripted::playing([priced(TurnId::FIRST)]), again);

    assert_eq!(outside.choose(Some("opus")).await, Err(Unchosen::Unoffered));
    let (first, _) = turn(&mut outside).await;

    assert!(first.is_ok(), "{first:?}");
    assert!(started.lock().expect("the record").is_empty());
}

/// A listed value its program would read as an option, or that is no model's
/// name, is not offered, so neither front end shows a row it cannot take.
#[tokio::test]
async fn a_listed_value_that_is_no_model_s_name_is_not_offered() {
    let (again, _) = Again::listing(
        [],
        [offering(&[
            None,
            Some("opus"),
            Some("--resume"),
            Some("a b"),
        ])],
    );
    let mut outside = outside(Scripted::playing([priced(TurnId::FIRST)]), again);

    let listing = outside.models().await;
    let models: Vec<Option<&str>> = match &listing {
        Listing::Listed(offered) => offered
            .iter()
            .map(|offered| offered.model.as_ref().map(Text::as_str))
            .collect(),
        _ => Vec::new(),
    };
    assert_eq!(models, [None, Some("opus")], "{listing:?}");
}

/// The agent's own choice adds nothing to its program's arguments, so it is
/// taken whether or not the agent lists a row for it.
#[tokio::test]
async fn the_agent_s_own_choice_is_taken_where_it_lists_no_row_for_it() {
    let (again, _) = Again::listing([], [offering(&[Some("opus")])]);
    let mut outside = outside(Scripted::playing([priced(TurnId::FIRST)]), again);

    assert_eq!(outside.choose(Some("opus")).await, Ok(()));
    assert_eq!(outside.choose(None).await, Ok(()));
    assert_eq!(outside.model(), None);
}
