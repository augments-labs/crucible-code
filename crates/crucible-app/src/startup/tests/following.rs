//! A session's web sources name the model it is asking now, from the provider
//! it is asking now, and are held as that model's route is.

use std::sync::Mutex;

use crucible_tools::SearchResponse;

use super::*;
use crate::providers::{Sourcing, re_serving, re_sourcing};
use crate::sample::transcribing;
use crate::switching::{Switched, Switching};

/// The key every lookup in these runs answers with.
const KEY: &str = "fabricated-web-key";

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

impl Run {
    fn new(tree: &str, provider: &str, model: &'static str, document: &str) -> Self {
        let sample = Sample::new(tree);
        let (logs, workspace) = (sample.logs(), sample.workspace());
        let settings = sample.user(document);
        let services = Services::new();
        let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());

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
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &|_| Some(KEY.to_owned()),
            stored: &StoredCredentials::default(),
            subscriptions: &subscriptions,
        })
        .expect("a start with a key");

        Self {
            serving: re_serving(
                settings.clone(),
                subscriptions.clone(),
                Box::new(|_| Some(KEY.to_owned())),
                services.http().clone(),
                services.consent().clone(),
            ),
            sourcing: re_sourcing(
                settings.clone(),
                subscriptions,
                Box::new(|_| Some(KEY.to_owned())),
                services.http().clone(),
                services.consent().clone(),
            ),
            logins: sample.store(),
            choosing: sample.user_file(),
            providers: catalogue(),
            settings,
            services,
            conversation,
            _sample: sample,
        }
    }

    /// Asks `model` from `provider` from the next turn on, as `/model` does.
    fn switch(&mut self, provider: &str, model: &str) {
        let with = Switching {
            providers: &self.providers,
            settings: &self.settings,
            serving: &self.serving,
            sourcing: &self.sourcing,
            logins: &self.logins,
            choosing: &self.choosing,
        };
        let runtime = self.services.runtime().handle().unwrap();
        let switched =
            runtime.block_on(
                self.conversation
                    .ask_for(serving(provider), model, None, &with),
            );
        assert!(matches!(switched, Switched::Taken { .. }), "{switched:?}");
    }

    /// One search through what the session's `web_search` tool answers with.
    fn search(&self) -> Result<SearchResponse, String> {
        let web = self.conversation.web.clone();
        let runtime = self.services.runtime().handle().unwrap();
        runtime
            .block_on(Search::search(
                &web,
                "what the user is working on",
                &Cancel::new(),
            ))
            .map_err(|problem| problem.to_string())
    }
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
    );

    run.switch("meta", "muse-spark-1.3");
    let searched = run.search();

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
    );
    let model = "muse-spark-1.3-contributor";

    run.switch("meta", model);
    let held = run.search();

    let route = content_use::model_route("meta", model);
    assert!(heard(&requests).is_empty(), "{held:?}");
    assert!(
        held.as_ref().is_err_and(|said| said.contains(&route)),
        "{held:?}"
    );

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
/// left. A provider that serves no search answers that nothing was sent.
#[test]
fn a_search_after_a_switch_of_provider_goes_to_the_provider_switched_to() {
    let (meta, from_meta) = transcribing();
    let (xai, from_xai) = transcribing();
    let document = format!(
        r#"{{"providers": {{"meta": {{"baseUrl": "{meta}"}}, "xai": {{"baseUrl": "{xai}"}}}}}}"#
    );
    let mut run = Run::new("web-follows-provider", "meta", "muse-spark-1.3", &document);

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

    run.switch("deepseek", "deepseek-flash");
    let unserved = run.search();
    assert!(
        unserved
            .as_ref()
            .is_err_and(|said| said.contains("nothing was sent")),
        "{unserved:?}"
    );
    assert!(heard(&from_meta).is_empty());
    assert_eq!(heard(&from_xai).len(), heard_xai.len());
}
