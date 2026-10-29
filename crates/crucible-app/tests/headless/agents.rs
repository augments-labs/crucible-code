//! The agents door, driven with no terminal and no conversation: nothing is
//! fetched before the answer, a no changes nothing, a download goes only where
//! it is listed and a cancelled one commits nothing, and an update and a
//! rollback move the instances' `executable` between versions. A version
//! the catalog revoked is refused at launch with its reason, an agent started
//! from the store cannot write into its version, and a switch is smoke-tested
//! only where a launcher's sandbox is on with the network closed and the
//! agent's route has its yes, keeping every instance where it was when the
//! smoke fails.
//!
//! The cases that start the fake peer in the sandbox pass over nothing on a
//! machine whose sandbox cannot enforce, and fail there on a job that sets
//! `CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX`.
//!
//! Each release here is a program published bare and built at review in its
//! vendor's place, so it is downloaded from the catalog's own host: the fake
//! host on this machine that the door's [`Shelf`] names.

use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::io::{Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crucible_app::agent_download::DownloadError;
use crucible_app::backends::{AgentRow, CatalogRow, REGISTRY, Registry};
use crucible_app::catalog_fetch::{self, Source};
use crucible_app::client::agents::{
    AgentsError, How, Outcome, Shelf, Signed, Typed, Watching, keep_key, run,
};
use crucible_app::client::{Front, Shown};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::{Agent, Launcher, Runs, Steps, Unlaunched};
use crucible_app::outside::Relaunch as _;
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, ErrorCode, InstallProgress, Name,
    Pending, PendingId, Refusal, Request, Said,
};
use crucible_config::{Driver, ExternalAgent};
use crucible_execution::Backend;
use crucible_external_harnesses::catalog::fixture::signed;
use crucible_external_harnesses::catalog::{Callback, Catalog, Held, SNAPSHOT, Uncalled, Unlinked};
use crucible_external_harnesses::fake_peer::{self, Reply as Played, Script as Peer, scripts::acp};
use crucible_external_harnesses::{Account, Control, Reading};
use crucible_http::ProxyEnv;
use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCapability, SandboxCommand,
    SandboxEnablement, SandboxError, SandboxFeature, SandboxFilesystemAccess, SandboxInput,
    SandboxInspection, SandboxLaunch, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy,
    SandboxProcess, SandboxRequest, SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::Mode;
use crucible_types::AgentKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{Failed, Tree};

/// What the fake host answers a path with.
#[derive(Clone)]
enum Reply {
    /// A 200 with these bytes.
    Body(Vec<u8>),
    /// A 302 to this address.
    Redirect(String),
    /// A 200 promising twice these bytes, of which it sends these, then holds
    /// the connection open.
    Half(Vec<u8>),
}

/// A host on this machine standing where the catalog is published.
struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
    asked: Arc<Mutex<Vec<String>>>,
    done: Arc<AtomicBool>,
}

impl Server {
    fn start() -> Result<Self, Failed> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let (routing, remembering, finishing) =
            (Arc::clone(&routes), Arc::clone(&asked), Arc::clone(&done));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if finishing.load(Ordering::Acquire) {
                    break;
                }
                let (routes, asked, done) = (
                    Arc::clone(&routing),
                    Arc::clone(&remembering),
                    Arc::clone(&finishing),
                );
                std::thread::spawn(move || answer(stream, &routes, &asked, &done));
            }
        });
        Ok(Self {
            port,
            routes,
            asked,
            done,
        })
    }

    fn serve(&self, path: &str, reply: Reply) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.insert(path.to_owned(), reply);
        }
    }

    fn at(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .map(|asked| asked.clone())
            .unwrap_or_default()
    }

    fn forget(&self) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.clear();
        }
    }

    /// The catalog on this host, signed with the test key, whose redirects
    /// reach this host alone.
    fn source(&self) -> Source {
        let listed: &'static str = Box::leak(format!("127.0.0.1:{}", self.port).into_boxed_str());
        Source {
            base: self.at("/catalog/"),
            keys: signed(b"{}").keys,
            hosts: Box::leak(Box::new([listed])),
            proxy: ProxyEnv::default(),
        }
    }

    /// Publishes a catalog of `goose` at each of `releases`, newest first,
    /// each a program built at review and kept on this host by its digest.
    fn publish(&self, serial: u64, releases: &[(&str, &[u8])], also: &[Value]) {
        let mut entries: Vec<Value> = releases
            .iter()
            .map(|(version, program)| {
                self.serve(&asset_path(program), Reply::Body(program.to_vec()));
                goose(version, program)
            })
            .collect();
        entries.extend(also.iter().cloned());
        let catalog = json!({
            "schema": 1,
            "serial": SNAPSHOT.serial() + serial,
            "entries": entries,
            "revoked": [],
        });
        let signed = signed(&serde_json::to_vec(&catalog).unwrap_or_default());
        self.serve("/catalog/index.json", Reply::Body(signed.index));
        self.serve("/catalog/index.json.minisig", Reply::Body(signed.signature));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn answer(
    mut stream: TcpStream,
    routes: &Mutex<HashMap<String, Reply>>,
    asked: &Mutex<Vec<String>>,
    done: &AtomicBool,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !head.windows(4).any(|end| end == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => head.extend_from_slice(buffer.get(..read).unwrap_or_default()),
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default()
        .to_owned();
    if let Ok(mut asked) = asked.lock() {
        asked.push(path.clone());
    }
    let reply = routes
        .lock()
        .ok()
        .and_then(|routes| routes.get(&path).cloned());
    let (status, extra, body, promised) = match reply {
        None => ("404 Not Found", String::new(), Vec::new(), 0),
        Some(Reply::Body(body)) => {
            let promised = body.len();
            ("200 OK", String::new(), body, promised)
        }
        Some(Reply::Redirect(to)) => ("302 Found", format!("location: {to}\r\n"), Vec::new(), 0),
        Some(Reply::Half(body)) => {
            let promised = body.len() * 2;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {promised}\r\n\r\n"
            );
            let _ = stream.write_all(&body);
            let _ = stream.flush();
            while !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(20));
            }
            return;
        }
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nconnection: close\r\n{extra}content-length: {promised}\r\n\r\n"
    );
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
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

/// Where the catalog keeps the asset `program` on its host.
fn asset_path(program: &[u8]) -> String {
    format!("/catalog/{}", digest(program))
}

/// How this machine's platform is spelled in a catalog.
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// `goose` at `version`, whose program `program` was built at review.
fn goose(version: &str, program: &[u8]) -> Value {
    entry("goose", version, program)
}

fn entry(key: &str, version: &str, program: &[u8]) -> Value {
    json!({
        "key": key,
        "registry_id": format!("{key}-acp"),
        "version": version,
        "kind": "binary",
        "license": "Apache-2.0",
        "terms": "https://example.com/terms",
        "caution": "Read the terms first.",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/12",
        "admits": {"name": key, "version": version},
        "targets": [{
            "platform": platform(),
            "cmd": "goose",
            "args": ["acp"],
            "env": {"GOOSE_NO_UPDATE": "1"},
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

/// A client that answers every install confirmation by picking `choice`, or
/// by declining where there is none, and remembers each one it was put and
/// what the host had been asked when it was.
struct Answering<'a> {
    choice: Option<&'static str>,
    server: &'a Server,
    put: Vec<(Pending, Vec<String>)>,
    /// A file written, and what with, while the question is out, as a person
    /// editing their configuration meanwhile would.
    meanwhile: Option<(PathBuf, Vec<u8>)>,
}

impl Front for Answering<'_> {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            if !matches!(shown, Shown::Install(_)) {
                return None;
            }
            self.put.push((pending.clone(), self.server.asked()));
            if let Some((file, bytes)) = &self.meanwhile {
                fs::write(file, bytes).ok()?;
            }
            let id = pending.id();
            Some(match self.choice {
                // Composed for an action other than the one put.
                Some(choice) if choice.starts_with("stale ") => Decision::Picked {
                    id: PendingId::new(id.number().wrapping_add(1)),
                    choice: Said::new(choice.trim_start_matches("stale ")).ok()?,
                },
                Some(choice) => Decision::Picked {
                    id,
                    choice: Said::new(choice).ok()?,
                },
                None => Decision::Declined { id },
            })
        })
    }

    fn refused(&mut self, _refusal: Refusal) {}
}

fn request(command: Command) -> Request {
    Request::new(
        Capabilities::none().with(Capability::Install),
        Correlation::new(1),
        command,
    )
}

fn name(word: &str) -> Result<Name, Failed> {
    Ok(Name::new(word).map_err(|refusal| refusal.to_string())?)
}

fn on_runtime<F: Future>(work: F) -> Result<F::Output, Failed> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(work))
}

/// The tree, its home, and the host the catalog is published on.
struct Bench {
    tree: Tree,
    home: crucible_config::Home,
    server: Server,
}

impl Bench {
    fn new(name: &str) -> Result<Self, Failed> {
        let tree = Tree::new(name)?;
        let home = tree.home()?;
        fs::create_dir_all(home.path())?;
        Ok(Self {
            tree,
            home,
            server: Server::start()?,
        })
    }

    fn user(&self) -> PathBuf {
        crucible_config::user(&self.home)
    }

    fn programs(&self) -> PathBuf {
        self.home.path().join("programs").join("goose")
    }

    /// Where version `version` of goose's program is put.
    fn program(&self, version: &str) -> PathBuf {
        self.programs().join(version).join("goose")
    }

    /// Writes the user file with one instance `work` of goose started as
    /// `executable`, and anything `more` adds.
    fn configure(&self, executable: &Path, more: &Value) -> Result<(), Failed> {
        let mut document = json!({
            "externalAgents": {
                "work": {
                    "driver": "catalog",
                    "agent": "goose",
                    "executable": executable.to_string_lossy(),
                }
            }
        });
        if let (Some(into), Some(from)) = (document.as_object_mut(), more.as_object()) {
            into.extend(from.clone());
        }
        fs::write(self.user(), serde_json::to_vec_pretty(&document)?)?;
        Ok(())
    }

    /// The executable the user file names for `work`.
    fn executable(&self) -> Result<String, Failed> {
        let value: Value = serde_json::from_slice(&fs::read(self.user())?)?;
        Ok(value
            .pointer("/externalAgents/work/executable")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// `command` asked of the door with `front`, cancelled at the first byte
    /// that arrives where `cancelling`, and what it told of progress.
    fn ask(
        &self,
        command: Command,
        front: &mut Answering<'_>,
        cancelling: bool,
    ) -> Result<(Result<Outcome, AgentsError>, Vec<InstallProgress>), Failed> {
        let lent = Shelf {
            catalog: self.server.source(),
            home: &self.home,
            smoke: None,
            steps: None,
        };
        let request = request(command);
        let cancel = Cancel::new();
        let mut told: Vec<InstallProgress> = Vec::new();
        let outcome = {
            let mut progress = |said: InstallProgress| {
                told.push(said);
                if cancelling {
                    cancel.request();
                }
            };
            on_runtime(run(
                &request,
                &lent,
                front,
                Watching {
                    progress: &mut progress,
                    cancel: &cancel,
                    link: &|_| {},
                },
            ))?
        };
        Ok((outcome, told))
    }

    /// The names in goose's store directory, sorted.
    fn stored(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.programs())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// What came of a command at the door, named by its variant alone, so a
/// failure says which came back and prints nothing it carries: what an
/// agent's account step read is never written into a failure.
const fn shape(outcome: &Result<Outcome, AgentsError>) -> &'static str {
    match outcome {
        Ok(Outcome::Refused(_)) => "Ok(Refused)",
        Ok(Outcome::Catalog { .. }) => "Ok(Catalog)",
        Ok(Outcome::Undecided) => "Ok(Undecided)",
        Ok(Outcome::Moved { .. }) => "Ok(Moved)",
        Ok(Outcome::Current { .. }) => "Ok(Current)",
        Ok(Outcome::Removed { .. }) => "Ok(Removed)",
        Ok(Outcome::SignedIn { .. }) => "Ok(SignedIn)",
        Ok(Outcome::Kept { .. }) => "Ok(Kept)",
        Ok(Outcome::Verified { .. }) => "Ok(Verified)",
        Err(AgentsError::Config(_)) => "Err(Config)",
        Err(AgentsError::Unoffered { .. }) => "Err(Unoffered)",
        Err(AgentsError::Unlisted(_)) => "Err(Unlisted)",
        Err(AgentsError::NotInstalled(_)) => "Err(NotInstalled)",
        Err(AgentsError::NoPrevious(_)) => "Err(NoPrevious)",
        Err(AgentsError::InUse { .. }) => "Err(InUse)",
        Err(AgentsError::Unsupported(_)) => "Err(Unsupported)",
        Err(AgentsError::Program(_)) => "Err(Program)",
        Err(AgentsError::Kept { .. }) => "Err(Kept)",
        Err(AgentsError::TooLarge) => "Err(TooLarge)",
        Err(AgentsError::Store(_)) => "Err(Store)",
        Err(AgentsError::Download(_)) => "Err(Download)",
        Err(AgentsError::Unpacked(_)) => "Err(Unpacked)",
        Err(AgentsError::Remember(_)) => "Err(Remember)",
        Err(AgentsError::Smoke { .. }) => "Err(Smoke)",
        Err(AgentsError::NoInstance(_)) => "Err(NoInstance)",
        Err(AgentsError::Unsigned(_)) => "Err(Unsigned)",
        Err(AgentsError::NoMethod { .. }) => "Err(NoMethod)",
        Err(AgentsError::Unclouded { .. }) => "Err(Unclouded)",
        Err(AgentsError::Unlocated(_)) => "Err(Unlocated)",
        Err(AgentsError::Conversation(_)) => "Err(Conversation)",
        Err(AgentsError::Unnamed(_)) => "Err(Unnamed)",
        Err(AgentsError::Unstored { .. }) => "Err(Unstored)",
        Err(AgentsError::Keyless(_)) => "Err(Keyless)",
        Err(AgentsError::Typed) => "Err(Typed)",
        Err(AgentsError::Credentials(_)) => "Err(Credentials)",
        Err(AgentsError::Unprofiled(_)) => "Err(Unprofiled)",
        Err(AgentsError::Unlaunched(_)) => "Err(Unlaunched)",
        Err(AgentsError::HoldUnkept(_)) => "Err(HoldUnkept)",
        Err(AgentsError::Unverified(_)) => "Err(Unverified)",
        Err(AgentsError::Unstopped(_)) => "Err(Unstopped)",
        Err(AgentsError::Running(_)) => "Err(Running)",
    }
}

fn install(version: &str) -> Result<Command, Failed> {
    Ok(Command::InstallAgent {
        agent: name("goose")?,
        version: name(version)?,
    })
}

const CATALOG: [&str; 2] = ["/catalog/index.json", "/catalog/index.json.minisig"];

#[test]
fn nothing_is_fetched_before_the_answer_and_a_no_leaves_every_file_as_it_was() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-no")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (outcome, told) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let [(Pending::Install { facts, choices, .. }, asked)] = front.put.as_slice() else {
        return Err(format!("one install confirmation, not {:?}", front.put).into());
    };
    assert_eq!(asked, &CATALOG);
    let facts: Vec<&str> = facts
        .iter()
        .map(crucible_client_api::Text::as_str)
        .collect();
    for said in [
        "goose 1.0.0",
        "Fetched from 127.0.0.1",
        "Licence Apache-2.0; terms at https://example.com/terms",
        "Caution: Read the terms first.",
        "Started as: goose acp",
        "GOOSE_NO_UPDATE=1",
        "Moves work from 1.0.0 onto 1.0.0",
        "Not smoke-tested: no conversation lent a launcher",
    ] {
        assert!(
            facts.iter().any(|fact| fact.contains(said)),
            "{said:?} in {facts:?}"
        );
    }
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.name.as_str())
            .collect::<Vec<_>>(),
        ["Install", "Cancel"]
    );
    assert_eq!(bench.server.asked(), CATALOG);
    assert!(told.is_empty());
    assert!(!bench.program("1.0.0").exists());
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

#[test]
fn update_rewrites_the_executable_and_rollback_restores_it() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-update")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (installed, told) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(
            &installed,
            Ok(Outcome::Moved { how: How::Installed, version, instances, downloaded: true, unpruned: None, .. })
                if &**version == "1.0.0" && instances.len() == 1
        ),
        "{}",
        shape(&installed)
    );
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    let total = u64::try_from(ONE.len())?;
    assert_eq!(
        told.last().map(|said| (said.done(), said.total())),
        Some((total, total))
    );

    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    let mut front = Answering {
        choice: Some("Update"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (updated, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert!(
        matches!(&updated, Ok(Outcome::Moved { how: How::Updated, version, .. }) if &**version == "2.0.0"),
        "{}",
        shape(&updated)
    );
    assert_eq!(
        bench.executable()?,
        bench.program("2.0.0").to_string_lossy()
    );
    assert_eq!(fs::read(bench.program("2.0.0"))?, TWO);
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);

    let mut front = Answering {
        choice: Some("Roll back"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let (rolled, _) = bench.ask(
        Command::RollbackAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert!(
        matches!(&rolled, Ok(Outcome::Moved { how: How::RolledBack, version, downloaded: false, .. }) if &**version == "1.0.0"),
        "{}",
        shape(&rolled)
    );
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    // The rollback fetched nothing but the catalog.
    assert_eq!(front.put.len(), 1);
    Ok(())
}

/// Installs goose 1.0.0 with a yes, then publishes 2.0.0 beside it.
fn installed_then_updated_in_the_catalog(bench: &Bench) -> Result<(), Failed> {
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(
            installed,
            Ok(Outcome::Moved {
                standing: false,
                ..
            })
        ),
        "{}",
        shape(&installed)
    );
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    Ok(())
}

#[test]
fn under_reviewed_an_update_runs_with_no_question_and_says_so() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-reviewed")?;
    bench.configure(
        &bench.program("1.0.0"),
        &json!({"agents": {"update": "reviewed"}}),
    )?;
    installed_then_updated_in_the_catalog(&bench)?;
    let mut front = answering(&bench.server, Some("Cancel"));

    let (updated, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert!(front.put.is_empty(), "nothing was put to the client");
    assert!(
        matches!(&updated, Ok(Outcome::Moved { how: How::Updated, version, standing: true, downloaded: true, .. })
            if &**version == "2.0.0"),
        "{}",
        shape(&updated)
    );
    assert_eq!(
        bench.executable()?,
        bench.program("2.0.0").to_string_lossy()
    );
    assert_eq!(fs::read(bench.program("2.0.0"))?, TWO);
    // No launcher was lent, so nothing was started, and the outcome says why.
    assert!(
        matches!(&updated, Ok(Outcome::Moved { unsmoked: Some(why), .. }) if why.contains("launcher")),
        "{}",
        shape(&updated)
    );
    Ok(())
}

#[test]
fn under_ask_an_update_is_put_to_the_client_and_a_no_changes_nothing() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-ask")?;
    bench.configure(
        &bench.program("1.0.0"),
        &json!({"agents": {"update": "ask"}}),
    )?;
    installed_then_updated_in_the_catalog(&bench)?;
    let mut front = answering(&bench.server, Some("Cancel"));

    let (updated, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert_eq!(front.put.len(), 1);
    assert!(
        matches!(updated, Ok(Outcome::Undecided)),
        "{}",
        shape(&updated)
    );
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    assert!(!bench.program("2.0.0").exists());
    Ok(())
}

#[test]
fn reviewed_asks_for_a_first_install_and_still_refuses_a_release_it_would_refuse_when_asked()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-reviewed-install")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(
        &bench.program("1.0.0"),
        &json!({"agents": {"update": "reviewed"}}),
    )?;
    let mut front = answering(&bench.server, Some("Cancel"));

    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert_eq!(front.put.len(), 1, "a first install is asked");
    assert!(
        matches!(installed, Ok(Outcome::Undecided)),
        "{}",
        shape(&installed)
    );

    // An update whose download is redirected off the listed hosts is refused
    // as it is when asked, and commits nothing.
    let elsewhere = Server::start()?;
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    elsewhere.serve("/stolen", Reply::Body(TWO.to_vec()));
    bench
        .server
        .serve(&asset_path(TWO), Reply::Redirect(elsewhere.at("/stolen")));
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let mut front = answering(&bench.server, None);
    let (refused, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;
    assert!(front.put.is_empty());
    assert!(
        matches!(refused, Err(AgentsError::Download(DownloadError::Redirect))),
        "{}",
        shape(&refused)
    );
    assert!(elsewhere.asked().is_empty());
    assert!(!bench.program("2.0.0").exists());
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    Ok(())
}

const ZERO: &[u8] = b"goose 0.9.0, as reviewed, and older";

/// Installs goose 1.0.0 with a yes at the catalog serial `serial`, the user
/// file saying `agents.update = "reviewed"`.
fn reviewed_with_one_installed(bench: &Bench, serial: u64) -> Result<(), Failed> {
    bench.configure(
        &bench.program("1.0.0"),
        &json!({"agents": {"update": "reviewed"}}),
    )?;
    bench.server.publish(serial, &[("1.0.0", ONE)], &[]);
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    Ok(())
}

/// An update asked of the door, whose question is declined, and what the
/// client was put.
fn update_declined(bench: &Bench) -> Result<(usize, Result<Outcome, AgentsError>), Failed> {
    let mut front = answering(&bench.server, Some("Cancel"));
    let (outcome, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;
    Ok((front.put.len(), outcome))
}

#[test]
fn under_reviewed_an_older_release_listed_first_is_asked_about_and_not_taken() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-reviewed-older")?;
    reviewed_with_one_installed(&bench, 1)?;
    bench
        .server
        .publish(2, &[("0.9.0", ZERO), ("1.0.0", ONE)], &[]);

    let (put, outcome) = update_declined(&bench)?;

    assert_eq!(put, 1, "an older release is never taken unasked");
    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    assert!(!bench.program("0.9.0").exists());
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    Ok(())
}

#[test]
fn under_reviewed_a_revoked_newest_release_does_not_install_an_older_one_unasked()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-reviewed-revoked")?;
    reviewed_with_one_installed(&bench, 1)?;
    bench
        .server
        .serve(&asset_path(TWO), Reply::Body(TWO.to_vec()));
    bench
        .server
        .serve(&asset_path(ZERO), Reply::Body(ZERO.to_vec()));
    bench.server.catalog(
        2,
        &[goose("2.0.0", TWO), goose("0.9.0", ZERO)],
        &json!([{"key": "goose", "version": "2.0.0", "reason": "It wrote outside its directory."}]),
    )?;

    let (put, outcome) = update_declined(&bench)?;

    assert_eq!(put, 1);
    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    assert!(!bench.program("0.9.0").exists());
    assert!(!bench.program("2.0.0").exists());

    // The revoked release itself is refused, asked or not.
    let mut front = answering(&bench.server, None);
    let (refused, _) = bench.ask(install("2.0.0")?, &mut front, false)?;
    assert!(
        matches!(refused, Err(AgentsError::Unoffered { .. })),
        "{}",
        shape(&refused)
    );
    assert!(front.put.is_empty());
    Ok(())
}

#[test]
fn under_reviewed_an_offer_from_a_catalog_older_than_the_install_s_is_asked_about()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-reviewed-serial")?;
    reviewed_with_one_installed(&bench, 1)?;
    // The install is recorded as taken from a catalog later than any the
    // offers below come from.
    let receipt = bench.programs().join("1.0.0").join(".receipt");
    let mut kept: Value = serde_json::from_slice(&fs::read(&receipt)?)?;
    set(&mut kept, "/serial", json!(SNAPSHOT.serial() + 1000));
    fs::write(&receipt, serde_json::to_vec(&kept)?)?;
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);

    let (put, outcome) = update_declined(&bench)?;

    assert_eq!(put, 1);
    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    assert!(!bench.program("2.0.0").exists());
    Ok(())
}

#[test]
fn under_reviewed_a_rollback_and_a_removal_are_still_asked() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-reviewed-rollback")?;
    reviewed_with_one_installed(&bench, 1)?;
    bench
        .server
        .publish(2, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    let mut front = answering(&bench.server, Some("Cancel"));
    let (updated, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;
    assert!(
        matches!(updated, Ok(Outcome::Moved { standing: true, .. })),
        "{}",
        shape(&updated)
    );

    let mut front = answering(&bench.server, Some("Cancel"));
    let (rolled, _) = bench.ask(
        Command::RollbackAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;
    assert_eq!(front.put.len(), 1);
    assert!(
        matches!(rolled, Ok(Outcome::Undecided)),
        "{}",
        shape(&rolled)
    );

    // Nothing runs it from the store once the instance is off it.
    bench.configure(
        &bench.home.path().join("bin").join("true"),
        &json!({"agents": {"update": "reviewed"}}),
    )?;
    let mut front = answering(&bench.server, Some("Cancel"));
    let (removed, _) = bench.ask(
        Command::RemoveAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;
    assert_eq!(front.put.len(), 1);
    assert!(
        matches!(removed, Ok(Outcome::Undecided)),
        "{}",
        shape(&removed)
    );
    Ok(())
}

#[test]
fn a_redirect_to_a_host_not_listed_is_refused_and_commits_nothing() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-redirect")?;
    let elsewhere = Server::start()?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    elsewhere.serve("/goose", Reply::Body(ONE.to_vec()));
    bench
        .server
        .serve(&asset_path(ONE), Reply::Redirect(elsewhere.at("/goose")));
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Err(AgentsError::Download(DownloadError::Redirect))),
        "{}",
        shape(&outcome)
    );
    assert!(elsewhere.asked().is_empty());
    assert!(!bench.program("1.0.0").exists());
    assert_eq!(bench.stored(), [".lock", ".staging"]);
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

#[test]
fn a_download_cancelled_part_way_commits_nothing() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-cancel")?;
    let mut doubled = ONE.to_vec();
    doubled.extend_from_slice(ONE);
    bench.server.publish(1, &[("1.0.0", &doubled)], &[]);
    bench
        .server
        .serve(&asset_path(&doubled), Reply::Half(ONE.to_vec()));
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (outcome, told) = bench.ask(install("1.0.0")?, &mut front, true)?;

    assert!(
        matches!(
            outcome,
            Err(AgentsError::Download(DownloadError::Cancelled))
        ),
        "{}",
        shape(&outcome)
    );
    let half = u64::try_from(ONE.len())?;
    assert_eq!(
        told.iter()
            .map(|said| (said.done(), said.total()))
            .collect::<Vec<_>>(),
        [(half, half * 2)]
    );
    assert!(!bench.program("1.0.0").exists());
    assert_eq!(bench.stored(), [".lock", ".staging"]);
    assert!(
        fs::read_dir(bench.programs().join(".staging"))?
            .next()
            .is_none()
    );
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

#[test]
fn a_listing_fetches_the_catalog_even_where_updates_are_never_checked() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-list")?;
    bench.server.publish(
        1,
        &[("1.0.0", ONE)],
        &[entry("claude", "9.0.0", ONE), entry("kiro", "2.0.0", TWO)],
    );
    bench.configure(Path::new("goose"), &json!({"updates": {"check": "never"}}))?;
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (outcome, _) = bench.ask(Command::AgentCatalog, &mut front, false)?;

    assert_eq!(bench.server.asked(), CATALOG);
    let Ok(Outcome::Catalog {
        agents,
        more: false,
        unrefreshed: None,
    }) = outcome
    else {
        return Err(format!("a listing, not {}", shape(&outcome)).into());
    };
    let named: Vec<(&str, Vec<&str>)> = agents
        .iter()
        .map(|listed| {
            (
                listed.agent.as_str(),
                listed.offered.iter().map(|version| &**version).collect(),
            )
        })
        .collect();
    // A key a built-in driver is spelled with is no catalog row, and neither
    // is the entry behind the built-in Antigravity.
    assert_eq!(named, [("goose", vec!["1.0.0"]), ("kiro", vec!["2.0.0"])]);
    assert!(front.put.is_empty());
    Ok(())
}

#[test]
fn a_client_that_did_not_claim_install_is_refused_before_anything_is_fetched() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-capability")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    let shelf = Shelf {
        catalog: bench.server.source(),
        home: &bench.home,
        smoke: None,
        steps: None,
    };
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let cancel = Cancel::new();

    for command in [Command::AgentCatalog, install("1.0.0")?] {
        let request = Request::new(
            Capabilities::none().with(Capability::Permissions),
            Correlation::new(1),
            command,
        );
        let outcome = on_runtime(run(
            &request,
            &shelf,
            &mut front,
            Watching {
                progress: &mut |_| {},
                cancel: &cancel,
                link: &|_| {},
            },
        ))?;

        assert!(
            matches!(&outcome, Ok(Outcome::Refused(refusal)) if refusal.code() == ErrorCode::NeedsCapability),
            "{}",
            shape(&outcome)
        );
    }
    assert!(bench.server.asked().is_empty());
    assert!(front.put.is_empty());
    assert!(!bench.programs().exists());
    Ok(())
}

#[test]
fn a_removal_waits_until_no_instance_runs_the_agent_then_takes_every_version() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-remove")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let remove = || -> Result<Command, Failed> {
        Ok(Command::RemoveAgent {
            agent: name("goose")?,
        })
    };
    let mut front = Answering {
        choice: Some("Remove"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (refused, _) = bench.ask(remove()?, &mut front, false)?;

    assert!(
        matches!(&refused, Err(AgentsError::InUse { instances, .. }) if instances.len() == 1),
        "{}",
        shape(&refused)
    );
    assert!(front.put.is_empty());
    assert!(bench.program("1.0.0").is_file());

    bench.configure(Path::new("goose"), &json!({}))?;
    bench.server.forget();
    let (removed, _) = bench.ask(remove()?, &mut front, false)?;

    assert!(
        matches!(&removed, Ok(Outcome::Removed { versions, .. }) if versions.len() == 1),
        "{}",
        shape(&removed)
    );
    assert!(!bench.program("1.0.0").exists());
    let _ = &bench.tree;
    Ok(())
}

#[test]
fn a_yes_naming_another_action_is_refused_and_after_three_nothing_is_fetched() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-stale")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = Answering {
        choice: Some("stale Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    assert_eq!(front.put.len(), 3);
    let ids: Vec<u64> = front
        .put
        .iter()
        .map(|(pending, _)| pending.id().number())
        .collect();
    assert!(
        ids.windows(2).all(|pair| pair.first() == pair.last()),
        "{ids:?}"
    );
    assert_eq!(bench.server.asked(), CATALOG);
    assert!(!bench.program("1.0.0").exists());
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

#[test]
fn a_release_larger_than_a_release_may_be_is_refused_before_it_is_asked_about() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-large")?;
    let mut large = goose("1.0.0", ONE);
    if let Some(asset) = large.pointer_mut("/targets/0/artifacts/0/replaced_by/size") {
        *asset = json!(5_u64 << 30);
    }
    bench.server.publish(1, &[], &[large]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Err(AgentsError::TooLarge)),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    Ok(())
}

/// `value` with what `pointer` names set to `to`, where it names anything.
fn set(value: &mut Value, pointer: &str, to: Value) {
    if let Some(at) = value.pointer_mut(pointer) {
        *at = to;
    }
}

/// The facts of the one install confirmation `front` was put.
fn facts(front: &Answering<'_>) -> Result<Vec<String>, Failed> {
    let [(Pending::Install { facts, .. }, _)] = front.put.as_slice() else {
        return Err(format!("one install confirmation, not {:?}", front.put).into());
    };
    Ok(facts.iter().map(|fact| fact.as_str().to_owned()).collect())
}

fn answering<'a>(server: &'a Server, choice: Option<&'static str>) -> Answering<'a> {
    Answering {
        choice,
        server,
        put: Vec::new(),
        meanwhile: None,
    }
}

#[test]
fn a_program_the_release_does_not_list_as_one_that_runs_is_refused_before_the_question()
-> Result<(), Failed> {
    for (index, (path, exec)) in [("goose", false), ("goose-helper", true)]
        .into_iter()
        .enumerate()
    {
        let bench = Bench::new(&format!("agents-door-program-{index}"))?;
        let mut release = goose("1.0.0", ONE);
        if let Some(file) = release.pointer_mut("/targets/0/artifacts/0/files/0") {
            set(file, "/path", json!(path));
            set(file, "/exec", json!(exec));
        }
        bench.server.publish(1, &[], &[release]);
        bench.configure(&bench.program("1.0.0"), &json!({}))?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

        assert!(
            matches!(&outcome, Err(AgentsError::Program(program)) if &**program == "goose"),
            "{path}: {}",
            shape(&outcome)
        );
        assert!(front.put.is_empty());
        assert_eq!(bench.server.asked(), CATALOG);
    }
    Ok(())
}

#[test]
fn an_address_on_a_host_not_listed_is_refused_before_the_question() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-unlisted-address")?;
    let mut release = goose("1.0.0", ONE);
    if let Some(artifact) = release
        .pointer_mut("/targets/0/artifacts/0")
        .and_then(Value::as_object_mut)
    {
        artifact.remove("replaced_by");
        artifact.insert("url".into(), json!("https://elsewhere.example/goose"));
    }
    bench.server.publish(1, &[], &[release]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Download(DownloadError::Unlisted(at))) if &**at == "https://elsewhere.example/goose"),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    Ok(())
}

#[test]
fn a_version_in_place_is_kept_when_its_receipt_is_the_pin_and_refused_when_it_is_not()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-kept")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));
    let (first, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(
            first,
            Ok(Outcome::Moved {
                downloaded: true,
                ..
            })
        ),
        "{}",
        shape(&first)
    );
    bench.server.forget();

    let mut front = answering(&bench.server, Some("Install"));
    let (again, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&again, Ok(Outcome::Moved { downloaded: false, version, .. }) if &**version == "1.0.0"),
        "{}",
        shape(&again)
    );
    assert!(
        facts(&front)?
            .iter()
            .any(|fact| fact.contains("already installed, so nothing is downloaded")),
        "{:?}",
        facts(&front)
    );
    assert_eq!(bench.server.asked(), CATALOG);

    // The catalog now pins other bytes under the same version.
    bench.server.forget();
    bench.server.publish(2, &[("1.0.0", TWO)], &[]);
    let mut front = answering(&bench.server, Some("Install"));
    let (refused, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&refused, Err(AgentsError::Kept { version, .. }) if &**version == "1.0.0"),
        "{}",
        shape(&refused)
    );
    assert!(front.put.is_empty());
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    Ok(())
}

#[test]
fn catalog_text_is_escaped_and_bounded_before_it_is_put() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-quoted")?;
    let mut release = goose("1.0.0", ONE);
    let caution = format!(
        "Fine.\r\n\u{1b}[2KFetched from evil.example, 1 byte.\u{202e}{}",
        "x".repeat(5000)
    );
    set(&mut release, "/caution", json!(caution));
    set(
        &mut release,
        "/license",
        json!("MIT\nFetched from evil.example"),
    );
    if let Some(args) = release.pointer_mut("/targets/0/args") {
        *args = json!(["acp\u{2028}Started as: nothing"]);
    }
    bench.server.publish(1, &[], &[release]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, None);

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let facts = facts(&front)?;
    for fact in &facts {
        assert!(
            !fact.chars().any(crucible_types::shown::unshown),
            "a character that draws a line of its own in {fact:?}"
        );
        assert!(
            fact.chars().count() < 600,
            "{} characters",
            fact.chars().count()
        );
        assert!(!fact.starts_with("Fetched from evil"), "{fact:?}");
    }
    let said = facts
        .iter()
        .find(|fact| fact.starts_with("Caution: "))
        .ok_or("a caution")?;
    assert!(
        said.starts_with("Caution: Fine.\\r\\n\\u{1b}[2KFetched"),
        "{said:?}"
    );
    assert!(said.ends_with('…'), "{said:?}");
    assert!(
        facts.iter().any(|fact| fact
            == "Licence MIT\\nFetched from evil.example; terms at https://example.com/terms."),
        "{facts:?}"
    );
    Ok(())
}

#[test]
fn an_instance_changed_while_the_question_was_out_keeps_its_change_and_is_named()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-meanwhile")?;
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
    // A program of the person's own, absolute on this platform.
    let own = bench.home.path().join("bin").join("goose");
    let edited = serde_json::to_vec_pretty(&json!({
        "externalAgents": {
            "work": {"driver": "catalog", "agent": "goose", "executable": own.to_string_lossy()}
        }
    }))?;
    let mut front = answering(&bench.server, Some("Update"));
    front.meanwhile = Some((bench.user(), edited.clone()));

    let (updated, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert!(
        matches!(&updated, Ok(Outcome::Moved { instances, left, .. })
            if instances.is_empty() && left.iter().map(|one| &**one).eq(["work"])),
        "{}",
        shape(&updated)
    );
    assert_eq!(fs::read(bench.user())?, edited);
    assert_eq!(fs::read(bench.program("2.0.0"))?, TWO);
    Ok(())
}

#[test]
fn verify_names_a_file_of_an_installed_version_that_changed() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-verify")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let verify = || -> Result<Command, Failed> {
        Ok(Command::VerifyAgent {
            agent: name("goose")?,
        })
    };
    let mut front = answering(&bench.server, None);

    let (whole, _) = bench.ask(verify()?, &mut front, false)?;

    assert!(
        matches!(&whole, Ok(Outcome::Verified { versions, .. })
            if versions.len() == 1 && versions.iter().all(|(version, differs)| &**version == "1.0.0" && differs.is_empty())),
        "{}",
        shape(&whole)
    );
    let mut changed = ONE.to_vec();
    if let Some(first) = changed.first_mut() {
        *first ^= 1;
    }
    fs::write(bench.program("1.0.0"), changed)?;
    let (differs, _) = bench.ask(verify()?, &mut front, false)?;

    assert!(
        matches!(&differs, Ok(Outcome::Verified { versions, .. })
            if versions.iter().any(|(_, differs)| differs.iter().any(|one| &*one.path == "goose"))),
        "{}",
        shape(&differs)
    );
    assert!(front.put.is_empty());
    Ok(())
}

#[test]
fn an_update_already_on_the_version_offered_is_current_and_asks_nothing() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-current")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let mut front = answering(&bench.server, Some("Update"));

    let (current, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert!(
        matches!(&current, Ok(Outcome::Current { version, .. }) if &**version == "1.0.0"),
        "{}",
        shape(&current)
    );
    assert!(front.put.is_empty());
    Ok(())
}

#[test]
fn a_listing_says_on_its_row_that_one_agent_s_store_cannot_be_read() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-list-unread")?;
    bench
        .server
        .publish(1, &[("1.0.0", ONE)], &[entry("kiro", "2.0.0", TWO)]);
    bench.configure(Path::new("goose"), &json!({}))?;
    let programs = bench.home.path().join("programs");
    fs::create_dir_all(&programs)?;
    fs::write(programs.join("kiro"), b"not a directory")?;
    let mut front = answering(&bench.server, None);

    let (outcome, _) = bench.ask(Command::AgentCatalog, &mut front, false)?;

    let Ok(Outcome::Catalog { agents, .. }) = outcome else {
        return Err(format!("a listing, not {}", shape(&outcome)).into());
    };
    let rows: Vec<(&str, bool)> = agents
        .iter()
        .map(|listed| (listed.agent.as_str(), listed.installed.is_ok()))
        .collect();
    assert_eq!(rows, [("goose", true), ("kiro", false)]);
    Ok(())
}

#[test]
fn an_install_no_instance_runs_keeps_the_version_before_it_for_a_rollback() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-kept-before")?;
    bench
        .server
        .publish(1, &[("2.0.0", TWO), ("1.0.0", ONE)], &[]);
    // The one instance runs a program of its own, so none moves.
    bench.configure(Path::new("goose"), &json!({}))?;
    for version in ["1.0.0", "2.0.0"] {
        let mut front = answering(&bench.server, Some("Install"));
        let (installed, _) = bench.ask(install(version)?, &mut front, false)?;
        assert!(
            matches!(&installed, Ok(Outcome::Moved { instances, unpruned: None, .. }) if instances.is_empty()),
            "{version}: {}",
            shape(&installed)
        );
    }

    assert_eq!(fs::read(bench.program("2.0.0"))?, TWO);
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    Ok(())
}

/// What a vendor says of what it is sent, so the route of an entry carrying
/// it can be asked about and given a yes.
fn statement() -> Value {
    json!({
        "text": "The vendor may use what you send to improve goose.",
        "source": "https://vendor.example/terms/data",
        "date": "24 Oct 2026",
        "origins": ["https://api.vendor.example"],
    })
}

/// `entry` carrying the vendor's statement of content use.
fn stating(mut entry: Value) -> Value {
    if let Some(object) = entry.as_object_mut() {
        object.insert("content_use".into(), statement());
    }
    entry
}

/// The row `entry` gives its agent, as a signed catalog carries it.
fn row_of(entry: &Value) -> Result<CatalogRow, Failed> {
    let catalog = json!({"schema": 1, "serial": 1, "entries": [entry], "revoked": []});
    let signed = signed(&serde_json::to_vec(&catalog)?);
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

impl Server {
    /// Publishes a catalog of `entries`, revoking what `revoked` names.
    fn catalog(&self, serial: u64, entries: &[Value], revoked: &Value) -> Result<(), Failed> {
        let catalog = json!({
            "schema": 1,
            "serial": SNAPSHOT.serial() + serial,
            "entries": entries,
            "revoked": revoked,
        });
        let signed = signed(&serde_json::to_vec(&catalog)?);
        self.serve("/catalog/index.json", Reply::Body(signed.index));
        self.serve("/catalog/index.json.minisig", Reply::Body(signed.signature));
        Ok(())
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, Failed> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

/// How a start of `work`, an instance of goose run as `executable`, from the
/// row `row`, ended with the trust state `bench` keeps, and what the steps
/// would say of it before any question about its route.
fn launched(
    bench: &Bench,
    row: CatalogRow,
    executable: &Path,
) -> Result<(Result<(), Unlaunched>, Option<Unlaunched>), Failed> {
    let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
    consent.give("agent:goose");
    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row.clone()]))),
    ));
    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &executable.to_string_lossy(),
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let agent = Agent::configured(&instance, registry, |_| None).ok_or("its catalog row")?;
    let runtime = runtime()?;
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = catalog_fetch::state_in(bench.home.path());
    let launcher = Launcher::new(
        Arc::clone(&sandbox),
        handle.clone(),
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;
    let ended = runtime
        .block_on(launcher.launch(&agent, &control, dialect.as_mut(), None))
        .map(drop);
    let steps = Steps::new(
        Launcher::new(
            sandbox,
            handle,
            &policy,
            &consent,
            &state,
            bench.home.sessions(),
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        registry,
        None,
    );
    let before = steps.refusal(&AgentRow::Catalog(row), &instance);
    Ok((ended, before))
}

#[test]
fn a_revoked_version_is_refused_at_launch_with_its_reason_and_offers_rollback_or_update()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-revoked")?;
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
        .serve(&asset_path(TWO), Reply::Body(TWO.to_vec()));
    bench.server.catalog(
        2,
        &[goose("2.0.0", TWO)],
        &json!([{"key": "goose", "version": "1.0.0", "reason": "It wrote outside its directory."}]),
    )?;
    // Any command at the door takes the catalog in.
    let (listed, _) = bench.ask(
        Command::AgentCatalog,
        &mut answering(&bench.server, None),
        false,
    )?;
    assert!(
        matches!(
            listed,
            Ok(Outcome::Catalog {
                unrefreshed: None,
                ..
            })
        ),
        "{}",
        shape(&listed)
    );

    let row = row_of(&stating(goose("1.0.0", ONE)))?;
    let (ended, before) = launched(&bench, row, &bench.program("1.0.0"))?;

    let refused = ended.err().ok_or("the revoked version was started")?;
    let said = refused.to_string();
    for part in [
        "1.0.0",
        "revoked",
        "It wrote outside its directory.",
        "roll",
        "update",
    ] {
        assert!(said.contains(part), "{part:?} in {said:?}");
    }
    assert_eq!(before.map(|why| why.to_string()), Some(said));
    Ok(())
}

#[test]
fn a_version_in_the_store_is_refused_at_launch_while_the_trust_state_cannot_be_used()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-untrusted")?;
    bench.server.publish(1, &[("1.0.0", ONE)], &[]);
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let file = catalog_fetch::trust_in(bench.home.path()).file();
    fs::write(&file, b"not a trust state")?;

    let row = row_of(&stating(goose("1.0.0", ONE)))?;
    let (ended, _) = launched(&bench, row, &bench.program("1.0.0"))?;

    let said = ended
        .err()
        .ok_or("a version was started whose revocations are not known")?
        .to_string();
    assert!(said.contains(&file.display().to_string()), "{said:?}");
    Ok(())
}

/// The program `command` of a release goose is published as, and the script
/// beside it, as the bytes of a fake peer playing `script`.
fn peer(script: &Peer) -> Result<(Vec<u8>, Vec<u8>), Failed> {
    let made = Tree::new(&format!(
        "agents-door-peer-{}",
        PEERS.fetch_add(1, Ordering::Relaxed)
    ))?;
    let program = fake_peer::install(&made.0, "goose", script)?;
    Ok((fs::read(&program)?, fs::read(made.0.join("script"))?))
}

static PEERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The command goose's peer is started as on this platform.
fn command() -> String {
    format!("goose{}", std::env::consts::EXE_SUFFIX)
}

/// One file of a release, built at review and kept on the catalog's host.
fn artifact(path: &str, bytes: &[u8]) -> Value {
    json!({
        "url": format!("https://downloads.example.com/{path}"),
        "sha256": "a".repeat(64),
        "size": 1000,
        "format": "bare",
        "unpacked": bytes.len(),
        "decompressed": bytes.len(),
        "files": [{"path": path, "size": bytes.len(), "sha256": digest(bytes), "exec": true}],
        "replaced_by": {"sha256": digest(bytes), "size": bytes.len()}
    })
}

impl Server {
    /// `goose` at `version`, released as the fake peer `played` is: its
    /// program and its script, each kept on this host.
    fn peer_release(&self, version: &str, played: &(Vec<u8>, Vec<u8>)) -> Value {
        let (program, script) = played;
        for bytes in [program, script] {
            self.serve(&asset_path(bytes), Reply::Body(bytes.clone()));
        }
        let mut release = stating(goose(version, program));
        set(&mut release, "/targets/0/cmd", json!(command()));
        set(&mut release, "/targets/0/args", json!([]));
        set(
            &mut release,
            "/targets/0/artifacts",
            json!([artifact(&command(), program), artifact("script", script)]),
        );
        release
    }
}

/// Whether this machine's sandbox holds a program to a closed network and to
/// its filesystem rules. Where it does not, a smoke test can run nowhere here,
/// which is a failure on a job that requires the sandbox and a pass over
/// nothing elsewhere.
fn enforcing(runtime: &tokio::runtime::Runtime) -> Result<bool, Failed> {
    let service = LocalSandbox::new().watching_on(runtime.handle().clone());
    let enforced = match runtime.block_on(service.probe()) {
        Ok((_, capabilities)) => [SandboxFeature::NetworkDeny, SandboxFeature::Filesystem]
            .into_iter()
            .all(|feature| capabilities.claim(feature) == SandboxCapability::Enforced),
        Err(_) => false,
    };
    if !enforced && std::env::var_os("CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX").is_some() {
        return Err("this job requires the enforcing sandbox, and it is unavailable".into());
    }
    Ok(enforced)
}

impl Bench {
    /// `command` asked of the door on `runtime`, lent a launcher whose sandbox
    /// is on where `confined`, whose network is closed, and whose route
    /// `agent:goose` has its yes where `yes`, as `release` states its content
    /// use, and the steps of a conversation that has made no run.
    fn ask_smoking(
        &self,
        (command, front): (Command, &mut Answering<'_>),
        (confined, yes): (bool, bool),
        release: &Value,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let sandbox: Arc<dyn SandboxService> =
            Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
        self.ask_launching(
            (command, front),
            (confined, yes),
            release,
            (runtime, sandbox, None),
        )
    }

    /// [`Bench::ask_smoking`], its sandbox the one given, and the lent
    /// launcher giving the agents it starts `home` as theirs where one is
    /// named.
    fn ask_launching(
        &self,
        (command, front): (Command, &mut Answering<'_>),
        (confined, yes): (bool, bool),
        release: &Value,
        (runtime, sandbox, home): (
            &tokio::runtime::Runtime,
            Arc<dyn SandboxService>,
            Option<&Path>,
        ),
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let row = row_of(release)?;
        let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
        if yes {
            consent.give("agent:goose");
        }
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let handle = runtime.handle().clone();
        let state = catalog_fetch::state_in(self.home.path());
        let steps = Steps::new(
            Launcher::new(
                Arc::clone(&sandbox),
                handle.clone(),
                &policy,
                &consent,
                &state,
                self.home.sessions(),
            ),
            Arc::new(SandboxEnablement::new(confined, false)),
            &REGISTRY,
            None,
        );
        let mut launcher = Launcher::new(
            sandbox,
            handle,
            &policy,
            &consent,
            &state,
            self.home.sessions(),
        );
        if let Some(home) = home {
            launcher = launcher.homed(home);
        }
        let lent = Shelf {
            catalog: self.server.source(),
            home: &self.home,
            smoke: Some(&launcher),
            steps: Some(&steps),
        };
        let request = request(command);
        let cancel = Cancel::new();
        Ok(runtime.block_on(run(
            &request,
            &lent,
            front,
            Watching {
                progress: &mut |_| {},
                cancel: &cancel,
                link: &|_| {},
            },
        )))
    }

    /// Installs goose 1.0.0 for `work`, as an earlier install left it.
    fn installed_before(&self) -> Result<(), Failed> {
        self.server.publish(1, &[("1.0.0", ONE)], &[]);
        self.configure(&self.program("1.0.0"), &json!({}))?;
        let (installed, _) = self.ask(
            install("1.0.0")?,
            &mut answering(&self.server, Some("Install")),
            false,
        )?;
        match installed {
            Ok(Outcome::Moved { .. }) => Ok(()),
            other => Err(format!("installed before: {}", shape(&other)).into()),
        }
    }
}

fn update() -> Result<Command, Failed> {
    Ok(Command::UpdateAgent {
        agent: name("goose")?,
    })
}

/// What the agent answers initializing with, once and at every start, writing
/// nothing beside itself.
fn answers(version: &str) -> Peer {
    Peer::once(
        Played::new()
            .hear(1)
            .say(acp::initialized("goose", version, "agent-login")),
    )
}

#[test]
fn a_switch_is_smoke_tested_where_the_route_has_its_yes_and_the_sandbox_closes_the_network()
-> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let bench = Bench::new("agents-door-smoked")?;
    bench.installed_before()?;
    let release = bench
        .server
        .peer_release("2.0.0", &peer(&answers("2.0.0"))?);
    bench
        .server
        .catalog(2, &[release.clone(), goose("1.0.0", ONE)], &json!([]))?;
    let mut front = answering(&bench.server, Some("Update"));

    let updated = bench.ask_smoking((update()?, &mut front), (true, true), &release, &runtime)?;

    assert!(
        matches!(&updated, Ok(Outcome::Moved { version, .. }) if &**version == "2.0.0"),
        "{}",
        shape(&updated)
    );
    assert!(
        facts(&front)?
            .iter()
            .any(|fact| fact.starts_with("Smoke-tested")),
        "{:?}",
        facts(&front)
    );
    // The program is named as the release names it on this platform.
    assert_eq!(
        bench.executable()?,
        bench
            .programs()
            .join("2.0.0")
            .join(command())
            .to_string_lossy()
    );
    Ok(())
}

#[test]
fn a_smoke_with_a_wrong_agent_info_keeps_the_old_executable() -> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let bench = Bench::new("agents-door-smoke-wrong")?;
    bench.installed_before()?;
    let release = bench
        .server
        .peer_release("2.0.0", &peer(&answers("9.9.9"))?);
    bench
        .server
        .catalog(2, &[release.clone(), goose("1.0.0", ONE)], &json!([]))?;
    let mut front = answering(&bench.server, Some("Update"));

    let updated = bench.ask_smoking((update()?, &mut front), (true, true), &release, &runtime)?;

    let failed = updated.err().ok_or("the switch went ahead")?.to_string();
    assert!(
        failed.contains("9.9.9"),
        "the refusal does not name the version the agent said"
    );
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    Ok(())
}

#[test]
fn the_fake_peer_writing_into_its_root_fails_its_smoke_and_keeps_the_old_executable()
-> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let bench = Bench::new("agents-door-smoke-writes")?;
    bench.installed_before()?;
    // It keeps its place in the script in a file beside itself.
    let writing =
        Peer::of([Played::new()
            .hear(1)
            .say(acp::initialized("goose", "2.0.0", "agent-login"))]);
    let release = bench.server.peer_release("2.0.0", &peer(&writing)?);
    bench
        .server
        .catalog(2, &[release.clone(), goose("1.0.0", ONE)], &json!([]))?;
    let mut front = answering(&bench.server, Some("Update"));

    let updated = bench.ask_smoking((update()?, &mut front), (true, true), &release, &runtime)?;

    assert!(updated.is_err(), "{}", shape(&updated));
    let root = bench.programs().join("2.0.0");
    assert!(
        root.join("script").is_file(),
        "the release was not put in place"
    );
    assert!(!root.join("played").exists(), "it wrote into its root");
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    Ok(())
}

#[test]
fn a_switch_says_why_it_is_not_smoke_tested_where_the_sandbox_is_off_or_the_route_has_no_yes()
-> Result<(), Failed> {
    let runtime = runtime()?;
    for (index, (confined, yes, why)) in [
        (false, true, "the sandbox is off"),
        (true, false, "has no yes"),
    ]
    .into_iter()
    .enumerate()
    {
        let bench = Bench::new(&format!("agents-door-unsmoked-{index}"))?;
        bench.installed_before()?;
        let release = stating(goose("2.0.0", TWO));
        bench
            .server
            .serve(&asset_path(TWO), Reply::Body(TWO.to_vec()));
        bench
            .server
            .catalog(2, &[release.clone(), goose("1.0.0", ONE)], &json!([]))?;
        let mut front = answering(&bench.server, Some("Update"));

        let updated =
            bench.ask_smoking((update()?, &mut front), (confined, yes), &release, &runtime)?;

        assert!(
            matches!(&updated, Ok(Outcome::Moved { version, .. }) if &**version == "2.0.0"),
            "{}",
            shape(&updated)
        );
        assert!(
            facts(&front)?
                .iter()
                .any(|fact| fact.starts_with("Not smoke-tested") && fact.contains(why)),
            "{why:?} in {:?}",
            facts(&front)
        );
    }
    Ok(())
}

/// What goose 1.0.0, installed through the door as the fake peer playing
/// `script`, says when its version is asked, started from the store with the
/// sandbox on.
fn asked_from_the_store(name: &str, script: &Peer) -> Result<Reading, Failed> {
    let runtime = runtime()?;
    let bench = Bench::new(name)?;
    let release = bench.server.peer_release("1.0.0", &peer(script)?);
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let program = bench.programs().join("1.0.0").join(command());
    bench.configure(&program, &json!({}))?;
    let (installed, _) = bench.ask(
        install("1.0.0")?,
        &mut answering(&bench.server, Some("Install")),
        false,
    )?;
    if !matches!(installed, Ok(Outcome::Moved { .. })) {
        return Err(format!("installed: {}", shape(&installed)).into());
    }
    let row = row_of(&release)?;
    let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
    consent.give("agent:goose");
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
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?;
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = catalog_fetch::state_in(bench.home.path());
    let launcher = Launcher::new(
        sandbox,
        handle,
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;
    let read = runtime.block_on(launcher.account(
        &agent,
        &control,
        dialect.as_mut(),
        Account::Version,
        &|_| {},
    ))?;
    assert!(
        !bench.programs().join("1.0.0").join("played").exists(),
        "it wrote into its root"
    );
    Ok(read)
}

#[test]
fn an_agent_started_from_the_store_reads_its_version_and_cannot_write_into_it() -> Result<(), Failed>
{
    if !enforcing(&runtime()?)? {
        return Ok(());
    }

    let reading = asked_from_the_store("agents-door-root-read", &answers("1.0.0"))?;
    let writing = asked_from_the_store(
        "agents-door-root-write",
        &Peer::of([Played::new()
            .hear(1)
            .say(acp::initialized("goose", "1.0.0", "agent-login"))]),
    )?;

    // What an account step read is never written into a failure.
    assert!(
        matches!(&reading, Reading::Version(version) if version.as_str() == "1.0.0"),
        "the agent started from the store did not read as version 1.0.0"
    );
    assert!(
        matches!(writing, Reading::Unreadable(_)),
        "the agent that writes into its root was read as though it ran"
    );
    Ok(())
}

/// A program that answers initializing as goose at the version its updater
/// variable holds, so what it was started with is what it says.
#[cfg(unix)]
const ECHOING: &[u8] = b"#!/bin/sh\nread line\nprintf '{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocolVersion\":1,\"agentInfo\":{\"name\":\"goose\",\"version\":\"v%s\"}}}\\n' \"$GOOSE_NO_UPDATE\"\n";

/// `goose` at `version`, published as `program`, setting nothing of its own
/// and turning its updater off with `GOOSE_NO_UPDATE=1`.
#[cfg(unix)]
fn updating_off(version: &str, program: &[u8]) -> Value {
    let mut release = stating(goose(version, program));
    set(&mut release, "/targets/0/env", json!({}));
    if let Some(object) = release.as_object_mut() {
        object.insert(
            "updater_off".into(),
            json!({"name": "GOOSE_NO_UPDATE", "value": "1"}),
        );
    }
    release
}

#[cfg(unix)]
#[test]
fn a_version_the_catalog_no_longer_lists_is_started_with_its_updater_off() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-superseded-updater")?;
    let release = updating_off("1.0.0", ECHOING);
    bench
        .server
        .serve(&asset_path(ECHOING), Reply::Body(ECHOING.to_vec()));
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let (installed, _) = bench.ask(
        install("1.0.0")?,
        &mut answering(&bench.server, Some("Install")),
        false,
    )?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    // A newer release supersedes it, and the catalog lists it no longer.
    bench
        .server
        .serve(&asset_path(TWO), Reply::Body(TWO.to_vec()));
    bench
        .server
        .catalog(2, &[updating_off("2.0.0", TWO)], &json!([]))?;
    let (listed, _) = bench.ask(
        Command::AgentCatalog,
        &mut answering(&bench.server, None),
        false,
    )?;
    assert!(
        matches!(
            listed,
            Ok(Outcome::Catalog {
                unrefreshed: None,
                ..
            })
        ),
        "{}",
        shape(&listed)
    );

    let row = row_of(&release)?;
    let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
    consent.give("agent:goose");
    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
    ));
    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &bench.program("1.0.0").to_string_lossy(),
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let agent = Agent::configured(&instance, registry, |_| None).ok_or("its catalog row")?;
    let runtime = runtime()?;
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = catalog_fetch::state_in(bench.home.path());
    let launcher = Launcher::new(
        sandbox,
        handle,
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;

    let read = runtime.block_on(launcher.account(
        &agent,
        &control,
        dialect.as_mut(),
        Account::Version,
        &|_| {},
    ))?;

    // What an account step read is never written into a failure.
    assert!(
        matches!(&read, Reading::Version(version) if version.as_str() == "v1"),
        "the agent was not started with GOOSE_NO_UPDATE=1"
    );
    Ok(())
}

#[test]
fn a_program_in_another_agent_s_store_directory_is_refused_rather_than_taken_for_the_user_s_own()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-elsewhere")?;
    bench.installed_before()?;
    // The same bytes, put where the store keeps another agent's versions,
    // and revoked for goose.
    let elsewhere = bench
        .home
        .path()
        .join("programs")
        .join("kiro")
        .join("1.0.0");
    fs::create_dir_all(&elsewhere)?;
    fs::copy(bench.program("1.0.0"), elsewhere.join("goose"))?;
    bench.server.catalog(
        2,
        &[goose("1.0.0", ONE)],
        &json!([{"key": "goose", "version": "1.0.0", "reason": "It wrote outside its directory."}]),
    )?;
    let (listed, _) = bench.ask(
        Command::AgentCatalog,
        &mut answering(&bench.server, None),
        false,
    )?;
    assert!(
        matches!(
            listed,
            Ok(Outcome::Catalog {
                unrefreshed: None,
                ..
            })
        ),
        "{}",
        shape(&listed)
    );

    let row = row_of(&stating(goose("1.0.0", ONE)))?;
    let (ended, before) = launched(&bench, row, &elsewhere.join("goose"))?;

    let said = ended
        .err()
        .ok_or("a program in another agent's directory was started")?
        .to_string();
    assert!(
        said.contains("install store") && said.contains("kiro"),
        "{said:?}"
    );
    assert_eq!(before.map(|why| why.to_string()), Some(said));
    Ok(())
}

/// A key nobody would mistake for a real one, which no output may carry.
const KEY: &str = "goose-key-never-shown-0123456789";

/// `key` at 1.0.0, stating its content use, signing in by a key given in
/// `GOOSE_API_KEY`, by a browser it opens itself, or at a terminal, and
/// showing sign-in links only on `accounts.vendor.example`.
fn signing_in(key: &str, program: &[u8]) -> Value {
    let mut release = stating(entry(key, "1.0.0", program));
    if let Some(object) = release.as_object_mut() {
        object.insert(
            "auth_methods".into(),
            json!([
                {"id": "api-key", "kind": "env_var", "name": "API key", "var": "GOOSE_API_KEY"},
                {"id": "browser", "kind": "agent", "name": "Sign in with a browser"},
                {"id": "terminal", "kind": "terminal", "name": "Sign in at a terminal",
                 "command": "goose", "args": ["login"], "env": {}, "route": "piped"},
            ]),
        );
        object.insert("sign_in_hosts".into(), json!(["accounts.vendor.example"]));
    }
    release
}

fn sign_in(instance: &str, method: &str) -> Result<Command, Failed> {
    Ok(Command::SignInAgent {
        instance: name(instance)?,
        method: name(method)?,
    })
}

/// The bytes of crucible's credential store in `bench`'s home, or nothing.
fn credentials(bench: &Bench) -> String {
    fs::read_to_string(bench.home.path().join("auth.json")).unwrap_or_default()
}

/// Whether the peer installed in `directory` has been started `starts` times,
/// allowing a start that was just made a moment to be recorded.
fn started(directory: &Path, starts: usize) -> bool {
    for _ in 0..100 {
        if fake_peer::played(directory) == starts {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_key_typed_for_a_method_is_kept_under_the_agent_and_method_and_adopted_without_being_shown()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-sign-in-key")?;
    bench
        .server
        .catalog(1, &[signing_in("goose", ONE)], &json!([]))?;
    bench.configure(&bench.program("1.0.0"), &json!({}))?;

    let (before, _) = bench.ask(
        sign_in("work", "api-key")?,
        &mut answering(&bench.server, None),
        false,
    )?;
    assert!(
        matches!(&before, Err(AgentsError::Unstored { .. })),
        "a sign-in by a key not yet typed was adopted"
    );

    let typed = Typed::new(format!("  {KEY}\n"));
    assert!(
        !format!("{typed:?}").contains(KEY),
        "the typed key's Debug names it"
    );
    keep_key(&bench.home, "work", "api-key", typed)?;
    let (adopted, _) = bench.ask(
        sign_in("work", "api-key")?,
        &mut answering(&bench.server, None),
        false,
    )?;

    assert!(
        matches!(
            &adopted,
            Ok(Outcome::Kept { instance, method })
                if &**instance == "work" && &**method == "api-key"
        ),
        "the kept key was not found kept, or was claimed signed in with nothing started"
    );
    assert!(
        credentials(&bench).contains("\"goose@api-key\""),
        "the key is not kept under the agent's name and the method's id"
    );
    // Nothing said of it, done or refused, carries it.
    let refused = [
        keep_key(&bench.home, "work", "browser", Typed::new(KEY.to_owned())),
        keep_key(&bench.home, "work", "none", Typed::new(KEY.to_owned())),
        keep_key(&bench.home, "else", "api-key", Typed::new(KEY.to_owned())),
        keep_key(
            &bench.home,
            "work",
            "api-key",
            Typed::new("short".to_owned()),
        ),
        keep_key(
            &bench.home,
            "work",
            "api-key",
            Typed::new(format!("{KEY} {KEY}")),
        ),
    ];
    for refusal in &refused {
        let Err(why) = refusal else {
            return Err("a key was kept where it should have been refused".into());
        };
        assert!(
            !why.to_string().contains(KEY) && !format!("{why:?}").contains(KEY),
            "a refusal carried the key"
        );
    }
    assert!(
        !format!("{adopted:?}").contains(KEY),
        "the outcome carried the key"
    );
    Ok(())
}

#[test]
fn a_catalog_agent_named_as_a_provider_keeps_no_key_and_the_provider_s_key_survives()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-sign-in-provider")?;
    bench
        .server
        .catalog(1, &[signing_in("openai", ONE)], &json!([]))?;
    let document = json!({"externalAgents": {"work": {
        "driver": "catalog",
        "agent": "openai",
        "executable": bench.program("1.0.0").to_string_lossy(),
    }}});
    fs::write(bench.user(), serde_json::to_vec_pretty(&document)?)?;
    let store = crucible_auth::Store::in_home(bench.home.path())
        .naming(crucible_app::providers::Rows::production().names());
    store.keep("openai", "fake-provider-key-for-openai")?;

    // Asked first, so the catalog is fetched and offers the agent.
    let (adopted, _) = bench.ask(
        sign_in("work", "api-key")?,
        &mut answering(&bench.server, None),
        false,
    )?;
    let kept = keep_key(&bench.home, "work", "api-key", Typed::new(KEY.to_owned()));

    assert!(
        matches!(kept, Err(AgentsError::Unnamed(_))),
        "a key was kept under a provider's name"
    );
    assert!(
        matches!(adopted, Err(AgentsError::Unnamed(_))),
        "a provider's name was read as an agent's key"
    );
    assert!(
        store.read().has_key("openai"),
        "the provider's key was taken out"
    );
    assert!(
        !credentials(&bench).contains("openai@"),
        "an agent's key joined the provider's"
    );
    Ok(())
}

#[test]
fn a_method_the_agent_carries_out_needs_a_conversation_and_one_not_offered_is_refused()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-sign-in-refused")?;
    bench
        .server
        .catalog(1, &[signing_in("goose", ONE)], &json!([]))?;
    bench.configure(&bench.program("1.0.0"), &json!({}))?;

    let mut outcomes = Vec::new();
    for (instance, method) in [
        ("work", "browser"),
        ("work", "terminal"),
        ("work", "unknown"),
        ("else", "browser"),
    ] {
        let (outcome, _) = bench.ask(
            sign_in(instance, method)?,
            &mut answering(&bench.server, None),
            false,
        )?;
        outcomes.push(outcome);
    }

    let [browser, terminal, unknown, unconfigured] = outcomes.as_slice() else {
        return Err("four outcomes".into());
    };
    assert!(
        matches!(browser, Err(AgentsError::Conversation(method)) if method.contains("browser")),
        "{}",
        shape(browser)
    );
    assert!(
        matches!(terminal, Err(AgentsError::Unsupported(_))),
        "{}",
        shape(terminal)
    );
    assert!(
        matches!(unknown, Err(AgentsError::NoMethod { .. })),
        "{}",
        shape(unknown)
    );
    assert!(
        matches!(unconfigured, Ok(Outcome::Refused(refusal)) if refusal.code() == ErrorCode::InvalidArgument),
        "{}",
        shape(unconfigured)
    );
    Ok(())
}

#[test]
fn a_method_the_agent_carries_out_is_asked_for_by_name_once_its_route_has_a_yes()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-agent")?;
    let release = signing_in("goose", ONE);
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let made = Tree::new("agents-door-sign-in-agent-peer")?;
    let program = fake_peer::install(
        &made.0,
        "goose",
        &Peer::once(
            Played::new()
                .hear(1)
                .say(acp::initialized("goose", "1.0.0", "browser"))
                .hear(1)
                .say(acp::response(1, &json!({})))
                .hear(1)
                .say(acp::opened(2)),
        ),
    )?;
    bench.configure(&program, &json!({}))?;

    let unasked = bench.ask_smoking(
        (
            sign_in("work", "browser")?,
            &mut answering(&bench.server, None),
        ),
        (false, false),
        &release,
        &runtime,
    )?;
    assert!(
        matches!(unasked, Ok(Outcome::Undecided)),
        "{}",
        shape(&unasked)
    );
    assert!(
        fake_peer::heard(&made.0).is_empty(),
        "the agent was started before its route had a yes"
    );

    let signed = bench.ask_smoking(
        (
            sign_in("work", "browser")?,
            &mut answering(&bench.server, None),
        ),
        (false, true),
        &release,
        &runtime,
    )?;

    assert!(
        matches!(&signed, Ok(Outcome::SignedIn { by: Signed::Agent, method, .. }) if &**method == "browser"),
        "{}",
        shape(&signed)
    );
    let heard = fake_peer::heard(&made.0).join("\n");
    assert!(
        heard.contains("\"authenticate\"") && heard.contains("\"methodId\":\"browser\""),
        "{heard}"
    );
    assert!(
        heard
            .find("\"authenticate\"")
            .zip(heard.find("session/new"))
            .is_some_and(|(asked, opened)| asked < opened),
        "the sign-in was not read by opening a session after it: {heard}"
    );
    Ok(())
}

/// What the refusal of the instance `work` of goose, naming a profile
/// directory its entry has no variable for, says.
const NO_SEPARATE_PROFILE: &str = "externalAgents.work sets profileDir, but goose offers no \
                                   separate profile; remove profileDir from externalAgents.work.";

/// The user file naming the instance `work` of goose, started as
/// `executable`, with `profile` as its profile directory.
fn profiled(executable: &Path, profile: &Path) -> Value {
    json!({
        "externalAgents": {
            "work": {
                "driver": "catalog",
                "agent": "goose",
                "executable": executable.to_string_lossy(),
                "profileDir": profile.to_string_lossy(),
            }
        }
    })
}

/// `release`, its entry given the field `name` as `to`, whether or not it had
/// one.
fn with_field(mut release: Value, name: &str, to: Value) -> Value {
    if let Some(object) = release.as_object_mut() {
        object.insert(name.into(), to);
    }
    release
}

/// `release`, its entry naming the variable the agent reads its profile
/// directory from.
fn reading_its_profile_from_a_variable(release: Value) -> Value {
    with_field(release, "profile_env", json!("GOOSE_PATH_ROOT"))
}

#[test]
fn a_sign_in_on_an_instance_naming_a_profile_directory_its_entry_has_no_variable_for_starts_nothing()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-no-profile")?;
    let release = signing_in("goose", ONE);
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let made = Tree::new("agents-door-sign-in-no-profile-peer")?;
    let program = fake_peer::install(
        &made.0,
        "goose",
        &Peer::once(
            Played::new()
                .hear(1)
                .say(acp::initialized("goose", "1.0.0", "browser")),
        ),
    )?;
    let profile = bench.tree.0.join("profile");
    bench.configure(&program, &profiled(&program, &profile))?;

    let signed = bench.ask_smoking(
        (
            sign_in("work", "browser")?,
            &mut answering(&bench.server, None),
        ),
        (false, true),
        &release,
        &runtime,
    )?;

    assert!(
        matches!(&signed, Err(AgentsError::Unlaunched(refused)) if refused.to_string() == NO_SEPARATE_PROFILE),
        "{}",
        shape(&signed)
    );
    assert_eq!(fake_peer::played(&made.0), 0, "the agent was started");
    assert!(!profile.exists(), "its profile directory was made");
    Ok(())
}

#[test]
fn a_sign_in_on_an_instance_naming_a_profile_directory_its_entry_gives_a_variable_for_goes_on()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-profile")?;
    let release = reading_its_profile_from_a_variable(signing_in("goose", ONE));
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let made = Tree::new("agents-door-sign-in-profile-peer")?;
    let program = fake_peer::install(
        &made.0,
        "goose",
        &Peer::once(
            Played::new()
                .hear(1)
                .say(acp::initialized("goose", "1.0.0", "browser"))
                .hear(1)
                .say(acp::response(1, &json!({})))
                .hear(1)
                .say(acp::opened(2)),
        ),
    )?;
    let profile = bench.tree.0.join("profile");
    bench.configure(&program, &profiled(&program, &profile))?;

    let signed = bench.ask_smoking(
        (
            sign_in("work", "browser")?,
            &mut answering(&bench.server, None),
        ),
        (false, true),
        &release,
        &runtime,
    )?;

    assert!(
        matches!(
            &signed,
            Ok(Outcome::SignedIn {
                by: Signed::Agent,
                ..
            })
        ),
        "{}",
        shape(&signed)
    );
    assert!(profile.is_dir(), "its profile directory was not made");
    Ok(())
}

/// A sandbox that keeps the policy each start asked of it, and starts none.
struct Policies {
    local: Arc<dyn SandboxService>,
    asked: Arc<Mutex<Vec<SandboxPolicy>>>,
}

impl SandboxService for Policies {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.local.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push(request.policy().clone());
            }
            Err(SandboxError::BackendUnavailable {
                reason: "this sandbox only records what it is asked".into(),
            })
        })
    }
}

/// A sign-in made under the sandbox is let write where its entry says the
/// agent keeps it, under the home the agent is given, made private to the
/// user where it was missing, and nowhere else of that home.
#[test]
fn a_sandboxed_sign_in_may_write_where_its_entry_keeps_the_sign_in() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-state")?;
    let release = with_field(
        signing_in("goose", ONE),
        "state_paths",
        json!([".goose", ".config/goose"]),
    );
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let made = Tree::new("agents-door-sign-in-state-peer")?;
    let program = fake_peer::install(&made.0, "goose", &Peer::once(Played::new()))?;
    bench.configure(&program, &json!({}))?;
    let home = bench.tree.0.join("user");
    fs::create_dir_all(&home)?;
    // The places are judged where they lead, so they are looked for there.
    let home = fs::canonicalize(home)?;
    let asked = Arc::new(Mutex::new(Vec::new()));
    let sandbox = Policies {
        local: Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone())),
        asked: Arc::clone(&asked),
    };

    // The sandbox starts nothing, so the sign-in itself is not read.
    let signed = bench.ask_launching(
        (
            sign_in("work", "browser")?,
            &mut answering(&bench.server, None),
        ),
        (true, true),
        &release,
        (&runtime, Arc::new(sandbox), Some(&home)),
    )?;

    assert!(signed.is_err(), "{}", shape(&signed));
    assert_eq!(fake_peer::played(&made.0), 0, "the agent was started");
    let asked = asked.lock().map_err(|_| "the recorded policies")?.clone();
    let [policy] = asked.as_slice() else {
        return Err(format!("{} starts asked the sandbox", asked.len()).into());
    };
    for root in [".goose", ".config/goose"] {
        assert!(
            policy.permits_path(
                &home.join(root).join("auth.json"),
                SandboxFilesystemAccess::ReadWrite
            ),
            "{root} is not writable"
        );
        assert!(home.join(root).is_dir(), "{root} was not made");
    }
    for beside in [".bashrc", ".ssh/id", ".config/other"] {
        assert!(
            !policy.permits_path(&home.join(beside), SandboxFilesystemAccess::ReadWrite),
            "{beside} is writable"
        );
    }
    Ok(())
}

/// A listing says of each agent whether its entry names a variable for a
/// profile directory, or a place it keeps its sign-in in; where it names
/// neither, a sign-in made under the sandbox is not kept.
#[test]
fn a_listing_says_which_agents_keep_no_sign_in_under_the_sandbox() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-list-keeps-no-sign-in")?;
    bench.server.catalog(
        1,
        &[
            goose("1.0.0", ONE),
            with_field(
                entry("kiro", "2.0.0", TWO),
                "profile_env",
                json!("KIRO_HOME"),
            ),
            with_field(
                entry("zed", "3.0.0", TWO),
                "state_paths",
                json!([".config/zed"]),
            ),
        ],
        &json!([]),
    )?;
    bench.configure(Path::new("goose"), &json!({}))?;
    let mut front = answering(&bench.server, None);

    let (outcome, _) = bench.ask(Command::AgentCatalog, &mut front, false)?;

    let Ok(Outcome::Catalog { agents, .. }) = outcome else {
        return Err(format!("a listing, not {}", shape(&outcome)).into());
    };
    let rows: Vec<(&str, bool)> = agents
        .iter()
        .map(|listed| (listed.agent.as_str(), listed.keeps_no_sign_in))
        .collect();
    assert_eq!(rows, [("goose", true), ("kiro", false), ("zed", false)]);
    Ok(())
}

#[test]
fn a_sign_in_the_agent_says_took_is_unverified_where_no_session_opens_after_it()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-unverified")?;
    let release = signing_in("goose", ONE);
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let made = Tree::new("agents-door-sign-in-unverified-peer")?;
    let program = fake_peer::install(
        &made.0,
        "goose",
        &Peer::once(
            Played::new()
                .hear(1)
                .say(acp::initialized("goose", "1.0.0", "browser"))
                .hear(1)
                .say(acp::response(1, &json!({})))
                .hear(1)
                .say(acp::sign_in_required(2)),
        ),
    )?;
    bench.configure(&program, &json!({}))?;

    let signed = bench.ask_smoking(
        (
            sign_in("work", "browser")?,
            &mut answering(&bench.server, None),
        ),
        (false, true),
        &release,
        &runtime,
    )?;

    assert!(
        matches!(&signed, Err(AgentsError::Unverified(_))),
        "{}",
        shape(&signed)
    );
    Ok(())
}

#[test]
fn a_kept_key_is_asked_for_by_its_method_where_a_conversation_lent_its_launcher()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-kept")?;
    let release = signing_in("goose", ONE);
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let made = Tree::new("agents-door-sign-in-kept-peer")?;
    let program = fake_peer::install(
        &made.0,
        "goose",
        &Peer::once(
            Played::new()
                .hear(1)
                .say(acp::initialized("goose", "1.0.0", "api-key"))
                .hear(1)
                .say(acp::response(1, &json!({})))
                .hear(1)
                .say(acp::opened(2)),
        ),
    )?;
    bench.configure(&program, &json!({}))?;
    let ask = |yes: bool| {
        bench.ask_smoking(
            (
                sign_in("work", "api-key")?,
                &mut answering(&bench.server, None),
            ),
            (false, yes),
            &release,
            &runtime,
        )
    };

    let unkept = ask(true)?;
    keep_key(&bench.home, "work", "api-key", Typed::new(KEY.to_owned()))?;
    let unasked = ask(false)?;
    assert!(
        matches!(&unkept, Err(AgentsError::Unstored { .. })),
        "{}",
        shape(&unkept)
    );
    assert!(
        matches!(&unasked, Ok(Outcome::Undecided)),
        "{}",
        shape(&unasked)
    );
    assert!(
        fake_peer::heard(&made.0).is_empty(),
        "the agent was started before a key was kept and its route had a yes"
    );

    let signed = ask(true)?;

    assert!(
        matches!(
            &signed,
            Ok(Outcome::SignedIn { instance, method, by: Signed::Kept })
                if &**instance == "work" && &**method == "api-key"
        ),
        "the kept key was not signed in by its method"
    );
    let heard = fake_peer::heard(&made.0).join("\n");
    assert!(
        heard.contains("\"authenticate\"")
            && heard.contains("\"methodId\":\"api-key\"")
            && heard.contains("session/new"),
        "the agent was not asked to sign in by the kept key's method"
    );
    assert!(
        !heard.contains(KEY) && !format!("{signed:?}").contains(KEY),
        "the key was sent on the wire or carried by the outcome"
    );
    Ok(())
}

#[test]
fn a_sign_in_link_is_shown_only_on_a_listed_host_and_a_paste_only_with_its_state()
-> Result<(), Failed> {
    let row = row_of(&signing_in("goose", ONE))?;
    let profile = row.profile();
    let link = profile.link(
        "https://accounts.vendor.example/authorize?client_id=c&redirect_uri=http%3A%2F%2F127.0.0.1%3A4000%2F&state=s3cret-state\r\n",
    )?;
    assert_eq!(link.state(), Some("s3cret-state"));
    assert!(
        link.as_str()
            .starts_with("https://accounts.vendor.example/authorize?")
    );
    for (printed, why) in [
        (
            "https://accounts.other.example/authorize?state=s",
            Unlinked::Unlisted,
        ),
        (
            "https://accounts.vendor.example.other.example/?state=s",
            Unlinked::Unlisted,
        ),
        (
            "https://other.example/?next=https://accounts.vendor.example/",
            Unlinked::Unlisted,
        ),
        (
            "http://accounts.vendor.example/authorize?state=s",
            Unlinked::Unsecured,
        ),
        (
            "https://user@accounts.vendor.example/authorize?state=s",
            Unlinked::User,
        ),
        (
            "https://accounts.vendor.example:8443/authorize?state=s",
            Unlinked::Port,
        ),
        ("https://127.0.0.1/authorize?state=s", Unlinked::Address),
        (
            "https://accounts.vendor.example/authorize?state=s#more",
            Unlinked::Fragment,
        ),
        (
            "https://accounts.vendor.example/authorize?state=a&state=b",
            Unlinked::States,
        ),
        (
            "https://accounts.vendor.example/authorize?state=s and more",
            Unlinked::Unprintable,
        ),
    ] {
        assert_eq!(profile.link(printed).err(), Some(why), "{printed}");
    }

    let code = "4/the-code-never-shown";
    let pasted = |state: &str| format!("http://127.0.0.1:4000/?code={code}&state={state}");
    let taken = Callback::checked(pasted("s3cret-state"), &link)?;
    assert!(
        !format!("{taken:?}").contains(code),
        "the paste's Debug names it"
    );
    assert!(taken.sent().contains(code));
    for (paste, why) in [
        (pasted("another-state"), Uncalled::Mismatched),
        (format!("?code={code}"), Uncalled::Stateless),
        (
            format!("?code={code}&state=s3cret-state&state=s3cret-state"),
            Uncalled::Mismatched,
        ),
        ("?state=s3cret-state".to_owned(), Uncalled::Codeless),
        (
            format!("code={code}\u{1b}[2J&state=s3cret-state"),
            Uncalled::Unreadable,
        ),
    ] {
        let refused = Callback::checked(paste, &link);
        let Err(refusal) = refused else {
            return Err("a paste was taken where it should have been refused".into());
        };
        assert_eq!(refusal, why);
        assert!(
            !refusal.to_string().contains(code),
            "a refusal carried the code"
        );
    }
    let unstated = profile.link("https://accounts.vendor.example/authorize")?;
    assert_eq!(
        Callback::checked(format!("?code={code}&state=s"), &unstated).err(),
        Some(Uncalled::Stateless)
    );
    Ok(())
}

/// The release of `signing_in`, showing sign-in links on any host under
/// `vendor.example`.
fn signing_in_under_a_wildcard() -> Value {
    let mut release = signing_in("goose", ONE);
    set(&mut release, "/sign_in_hosts", json!(["*.vendor.example"]));
    release
}

/// A link's host is held to the letters a host name is spelled in before it
/// is matched, so no character a browser reads differently from a resolver
/// carries it past a wildcard.
#[test]
fn a_sign_in_link_host_is_spelled_as_a_host_name_before_it_is_matched() -> Result<(), Failed> {
    let row = row_of(&signing_in_under_a_wildcard())?;
    let profile = row.profile();
    for printed in [
        "https://login.vendor.example/authorize?state=s",
        "https://LOGIN.Vendor.Example/authorize?state=s",
        "https://deep.login.vendor.example:443/authorize?state=s",
    ] {
        assert_eq!(profile.link(printed)?.state(), Some("s"), "{printed}");
    }
    let misspelled = [
        "https://evil.example\\.vendor.example/authorize?state=s",
        "https://evil.example%5c.vendor.example/authorize?state=s",
        "https://evil.example%5C.vendor.example/authorize?state=s",
        "https://evil!.vendor.example/authorize?state=s",
        "https://evil_host.vendor.example/authorize?state=s",
    ];
    let refused: Vec<(&str, Option<Unlinked>)> = misspelled
        .iter()
        .map(|printed| (*printed, profile.link(printed).err()))
        .collect();
    let expected: Vec<(&str, Option<Unlinked>)> = misspelled
        .iter()
        .map(|printed| (*printed, Some(Unlinked::Unnamed)))
        .collect();
    assert_eq!(refused, expected);
    Ok(())
}

/// A link's `state`, and a paste's fields, are read only from after a `?`, so
/// a path that looks like a query carries none.
#[test]
fn a_sign_in_state_is_read_only_from_a_query() -> Result<(), Failed> {
    let row = row_of(&signing_in_under_a_wildcard())?;
    let profile = row.profile();
    let pathed = profile.link("https://login.vendor.example/a&state=s")?;
    let link = profile.link("https://login.vendor.example/authorize?state=s3cret-state")?;
    let code = "4/the-code-never-shown";
    let unqueried = Callback::checked(format!("code={code}&state=s3cret-state"), &link).err();
    assert_eq!(
        (pathed.state(), unqueried),
        (None, Some(Uncalled::Codeless))
    );
    Ok(())
}

#[test]
fn sign_in_hosts_that_name_every_host_or_an_address_give_no_profile() -> Result<(), Failed> {
    for (hosts, said) in [
        (json!(["*"]), "sign_in_hosts[0] names every host"),
        (
            json!(["accounts.vendor.example", "*.example"]),
            "sign_in_hosts[1] names every host",
        ),
        (json!(["127.0.0.1"]), "sign_in_hosts[0] is an address"),
        (json!(["[::1]"]), "sign_in_hosts[0] is an address"),
        (json!(["0x7f.1"]), "sign_in_hosts[0] is an address"),
    ] {
        let mut release = signing_in("goose", ONE);
        set(&mut release, "/sign_in_hosts", hosts);
        let refused = row_of(&release).err().ok_or("a profile with such hosts")?;
        assert!(refused.to_string().contains(said), "{refused}");
    }
    Ok(())
}

#[test]
fn antigravity_offers_its_four_methods_and_a_business_sign_in_names_what_it_needs()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-sign-in-antigravity")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let profile = bench.home.path().join("agents").join("antigravity");
    let document = json!({"externalAgents": {"google": {
        "driver": "antigravity",
        "executable": bench.program("1.0.0").to_string_lossy(),
        "profileDir": profile.to_string_lossy(),
    }}});
    fs::write(bench.user(), serde_json::to_vec_pretty(&document)?)?;

    let ask = |method: &str| -> Result<Result<Outcome, AgentsError>, Failed> {
        Ok(bench
            .ask(
                sign_in("google", method)?,
                &mut answering(&bench.server, None),
                false,
            )?
            .0)
    };
    let business = ask("oauth-business")?;
    let personal = ask("oauth-personal")?;
    keep_key(
        &bench.home,
        "google",
        "gemini-api-key",
        Typed::new(KEY.to_owned()),
    )?;
    let gemini = ask("gemini-api-key")?;
    let platform = ask("agent-platform")?;
    let elsewhere = ask("application-default")?;

    assert!(
        matches!(&business, Err(AgentsError::Unclouded { .. })),
        "{}",
        shape(&business)
    );
    // The agent reads a business project and location only from its own
    // settings, so the refusal names the keys crucible writes them from and
    // no variable it could be given them in.
    let said = business
        .err()
        .map(|refused| refused.to_string())
        .unwrap_or_default();
    for key in [
        "externalAgents.google.cloudProject",
        "externalAgents.google.cloudLocation",
    ] {
        assert!(
            said.contains(key),
            "the refusal does not name a key a business sign-in needs"
        );
    }
    assert!(!said.contains("GOOGLE_"), "the refusal names a variable");
    assert!(
        !profile.join("antigravity-acp").exists(),
        "the agent's settings were written for a refused sign-in"
    );
    assert!(
        matches!(personal, Err(AgentsError::Conversation(_))),
        "{}",
        shape(&personal)
    );
    assert!(
        matches!(gemini, Ok(Outcome::Kept { .. })),
        "the Gemini key was not found kept, or was claimed signed in with nothing started"
    );
    assert!(
        matches!(platform, Err(AgentsError::Unstored { .. })),
        "{}",
        shape(&platform)
    );
    assert!(
        matches!(elsewhere, Err(AgentsError::NoMethod { .. })),
        "{}",
        shape(&elsewhere)
    );
    assert!(credentials(&bench).contains("\"antigravity@gemini-api-key\""));
    Ok(())
}

/// What the fake Antigravity answers initializing with: the release the
/// snapshot reviewed, with the four methods it signs in by.
fn business_initialized() -> Result<String, Failed> {
    let shipped = SNAPSHOT
        .entries()
        .iter()
        .find(|entry| entry.key.as_str() == "antigravity")
        .ok_or("the snapshot reviews Antigravity")?;
    let methods: Vec<Value> = [
        "oauth-personal",
        "oauth-business",
        "gemini-api-key",
        "agent-platform",
    ]
    .into_iter()
    .map(|id| json!({"id": id, "name": id}))
    .collect();
    Ok(acp::response(
        0,
        &json!({
            "protocolVersion": 1,
            "agentInfo": {"name": &*shipped.admits.name, "version": &*shipped.admits.version},
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"resume": {}},
                "auth": {"logout": {}},
            },
            "authMethods": methods,
        }),
    ))
}

/// What the fake Antigravity does for one start: answers initializing as
/// [`business_initialized`] says, signs in by a business account when asked
/// and opens a session after it.
fn business_reply() -> Result<Played, Failed> {
    Ok(Played::new()
        .hear(1)
        .say(business_initialized()?)
        .hear(1)
        .say(acp::response(1, &json!({})))
        .hear(1)
        .say(acp::opened(2)))
}

/// The fake Antigravity installed in `directory`, doing [`business_reply`] at
/// every start.
fn business_peer(directory: &Path) -> Result<PathBuf, Failed> {
    Ok(fake_peer::install(
        directory,
        "antigravity",
        &Peer::once(business_reply()?),
    )?)
}

/// What a start's sandbox was asked for: the network its policy grants, and
/// the bytes of the agent's settings in its profile at that moment.
type Prepared = (SandboxNetworkPolicy, Option<Vec<u8>>);

/// A sandbox that keeps what each start asked of it, then refuses it where
/// `refusing`, and otherwise runs it as the local sandbox does.
struct Recording {
    local: Arc<dyn SandboxService>,
    settings: PathBuf,
    refusing: bool,
    prepared: Arc<Mutex<Vec<Prepared>>>,
}

impl SandboxService for Recording {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.local.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            if let Ok(mut prepared) = self.prepared.lock() {
                prepared.push((
                    request.policy().network().clone(),
                    fs::read(&self.settings).ok(),
                ));
            }
            if self.refusing {
                return Err(SandboxError::BackendUnavailable {
                    reason: "this sandbox only records what it is asked".into(),
                });
            }
            self.local.prepare(request).await
        })
    }
}

/// The settings a business sign-in billed to `my-project-id` in `us-central1`
/// gives the agent.
const BILLED: &str = r#"{"auth":{"type":"oauth-business"},"gcp":{"project":"my-project-id","location":"us-central1"}}"#;

/// The consent of a conversation, whose route for Antigravity has its yes
/// where `yes`.
fn consent_for_antigravity(yes: bool) -> Result<Consent, Failed> {
    let row = REGISTRY
        .built(Driver::Antigravity)
        .ok_or("Antigravity has no row")?;
    let consent = Consent::new(Routes::production());
    if yes {
        consent.give(row.route);
    }
    Ok(consent)
}

impl Bench {
    /// Where the instance `google` keeps its profile.
    fn agy_profile(&self) -> PathBuf {
        self.home.path().join("agents").join("antigravity")
    }

    /// Writes the user file with one instance `google` of Antigravity run as
    /// `executable`, and the keys `cloud` adds to it.
    fn antigravity(&self, executable: &Path, cloud: &Value) -> Result<(), Failed> {
        let mut instance = json!({
            "driver": "antigravity",
            "executable": executable.to_string_lossy(),
            "profileDir": self.agy_profile().to_string_lossy(),
        });
        if let (Some(into), Some(from)) = (instance.as_object_mut(), cloud.as_object()) {
            into.extend(from.clone());
        }
        let document = json!({"externalAgents": {"google": instance}});
        fs::write(self.user(), serde_json::to_vec_pretty(&document)?)?;
        Ok(())
    }

    /// Writes the user file with the instances `google` and `spare` of
    /// Antigravity on one profile and `apart` on another, each run as
    /// `executable` and billed to a project and location.
    fn antigravity_sharing(&self, executable: &Path) -> Result<(), Failed> {
        let instance = |profile: PathBuf| {
            json!({
                "driver": "antigravity",
                "executable": executable.to_string_lossy(),
                "profileDir": profile.to_string_lossy(),
                "cloudProject": "my-project-id",
                "cloudLocation": "us-central1",
            })
        };
        let document = json!({"externalAgents": {
            "google": instance(self.agy_profile()),
            "spare": instance(self.agy_profile()),
            "apart": instance(self.home.path().join("agents").join("apart")),
        }});
        fs::write(self.user(), serde_json::to_vec_pretty(&document)?)?;
        Ok(())
    }

    /// The steps of a conversation on `sandbox` that launched a run of
    /// `google`, run as `program`, and the run, which is live until it is
    /// closed or dropped.
    fn running(
        &self,
        sandbox: Arc<dyn SandboxService>,
        program: &Path,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<(Steps, Box<dyn Backend>), Failed> {
        let steps = self.steps(sandbox, false, true, runtime)?;
        let runs = self.runs_of_google(&steps, program)?;
        let run = runtime.block_on(runs.launch(Mode::Ask, None, None, None))?;
        Ok((steps, run))
    }

    /// The runs `steps` hands out for `google`, run as `program`, none of them
    /// launched.
    fn runs_of_google(&self, steps: &Steps, program: &Path) -> Result<Runs, Failed> {
        let entry = ExternalAgent::unwritten(
            "google",
            Driver::Antigravity,
            &program.to_string_lossy(),
            Some(&self.agy_profile().to_string_lossy()),
            None,
        )
        .ok_or("an instance of a built-in driver")?;
        let (agent, _) = steps.onto(&entry, Mode::Ask)?;
        Ok(steps.runs(agent, "google"))
    }

    /// The steps of a conversation on `sandbox` that launched a run of
    /// `google`, run as `program`, and closed it, and whether the close
    /// confirmed that the run had stopped.
    fn ran(
        &self,
        sandbox: Arc<dyn SandboxService>,
        program: &Path,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<(Steps, bool), Failed> {
        let (steps, run) = self.running(sandbox, program, runtime)?;
        let confirmed = runtime.block_on(async move { run.close().await.is_ok() });
        Ok((steps, confirmed))
    }

    /// `command` asked of the door on `runtime`, lent a launcher on `sandbox`
    /// whose conversation confines its commands where `confined`, and whose
    /// route for Antigravity has its yes, and the steps of a conversation that
    /// has made no run.
    fn ask_lending(
        &self,
        command: Command,
        (sandbox, confined): (Arc<dyn SandboxService>, bool),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let steps = self.steps(Arc::clone(&sandbox), confined, true, runtime)?;
        self.ask_with(
            (command, &mut answering(&self.server, None)),
            (sandbox, confined),
            (true, Some(&steps)),
            runtime,
        )
    }

    /// The steps of a conversation on `sandbox` that confines its commands
    /// where `confined`, whose route for Antigravity has its yes where `yes`.
    fn steps(
        &self,
        sandbox: Arc<dyn SandboxService>,
        confined: bool,
        yes: bool,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Steps, Failed> {
        let consent = consent_for_antigravity(yes)?;
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let state = catalog_fetch::state_in(self.home.path());
        Ok(Steps::new(
            Launcher::new(
                sandbox,
                runtime.handle().clone(),
                &policy,
                &consent,
                &state,
                self.home.sessions(),
            ),
            Arc::new(SandboxEnablement::new(confined, false)),
            &REGISTRY,
            None,
        ))
    }

    /// `command` asked of the door on `runtime` and answered by `front`, lent
    /// a launcher on `sandbox` whose conversation confines its commands where
    /// `confined`, whose route for Antigravity has its yes where `yes`, and
    /// `steps`.
    fn ask_with(
        &self,
        (command, front): (Command, &mut Answering<'_>),
        (sandbox, confined): (Arc<dyn SandboxService>, bool),
        (yes, steps): (bool, Option<&Steps>),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        self.ask_requested(
            (request(command), front),
            (sandbox, confined),
            (yes, steps),
            runtime,
        )
    }

    /// `request` asked of the door as [`Bench::ask_with`] asks a command, and
    /// answered by any `front`.
    fn ask_requested(
        &self,
        (request, front): (Request, &mut dyn Front),
        (sandbox, confined): (Arc<dyn SandboxService>, bool),
        (yes, steps): (bool, Option<&Steps>),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let consent = consent_for_antigravity(yes)?;
        // A yes the front gives is written into the user file.
        consent.keeps_in(self.user());
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let state = catalog_fetch::state_in(self.home.path());
        let launcher = Launcher::new(
            sandbox,
            runtime.handle().clone(),
            &policy,
            &consent,
            &state,
            self.home.sessions(),
        );
        let lent = Shelf {
            catalog: self.server.source(),
            home: &self.home,
            smoke: Some(&launcher),
            steps,
        };
        let cancel = Cancel::new();
        Ok(runtime.block_on(run(
            &request,
            &lent,
            front,
            Watching {
                progress: &mut |_| {},
                cancel: &cancel,
                link: &|_| {},
            },
        )))
    }
}

/// A business sign-in for an entry naming its project and location writes
/// them into the agent's settings before the agent starts, starts it reaching
/// the business service's own host and the one its location names, and asks
/// the agent to sign in by that method.
#[test]
fn a_business_sign_in_writes_its_project_and_location_before_the_start_and_reaches_its_hosts()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-business")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    let program = business_peer(&peer)?;
    bench.antigravity(
        &program,
        &json!({"cloudProject": "my-project-id", "cloudLocation": "us-central1"}),
    )?;
    let settings = bench
        .agy_profile()
        .join("antigravity-acp")
        .join("settings.json");
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let recording = |refusing| Recording {
        local: Arc::clone(&local),
        settings: settings.clone(),
        refusing,
        prepared: Arc::default(),
    };

    // Confined, as only a sandbox that holds the agent to its hosts is asked,
    // and refused at the sandbox, so what the start asked for is all it did.
    let confining = recording(true);
    let confined = Arc::clone(&confining.prepared);
    let refused = bench.ask_lending(
        sign_in("google", "oauth-business")?,
        (Arc::new(confining), true),
        &runtime,
    )?;
    assert!(
        matches!(refused, Err(AgentsError::Unlaunched(_))),
        "{}",
        shape(&refused)
    );
    let confined = confined.lock().map(|seen| seen.clone()).unwrap_or_default();
    let [(network, written)] = confined.as_slice() else {
        return Err("the start did not ask the sandbox once".into());
    };
    assert!(
        written.as_deref() == Some(BILLED.as_bytes()),
        "the settings did not name the project and location before the start"
    );
    let SandboxNetworkPolicy::Domains(domains) = network else {
        return Err("the confined start was given no hosts".into());
    };
    for host in [
        "businessaicode.googleapis.com",
        "us-central1-businessaicode.googleapis.com",
    ] {
        assert!(domains.permits_host(host), "{host} is not reached");
    }
    assert!(
        !domains.permits_host("europe-west1-businessaicode.googleapis.com"),
        "a location the entry does not name is reached"
    );
    assert!(fake_peer::heard(&peer).is_empty(), "the agent was started");

    fs::write(&settings, b"{\"auth\":{\"type\":\"oauth-personal\"}}")?;
    let running = recording(false);
    let ran = Arc::clone(&running.prepared);
    let signed = bench.ask_lending(
        sign_in("google", "oauth-business")?,
        (Arc::new(running), false),
        &runtime,
    )?;
    assert!(
        matches!(&signed, Ok(Outcome::SignedIn { by: Signed::Agent, method, .. }) if &**method == "oauth-business"),
        "{}",
        shape(&signed)
    );
    let ran = ran.lock().map(|seen| seen.clone()).unwrap_or_default();
    assert!(
        matches!(ran.as_slice(), [(_, Some(written))] if written == BILLED.as_bytes()),
        "the settings the agent was started on were not replaced first"
    );
    let heard = fake_peer::heard(&peer).join("\n");
    assert!(
        heard.contains("\"authenticate\"") && heard.contains("\"methodId\":\"oauth-business\""),
        "the agent was not asked to sign in by a business account"
    );
    Ok(())
}

/// A business sign-in for an entry that does not give both its project and
/// its location is refused naming both keys, and writes and starts nothing.
#[test]
fn a_business_sign_in_without_its_project_and_location_names_both_and_starts_nothing()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-unclouded")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    let program = business_peer(&peer)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));

    for (cloud, named) in [
        (json!({}), true),
        (json!({"cloudProject": "my-project-id"}), false),
        (json!({"cloudLocation": "us-central1"}), false),
    ] {
        bench.antigravity(&program, &cloud)?;
        let refused = bench.ask_lending(
            sign_in("google", "oauth-business")?,
            (Arc::clone(&local), false),
            &runtime,
        )?;
        // An entry giving one without the other is refused as the
        // configuration is read; one giving neither, at the sign-in.
        assert!(
            if named {
                matches!(refused, Err(AgentsError::Unclouded { .. }))
            } else {
                matches!(refused, Err(AgentsError::Config(_)))
            },
            "{}",
            shape(&refused)
        );
        let said = refused
            .err()
            .map(|refused| refused.to_string())
            .unwrap_or_default();
        for key in ["cloudProject", "cloudLocation"] {
            assert!(said.contains(key), "the refusal does not name both keys");
        }
        assert!(
            !bench.agy_profile().join("antigravity-acp").exists(),
            "the agent's settings were written for a refused sign-in"
        );
        assert!(fake_peer::heard(&peer).is_empty(), "the agent was started");
    }
    Ok(())
}

/// A business sign-in whose location makes a host no policy can name is
/// refused naming the location's key, and writes and starts nothing.
#[test]
fn a_business_sign_in_whose_location_names_no_host_is_refused_naming_it() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-unlocated")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    let program = business_peer(&peer)?;
    // One label the configuration takes, too long for the host it makes.
    let location = format!("a-{}", "b".repeat(47));
    bench.antigravity(
        &program,
        &json!({"cloudProject": "my-project-id", "cloudLocation": location}),
    )?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));

    let refused = bench.ask_lending(
        sign_in("google", "oauth-business")?,
        (local, false),
        &runtime,
    )?;

    assert!(
        matches!(refused, Err(AgentsError::Unlocated(_))),
        "{}",
        shape(&refused)
    );
    let said = refused
        .err()
        .map(|refused| refused.to_string())
        .unwrap_or_default();
    assert!(
        said.contains("externalAgents.google.cloudLocation"),
        "the refusal does not name the location's key"
    );
    assert!(
        !bench.agy_profile().join("antigravity-acp").exists(),
        "the agent's settings were written for a refused sign-in"
    );
    assert!(fake_peer::heard(&peer).is_empty(), "the agent was started");
    Ok(())
}

/// Where the agent keeps its settings is a link out of its profile, a business
/// sign-in writes nothing where it leads and is refused before the start.
#[cfg(unix)]
#[test]
fn a_business_sign_in_through_a_link_out_of_the_profile_writes_nothing_and_is_refused()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-linked")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    let program = business_peer(&peer)?;
    bench.antigravity(
        &program,
        &json!({"cloudProject": "my-project-id", "cloudLocation": "us-central1"}),
    )?;
    let outside = bench.tree.0.join("outside");
    fs::create_dir_all(&outside)?;
    fs::create_dir_all(bench.agy_profile())?;
    std::os::unix::fs::symlink(&outside, bench.agy_profile().join("antigravity-acp"))?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));

    let refused = bench.ask_lending(
        sign_in("google", "oauth-business")?,
        (local, false),
        &runtime,
    )?;

    assert!(
        matches!(refused, Err(AgentsError::Unlaunched(_))),
        "{}",
        shape(&refused)
    );
    assert!(
        fs::read_dir(&outside)?.next().is_none(),
        "something was written where the link leads"
    );
    assert!(fake_peer::heard(&peer).is_empty(), "the agent was started");
    Ok(())
}

/// Every run of Antigravity reaches the hosts each of its sign-ins asks.
#[test]
fn antigravity_reaches_the_hosts_its_google_cloud_sign_ins_ask() -> Result<(), Failed> {
    let row = REGISTRY
        .built(Driver::Antigravity)
        .ok_or("Antigravity has no row")?;
    for host in [
        "generativelanguage.googleapis.com",
        "aiplatform.googleapis.com",
        "businessaicode.googleapis.com",
        "www.googleapis.com",
    ] {
        assert!(row.reaches.contains(&host), "{host} is not reached");
    }
    Ok(())
}

/// A program that answers initializing as goose at a version spelled from
/// the key it was given, so what it would say of the key is what is kept.
#[cfg(unix)]
const SAYING_THE_KEY: &[u8] = b"#!/bin/sh\nread line\nprintf '{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocolVersion\":1,\"agentInfo\":{\"name\":\"goose\",\"version\":\"k%s\"}}}\\n' \"$GOOSE_API_KEY\"\n";

#[cfg(unix)]
#[test]
fn a_kept_key_is_given_at_launch_and_withheld_from_what_the_agent_says() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-sign-in-launch")?;
    let mut release = signing_in("goose", SAYING_THE_KEY);
    set(&mut release, "/targets/0/env", json!({}));
    bench.server.serve(
        &asset_path(SAYING_THE_KEY),
        Reply::Body(SAYING_THE_KEY.to_vec()),
    );
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let (installed, _) = bench.ask(
        install("1.0.0")?,
        &mut answering(&bench.server, Some("Install")),
        false,
    )?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );

    let unkeyed = said_from_the_store(&bench, &release)?;
    keep_key(&bench.home, "work", "api-key", Typed::new(KEY.to_owned()))?;
    let keyed = said_from_the_store(&bench, &release)?;

    // What the agent said, as it is kept, is compared without being printed.
    assert!(
        unkeyed == "k",
        "the agent was given a key before one was kept"
    );
    assert!(
        keyed == "k[withheld]",
        "the kept key was not given, or was not withheld"
    );
    Ok(())
}

/// The version goose, installed for `work` in `bench` from `release`, says it
/// is, as what it says is kept.
#[cfg(unix)]
fn said_from_the_store(bench: &Bench, release: &Value) -> Result<String, Failed> {
    let row = row_of(release)?;
    let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
    consent.give("agent:goose");
    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
    ));
    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &bench.program("1.0.0").to_string_lossy(),
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let agent = Agent::configured(&instance, registry, |_| None).ok_or("its catalog row")?;
    let runtime = runtime()?;
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = catalog_fetch::state_in(bench.home.path());
    let launcher = Launcher::new(
        sandbox,
        handle,
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;
    let read = runtime.block_on(launcher.account(
        &agent,
        &control,
        dialect.as_mut(),
        Account::Version,
        &|_| {},
    ))?;
    match read {
        Reading::Version(version) => Ok(version.as_str().to_owned()),
        _ => Err("the agent did not say its version".into()),
    }
}

/// A sandbox whose processes run as the local one's do, but which can never
/// confirm that one of them stopped, so that a run made on it is one that may
/// still be running.
struct Stubborn(Arc<dyn SandboxService>);

impl SandboxService for Stubborn {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.0.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let session: Box<dyn SandboxSession> =
                Box::new(StubbornSession(self.0.prepare(request).await?));
            Ok(session)
        })
    }
}

/// Its session.
struct StubbornSession(Box<dyn SandboxSession>);

impl SandboxSession for StubbornSession {
    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        self.0.materialize()
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let launch: Box<dyn SandboxLaunch> =
                Box::new(StubbornLaunch(self.0.stage(command).await?));
            Ok(launch)
        })
    }
}

/// Its staged launch.
struct StubbornLaunch(Box<dyn SandboxLaunch>);

impl SandboxLaunch for StubbornLaunch {
    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let process: Box<dyn SandboxProcess> =
                Box::new(StubbornProcess(self.0.release().await?));
            Ok(process)
        })
    }
}

/// Its process, which never reads as ended and whose stop always fails.
struct StubbornProcess(Box<dyn SandboxProcess>);

impl SandboxProcess for StubbornProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.0.take_stdin()
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
        self.0.take_async_stdin()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.0.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.0.take_stderr()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Ok(None)
    }

    fn stop(&mut self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async { Err(std::io::Error::other("the scope did not answer")) })
    }

    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.0.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.0.violation()
    }
}

/// A sign-in that would start the agent is refused beside a run that could
/// not be confirmed stopped, for the instance that made it and for every other
/// instance on its profile, before the route is asked, a setting is written or
/// the agent is started; an instance on another profile is not refused for it.
#[test]
fn a_sign_in_beside_a_run_not_confirmed_stopped_is_refused_for_every_instance_on_its_profile()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-unstopped")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    // The first start is the run left running, the second the sign-in of the
    // instance on another profile.
    let program = fake_peer::install(
        &peer,
        "antigravity",
        &Peer::of([acp::opening(&business_initialized()?), business_reply()?]),
    )?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let (steps, confirmed) =
        bench.ran(Arc::new(Stubborn(Arc::clone(&local))), &program, &runtime)?;
    assert!(!confirmed, "the run's stop was confirmed");
    assert_eq!(fake_peer::played(&peer), 1, "the run was not started");
    let user = fs::read(bench.user())?;
    let kept = credentials(&bench);

    // The conversation's route has no yes, so that a sign-in that went on
    // would ask for one.
    for instance in ["google", "spare"] {
        let mut front = answering(&bench.server, None);
        let refused = bench.ask_with(
            (sign_in(instance, "oauth-business")?, &mut front),
            (Arc::clone(&local), false),
            (false, Some(&steps)),
            &runtime,
        )?;
        let Err(AgentsError::Unstopped(held)) = &refused else {
            return Err(shape(&refused).into());
        };
        assert!(
            held.iter().any(|held| &**held == "google"),
            "the refusal does not name the instance with the run"
        );
        assert!(
            held.iter().all(|held| &**held != "apart"),
            "the refusal names an instance on another profile"
        );
        let said = refused
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            said.contains(&crucible_external_harnesses::Refusal::Unstopped.to_string()),
            "the refusal does not say what a refusal beside a run says"
        );
        assert!(front.put.is_empty(), "a route was asked");
        assert!(
            !bench.agy_profile().join("antigravity-acp").exists(),
            "the agent's settings were written for a refused sign-in"
        );
        assert_eq!(fake_peer::played(&peer), 1, "the agent was started");
        assert!(
            fs::read(bench.user())? == user && credentials(&bench) == kept,
            "a refused sign-in wrote the configuration or the credentials"
        );
    }

    let mut front = answering(&bench.server, None);
    let apart = bench.ask_with(
        (sign_in("apart", "oauth-business")?, &mut front),
        (Arc::clone(&local), false),
        (true, Some(&steps)),
        &runtime,
    )?;
    assert!(
        matches!(
            &apart,
            Ok(Outcome::SignedIn {
                by: Signed::Agent,
                ..
            })
        ),
        "{}",
        shape(&apart)
    );
    assert_eq!(fake_peer::played(&peer), 2, "the agent was not started");
    Ok(())
}

/// The sign-in a run not confirmed stopped refuses goes ahead where the run
/// beside it was confirmed stopped.
#[test]
fn a_sign_in_goes_ahead_beside_a_run_confirmed_stopped() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-stopped")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    let program = fake_peer::install(
        &peer,
        "antigravity",
        &Peer::of([acp::opening(&business_initialized()?), business_reply()?]),
    )?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let (steps, confirmed) = bench.ran(Arc::clone(&local), &program, &runtime)?;
    assert!(confirmed, "the run's stop was not confirmed");
    assert_eq!(fake_peer::played(&peer), 1, "the run was not started");

    let mut front = answering(&bench.server, None);
    let signed = bench.ask_with(
        (sign_in("google", "oauth-business")?, &mut front),
        (local, false),
        (true, Some(&steps)),
        &runtime,
    )?;
    assert!(
        matches!(
            &signed,
            Ok(Outcome::SignedIn {
                by: Signed::Agent,
                ..
            })
        ),
        "{}",
        shape(&signed)
    );
    assert_eq!(fake_peer::played(&peer), 2, "the agent was not started");
    Ok(())
}

/// A launcher lent with no steps knows nothing of the runs beside a sign-in
/// that would start the agent, so the sign-in is refused as one that has no
/// conversation, and starts, writes and asks nothing.
#[test]
fn a_sign_in_with_a_launcher_and_no_steps_is_refused_before_anything_is_started()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-no-steps")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    let program = business_peer(&peer)?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let user = fs::read(bench.user())?;

    let mut front = answering(&bench.server, None);
    let refused = bench.ask_with(
        (sign_in("google", "oauth-business")?, &mut front),
        (local, false),
        (false, None),
        &runtime,
    )?;

    assert!(
        matches!(refused, Err(AgentsError::Conversation(_))),
        "{}",
        shape(&refused)
    );
    assert!(front.put.is_empty(), "a route was asked");
    assert!(
        !bench.agy_profile().join("antigravity-acp").exists(),
        "the agent's settings were written for a refused sign-in"
    );
    assert!(fake_peer::heard(&peer).is_empty(), "the agent was started");
    assert!(
        fs::read(bench.user())? == user,
        "a refused sign-in wrote the configuration"
    );
    Ok(())
}

/// A sign-in by a key already kept starts the agent where a conversation lent
/// its launcher, so it is refused beside a run not confirmed stopped as one the
/// agent carries out is: before the route is asked or the agent is started,
/// and with the kept key left as it was.
#[test]
fn a_kept_key_is_not_signed_in_beside_a_run_not_confirmed_stopped_where_a_launcher_is_lent()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-unstopped-kept")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    // The one start is the run left running.
    let program = fake_peer::install(
        &peer,
        "antigravity",
        &Peer::of([acp::opening(&business_initialized()?), business_reply()?]),
    )?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let (steps, confirmed) =
        bench.ran(Arc::new(Stubborn(Arc::clone(&local))), &program, &runtime)?;
    assert!(!confirmed, "the run's stop was confirmed");
    assert_eq!(fake_peer::played(&peer), 1, "the run was not started");
    keep_key(
        &bench.home,
        "google",
        "gemini-api-key",
        Typed::new(KEY.to_owned()),
    )?;
    let user = fs::read(bench.user())?;
    let kept = credentials(&bench);
    assert!(
        kept.contains("\"antigravity@gemini-api-key\""),
        "the key was not kept under the method"
    );

    // The conversation's route has no yes, so that a sign-in that went on
    // would ask for one.
    let mut front = answering(&bench.server, None);
    let refused = bench.ask_with(
        (sign_in("google", "gemini-api-key")?, &mut front),
        (local, false),
        (false, Some(&steps)),
        &runtime,
    )?;

    let Err(AgentsError::Unstopped(held)) = &refused else {
        return Err(shape(&refused).into());
    };
    assert!(
        held.iter().any(|held| &**held == "google"),
        "the refusal does not name the instance with the run"
    );
    assert!(front.put.is_empty(), "a route was asked");
    assert!(
        !bench.agy_profile().join("antigravity-acp").exists(),
        "the agent's settings were written for a refused sign-in"
    );
    assert_eq!(fake_peer::played(&peer), 1, "the agent was started");
    assert!(
        fs::read(bench.user())? == user && credentials(&bench) == kept,
        "a refused sign-in wrote the configuration or the credentials"
    );
    Ok(())
}

/// A sign-in that would start the agent is refused beside a live run of the
/// instance or of any other instance on its profile, by a method the agent
/// carries out and by a kept key alike, before the route is asked, a setting or
/// a key is written or the agent is started; the refusal ends nothing the
/// conversation runs, an instance on another profile is not held back by it,
/// and once that run has ended the same sign-ins go ahead.
#[test]
fn a_sign_in_beside_a_live_run_is_refused_for_every_instance_on_its_profile_until_it_ends()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-live")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    // The first start is the run left live, the second the sign-in once it
    // has ended; a start beside it would take that one.
    let program = fake_peer::install(
        &peer,
        "antigravity",
        &Peer::of([acp::opening(&business_initialized()?), business_reply()?]),
    )?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let (steps, run) = bench.running(Arc::clone(&local), &program, &runtime)?;
    assert!(started(&peer, 1), "the run was not started");
    keep_key(
        &bench.home,
        "google",
        "gemini-api-key",
        Typed::new(KEY.to_owned()),
    )?;
    let user = fs::read(bench.user())?;
    let kept = credentials(&bench);
    assert!(
        kept.contains("\"antigravity@gemini-api-key\""),
        "the key was not kept under the method"
    );

    for instance in ["google", "spare"] {
        for method in ["oauth-business", "gemini-api-key"] {
            // The conversation's route has no yes, so that a sign-in that went
            // on would ask for one.
            let mut front = answering(&bench.server, None);
            let refused = bench.ask_with(
                (sign_in(instance, method)?, &mut front),
                (Arc::clone(&local), false),
                (false, Some(&steps)),
                &runtime,
            )?;
            let Err(AgentsError::Running(live)) = &refused else {
                return Err(shape(&refused).into());
            };
            assert!(
                live.iter().any(|live| &**live == "google"),
                "the refusal does not name the instance with the run"
            );
            let said = refused
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default();
            assert!(
                said.contains("must end first"),
                "the refusal does not say the runs must end first"
            );
            assert!(front.put.is_empty(), "a route was asked");
            assert!(
                !bench.agy_profile().join("antigravity-acp").exists(),
                "the agent's settings were written for a refused sign-in"
            );
            assert_eq!(fake_peer::played(&peer), 1, "a second process was started");
            assert!(
                fs::read(bench.user())? == user && credentials(&bench) == kept,
                "a refused sign-in wrote the configuration or the credentials"
            );
        }
    }

    // An instance on another profile is not held back by the run, which is
    // still live: it gets past the check to the route's question, and a route
    // with no yes leaves it undecided, with nothing started. The request asks
    // no permission question of the front, so none is put to it; the outcome
    // is what shows the question was reached.
    let mut front = answering(&bench.server, None);
    let apart = bench.ask_with(
        (sign_in("apart", "oauth-business")?, &mut front),
        (Arc::clone(&local), false),
        (false, Some(&steps)),
        &runtime,
    )?;
    assert!(
        matches!(&apart, Ok(Outcome::Undecided)),
        "{}",
        shape(&apart)
    );
    assert_eq!(fake_peer::played(&peer), 1, "a second process was started");

    let ended = runtime.block_on(async move { run.close().await.is_ok() });
    assert!(ended, "the run's stop was not confirmed");
    let mut front = answering(&bench.server, None);
    let signed = bench.ask_with(
        (sign_in("google", "oauth-business")?, &mut front),
        (local, false),
        (true, Some(&steps)),
        &runtime,
    )?;
    assert!(
        matches!(
            &signed,
            Ok(Outcome::SignedIn {
                by: Signed::Agent,
                ..
            })
        ),
        "{}",
        shape(&signed)
    );
    assert_eq!(fake_peer::played(&peer), 2, "the agent was not started");
    Ok(())
}

/// A sign-in is started on its instance's own control, so one whose stop could
/// not be confirmed leaves the instance holding a run that may still be running:
/// the next sign-in of the instance and of any instance on its profile is
/// refused as a run not confirmed stopped is, before the agent is started.
#[test]
fn a_sign_in_whose_stop_could_not_be_confirmed_leaves_the_profile_unstopped() -> Result<(), Failed>
{
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-own-unstopped")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    // The first start is the sign-in left running; a start after it would
    // take another reply.
    let program = fake_peer::install(
        &peer,
        "antigravity",
        &Peer::of([business_reply()?, business_reply()?, business_reply()?]),
    )?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let steps = bench.steps(Arc::clone(&local), false, true, &runtime)?;

    // Whatever it reads, it is the stop of its process that is under test.
    let mut front = answering(&bench.server, None);
    let _read = bench.ask_with(
        (sign_in("google", "oauth-business")?, &mut front),
        (Arc::new(Stubborn(Arc::clone(&local))), false),
        (true, Some(&steps)),
        &runtime,
    )?;
    assert_eq!(fake_peer::played(&peer), 1, "the sign-in was not started");
    let user = fs::read(bench.user())?;
    let kept = credentials(&bench);

    // The sandbox of these can confirm a stop, so that a refusal is the
    // instance's and not the sandbox's.
    for instance in ["google", "spare"] {
        let mut front = answering(&bench.server, None);
        let refused = bench.ask_with(
            (sign_in(instance, "oauth-business")?, &mut front),
            (Arc::clone(&local), false),
            (true, Some(&steps)),
            &runtime,
        )?;
        let Err(AgentsError::Unstopped(held)) = &refused else {
            return Err(shape(&refused).into());
        };
        assert!(
            held.iter().any(|held| &**held == "google"),
            "the refusal does not name the instance with the run"
        );
        assert!(front.put.is_empty(), "a route was asked");
        assert_eq!(fake_peer::played(&peer), 1, "the agent was started");
        assert!(
            fs::read(bench.user())? == user && credentials(&bench) == kept,
            "a refused sign-in wrote the configuration or the credentials"
        );
    }
    Ok(())
}

/// A client that says yes to the agent's route, and starts a run of `google`
/// while the question is out, as a conversation on that agent would meanwhile.
struct Beginning {
    runs: Runs,
    run: Option<Box<dyn Backend>>,
}

impl Front for Beginning {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            if !matches!(shown, Shown::Warning(_)) {
                return None;
            }
            self.run = self.runs.launch(Mode::Ask, None, None, None).await.ok();
            Some(Decision::Accepted { id: pending.id() })
        })
    }

    fn refused(&mut self, _refusal: Refusal) {}
}

/// A run that begins while the agent's route is being asked is not started
/// beside: the sign-in is checked again once the route has its yes, and is
/// refused naming the instance with the run, before a setting is written or the
/// agent is started.
#[test]
fn a_run_that_begins_while_the_route_is_asked_is_not_signed_in_beside() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-sign-in-asking")?;
    bench.server.catalog(1, &[], &json!([]))?;
    let peer = bench.tree.0.join("work").join("peer");
    // The one start is the run begun while the route is asked; a start beside
    // it would take the second reply.
    let program = fake_peer::install(
        &peer,
        "antigravity",
        &Peer::of([acp::opening(&business_initialized()?), business_reply()?]),
    )?;
    bench.antigravity_sharing(&program)?;
    let local: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let steps = bench.steps(Arc::clone(&local), false, true, &runtime)?;
    let mut front = Beginning {
        runs: bench.runs_of_google(&steps, &program)?,
        run: None,
    };
    let asked = Request::new(
        Capabilities::none()
            .with(Capability::Install)
            .with(Capability::Permissions),
        Correlation::new(1),
        sign_in("spare", "oauth-business")?,
    );

    // The conversation's route has no yes, so that the sign-in asks for one.
    let refused = bench.ask_requested(
        (asked, &mut front),
        (Arc::clone(&local), false),
        (false, Some(&steps)),
        &runtime,
    )?;

    let Err(AgentsError::Running(live)) = &refused else {
        return Err(shape(&refused).into());
    };
    assert!(
        live.iter().any(|live| &**live == "google"),
        "the refusal does not name the instance with the run"
    );
    let run = front
        .run
        .take()
        .ok_or("no run began while the route was asked")?;
    assert!(started(&peer, 1), "the run was not started");
    assert!(
        !bench.agy_profile().join("antigravity-acp").exists(),
        "the agent's settings were written for a refused sign-in"
    );
    let ended = runtime.block_on(async move { run.close().await.is_ok() });
    assert!(ended, "the run's stop was not confirmed");
    Ok(())
}

/// A client of `/login` that picks the next of `picks` at each account
/// question it is put, declining once they run out, and keeps each question.
struct Connecting {
    picks: std::collections::VecDeque<&'static str>,
    put: Vec<Pending>,
}

impl Front for Connecting {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            self.put.push(pending.clone());
            let id = pending.id();
            Some(match self.picks.pop_front() {
                Some(choice) => Decision::Picked {
                    id,
                    choice: Said::new(choice).ok()?,
                },
                None => Decision::Declined { id },
            })
        })
    }

    fn refused(&mut self, _refusal: Refusal) {}
}

/// A `/login` connect naming no instance, asked at `bench`'s home by a client
/// claiming every capability that answers as `front` does; and how many
/// shipping agents its conversation lists before any other.
fn connecting(
    bench: &Bench,
    front: &mut Connecting,
) -> Result<(crucible_client_api::Outcome, usize), Failed> {
    let mut conversation = super::conversation(&bench.tree, super::Script::new(Vec::new()), false)?
        .consenting(Consent::new(Routes::production()));
    let shipping = conversation.agents().shipping().count();
    let mut desk = super::Desk::new(&bench.tree, &[])?;
    desk.settings = super::written(&bench.tree)?;
    let (workspace, sessions) = (bench.tree.workspace()?, bench.tree.sessions());
    let at = crucible_app::client::Desk {
        switching: desk.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads: |_| false,
        environment: super::unset,
        notes: |_| Err(ErrorCode::InvalidArgument.into()),
    };
    let request = Request::new(
        Capabilities::every(),
        Correlation::new(1),
        Command::Connect { instance: None },
    );
    let performed = on_runtime(crucible_app::client::account(
        &mut conversation,
        &request,
        &at,
        front,
    ))?;
    Ok((performed.response(&request).outcome, shipping))
}

/// The catalog agents a `/login` connect lists after the shipping ones are
/// the agents the door lists, in the door's order, for the one trust state.
#[test]
fn a_login_connect_lists_the_catalog_agents_the_door_lists_in_its_order() -> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, Outcome as Answered};

    let bench = Bench::new("agents-door-login-rows")?;
    bench.server.publish(
        1,
        &[("1.0.0", ONE)],
        &[entry("claude", "9.0.0", ONE), entry("kiro", "2.0.0", TWO)],
    );
    let mut listing = answering(&bench.server, None);
    let (outcome, _) = bench.ask(Command::AgentCatalog, &mut listing, false)?;
    let Ok(Outcome::Catalog { agents, .. }) = &outcome else {
        return Err(format!("a listing, not {}", shape(&outcome)).into());
    };
    let door: Vec<&str> = agents.iter().map(|listed| listed.agent.as_str()).collect();
    let mut front = Connecting {
        picks: std::collections::VecDeque::new(),
        put: Vec::new(),
    };

    let (answered, shipping) = connecting(&bench, &mut front)?;

    assert_eq!(answered, Answered::Login(LoginOutcome::Undecided));
    let Some(Pending::Account { choices, .. }) = front.put.first() else {
        return Err("no agent was asked for".into());
    };
    let listed: Vec<&str> = choices
        .iter()
        .skip(shipping)
        .map(|choice| choice.name.as_str())
        .collect();
    let (cancel, rows) = listed.split_last().ok_or("no choice was offered")?;
    assert_eq!(*cancel, "Cancel");
    assert_eq!(rows, door.as_slice());
    assert_eq!(door, ["goose", "kiro"]);
    Ok(())
}

/// The move a `/login` connect ends on, sent to the door as its command, is
/// one the door puts to the client: the install of the release the connect
/// named for an Antigravity not installed, which a no leaves undone.
#[test]
fn the_door_puts_the_install_a_login_connect_ended_on() -> Result<(), Failed> {
    use crucible_client_api::{AgentAction, LoginOutcome, Outcome as Answered};

    let bench = Bench::new("agents-door-login-install")?;
    let mut front = Connecting {
        picks: ["Antigravity", "Install"].into(),
        put: Vec::new(),
    };
    let (answered, _) = connecting(&bench, &mut front)?;
    let Answered::Login(LoginOutcome::AgentChosen { action }) = answered else {
        return Err("the connect ended on no move".into());
    };
    assert!(
        matches!(&action, AgentAction::Install { agent, .. } if agent.as_str() == "antigravity"),
        "the connect ended on a move other than Antigravity's install"
    );
    let mut declining = answering(&bench.server, None);

    let (outcome, _) = bench.ask(action.command(), &mut declining, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let put: Vec<bool> = declining
        .put
        .iter()
        .map(|(pending, _)| matches!(pending, Pending::Install { .. }))
        .collect();
    assert_eq!(put, [true]);
    Ok(())
}
