//! `/login`'s row of a catalog agent reads its instances from the user file as
//! the agents door does, not from the settings read at the start.

use std::fs;
use std::path::Path;

use crucible_client_api::{Command, Pending, Text};
use serde_json::json;

use super::{Bench, Connecting, ONE, Outcome, answering, connecting_after};
use crate::Failed;

/// `/login`'s row of goose and the question it opens, as a connect picking it
/// puts them, the user file changed by `meanwhile` after the conversation read
/// its settings: what the row says, and the question's facts and choices.
fn goose_row_after(
    bench: &Bench,
    meanwhile: impl FnOnce() -> Result<(), Failed>,
) -> Result<(String, Vec<String>, Vec<String>), Failed> {
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    let mut listing = answering(&bench.server, None);
    let (listed, _) = bench.ask(Command::AgentCatalog, &mut listing, false)?;
    assert!(
        matches!(listed, Ok(Outcome::Catalog { .. })),
        "the catalog was not taken in"
    );
    let mut front = Connecting {
        picks: ["goose"].into(),
        put: Vec::new(),
    };
    connecting_after(bench, &mut front, meanwhile)?;
    let Some(Pending::Account { choices, .. }) = front.put.first() else {
        return Err("no agent was asked for".into());
    };
    let row = choices
        .iter()
        .find(|choice| choice.name.as_str() == "goose")
        .ok_or("no row of goose")?
        .says
        .as_str()
        .to_owned();
    let Some(Pending::Account { facts, choices, .. }) = front.put.get(1) else {
        return Err("the row asked nothing".into());
    };
    let said = |text: &Text| text.as_str().to_owned();
    Ok((
        row,
        facts.iter().map(said).collect(),
        choices.iter().map(|choice| said(&choice.name)).collect(),
    ))
}

/// An instance `/catalog` added while the client ran is in the user file and
/// not in the settings read at the start: `/login`'s row of its agent finds it
/// there, without a restart, and no line says there is none.
#[test]
fn a_login_row_finds_an_instance_written_after_the_client_started() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-login-fresh")?;

    let (row, facts, choices) =
        goose_row_after(&bench, || bench.configure(Path::new("goose"), &json!({})))?;

    assert!(
        row.ends_with("Runs your own program, not a release Crucible installed."),
        "the row said nothing of the instance in the file"
    );
    assert!(
        facts
            .iter()
            .any(|fact| fact == "Agent: work, run as goose."),
        "the question did not name the instance in the file"
    );
    assert!(
        !facts.iter().any(|fact| fact.starts_with("No instance")),
        "the question said no instance is configured"
    );
    assert_eq!(choices.last().map(String::as_str), Some("Go back"));
    Ok(())
}

/// A user file that cannot be read is said, in what the door says of it, by
/// the row and its question, and is not taken for one naming no instance. No
/// move is offered, since the door refuses each on the same read.
#[test]
fn a_login_row_says_a_user_file_it_cannot_read_and_not_that_no_instance_is_configured()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-login-unread")?;

    let (row, facts, choices) =
        goose_row_after(&bench, || Ok(fs::write(bench.user(), "{ not json")?))?;

    assert!(
        row.ends_with("Its instances could not be read."),
        "the row did not say the file could not be read"
    );
    assert!(
        facts.iter().any(|fact| fact.contains("is not valid JSON")),
        "the question did not say the file could not be read"
    );
    assert!(
        !facts.iter().any(|fact| fact.starts_with("No instance")),
        "the question said no instance is configured"
    );
    assert_eq!(choices, ["Go back"]);
    Ok(())
}
