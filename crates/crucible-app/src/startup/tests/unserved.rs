//! What a session with nobody chosen is missing, read through the provider
//! set-up a run is handed, as the terminal reads it.

use super::*;
use crate::providers::{NO_PROVIDER_CHOSEN, available, re_serving};
use crate::switching::{LoggedOut, Switching};

/// A provider with a credential and a setting that keeps it from being built
/// is still a provider somebody could choose: the terminal names a provider
/// as what is missing, and a client is told the same.
#[test]
fn a_credential_whose_provider_cannot_be_built_still_leaves_a_provider_to_choose() {
    let sample = Sample::new("unserved-unbuildable");
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let settings =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "http://gateway.example/v1"}}}"#);
    let services = Services::new();
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let logins = sample.store();
    logins
        .keep("anthropic", "a-key-no-vendor-issued")
        .expect("a writable home");
    let from = |name: &str| (name == "OPENAI_API_KEY").then(|| "an-exported-key".to_owned());

    let mut conversation = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NO_PROVIDER_CHOSEN,
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
        from: &from,
        stored: &logins.read(),
        subscriptions: &subscriptions,
    })
    .expect("a session with nothing chosen still starts");
    assert_eq!(conversation.missing(), Some(Missing::Provider));

    let set_up = re_serving(
        settings.clone(),
        subscriptions.clone(),
        Box::new(from),
        services.http().clone(),
        services.consent().clone(),
    );
    let sourcing: providers::Sourcing = Box::new(|_, _, _| Reaching::nothing());
    let (providers, choosing) = (catalogue(), sample.user_file());
    let with = Switching {
        providers: &providers,
        settings: &settings,
        serving: &set_up,
        sourcing: &sourcing,
        logins: &logins,
        choosing: &choosing,
    };
    let left = services
        .runtime()
        .handle()
        .expect("a runtime")
        .block_on(conversation.log_out(serving("anthropic"), &with));
    assert!(matches!(left, LoggedOut::Kept), "{left:?}");

    // What the terminal says, read off the store as it is now.
    let stored = logins.read();
    let auth = ProviderAuth {
        settings: &settings,
        from: &from,
        stored: &stored,
        subscriptions: &subscriptions,
    };
    assert!(available(&providers, auth).next().is_some());
    assert_eq!(conversation.missing(), Some(Missing::Provider));
}
