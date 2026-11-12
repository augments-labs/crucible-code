//! `crucible agents try`'s move, as a command line asks the agents door it:
//! with no conversation, but a launcher and its steps lent to start the
//! user's own program, the path the door finds for that one agent, and the
//! door's question answered yes. What it writes is what `/login` writes for
//! the same program.

use crucible_app::Conversation;
use crucible_app::client::agents::{Outcome as Door, OwnProgram};
use crucible_client_api::{Command, Name, Text};

use crate::external::choosing::{
    agy_of_your_own, found_at_door, goose_of_the_registry, goose_of_your_own,
    goose_past_the_listing, instance_written, listed_with_the_registry, tried_at_door,
    tried_through_login,
};
#[cfg(unix)]
use crate::external::choosing::{goose_through_a_link, tools, user};
use crate::{Failed, Tree};

/// The instance `crucible agents try agent` writes in `tree`, set up by
/// `of_your_own`, as [`instance_written`] gives it, once the door found the
/// user's own program at the path the setup says its links lead to.
fn tried_from_the_command_line(
    tree: &Tree,
    of_your_own: fn(&Tree) -> Result<(Conversation, std::path::PathBuf), Failed>,
    agent: &str,
) -> Result<String, Failed> {
    let (conversation, program) = of_your_own(tree)?;
    let found = own_found(tree, &conversation, agent)?;
    assert_eq!(found.as_ref(), Some(&program), "the door's own {agent}");
    let tried = tried_at_door(
        tree,
        &conversation,
        Command::TryOwnAgent {
            agent: Name::new(agent)?,
            program: Text::cut(&program.to_string_lossy()),
        },
    )?;
    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    instance_written(tree, &instance)
}

/// The user's own program of `agent` the door finds in `tree` for
/// `conversation`, as `crucible agents try` asks it.
fn own_found(
    tree: &Tree,
    conversation: &Conversation,
    agent: &str,
) -> Result<Option<std::path::PathBuf>, Failed> {
    match found_at_door(tree, conversation, agent)? {
        Ok(OwnProgram::Found(program)) => Ok(Some(program)),
        Ok(OwnProgram::Unfound | OwnProgram::Unlisted) => Ok(None),
        Ok(OwnProgram::Beside { instance, .. }) => {
            Err(format!("the door refused a try beside the configured {instance}").into())
        }
        Err(why) => Err(why.to_string().into()),
    }
}

/// `crucible agents try goose` finds the user's own goose in the listing and
/// writes the instance `/login`'s "Try your own" writes for it.
#[test]
fn a_command_line_try_of_a_catalog_key_writes_what_login_writes() -> Result<(), Failed> {
    let chosen = tried_through_login(
        &Tree::new("agents-try-goose-login")?,
        goose_of_your_own,
        "goose",
    )?;

    let tried = tried_from_the_command_line(
        &Tree::new("agents-try-goose-command")?,
        goose_of_your_own,
        "goose",
    )?;

    assert_eq!(tried, chosen);
    Ok(())
}

/// `crucible agents try antigravity` finds the user's own `agy` in the
/// listing and writes the Antigravity instance `/login`'s "Try your own"
/// writes for it.
#[test]
fn a_command_line_try_of_antigravity_writes_what_login_writes() -> Result<(), Failed> {
    let chosen = tried_through_login(
        &Tree::new("agents-try-agy-login")?,
        agy_of_your_own,
        "Antigravity",
    )?;

    let tried = tried_from_the_command_line(
        &Tree::new("agents-try-agy-command")?,
        agy_of_your_own,
        "antigravity",
    )?;

    assert_eq!(tried, chosen);
    Ok(())
}

/// A registry agent whose guessed name is on `PATH` is marked as the user's
/// own in `crucible agents search` as `/login` offers it, and
/// `crucible agents try goose` writes the instance `/login`'s "Try your own"
/// writes for it.
#[test]
fn a_command_line_try_of_a_registry_key_writes_what_login_writes() -> Result<(), Failed> {
    let chosen = tried_through_login(
        &Tree::new("agents-try-registry-login")?,
        goose_of_the_registry,
        "goose",
    )?;

    let tree = Tree::new("agents-try-registry-search")?;
    let (conversation, program) = goose_of_the_registry(&tree)?;
    let listed = listed_with_the_registry(&tree, &conversation)?;
    let Ok(Door::Catalog { agents, .. }) = listed else {
        return Err(format!("the listing answered {listed:?}").into());
    };
    let goose = agents
        .iter()
        .find(|listed| listed.agent.as_str() == "goose")
        .ok_or("the registry's goose was not listed")?;
    assert!(goose.registry, "goose was listed as no registry release");
    assert_eq!(
        goose.own.as_ref(),
        Some(&program),
        "the listing's own goose"
    );

    let tried = tried_from_the_command_line(
        &Tree::new("agents-try-registry-command")?,
        goose_of_the_registry,
        "goose",
    )?;

    assert_eq!(tried, chosen);
    Ok(())
}

/// `crucible agents try goose` tries the user's own goose as the door would
/// where the listing shows as many agents as it can before it: the instance
/// it writes is the one written where goose is listed.
#[test]
fn a_command_line_try_of_a_key_past_the_listing_writes_what_a_listed_one_does() -> Result<(), Failed>
{
    let tree = Tree::new("agents-try-past-listing")?;
    let (conversation, _) = goose_past_the_listing(&tree)?;
    let listed = listed_with_the_registry(&tree, &conversation)?;
    let Ok(Door::Catalog { agents, more, .. }) = listed else {
        return Err(format!("the listing answered {listed:?}").into());
    };
    assert!(more, "the listing showed every agent");
    assert!(
        agents.iter().all(|listed| listed.agent.as_str() != "goose"),
        "goose was within the listing"
    );

    let past = tried_from_the_command_line(
        &Tree::new("agents-try-past-listing-command")?,
        goose_past_the_listing,
        "goose",
    )?;
    let listed = tried_from_the_command_line(
        &Tree::new("agents-try-listed-command")?,
        goose_of_the_registry,
        "goose",
    )?;

    assert_eq!(past, listed);
    Ok(())
}

/// The door finds no program of the user's own for an agent it offers whose
/// guessed name is on no `PATH`, and lists no agent for a name neither the
/// catalog nor the registry offers, or one a built-in driver is spelled with.
#[test]
fn the_door_tells_an_agent_with_no_program_from_one_it_does_not_list() -> Result<(), Failed> {
    let tree = Tree::new("agents-try-unfound")?;
    let (conversation, _) = goose_past_the_listing(&tree)?;

    let found = |agent| found_at_door(&tree, &conversation, agent);

    assert_eq!(found("listed-00")?.ok(), Some(OwnProgram::Unfound));
    assert_eq!(found("nonesuch")?.ok(), Some(OwnProgram::Unlisted));
    assert_eq!(found("claude")?.ok(), Some(OwnProgram::Unlisted));
    Ok(())
}

/// `crucible agents try goose` finds the user's own goose through a link on
/// `PATH` into a directory no `PATH` names, tries it there, admits it, and
/// writes an instance that runs it by the path the link leads to.
#[cfg(unix)]
#[test]
fn a_program_found_through_a_link_into_another_directory_is_tried_and_admitted()
-> Result<(), Failed> {
    let tree = Tree::new("agents-try-through-a-link")?;

    let tried = tried_from_the_command_line(&tree, goose_through_a_link, "goose")?;

    let program = std::fs::canonicalize(tools(&tree).join("goose"))?;
    assert_ne!(program.parent(), Some(tools(&tree).as_path()));
    let written = serde_json::json!({
        "driver": "catalog",
        "agent": "goose",
        "executable": program.to_string_lossy(),
    });
    assert_eq!(tried, written.to_string());
    Ok(())
}

/// An instance that names the user's own goose by its command, found through
/// a link on `PATH` into a directory no `PATH` names, is started under the
/// sandbox, once its program was probed at the path the link leads to, and
/// answers a turn.
#[cfg(unix)]
#[test]
fn an_instance_naming_a_program_through_a_link_is_launched_under_the_sandbox() -> Result<(), Failed>
{
    let tree = Tree::new("agents-launch-through-a-link")?;
    let (_, program) = goose_through_a_link(&tree)?;
    own_goose_probed(&tree, &own_goose("goose", &program.to_string_lossy())?)?;

    let turned = own_goose_turned(&tree, &own_goose("goose", "goose")?)?;

    assert_eq!(turned.as_deref(), Ok("Answered"));
    Ok(())
}

/// The catalog instance `name` of goose, run as `executable`.
#[cfg(unix)]
fn own_goose(name: &str, executable: &str) -> Result<crucible_config::ExternalAgent, Failed> {
    let key = crucible_types::AgentKey::new("goose")?;
    Ok(crucible_config::ExternalAgent::unwritten(
        name,
        crucible_config::Driver::Catalog,
        executable,
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?)
}

/// What `PATH` is in `tree` to a test of the user's own programs.
#[cfg(unix)]
fn path_in(tree: &Tree) -> impl Fn(&str) -> Option<std::ffi::OsString> + use<> {
    let path = tools(tree).into_os_string();
    move |name: &str| (name == "PATH").then(|| path.clone())
}

/// Probes `configured` in `tree` as `crucible agents try` does, keeping its
/// record.
#[cfg(unix)]
fn own_goose_probed(
    tree: &Tree,
    configured: &crucible_config::ExternalAgent,
) -> Result<(), Failed> {
    use crucible_app::own_program::{self, Places};

    let home = tree.home()?;
    own_program::tried(
        &Places {
            home: &user(tree),
            crucible: home.path(),
            sessions: &tree.sessions(),
        },
        configured,
        &[configured.name()],
        path_in(tree),
        &crucible_app::probing::local,
        std::time::Duration::from_secs(20),
    )
    .map_err(|why| why.to_string())?;
    Ok(())
}

/// How a turn taken by `configured` in `tree` ended, started under the
/// sandbox with its programs looked for on `tree`'s `PATH`; else why it did
/// not.
#[cfg(unix)]
fn own_goose_turned(
    tree: &Tree,
    configured: &crucible_config::ExternalAgent,
) -> Result<Result<String, String>, Failed> {
    use crucible_app::backends::{REGISTRY, Sandboxing};
    use crucible_app::catalog_fetch;
    use crucible_app::launching::{Agent, Launcher};
    use crucible_execution::{SessionRef, TurnRequest};
    use crucible_external_harnesses::Control;
    use crucible_external_harnesses::catalog::Trust;
    use crucible_sandbox::SandboxPolicy;
    use crucible_types::TurnId;

    let runtime = super::runtime()?;
    let home = tree.home()?;
    let state = catalog_fetch::state_in(home.path());
    let agent = Agent::believing(configured, &REGISTRY, &state, path_in(tree))
        .map_err(|why| why.to_string())?;
    let consent = super::shipped_hold::shipped_given(agent.row().route())?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(true);
    let sessions = tree.sessions();
    let launcher = Launcher::new(
        crucible_app::probing::local(runtime.handle().clone()),
        runtime.handle().clone(),
        &policy,
        &consent,
        &state,
        &sessions,
    )
    .homed(user(tree));
    let control = Control::new().ok_or("a control")?;
    let trust = Trust::load(&state, std::time::SystemTime::now());
    let mut dialect = agent.dialect(
        crucible_tools::Mode::Ask,
        &trust,
        &home.path().join("programs"),
        Sandboxing {
            on: true,
            required: false,
        },
    )?;
    Ok(runtime.block_on(async {
        let mut host = launcher
            .launch(&agent, &control, dialect.as_mut(), None)
            .await
            .map_err(|why| why.to_string())?;
        let turned = host
            .turn(
                dialect.as_mut(),
                &SessionRef::new("one").ok_or("a session's name")?,
                &TurnRequest::new(TurnId::FIRST, "go"),
                &Unwatched,
            )
            .await;
        drop(host.stop(crucible_external_harnesses::STOP_GRACE).await);
        turned
            .map(|outcome| format!("{outcome:?}"))
            .map_err(|failure| failure.reason().as_str().to_owned())
    }))
}

/// Where a turn's events go when none of them is looked at.
#[cfg(unix)]
struct Unwatched;

#[cfg(unix)]
impl crucible_execution::Sink for Unwatched {
    fn post(&self, _: crucible_execution::Event) {}
}
