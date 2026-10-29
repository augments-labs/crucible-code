//! What a connect offers a client that can carry out a move on an agent, and
//! what it keeps from one that cannot.
//!
//! A client that claims the install capability is offered the release of an
//! agent not installed, each agent the catalog offers after the shipping
//! ones, with that agent's own moves, and a sign-in by each method of an
//! agent read signed out. The move chosen ends the connect: one chosen before
//! the agent's status is read with nothing started, asked of its route or
//! written, and a sign-in after the route's yes and that read, with nothing
//! more. A client that does not claim it is put the questions it was put
//! before any of this was offered.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crucible_app::Conversation;
use crucible_app::access::Route;
use crucible_app::backends::{REGISTRY, Registry, Row};
use crucible_app::catalog_fetch;
use crucible_app::client::accounts::CHECK_AGAIN;
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_client_api::{
    AgentAction, Capabilities as Asking, Capability, Command, LoginOutcome, Name, Outcome, Pending,
    Text,
};
use crucible_config::{Driver, Serves};
use crucible_external_harnesses::catalog::{File, FileKind, SNAPSHOT, Trust, fixture};
use crucible_external_harnesses::fake_peer::{self, Reply, Script as Peer, scripts::claude};
use crucible_external_harnesses::store::{Listing, Provenance, Store, Version};
use crucible_sandbox::{SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::AgentKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    Asked, Launching, Outside, Picking, UNINSTALLED, accounting, accounting_on, connected, offered,
    runtime,
};
use crate::{Failed, Script, Tree};

/// A client that claims every capability but the install one.
fn uninstalling() -> Asking {
    Asking::every().without(Capability::Install)
}

/// Asks a connect naming no instance of a client claiming `asking` that
/// answers as `picking` does, the user file holding `file` first, once the
/// conversation's own run has started; and answers what the client was told,
/// what the user file then held, and how many times the agent's program was
/// started for the connect.
fn connecting_as(
    tree: &Tree,
    outside: &mut Outside,
    file: &str,
    asking: Asking,
    picking: &mut Picking,
) -> Result<((Outcome, String), usize), Failed> {
    let command = |_| Command::Connect { instance: None };
    let asked = Asked {
        installed: (file, "work"),
        command: &command,
        asking,
        after: None,
    };
    for _ in 0..500 {
        if fake_peer::played(&outside.peer) >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let before = fake_peer::played(&outside.peer);
    let asked = accounting(tree, outside, &asked, picking)?;
    Ok((asked, fake_peer::played(&outside.peer) - before))
}

/// [`connecting_as`], of a conversation that started no agent and takes no
/// agent steps, its routes warned about as the application's are, through
/// `instance` where one is named.
fn connecting_unstepped(
    tree: &Tree,
    (file, instance): (&str, Option<&str>),
    asking: Asking,
    picking: &mut Picking,
) -> Result<(Outcome, String), Failed> {
    let mut conversation = crate::conversation(tree, Script::new(Vec::new()), false)?
        .consenting(Consent::new(Routes::production()));
    connecting_on(tree, &mut conversation, (file, instance), asking, picking)
}

/// A connect through `instance`, or naming none, of `conversation`.
fn connecting_on(
    tree: &Tree,
    conversation: &mut Conversation,
    (file, instance): (&str, Option<&str>),
    asking: Asking,
    picking: &mut Picking,
) -> Result<(Outcome, String), Failed> {
    let named = instance.is_some();
    let command = move |instance| Command::Connect {
        instance: named.then_some(instance),
    };
    let asked = Asked {
        installed: (file, instance.unwrap_or("work")),
        command: &command,
        asking,
        after: None,
    };
    accounting_on(tree, conversation, &asked, picking)
}

/// Each account question in `put`, written out whole: what it asks, each
/// fact, and each choice with what it says; `tree`'s directory written as
/// `<tree>`, every separator as `/` and a program's suffix (`.exe` on
/// Windows, `.par` where Antigravity's release elsewhere names its program)
/// left out, so it reads the same on any machine.
fn written_out(put: &[Pending], tree: &Tree) -> Vec<String> {
    let mut roots: Vec<String> = std::fs::canonicalize(&tree.0)
        .into_iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    roots.push(tree.0.to_string_lossy().into_owned());
    put.iter()
        .filter_map(|put| match put {
            Pending::Account {
                asks,
                facts,
                choices,
                ..
            } => {
                let mut lines = vec![format!("asks: {}", asks.as_str())];
                lines.extend(facts.iter().map(|fact| format!("fact: {}", fact.as_str())));
                lines.extend(choices.iter().map(|choice| {
                    let marked = if choice.recommended {
                        " (recommended)"
                    } else {
                        ""
                    };
                    format!(
                        "choice: {}{marked}: {}",
                        choice.name.as_str(),
                        choice.says.as_str()
                    )
                }));
                let mut text = lines.join("\n");
                for root in &roots {
                    text = text.replace(root.as_str(), "<tree>");
                }
                Some(
                    text.replace(std::path::MAIN_SEPARATOR, "/")
                        .replace(".exe", "")
                        .replace(".par", ""),
                )
            }
            _ => None,
        })
        .collect()
}

/// What an account question asks, its facts, and its choices, each by name
/// and what it says.
type Question<'a> = (&'a str, Vec<&'a str>, Vec<(&'a str, &'a str)>);

/// The account question put `at`, as a [`Question`].
fn question(put: &[Pending], at: usize) -> Option<Question<'_>> {
    match put
        .iter()
        .filter(|put| matches!(put, Pending::Account { .. }))
        .nth(at)?
    {
        Pending::Account {
            asks,
            facts,
            choices,
            ..
        } => Some((
            asks.as_str(),
            facts.iter().map(Text::as_str).collect(),
            choices
                .iter()
                .map(|choice| (choice.name.as_str(), choice.says.as_str()))
                .collect(),
        )),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// How this machine's platform is spelled in a catalog.
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// The catalog entry of `version` of the agent `key`, whose program
/// `program` was built at review.
fn entry(key: &str, version: &str, program: &[u8]) -> Value {
    json!({
        "key": key,
        "registry_id": format!("{key}-acp"),
        "version": version,
        "kind": "binary",
        "license": "Apache-2.0",
        "terms": "https://example.com/terms",
        "caution": "Read the terms first.",
        "review": "https://example.com/review/12",
        "admits": {"name": key, "version": version},
        "targets": [{
            "platform": platform(),
            "cmd": "goose",
            "args": ["acp"],
            "env": {},
            "hosts": ["downloads.example.com"],
            "artifacts": [{
                "url": "https://downloads.example.com/goose",
                "sha256": "a".repeat(64),
                "size": 1000,
                "format": "bare",
                "unpacked": program.len(),
                "decompressed": program.len(),
                "files": [{
                    "path": "goose",
                    "size": program.len(),
                    "sha256": digest(program),
                    "exec": true
                }],
                "replaced_by": {"sha256": digest(program), "size": program.len()}
            }]
        }]
    })
}

const ONE: &[u8] = b"goose 1.0.0, as reviewed";
const TWO: &[u8] = b"goose 2.0.0, as reviewed, and longer";

/// Takes into the trust state in `tree`'s home a catalog signed for it that
/// lists `entries`, as a fetch of it would.
fn trusting(tree: &Tree, entries: &[Value]) -> Result<(), Failed> {
    let catalog = json!({
        "schema": 1,
        "serial": SNAPSHOT.serial() + 1,
        "entries": entries,
        "revoked": [],
    });
    let signed = fixture::signed(&serde_json::to_vec(&catalog)?);
    let now = SystemTime::now();
    let home = tree.home()?;
    Trust::load(&catalog_fetch::state_in(home.path()), now).take_in(
        &signed.index,
        &signed.signature,
        &signed.keys,
        now,
    )?;
    Ok(())
}

/// Puts `version` of goose, its program `program`, into crucible's store in
/// `tree`'s home as fetched at `fetched`, and answers where its program lies.
fn stored(
    tree: &Tree,
    (version, program): (&str, &[u8]),
    fetched: SystemTime,
) -> Result<PathBuf, Failed> {
    let home = tree.home()?;
    let store = Store::new(home.path().join("programs"));
    let key = AgentKey::new("goose")?;
    let staging = store.stage(&key)?;
    let path = staging.directory().join("goose");
    std::fs::write(&path, program)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    let version = Version::new(version, None)?;
    let file = File {
        path: "goose".into(),
        kind: FileKind::File {
            size: u64::try_from(program.len())?,
            sha256: digest(program).into(),
            exec: true,
        },
    };
    let provenance = Provenance {
        listing: Listing {
            serial: SNAPSHOT.serial() + 1,
            review: "https://example.com/review/12".into(),
            terms: None,
        },
        fetched,
        artifacts: Vec::new(),
    };
    store.commit(staging, &version, &[file], provenance)?;
    Ok(store.directory(&key, &version).join("goose"))
}

/// A user file naming one instance `work` of goose, run as `executable`.
fn goose_file(executable: &str) -> Result<String, Failed> {
    Ok(serde_json::to_string_pretty(&json!({
        "externalAgents": {
            "work": {"driver": "catalog", "agent": "goose", "executable": executable}
        }
    }))?)
}

/// An Antigravity not installed, connected naming no instance by a client
/// that claims the install capability, is offered the release crucible
/// installs, first; choosing it ends the connect on that install, with
/// nothing started or written.
#[test]
fn a_client_that_installs_is_offered_the_release_of_an_agent_not_installed() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-install")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut picking = Picking::of([Some("Antigravity"), Some("Install")]);

    let ((outcome, file), started) = connecting_as(
        &tree,
        &mut outside,
        UNINSTALLED,
        Asking::every(),
        &mut picking,
    )?;

    let row = REGISTRY
        .built(Driver::Antigravity)
        .ok_or("Antigravity has no row")?;
    let pinned = (row.pinned)(&catalog_fetch::trust_in(tree.home()?.path()))
        .ok_or("Antigravity's release is pinned")?;
    let action = AgentAction::Install {
        agent: Name::new("antigravity")?,
        version: Name::new(&pinned.version)?,
    };
    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentChosen { action })
    );
    let agents = vec!["Claude Code", "Antigravity", "Cancel"];
    let missing = vec!["Install", CHECK_AGAIN, "Go back"];
    assert_eq!(offered(&picking.put), [agents, missing]);
    let install = question(&picking.put, 1).and_then(|(_, _, choices)| choices.first().copied());
    let says = format!("Install Antigravity {}.", pinned.version);
    assert_eq!(install, Some(("Install", says.as_str())));
    assert_eq!(started, 0, "the agent's program was started");
    assert_eq!(file, UNINSTALLED);
    Ok(())
}

/// The agents a connect naming no instance asks a client to choose among,
/// written out as [`written_out`] writes it.
const AGENTS: &str = "asks: Which installed agent should Crucible use?
fact: Crucible looks for the agent's program; nothing is started or written yet.
choice: Claude Code (recommended): Reach anthropic through Claude Code.
choice: Antigravity: Reach google through Antigravity.
choice: Cancel: Change nothing.";

/// What a connect says of an Antigravity not installed, written out as
/// [`written_out`] writes it.
const NOT_INSTALLED: &str = "asks: Antigravity cannot be started.
fact: Not installed: no antigravity command on PATH and none at the configured executable.
fact: Run `crucible agents install antigravity` in a terminal, then choose Check again.
fact: Agent: antigravity, run as <tree>/home/programs/antigravity/1.3.0/agy_acp_server.
choice: Check again (recommended): Look for the agent's program again.
choice: Go back: Choose another agent.";

/// What a client that did not claim the install capability is put for the
/// bench below, as it was put before any move was offered.
const UNOFFERED: [&str; 4] = [AGENTS, NOT_INSTALLED, NOT_INSTALLED, AGENTS];

/// A client that does not claim the install capability is put the very
/// questions it was put before: no catalog agent listed, though the trust
/// state offers one, and no install offered for an agent not installed.
#[test]
fn a_client_that_does_not_install_is_put_the_questions_it_was_put_before() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-unoffered")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    let mut picking = Picking::of([
        Some("Antigravity"),
        Some(CHECK_AGAIN),
        Some("Go back"),
        Some("Cancel"),
    ]);

    let ((outcome, file), started) = connecting_as(
        &tree,
        &mut outside,
        UNINSTALLED,
        uninstalling(),
        &mut picking,
    )?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    assert_eq!(written_out(&picking.put, &tree), UNOFFERED);
    assert_eq!(started, 0, "the agent's program was started");
    assert_eq!(file, UNINSTALLED);
    Ok(())
}

/// Each agent the catalog offers is listed after the shipping ones, saying
/// the version offered and, of an instance running the user's own program,
/// so; choosing it asks what to do with that agent, whose instance is never
/// connected, and going back offers the agents again.
#[test]
fn a_catalog_agent_is_listed_after_the_shipping_ones_and_never_connected() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-catalog")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    let before = goose_file("goose")?;
    let mut picking = Picking::of([Some("goose"), Some("Go back"), Some("Cancel")]);

    let (outcome, file) =
        connecting_unstepped(&tree, (&before, None), Asking::every(), &mut picking)?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    let agents = vec!["Claude Code", "Antigravity", "goose", "Cancel"];
    let managed = vec!["Install", "Go back"];
    assert_eq!(offered(&picking.put), [agents.clone(), managed, agents]);
    let own = "Runs your own program, not a release Crucible installed.";
    let listed = question(&picking.put, 0)
        .and_then(|(_, _, choices)| choices.get(2).copied())
        .map(|(_, says)| says.to_owned());
    assert_eq!(
        listed,
        Some(format!(
            "Manage goose, which the catalog offers at 1.0.0. {own}"
        ))
    );
    let (asks, facts, choices) = question(&picking.put, 1).ok_or("the agent's question")?;
    assert_eq!(asks, "What should Crucible do with goose?");
    assert_eq!(
        facts,
        [
            "Crucible's store holds no version of it.",
            "Agent: work, run as goose.",
            own,
        ]
    );
    assert_eq!(choices.first(), Some(&("Install", "Install goose 1.0.0.")));
    assert_eq!(file, before);
    Ok(())
}

/// Antigravity's own row, reaching no host: the peer reaches none.
fn antigravity_row() -> Result<&'static Row, Failed> {
    let shipped = REGISTRY
        .built(Driver::Antigravity)
        .ok_or("Antigravity has no row")?;
    Ok(Box::leak(Box::new(Row {
        reaches: &[],
        ..*shipped
    })))
}

/// A conversation whose instance `space` of Antigravity is played by the peer
/// reading signed out at each of `runs` status checks, its route unanswered,
/// a yes to it written into the user file a connect is asked with; and that
/// file, naming the instance.
fn signed_out_antigravity(tree: &Tree, runs: usize) -> Result<(Conversation, String), Failed> {
    use crucible_external_harnesses::acp::SignIn;
    use crucible_external_harnesses::antigravity::Manifest;
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let shipped = SNAPSHOT.entries().first().ok_or("the snapshot's entry")?;
    let reviewed = acp::initialized(
        &shipped.admits.name,
        &shipped.admits.version,
        Manifest::REVIEWED.sign_in.first().map_or("", SignIn::id),
    );
    let signed_out = Reply::new()
        .hear(1)
        .say(reviewed.as_str())
        .hear(1)
        .say(acp::sign_in_required(1))
        .hear_out();
    let peer = tree.0.join("peer");
    let program = fake_peer::install(&peer, "antigravity", &Peer::of(vec![signed_out; runs]))?;
    let row = antigravity_row()?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(crate::Desk::new(tree, &[])?.choosing);
    let runtime = runtime()?;
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.clone()));
    let executable = program.to_string_lossy().into_owned();
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: executable.as_str().into(),
        profile: None,
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let registry = Registry::new(std::slice::from_ref(row));
    let agent = Agent::routed(&route, &registry, |_| None).ok_or("Antigravity's route")?;
    let launching = Launching {
        sandbox,
        runtime,
        policy: SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false),
        consent: consent.clone(),
        state: tree.0.join("state"),
        sessions: tree.sessions(),
        agent,
        row,
        launched: Arc::default(),
    };
    let conversation = crate::conversation(tree, Script::new(Vec::new()), false)?
        .consenting(consent)
        .stepping(super::steps(&launching, &peer));
    let profile = tree.home()?.path().join("agy");
    let file = serde_json::to_string_pretty(&json!({
        "externalAgents": {
            "space": {
                "driver": "antigravity",
                "executable": executable,
                "profileDir": profile.to_string_lossy(),
            }
        }
    }))?;
    Ok((conversation, file))
}

/// An agent read signed out offers a client that claims the install
/// capability a sign-in by each method it gives, before checking again.
/// Choosing one ends the connect on that sign-in after the route's yes, which
/// is written down, and after the agent was started once to read its status;
/// the choice itself writes and starts nothing more. A client that does not
/// claim it is offered only to check again or cancel.
#[test]
fn an_agent_read_signed_out_offers_its_sign_in_methods() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-sign-in")?;
    let (mut conversation, before) = signed_out_antigravity(&tree, 2)?;
    let peer = tree.0.join("peer");
    let route = antigravity_row()?.route;
    let mut picking = Picking::of([Some("Accept"), Some("Gemini API key")]);

    let (outcome, file) = connecting_on(
        &tree,
        &mut conversation,
        (&before, Some("space")),
        Asking::every(),
        &mut picking,
    )?;

    let action = AgentAction::SignIn {
        instance: Name::new("space")?,
        method: Name::new("gemini-api-key")?,
    };
    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentChosen { action })
    );
    assert!(
        matches!(
            picking.put.first(),
            Some(Pending::Warning { route: asked, .. }) if asked.as_str() == route
        ),
        "the route is asked before anything of the agent's is started: {:?}",
        picking.put
    );
    assert_eq!(
        offered(&picking.put),
        [vec![
            "Sign in with Google",
            "Sign in with a Google business account",
            "Gemini API key",
            "Gemini Enterprise Agent Platform API key",
            CHECK_AGAIN,
            "Cancel",
        ]]
    );
    assert_eq!(
        fake_peer::played(&peer),
        1,
        "the agent is started once, to read its status"
    );
    let mut written: Value = serde_json::from_str(&file)?;
    let accepted = written
        .as_object_mut()
        .and_then(|file| file.remove("contentUse"))
        .ok_or("a yes written into the user file")?;
    assert_eq!(accepted, json!({ "accepted": [route] }));
    assert_eq!(
        written,
        serde_json::from_str::<Value>(&before)?,
        "nothing but the yes is written"
    );

    // The route has its yes now, so the client that does not claim install
    // is put the signed-out question at once.
    let mut unclaimed = Picking::of([Some("Cancel")]);
    let (outcome, file) = connecting_on(
        &tree,
        &mut conversation,
        (&before, Some("space")),
        uninstalling(),
        &mut unclaimed,
    )?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    assert_eq!(offered(&unclaimed.put), [vec![CHECK_AGAIN, "Cancel"]]);
    assert_eq!(file, before);
    assert_eq!(fake_peer::played(&peer), 2);
    let (asks, facts, _) = question(&picking.put, 0).ok_or("the sign-in question")?;
    assert_eq!(asks, "space is not signed in.");
    let (unclaimed_asks, unclaimed_facts, _) =
        question(&unclaimed.put, 0).ok_or("the sign-in question")?;
    assert_eq!((asks, facts), (unclaimed_asks, unclaimed_facts));
    Ok(())
}

/// A catalog agent with two versions in crucible's store is offered a roll
/// back where its instance runs the version offered first, and an update as
/// well where it runs the older one, each naming the version it runs and no
/// other, since the door picks the one it moves to; the move chosen ends the
/// connect.
#[test]
fn a_catalog_agent_with_two_versions_stored_offers_update_or_roll_back() -> Result<(), Failed> {
    let cases = [
        (
            "newest",
            "2.0.0",
            vec![
                ("Roll back", "Roll back goose from 2.0.0."),
                ("Go back", "Choose another agent."),
            ],
            "Roll back",
        ),
        (
            "older",
            "1.0.0",
            vec![
                ("Update", "Update goose from 1.0.0."),
                ("Roll back", "Roll back goose from 1.0.0."),
                ("Go back", "Choose another agent."),
            ],
            "Update",
        ),
    ];
    for (case, running, expected, pick) in cases {
        let tree = Tree::new(&format!("external-choosing-moves-{case}"))?;
        trusting(
            &tree,
            &[entry("goose", "2.0.0", TWO), entry("goose", "1.0.0", ONE)],
        )?;
        let now = SystemTime::now();
        let older = stored(&tree, ("1.0.0", ONE), now - Duration::from_mins(1))?;
        let newest = stored(&tree, ("2.0.0", TWO), now)?;
        let program = if running == "1.0.0" { older } else { newest };
        let before = goose_file(&program.to_string_lossy())?;
        let mut picking = Picking::of([Some("goose"), Some(pick)]);

        let (outcome, file) =
            connecting_unstepped(&tree, (&before, None), Asking::every(), &mut picking)?;

        let goose = Name::new("goose")?;
        let action = if pick == "Update" {
            AgentAction::Update { agent: goose }
        } else {
            AgentAction::Rollback { agent: goose }
        };
        assert_eq!(
            outcome,
            Outcome::Login(LoginOutcome::AgentChosen { action }),
            "{case}"
        );
        let (_, _, choices) = question(&picking.put, 1).ok_or("the agent's question")?;
        assert_eq!(choices, expected, "{case}");
        assert_eq!(file, before, "{case}");
    }
    Ok(())
}

/// A sign-in method's name the catalog gave is put with its control
/// characters escaped, so it draws nothing of its own; a method only a
/// terminal runs is not offered; and a method chosen ends the connect on a
/// sign-in of the instance by it.
#[test]
fn a_catalog_method_s_name_is_escaped_in_the_question() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-escaped")?;
    let mut release = entry("goose", "1.0.0", ONE);
    if let Some(object) = release.as_object_mut() {
        object.insert(
            "auth_methods".into(),
            json!([
                {"id": "browser", "kind": "agent", "name": "Sign in\u{1b}[2J here"},
                {"id": "api-key", "kind": "env_var", "name": "API key", "var": "GOOSE_API_KEY"},
                {"id": "terminal", "kind": "terminal", "name": "Sign in at a terminal",
                 "command": "goose", "args": ["login"], "env": {}, "route": "piped"},
            ]),
        );
        object.insert("sign_in_hosts".into(), json!(["accounts.vendor.example"]));
    }
    trusting(&tree, &[release])?;
    let before = goose_file("goose")?;
    let mut picking = Picking::of([Some("goose"), Some("API key")]);

    let (outcome, file) =
        connecting_unstepped(&tree, (&before, None), Asking::every(), &mut picking)?;

    let action = AgentAction::SignIn {
        instance: Name::new("work")?,
        method: Name::new("api-key")?,
    };
    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentChosen { action })
    );
    let (_, _, choices) = question(&picking.put, 1).ok_or("the agent's question")?;
    let named: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        named,
        ["Install", "Sign in\\u{1b}[2J here", "API key", "Go back"]
    );
    let drawn = picking.put.iter().any(|put| match put {
        Pending::Account {
            asks,
            facts,
            choices,
            ..
        } => {
            asks.as_str().contains('\u{1b}')
                || facts.iter().any(|fact| fact.as_str().contains('\u{1b}'))
                || choices.iter().any(|choice| {
                    choice.name.as_str().contains('\u{1b}')
                        || choice.says.as_str().contains('\u{1b}')
                })
        }
        _ => false,
    });
    assert!(!drawn, "a control character was put as it came");
    assert_eq!(file, before);
    Ok(())
}

/// A sign-in method the catalog names like a choice the question already
/// gives, or like a method before it, is not offered: names are told apart
/// without the whitespace around them and whatever their case, so a method
/// cannot pass for "Go back" or "Install" by a trailing space or a capital.
#[test]
fn a_catalog_method_named_like_a_fixed_choice_is_not_offered() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-lookalike")?;
    let mut release = entry("goose", "1.0.0", ONE);
    if let Some(object) = release.as_object_mut() {
        object.insert(
            "auth_methods".into(),
            json!([
                {"id": "back", "kind": "agent", "name": "Go back "},
                {"id": "lower", "kind": "agent", "name": "go back"},
                {"id": "install", "kind": "agent", "name": " INSTALL"},
                {"id": "api-key", "kind": "env_var", "name": "API key", "var": "GOOSE_API_KEY"},
                {"id": "again", "kind": "env_var", "name": "api key ", "var": "GOOSE_KEY"},
            ]),
        );
        object.insert("sign_in_hosts".into(), json!(["accounts.vendor.example"]));
    }
    trusting(&tree, &[release])?;
    let before = goose_file("goose")?;
    let mut picking = Picking::of([Some("goose"), Some("API key")]);

    let (outcome, file) =
        connecting_unstepped(&tree, (&before, None), Asking::every(), &mut picking)?;

    let action = AgentAction::SignIn {
        instance: Name::new("work")?,
        method: Name::new("api-key")?,
    };
    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentChosen { action })
    );
    let (_, _, choices) = question(&picking.put, 1).ok_or("the agent's question")?;
    let named: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, ["Install", "API key", "Go back"]);
    assert_eq!(file, before);
    Ok(())
}
