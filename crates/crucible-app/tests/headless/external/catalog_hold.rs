//! A catalog agent is held to the statement its own entry carries.
//!
//! The entry of the agent keyed `goose` is signed twice, once with a statement
//! of how its vendor uses what is sent and once without. With none, the agent
//! is listed and refused at launch with the reason, whatever yes its route has
//! been given; with one, the launch asks that statement's route first, and the
//! agent's program is not started until it has a yes.

use std::path::Path;
use std::time::SystemTime;

use crucible_app::backends::{CatalogRow, Registry};
use crucible_app::content_use::{Consent, Routes, Unstated};
use crucible_app::launching::{Agent, Launcher, Unlaunched};
use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::Control;
use crucible_external_harnesses::catalog::{Catalog, Held, fixture};
use crucible_external_harnesses::fake_peer::{self, scripts::claude};
use crucible_sandbox::SandboxPolicy;
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::Mode;
use crucible_types::AgentKey;

use super::{Peer, runtime};
use crate::{Failed, Tree};

/// The row a signed catalog gives the agent keyed `goose`, its entry carrying
/// `statement` as its statement of content use where one is given.
fn goose_row(statement: Option<serde_json::Value>) -> Result<CatalogRow, Failed> {
    let file =
        serde_json::json!({"path": "goose", "size": 1, "sha256": "b".repeat(64), "exec": true});
    let mut entry = serde_json::json!({
        "key": "goose",
        "registry_id": "goose",
        "version": "1.4.1",
        "kind": "binary",
        "license": "Apache-2.0",
        "review": "https://example.com/review/1",
        "admits": {"name": "goose", "version": "1.4.1"},
        "targets": [{
            "platform": "any",
            "cmd": "goose",
            "artifacts": [{
                "url": "https://example.com/goose.zip",
                "sha256": "a".repeat(64),
                "size": 1,
                "format": "zip",
                "unpacked": 1,
                "decompressed": 1,
                "files": [file],
            }],
        }],
    });
    if let Some(statement) = statement {
        entry
            .as_object_mut()
            .ok_or("an entry is an object")?
            .insert("content_use".into(), statement);
    }
    let catalog = serde_json::json!({
        "schema": 1,
        "serial": 1,
        "revoked": [],
        "entries": [entry],
    });
    let signed = fixture::signed(catalog.to_string().as_bytes());
    let catalog = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        SystemTime::now(),
    )?;
    let entry = catalog.entries().first().ok_or("an entry")?;
    Ok(CatalogRow::new(
        entry,
        entry.targets.first().ok_or("a target")?,
    )?)
}

/// What a start of the agent keyed `goose`, its program the fake peer, came
/// to over a consent holding `yes`: how it ended, and how many times the peer
/// was started.
fn started(
    name: &str,
    row: CatalogRow,
    routes: impl FnOnce(&CatalogRow) -> Routes,
    yes: &[&str],
) -> Result<(Result<(), Unlaunched>, usize), Failed> {
    let tree = Tree::new(name)?;
    let program = fake_peer::install(&tree.0.join("peer"), "claude", &Peer::of([claude::clean()]))?;
    let consent = Consent::new(routes(&row));
    for route in yes {
        consent.give(route);
    }
    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
    ));
    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &program.to_string_lossy(),
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let agent = Agent::configured(&instance, registry, |_| None).ok_or("its catalog row")?;
    let handle = runtime()?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false);
    let sandbox = std::sync::Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = tree.0.join("state");
    let launcher = Launcher::new(sandbox, handle.clone(), &policy, &consent, &state);
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;

    let ended = handle.block_on(launcher.launch(&agent, &control, dialect.as_mut(), None));
    Ok((
        ended.map(drop),
        fake_peer::played(Path::new(&program).parent().ok_or("a directory")?),
    ))
}

/// A catalog agent whose entry gives no statement is refused at launch with
/// the reason, whatever yes its route has been given, and its program is
/// never started.
#[test]
fn a_catalog_agent_without_a_statement_is_refused_at_launch_with_the_reason() -> Result<(), Failed>
{
    let row = goose_row(None)?;
    assert_eq!(row.warned().map(|_| ()), Err(&Unstated::Missing));

    let (ended, played) = started(
        "catalog-hold-unstated",
        row,
        |_| Routes::production(),
        &["agent:goose"],
    )?;

    let refused = ended.err().ok_or("the launch went ahead")?;
    assert!(
        matches!(
            refused,
            Unlaunched::Unstated {
                why: Unstated::Missing,
                ..
            }
        ),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "goose cannot be launched: its catalog entry gives no statement of how its vendor \
         uses what is sent."
    );
    assert_eq!(played, 0, "the program was started");
    Ok(())
}

/// A catalog agent whose entry carries a statement asks the hold of its own
/// route before any byte goes out, and goes ahead once that route has a yes.
#[test]
fn a_catalog_agent_with_a_statement_asks_the_hold_before_any_byte_goes_out() -> Result<(), Failed> {
    let statement = serde_json::json!({
        "text": "The vendor may use what you send to improve goose.",
        "source": "https://vendor.example/terms/data",
        "date": "24 Oct 2026",
        "origins": ["https://api.vendor.example"],
    });
    let hold = |row: &CatalogRow| Routes::production().cataloging(row.warned().ok().cloned());

    let (asked, played) = started(
        "catalog-hold-asked",
        goose_row(Some(statement.clone()))?,
        hold,
        &["agent:another"],
    )?;
    let refused = asked.err().ok_or("the launch went ahead")?;
    let Unlaunched::Asks(warned) = refused else {
        return Err(format!("not asked: {refused:?}").into());
    };
    assert_eq!(warned.route, "agent:goose");
    assert!(
        warned
            .warning
            .sentence
            .starts_with("The vendor may use what you send")
    );
    assert_eq!(warned.warning.link, "https://vendor.example/terms/data");
    assert_eq!(played, 0, "the program was started before the yes");

    let (answered, _played) = started(
        "catalog-hold-answered",
        goose_row(Some(statement))?,
        hold,
        &["agent:goose"],
    )?;
    assert!(answered.is_ok(), "{answered:?}");
    Ok(())
}

/// The sign-out of a catalog instance, which drives a conversation over the
/// registry a test chooses and so needs the `proof` build.
#[cfg(feature = "proof")]
mod sign_out {
    use std::sync::Arc;

    use crucible_app::backends::{CatalogRow, Registry};
    use crucible_app::client;
    use crucible_app::content_use::{Consent, Routes};
    use crucible_app::launching::Steps;
    use crucible_client_api::{Command, LogoutOutcome, Outcome, Pending};
    use crucible_sandbox::SandboxEnablement;

    use super::goose_row;
    use crate::external::{Asked, Picking, accounting_on, connected, launching};
    use crate::{Failed, Script, Tree};

    /// The user file naming one instance of the catalog agent keyed `goose`.
    const GOOSE: &str = r#"{
      "externalAgents": {"work": {"driver": "catalog", "agent": "goose", "executable": "claude"}},
      "updates": {"check": "auto"}
    }
    "#;

    /// A sign-out of the instance of `goose`, its row listed with `row`, over a
    /// consent holding `routes`, answered by `picking`: what the client was told,
    /// what it was put, and what the user file then held.
    fn signed_out(
        name: &str,
        row: CatalogRow,
        routes: Routes,
        picking: &mut Picking,
    ) -> Result<(Outcome, String), Failed> {
        let tree = Tree::new(name)?;
        let (mut launching, peer) = launching(&tree, &connected([]))?;
        launching.consent = Consent::new(routes);
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
        let asked = Asked {
            installed: (GOOSE, "work"),
            command: &|instance| Command::SignOut { instance },
            asking: crucible_client_api::Capabilities::every(),
            after: None,
        };
        accounting_on(&tree, &mut conversation, &asked, picking)
    }

    /// A sign-out of a catalog instance whose route has no hold is refused before
    /// anything is written or started, as a launch is, and not taken for a route
    /// that has its yes; one whose route is held asks it first.
    #[test]
    fn a_sign_out_on_a_catalog_route_with_no_hold_is_refused_before_anything_is_written()
    -> Result<(), Failed> {
        let statement = serde_json::json!({
            "text": "The vendor may use what you send to improve goose.",
            "source": "https://vendor.example/terms/data",
            "date": "24 Oct 2026",
            "origins": [],
        });
        // The entry gives a hold, but the build's routes were not given it.
        let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
        let (outcome, file) = signed_out(
            "catalog-hold-sign-out-unheld",
            goose_row(Some(statement.clone()))?,
            Routes::production(),
            &mut picking,
        )?;
        assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided));
        assert_eq!(file, GOOSE, "the user file was written");
        let said: Vec<&str> = picking
            .put
            .iter()
            .flat_map(|pending| match pending {
                Pending::Account { asks, facts, .. } => std::iter::once(asks)
                    .chain(facts)
                    .map(crucible_client_api::Text::as_str)
                    .collect(),
                _ => Vec::new(),
            })
            .collect();
        assert!(
            said.iter()
                .any(|fact| fact.contains("no hold on what it sends")),
            "{said:?}"
        );

        // The same entry with its hold given asks its route before anything.
        let row = goose_row(Some(statement))?;
        let routes = Routes::production().cataloging(row.warned().ok().cloned());
        let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
        let (outcome, file) = signed_out("catalog-hold-sign-out-held", row, routes, &mut picking)?;
        assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided));
        assert_eq!(file, GOOSE, "the user file was written");
        assert!(
            picking.put.iter().any(|pending| matches!(
                pending,
                Pending::Warning { route, .. } if route.as_str() == "agent:goose"
            )),
            "the route was not asked: {:?}",
            picking.put
        );
        Ok(())
    }
}
