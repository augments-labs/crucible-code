//! An artifact arrives only as its pin, from the places listed for it, and a
//! vendor that fails to answer is stood in for by the mirror, while one that
//! answers with other bytes is not.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_external_harnesses::catalog::{Artifact, Files, Format};
use crucible_external_harnesses::store::Fetched;
use crucible_http::ProxyEnv;
use crucible_runtime::Cancel;
use sha2::{Digest as _, Sha256};

use super::*;

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
    /// A 200 promising twice these bytes, of which it sends these and closes.
    Short(Vec<u8>),
}

/// A host on this machine answering by path, remembering what it was asked.
struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
    asked: Arc<Mutex<Vec<String>>>,
    done: Arc<AtomicBool>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().expect("an address").port();
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
        Self {
            port,
            routes,
            asked,
            done,
        }
    }

    fn serve(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_owned(), reply);
    }

    fn at(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    /// The catalog's place on this host, which lists only this host.
    fn source(&self) -> Source {
        let listed: &'static str = Box::leak(format!("127.0.0.1:{}", self.port).into_boxed_str());
        Source {
            base: self.at("/catalog/"),
            keys: crucible_external_harnesses::catalog::KEYS,
            hosts: Box::leak(Box::new([listed])),
            proxy: ProxyEnv::default(),
        }
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
    asked.lock().unwrap().push(path.clone());
    let reply = routes.lock().unwrap().get(&path).cloned();
    let (status, extra, body, promised) = match reply {
        None => ("404 Not Found", String::new(), Vec::new(), 0),
        Some(Reply::Body(body)) => {
            let promised = body.len();
            ("200 OK", String::new(), body, promised)
        }
        Some(Reply::Redirect(to)) => ("302 Found", format!("location: {to}\r\n"), Vec::new(), 0),
        Some(Reply::Short(body)) => {
            let promised = body.len() * 2;
            ("200 OK", String::new(), body, promised)
        }
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

fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(work)
}

fn digest(bytes: &[u8]) -> Box<str> {
    hex(&Sha256::digest(bytes)).into()
}

/// The program `bytes`, published bare at `url` and copied to `mirror`.
fn bare(bytes: &[u8], url: String, mirror: Option<String>) -> Artifact {
    Artifact {
        url: url.into(),
        mirror: mirror.map(Into::into),
        sha256: digest(bytes),
        size: u64::try_from(bytes.len()).expect("a size"),
        format: Format::Bare,
        unpacked: u64::try_from(bytes.len()).expect("a size"),
        decompressed: u64::try_from(bytes.len()).expect("a size"),
        files: Files::Listed(Vec::new()),
        kept: Vec::new(),
        replaced_by: None,
    }
}

const PROGRAM: &[u8] = b"the reviewed agent program";

/// The download hosts an entry lists for its target in these tests: a name,
/// which this host is not.
fn hosts() -> Vec<Box<str>> {
    vec!["downloads.example.com".into()]
}

/// A file a download lands in, in a directory of its own removed with it.
struct Landed(std::path::PathBuf);

impl Landed {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "crucible-download-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        Self(directory)
    }

    fn file(&self) -> std::fs::File {
        std::fs::File::create(self.0.join("download")).expect("a download file")
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.0.join("download")).unwrap_or_default()
    }
}

impl Drop for Landed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `artifact` downloaded under `server`'s catalog with `hosts` listed, told of
/// progress through `progress`, and what landed.
fn downloading(
    server: &Server,
    hosts: &[Box<str>],
    artifact: &Artifact,
    progress: &mut (dyn FnMut(u64, u64) + Send),
    cancel: &Cancel,
) -> (Result<Fetched, DownloadError>, Vec<u8>) {
    let source = server.source();
    let landed = Landed::new();
    let mut file = landed.file();
    let fetched = run(super::artifact(
        &source,
        hosts,
        artifact,
        Landing {
            file: &mut file,
            progress,
        },
        cancel,
    ));
    drop(file);
    (fetched, landed.bytes())
}

fn fetched(server: &Server, artifact: &Artifact) -> (Result<Fetched, DownloadError>, Vec<u8>) {
    downloading(server, &hosts(), artifact, &mut |_, _| {}, &Cancel::new())
}

#[test]
fn a_vendor_that_answers_404_is_stood_in_for_by_the_mirror_with_the_same_bytes() {
    let server = Server::start();
    server.serve("/mirror/agent", Reply::Body(PROGRAM.to_vec()));
    let artifact = bare(
        PROGRAM,
        server.at("/vendor/agent"),
        Some(server.at("/mirror/agent")),
    );

    let (fetched, landed) = fetched(&server, &artifact);
    let fetched = fetched.expect("the mirror's copy");

    assert_eq!(landed, PROGRAM);
    assert_eq!(&*fetched.url, server.at("/mirror/agent"));
    assert_eq!(&*fetched.sha256, &*artifact.sha256);
    assert_eq!(server.asked(), ["/vendor/agent", "/mirror/agent"]);
}

#[test]
fn a_vendor_that_answers_other_bytes_is_refused_and_the_mirror_is_not_asked() {
    let server = Server::start();
    let mut other = PROGRAM.to_vec();
    if let Some(first) = other.first_mut() {
        *first ^= 1;
    }
    server.serve("/vendor/agent", Reply::Body(other));
    server.serve("/mirror/agent", Reply::Body(PROGRAM.to_vec()));
    let artifact = bare(
        PROGRAM,
        server.at("/vendor/agent"),
        Some(server.at("/mirror/agent")),
    );

    let (refused, _) = fetched(&server, &artifact);

    assert!(matches!(refused, Err(DownloadError::Digest)), "{refused:?}");
    assert_eq!(server.asked(), ["/vendor/agent"]);

    // Nor for a body longer than the pin.
    let mut longer = PROGRAM.to_vec();
    longer.push(b'!');
    server.serve("/vendor/agent", Reply::Body(longer));
    let (refused, _) = fetched(&server, &artifact);
    assert!(
        matches!(refused, Err(DownloadError::Size { .. })),
        "{refused:?}"
    );
    assert_eq!(server.asked(), ["/vendor/agent", "/vendor/agent"]);
}

#[test]
fn a_redirect_to_a_place_not_listed_is_refused_and_nothing_is_sent_there() {
    let server = Server::start();
    let elsewhere = Server::start();
    elsewhere.serve("/agent", Reply::Body(PROGRAM.to_vec()));
    server.serve("/vendor/agent", Reply::Redirect(elsewhere.at("/agent")));
    let artifact = bare(PROGRAM, server.at("/vendor/agent"), None);

    let (refused, _) = fetched(&server, &artifact);

    assert!(
        matches!(refused, Err(DownloadError::Redirect)),
        "{refused:?}"
    );
    assert!(elsewhere.asked().is_empty());

    // A listed place is followed, and the receipt keeps the pinned address
    // with the hosts it came through.
    server.serve(
        "/vendor/agent",
        Reply::Redirect(server.at("/storage/agent?signature=short-lived")),
    );
    server.serve(
        "/storage/agent?signature=short-lived",
        Reply::Body(PROGRAM.to_vec()),
    );
    let (fetched, landed) = fetched(&server, &artifact);
    let fetched = fetched.expect("followed");
    assert_eq!(landed, PROGRAM);
    assert_eq!(&*fetched.url, server.at("/vendor/agent"));
    assert_eq!(fetched.hosts, [Box::<str>::from("127.0.0.1")]);
}

#[test]
fn a_download_host_that_is_an_address_or_every_host_is_refused_before_a_request() {
    let server = Server::start();
    server.serve("/vendor/agent", Reply::Body(PROGRAM.to_vec()));
    let artifact = bare(PROGRAM, server.at("/vendor/agent"), None);

    for listed in [
        "10.0.0.1",
        "[::1]",
        "0x7f.1",
        "*",
        "*.com",
        "user@example.com:x",
    ] {
        let (refused, _) = downloading(
            &server,
            &[listed.into()],
            &artifact,
            &mut |_, _| {},
            &Cancel::new(),
        );

        assert!(
            matches!(refused, Err(DownloadError::Host(ref host)) if &**host == listed),
            "{listed}: {refused:?}"
        );
    }
    assert!(server.asked().is_empty());
}

/// An address a refusal names is quoted and cut, however long the catalog or
/// a lock wrote it, so no refusal carries the whole of it.
#[test]
fn an_address_a_refusal_names_is_cut_however_long_it_is() {
    let server = Server::start();
    let source = server.source();
    let long = "a".repeat(32 * 1024);

    let unlisted = bare(PROGRAM, format!("https://elsewhere.example/{long}"), None);
    let refused = reachable(&source, &hosts(), &unlisted);
    assert!(matches!(refused, Err(DownloadError::Unlisted(_))));
    let said = refused
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        said.chars().count() < 512,
        "{} characters",
        said.chars().count()
    );
    assert!(said.contains('…'), "the address is said to be cut");

    let unread = bare(PROGRAM, format!("not an address {long}"), None);
    let refused = reachable(&source, &hosts(), &unread);
    assert!(matches!(refused, Err(DownloadError::Address(_))));
    let said = refused
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        said.chars().count() < 512,
        "{} characters",
        said.chars().count()
    );
    assert!(said.contains('…'), "the address is said to be cut");
    assert!(server.asked().is_empty());
}

#[test]
fn an_address_or_mirror_on_a_host_not_listed_is_refused_before_a_request() {
    let server = Server::start();
    server.serve("/vendor/agent", Reply::Body(PROGRAM.to_vec()));
    let unlisted = format!("http://localhost:{}/vendor/agent", server.port);
    let source = server.source();

    for artifact in [
        bare(PROGRAM, unlisted.clone(), None),
        bare(PROGRAM, server.at("/vendor/agent"), Some(unlisted.clone())),
    ] {
        let refused = reachable(&source, &hosts(), &artifact);
        assert!(
            matches!(&refused, Err(DownloadError::Unlisted(at)) if **at == *unlisted),
            "{refused:?}"
        );
        let (refused, _) = fetched(&server, &artifact);
        assert!(
            matches!(&refused, Err(DownloadError::Unlisted(_))),
            "{refused:?}"
        );
    }
    assert!(server.asked().is_empty());

    // What may be reached is every listed host and the catalog's.
    let artifact = bare(PROGRAM, server.at("/vendor/agent"), None);
    let reached = reachable(&source, &hosts(), &artifact).expect("reachable");
    assert_eq!(
        reached,
        [
            "downloads.example.com".to_owned(),
            format!("127.0.0.1:{}", server.port)
        ]
    );
}

#[test]
fn progress_is_told_and_a_cancel_mid_download_gives_up_without_the_mirror() {
    let server = Server::start();
    server.serve("/vendor/agent", Reply::Half(PROGRAM.to_vec()));
    server.serve("/mirror/agent", Reply::Body(PROGRAM.to_vec()));
    let mut doubled = PROGRAM.to_vec();
    doubled.extend_from_slice(PROGRAM);
    let artifact = bare(
        &doubled,
        server.at("/vendor/agent"),
        Some(server.at("/mirror/agent")),
    );
    let cancel = Cancel::new();
    let mut told = Vec::new();

    let (refused, _) = downloading(
        &server,
        &hosts(),
        &artifact,
        &mut |done, total| {
            told.push((done, total));
            cancel.request();
        },
        &cancel,
    );

    assert!(
        matches!(refused, Err(DownloadError::Cancelled)),
        "{refused:?}"
    );
    let half = u64::try_from(PROGRAM.len()).expect("a size");
    assert_eq!(told, [(half, half * 2)]);
    assert_eq!(server.asked(), ["/vendor/agent"]);
}

#[test]
fn a_mirror_after_a_failed_vendor_lands_its_bytes_alone() {
    let server = Server::start();
    // The vendor sends half and closes: a failure that is not a mismatch.
    let mut doubled = PROGRAM.to_vec();
    doubled.extend_from_slice(PROGRAM);
    server.serve("/vendor/agent", Reply::Short(PROGRAM.to_vec()));
    server.serve("/mirror/agent", Reply::Body(doubled.clone()));
    let artifact = bare(
        &doubled,
        server.at("/vendor/agent"),
        Some(server.at("/mirror/agent")),
    );

    let (fetched, landed) = fetched(&server, &artifact);

    assert!(fetched.is_ok(), "{fetched:?}");
    assert_eq!(landed, doubled);
}

#[test]
fn an_asset_built_at_review_comes_from_the_catalog_by_its_digest() {
    let server = Server::start();
    let built = b"the agent as built at review";
    let mut artifact = bare(PROGRAM, server.at("/vendor/agent"), None);
    artifact.replaced_by = Some(crucible_external_harnesses::catalog::AssetRef {
        sha256: digest(built),
        size: u64::try_from(built.len()).expect("a size"),
    });
    server.serve(
        &format!("/catalog/{}", digest(built)),
        Reply::Body(built.to_vec()),
    );

    let (fetched, landed) = fetched(&server, &artifact);

    assert!(fetched.is_ok(), "{fetched:?}");
    assert_eq!(landed, built);
    assert_eq!(server.asked(), [format!("/catalog/{}", digest(built))]);
}

/// `body` downloaded under `server`'s catalog through the capped path, held
/// to the digest of `pinned` and told of progress through `progress`, and what
/// landed.
// Each is an input of its own to the case, and a value gathering them would be
// built at each caller and taken apart here.
#[allow(clippy::too_many_arguments)]
fn capped_from(
    server: &Server,
    hosts: &[Box<str>],
    address: &str,
    pinned: &[u8],
    progress: &mut (dyn FnMut(u64, u64) + Send),
    cancel: &Cancel,
) -> (Result<Fetched, DownloadError>, Vec<u8>) {
    let source = server.source();
    let landed = Landed::new();
    let mut file = landed.file();
    let fetched = run(super::capped(
        &source,
        hosts,
        address,
        &digest(pinned),
        Landing {
            file: &mut file,
            progress,
        },
        cancel,
    ));
    drop(file);
    (fetched, landed.bytes())
}

/// A capped download whose body is shorter than the cap is accepted, and the
/// size it records is what arrived rather than a pin.
#[test]
fn a_capped_download_accepts_a_shorter_body_and_records_the_size_that_arrived() {
    let server = Server::start();
    let shorter = PROGRAM.get(..10).expect("ten bytes");
    server.serve("/registry/agent.tar.gz", Reply::Body(shorter.to_vec()));
    let address = server.at("/registry/agent.tar.gz");
    let mut told = Vec::new();

    let (fetched, landed) = capped_from(
        &server,
        &hosts(),
        &address,
        shorter,
        &mut |done, total| told.push((done, total)),
        &Cancel::new(),
    );

    let fetched = fetched.expect("the shorter body");
    assert_eq!(landed, shorter);
    assert_eq!(&*fetched.url, address);
    assert_eq!(&*fetched.sha256, &*digest(shorter));
    assert_eq!(fetched.size, u64::try_from(shorter.len()).expect("a size"));
    assert_eq!(
        told.last(),
        Some(&(
            u64::try_from(shorter.len()).expect("a size"),
            super::ARCHIVE_BYTES
        ))
    );
    assert_eq!(server.asked(), ["/registry/agent.tar.gz"]);
}

/// A body past the cap is refused, however much of it arrived and whatever it
/// hashes to.
#[test]
fn a_capped_download_over_its_ceiling_is_refused() {
    let server = Server::start();
    let body = PROGRAM.to_vec();
    server.serve("/registry/agent.tar.gz", Reply::Body(body.clone()));
    let source = server.source();
    let places = super::capped_places(&source, &hosts(), &server.at("/registry/agent.tar.gz"))
        .expect("a listed place");
    let listed: Vec<&str> = places.iter().map(String::as_str).collect();
    let address = server.at("/registry/agent.tar.gz");
    let sha256 = digest(&body);
    let landed = Landed::new();
    let mut file = landed.file();
    let pin = super::Pinned {
        address: &address,
        length: super::Length::Capped(10),
        digest: super::Digest::Sha256(&sha256),
    };
    let mut told = Vec::new();

    let refused = run(super::fetch(
        &source,
        &listed,
        pin,
        &mut Landing {
            file: &mut file,
            progress: &mut |done, total| told.push((done, total)),
        },
        &Cancel::new(),
        super::Bounds {
            lifetime: Duration::from_secs(5),
            stall: Duration::from_secs(1),
        },
    ));

    assert!(
        matches!(refused, Err(DownloadError::OverCap { ceiling: 10 })),
        "{refused:?}"
    );
}

/// A registry download uses the shipped lifetime and stall bound, not a bound
/// of its own.
#[test]
fn a_registry_download_uses_the_shipped_lifetime_and_stall() {
    assert_eq!(super::Bounds::SHIPPED.lifetime, Duration::from_mins(45));
    assert_eq!(super::Bounds::SHIPPED.stall, Duration::from_mins(1));
}

/// A registry archive is downloaded through the capped path and surveyed from
/// the file it landed in, with the registry's bounds rather than a pin's.
#[cfg(feature = "proof")]
#[test]
fn a_registry_archive_is_downloaded_within_its_cap_and_surveyed_from_its_file() {
    use crucible_external_harnesses::release::{UNPACKED_BYTES, Unpacker, proof};

    let server = Server::start();
    let (archive, _) = proof::catalog_release();
    server.serve("/registry/agent.tar.gz", Reply::Body(archive.clone()));
    let source = server.source();
    let address = server.at("/registry/agent.tar.gz");
    let landed = Landed::new();
    let mut file = landed.file();
    let fetched = run(super::capped(
        &source,
        &hosts(),
        &address,
        &digest(&archive),
        Landing {
            file: &mut file,
            progress: &mut |_, _| {},
        },
        &Cancel::new(),
    ))
    .expect("a registry archive");
    drop(file);
    let artifact = crate::installing::registry_artifact(
        &address,
        &fetched.sha256,
        fetched.size,
        Format::TarGz,
    );
    let staging = std::env::temp_dir().join(format!(
        "crucible-registry-survey-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos())
    ));
    std::fs::create_dir(&staging).expect("a staging directory");

    let surveyed = Unpacker::survey_from(
        &mut std::fs::File::open(landed.0.join("download")).expect("the download"),
        &artifact,
        &staging,
    )
    .expect("surveyed from its file");

    let _ = std::fs::remove_dir_all(&staging);
    assert_eq!(fetched.size, u64::try_from(archive.len()).expect("a size"));
    assert_eq!(artifact.size, fetched.size);
    assert_eq!(artifact.unpacked, UNPACKED_BYTES);
    assert_eq!(
        surveyed.unpacked(),
        u64::try_from(proof::EXECUTABLE.len() + proof::LICENSE.len() + proof::COMPANION.len())
            .expect("a size")
    );
    assert_eq!(surveyed.files().len(), 5);
}

/// A body exactly at the cap is accepted: the cap is the most it may be, not
/// one less.
#[test]
fn a_capped_download_at_its_ceiling_is_accepted() {
    let server = Server::start();
    let body = PROGRAM.to_vec();
    server.serve("/registry/agent.tar.gz", Reply::Body(body.clone()));
    let source = server.source();
    let address = server.at("/registry/agent.tar.gz");
    let places = super::capped_places(&source, &hosts(), &address).expect("a listed place");
    let listed: Vec<&str> = places.iter().map(String::as_str).collect();
    let sha256 = digest(&body);
    let ceiling = u64::try_from(body.len()).expect("a size");
    let landed = Landed::new();
    let mut file = landed.file();
    let pin = super::Pinned {
        address: &address,
        length: super::Length::Capped(ceiling),
        digest: super::Digest::Sha256(&sha256),
    };

    let fetched = run(super::fetch(
        &source,
        &listed,
        pin,
        &mut Landing {
            file: &mut file,
            progress: &mut |_, _| {},
        },
        &Cancel::new(),
        super::Bounds::SHIPPED,
    ));

    let (_, arrived) = fetched.expect("a body at the cap");
    assert_eq!(arrived, ceiling);
}

/// A capped download that goes quiet past the stall bound is refused.
#[test]
fn a_capped_download_that_stalls_is_refused() {
    let server = Server::start();
    server.serve("/registry/agent.tar.gz", Reply::Half(PROGRAM.to_vec()));
    let source = server.source();
    let places = super::capped_places(&source, &hosts(), &server.at("/registry/agent.tar.gz"))
        .expect("a listed place");
    let listed: Vec<&str> = places.iter().map(String::as_str).collect();
    let address = server.at("/registry/agent.tar.gz");
    let sha256 = digest(PROGRAM);
    let landed = Landed::new();
    let mut file = landed.file();
    let pin = super::Pinned {
        address: &address,
        length: super::Length::Capped(super::ARCHIVE_BYTES),
        digest: super::Digest::Sha256(&sha256),
    };

    let refused = run(super::fetch(
        &source,
        &listed,
        pin,
        &mut Landing {
            file: &mut file,
            progress: &mut |_, _| {},
        },
        &Cancel::new(),
        super::Bounds {
            lifetime: Duration::from_secs(5),
            stall: Duration::from_millis(300),
        },
    ));

    assert!(
        matches!(refused, Err(DownloadError::Stalled)),
        "{refused:?}"
    );
}

/// A capped download that outlives its bound is refused.
#[test]
fn a_capped_download_past_its_lifetime_is_refused() {
    let server = Server::start();
    server.serve("/registry/agent.tar.gz", Reply::Half(PROGRAM.to_vec()));
    let source = server.source();
    let places = super::capped_places(&source, &hosts(), &server.at("/registry/agent.tar.gz"))
        .expect("a listed place");
    let listed: Vec<&str> = places.iter().map(String::as_str).collect();
    let address = server.at("/registry/agent.tar.gz");
    let sha256 = digest(PROGRAM);
    let landed = Landed::new();
    let mut file = landed.file();
    let pin = super::Pinned {
        address: &address,
        length: super::Length::Capped(super::ARCHIVE_BYTES),
        digest: super::Digest::Sha256(&sha256),
    };

    let refused = run(super::fetch(
        &source,
        &listed,
        pin,
        &mut Landing {
            file: &mut file,
            progress: &mut |_, _| {},
        },
        &Cancel::new(),
        super::Bounds {
            lifetime: Duration::from_millis(50),
            stall: Duration::from_secs(5),
        },
    ));

    assert!(matches!(refused, Err(DownloadError::Late)), "{refused:?}");
}

/// A capped download is refused before a request where its address is on no
/// host the target lists, or the download may reach.
#[test]
fn a_capped_download_off_its_listed_hosts_is_refused_before_a_request() {
    let server = Server::start();
    server.serve("/registry/agent.tar.gz", Reply::Body(PROGRAM.to_vec()));
    let source = server.source();

    let refused = super::capped_places(
        &source,
        &["downloads.example.com".into()],
        &format!("http://localhost:{}/registry/agent.tar.gz", server.port),
    );

    assert!(
        matches!(refused, Err(DownloadError::Unlisted(_))),
        "{refused:?}"
    );
    assert!(server.asked().is_empty(), "{:?}", server.asked());
}
