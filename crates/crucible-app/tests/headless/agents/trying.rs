//! `crucible agents try`'s move, as a command line asks the agents door it:
//! with no conversation, but a launcher and its steps lent to start the
//! user's own program, the path the door finds for that one agent, and the
//! door's question answered yes. What it writes is what `/login` writes for
//! the same program.

use crucible_app::Conversation;
use crucible_app::client::agents::{Outcome as Door, OwnProgram};
use crucible_client_api::{Command, Name, Text};

#[cfg(unix)]
use crate::external::choosing::{
    Unconfining, counted_goose, goose_through_a_link, goose_turned, goose_turned_by_command,
    turning_goose_of_your_own,
};
use crate::external::choosing::{
    agy_of_your_own, counted_goose_of_your_own, found_at_door, goose_of_the_registry,
    goose_of_your_own, goose_past_the_listing, instance_written, listed_with_the_registry,
    this_sandbox, tools, tried_at_door, tried_through_login,
};
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
/// `PATH` into a directory no `PATH` names, tries it there, and writes an
/// instance that runs it by the path the link leads to.
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
/// sandbox at the path the link leads to, and answers a turn.
#[cfg(unix)]
#[test]
fn an_instance_naming_a_program_through_a_link_is_launched_under_the_sandbox() -> Result<(), Failed>
{
    let tree = Tree::new("agents-launch-through-a-link")?;
    goose_through_a_link(&tree)?;

    let turned = goose_turned_by_command(
        &tree,
        &super::runtime()?,
        super::shipped_hold::shipped_given,
    )?;

    assert_eq!(turned.as_deref(), Ok("Answered"));
    Ok(())
}

/// The instance a try of the user's own goose writes, with the directory its
/// programs lie in written `<tools>`: one that runs it by the path its links
/// lead to, spelled as this platform spells that program and escaped as the
/// written instance escapes it (`<tools>/goose` on Unix).
fn goose_written() -> Result<String, Failed> {
    let program =
        std::path::Path::new("<tools>").join(format!("goose{}", std::env::consts::EXE_SUFFIX));
    Ok(format!(
        r#"{{"agent":"goose","driver":"catalog","executable":{}}}"#,
        serde_json::to_string(&program.to_string_lossy())?
    ))
}

/// `crucible agents try goose`, with the user's own goose on the steps'
/// `PATH`, asks, starts nothing and writes an instance that runs the program
/// at the path its links lead to.
#[test]
fn trying_your_own_program_starts_nothing_and_writes_the_instance() -> Result<(), Failed> {
    let tree = Tree::new("agents-try-starts-nothing")?;

    let tried = tried_from_the_command_line(
        &tree,
        |tree| counted_goose_of_your_own(tree, (this_sandbox()?, true)),
        "goose",
    )?;

    assert_eq!(
        super::fake_peer::played(&tools(&tree)),
        0,
        "the program was started"
    );
    assert_eq!(tried, goose_written()?);
    Ok(())
}

/// With the sandbox off, the user's own goose is tried and launched where the
/// sandbox cannot close the network: nothing refuses it, and it is started
/// once, as a plain process, and answers a turn.
#[cfg(unix)]
#[test]
fn your_own_program_is_launched_with_the_sandbox_off_where_no_sandbox_can_close()
-> Result<(), Failed> {
    let tree = Tree::new("agents-own-unsandboxed")?;
    let tried = tried_from_the_command_line(
        &tree,
        |tree| counted_goose_of_your_own(tree, (Unconfining::here()?, false)),
        "goose",
    )?;
    assert_eq!(tried, goose_written()?);
    let program = std::fs::canonicalize(tools(&tree).join("goose"))?;

    let turned = goose_turned(
        &tree,
        &super::runtime()?,
        super::shipped_hold::shipped_given,
        (&program.to_string_lossy(), (Unconfining::here()?, false)),
    )?;

    assert_eq!(turned.as_deref(), Ok("Answered"));
    assert_eq!(super::fake_peer::played(&tools(&tree)), 1);
    Ok(())
}

/// With the sandbox on where it cannot close the network, an instance that
/// runs the user's own goose is refused as any start is there, and the
/// program is never started.
#[cfg(unix)]
#[test]
fn your_own_program_is_refused_with_the_sandbox_on_where_no_sandbox_can_close() -> Result<(), Failed>
{
    let tree = Tree::new("agents-own-unclosable")?;
    let (_, program) = counted_goose_of_your_own(&tree, (Unconfining::here()?, true))?;

    let turned = goose_turned(
        &tree,
        &super::runtime()?,
        super::shipped_hold::shipped_given,
        (&program.to_string_lossy(), (Unconfining::here()?, true)),
    )?;

    let Err(refused) = &turned else {
        return Err(format!("the program answered: {turned:?}").into());
    };
    assert!(
        refused.contains("a sandbox is required") && refused.contains("no confinement at all"),
        "the refusal does not name the missing sandbox: {refused}"
    );
    assert_eq!(
        super::fake_peer::played(&tools(&tree)),
        0,
        "the program was started"
    );
    Ok(())
}

/// An instance that runs the user's own goose, whose program was rewritten
/// after the instance was written, as a program that updates itself is, is
/// launched as it is now with nothing started first: the program is started
/// once, and answers a turn.
#[cfg(unix)]
#[test]
fn a_changed_own_program_is_launched_without_a_probe() -> Result<(), Failed> {
    let tree = Tree::new("agents-own-changed")?;
    let tried = tried_from_the_command_line(&tree, turning_goose_of_your_own, "goose")?;
    assert_eq!(tried, goose_written()?);
    let program = counted_goose(&tools(&tree))?;

    let turned = goose_turned(
        &tree,
        &super::runtime()?,
        super::shipped_hold::shipped_given,
        (&program.to_string_lossy(), (this_sandbox()?, false)),
    )?;

    assert_eq!(turned.as_deref(), Ok("Answered"));
    assert_eq!(super::fake_peer::played(&tools(&tree)), 1);
    Ok(())
}
