//! A session's web sources name the model it is asking now, from the provider
//! it is asking now, signed with the credential it is served with now, and
//! are held as that model's route is.
//!
//! Every search here is a `web_search` call the model makes in a turn, run by
//! the tool the session registered at its start, so what is proved is what the
//! registered tool reaches and not only what the conversation holds.

use std::sync::Mutex;

use crucible_models::Delta;
use crucible_types::{StopReason, ToolId};

use super::lending::Rounds;
use super::*;
use crate::providers::{Sourcing, re_serving, re_sourcing};
use crate::sample::transcribing;
use crate::switching::{LoggedIn, LoggedOut, Switched, Switching};

/// The key the environment answers with, where it answers.
const KEY: &str = "fabricated-web-key";

/// Where a run's keys come from.
#[derive(Clone, Copy)]
struct Keys {
    /// What the environment holds when the run starts.
    starting: Option<&'static str>,
    /// What the environment holds when a switch reads it.
    switching: Option<&'static str>,
    /// What `/login` wrote down for the provider the run starts on.
    stored: Option<&'static str>,
}

impl Keys {
    /// A key exported for every provider, read the same at every point.
    const EXPORTED: Self = Self {
        starting: Some(KEY),
        switching: Some(KEY),
        stored: None,
    };

    /// Only a key `/login` wrote down.
    const fn stored(key: &'static str) -> Self {
        Self {
            starting: None,
            switching: None,
            stored: Some(key),
        }
    }
}

/// A run assembled on `provider` and `model`, with its own files and what a
/// switch is decided from, as the terminal hands it over.
struct Run {
    _sample: Sample,
    services: Services,
    settings: Settings,
    providers: Providers,
    serving: providers::Serving,
    sourcing: Sourcing,
    logins: crucible_auth::Store,
    choosing: std::path::PathBuf,
    conversation: Conversation,
}

/// Allows every call it is asked about, and counts them.
#[derive(Default)]
struct Counting(usize);

impl Ask for Counting {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
        self.0 += 1;
        Box::pin(async { (Verdict::Allow, Remember::Never) })
    }
}

impl Run {
    fn new(tree: &str, provider: &str, model: &'static str, document: &str, keys: Keys) -> Self {
        let sample = Sample::new(tree);
        let (logs, workspace) = (sample.logs(), sample.workspace());
        let settings = sample.user(document);
        let services = Services::new();
        let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
        let logins = sample.store();
        if let Some(key) = keys.stored {
            logins.keep(row(provider), key).expect("a writable home");
        }
        // Looked up once the model asks for it, as `tool_search` would.
        let revealed = Revealed::new();
        revealed.reveal("web_search");

        let conversation = assemble(&Startup {
            providers: &catalogue(),
            provider: Some(serving(provider)),
            unasked: NO_MODEL_CHOSEN,
            model: Some(model),
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_builtins::Background::new(),
            services: &services,
            settings: &settings,
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &revealed,
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &|_| keys.starting.map(str::to_owned),
            stored: &logins.read(),
            subscriptions: &subscriptions,
        })
        .expect("a start with a key");

        let switching = keys.switching;
        Self {
            serving: re_serving(
                settings.clone(),
                subscriptions.clone(),
                Box::new(move |_| switching.map(str::to_owned)),
                services.http().clone(),
                services.consent().clone(),
            ),
            sourcing: re_sourcing(
                settings.clone(),
                subscriptions,
                Box::new(move |_| switching.map(str::to_owned)),
                services.http().clone(),
                services.consent().clone(),
            ),
            logins,
            choosing: sample.user_file(),
            providers: catalogue(),
            settings,
            services,
            conversation,
            _sample: sample,
        }
    }

    /// Runs `switch` on the conversation with what a switch is decided from.
    fn switching<T>(
        &mut self,
        switch: impl AsyncFnOnce(&mut Conversation, &Switching<'_>) -> T,
    ) -> T {
        let runtime = self.services.runtime().handle().unwrap();
        let with = Switching {
            providers: &self.providers,
            settings: &self.settings,
            serving: &self.serving,
            sourcing: &self.sourcing,
            logins: &self.logins,
            choosing: &self.choosing,
        };
        runtime.block_on(switch(&mut self.conversation, &with))
    }

    /// Asks `model` from `provider` from the next turn on, as `/model` does.
    fn ask_for(&mut self, provider: &str, model: &str) -> Switched {
        self.switching(async |conversation, with| {
            conversation
                .ask_for(serving(provider), model, None, with)
                .await
        })
    }

    fn switch(&mut self, provider: &str, model: &str) {
        let switched = self.ask_for(provider, model);
        assert!(matches!(switched, Switched::Taken { .. }), "{switched:?}");
    }

    /// A turn in which the model calls `web_search` once, run by the tool the
    /// session registered: what the call answered, and how many times the
    /// user was asked about it.
    fn search(&mut self) -> (String, usize) {
        self.conversation.runner.serve(Box::new(Rounds::new(vec![
            vec![
                Delta::ToolStarted {
                    id: ToolId::new("look"),
                    name: "web_search".into(),
                },
                Delta::ToolArgs(r#"{"query":"what the user is working on"}"#.into()),
                Delta::Stopped(StopReason::WantsTools),
            ],
            vec![
                Delta::Text("looked".into()),
                Delta::Stopped(StopReason::Yielded),
            ],
        ])));

        let runtime = self.services.runtime().handle().unwrap();
        let mut asked = Counting::default();
        let runner = &mut self.conversation.runner;
        let (events, _seen) = std::sync::mpsc::channel();
        let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
        let run = runner.starting(&events, &cancel, &steer, &aside);
        let turned = runtime.block_on(runner.turn("look it up", Box::new([]), &mut asked, &run));
        assert!(turned.is_ok(), "{turned:?}");

        let answered = runner
            .transcript()
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults(results) => Some(results),
                _ => None,
            })
            .flatten()
            .last()
            .map(|result| result.output.text().to_owned())
            .expect("the call was answered");
        (answered, asked.0)
    }
}

/// The `/login` row a key for `provider` is written under.
fn row(provider: &str) -> &'static str {
    crate::providers::Rows::production()
        .environment(provider)
        .map(|row| row.stored)
        .expect("a row for the provider")
}

/// Every request heard so far, once anything in flight has arrived.
fn heard(requests: &Mutex<Vec<String>>) -> Vec<String> {
    std::thread::sleep(std::time::Duration::from_millis(100));
    requests.lock().unwrap().clone()
}

fn at(url: &str, provider: &str) -> String {
    format!(r#"{{"providers": {{"{provider}": {{"baseUrl": "{url}"}}}}}}"#)
}

#[test]
fn a_search_after_a_switch_of_model_names_the_model_switched_to() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-follows-model",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys::EXPORTED,
    );

    run.switch("meta", "muse-spark-1.2");
    let _ = run.search();

    let heard = heard(&requests);
    assert!(
        heard
            .iter()
            .any(|request| request.contains("muse-spark-1.2\"")),
        "{heard:?}"
    );
    assert!(
        !heard
            .iter()
            .any(|request| request.contains("muse-spark-1.3")),
        "{heard:?}"
    );
}

#[test]
fn a_run_that_left_an_unaccepted_contributor_model_searches_on_its_twin() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-leaves-contributor",
        "meta",
        "muse-spark-1.3-contributor",
        &at(&url, "meta"),
        Keys::EXPORTED,
    );

    run.switch("meta", "muse-spark-1.3");
    let (searched, _) = run.search();

    let heard = heard(&requests);
    assert!(
        heard
            .iter()
            .any(|request| request.contains("muse-spark-1.3\"")),
        "{searched:?} {heard:?}"
    );
    assert!(
        !heard.iter().any(|request| request.contains("contributor")),
        "{heard:?}"
    );
}

#[test]
fn a_switch_to_a_contributor_model_holds_its_searches_until_its_yes() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-reaches-contributor",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys::EXPORTED,
    );
    let model = "muse-spark-1.3-contributor";

    run.switch("meta", model);
    let (held, _) = run.search();

    let route = content_use::model_route("meta", model);
    assert!(heard(&requests).is_empty(), "{held:?}");
    assert!(held.contains(&route), "{held:?}");

    run.services.consent().record(&route);
    let _ = run.search();
    let heard = heard(&requests);
    assert!(
        heard
            .iter()
            .any(|request| request.contains(&format!("{model}\""))),
        "{heard:?}"
    );
}

/// A switch of provider moves the searches with it: to the provider asked now,
/// signed with its credential, naming its model, and nothing more to the one
/// left.
#[test]
fn a_search_after_a_switch_of_provider_goes_to_the_provider_switched_to() {
    let (meta, from_meta) = transcribing();
    let (xai, from_xai) = transcribing();
    let document = format!(
        r#"{{"providers": {{"meta": {{"baseUrl": "{meta}"}}, "xai": {{"baseUrl": "{xai}"}}}}}}"#
    );
    let mut run = Run::new(
        "web-follows-provider",
        "meta",
        "muse-spark-1.3",
        &document,
        Keys::EXPORTED,
    );

    run.switch("xai", "grok-4.7");
    let _ = run.search();

    let heard_xai = heard(&from_xai);
    assert!(
        heard_xai
            .iter()
            .any(|request| request.contains("grok-4.7\"")),
        "{heard_xai:?}"
    );
    assert!(heard(&from_meta).is_empty());
}

/// A provider that gives the session no search leaves the registered tool
/// nothing to send with: the call is refused before anybody is asked about
/// it, says that nothing was sent, and nothing goes to the provider left.
#[test]
fn a_search_on_a_provider_with_no_search_is_refused_before_it_is_asked_about() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-follows-to-nothing",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys::EXPORTED,
    );

    run.switch("deepseek", "deepseek-flash");
    let (answered, asked) = run.search();

    assert!(answered.contains("nothing was sent"), "{answered}");
    assert_eq!(asked, 0, "{answered}");
    assert!(heard(&requests).is_empty());
}

/// A key given again with `/login` for the provider in force is the one the
/// next search is signed with.
#[test]
fn a_search_after_a_new_login_is_signed_with_the_new_key() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-follows-login",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys::stored("first-stored-key"),
    );

    run.logins
        .keep(row("meta"), "second-stored-key")
        .expect("a writable home");
    let logged = run
        .switching(async |conversation, with| conversation.logged_in(serving("meta"), with).await);
    assert!(matches!(logged, LoggedIn::Serving { .. }), "{logged:?}");
    let _ = run.search();

    let heard = heard(&requests);
    assert!(
        heard
            .iter()
            .any(|request| request.contains("Bearer second-stored-key")),
        "{heard:?}"
    );
    assert!(
        !heard
            .iter()
            .any(|request| request.contains("first-stored-key")),
        "{heard:?}"
    );
}

/// A key forgotten with `/logout` is never sent again: where nothing is left
/// to ask, a search sends nothing, and where the provider is still served, it
/// is signed with what serves it now.
#[test]
fn a_search_after_a_logout_never_carries_the_forgotten_key() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-follows-logout",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys::stored("forgotten-key"),
    );

    let left =
        run.switching(async |conversation, with| conversation.log_out(serving("meta"), with).await);
    assert!(matches!(left, LoggedOut::SignedOut { .. }), "{left:?}");
    let (answered, _) = run.search();

    assert!(answered.contains("nothing was sent"), "{answered}");
    assert!(heard(&requests).is_empty());

    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-follows-logout-served",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys {
            switching: Some("still-serving-key"),
            ..Keys::stored("forgotten-key")
        },
    );

    let left =
        run.switching(async |conversation, with| conversation.log_out(serving("meta"), with).await);
    assert!(matches!(left, LoggedOut::StillServed { .. }), "{left:?}");
    let _ = run.search();

    let heard = heard(&requests);
    assert!(
        heard
            .iter()
            .any(|request| request.contains("Bearer still-serving-key")),
        "{heard:?}"
    );
    assert!(
        !heard
            .iter()
            .any(|request| request.contains("forgotten-key")),
        "{heard:?}"
    );
}

/// A switch refused leaves the searches where they were: on the provider and
/// model the session still asks.
#[test]
fn a_refused_switch_leaves_the_searches_where_they_were() {
    let (url, requests) = transcribing();
    let mut run = Run::new(
        "web-stays-on-refusal",
        "meta",
        "muse-spark-1.3",
        &at(&url, "meta"),
        Keys::stored("standing-key"),
    );

    // No key anywhere for xAI.
    let switched = run.ask_for("xai", "grok-4.7");
    assert!(matches!(switched, Switched::Unreachable(_)), "{switched:?}");
    let _ = run.search();

    let heard = heard(&requests);
    assert!(
        heard
            .iter()
            .any(|request| request.contains("muse-spark-1.3\"")
                && request.contains("Bearer standing-key")),
        "{heard:?}"
    );
}

/// What the conversation holds and what the registered tool reaches are one
/// thing, whichever of the two a search goes through.
#[test]
fn the_registered_search_and_the_conversation_follow_one_source() {
    let mut run = Run::new(
        "web-one-source",
        "meta",
        "muse-spark-1.3",
        r#"{"providers": {"meta": {"baseUrl": "http://127.0.0.1:9/v1/responses"}}}"#,
        Keys::EXPORTED,
    );

    // Stopped through the conversation, and asked through the tool.
    run.conversation.web.stop();
    let (answered, asked) = run.search();

    assert!(answered.contains("nothing was sent"), "{answered}");
    assert_eq!(asked, 0, "{answered}");
}
