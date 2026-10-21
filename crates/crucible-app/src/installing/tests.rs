//! A pinned release is downloaded whole, from where it was pinned, and kept
//! once in place.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::OnceLock;

use crucible_config::Driver;
use crucible_external_harnesses::release::proof::{EXECUTABLE, released};
use crucible_http::{PlainLookups, ProxyEnv, Tls};

use super::*;
use crate::backends::ANTIGRAVITY;
use crate::sample::Sample;

/// Answers every request with `head` and then `body`, and gives the address
/// it listens on.
fn serving(head: String, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    let url = format!(
        "http://{}/agent.zip",
        listener.local_addr().expect("its address")
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0_u8];
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.extend_from_slice(&byte);
            }
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    url
}

/// Answers with `status` and `body`, saying its length.
fn answering(status: &str, body: Vec<u8>) -> String {
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    serving(head, body)
}

/// A client that dials the loopback address itself.
fn loopback() -> Http {
    let plain = Lookups::plain(NonZeroUsize::MIN);
    let targets: Lookups = PlainLookups::clone(&plain).into();
    Http::new(
        &Tls::new().expect("the pinned TLS configuration"),
        targets,
        plain,
        ProxyEnv::read(|_| None),
    )
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a runtime")
}

/// The names under `root`, sorted.
fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
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

/// Downloads the release published at `url` as `archive` was into `root`.
fn fetching(url: String, root: &Path) -> (Result<PathBuf, Uninstalled>, Vec<(u64, u64)>) {
    let (_, pin) = released(url.leak());
    let mut heard = Vec::new();
    let runtime = runtime();
    let done = downloaded(
        runtime.handle(),
        &loopback(),
        &pin,
        root,
        &mut |held, of| {
            heard.push((held, of));
        },
    );
    (done, heard)
}

#[test]
fn the_pinned_release_is_downloaded_unpacked_and_its_download_removed() {
    let sample = Sample::new("installing-downloaded");
    let root = sample.root().join("programs").join("agent");
    let (archive, _) = released("");
    let whole = u64::try_from(archive.len()).expect("a size");
    let url = answering("200 OK", archive);

    let (done, heard) = fetching(url, &root);

    let executable = done.expect("the release is installed");
    assert_eq!(executable, root.join("1.0.0").join("agent"));
    assert_eq!(
        std::fs::read(&executable).expect("the executable"),
        EXECUTABLE
    );
    assert_eq!(heard.last(), Some(&(whole, whole)));
    assert_eq!(names(&root), ["1.0.0"]);
}

#[test]
fn a_body_longer_than_the_pinned_archive_is_refused_and_nothing_is_kept() {
    let sample = Sample::new("installing-longer");
    let root = sample.root().join("agent");
    let (mut archive, _) = released("");
    archive.push(0);
    let url = answering("200 OK", archive);

    let (done, _) = fetching(url, &root);

    assert!(matches!(done, Err(Uninstalled::Download(_))), "{done:?}");
    assert!(names(&root).is_empty(), "{:?}", names(&root));
}

#[test]
fn a_body_cut_short_is_refused_and_nothing_is_kept() {
    let sample = Sample::new("installing-short");
    let root = sample.root().join("agent");
    let (archive, _) = released("");
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        archive.len()
    );
    let half = archive.get(..archive.len() / 2).expect("half").to_vec();
    let url = serving(head, half);

    let (done, _) = fetching(url, &root);

    assert!(matches!(done, Err(Uninstalled::Download(_))), "{done:?}");
    assert!(names(&root).is_empty(), "{:?}", names(&root));
}

#[test]
fn a_redirect_is_not_followed() {
    let sample = Sample::new("installing-redirect");
    let root = sample.root().join("agent");
    let head = "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/agent.zip\r\n\
                content-length: 0\r\nconnection: close\r\n\r\n"
        .to_owned();
    let url = serving(head, Vec::new());

    let (done, _) = fetching(url, &root);

    assert!(matches!(done, Err(Uninstalled::Download(_))), "{done:?}");
    assert!(names(&root).is_empty(), "{:?}", names(&root));
}

/// A release published nowhere it could be downloaded from.
static NOWHERE: OnceLock<Pin> = OnceLock::new();

#[test]
fn a_release_already_in_place_is_kept_and_nothing_is_asked() {
    let sample = Sample::new("installing-kept");
    let home = sample.home();
    let (archive, pin) = released("http://127.0.0.1:1/agent.zip");
    let pin = NOWHERE.get_or_init(|| pin);
    let row = Row {
        pinned: || NOWHERE.get(),
        ..ANTIGRAVITY
    };
    let root = root(&home, &row);
    let held = sample.root().join("held.zip");
    std::fs::write(&held, archive).expect("the archive");
    unpack(pin, &held, &root).expect("the release is in place");

    let (installed, finished) = install(&row, &home, &mut |_, _| {});

    assert_eq!(
        installed.expect("the release is kept"),
        Installed {
            version: "1.0.0",
            executable: root.join("1.0.0").join("agent"),
            kept: true,
        }
    );
    assert!(finished.is_ok());
    assert_eq!(
        executable(&home, &row),
        Some(root.join("1.0.0").join("agent"))
    );
}

#[test]
fn an_agent_with_no_pinned_release_is_refused_by_name() {
    let sample = Sample::new("installing-unpinned");
    let row = Row {
        driver: Driver::Claude,
        pinned: || None,
        ..ANTIGRAVITY
    };

    let (installed, _) = install(&row, &sample.home(), &mut |_, _| {});

    let refused = installed.expect_err("nothing is pinned");
    assert!(
        refused
            .to_string()
            .starts_with("this build of crucible installs no release of claude on"),
        "{refused}"
    );
    assert_eq!(executable(&sample.home(), &row), None);
}

#[test]
fn an_agent_is_kept_under_its_driver_in_crucible_home() {
    let home = Path::new("/home/me/.crucible");
    assert_eq!(
        root(home, &ANTIGRAVITY),
        Path::new("/home/me/.crucible/programs/antigravity")
    );
}
