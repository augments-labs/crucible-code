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
use crucible_app::client::agents::{AgentsError, Outcome as Door, OwnProgram, refused_beside};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_app::own_program;
use crucible_client_api::{
    AgentAction, Capabilities as Asking, Capability, Command, LoginOutcome, Name, Outcome, Pending,
    Text,
};
use crucible_config::{Driver, Serves};
use crucible_external_harnesses::catalog::{File, FileKind, Keys, SNAPSHOT, Trust, fixture};
use crucible_external_harnesses::fake_peer::{self, Reply, Script as Peer, scripts::claude};
use crucible_external_harnesses::store::{Listing, Provenance, Source, Store, Version};
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
                Some(portable(&text))
            }
            _ => None,
        })
        .collect()
}

/// `text` as [`written_out`] writes it once the tree is named: every
/// separator as `/` and a program's suffix left out. A test looking for text
/// the machine's separator could appear in, as a `\` an escape writes on
/// Windows, writes what it looks for through this too.
pub(crate) fn portable(text: &str) -> String {
    text.replace(std::path::MAIN_SEPARATOR, "/")
        .replace(".exe", "")
        .replace(".par", "")
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
        source: Source::Catalog,
        probe: None,
        fetched,
        artifacts: Vec::new(),
        decompressed: u64::try_from(program.len())?,
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
fact: Not installed: no agy or antigravity command on PATH and none at the configured executable. If it is installed under another name, set executable to its path or name.
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
/// so; choosing it asks what to do with that agent, whose instance running
/// the user's own program has nothing to connect until a probe of it is
/// kept, and going back offers the agents again. Once trying it keeps one,
/// that instance is offered Connect, and the user file is as it was.
#[test]
fn a_catalog_agent_is_listed_after_the_shipping_ones_and_its_own_program_is_not_connected()
-> Result<(), Failed> {
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

    own_goose(&tools(&tree), true)?;
    let mut conversation = own_sandboxed(&tree)?;
    let (_, choices) = goose_managed(&tree, &mut conversation, &before)?;
    assert_eq!(choices, ["Try your own", "Install", "Go back"]);
    let (action, _) = chose_own(&tree, &mut conversation, &before, "goose")?;
    let tried = carried(
        &tree,
        &conversation,
        &action,
        &mut Picking::of([Some("Try it")]),
    )?;
    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert_eq!(&*instance, "work");
    assert_eq!(
        std::fs::read_to_string(crate::Desk::new(&tree, &[])?.choosing)?,
        before
    );
    assert!(
        goose_record(&tree, "work")?.is_some(),
        "no probe record was kept"
    );
    let (_, choices) = goose_managed(&tree, &mut conversation, &before)?;
    assert_eq!(choices, ["Connect", "Install", "Go back"]);
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
    antigravity_started(tree, runs, Started::Profiled)
}

/// How [`antigravity_started`] starts the instance.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Started {
    /// With the sandbox off and a profile directory.
    Profiled,
    /// As the user's own program under the sandbox, with no profile
    /// directory, the user file turning the sandbox on.
    Sandboxed,
    /// As [`Started::Sandboxed`], the sandbox required by a project.
    Required,
}

/// A connect's outcome, the choices of each account question it put, and each
/// written out as [`written_out`] writes it.
pub(crate) type Connected = (Outcome, Vec<Vec<String>>, Vec<String>);

/// What a connect to the instance `space` of the user's own Antigravity,
/// started as `started` says and read signed out, puts to a client that
/// claims every capability, accepts the route and then cancels: the outcome,
/// the choices of each account question, and each written out as
/// [`written_out`] writes it.
pub(crate) fn own_agy_connected(tree: &Tree, started: Started) -> Result<Connected, Failed> {
    let (mut conversation, before) = antigravity_started(tree, 1, started)?;
    let mut picking = Picking::of([Some("Accept"), Some("Cancel")]);
    let (outcome, _) = connecting_on(
        tree,
        &mut conversation,
        (&before, Some("space")),
        Asking::every(),
        &mut picking,
    )?;
    let choices = offered(&picking.put)
        .into_iter()
        .map(|one| one.into_iter().map(str::to_owned).collect())
        .collect();
    Ok((outcome, choices, written_out(&picking.put, tree)))
}

/// What a connect naming `instance`, asked in `conversation` by a client that
/// claims every capability and cancels at the first account question, the
/// user file holding `file` first, ends with, and each account question
/// written out as [`written_out`] writes it.
pub(crate) fn connect_cancelled(
    tree: &Tree,
    conversation: &mut Conversation,
    (file, instance): (&str, &str),
) -> Result<(Outcome, Vec<String>), Failed> {
    let mut picking = Picking::of([Some("Cancel")]);
    let (outcome, _) = connecting_on(
        tree,
        conversation,
        (file, Some(instance)),
        Asking::every(),
        &mut picking,
    )?;
    Ok((outcome, written_out(&picking.put, tree)))
}

/// [`signed_out_antigravity`], its instance started as `started` says.
fn antigravity_started(
    tree: &Tree,
    runs: usize,
    started: Started,
) -> Result<(Conversation, String), Failed> {
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
    let sandboxed = matches!(started, Started::Sandboxed | Started::Required);
    // A confined peer cannot count its starts in its read-only directory, so
    // it plays the one reply at every start.
    let script = if sandboxed {
        Peer::once(signed_out)
    } else {
        Peer::of(vec![signed_out; runs])
    };
    let program = fake_peer::install(&peer, "antigravity", &script)?;
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
        policy: SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(sandboxed),
        consent: consent.clone(),
        // Under the sandbox, crucible's home is the tree's own, which the
        // peer lies outside; it holds the whole tree otherwise.
        state: if sandboxed {
            catalog_fetch::state_in(tree.home()?.path())
        } else {
            tree.0.join("state")
        },
        sessions: tree.sessions(),
        agent,
        row,
        launched: Arc::default(),
    };
    let conversation = crate::conversation(tree, Script::new(Vec::new()), false)?
        .consenting(consent)
        .stepping(super::steps_confined(
            &launching,
            &peer,
            (sandboxed, started == Started::Required),
        ));
    let file = if sandboxed {
        json!({
            "sandbox": {"enabled": true},
            "externalAgents": {
                "space": {"driver": "antigravity", "executable": executable}
            }
        })
    } else {
        let profile = tree.home()?.path().join("agy");
        json!({
            "externalAgents": {
                "space": {
                    "driver": "antigravity",
                    "executable": executable,
                    "profileDir": profile.to_string_lossy(),
                }
            }
        })
    };
    Ok((conversation, serde_json::to_string_pretty(&file)?))
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
                ("Connect", "Move this conversation onto it."),
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
                ("Connect", "Move this conversation onto it."),
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
/// characters escaped, so it draws nothing of its own; a method run at a real
/// terminal is not offered, and one run as a command with no real terminal is;
/// and a method chosen ends the connect on a
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
                {"id": "console", "kind": "terminal", "name": "Sign in at the console",
                 "command": "goose", "args": ["login"], "env": {}, "route": "terminal"},
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
        [
            "Install",
            "Sign in\\u{1b}[2J here",
            "API key",
            "Sign in at a terminal",
            "Sign in at the console",
            "Go back"
        ]
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

/// Where the user's own programs lie on the `PATH` a test of them looks on.
fn tools(tree: &Tree) -> PathBuf {
    tree.0.join("tools")
}

/// The user's home a test of their own programs judges a directory against.
fn user(tree: &Tree) -> PathBuf {
    tree.0.join("user")
}

/// The user's own goose in `directory`, answering `initialize` as an agent
/// where `answers` holds and with nothing at all where it does not, as a
/// program a guessed name found that is no agent; and the path its links
/// lead to.
fn own_goose(directory: &std::path::Path, answers: bool) -> Result<PathBuf, Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let reply = if answers {
        Reply::new()
            .hear(1)
            .say(acp::offering("goose-acp", "1.2.3", &json!([])))
    } else {
        Reply::new().hear(1)
    };
    let program = fake_peer::install(directory, "goose", &Peer::once(reply))?;
    Ok(std::fs::canonicalize(program)?)
}

/// A conversation that took no agent run, whose steps start each agent under
/// `service` with the sandbox on, the user's home [`user`], the programs
/// looked for on `path`, and the catalog's trust state in `tree`'s home.
fn own_stepped(
    tree: &Tree,
    service: Arc<dyn SandboxService>,
    path: &std::path::Path,
) -> Result<Conversation, Failed> {
    use crucible_app::launching::{Launcher, Steps};
    use crucible_sandbox::SandboxEnablement;

    std::fs::create_dir_all(user(tree))?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(true);
    let consent = Consent::new(Routes::production());
    consent.keeps_in(crate::Desk::new(tree, &[])?.choosing);
    let state = catalog_fetch::state_in(tree.home()?.path());
    let sessions = tree.sessions();
    let launcher =
        Launcher::new(service, runtime()?, &policy, &consent, &state, &sessions).homed(user(tree));
    let steps = Steps::new(
        launcher,
        Arc::new(SandboxEnablement::new(true, false)),
        &REGISTRY,
        Some(path.as_os_str().to_owned()),
    );
    Ok(crate::conversation(tree, Script::new(Vec::new()), false)?
        .consenting(consent)
        .stepping(steps))
}

/// [`own_stepped`] under the sandbox this machine has, its programs looked
/// for in [`tools`].
fn own_sandboxed(tree: &Tree) -> Result<Conversation, Failed> {
    let sandbox: Arc<dyn SandboxService> = Arc::new(LocalSandbox::new().watching_on(runtime()?));
    own_stepped(tree, sandbox, &tools(tree))
}

/// What the agents door answers `action`, chosen in a connect of
/// `conversation`, asked by a client that claims every capability and answers
/// as `picking` does, lent that conversation's steps and no catalog it can
/// reach.
fn carried(
    tree: &Tree,
    conversation: &Conversation,
    action: &AgentAction,
    picking: &mut Picking,
) -> Result<Result<Door, AgentsError>, Failed> {
    at_door(tree, conversation, action.command(), picking)
}

/// What the agents door answers `command`, asked as [`carried`] asks a move.
fn at_door(
    tree: &Tree,
    conversation: &Conversation,
    command: Command,
    picking: &mut Picking,
) -> Result<Result<Door, AgentsError>, Failed> {
    use crucible_app::client::agents::{Watching, run};
    use crucible_client_api::{Correlation, Request};

    let home = tree.home()?;
    let shelf = lent(&home, conversation, None);
    let request = Request::new(Asking::every(), Correlation::new(2), command);
    let mut progress = |_| {};
    let cancel = crucible_runtime::Cancel::new();
    let link = |_: crucible_execution::Text| {};
    let watching = Watching {
        progress: &mut progress,
        cancel: &cancel,
        link: &link,
        sign_in: None,
        terminal: None,
    };
    Ok(runtime()?.block_on(run(&request, &shelf, picking, watching)))
}

/// The shelf the agents door is lent in `home`, with `conversation`'s steps,
/// no catalog it can reach, and the ACP Registry read from `registry`.
fn lent<'a>(
    home: &'a crucible_config::Home,
    conversation: &'a Conversation,
    registry: Option<catalog_fetch::RegistrySource>,
) -> crucible_app::client::agents::Shelf<'a> {
    crucible_app::client::agents::Shelf {
        catalog: catalog_fetch::Source {
            base: "http://127.0.0.1:1/catalog/".into(),
            keys: Keys::new(&[]),
            hosts: &[],
            proxy: crucible_http::ProxyEnv::default(),
        },
        registry,
        home,
        smoke: None,
        steps: conversation.steps(),
    }
}

/// A registry source nothing answers at, so the door reads the index cached
/// in its home, as a command line's door reads it within its five minutes.
fn cached_registry() -> catalog_fetch::RegistrySource {
    catalog_fetch::RegistrySource {
        url: "http://127.0.0.1:1/registry.json".into(),
        hosts: &[],
        proxy: crucible_http::ProxyEnv::default(),
    }
}

/// What the agents door lists in `tree` for `conversation`, the ACP
/// Registry read from the index cached there, as `crucible agents search`
/// asks it.
pub(crate) fn listed_with_the_registry(
    tree: &Tree,
    conversation: &Conversation,
) -> Result<Result<Door, AgentsError>, Failed> {
    use crucible_app::client::agents::{Watching, run};
    use crucible_client_api::{Correlation, Request};

    let home = tree.home()?;
    let shelf = lent(&home, conversation, Some(cached_registry()));
    let request = Request::new(Asking::every(), Correlation::new(2), Command::AgentCatalog);
    let mut progress = |_| {};
    let cancel = crucible_runtime::Cancel::new();
    let link = |_: crucible_execution::Text| {};
    let watching = Watching {
        progress: &mut progress,
        cancel: &cancel,
        link: &link,
        sign_in: None,
        terminal: None,
    };
    Ok(runtime()?.block_on(run(&request, &shelf, &mut Picking::of([]), watching)))
}

/// The user's own program of `agent` the agents door finds in `tree` for
/// `conversation`, the ACP Registry read from the index cached there, as
/// `crucible agents try` asks it.
pub(crate) fn found_at_door(
    tree: &Tree,
    conversation: &Conversation,
    agent: &str,
) -> Result<Result<OwnProgram, AgentsError>, Failed> {
    let home = tree.home()?;
    let shelf = lent(&home, conversation, Some(cached_registry()));
    Ok(runtime()?.block_on(crucible_app::client::agents::own_program(&shelf, agent)))
}

/// The ACP Registry agent `id`, released at 1.0.0 for this machine, whose
/// program is started as `./id acp`.
fn registered(id: &str) -> Value {
    json!({
        "id": id,
        "name": id,
        "version": "1.0.0",
        "license": "Apache-2.0",
        "license_url": "https://example.com/terms",
        "distribution": {"binary": {platform(): {
            "archive": "https://downloads.example.com/goose-1.0.0.tar.gz",
            "cmd": format!("./{id}"),
            "sha256": "a".repeat(64),
            "args": ["acp"],
        }}},
    })
}

/// Writes in `tree`'s home the ACP Registry index listing `agents`, fetched
/// now, as a read of it within its five minutes would find it.
fn registry_cached(tree: &Tree, agents: &[Value]) -> Result<(), Failed> {
    let path = catalog_fetch::registry_in(tree.home()?.path()).join("registry.json");
    std::fs::create_dir_all(path.parent().ok_or("the cache's directory")?)?;
    let index = json!({"version": "1.0.0", "agents": agents});
    std::fs::write(&path, serde_json::to_vec(&index)?)?;
    Ok(())
}

/// The move a connect of `conversation` naming no instance ends on, the user
/// file holding `file`, where the client picks `agent` and then "Try your
/// own"; and every question it was put.
pub(crate) fn chose_own(
    tree: &Tree,
    conversation: &mut Conversation,
    file: &str,
    agent: &'static str,
) -> Result<(AgentAction, Vec<Pending>), Failed> {
    let mut picking = Picking::of([Some(agent), Some("Try your own")]);
    let (outcome, after) = connecting_on(
        tree,
        conversation,
        (file, None),
        Asking::every(),
        &mut picking,
    )?;
    assert_eq!(after, file, "a choice wrote the user file");
    match outcome {
        Outcome::Login(LoginOutcome::AgentChosen { action }) => Ok((action, picking.put)),
        other => Err(format!("the connect ended on {other:?}, not a move").into()),
    }
}

/// The instance the user file in `tree`'s home names `name`, as written.
fn written(tree: &Tree, name: &str) -> Result<Option<Value>, Failed> {
    let file = std::fs::read_to_string(crate::Desk::new(tree, &[])?.choosing)?;
    let file: Value = serde_json::from_str(&file)?;
    Ok(file.pointer(&format!("/externalAgents/{name}")).cloned())
}

/// The probe record kept for the instance `name` of goose, started as its
/// entry here proposes, where one holds.
fn goose_record(tree: &Tree, name: &str) -> Result<Option<own_program::Record>, Failed> {
    let args: [Box<str>; 1] = ["acp".into()];
    Ok(own_program::read(
        tree.home()?.path(),
        &own_program::Instance {
            name,
            driver: "catalog",
            agent: Some("goose"),
            args: &args,
            env: &[],
        },
    ))
}

/// What the facts say of the directory `directory` an own program lies in,
/// where it is given to the program read-only.
fn readable(directory: &str) -> String {
    format!(
        "Everything in {directory} is readable to it, read-only, during its probe and at every \
         launch, where the network is open to its vendor."
    )
}

/// What the facts say on Linux of the home the program starts with.
const PRIVATE_HOME: &str = "Under the sandbox on Linux it starts with a private home of its own, \
                            and a sign-in made there is gone when that start ends.";

/// A catalog agent with no instance whose guessed name is on the client's
/// `PATH` is offered first as the user's own program, at the path its links
/// lead to, before the install; its facts name that path, the entry's key,
/// the arguments and each variable it is started with, escaped, and what it
/// can read; and choosing it ends the connect on that move, with nothing
/// started or written.
#[test]
fn a_catalog_agent_on_path_is_offered_as_the_users_own_before_install() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-offered")?;
    let mut goose = entry("goose", "1.0.0", ONE);
    if let Some(env) = goose.pointer_mut("/targets/0/env") {
        *env = json!({"GOOSE_MODE": "a\u{1b}b"});
    }
    trusting(&tree, &[goose])?;
    let program = own_goose(&tools(&tree), true)?;
    let mut conversation = own_sandboxed(&tree)?;

    let (action, put) = chose_own(&tree, &mut conversation, "{}", "goose")?;

    assert_eq!(
        action,
        AgentAction::TryOwn {
            agent: Name::new("goose")?,
            program: Text::cut(&program.to_string_lossy()),
        }
    );
    let mut expected = vec![
        "asks: What should Crucible do with goose?".to_owned(),
        "fact: Crucible's store holds no version of it.".to_owned(),
        "fact: No instance of it is configured, so there is nothing to sign in.".to_owned(),
        "fact: Program: <tree>/tools/goose.".to_owned(),
        "fact: Key: goose.".to_owned(),
        "fact: Arguments: acp.".to_owned(),
        "fact: Environment: GOOSE_MODE=a\\u{1b}b.".to_owned(),
        format!("fact: {}", readable("<tree>/tools")),
    ];
    if cfg!(target_os = "linux") {
        expected.push(format!("fact: {PRIVATE_HOME}"));
    }
    expected.extend([
        "choice: Try your own (recommended): Try your own goose at <tree>/tools/goose.".to_owned(),
        "choice: Install: Install goose 1.0.0.".to_owned(),
        "choice: Go back: Choose another agent.".to_owned(),
    ]);
    let shown = written_out(&put, &tree);
    assert_eq!(shown.get(1), Some(&portable(&expected.join("\n"))));
    assert_eq!(
        fake_peer::played(&tools(&tree)),
        0,
        "the program was started"
    );
    Ok(())
}

/// Choosing the user's own program probes it once, confined, after the door's
/// own yes, and writes an instance that runs it by the path its links lead
/// to, keeping what it answered for that instance.
#[test]
fn trying_your_own_probes_it_and_writes_the_instance() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-tried")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    let program = own_goose(&tools(&tree), true)?;
    let mut conversation = own_sandboxed(&tree)?;
    let (action, _) = chose_own(&tree, &mut conversation, "{}", "goose")?;
    let mut door = Picking::of([Some("Try it")]);

    let tried = carried(&tree, &conversation, &action, &mut door)?;

    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert_eq!(&*instance, "goose");
    let asks = question_asked(&door.put);
    assert_eq!(
        asks.as_deref().map(portable),
        Some(portable(&format!(
            "Try your own goose at {}?",
            program.display()
        )))
    );
    assert_eq!(
        written(&tree, "goose")?,
        Some(json!({
            "driver": "catalog",
            "agent": "goose",
            "executable": program.to_string_lossy(),
        }))
    );
    let record = goose_record(&tree, "goose")?.ok_or("no probe record was kept")?;
    assert_eq!((record.name(), record.version()), ("goose-acp", "1.2.3"));
    Ok(())
}

/// What the first question `put` asks, whatever kind it is.
fn question_asked(put: &[Pending]) -> Option<String> {
    put.iter().find_map(|put| match put {
        Pending::Install { asks, .. } | Pending::Account { asks, .. } => {
            Some(asks.as_str().to_owned())
        }
        _ => None,
    })
}

/// A guessed name that finds a program that is no agent is refused by its
/// probe, saying it did not answer as one, and nothing is written or kept.
#[test]
fn a_wrong_guess_is_refused_and_writes_nothing() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-wrong")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    own_goose(&tools(&tree), false)?;
    let mut conversation = own_sandboxed(&tree)?;
    let (action, _) = chose_own(&tree, &mut conversation, "{}", "goose")?;

    let tried = carried(
        &tree,
        &conversation,
        &action,
        &mut Picking::of([Some("Try it")]),
    )?;

    let Err(refused @ AgentsError::Own { .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert!(
        refused.to_string().contains("did not answer as an agent"),
        "{refused}"
    );
    assert!(
        refused.to_string().contains("nothing was written"),
        "{refused}"
    );
    assert_eq!(written(&tree, "goose")?, None);
    assert!(goose_record(&tree, "goose")?.is_none(), "a record was kept");
    Ok(())
}

/// Where the sandbox cannot close the network the user's own program is not
/// started: the refusal says it could not be checked, quotes why and points
/// at the sandboxing page, and nothing is written or kept.
#[test]
fn trying_your_own_where_the_sandbox_cannot_close_writes_nothing() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-unclosable")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    own_goose(&tools(&tree), true)?;
    let mut conversation = own_stepped(
        &tree,
        Arc::new(crate::agents::registry::Unclosing),
        &tools(&tree),
    )?;
    let (action, _) = chose_own(&tree, &mut conversation, "{}", "goose")?;

    let tried = carried(
        &tree,
        &conversation,
        &action,
        &mut Picking::of([Some("Try it")]),
    )?;

    let Err(refused @ AgentsError::Own { .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    let said = refused.to_string();
    assert!(said.contains("could not be checked"), "{said}");
    assert!(said.contains("docs/security/sandboxing.md"), "{said}");
    assert!(!said.contains("crucible agents install"), "{said}");
    assert_eq!(
        fake_peer::played(&tools(&tree)),
        0,
        "the program was started"
    );
    assert_eq!(written(&tree, "goose")?, None);
    assert!(goose_record(&tree, "goose")?.is_none(), "a record was kept");
    Ok(())
}

/// The choices and facts of the question about goose a connect of
/// `conversation` puts, the user file holding `file`, where the client picks
/// goose and then goes back and cancels.
fn goose_managed(
    tree: &Tree,
    conversation: &mut Conversation,
    file: &str,
) -> Result<(Vec<String>, Vec<String>), Failed> {
    let mut picking = Picking::of([Some("goose"), Some("Go back"), Some("Cancel")]);
    let (outcome, after) = connecting_on(
        tree,
        conversation,
        (file, None),
        Asking::every(),
        &mut picking,
    )?;
    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    assert_eq!(after, file, "going back wrote the user file");
    let (_, facts, choices) = question(&picking.put, 1).ok_or("the agent's question")?;
    Ok((
        facts.into_iter().map(portable).collect(),
        choices
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect(),
    ))
}

/// An instance that runs the user's own program, once a probe of it as it is
/// configured is kept, is offered Connect, and no longer the try; its facts
/// name the program it runs, and its note says that crucible did not review
/// it and what its admission proves.
#[test]
fn an_instance_running_your_own_program_is_offered_connect() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-connect")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    let program = own_goose(&tools(&tree), true)?;
    let mut conversation = own_sandboxed(&tree)?;
    let (action, _) = chose_own(&tree, &mut conversation, "{}", "goose")?;
    carried(
        &tree,
        &conversation,
        &action,
        &mut Picking::of([Some("Try it")]),
    )?
    .map_err(|why| why.to_string())?;
    let file = std::fs::read_to_string(crate::Desk::new(&tree, &[])?.choosing)?;

    let (facts, choices) = goose_managed(&tree, &mut conversation, &file)?;

    assert!(choices.iter().any(|name| name == "Connect"), "{choices:?}");
    assert!(
        !choices.iter().any(|name| name == "Try your own"),
        "{choices:?}"
    );
    let shown = portable(&program.to_string_lossy());
    assert_eq!(facts.first(), Some(&format!("Program: {shown}.")));
    assert!(
        facts.contains(&portable(
            "Runs your own program, which Crucible did not review: its admission proves it \
             speaks ACP, not which program it is."
        )),
        "{facts:?}"
    );
    Ok(())
}

/// A program in a directory no agent is given is still offered, saying why
/// its directory is refused; choosing it runs nothing, asks nothing more and
/// writes nothing.
#[test]
fn a_refused_root_is_listed_with_its_reason_and_runs_nothing() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-refused")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    std::fs::create_dir_all(user(&tree))?;
    let program = own_goose(&user(&tree), true)?;
    let sandbox: Arc<dyn SandboxService> = Arc::new(LocalSandbox::new().watching_on(runtime()?));
    let mut conversation = own_stepped(&tree, sandbox, &user(&tree))?;
    let home = tree.home()?;
    let own_program::Root::Refused(why) = own_program::root(
        &program,
        &std::fs::canonicalize(user(&tree))?,
        home.path(),
        &tree.sessions(),
    ) else {
        return Err("the user's home was given to an agent".into());
    };

    let (action, put) = chose_own(&tree, &mut conversation, "{}", "goose")?;

    let (_, facts, _) = question(&put, 1).ok_or("the agent's question")?;
    assert!(facts.contains(&why.as_str()), "{facts:?}");
    let mut door = Picking::of([Some("Try it")]);
    let tried = carried(&tree, &conversation, &action, &mut door)?;
    let Err(refused @ AgentsError::Own { .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert!(refused.to_string().contains(&why), "{refused}");
    assert!(door.put.is_empty(), "the door asked: {:?}", door.put);
    assert_eq!(
        fake_peer::played(&user(&tree)),
        0,
        "the program was started"
    );
    assert_eq!(written(&tree, "goose")?, None);
    assert!(goose_record(&tree, "goose")?.is_none(), "a record was kept");
    Ok(())
}

/// The facts say everything in the directory the program lies in is
/// readable to it, read-only, at its probe and at every launch, and never
/// that the directory holds no credential.
#[test]
fn the_facts_say_the_programs_directory_is_readable() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-readable")?;
    trusting(&tree, &[entry("goose", "1.0.0", ONE)])?;
    let program = own_goose(&tools(&tree), true)?;
    let mut conversation = own_sandboxed(&tree)?;

    let (_, put) = chose_own(&tree, &mut conversation, "{}", "goose")?;

    let (_, facts, _) = question(&put, 1).ok_or("the agent's question")?;
    let directory = program.parent().ok_or("the program's directory")?;
    let said = readable(&directory.display().to_string());
    assert!(facts.contains(&said.as_str()), "{facts:?}");
    assert!(
        !facts.iter().any(|fact| fact.contains("credential")),
        "{facts:?}"
    );
    assert_eq!(
        facts.contains(&PRIVATE_HOME),
        cfg!(target_os = "linux"),
        "{facts:?}"
    );
    Ok(())
}

/// The user's own `agy` on `PATH`, with no Antigravity instance, is offered
/// as the catalog agent's own program is, its facts carrying what it is told
/// of its sign-in under the sandbox; trying it writes an Antigravity instance
/// that runs it by the path its links lead to, with no profile directory.
#[test]
fn trying_your_own_agy_writes_an_antigravity_instance_with_no_profile() -> Result<(), Failed> {
    use crucible_external_harnesses::antigravity;

    let tree = Tree::new("external-choosing-own-agy")?;
    let (mut conversation, program) = agy_of_your_own(&tree)?;

    let (action, put) = chose_own(&tree, &mut conversation, "{}", "Antigravity")?;

    assert_eq!(
        action,
        AgentAction::TryOwn {
            agent: Name::new("antigravity")?,
            program: Text::cut(&program.to_string_lossy()),
        }
    );
    let (_, facts, _) = question(&put, 1).ok_or("the agent's question")?;
    assert!(facts.contains(&antigravity::own_note(true)), "{facts:?}");
    let tried = carried(
        &tree,
        &conversation,
        &action,
        &mut Picking::of([Some("Try it")]),
    )?;
    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert_eq!(
        written(&tree, &instance)?,
        Some(json!({
            "driver": "antigravity",
            "executable": program.to_string_lossy(),
        }))
    );
    Ok(())
}

/// Where the sandbox cannot close the network the user's own `agy` is not
/// started: the refusal says it could not be checked, quotes why, points at
/// the sandboxing page and at the release Crucible's store holds, and nothing
/// is written.
#[test]
fn trying_your_own_agy_where_the_sandbox_cannot_close_points_at_the_store() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-agy-unclosable")?;
    own_agy(&tree)?;
    let mut conversation = own_stepped(
        &tree,
        Arc::new(crate::agents::registry::Unclosing),
        &tools(&tree),
    )?;
    let (action, _) = chose_own(&tree, &mut conversation, "{}", "Antigravity")?;
    let before = std::fs::read_to_string(crate::Desk::new(&tree, &[])?.choosing).ok();

    let tried = carried(
        &tree,
        &conversation,
        &action,
        &mut Picking::of([Some("Try it")]),
    )?;

    let Err(refused @ AgentsError::Own { .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    let said = refused.to_string();
    let why = crucible_sandbox::SandboxError::Unsupported {
        feature: crucible_sandbox::SandboxFeature::NetworkDeny,
    }
    .to_string();
    assert!(said.contains("could not be checked"), "{said}");
    assert!(said.contains(&format!("({why})")), "{said}");
    assert!(said.contains("docs/security/sandboxing.md"), "{said}");
    assert!(
        said.ends_with(
            "Crucible's own release of Antigravity works on this machine: run `crucible agents \
             install antigravity`."
        ),
        "{said}"
    );
    assert_eq!(
        fake_peer::played(&tools(&tree)),
        0,
        "the program was started"
    );
    assert_eq!(
        std::fs::read_to_string(crate::Desk::new(&tree, &[])?.choosing).ok(),
        before,
        "the user file was written"
    );
    Ok(())
}

/// Every fact the account questions `put` state, in the order put.
pub(crate) fn every_fact(put: &[Pending]) -> Vec<String> {
    (0..)
        .map_while(|at| question(put, at))
        .flat_map(|(_, facts, _)| facts.into_iter().map(str::to_owned))
        .collect()
}

/// Every choice's name among the account questions `put`.
pub(crate) fn every_choice(put: &[Pending]) -> Vec<String> {
    (0..)
        .map_while(|at| question(put, at))
        .flat_map(|(_, _, choices)| choices.into_iter().map(|(name, _)| name.to_owned()))
        .collect()
}

/// With an instance of the agent configured that runs another program, the
/// user's own program of it on `PATH` is offered neither by `/login`, whose
/// every question `put` holds, nor by the door's listing, and a try of it at
/// `program`, as `agent`, is refused, naming the instance `first`
/// configured: it starts nothing and the user file stays as `file`.
fn beside_is_refused(
    tree: &Tree,
    conversation: &Conversation,
    (put, file): (&[Pending], &str),
    (agent, program, first): (&str, &std::path::Path, &str),
) -> Result<(), Failed> {
    let choices = every_choice(put);
    assert!(
        !choices.iter().any(|name| name == "Try your own"),
        "{choices:?}"
    );
    let listed = at_door(
        tree,
        conversation,
        Command::AgentCatalog,
        &mut Picking::of([Some("Try it")]),
    )?;
    let Ok(Door::Catalog {
        agents,
        antigravity,
        ..
    }) = listed
    else {
        return Err(format!("the listing answered {listed:?}").into());
    };
    assert_eq!(antigravity, None, "the listing offered the own Antigravity");
    let own: Vec<_> = agents
        .iter()
        .filter_map(|listed| listed.own.as_ref())
        .collect();
    assert!(own.is_empty(), "the listing offered {own:?}");
    let mut door = Picking::of([Some("Try it")]);
    let tried = at_door(
        tree,
        conversation,
        Command::TryOwnAgent {
            agent: Name::new(agent)?,
            program: Text::cut(&program.to_string_lossy()),
        },
        &mut door,
    )?;
    let Err(refused @ AgentsError::Own { .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert!(
        refused.to_string().contains(&format!(
            "the instance {first} configured for it runs another program"
        )),
        "{refused}"
    );
    // What `crucible agents try` asks the door for, and the refusal it says
    // for what it is told: the door's own.
    let found = found_at_door(tree, conversation, agent)?;
    assert_eq!(
        found.as_ref().ok(),
        Some(&OwnProgram::Beside {
            program: program.to_owned(),
            instance: first.into(),
        }),
        "{found:?}"
    );
    assert_eq!(
        refused_beside(AgentKey::new(agent)?, program.to_owned(), first).to_string(),
        refused.to_string()
    );
    assert!(door.put.is_empty(), "the door asked: {:?}", door.put);
    assert_eq!(
        fake_peer::played(&tools(tree)),
        0,
        "the program was started"
    );
    assert_eq!(
        std::fs::read_to_string(crate::Desk::new(tree, &[])?.choosing)?,
        file,
        "the user file was written"
    );
    Ok(())
}

/// The user file configuring the instances `names` of the driver `driver`,
/// each run as a program that is not the user's own one on `PATH`, and of
/// `agent` where it is a catalog agent's.
fn beside(
    tree: &Tree,
    names: &[&str],
    (driver, agent): (&str, Option<&str>),
) -> Result<String, Failed> {
    let elsewhere = tree.0.join("elsewhere").join(driver);
    let mut instances = serde_json::Map::new();
    for name in names {
        let mut instance = json!({"driver": driver, "executable": elsewhere.to_string_lossy()});
        if let (Some(agent), Some(fields)) = (agent, instance.as_object_mut()) {
            fields.insert("agent".to_owned(), json!(agent));
        }
        instances.insert((*name).to_owned(), instance);
    }
    Ok(serde_json::to_string_pretty(
        &json!({"externalAgents": instances}),
    )?)
}

/// `/login` offers the user's own `agy` only where no Antigravity instance
/// is configured, and the door neither lists nor writes it beside one that
/// runs another program.
#[test]
fn an_own_agy_beside_a_configured_antigravity_is_neither_offered_nor_written() -> Result<(), Failed>
{
    let tree = Tree::new("external-choosing-own-agy-beside")?;
    let (mut conversation, program) = agy_of_your_own(&tree)?;
    // Two, so `/login` asks which rather than connecting the one.
    let file = beside(&tree, &["play", "work"], ("antigravity", None))?;
    let mut picking = Picking::of([Some("Antigravity"), Some("Go back"), Some("Cancel")]);

    let (outcome, after) = connecting_on(
        &tree,
        &mut conversation,
        (&file, None),
        Asking::every(),
        &mut picking,
    )?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    assert_eq!(after, file, "going back wrote the user file");
    beside_is_refused(
        &tree,
        &conversation,
        (&picking.put, &file),
        ("antigravity", &program, "play"),
    )
}

/// An Antigravity instance configured that runs the user's own `agy` keeps it
/// listed, and a try of it probes it as that instance and writes no second.
#[test]
fn an_own_agy_a_configured_antigravity_runs_is_tried_as_that_instance() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-agy-configured")?;
    let (conversation, program) = agy_of_your_own(&tree)?;
    let file = serde_json::to_string_pretty(&json!({
        "externalAgents": {
            "work": {"driver": "antigravity", "executable": program.to_string_lossy()}
        }
    }))?;
    let user = crate::Desk::new(&tree, &[])?.choosing;
    std::fs::create_dir_all(user.parent().ok_or("the user file's directory")?)?;
    std::fs::write(&user, &file)?;

    let listed = tried_at_door(&tree, &conversation, Command::AgentCatalog)?;
    let tried = tried_at_door(
        &tree,
        &conversation,
        Command::TryOwnAgent {
            agent: Name::new("antigravity")?,
            program: Text::cut(&program.to_string_lossy()),
        },
    )?;

    let Ok(Door::Catalog { antigravity, .. }) = listed else {
        return Err(format!("the listing answered {listed:?}").into());
    };
    assert_eq!(antigravity.map(|own| own.program), Some(program));
    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    assert_eq!(&*instance, "work");
    assert_eq!(
        std::fs::read_to_string(crate::Desk::new(&tree, &[])?.choosing)?,
        file,
        "the user file was written"
    );
    Ok(())
}

/// `/login` offers the user's own program of a catalog agent with an
/// instance configured only as that instance runs it, and the door neither
/// lists nor writes it beside one that runs another program.
#[test]
fn an_own_goose_beside_a_configured_goose_is_neither_offered_nor_written() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-own-goose-beside")?;
    let (mut conversation, program) = goose_of_your_own(&tree)?;
    let file = beside(&tree, &["work"], ("catalog", Some("goose")))?;
    let mut picking = Picking::of([Some("goose"), Some("Go back"), Some("Cancel")]);

    let (outcome, after) = connecting_on(
        &tree,
        &mut conversation,
        (&file, None),
        Asking::every(),
        &mut picking,
    )?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    assert_eq!(after, file, "going back wrote the user file");
    beside_is_refused(
        &tree,
        &conversation,
        (&picking.put, &file),
        ("goose", &program, "work"),
    )
}

/// What the agents door answers `command`, asked as [`carried`] asks a move
/// by a client that says yes to the door's question about a try.
pub(crate) fn tried_at_door(
    tree: &Tree,
    conversation: &Conversation,
    command: Command,
) -> Result<Result<Door, AgentsError>, Failed> {
    at_door(
        tree,
        conversation,
        command,
        &mut Picking::of([Some("Try it")]),
    )
}

/// A tree whose catalog offers goose and whose user installed their own goose
/// on the `PATH` a conversation's steps look on, which answers as an agent;
/// that conversation, and the path the program's links lead to.
pub(crate) fn goose_of_your_own(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    trusting(tree, &[entry("goose", "1.0.0", ONE)])?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// A tree whose catalog offers nothing but whose cached ACP Registry index
/// lists goose, and whose user installed their own goose on the `PATH` a
/// conversation's steps look on, which answers as an agent; that
/// conversation, and the path the program's links lead to.
pub(crate) fn goose_of_the_registry(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    registry_cached(tree, &[registered("goose")])?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// As [`goose_of_the_registry`], but the index lists, before goose, an agent
/// whose id is a built-in driver's word, which no listing shows, and as many
/// other agents as a listing shows, so goose is past what it shows.
pub(crate) fn goose_past_the_listing(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    use crucible_app::client::agents::LISTED;

    let mut agents = vec![registered("claude")];
    agents.extend((0..LISTED).map(|at| registered(&format!("listed-{at:02}"))));
    agents.push(registered("goose"));
    registry_cached(tree, &agents)?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// A tree whose signed catalog offers one more agent than a listing shows,
/// `signed-00` on, and whose ACP Registry index is cached with none; a
/// conversation there.
pub(crate) fn signed_past_the_listing(tree: &Tree) -> Result<Conversation, Failed> {
    use crucible_app::client::agents::LISTED;

    let entries: Vec<Value> = (0..=LISTED)
        .map(|at| entry(&format!("signed-{at:02}"), "1.0.0", ONE))
        .collect();
    trusting(tree, &entries)?;
    registry_cached(tree, &[])?;
    own_sandboxed(tree)
}

/// A tree whose user installed their own Antigravity on the `PATH` a
/// conversation's steps look on, which answers as the reviewed release does;
/// that conversation, and the path the program's links lead to.
pub(crate) fn agy_of_your_own(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    let program = own_agy(tree)?;
    Ok((own_sandboxed(tree)?, program))
}

/// The user's own Antigravity installed on the `PATH` a conversation's steps
/// look on in `tree`, answering as the reviewed release does: the path its
/// links lead to.
fn own_agy(tree: &Tree) -> Result<PathBuf, Failed> {
    use crucible_external_harnesses::acp::SignIn;
    use crucible_external_harnesses::antigravity::Manifest;
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let shipped = SNAPSHOT.entries().first().ok_or("the snapshot's entry")?;
    let reviewed = acp::initialized(
        &shipped.admits.name,
        &shipped.admits.version,
        Manifest::REVIEWED.sign_in.first().map_or("", SignIn::id),
    );
    let program = fake_peer::install(
        &tools(tree),
        "agy",
        &Peer::once(Reply::new().hear(1).say(reviewed.as_str())),
    )?;
    Ok(std::fs::canonicalize(program)?)
}

/// The instance `/login` writes in `tree`, set up by `of_your_own`, where the
/// client picks the agent shown as `agent`, then "Try your own", and says yes
/// to the door's question: as written, with the directory the user's own
/// programs lie in written `<tools>`.
pub(crate) fn tried_through_login(
    tree: &Tree,
    of_your_own: fn(&Tree) -> Result<(Conversation, PathBuf), Failed>,
    agent: &'static str,
) -> Result<String, Failed> {
    let (mut conversation, _) = of_your_own(tree)?;
    let (action, _) = chose_own(tree, &mut conversation, "{}", agent)?;
    let tried = tried_at_door(tree, &conversation, action.command())?;
    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };
    instance_written(tree, &instance)
}

/// The instance the user file in `tree`'s home names `name`, as written, with
/// the directory the user's own programs lie in written `<tools>`.
pub(crate) fn instance_written(tree: &Tree, name: &str) -> Result<String, Failed> {
    let instance = written(tree, name)?.ok_or("no instance was written")?;
    let tools = std::fs::canonicalize(tools(tree))?;
    Ok(instance
        .to_string()
        .replace(&tools.to_string_lossy().replace('\\', "\\\\"), "<tools>"))
}

/// The ACP Registry agent `id`, released at 1.0.0 for this machine with no
/// SHA-256, whose program is started as `./cmd acp`: an entry crucible never
/// installs.
pub(crate) fn unhashed(id: &str, cmd: &str) -> Value {
    json!({
        "id": id,
        "name": id,
        "version": "1.0.0",
        "license": "Apache-2.0",
        "distribution": {"binary": {platform(): {
            "archive": "https://downloads.example.com/goose-1.0.0.tar.gz",
            "cmd": format!("./{cmd}"),
            "args": ["acp"],
        }}},
    })
}

/// A tree whose catalog offers nothing and whose cached ACP Registry index
/// lists goose with no SHA-256 for this machine, and whose user installed
/// their own goose on the `PATH` a conversation's steps look on, which
/// answers as an agent; that conversation, and the path the program's links
/// lead to.
pub(crate) fn goose_unhashed(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    registry_cached(tree, &[unhashed("goose", "goose")])?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// As [`goose_unhashed`], but with no program of goose on that `PATH`.
pub(crate) fn unhashed_with_nothing_on_path(tree: &Tree) -> Result<Conversation, Failed> {
    registry_cached(tree, &[unhashed("goose", "goose")])?;
    own_sandboxed(tree)
}

/// As [`goose_unhashed`], but the index lists a second agent, `goose-next`,
/// also with no SHA-256, whose command guesses the same name.
pub(crate) fn two_guessing_goose(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    registry_cached(
        tree,
        &[unhashed("goose", "goose"), unhashed("goose-next", "goose")],
    )?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// As [`goose_unhashed`], but the index lists `count` agents, `own-0` on,
/// each with no SHA-256 and each guessing goose's name, and no goose itself.
pub(crate) fn many_unhashed(tree: &Tree, count: usize) -> Result<Conversation, Failed> {
    let entries: Vec<Value> = (0..count)
        .map(|at| unhashed(&format!("own-{at}"), "goose"))
        .collect();
    registry_cached(tree, &entries)?;
    own_goose(&tools(tree), true)?;
    own_sandboxed(tree)
}

/// The ACP Registry agent `id`, released at 1.0.0 with no binary for any
/// platform, published only as `package` for the package runner `runner` and
/// started with `args`: an entry crucible never installs, whose package it
/// never runs.
pub(crate) fn packaged(id: &str, runner: &str, package: &str, args: &[&str]) -> Value {
    json!({
        "id": id,
        "name": id,
        "version": "1.0.0",
        "license": "Apache-2.0",
        "distribution": {runner: {"package": package, "args": args}},
    })
}

/// A tree whose catalog offers nothing and whose cached ACP Registry index
/// lists goose published only as the npx package `@example/goose@1.0.0`,
/// started with `acp`, and whose user installed their own goose on the `PATH`
/// a conversation's steps look on, which answers as an agent; that
/// conversation, and the path the program's links lead to.
pub(crate) fn goose_packaged(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    registry_cached(
        tree,
        &[packaged("goose", "npx", "@example/goose@1.0.0", &["acp"])],
    )?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// As [`goose_packaged`], but with no program of goose on that `PATH`.
pub(crate) fn packaged_with_nothing_on_path(tree: &Tree) -> Result<Conversation, Failed> {
    registry_cached(
        tree,
        &[packaged("goose", "npx", "@example/goose@1.0.0", &["acp"])],
    )?;
    own_sandboxed(tree)
}

/// As [`goose_packaged`], but the index lists a second agent, `goose-next`,
/// published only as the uvx package `goose==2.0.0` and started with
/// `serve --stdio`, whose package guesses the same name.
pub(crate) fn two_packaged_guessing_goose(tree: &Tree) -> Result<(Conversation, PathBuf), Failed> {
    registry_cached(
        tree,
        &[
            packaged("goose", "npx", "@example/goose@1.0.0", &["acp"]),
            packaged("goose-next", "uvx", "goose==2.0.0", &["serve", "--stdio"]),
        ],
    )?;
    let program = own_goose(&tools(tree), true)?;
    Ok((own_sandboxed(tree)?, program))
}

/// The name of each choice the first question of a connect of `conversation`
/// naming no instance offers, asked of a client that claims every capability
/// and declines it.
pub(crate) fn first_choices(
    tree: &Tree,
    conversation: &mut Conversation,
) -> Result<Vec<String>, Failed> {
    let mut picking = Picking::of([None]);
    connecting_on(
        tree,
        conversation,
        ("{}", None),
        Asking::every(),
        &mut picking,
    )?;
    Ok(every_choice(&picking.put))
}

/// `crucible agents try goose`, for a registry entry with no SHA-256 whose
/// guessed name is on `PATH`, finds that program and writes the instance
/// `/login`'s "Try your own" writes for it.
#[test]
fn a_command_line_try_of_an_unhashed_registry_key_writes_what_login_writes() -> Result<(), Failed> {
    let chosen = tried_through_login(
        &Tree::new("external-choosing-unhashed-login")?,
        goose_unhashed,
        "goose",
    )?;

    let tree = Tree::new("external-choosing-unhashed-command")?;
    let (conversation, program) = goose_unhashed(&tree)?;
    let found = found_at_door(&tree, &conversation, "goose")?;
    assert_eq!(found.ok(), Some(OwnProgram::Found(program.clone())));
    let tried = tried_at_door(
        &tree,
        &conversation,
        Command::TryOwnAgent {
            agent: Name::new("goose")?,
            program: Text::cut(&program.to_string_lossy()),
        },
    )?;
    let Ok(Door::Tried { instance, .. }) = tried else {
        return Err(format!("the door answered {tried:?}").into());
    };

    assert_eq!(instance_written(&tree, &instance)?, chosen);
    Ok(())
}

/// `crucible agents try goose`, for a registry entry with no SHA-256 whose
/// guessed name is on no `PATH`, finds no program of the user's own rather
/// than no agent listed, and writes nothing.
#[test]
fn a_command_line_try_of_an_unhashed_key_with_nothing_on_path_says_path() -> Result<(), Failed> {
    let tree = Tree::new("external-choosing-unhashed-unfound")?;
    let conversation = unhashed_with_nothing_on_path(&tree)?;

    let found = found_at_door(&tree, &conversation, "goose")?;

    assert_eq!(found.ok(), Some(OwnProgram::Unfound));
    let file = std::fs::read_to_string(crate::Desk::new(&tree, &[])?.choosing).unwrap_or_default();
    assert!(!file.contains("goose"), "an instance was written: {file}");
    Ok(())
}

/// A connect naming no instance does not list an ACP Registry entry with no
/// SHA-256 when the user file can no longer be read, though its guessed name
/// is on `PATH`: which program the user configured for it is unknown, so it
/// is not looked for under that name, as the agents door then lists nothing.
#[test]
fn an_unhashed_entry_is_not_listed_by_login_once_the_user_file_cannot_be_read() -> Result<(), Failed>
{
    let tree = Tree::new("external-choosing-unhashed-unreadable")?;
    let (mut conversation, _program) = goose_unhashed(&tree)?;
    let mut picking = Picking::of([None]);
    let asked = Asked {
        installed: ("{}", "work"),
        command: &|_| Command::Connect { instance: None },
        asking: Asking::every(),
        after: Some("{ not json"),
    };

    accounting_on(&tree, &mut conversation, &asked, &mut picking)?;

    let choices = every_choice(&picking.put);
    assert!(!choices.is_empty(), "nothing was asked: {:?}", picking.put);
    assert!(
        choices.iter().all(|choice| choice != "goose"),
        "{choices:?}"
    );
    Ok(())
}
