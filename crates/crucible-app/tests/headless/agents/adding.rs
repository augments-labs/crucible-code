//! The agents door adds a catalog agent as one more instance: under the
//! agent's key first, then under the next name the user file leaves free when
//! it is written, and only after a yes, which a standing yes to updates does
//! not stand in for; a no fetches and writes nothing, and a file with no room
//! for it keeps the version and writes nothing, not even the moves the add
//! would have made.

use std::fs;
use std::path::Path;

use crucible_app::client::agents::{AgentsError, How, Outcome};
use crucible_client_api::Pending;
use crucible_config::ConfigError;
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::{
    Answering, Bench, CATALOG, Failed, ONE, TWO, add, answering, facts, install,
    reviewed_with_one_installed, shape,
};

/// The entry the user file holds for instance `instance`, or null.
fn instance(bench: &Bench, name: &str) -> Result<Value, Failed> {
    let value: Value = serde_json::from_slice(&fs::read(bench.user())?)?;
    let at = format!("/externalAgents/{name}");
    Ok(value.pointer(&at).cloned().unwrap_or_default())
}

/// The name an add says it wrote, the last instance its outcome names, and
/// whether it downloaded.
fn added(outcome: &Result<Outcome, AgentsError>) -> Option<(&str, bool)> {
    match outcome {
        Ok(Outcome::Moved {
            how: How::Added,
            instances,
            downloaded,
            ..
        }) => instances.last().map(|instance| (&**instance, *downloaded)),
        _ => None,
    }
}

/// The entry an add writes for goose started as `executable`.
fn written(executable: &Path) -> Value {
    json!({"driver": "catalog", "agent": "goose", "executable": executable.to_string_lossy()})
}

/// The user file read as a document, with instance `instance` taken out.
fn without(bench: &Bench, instance: &str) -> Result<Value, Failed> {
    let mut value: Value = serde_json::from_slice(&fs::read(bench.user())?)?;
    if let Some(agents) = value
        .pointer_mut("/externalAgents")
        .and_then(Value::as_object_mut)
    {
        agents.remove(instance);
    }
    Ok(value)
}

/// What the one install question `front` was put asks, and each choice it
/// offers with what that choice says.
fn asked(front: &Answering<'_>) -> Result<(String, Vec<(String, String)>), Failed> {
    let [(Pending::Install { asks, choices, .. }, _)] = front.put.as_slice() else {
        return Err(format!("one install confirmation, not {:?}", front.put).into());
    };
    let choices = choices.iter().map(|choice| {
        (
            choice.name.as_str().to_owned(),
            choice.says.as_str().to_owned(),
        )
    });
    Ok((asks.as_str().to_owned(), choices.collect()))
}

/// Whether one of the facts `front` was put says `said`.
fn says(front: &Answering<'_>, said: &str) -> Result<bool, Failed> {
    Ok(facts(front)?.iter().any(|fact| fact.contains(said)))
}

#[test]
fn an_add_writes_an_instance_under_the_agent_key_then_one_under_the_next_free_name()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-add")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = without(&bench, "goose")?;
    let written = written(&bench.program("1.0.0"));
    let mut front = answering(&bench.server, Some("Add"));

    let (first, _) = bench.ask(add("1.0.0")?, &mut front, false)?;

    assert_eq!(added(&first), Some(("goose", true)), "{}", shape(&first));
    assert_eq!(
        asked(&front)?,
        (
            "Add goose 1.0.0 as the instance goose?".to_owned(),
            vec![
                (
                    "Add".to_owned(),
                    "Put 1.0.0 in place, move the instances named onto it and add the instance goose"
                        .to_owned()
                ),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ]
        )
    );
    assert!(says(&front, "Adds the instance goose to")?);
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    assert_eq!(instance(&bench, "goose")?, written);
    assert_eq!(without(&bench, "goose")?, before);

    // The version is kept now, so the second add downloads nothing.
    bench.server.forget();
    let mut front = answering(&bench.server, Some("Add"));
    let (second, _) = bench.ask(add("1.0.0")?, &mut front, false)?;

    assert_eq!(
        added(&second),
        Some(("goose-2", false)),
        "{}",
        shape(&second)
    );
    assert_eq!(bench.server.asked(), CATALOG);
    assert!(says(&front, "already installed, so nothing is downloaded")?);
    assert!(says(&front, "Adds the instance goose-2 to")?);
    assert_eq!(asked(&front)?.0, "Add goose 1.0.0 as the instance goose-2?");
    assert_eq!(instance(&bench, "goose-2")?, written);
    assert_eq!(instance(&bench, "goose")?, written);
    Ok(())
}

#[test]
fn a_no_to_an_add_fetches_nothing_and_writes_nothing() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-add-no")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, None);

    let (outcome, told) = bench.ask(add("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    assert!(says(&front, "Adds the instance goose to")?);
    assert_eq!(bench.server.asked(), CATALOG);
    assert!(told.is_empty());
    assert!(!bench.program("1.0.0").exists());
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

#[test]
fn an_add_whose_name_was_taken_while_it_was_asked_writes_the_next_free_one() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-add-taken")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let program = bench.program("1.0.0").to_string_lossy().into_owned();
    let theirs =
        json!({"driver": "catalog", "agent": "goose", "executable": program, "enabled": false});
    // Another instance named goose, written beside work while it is asked.
    let edited =
        json!({"externalAgents": {"work": written(&bench.program("1.0.0")), "goose": theirs}});
    let mut front = answering(&bench.server, Some("Add"));
    front.meanwhile = Some((bench.user(), serde_json::to_vec_pretty(&edited)?));

    let (outcome, _) = bench.ask(add("1.0.0")?, &mut front, false)?;

    assert!(says(&front, "Adds the instance goose to")?);
    assert_eq!(
        added(&outcome),
        Some(("goose-2", true)),
        "{}",
        shape(&outcome)
    );
    assert_eq!(instance(&bench, "goose")?, theirs);
    assert_eq!(
        instance(&bench, "goose-2")?,
        written(&bench.program("1.0.0"))
    );
    Ok(())
}

#[test]
fn an_add_the_user_file_has_no_room_for_keeps_the_version_and_writes_nothing() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-add-full")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    let goose = AgentKey::new("goose")?;
    let program = bench.program("1.0.0").to_string_lossy().into_owned();
    // As many instances as a file may name; the one more is refused.
    let mut full = String::new();
    for _ in 0..64 {
        let Ok((more, _)) = crucible_config::adding(&full, "user", &goose, &program) else {
            break;
        };
        full = more;
    }
    fs::write(bench.user(), &full)?;
    let mut front = answering(&bench.server, Some("Add"));

    let (outcome, _) = bench.ask(add("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unadded { version, .. }) if &**version == "1.0.0"),
        "{}",
        shape(&outcome)
    );
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    assert_eq!(fs::read_to_string(bench.user())?, full);
    Ok(())
}

#[test]
fn an_add_the_user_file_has_no_room_for_moves_no_instance_onto_the_version_it_keeps()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-add-full-moving")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    // Filled to as many instances as a file may name with ones started from
    // outside the store, so `work` is the one instance an add would move.
    let goose = AgentKey::new("goose")?;
    let elsewhere = bench.home.path().join("elsewhere").join("goose");
    let elsewhere = elsewhere.to_string_lossy();
    let mut full = fs::read_to_string(bench.user())?;
    for _ in 0..64 {
        let Ok((more, _)) = crucible_config::adding(&full, "user", &goose, &elsewhere) else {
            break;
        };
        full = more;
    }
    assert!(matches!(
        crucible_config::adding(&full, "user", &goose, &elsewhere),
        Err(ConfigError::TooMany { .. })
    ));
    fs::write(bench.user(), &full)?;
    let mut front = answering(&bench.server, Some("Add"));

    let (outcome, _) = bench.ask(add("2.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unadded { version, .. }) if &**version == "2.0.0"),
        "{}",
        shape(&outcome)
    );
    assert!(says(&front, "Moves work from 1.0.0 onto 2.0.0.")?);
    assert_eq!(fs::read_to_string(bench.user())?, full);
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    assert_eq!(fs::read(bench.program("2.0.0"))?, TWO);
    Ok(())
}

#[test]
fn a_standing_yes_to_updates_still_asks_before_an_add() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-add-reviewed")?;
    reviewed_with_one_installed(&bench, 1)?;
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    bench.server.forget();
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, None);

    let (outcome, told) = bench.ask(add("2.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    // One install question was put, and it was the add's.
    assert!(says(&front, "Adds the instance goose to")?);
    assert_eq!(bench.server.asked(), CATALOG);
    assert!(told.is_empty());
    assert!(!bench.program("2.0.0").exists());
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

#[test]
fn an_add_of_a_newer_version_moves_the_instance_onto_it_beside_the_one_it_writes()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-add-moving")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    let before = fs::read_to_string(bench.user())?;
    let two = bench.program("2.0.0");
    let onto = two.to_string_lossy();
    // The two edits the add makes and nothing else: `work`'s executable
    // spelled as its writer spells it, then the instance spliced in.
    let from = Value::from(&*bench.program("1.0.0").to_string_lossy()).to_string();
    assert_eq!(before.matches(&from).count(), 1, "work names 1.0.0 once");
    let moved = before.replace(&from, &Value::from(&*onto).to_string());
    let (expected, _) = crucible_config::adding(&moved, "user", &AgentKey::new("goose")?, &onto)?;
    let mut front = answering(&bench.server, Some("Add"));

    let (outcome, _) = bench.ask(add("2.0.0")?, &mut front, false)?;

    let Ok(Outcome::Moved {
        how: How::Added,
        instances,
        left,
        downloaded: true,
        ..
    }) = &outcome
    else {
        return Err(shape(&outcome).into());
    };
    let instances: Vec<&str> = instances.iter().map(|instance| &**instance).collect();
    assert_eq!(instances, ["work", "goose"], "{}", shape(&outcome));
    assert!(left.is_empty(), "{}", shape(&outcome));
    assert!(
        says(&front, "Moves work from 1.0.0 onto 2.0.0.")?,
        "the question names the move"
    );
    assert_eq!(
        fs::canonicalize(bench.executable()?)?,
        fs::canonicalize(&two)?,
        "work starts the 2.0.0 program"
    );
    assert_eq!(
        instance(&bench, "goose")?,
        written(&two),
        "the added instance"
    );
    assert_eq!(
        fs::read_to_string(bench.user())?,
        expected,
        "only the two edits"
    );
    assert_eq!(fs::read(&two)?, TWO, "the 2.0.0 program");
    Ok(())
}
