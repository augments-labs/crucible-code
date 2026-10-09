use std::ffi::OsStr;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::*;

/// A loopback release source: each path asked is answered from `routes`, or
/// with 404, and kept, so a test can say what went out.
struct Served {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Served {
    fn new(routes: Vec<(String, &'static str, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let url = format!("http://{}", listener.local_addr().expect("its address"));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&asked);
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
                let request = String::from_utf8_lossy(&request);
                let path = request.split(' ').nth(1).unwrap_or_default().to_owned();
                heard.lock().expect("the paths").push(path.clone());
                let (status, body) = routes
                    .iter()
                    .find(|(route, _, _)| *route == path)
                    .map_or(("404 Not Found", Vec::new()), |(_, status, body)| {
                        (*status, body.clone())
                    });
                let mut response = format!(
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&body);
                let _ = stream.write_all(&response);
            }
        });
        Self { url, asked }
    }

    /// A source whose newest release is `tag`, with nothing to download.
    fn naming(tag: &str) -> Self {
        Self::new(vec![latest(tag)])
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("the paths").clone()
    }
}

/// The answer naming `tag` as the newest release.
fn latest(tag: &str) -> (String, &'static str, Vec<u8>) {
    (
        "/releases/latest".to_owned(),
        "200 OK",
        format!(r#"{{"tag_name":"{tag}"}}"#).into_bytes(),
    )
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a runtime")
}

/// Runs `asked` against `source` as the executable at `executable`, a build
/// of `running`.
fn update(asked: Asked, source: &str, executable: &Path, running: &str) -> Result<Answer, Refused> {
    let runtime = runtime();
    UpdateCrateReleaseCheck::new().update(
        runtime.handle(),
        &SelfUpdateCommand {
            asked,
            source: Some(OsStr::new(source)),
            executable,
            running,
        },
    )
}

fn version(text: &str) -> Version {
    Version::parse(text.as_bytes()).expect("a release number")
}

#[test]
fn a_check_finds_a_later_release_and_exits_three() {
    let served = Served::naming("v0.48.0");

    let answer =
        update(Asked::Check, &served.url, Path::new("/nowhere"), "0.47.0").expect("an answer");

    assert_eq!(
        answer,
        Answer::Available {
            running: version("0.47.0"),
            newest: version("0.48.0"),
        }
    );
    assert_eq!(answer.exit(), 3);
    assert_eq!(served.asked(), ["/releases/latest"]);
}

#[test]
fn a_check_against_an_earlier_or_the_same_release_is_current() {
    for tag in ["v0.46.9", "v0.47.0", "v0.9.99"] {
        let served = Served::naming(tag);

        let answer =
            update(Asked::Check, &served.url, Path::new("/nowhere"), "0.47.0").expect("an answer");

        assert_eq!(
            answer,
            Answer::Current {
                running: version("0.47.0")
            },
            "{tag}"
        );
        assert_eq!(answer.exit(), 0);
    }
}

#[test]
fn a_name_that_is_not_a_release_number_is_refused_and_not_shown() {
    for tag in [
        "v0.48.0-rc.1",
        "v0.48.0.1",
        "v0.048.0",
        r"v0.48.0\u001b]0;x\u0007",
    ] {
        let served = Served::naming(tag);

        let refused = update(Asked::Check, &served.url, Path::new("/nowhere"), "0.47.0")
            .expect_err("a refusal");

        assert!(matches!(refused, Refused::NotStable), "{tag}: {refused:?}");
        assert!(!refused.to_string().contains("0.48"), "{refused}");
    }
}

#[test]
fn a_source_that_does_not_answer_is_unreachable() {
    let served = Served::new(vec![(
        "/releases/latest".to_owned(),
        "500 Internal Server Error",
        Vec::new(),
    )]);

    let refused =
        update(Asked::Check, &served.url, Path::new("/nowhere"), "0.47.0").expect_err("a refusal");

    assert!(matches!(refused, Refused::Unreachable), "{refused:?}");
}

#[test]
fn a_source_off_this_machine_is_refused_before_anything_is_asked() {
    let served = Served::naming("v0.48.0");
    let port = served.url.rsplit(':').next().expect("a port");
    for named in [
        format!("https://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://user@127.0.0.1:{port}"),
        format!("http://127.0.0.1:{port}/?to=elsewhere"),
        format!("http://127.0.0.1:{port}/#elsewhere"),
        format!("http://127.0.0.2:{port}"),
        "http://example.com".to_owned(),
        "ftp://127.0.0.1".to_owned(),
        "127.0.0.1".to_owned(),
        String::new(),
    ] {
        let refused =
            update(Asked::Check, &named, Path::new("/nowhere"), "0.47.0").expect_err("a refusal");

        assert!(matches!(refused, Refused::Source), "{named}: {refused:?}");
        assert!(refused.to_string().contains(SOURCE), "{refused}");
    }
    assert!(served.asked().is_empty(), "{:?}", served.asked());
}

#[test]
fn a_loopback_source_of_either_family_is_taken() {
    for named in [
        "http://127.0.0.1:9",
        "http://[::1]:9/",
        "http://127.0.0.1:9/mirror",
    ] {
        assert!(
            matches!(
                Source::named(Some(OsStr::new(named))),
                Ok(Source::Loopback(_))
            ),
            "{named}"
        );
    }
    assert!(matches!(Source::named(None), Ok(Source::GitHub)));
}

#[test]
fn a_build_that_is_not_a_release_is_refused() {
    let served = Served::naming("v0.48.0");

    let refused = update(
        Asked::Check,
        &served.url,
        Path::new("/nowhere"),
        "0.47.0-dev",
    )
    .expect_err("a refusal");

    assert!(matches!(refused, Refused::Running), "{refused:?}");
}

#[cfg(unix)]
mod installed {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::install::fixture::{
        archive_name, broker, executable, hex, installed, packed, release_archive,
    };
    use crate::install::refusing_sync_after;
    use crate::{ActivationError, ReceiptLayout, RecoverableActivation};

    /// The release each test install has active.
    const ACTIVE: &str = "0.47.0";

    /// The release each test source offers.
    const NEXT: &str = "0.47.1";

    /// An install laid out as `install.sh` lays one out, in a directory of
    /// its own that is deleted with the value.
    struct Install {
        dir: PathBuf,
    }

    impl Install {
        fn new(name: &str) -> Self {
            let at = std::env::temp_dir()
                .join(format!("crucible-command-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&at);
            fs::create_dir_all(&at).expect("a temporary directory");
            let dir = at.canonicalize().expect("a canonical temporary directory");
            installed(&dir, ACTIVE);
            Self { dir }
        }

        fn executable(&self) -> PathBuf {
            self.dir.join("crucible")
        }

        fn prefix(&self) -> PathBuf {
            self.dir.join(".crucible-install")
        }

        fn update(&self, asked: Asked, served: &Served) -> Result<Answer, Refused> {
            update(asked, &served.url, &self.executable(), ACTIVE)
        }

        /// The release `current` names.
        fn active(&self) -> String {
            let target = fs::read_link(self.dir.join(".crucible-install/current"))
                .expect("the active-release link");
            target
                .strip_prefix("releases")
                .expect("a release")
                .to_string_lossy()
                .into_owned()
        }

        /// Every name under the install, each with its link's target or its
        /// content's SHA-256.
        fn snapshot(&self) -> Vec<(PathBuf, String)> {
            let mut seen = Vec::new();
            let mut left = vec![self.dir.clone()];
            while let Some(at) = left.pop() {
                for entry in fs::read_dir(&at).expect("a directory") {
                    let path = entry.expect("an entry").path();
                    let about = fs::symlink_metadata(&path).expect("its metadata");
                    let said = if about.is_symlink() {
                        fs::read_link(&path)
                            .expect("a link")
                            .to_string_lossy()
                            .into_owned()
                    } else if about.is_dir() {
                        left.push(path.clone());
                        "directory".to_owned()
                    } else {
                        hex(&fs::read(&path).expect("a file"))
                    };
                    seen.push((path, said));
                }
            }
            seen.sort();
            seen
        }
    }

    impl Drop for Install {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// The routes a source publishing `archive` as `version` answers, with
    /// `version` named as the newest release.
    fn publishing(version: &str, archive: Vec<u8>) -> Vec<(String, &'static str, Vec<u8>)> {
        let name = archive_name(version);
        let sums = format!("{}  {name}\n", hex(&archive)).into_bytes();
        vec![
            latest(&format!("v{version}")),
            (
                format!("/releases/download/v{version}/SHA256SUMS"),
                "200 OK",
                sums,
            ),
            (
                format!("/releases/download/v{version}/{name}"),
                "200 OK",
                archive,
            ),
        ]
    }

    #[test]
    fn an_earlier_release_is_never_downloaded_or_installed() {
        let install = Install::new("earlier");
        let before = install.snapshot();
        let served = Served::new(publishing("0.46.9", release_archive("0.46.9")));

        for asked in [Asked::DryRun, Asked::Apply] {
            let answer = install.update(asked, &served).expect("an answer");

            assert_eq!(
                answer,
                Answer::Current {
                    running: version(ACTIVE)
                },
                "{asked:?}"
            );
        }
        assert_eq!(served.asked(), ["/releases/latest", "/releases/latest"]);
        assert_eq!(install.snapshot(), before);
        assert_eq!(install.active(), ACTIVE);
    }

    #[test]
    fn a_later_release_is_installed_and_runs_as_itself() {
        let install = Install::new("later");
        let served = Served::new(publishing(NEXT, release_archive(NEXT)));

        let answer = install.update(Asked::Apply, &served).expect("an answer");

        assert_eq!(
            answer,
            Answer::Updated {
                from: version(ACTIVE),
                to: version(NEXT),
            }
        );
        assert_eq!(answer.exit(), 0);
        assert_eq!(install.active(), NEXT);
        let said = std::process::Command::new(install.executable())
            .arg("--version")
            .output()
            .expect("the command run");
        assert_eq!(said.stdout, format!("crucible {NEXT}\n").into_bytes());
    }

    #[test]
    fn a_check_and_a_dry_run_change_nothing() {
        let install = Install::new("unchanged");
        let before = install.snapshot();
        let served = Served::new(publishing(NEXT, release_archive(NEXT)));

        let checked = install.update(Asked::Check, &served).expect("an answer");
        let proposed = install.update(Asked::DryRun, &served).expect("an answer");

        assert_eq!(checked.exit(), 3);
        assert_eq!(
            proposed,
            Answer::Proposed {
                running: version(ACTIVE),
                newest: version(NEXT),
            }
        );
        assert_eq!(served.asked(), ["/releases/latest", "/releases/latest"]);
        assert_eq!(install.snapshot(), before);
    }

    #[test]
    fn an_archive_that_is_not_the_one_listed_is_not_installed() {
        let install = Install::new("mismatch");
        let before = install.snapshot();
        let mut routes = publishing(NEXT, release_archive(NEXT));
        if let Some((_, _, archive)) = routes.last_mut() {
            archive.push(0);
        }
        let served = Served::new(routes);

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(
            matches!(refused, Refused::Stage(crate::StageError::Mismatch)),
            "{refused:?}"
        );
        assert_eq!(install.snapshot(), before);
    }

    #[test]
    fn a_listed_archive_that_does_not_unpack_is_not_installed() {
        let install = Install::new("corrupt");
        let before = install.snapshot();
        let served = Served::new(publishing(NEXT, b"not a gzip stream".to_vec()));

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(matches!(refused, Refused::Stage(_)), "{refused:?}");
        assert_eq!(install.snapshot(), before);
    }

    #[test]
    fn a_missing_file_is_named_and_nothing_is_installed() {
        let install = Install::new("missing");
        let before = install.snapshot();
        let mut routes = publishing(NEXT, release_archive(NEXT));
        routes.pop();
        let served = Served::new(routes);

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert_eq!(
            refused.to_string(),
            format!(
                "could not download {} from the release source",
                archive_name(NEXT)
            )
        );
        assert_eq!(install.snapshot(), before);
    }

    #[test]
    fn a_release_without_the_broker_the_active_one_has_is_not_installed() {
        let install = Install::new("broker");
        let before = install.snapshot();
        let served = Served::new(publishing(NEXT, packed(NEXT, &executable(NEXT), None)));

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(matches!(refused, Refused::Broker), "{refused:?}");
        assert_eq!(install.snapshot(), before);
    }

    #[test]
    fn an_install_another_update_holds_is_left_alone() {
        let install = Install::new("held");
        let layout = ReceiptLayout::of_executable(&install.executable()).expect("a layout");
        let holding = RecoverableActivation::begin(&layout).expect("the lock");
        let served = Served::new(publishing(NEXT, release_archive(NEXT)));

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(
            matches!(refused, Refused::Activation(ActivationError::Held)),
            "{refused:?}"
        );
        drop(holding);
        assert_eq!(install.active(), ACTIVE);
        assert_eq!(served.asked(), ["/releases/latest"]);
    }

    #[test]
    fn a_release_that_does_not_say_its_version_is_rolled_back() {
        let install = Install::new("rolled");
        let served = Served::new(publishing(
            NEXT,
            packed(NEXT, &executable("9.9.9"), Some(&broker(NEXT))),
        ));

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(
            matches!(&refused, Refused::RolledBack { version: rolled } if *rolled == version(NEXT)),
            "{refused:?}"
        );
        assert_eq!(install.active(), ACTIVE);
    }

    #[test]
    fn a_release_whose_switch_cannot_be_synced_is_said_to_be_active() {
        let install = Install::new("unsynced");
        let served = Served::new(publishing(NEXT, release_archive(NEXT)));

        refusing_sync_after("made the activation link", install.prefix());
        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert_eq!(
            refused.to_string(),
            format!(
                "crucible {NEXT} is installed and active, but the switch to it could not be \
                 made durable; a crash of the system may make the release before it active again"
            )
        );
        assert_eq!(install.active(), NEXT);
    }

    #[test]
    fn a_release_whose_switch_cannot_be_synced_is_still_run_and_rolled_back() {
        let install = Install::new("unsynced-rolled");
        let served = Served::new(publishing(
            NEXT,
            packed(NEXT, &executable("9.9.9"), Some(&broker(NEXT))),
        ));

        refusing_sync_after("made the activation link", install.prefix());
        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(
            matches!(&refused, Refused::RolledBack { version: rolled } if *rolled == version(NEXT)),
            "{refused:?}"
        );
        assert_eq!(install.active(), ACTIVE);
    }

    #[test]
    fn an_install_not_made_by_install_sh_says_how_it_is_updated() {
        let install = Install::new("unmanaged");
        let plain = install.dir.join("plain");
        fs::create_dir(&plain).expect("a directory");
        fs::write(plain.join("crucible"), executable(ACTIVE)).expect("a copy");
        let cargo = install.dir.join("target/release");
        fs::create_dir_all(&cargo).expect("a target directory");
        fs::write(
            install.dir.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n\
             # This file is a cache directory tag created by cargo.\n",
        )
        .expect("cargo's tag");
        fs::write(cargo.join("crucible"), executable(ACTIVE)).expect("a build");
        let root = install.dir.join("cargo-root");
        fs::create_dir_all(root.join("bin")).expect("an install root");
        fs::write(root.join(".crates2.json"), "{}").expect("cargo's records");
        fs::write(root.join("bin/crucible"), executable(ACTIVE)).expect("an installed build");
        let served = Served::naming("v0.48.0");

        for (executable, expected) in [
            (plain.join("crucible"), "Unmanaged"),
            (cargo.join("crucible"), "Cargo"),
            (root.join("bin/crucible"), "Cargo"),
        ] {
            for asked in [Asked::DryRun, Asked::Apply] {
                let refused =
                    update(asked, &served.url, &executable, ACTIVE).expect_err("a refusal");

                assert_eq!(format!("{refused:?}"), expected, "{}", executable.display());
            }
        }
        assert!(served.asked().is_empty(), "{:?}", served.asked());
    }

    #[test]
    fn an_install_whose_receipt_does_not_read_is_refused_and_not_taken_for_a_copy() {
        let install = Install::new("receipt");
        let receipt = install
            .dir
            .join(".crucible-install/releases")
            .join(ACTIVE)
            .join("receipt");
        fs::write(&receipt, b"{ not a receipt").expect("a damaged receipt");
        let before = install.snapshot();
        let served = Served::naming("v0.48.0");

        let refused = install
            .update(Asked::Apply, &served)
            .expect_err("a refusal");

        assert!(
            matches!(refused, Refused::Layout(crate::LayoutError::Receipt(_))),
            "{refused:?}"
        );
        assert_eq!(install.snapshot(), before);
        assert!(served.asked().is_empty(), "{:?}", served.asked());
    }

    #[test]
    fn a_redirect_is_followed_only_from_github_to_its_own_hosts() {
        for (location, followed_to) in [
            ("https://release-assets.githubusercontent.com/a?b=c", true),
            ("https://github.com/augments-labs/crucible-code/x", true),
            ("http://objects.githubusercontent.com/a", false),
            ("https://github.com.example.com/a", false),
            ("https://githubusercontent.com.example.com/a", false),
            ("https://user@github.com/a", false),
            ("https://github.com:8443/a", false),
            ("/relative", false),
        ] {
            assert_eq!(
                super::super::apply::followed(&Source::GitHub, location).is_some(),
                followed_to,
                "{location}"
            );
        }
        assert!(
            super::super::apply::followed(
                &Source::Loopback("http://127.0.0.1:9".into()),
                "http://127.0.0.1:9/a"
            )
            .is_none()
        );
    }
}
