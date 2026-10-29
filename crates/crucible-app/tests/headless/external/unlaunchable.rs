//! A prompt whose turn cannot start the agent, on a route with no yes.
//!
//! Each case holds a conversation moved onto Claude Code's route with no run
//! launched, so its next prompt would start the agent, and with the route
//! unanswered, so that prompt would put the route to the client first. A
//! prompt that carries an attachment, which no agent is handed, and one to an
//! agent whose program is nowhere, are each refused for that reason before
//! the route is put: nobody is asked to agree to a launch that cannot
//! happen, no yes is written into the user file, and nothing is started. So is
//! one to a catalog agent whose instance names a profile directory its entry
//! gives no variable for, and the account door's sign-out of it.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crucible_app::access::Route;
use crucible_app::backends::Registry;
use crucible_app::client;
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_app::outside::Installed;
use crucible_client_api::{
    Capabilities as Asking, Command, Correlation, Outcome, Pending, Prompt, Request, TurnOutcome,
};
use crucible_config::{Driver, Serves};
use crucible_external_harnesses::catalog::Trust;
use crucible_external_harnesses::fake_peer::{self, scripts::claude};
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_sandbox::{SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_session::Session;
use crucible_types::{Attachment, Modality, Transcript};

use super::{INSTALLED, Launched, Launching, Peer, Picking, Watching, binding, row, runtime};
use crate::{Failed, Script, Tree};

/// What an unlaunched conversation was made of, beside the conversation.
struct Unlaunched {
    conversation: crucible_app::Conversation,
    /// The agent its runs would be started as.
    agent: Agent,
    /// The yeses its sends are held to.
    consent: Consent,
    /// The user file a yes is written into.
    file: PathBuf,
    /// What each launch was asked for.
    launched: Arc<Mutex<Vec<Launched>>>,
}

/// A conversation in `tree` on Claude Code's route, no run launched and the
/// route unanswered, its runs started as the program at `program`: the fake
/// peer installed there where `program` is `None`, else a path that is not
/// there.
///
/// This is the headless [`Launching`] whose executable is missing, where
/// `program` names one.
fn unlaunched(tree: &Tree, program: Option<PathBuf>) -> Result<Unlaunched, Failed> {
    let executable = match program {
        Some(missing) => missing,
        None => fake_peer::install(&tree.0.join("peer"), "claude", &Peer::of([claude::clean()]))?,
    };
    let row = row()?;
    let route = Route::External {
        serves: Serves::Provider("anthropic"),
        instance: "work".into(),
        driver: Driver::Claude,
        executable: executable.to_string_lossy().into(),
        profile: None,
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let registry = Registry::new(std::slice::from_ref(row));
    let agent = Agent::routed(&route, &registry, |_| None).ok_or("Claude Code's route")?;
    unlaunched_as(tree, agent)
}

/// A conversation in `tree` on Claude Code's route, no run launched and the
/// route unanswered, its runs started as `agent`.
fn unlaunched_as(tree: &Tree, agent: Agent) -> Result<Unlaunched, Failed> {
    let row = row()?;
    let consent = Consent::new(Routes::production());
    let file = tree.0.join("config.json");
    std::fs::write(&file, INSTALLED)?;
    consent.keeps_in(file.clone());
    let runtime = runtime()?;
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.clone()));
    let launching = Launching {
        sandbox,
        runtime: runtime.clone(),
        policy: SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false),
        consent: consent.clone(),
        state: tree.0.join("state"),
        agent: agent.clone(),
        row,
        launched: Arc::default(),
    };
    let launched = Arc::clone(&launching.launched);
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);
    let mut conversation =
        crate::conversation(tree, Script::new(Vec::new()), false)?.consenting(consent.clone());
    let installed = Installed {
        backend: None,
        relaunch: Box::new(launching),
        route: binding()?,
    };
    drop(runtime.block_on(conversation.external(session, Transcript::new(), installed))?);
    Ok(Unlaunched {
        conversation,
        agent,
        consent,
        file,
        launched,
    })
}

/// Sends `prompt` with `attached` through the client's door, a client that
/// accepts whatever it is put answering; what it was put and how the turn
/// ended.
fn prompted(
    unlaunched: &mut Unlaunched,
    attached: Box<[Attachment]>,
) -> Result<(Vec<Pending>, Outcome), Failed> {
    let request = Request::new(
        Asking::every(),
        Correlation::new(1),
        Command::Prompt(Prompt::new("hello")?),
    );
    let mut picking = Picking::of([Some("Accept")]);
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let quiet = Watching {
        events: mpsc::channel().0,
        stopping: None,
    };
    let conversation = &mut unlaunched.conversation;
    let ended = {
        let run = conversation.starting(&quiet, &cancel, &steer, &aside);
        runtime()?.block_on(client::turn(
            conversation,
            &request,
            attached,
            &mut picking,
            &run,
        ))
    };
    Ok((picking.put, ended.response(&request).outcome))
}

/// Every failure of a refused turn's case: the route put, a yes written, a
/// launch tried, or an outcome other than a failure saying `reason`.
fn refused_first(
    unlaunched: &Unlaunched,
    (put, outcome): (&[Pending], &Outcome),
    reason: &str,
) -> Result<Vec<String>, Failed> {
    let mut failures = Vec::new();
    if put
        .iter()
        .any(|pending| matches!(pending, Pending::Warning { .. }))
    {
        failures.push(format!("the route was put first: {put:?}"));
    }
    let written = std::fs::read_to_string(&unlaunched.file)?;
    if written != INSTALLED {
        failures.push(format!("a yes was written: {written}"));
    }
    if unlaunched.consent.asks("agent:claude").is_none() {
        failures.push("the route was taken as answered".to_owned());
    }
    let launched = unlaunched
        .launched
        .lock()
        .map(|launched| launched.clone())
        .unwrap_or_default();
    if !launched.is_empty() {
        failures.push(format!("a launch was tried: {launched:?}"));
    }
    match outcome {
        Outcome::Turn(TurnOutcome::Failed(problem))
            if problem.message.as_str().contains(reason) => {}
        other => failures.push(format!(
            "the turn did not fail saying {reason:?}: {other:?}"
        )),
    }
    Ok(failures)
}

/// A prompt carrying an attachment, on an unanswered route whose turn would
/// start the agent, is refused for the attachment before the route is put.
/// Before, the route was put, the yes written, and only then was the turn
/// refused.
#[test]
fn an_attachment_on_an_unanswered_agent_route_is_refused_before_the_route_is_asked()
-> Result<(), Failed> {
    let tree = Tree::new("external-unanswered-attachment")?;
    let mut unlaunched = unlaunched(&tree, None)?;
    let attached: Box<[Attachment]> = Box::new([Attachment {
        path: tree
            .0
            .join("work")
            .join("shot.png")
            .to_string_lossy()
            .into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: [7; 32],
    }]);

    let (put, outcome) = prompted(&mut unlaunched, attached)?;

    let failures = refused_first(
        &unlaunched,
        (&put, &outcome),
        "an attachment to an installed agent's turn",
    )?;
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}

/// A prompt to an agent whose program is not there, on an unanswered route,
/// is refused with what the launch would say before the route is put.
/// Before, the route was put, the yes written, and only then did the launch
/// fail.
#[test]
fn a_missing_program_on_an_unanswered_agent_route_is_refused_before_the_route_is_asked()
-> Result<(), Failed> {
    let tree = Tree::new("external-unanswered-missing")?;
    let mut unlaunched = unlaunched(&tree, Some(tree.0.join("no-such-claude-program")))?;
    let trust = Trust::load(&tree.0.join("state"), SystemTime::now());
    let reason = unlaunched
        .agent
        .program(&unlaunched.consent, &trust)
        .err()
        .ok_or("the program was found")?
        .to_string();

    let (put, outcome) = prompted(&mut unlaunched, Box::default())?;

    let failures = refused_first(&unlaunched, (&put, &outcome), &reason)?;
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}

mod profiles {
    //! A catalog agent whose instance names a profile directory its entry gives
    //! no variable for.
    //!
    //! The entry says which variable the agent reads its profile from, or none. An
    //! instance that names `profileDir` for an agent with none would be started
    //! with a directory the agent never reads, and so signed in somewhere the
    //! user did not choose. It is refused wherever a start is asked, saying which
    //! instance to change, before the route is put, before anything is created and
    //! before any process: a turn, and the account door's sign-out, which asks
    //! the launcher whether a start would be refused.

    use std::path::Path;

    use crucible_app::backends::{CatalogRow, Registry};
    use crucible_app::launching::Agent;
    use crucible_config::{Driver, ExternalAgent};
    use crucible_external_harnesses::fake_peer::{self, Script as Peer, scripts::claude};
    use crucible_types::AgentKey;
    use serde_json::{Value, json};

    use super::{prompted, refused_first, unlaunched_as};
    use crate::external::catalog_hold::goose_row_with;
    use crate::{Failed, Tree};

    /// What the refusal of an instance `work` of goose says.
    const REFUSED: &str = "externalAgents.work sets profileDir, but goose offers no separate \
                           profile; remove profileDir from externalAgents.work.";

    /// What the vendor says of what it is sent, so the route of the agent can be
    /// held.
    fn statement() -> Value {
        json!({
            "text": "The vendor may use what you send to improve goose.",
            "source": "https://vendor.example/terms/data",
            "date": "24 Oct 2026",
            "origins": [],
        })
    }

    /// The row a catalog gives goose, its entry carrying every field of the
    /// object `more`.
    fn row(more: &Value) -> Result<CatalogRow, Failed> {
        goose_row_with(Some(statement()), more)
    }

    /// The instance `work` of goose, run as `executable` with `profile` as its
    /// profile directory where one is named, from a row whose entry carries
    /// every field of the object `more`.
    fn instance(executable: &Path, profile: Option<&Path>, more: &Value) -> Result<Agent, Failed> {
        let registry: &'static Registry = Box::leak(Box::new(
            Registry::new(&[]).cataloging(Box::leak(Box::new([row(more)?]))),
        ));
        let key = AgentKey::new("goose")?;
        let profile = profile.map(|dir| dir.to_string_lossy().into_owned());
        let configured = ExternalAgent::unwritten(
            "work",
            Driver::Catalog,
            &executable.to_string_lossy(),
            profile.as_deref(),
            Some(&key),
        )
        .ok_or("a catalog instance naming its agent")?;
        Ok(Agent::configured(&configured, registry, |_| None).ok_or("its catalog row")?)
    }

    /// A prompt to the instance is refused with what the launch would say before
    /// the route is put: no yes is written, nothing is created and nothing is
    /// started.
    #[test]
    fn a_turn_on_an_instance_naming_a_profile_directory_its_entry_has_no_variable_for_starts_nothing()
    -> Result<(), Failed> {
        let tree = Tree::new("external-profile-unusable-turn")?;
        let peer = tree.0.join("peer");
        let program = fake_peer::install(&peer, "goose", &Peer::of([claude::clean()]))?;
        let profile = tree.0.join("profile");
        let agent = instance(&program, Some(&profile), &json!({}))?;
        let mut unlaunched = unlaunched_as(&tree, agent)?;

        let (put, outcome) = prompted(&mut unlaunched, Box::default())?;

        let failures = refused_first(&unlaunched, (&put, &outcome), REFUSED)?;
        assert!(failures.is_empty(), "{failures:#?}");
        assert_eq!(fake_peer::played(&peer), 0, "the agent was started");
        assert!(!profile.exists(), "its profile directory was made");
        Ok(())
    }

    /// The account door asks the launcher whether a start would be refused
    /// before it asks anything of the agent, so an instance naming a profile
    /// directory its entry has no variable for is told so there.
    #[cfg(feature = "proof")]
    mod account {
        use std::path::Path;
        use std::sync::Arc;

        use crucible_app::backends::Registry;
        use crucible_app::client;
        use crucible_app::content_use::{Consent, Routes};
        use crucible_app::launching::Steps;
        use crucible_client_api::{Command, LogoutOutcome, Outcome, Pending};
        use crucible_external_harnesses::fake_peer;
        use crucible_sandbox::SandboxEnablement;
        use serde_json::{Value, json};

        use super::{REFUSED, row};
        use crate::external::{Asked, Picking, accounting_on, connected, launching};
        use crate::{Failed, Script, Tree};

        /// How many times setting a conversation up on a connected agent starts
        /// its program: once for the version and once for the sign-in.
        const SETUP_STARTS: usize = 2;

        /// The user file naming one instance `work` of the catalog agent keyed
        /// `goose`, its profile directory at `profile` where one is given.
        fn user_file(profile: Option<&Path>) -> String {
            let mut work = json!({"driver": "catalog", "agent": "goose", "executable": "claude"});
            if let (Some(profile), Some(object)) = (profile, work.as_object_mut()) {
                object.insert("profileDir".into(), json!(profile.to_string_lossy()));
            }
            json!({"externalAgents": {"work": work}, "updates": {"check": "auto"}}).to_string()
        }

        /// A sign-out of `work`, whose agent's entry carries every field of the
        /// object `more`, answered by `picking`: what the user file then held,
        /// what the client was told, and how many times the agent's program was
        /// started by the sign-out itself.
        fn signed_out(
            name: &str,
            profile: &Path,
            more: &Value,
            picking: &mut Picking,
        ) -> Result<(Outcome, (String, String), usize), Failed> {
            let tree = Tree::new(name)?;
            let (mut launching, peer) = launching(&tree, &connected([]))?;
            let row = row(more)?;
            launching.consent =
                Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
            let registry: &'static Registry = Box::leak(Box::new(
                Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
            ));
            let steps = Steps::new(
                launching.launcher(),
                Arc::new(SandboxEnablement::new(false, false)),
                registry,
                Some(peer.into_os_string()),
            );
            let mut conversation = crucible_app::proof::agents(
                crate::conversation(&tree, Script::new(Vec::new()), false)?,
                registry,
            )
            .consenting(launching.consent.clone())
            .stepping(steps);
            // Setting the conversation up starts the program for its connect, which
            // is not what is asked about here.
            let peer_dir = tree.0.join("peer");
            for _ in 0..500 {
                if fake_peer::played(&peer_dir) >= SETUP_STARTS {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let before = fake_peer::played(&peer_dir);
            let written = user_file(Some(&profile.join(name)));
            let asked = Asked {
                installed: (&written, "work"),
                command: &|instance| Command::SignOut { instance },
                asking: crucible_client_api::Capabilities::every(),
                after: None,
            };
            let (outcome, file) = accounting_on(&tree, &mut conversation, &asked, picking)?;
            let started = fake_peer::played(&peer_dir) - before;
            Ok((outcome, (written, file), started))
        }

        /// Every sentence a client was put as an account question.
        fn told(picking: &Picking) -> Vec<&str> {
            picking
                .put
                .iter()
                .flat_map(|pending| match pending {
                    Pending::Account { asks, facts, .. } => std::iter::once(asks)
                        .chain(facts)
                        .map(crucible_client_api::Text::as_str)
                        .collect(),
                    _ => Vec::new(),
                })
                .collect()
        }

        /// A sign-out of an instance naming a profile directory its entry has no
        /// variable for says the instance must drop it, and changes and starts
        /// nothing; the same instance on an entry that gives the variable is not
        /// refused for it.
        #[test]
        fn a_sign_out_of_an_instance_naming_a_profile_directory_its_entry_has_no_variable_for_says_so()
        -> Result<(), Failed> {
            let profiles = Tree::new("external-profile-unusable-sign-out-dirs")?;
            let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
            let (outcome, (written, file), started) = signed_out(
                "external-profile-unusable-sign-out",
                &profiles.0,
                &json!({}),
                &mut picking,
            )?;

            assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided));
            assert_eq!(file, written, "the user file was written");
            assert_eq!(started, 0, "the agent was started");
            let said = told(&picking);
            assert!(
                said.iter().any(|fact| fact.contains(REFUSED)),
                "the refusal was not said: {said:?}"
            );
            assert!(
                !picking
                    .put
                    .iter()
                    .any(|pending| matches!(pending, Pending::Warning { .. })),
                "the route was put: {:?}",
                picking.put
            );

            let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
            signed_out(
                "external-profile-usable-sign-out",
                &profiles.0,
                &json!({"profile_env": "GOOSE_PATH_ROOT"}),
                &mut picking,
            )?;
            let said = told(&picking);
            assert!(
                !said
                    .iter()
                    .any(|fact| fact.contains("offers no separate profile")),
                "the profile was refused where the entry names a variable: {said:?}"
            );
            Ok(())
        }
    }
}
