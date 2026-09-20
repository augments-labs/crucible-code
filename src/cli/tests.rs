//! What the command line and the files together decide.

use clap::CommandFactory;
use crucible_app::providers::{NOTHING_TO_ASK, offered};
use crucible_auth::StoredCredentials;

use super::*;
use crate::cli::sample::Sample;

fn choice(flag: &str) -> Choice {
    Choice::parse(flag).expect("a provider")
}

/// The built-in providers, as one generation the tests read against.
fn catalogue() -> Providers {
    providers()
        .expect("the built-in providers register")
        .snapshot()
}

/// Every record the registry holds, in the order it holds them.
fn every() -> Vec<Served> {
    offered(&catalogue()).collect()
}

/// The record `run` resolves before it asks for a model.
fn serving(named: &str) -> Served {
    served(&catalogue(), named).expect("a provider this build has")
}

/// The credential sources a test resolves a provider against.
fn authenticating<'a>(
    settings: &'a Settings,
    from: &'a dyn Fn(&str) -> Option<String>,
    stored: &'a StoredCredentials,
    subscriptions: &'a Subscriptions,
) -> startup::ProviderAuth<'a> {
    startup::ProviderAuth {
        settings,
        from,
        stored,
        subscriptions,
    }
}

#[test]
fn the_flag_names_the_model_over_anything_a_file_says() {
    let sample = Sample::new("model-flag");
    let settings = sample.settings(r#"{"providers": {"anthropic": {"model": "from-a-file"}}}"#);

    assert_eq!(
        wanted(
            &choice("claude-opus-5"),
            &settings,
            Some(serving("anthropic"))
        )
        .as_deref(),
        Some("claude-opus-5")
    );
}

#[test]
fn a_provider_with_no_model_after_it_takes_the_one_configured_for_it() {
    // The reason `--model openai/` parses at all. Without it every way of
    // choosing a provider names a model in the same breath, and the model in
    // the file could never be the one asked for.
    let sample = Sample::new("model-file");
    let settings = sample.settings(
        r#"{"providers": {"anthropic": {"model": "claude-opus-5"},
                          "openai": {"model": "gpt-5.6"}}}"#,
    );

    assert_eq!(
        wanted(&choice("openai/"), &settings, Some(serving("openai"))).as_deref(),
        Some("gpt-5.6")
    );
    assert_eq!(
        wanted(&Choice::default(), &settings, Some(serving("anthropic"))).as_deref(),
        Some("claude-opus-5")
    );
}

#[test]
fn a_model_named_nowhere_at_all_is_no_model() {
    // The rung that used to be a name written into this build. It sent whatever
    // model this binary was compiled with to whichever provider the key
    // belonged to, which is the pairing nobody asked for. There is no such rung
    // now, and the session says so rather than guessing.
    for one in every() {
        let asked = wanted(
            &choice(&format!("{}/", one.name)),
            &Settings::default(),
            Some(one),
        );

        assert_eq!(asked, None, "{}", one.name);
    }
}

#[test]
fn a_model_configured_as_nothing_at_all_is_no_model() {
    // A key written and left empty is a file that says nothing rather than one
    // that asks for a model called "". Sent as it stands it would reach a
    // vendor as a request for a model with no name.
    let sample = Sample::new("model-blank");

    for blank in ["", "   "] {
        let settings = sample.settings(&format!(
            r#"{{"providers": {{"anthropic": {{"model": "{blank}"}}}}}}"#
        ));

        assert_eq!(
            wanted(&Choice::default(), &settings, Some(serving("anthropic"))),
            None,
            "{blank:?}"
        );
    }
}

#[test]
fn the_flag_says_how_hard_to_think_over_anything_a_file_says() {
    let sample = Sample::new("effort-flag");
    let settings = sample.settings(r#"{"providers": {"anthropic": {"effort": "low"}}}"#);

    assert_eq!(
        thinking(Some(Effort::Max), &settings, Some(serving("anthropic"))),
        Some(Effort::Max)
    );
}

#[test]
fn a_run_that_says_nothing_takes_the_rung_configured_for_the_provider_it_is_going_to() {
    // Per provider rather than one answer for the machine, because which rungs
    // exist is the vendor's business: a file that chose `xhigh` for the one
    // serving it has said nothing about the one that would refuse it.
    let sample = Sample::new("effort-file");
    let settings = sample.settings(
        r#"{"providers": {"anthropic": {"effort": "xhigh"},
                          "openai": {"effort": "low"}}}"#,
    );

    assert_eq!(
        thinking(None, &settings, Some(serving("anthropic"))),
        Some(Effort::Xhigh)
    );
    assert_eq!(
        thinking(None, &settings, Some(serving("openai"))),
        Some(Effort::Low)
    );
    assert_eq!(thinking(None, &settings, Some(serving("moonshot"))), None);
}

#[test]
fn a_run_nobody_told_how_hard_to_think_asks_for_no_rung_at_all() {
    // Not the middle one, and not the rung the picker opens on. Every vendor
    // here chose a default per model, and one asked for on somebody's behalf
    // would reach the models that do not take the field at all — turning a
    // session nobody configured into a refusal from a vendor they did not
    // knowingly ask anything of.
    assert_eq!(
        thinking(None, &Settings::default(), Some(serving("anthropic"))),
        None
    );
    assert_eq!(thinking(None, &Settings::default(), None), None);
}

#[test]
fn a_rung_that_is_not_one_is_refused_with_the_rungs_that_are() {
    // The flag is parsed before there is anything on screen to look at, so the
    // sentence is the whole of what somebody who mistyped gets back.
    let refused =
        Cli::try_parse_from(["crucible", "--effort", "maximum"]).expect_err("a rung nobody serves");

    let said = refused.to_string();
    assert!(said.contains("no effort called maximum"), "{said}");
    assert!(said.contains("low, medium, high, xhigh, max"), "{said}");
}

#[test]
fn a_remembered_provider_without_any_credential_does_not_stop_startup() {
    let sample = Sample::new("named-without-key");
    let settings = sample.user(
        r#"{"provider":"openai","providers":{"openai":{"model":"gpt-5.6-sol","effort":"high"}}}"#,
    );

    let launch = launch(
        &Cli::try_parse_from(["crucible"]).unwrap(),
        &catalogue(),
        authenticating(
            &settings,
            &|_| None,
            &sample.store().read(),
            &Subscriptions::production(),
        ),
    )
    .expect("an unavailable remembered provider is an interactive setup state");

    assert!(launch.serving.is_none());
    assert!(launch.model.is_none());
    assert!(launch.effort.is_none());
    assert_eq!(launch.unasked, NOTHING_TO_ASK);
}

#[test]
fn a_model_configured_for_another_provider_is_not_configured_for_this_one() {
    let sample = Sample::new("model-elsewhere");
    let elsewhere = sample.settings(r#"{"providers": {"openai": {"model": "gpt-5.6"}}}"#);

    assert_eq!(
        wanted(&Choice::default(), &elsewhere, Some(serving("anthropic"))),
        None
    );
}

#[test]
fn the_help_text_names_every_provider_this_build_serves_and_its_variable() {
    // The registry and `long_about` can disagree, and a user meets
    // whichever of them is wrong: a provider the parser accepts and the help
    // text never mentions is one nobody finds, and a variable named in the help
    // text with no entry behind it is one they export and watch do nothing.
    let help = Cli::command().render_long_help().to_string();

    for one in every() {
        assert!(
            help.contains(one.key),
            "the help text never says where {}'s key is read from",
            one.name
        );
    }
}

#[test]
fn resume_and_continue_cannot_be_asked_for_together() {
    // Each names a different session to pick up. Refused at the parser, so
    // whichever the user meant, nothing is opened on the other's behalf.
    let refused = Cli::try_parse_from(["crucible", "--resume", "some-id", "--continue"])
        .expect_err("two ways back at once");

    let said = refused.to_string();
    assert!(said.contains("cannot be used with"), "{said}");
}

#[test]
fn windows_sandbox_maintenance_is_an_exclusive_early_action() {
    let setup = Cli::try_parse_from(["crucible", "sandbox", "setup", "--owner", r"MACHINE\person"])
        .expect("targeted setup");
    assert!(matches!(
        setup.command,
        Some(Command::Sandbox {
            action: SandboxMaintenance::Setup { owner }
        }) if owner.as_deref() == Some(std::ffi::OsStr::new(r"MACHINE\person"))
    ));

    for invalid in [
        vec!["crucible", "sandbox", "--owner", "person"],
        vec!["crucible", "sandbox", "setup", "uninstall"],
        vec!["crucible", "--model", "some-model", "sandbox", "setup"],
    ] {
        assert!(Cli::try_parse_from(invalid).is_err());
    }
}

#[test]
fn resume_round_trip() {
    use crucible_session::Session;

    let sample = Sample::new("resume-round-trip");
    let workspace = sample.workspace();
    let session =
        Session::start(&sample.logs(), &workspace, None).expect("a new session to record");
    let id = session.id().expect("a recorded session has a name").clone();
    let path = session.path().to_owned();
    session.append(&crucible_core::Message::said("keep this turn"));
    drop(session);

    // The parting message names the command that comes back to this session.
    let mut renderer = crucible_tui::Renderer::new(crucible_tui::Recording::new(80, 24));
    draw::parting(
        &mut renderer,
        &converse::Parting::Kept(path),
        style::Style::plain(),
    )
    .expect("a parting to draw");
    let written = renderer.terminal().written().to_string();
    assert!(
        written.contains(&format!("crucible --resume {}", id.as_str())),
        "{written}"
    );

    // The recorded id reopens the session it names, with its transcript.
    let (reopened, transcript) =
        startup::reopening(&sample.logs(), &workspace, &id).expect("the session named");
    assert_eq!(transcript.len(), 1);
    drop(reopened);

    // An id nothing here answers to is told so in one sentence.
    let stranger = crucible_core::SessionId::new();
    let refused = startup::reopening(&sample.logs(), &workspace, &stranger)
        .expect_err("a session nobody recorded");
    assert_eq!(
        refused.to_string(),
        format!("no session {} in this workspace", stranger.as_str())
    );
}
