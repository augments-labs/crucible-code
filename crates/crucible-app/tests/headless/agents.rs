//! The agents door, driven with no terminal and no conversation: nothing is
//! fetched before the answer, a no changes nothing, a download goes only where
//! it is listed and a cancelled one commits nothing, and an update and a
//! rollback move the instances' `executable` between versions.
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
use std::time::Duration;

use crucible_app::agent_download::DownloadError;
use crucible_app::catalog_fetch::Source;
use crucible_app::client::agents::{AgentsError, How, Outcome, Shelf, Watching, run};
use crucible_app::client::{Front, Shown};
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, ErrorCode, InstallProgress, Name,
    Pending, PendingId, Refusal, Request, Said,
};
use crucible_external_harnesses::catalog::SNAPSHOT;
use crucible_external_harnesses::catalog::fixture::signed;
use crucible_http::ProxyEnv;
use crucible_runtime::{BoxFuture, Cancel};
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
    let edited = serde_json::to_vec_pretty(&json!({
        "externalAgents": {
            "work": {"driver": "catalog", "agent": "goose", "executable": "/usr/local/bin/goose"}
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
