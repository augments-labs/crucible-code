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
use crucible_app::client::agents::{AgentsError, Outcome as Door};
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
    use crucible_app::client::agents::{Shelf, Watching, run};
    use crucible_client_api::{Correlation, Request};

    let home = tree.home()?;
    let shelf = Shelf {
        catalog: catalog_fetch::Source {
            base: "http://127.0.0.1:1/catalog/".into(),
            keys: Keys::new(&[]),
            hosts: &[],
            proxy: crucible_http::ProxyEnv::default(),
        },
        registry: None,
        home: &home,
        smoke: None,
        steps: conversation.steps(),
    };
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

/// The move a connect of `conversation` naming no instance ends on, the user
/// file holding `file`, where the client picks `agent` and then "Try your
/// own"; and every question it was put.
fn chose_own(
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
