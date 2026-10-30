//! A pinned release is downloaded whole, from where it was pinned, and kept
//! once in place.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::num::NonZeroUsize;
use std::path::Path;

use crucible_config::Driver;
use crucible_external_harnesses::catalog::Trust;
use crucible_external_harnesses::release::proof::{self, EXECUTABLE, released};
use crucible_http::{PlainLookups, ProxyEnv, Tls};

use super::*;
use crate::backends::tests::{release_of, snapshot_version, trust_of, trusting};
use crate::backends::{ANTIGRAVITY, CLAUDE};
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

/// The catalog entry for the release `released` makes, as Antigravity's, listed
/// at an address it is not downloaded from.
fn listing(version_of: &Pin<'_>) -> serde_json::Value {
    release_of(
        "antigravity",
        version_of.version,
        (
            "https://example.invalid/agent.zip",
            version_of.sha256,
            version_of.archive,
        ),
        [
            (version_of.executable.name, version_of.executable.bytes),
            (version_of.companion.name, version_of.companion.bytes),
        ],
        &[],
    )
}

#[test]
fn a_release_already_in_place_is_kept_and_nothing_is_asked() {
    let sample = Sample::new("installing-kept");
    let home = sample.home();
    let (archive, pin) = released("http://127.0.0.1:1/agent.zip");
    let trust = trusting(&sample, 2, &[listing(&pin)]);
    let row = ANTIGRAVITY;
    let root = root(&home, &row);
    let held = sample.root().join("held.zip");
    std::fs::write(&held, archive).expect("the archive");
    unpack(&pin, &held, &root).expect("the release is in place");

    let (installed, finished) = install(&row, &home, &trust, &mut |_, _| {});

    assert_eq!(
        installed.expect("the release is kept"),
        Installed {
            version: "1.0.0".into(),
            executable: root.join("1.0.0").join("agent"),
            kept: true,
        }
    );
    assert!(finished.is_ok());
    assert_eq!(
        executable(&home, &row, &trust),
        Some(root.join("1.0.0").join("agent"))
    );
}

/// The program the row runs once installed is the version the catalog lists,
/// in a directory of its own beside the version 0.48 installed.
#[test]
fn the_program_is_the_executable_of_the_version_the_catalog_lists() {
    let sample = Sample::new("installing-executable");
    let home = sample.home();
    let (_, pin) = released("https://example.invalid/agent.zip");
    let newer = Pin {
        version: "1.4.1",
        ..pin
    };

    let before = executable(&home, &ANTIGRAVITY, &trust_of(&sample));
    let after = executable(
        &home,
        &ANTIGRAVITY,
        &trusting(&sample, 2, &[listing(&newer)]),
    );

    let programs = root(&home, &ANTIGRAVITY);
    assert_eq!(after, Some(programs.join("1.4.1").join("agent")));
    assert_eq!(
        before
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::file_name),
        snapshot_version().map(std::ffi::OsStr::new),
        "without the catalog it is the snapshot's"
    );
}

#[test]
fn an_agent_with_no_pinned_release_is_refused_by_name() {
    let sample = Sample::new("installing-unpinned");
    let row = Row {
        driver: Driver::Claude,
        pinned: |_| None,
        ..ANTIGRAVITY
    };

    let (installed, _) = install(&row, &sample.home(), &trust_of(&sample), &mut |_, _| {});

    let refused = installed.expect_err("nothing is pinned");
    assert!(
        refused
            .to_string()
            .starts_with("the catalog offers no release of claude to install on"),
        "{refused}"
    );
    assert_eq!(executable(&sample.home(), &row, &trust_of(&sample)), None);
}

/// Writes a trust state nothing can read, and loads it.
fn unusable(sample: &Sample) -> Trust {
    let file = trust_of(sample).file();
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("a directory");
    std::fs::write(&file, "not a trust state").expect("the file");
    trust_of(sample)
}

/// A release is offered by nothing while the trust state cannot be used, and
/// the user is told so, naming the file to repair or remove, rather than that
/// there is nothing to install.
#[test]
fn a_trust_state_that_cannot_be_used_is_told_and_not_read_as_nothing_to_install() {
    let sample = Sample::new("installing-untrusted");
    let trust = unusable(&sample);
    let file = trust.file();

    let (installed, finished) = install(&ANTIGRAVITY, &sample.home(), &trust, &mut |_, _| {});

    let refused = installed.expect_err("nothing is offered");
    let said = refused.to_string();
    assert!(matches!(refused, Uninstalled::Untrusted { .. }), "{said}");
    assert!(
        said.contains(&file.display().to_string()) && said.contains("repair or remove"),
        "{said}"
    );
    assert!(finished.is_ok());
    assert_eq!(executable(&sample.home(), &ANTIGRAVITY, &trust), None);
    let [_, next] = refusal(
        &ANTIGRAVITY,
        &Discovery::Missing,
        "Antigravity",
        "antigravity",
        &trust,
    )
    .expect("a refusal");
    assert!(
        next.contains(&file.display().to_string()) && next.contains("Repair or remove"),
        "{next}"
    );
}

/// An agent that installs itself is not told of a trust state it does not
/// read.
#[test]
fn a_trust_state_that_cannot_be_used_is_not_told_of_an_agent_that_installs_itself() {
    let sample = Sample::new("installing-untrusted-claude");
    let trust = unusable(&sample);

    let [_, next] = refusal(
        &CLAUDE,
        &Discovery::Missing,
        "Claude Code",
        "claude",
        &trust,
    )
    .expect("a refusal");
    let (installed, _) = install(&CLAUDE, &sample.home(), &trust, &mut |_, _| {});

    assert!(!next.contains("trust state"), "{next}");
    assert!(
        matches!(installed, Err(Uninstalled::Unpinned { agent: "claude" })),
        "{installed:?}"
    );
}

/// Where a release is offered, a program that is missing is told to be
/// installed by the command that does it.
#[test]
fn a_missing_program_of_an_agent_with_a_release_is_told_to_install_it() {
    let sample = Sample::new("installing-refusal");
    let trust = trusting(
        &sample,
        2,
        &[listing(&released("https://example.invalid/a.zip").1)],
    );

    let [_, next] = refusal(
        &ANTIGRAVITY,
        &Discovery::Missing,
        "Antigravity",
        "antigravity",
        &trust,
    )
    .expect("a refusal");

    assert!(
        next.contains("crucible agents install antigravity"),
        "{next}"
    );
}

#[test]
fn an_agent_is_kept_under_its_driver_in_crucible_home() {
    let home = Path::new("/home/me/.crucible");
    assert_eq!(
        root(home, &ANTIGRAVITY),
        Path::new("/home/me/.crucible/programs/antigravity")
    );
}

/// A wheel is surveyed by the wheel's reader, so the list the intake records
/// names each file under `site/`, where an install puts it.
#[test]
fn unpack_check_surveys_a_wheel_into_its_site_directory() {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;

    let sample = Sample::new("unpack-check-wheel");
    let root = sample.root();
    let (wheel, installs) = proof::wheel(
        "peer-1.0.dist-info",
        &[
            ("peer/__init__.py", b"".as_slice()),
            ("peer-1.0.dist-info/METADATA", b"Name: peer\n".as_slice()),
        ],
    );
    let sha256 = Sha256::digest(&wheel)
        .iter()
        .fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        });
    let described = serde_json::json!({
        "url": "https://example.invalid/peer-1.0-py3-none-any.whl",
        "sha256": sha256,
        "size": wheel.len(),
        "format": "wheel",
        "unpacked": 1 << 20,
        "decompressed": 1 << 20,
    });
    let (entry, artifact, out) = (
        root.join("artifact.json"),
        root.join("peer.whl"),
        root.join("out"),
    );
    fs::write(&entry, described.to_string()).expect("the artifact");
    fs::write(&artifact, &wheel).expect("the download");

    let listed = unpack_check(&entry, &artifact, &out).expect("surveyed");

    let listed: serde_json::Value = serde_json::from_str(&listed).expect("a list");
    let mut paths: Vec<&str> = listed
        .get("files")
        .and_then(serde_json::Value::as_array)
        .expect("its files")
        .iter()
        .filter_map(|file| file.get("path").and_then(serde_json::Value::as_str))
        .collect();
    paths.sort_unstable();
    let mut installed: Vec<&str> = installs.iter().map(|(path, _)| path.as_str()).collect();
    installed.sort_unstable();
    assert_eq!(paths, installed);
    for (path, bytes) in &installs {
        assert_eq!(
            fs::read(out.join(path)).ok().as_ref(),
            Some(bytes),
            "{path}"
        );
    }
}
