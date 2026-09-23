//! A conversation driven with no terminal anywhere in the process.
//!
//! Everything a front end does to a run — a prompt, `/clear`, `/resume`, a
//! choice written down to last — is asked of the application here through the
//! same calls the command line makes, and read back off what the application
//! reports: the value a turn comes back as, the events it posted on the way,
//! the session it says it left, and the file it wrote. Nothing is drawn, so
//! nothing here can pass because of what a screen happened to show.
//!
//! The helpers hand their failures back rather than panicking, so that the
//! only place a test stops is the assertion it is about.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use crucible_agents::{
    AgentBuilder, AgentContext, Decision, InputGuardrail, Model, OutputGuardrail, Undecided,
};
use crucible_app::providers::{
    CredentialSource, NO_PROVIDER_CHOSEN, NOTHING_TO_ASK, Providers, Resolved, Served, Serving,
    providers,
};
use crucible_app::startup::served;
use crucible_app::switching::{LoggedIn, LoggedOut, Retained, Rung, Switched, Switching};
use crucible_app::{AppError, Conversation, remember};
use crucible_auth::Store;
use crucible_config::{Home, Settings};
use crucible_models::{
    Delta, DeltaStream, Effort, PromptCacheCapabilities, PromptCacheRoute, Provider, ProviderError,
    Request,
};
use crucible_runner::{Event, EventEnvelope, Runner, Tools, Turned};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_session::Session;
use crucible_tools::{Ask, Remember, Sensitivity, Verdict};
use crucible_types::{
    AgentId, CredentialScopeId, Message, Modalities, Modality, PromptCacheEncoding, SessionId,
    StopReason, ToolCall,
};
use crucible_workspace::Workspace;

#[path = "headless/client.rs"]
mod client;
#[path = "headless/crossing.rs"]
mod crossing;

/// Whatever stopped a test before its assertion.
type Failed = Box<dyn std::error::Error>;

/// The runtime these conversations wait for their turns on, standing where
/// the application's own would: built once for the whole test binary, with
/// its workers driving whatever a turn waits on.
fn runtime() -> Result<tokio::runtime::Handle, Failed> {
    static RUNTIME: std::sync::OnceLock<std::io::Result<tokio::runtime::Runtime>> =
        std::sync::OnceLock::new();
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
    }) {
        Ok(runtime) => Ok(runtime.handle().clone()),
        Err(problem) => Err(format!("no runtime to take a turn on: {problem}").into()),
    }
}

/// A workspace, a sessions directory and a home, removed when this is dropped.
struct Tree(PathBuf);

impl Tree {
    /// A tree of its own. `name` keeps two tests in one process apart.
    fn new(name: &str) -> Result<Self, Failed> {
        let base = std::env::temp_dir().join(format!("crucible-app-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let tree = Self(base);
        fs::create_dir_all(tree.0.join("work"))?;
        Ok(tree)
    }

    fn workspace(&self) -> Result<Workspace, Failed> {
        Ok(Workspace::open(self.0.join("work"))?)
    }

    fn sessions(&self) -> PathBuf {
        self.0.join("logs")
    }

    /// This tree's home as crucible would find it, never the machine's own.
    fn home(&self) -> Result<Home, Failed> {
        Ok(Home::find(&|name: &str| {
            (name == crucible_config::HOME).then(|| OsString::from(self.0.join("home")))
        })?)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Answers each request with the next batch of deltas it was given, and counts
/// the requests.
#[derive(Debug)]
struct Script {
    name: &'static str,
    scope: CredentialScopeId,
    rounds: Mutex<std::vec::IntoIter<Vec<Delta>>>,
    asked: Arc<AtomicUsize>,
}

impl Script {
    fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            name: "script",
            scope: CredentialScopeId::new(),
            rounds: Mutex::new(rounds.into_iter()),
            asked: Arc::default(),
        }
    }

    /// A provider with nothing to say, answering to `name`: what a switch is
    /// handed, so that who is being asked can be read back off the runner.
    fn named(name: &'static str) -> Self {
        Self {
            name,
            ..Self::new(Vec::new())
        }
    }
}

impl Provider for Script {
    fn name(&self) -> &'static str {
        self.name
    }

    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("headless-script-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "script",
            endpoint: "script",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "headless-script-fixture-v1",
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
            self.asked.fetch_add(1, Ordering::Relaxed);
            let round = self
                .rounds
                .lock()
                .map_err(|_| ProviderError::Transport {
                    provider: "script",
                    problem: "poisoned".into(),
                })?
                .next()
                .unwrap_or_default();

            Ok(Box::new(Reading(round.into_iter())) as Box<dyn DeltaStream>)
        })
    }
}

/// The deltas of one round, handed over one at a time.
struct Reading(std::vec::IntoIter<Delta>);

impl DeltaStream for Reading {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(async move { self.0.next().map(Ok) })
    }
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

/// Refuses every prompt, by name and for a reason a reader can act on.
#[derive(Debug)]
struct Refusing;

impl InputGuardrail for Refusing {
    fn name(&self) -> &'static str {
        "no-secrets"
    }

    fn checking(&self, _context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        Ok(Decision::rejected("the prompt carries a credential"))
    }
}

/// Nobody to ask: no test here offers a tool, so there is nothing to allow.
struct Nobody;

impl Ask for Nobody {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Deny, Remember::Never)
    }
}

/// Cannot say either way, by name and with the reason it could not.
#[derive(Debug)]
struct Unsure;

impl InputGuardrail for Unsure {
    fn name(&self) -> &'static str {
        "classifier"
    }

    fn checking(&self, _context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        Err(Undecided::because("its model timed out"))
    }
}

/// Refuses every answer, after the model has been asked for it.
#[derive(Debug)]
struct Vetoing;

impl OutputGuardrail for Vetoing {
    fn name(&self) -> &'static str {
        "no-leaks"
    }

    fn checking(
        &self,
        _context: &AgentContext<'_>,
        _candidate: &str,
    ) -> Result<Decision, Undecided> {
        Ok(Decision::rejected("the answer repeats a credential"))
    }
}

/// What checks a conversation's words on the way in or out.
enum Guard {
    Nothing,
    Refusing,
    Unsure,
    Vetoing,
}

/// A conversation over `script`, recording into a session started in `tree`.
fn conversation(tree: &Tree, script: Script, guarded: bool) -> Result<Conversation, Failed> {
    let guard = if guarded {
        Guard::Refusing
    } else {
        Guard::Nothing
    };
    conversing(tree, script, &guard, None)
}

/// A conversation over `script` checked by `guard`, with `serving` as the
/// provider it is asking.
fn conversing(
    tree: &Tree,
    script: Script,
    guard: &Guard,
    serving: Option<&'static str>,
) -> Result<Conversation, Failed> {
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
    let agent = match guard {
        Guard::Nothing => agent,
        Guard::Refusing => agent.checking_input(Arc::new(Refusing))?,
        Guard::Unsure => agent.checking_input(Arc::new(Unsure))?,
        Guard::Vetoing => agent.checking_output(Arc::new(Vetoing))?,
    };
    let work = tree.0.join("work");

    Ok(Conversation::recording(session, serving, |session| {
        Runner::new(
            Box::new(script),
            Tools::new(),
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
    })
    .on(runtime()?))
}

/// Everything a switch is decided from, all of it under one tree: the
/// registry this build ships, a store and a settings file in the tree's home,
/// and a way of reaching a provider that reaches only the ones it was told to.
struct Desk {
    providers: Providers,
    settings: Settings,
    serving: Serving,
    logins: Store,
    choosing: PathBuf,
    reached: Arc<Mutex<Vec<&'static str>>>,
}

impl Desk {
    fn new(tree: &Tree, reachable: &'static [&'static str]) -> Result<Self, Failed> {
        let home = tree.home()?;
        let reached = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&reached);
        let serving: Serving = Box::new(move |one: Served, _stored| {
            if let Ok(mut seen) = seen.lock() {
                seen.push(one.name);
            }
            if reachable.contains(&one.name) {
                Ok(Resolved {
                    provider: Box::new(Script::named(one.name)),
                    source: CredentialSource::Environment(one.key.into()),
                })
            } else {
                Err(AppError::Authentication {
                    provider: one.name.into(),
                })
            }
        });

        Ok(Self {
            providers: providers()?.snapshot(),
            settings: Settings::default(),
            serving,
            logins: Store::in_home(home.path()),
            choosing: crucible_config::user(&home),
            reached,
        })
    }

    fn with(&self) -> Switching<'_> {
        Switching {
            providers: &self.providers,
            settings: &self.settings,
            serving: &self.serving,
            logins: &self.logins,
            choosing: &self.choosing,
        }
    }

    fn one(&self, name: &str) -> Result<Served, Failed> {
        Ok(served(&self.providers, name)?)
    }

    /// Which providers were reached for, in order.
    fn reached(&self) -> Vec<&'static str> {
        self.reached
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

/// What the settings file now says, read the way the next start reads it.
fn written(tree: &Tree) -> Result<Settings, Failed> {
    Ok(Settings::read_home(&tree.home()?)?)
}

/// Takes one turn, and says how it ended beside the prose it reported.
fn turn(conversation: &mut Conversation, prompt: &str) -> Result<(Turned, String), Failed> {
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let turned = {
        let run = conversation
            .runner()
            .starting(&events, &cancel, &steer, &aside);
        conversation.turn(prompt, Box::default(), &mut Nobody, &run)?
    };
    drop(events);

    Ok((turned, prose(&reported)))
}

/// The prose a turn reported, in the order it arrived.
fn prose(reported: &Receiver<EventEnvelope>) -> String {
    reported
        .try_iter()
        .filter_map(|envelope| match envelope.into_event() {
            Event::Delta { text } => Some(text),
            _ => None,
        })
        .collect()
}

/// How many things were said, either way: the typed context a turn is asked
/// under is in the transcript too, and is not what these tests are about.
fn spoken(conversation: &Conversation) -> usize {
    conversation
        .runner()
        .transcript()
        .messages()
        .iter()
        .filter(|message| matches!(message, Message::User { .. } | Message::Agent { .. }))
        .count()
}

#[test]
fn a_prompt_is_answered_and_what_was_said_is_reported_with_no_terminal() -> Result<(), Failed> {
    let tree = Tree::new("prompt")?;
    let script = Script::new(vec![saying("hello from the script")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = conversation(&tree, script, false)?;

    let (turned, said) = turn(&mut conversation, "hi")?;

    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert_eq!(turned.stop(), Some(StopReason::Yielded));
    assert_eq!(said, "hello from the script");
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    assert_eq!(spoken(&conversation), 2, "the prompt and the answer");
    Ok(())
}

#[test]
fn a_prompt_a_guardrail_refuses_comes_back_as_a_rejection_naming_it() -> Result<(), Failed> {
    // A refusal on the way in posts no event: the value the turn comes back as
    // is the only place a front end can learn of it, so it has to arrive here
    // whole — the guardrail, its reason, and no stop, because nothing ran.
    let tree = Tree::new("refused")?;
    let script = Script::new(vec![saying("never asked")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = conversation(&tree, script, true)?;

    let (turned, said) = turn(&mut conversation, "here is my key")?;

    let Turned::Rejected { rejection, stop } = turned else {
        panic!("a refused prompt is a rejection: {turned:?}");
    };
    assert_eq!(rejection.guard(), "no-secrets");
    assert_eq!(rejection.why(), "the prompt carries a credential");
    assert_eq!(stop, None, "nothing ran, so nothing stopped");
    assert_eq!(said, "", "nothing was reported on the way");
    assert_eq!(asked.load(Ordering::Relaxed), 0, "no provider was reached");
    Ok(())
}

#[test]
fn clearing_starts_a_new_session_and_hands_back_the_one_left() -> Result<(), Failed> {
    let tree = Tree::new("clear")?;
    let script = Script::new(vec![saying("before"), saying("after")]);
    let mut conversation = conversation(&tree, script, false)?;
    let first = conversation.session().path().to_owned();
    turn(&mut conversation, "one")?;

    let left = conversation.clear(&tree.sessions(), &tree.workspace()?, None)?;

    assert_eq!(
        left.path(),
        first,
        "the session handed back is the one left"
    );
    assert_ne!(conversation.session().path(), first);
    assert_eq!(spoken(&conversation), 0, "nothing is carried over");

    // What is said next lands in the new log and not the one that was left.
    assert_eq!(left.finish(), None);
    let before = fs::read_to_string(&first)?;
    turn(&mut conversation, "two")?;
    assert_eq!(conversation.session().finish(), None);

    assert_eq!(fs::read_to_string(&first)?, before);
    let now = fs::read_to_string(conversation.session().path())?;
    assert!(now.contains("after"), "{now}");
    assert!(!now.contains("before"), "{now}");
    Ok(())
}

#[test]
fn resuming_picks_a_session_back_up_with_what_it_held() -> Result<(), Failed> {
    let tree = Tree::new("resume")?;
    let script = Script::new(vec![saying("remembered")]);
    let mut conversation = conversation(&tree, script, false)?;
    let first = conversation.session().path().to_owned();
    let id = conversation.session().id().cloned();
    turn(&mut conversation, "one")?;
    let left = conversation.clear(&tree.sessions(), &tree.workspace()?, None)?;
    assert_eq!(left.finish(), None);
    // A log is held for as long as its session is: letting go of the one left
    // is what makes it somebody's to pick up again.
    drop(left);
    let second = conversation.session().path().to_owned();

    let id = id.expect("a started session has an id");
    let left = conversation.resume(&tree.sessions(), &tree.workspace()?, &id)?;

    assert_eq!(
        left.path(),
        second,
        "the session handed back is the one left"
    );
    assert_eq!(conversation.session().path(), first);
    assert_eq!(
        spoken(&conversation),
        2,
        "the prompt and the answer came back"
    );
    Ok(())
}

#[test]
fn a_session_nobody_recorded_is_refused_and_the_one_in_hand_is_kept() -> Result<(), Failed> {
    let tree = Tree::new("resume-missing")?;
    let script = Script::new(vec![saying("kept")]);
    let mut conversation = conversation(&tree, script, false)?;
    let first = conversation.session().path().to_owned();
    turn(&mut conversation, "one")?;

    let refused = conversation.resume(&tree.sessions(), &tree.workspace()?, &SessionId::new());

    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(conversation.session().path(), first);
    assert_eq!(spoken(&conversation), 2);
    Ok(())
}

#[test]
fn a_choice_written_down_is_what_the_next_start_reads() -> Result<(), Failed> {
    let tree = Tree::new("remember")?;
    let home = tree.home()?;
    let file = crucible_config::user(&home);

    remember::choosing(&file, "anthropic", "a-model-somebody-chose")?;
    remember::asking(&file, "anthropic")?;

    let settings = Settings::read_home(&home)?;
    assert_eq!(settings.provider(), Some("anthropic"));
    assert_eq!(settings.model("anthropic"), Some("a-model-somebody-chose"));
    Ok(())
}

#[test]
fn a_prompt_a_check_could_not_decide_comes_back_undecided_and_unasked() -> Result<(), Failed> {
    // Not a refusal: the check said nothing about the prompt, and the value has
    // to keep the two apart so a front end does not tell somebody their words
    // were rejected when nothing judged them.
    let tree = Tree::new("undecided")?;
    let script = Script::new(vec![saying("never asked")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = conversing(&tree, script, &Guard::Unsure, None)?;

    let (turned, said) = turn(&mut conversation, "hi")?;

    let Turned::Undecided { problem, stop } = turned else {
        panic!("a check that could not decide is undecided: {turned:?}");
    };
    assert_eq!(
        problem.to_string(),
        "the guardrail `classifier` could not decide: its model timed out"
    );
    assert_eq!(stop, None, "nothing ran, so nothing stopped");
    assert_eq!(said, "");
    assert_eq!(asked.load(Ordering::Relaxed), 0, "no provider was reached");
    Ok(())
}

#[test]
fn an_answer_a_guardrail_refuses_comes_back_rejected_with_how_the_turn_stopped()
-> Result<(), Failed> {
    // The model was asked and what it said was reported on the way, so the
    // stop is there: it is what tells a front end the refusal is of the answer
    // and not of the prompt.
    let tree = Tree::new("vetoed")?;
    let script = Script::new(vec![saying("the key is hunter2")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = conversing(&tree, script, &Guard::Vetoing, None)?;

    let (turned, said) = turn(&mut conversation, "what is the key")?;

    let Turned::Rejected { rejection, stop } = turned else {
        panic!("a refused answer is a rejection: {turned:?}");
    };
    assert_eq!(rejection.guard(), "no-leaks");
    assert_eq!(rejection.why(), "the answer repeats a credential");
    assert_eq!(stop, Some(StopReason::Yielded), "the model ran and stopped");
    assert_eq!(said, "the key is hunter2", "what streamed was provisional");
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn a_model_switched_to_within_a_provider_is_asked_for_and_written_down() -> Result<(), Failed> {
    let tree = Tree::new("switch-model")?;
    let desk = Desk::new(&tree, &["anthropic"])?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let switched = conversation.ask_for(
        desk.one("anthropic")?,
        "claude-haiku-4-5",
        None,
        &desk.with(),
    );

    assert!(
        matches!(
            switched,
            Switched::Taken {
                retained: Retained {
                    ambiguous: 0,
                    orphaned: 0
                },
                unwritten: None
            }
        ),
        "{switched:?}"
    );
    assert_eq!(conversation.runner().model(), "claude-haiku-4-5");
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(
        desk.reached(),
        Vec::<&str>::new(),
        "the provider already answering is not set up a second time"
    );
    let written = written(&tree)?;
    assert_eq!(written.provider(), Some("anthropic"));
    assert_eq!(written.model("anthropic"), Some("claude-haiku-4-5"));
    Ok(())
}

#[test]
fn a_provider_switched_to_is_the_one_asked_from_then_on() -> Result<(), Failed> {
    let tree = Tree::new("switch-provider")?;
    let desk = Desk::new(&tree, &["anthropic", "google"])?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let switched =
        conversation.ask_for(desk.one("google")?, "gemini-3.8-flash", None, &desk.with());

    assert!(
        matches!(
            switched,
            Switched::Taken {
                unwritten: None,
                ..
            }
        ),
        "{switched:?}"
    );
    assert_eq!(desk.reached(), ["google"]);
    assert_eq!(conversation.serving(), Some("google"));
    assert_eq!(conversation.runner().serving(), "google");
    assert_eq!(conversation.runner().model(), "gemini-3.8-flash");
    let written = written(&tree)?;
    assert_eq!(written.provider(), Some("google"));
    assert_eq!(written.model("google"), Some("gemini-3.8-flash"));
    Ok(())
}

#[test]
fn a_provider_nothing_can_reach_is_refused_and_the_one_answering_is_kept() -> Result<(), Failed> {
    let tree = Tree::new("switch-unreachable")?;
    let desk = Desk::new(&tree, &["anthropic"])?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let switched =
        conversation.ask_for(desk.one("google")?, "gemini-3.8-flash", None, &desk.with());

    assert!(
        matches!(
            switched,
            Switched::Unreachable(AppError::Authentication { .. })
        ),
        "{switched:?}"
    );
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(conversation.runner().serving(), "anthropic");
    assert_eq!(conversation.runner().model(), "script");
    assert_eq!(written(&tree)?.provider(), None, "nothing was written");
    Ok(())
}

#[test]
fn a_switch_is_refused_for_a_rung_the_model_does_not_serve() -> Result<(), Failed> {
    // Before anything is retired or replaced: a session thinking at a rung
    // Gemini's ladder does not hold keeps the provider, the model and the file
    // it had.
    let tree = Tree::new("switch-effort")?;
    let desk = Desk::new(&tree, &["anthropic", "google"])?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;
    let thought = conversation.think(Effort::Max, &desk.with());
    assert!(
        matches!(thought, Rung::Taken { unwritten: None }),
        "{thought:?}"
    );

    let switched =
        conversation.ask_for(desk.one("google")?, "gemini-3.8-flash", None, &desk.with());

    assert!(
        matches!(switched, Switched::Unsupported(Effort::Max)),
        "{switched:?}"
    );
    assert_eq!(desk.reached(), Vec::<&str>::new(), "nobody was reached for");
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(conversation.runner().model(), "script");
    assert_eq!(conversation.runner().effort(), Some(Effort::Max));
    let written = written(&tree)?;
    assert_eq!(written.provider(), None);
    assert_eq!(written.effort("anthropic"), Some(Effort::Max));
    Ok(())
}

#[test]
fn a_rung_gemini_does_not_serve_is_refused_and_the_one_in_force_is_kept() -> Result<(), Failed> {
    let tree = Tree::new("effort-refused")?;
    let desk = Desk::new(&tree, &["google"])?;
    let mut conversation = conversing(
        &tree,
        Script::named("google"),
        &Guard::Nothing,
        Some("google"),
    )?;
    let switched =
        conversation.ask_for(desk.one("google")?, "gemini-3.8-flash", None, &desk.with());
    assert!(matches!(switched, Switched::Taken { .. }), "{switched:?}");

    let thought = conversation.think(Effort::Max, &desk.with());

    assert!(matches!(thought, Rung::Unsupported), "{thought:?}");
    assert_eq!(conversation.runner().effort(), None);
    assert_eq!(written(&tree)?.effort("google"), None);
    Ok(())
}

#[test]
fn a_logout_falls_back_to_a_credential_the_store_never_held() -> Result<(), Failed> {
    let tree = Tree::new("logout-falls-back")?;
    let desk = Desk::new(&tree, &["anthropic"])?;
    desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;
    let mut conversation = conversing(
        &tree,
        Script::named("before"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;
    let anthropic = desk.one("anthropic")?;

    let left = conversation.log_out(anthropic, &desk.with());

    let LoggedOut::StillServed { source, .. } = left else {
        panic!("the environment still serves it: {left:?}");
    };
    assert_eq!(source, CredentialSource::Environment(anthropic.key.into()));
    assert_eq!(desk.logins.read().providers().count(), 0, "the key is gone");
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(
        conversation.runner().serving(),
        "anthropic",
        "set up again from what remains"
    );
    assert_eq!(conversation.runner().model(), "script", "the model is kept");
    Ok(())
}

#[test]
fn a_logout_with_nothing_to_fall_back_to_leaves_nobody_asked() -> Result<(), Failed> {
    let tree = Tree::new("logout-signed-out")?;
    let desk = Desk::new(&tree, &[])?;
    desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let left = conversation.log_out(desk.one("anthropic")?, &desk.with());

    assert!(matches!(left, LoggedOut::SignedOut { .. }), "{left:?}");
    assert_eq!(desk.logins.read().providers().count(), 0);
    assert_eq!(conversation.serving(), None);
    assert_eq!(
        conversation.runner().model(),
        "",
        "a model belongs to a vendor"
    );
    assert_eq!(conversation.runner().serving(), "none");
    let refused = turn(&mut conversation, "anybody there")
        .err()
        .map(|problem| problem.to_string())
        .unwrap_or_default();
    assert!(refused.contains(NOTHING_TO_ASK), "{refused}");
    Ok(())
}

#[test]
fn a_logout_that_leaves_other_providers_reachable_says_none_is_chosen() -> Result<(), Failed> {
    let tree = Tree::new("logout-unchosen")?;
    let desk = Desk::new(&tree, &["google"])?;
    desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let left = conversation.log_out(desk.one("anthropic")?, &desk.with());

    assert!(matches!(left, LoggedOut::SignedOut { .. }), "{left:?}");
    assert_eq!(conversation.serving(), None);
    let refused = turn(&mut conversation, "anybody there")
        .err()
        .map(|problem| problem.to_string())
        .unwrap_or_default();
    assert!(refused.contains(NO_PROVIDER_CHOSEN), "{refused}");
    Ok(())
}

#[test]
fn a_logout_of_a_provider_nobody_is_asking_changes_nothing_else() -> Result<(), Failed> {
    let tree = Tree::new("logout-other")?;
    let desk = Desk::new(&tree, &["anthropic", "google"])?;
    desk.logins.keep("google", "a-key-no-vendor-issued")?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let left = conversation.log_out(desk.one("google")?, &desk.with());

    assert!(matches!(left, LoggedOut::Kept), "{left:?}");
    assert_eq!(desk.logins.read().providers().count(), 0);
    assert_eq!(desk.reached(), Vec::<&str>::new());
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(conversation.runner().serving(), "anthropic");
    Ok(())
}

#[test]
fn a_credential_given_to_a_session_asking_nobody_is_who_it_asks_next() -> Result<(), Failed> {
    let tree = Tree::new("login-first")?;
    let desk = Desk::new(&tree, &["anthropic"])?;
    let mut conversation = conversing(&tree, Script::named("none"), &Guard::Nothing, None)?;

    let signed = conversation.logged_in(desk.one("anthropic")?, &desk.with());

    assert!(
        matches!(
            signed,
            LoggedIn::Serving {
                unwritten: None,
                ..
            }
        ),
        "{signed:?}"
    );
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(conversation.runner().serving(), "anthropic");
    assert_eq!(
        conversation.runner().model(),
        "",
        "a name left over from before belongs to no vendor"
    );
    assert_eq!(written(&tree)?.provider(), Some("anthropic"));
    Ok(())
}

#[test]
fn a_credential_for_another_provider_keeps_the_one_already_answering() -> Result<(), Failed> {
    let tree = Tree::new("login-second")?;
    let desk = Desk::new(&tree, &["anthropic", "google"])?;
    let mut conversation = conversing(
        &tree,
        Script::named("anthropic"),
        &Guard::Nothing,
        Some("anthropic"),
    )?;

    let signed = conversation.logged_in(desk.one("google")?, &desk.with());

    assert!(matches!(signed, LoggedIn::Elsewhere), "{signed:?}");
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert_eq!(conversation.runner().serving(), "anthropic");
    assert_eq!(conversation.runner().model(), "script");
    assert_eq!(written(&tree)?.provider(), None);
    Ok(())
}
