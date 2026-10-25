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
use crucible_app::backends::{AgentRow, CatalogRow, Registry};
use crucible_app::catalog_fetch::{self, Source};
use crucible_app::client::agents::{
    AgentsError, How, Outcome, Shelf, Signed, Typed, Watching, keep_key, run,
};
use crucible_app::client::{Front, Shown};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::{Agent, Launcher, Steps, Unlaunched};
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, ErrorCode, InstallProgress, Name,
    Pending, PendingId, Refusal, Request, Said,
};
use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::catalog::fixture::signed;
use crucible_external_harnesses::catalog::{Callback, Catalog, Held, SNAPSHOT, Uncalled, Unlinked};
use crucible_external_harnesses::fake_peer::{self, Reply as Played, Script as Peer, scripts::acp};
use crucible_external_harnesses::{Account, Control, Reading};
use crucible_http::ProxyEnv;
use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::{
    SandboxCapability, SandboxEnablement, SandboxFeature, SandboxPolicy, SandboxService,
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

    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
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
        "{installed:?}"
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
        "{updated:?}"
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
        "{rolled:?}"
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
        "{installed:?}"
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
        "{updated:?}"
    );
    assert_eq!(
        bench.executable()?,
        bench.program("2.0.0").to_string_lossy()
    );
    assert_eq!(fs::read(bench.program("2.0.0"))?, TWO);
    // No launcher was lent, so nothing was started, and the outcome says why.
    assert!(
        matches!(&updated, Ok(Outcome::Moved { unsmoked: Some(why), .. }) if why.contains("launcher")),
        "{updated:?}"
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
    assert!(matches!(updated, Ok(Outcome::Undecided)), "{updated:?}");
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
    assert!(matches!(installed, Ok(Outcome::Undecided)), "{installed:?}");

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
        "{installed:?}"
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
        "{refused:?}"
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
        "{installed:?}"
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
    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
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
    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
    assert!(!bench.program("0.9.0").exists());
    assert!(!bench.program("2.0.0").exists());

    // The revoked release itself is refused, asked or not.
    let mut front = answering(&bench.server, None);
    let (refused, _) = bench.ask(install("2.0.0")?, &mut front, false)?;
    assert!(
        matches!(refused, Err(AgentsError::Unoffered { .. })),
        "{refused:?}"
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
    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
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
        "{updated:?}"
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
    assert!(matches!(rolled, Ok(Outcome::Undecided)), "{rolled:?}");

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
    assert!(matches!(removed, Ok(Outcome::Undecided)), "{removed:?}");
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
        "{outcome:?}"
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
        "{outcome:?}"
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
        return Err(format!("a listing, not {outcome:?}").into());
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
            "{outcome:?}"
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
        "{installed:?}"
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
        "{refused:?}"
    );
    assert!(front.put.is_empty());
    assert!(bench.program("1.0.0").is_file());

    bench.configure(Path::new("goose"), &json!({}))?;
    bench.server.forget();
    let (removed, _) = bench.ask(remove()?, &mut front, false)?;

    assert!(
        matches!(&removed, Ok(Outcome::Removed { versions, .. }) if versions.len() == 1),
        "{removed:?}"
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

    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
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

    assert!(matches!(outcome, Err(AgentsError::TooLarge)), "{outcome:?}");
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
            "{path}: {outcome:?}"
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
        "{outcome:?}"
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
        "{first:?}"
    );
    bench.server.forget();

    let mut front = answering(&bench.server, Some("Install"));
    let (again, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&again, Ok(Outcome::Moved { downloaded: false, version, .. }) if &**version == "1.0.0"),
        "{again:?}"
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
        "{refused:?}"
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

    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
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
        "{installed:?}"
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
        "{updated:?}"
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
        "{installed:?}"
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
        "{whole:?}"
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
        "{differs:?}"
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
        "{installed:?}"
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
        "{current:?}"
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
        return Err(format!("a listing, not {outcome:?}").into());
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
            "{version}: {installed:?}"
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
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;
    let ended = runtime
        .block_on(launcher.launch(&agent, &control, dialect.as_mut(), None))
        .map(drop);
    let steps = Steps::new(
        Launcher::new(sandbox, handle, &policy, &consent, &state),
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
        "{installed:?}"
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
        "{listed:?}"
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
        "{installed:?}"
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
    /// use.
    fn ask_smoking(
        &self,
        (command, front): (Command, &mut Answering<'_>),
        (confined, yes): (bool, bool),
        release: &Value,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let row = row_of(release)?;
        let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
        if yes {
            consent.give("agent:goose");
        }
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let handle = runtime.handle().clone();
        let sandbox: Arc<dyn SandboxService> =
            Arc::new(LocalSandbox::new().watching_on(handle.clone()));
        let state = catalog_fetch::state_in(self.home.path());
        let launcher = Launcher::new(sandbox, handle, &policy, &consent, &state);
        let lent = Shelf {
            catalog: self.server.source(),
            home: &self.home,
            smoke: Some(&launcher),
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
            other => Err(format!("installed before: {other:?}").into()),
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
        "{updated:?}"
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
    assert!(failed.contains("9.9.9"), "{failed:?}");
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

    assert!(updated.is_err(), "{updated:?}");
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
            "{updated:?}"
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
        return Err(format!("installed: {installed:?}").into());
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
    let launcher = Launcher::new(sandbox, handle, &policy, &consent, &state);
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
        "{installed:?}"
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
        "{listed:?}"
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
    let launcher = Launcher::new(sandbox, handle, &policy, &consent, &state);
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
        "{listed:?}"
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
            Ok(Outcome::SignedIn { instance, method, by: Signed::Kept })
                if &**instance == "work" && &**method == "api-key"
        ),
        "the kept key was not adopted"
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
        "{browser:?}"
    );
    assert!(
        matches!(terminal, Err(AgentsError::Unsupported(_))),
        "{terminal:?}"
    );
    assert!(
        matches!(unknown, Err(AgentsError::NoMethod { .. })),
        "{unknown:?}"
    );
    assert!(
        matches!(unconfigured, Ok(Outcome::Refused(refusal)) if refusal.code() == ErrorCode::InvalidArgument),
        "{unconfigured:?}"
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
                .say(acp::response(1, &json!({}))),
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
    assert!(matches!(unasked, Ok(Outcome::Undecided)), "{unasked:?}");
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
        "{signed:?}"
    );
    let heard = fake_peer::heard(&made.0).join("\n");
    assert!(
        heard.contains("\"authenticate\"") && heard.contains("\"methodId\":\"browser\""),
        "{heard}"
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
        (format!("code={code}"), Uncalled::Stateless),
        (
            format!("?code={code}&state=s3cret-state&state=s3cret-state"),
            Uncalled::Mismatched,
        ),
        ("state=s3cret-state".to_owned(), Uncalled::Codeless),
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
        Callback::checked(format!("code={code}&state=s"), &unstated).err(),
        Some(Uncalled::Stateless)
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
    let vertex = ask("vertex-ai")?;
    let elsewhere = ask("application-default")?;

    assert!(
        matches!(&business, Err(AgentsError::Needs { variables, .. })
            if variables == &["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_LOCATION"]),
        "{business:?}"
    );
    assert!(
        matches!(personal, Err(AgentsError::Conversation(_))),
        "{personal:?}"
    );
    assert!(
        matches!(
            gemini,
            Ok(Outcome::SignedIn {
                by: Signed::Kept,
                ..
            })
        ),
        "the Gemini key was not adopted"
    );
    assert!(
        matches!(vertex, Err(AgentsError::Unstored { .. })),
        "{vertex:?}"
    );
    assert!(
        matches!(elsewhere, Err(AgentsError::NoMethod { .. })),
        "{elsewhere:?}"
    );
    assert!(credentials(&bench).contains("\"antigravity@gemini-api-key\""));
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
        "{installed:?}"
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
    let launcher = Launcher::new(sandbox, handle, &policy, &consent, &state);
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
