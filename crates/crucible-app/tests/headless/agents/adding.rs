//! The agents door adds a catalog agent as one more instance: under the
//! agent's key first, then under the next name the user file leaves free when
//! it is written, and only after a yes; a no fetches and writes nothing, and
//! a file with no room for it keeps the version and writes nothing.

use std::fs;
use std::path::Path;

use crucible_app::client::agents::{AgentsError, How, Outcome};
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::{Answering, Bench, CATALOG, Failed, ONE, add, answering, facts, shape};

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
    let mut front = answering(&bench.server, Some("Install"));

    let (first, _) = bench.ask(add("1.0.0")?, &mut front, false)?;

    assert_eq!(added(&first), Some(("goose", true)), "{}", shape(&first));
    assert!(says(&front, "Adds the instance goose to")?);
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    assert_eq!(instance(&bench, "goose")?, written);
    assert_eq!(without(&bench, "goose")?, before);

    // The version is kept now, so the second add downloads nothing.
    bench.server.forget();
    let mut front = answering(&bench.server, Some("Install"));
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
    let mut front = answering(&bench.server, Some("Install"));
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
    let mut front = answering(&bench.server, Some("Install"));

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
