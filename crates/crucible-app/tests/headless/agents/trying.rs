//! `crucible agents try`'s move, as a command line asks the agents door it:
//! with no conversation, but a launcher and its steps lent to start the
//! user's own program, the path taken from the catalog's listing, and the
//! door's question answered yes. What it writes is what `/login` writes for
//! the same program.

use crucible_app::Conversation;
use crucible_app::client::agents::Outcome as Door;
use crucible_client_api::{Command, Name, Text};

use crate::external::choosing::{
    agy_of_your_own, goose_of_your_own, instance_written, tried_at_door, tried_through_login,
};
use crate::{Failed, Tree};

/// The instance `crucible agents try agent` writes in `tree`, set up by
/// `of_your_own`, as [`instance_written`] gives it, once the listing found
/// the user's own program at the path the setup says its links lead to.
fn tried_from_the_command_line(
    tree: &Tree,
    of_your_own: fn(&Tree) -> Result<(Conversation, std::path::PathBuf), Failed>,
    agent: &str,
) -> Result<String, Failed> {
    let (conversation, program) = of_your_own(tree)?;
    let listed = tried_at_door(tree, &conversation, Command::AgentCatalog)?;
    let Ok(Door::Catalog {
        agents,
        antigravity,
        ..
    }) = listed
    else {
        return Err(format!("the listing answered {listed:?}").into());
    };
    let found = match antigravity.filter(|own| own.agent.as_str() == agent) {
        Some(own) => Some(own.program),
        None => agents
            .into_iter()
            .find(|listed| listed.agent.as_str() == agent)
            .and_then(|listed| listed.own),
    };
    assert_eq!(found.as_ref(), Some(&program), "the listing's own {agent}");
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
