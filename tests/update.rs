//! Where `crucible update` asks which release is newest, and what can change
//! that.
//!
//! The built binary is run in a home of the test's own, with an environment
//! cleared down to what it needs and every proxy pointed at a listener of the
//! test's own that hangs up on whatever reaches it, so a request bound for
//! GitHub is heard there and goes no further. A release source of the test's
//! own answers on loopback, and keeps every path it was asked for.

#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

/// A directory under the system temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-update-{probe}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("home/.crucible")).expect("a temporary directory");
        Self(path)
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }

    /// Writes `config` as the home configuration.
    fn configured(&self, config: &str) {
        fs::write(self.home().join(".crucible/config.json"), config).expect("a configuration");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A loopback listener that keeps one line for each connection: the head of
/// what was asked where it is a request, and is answered from `latest` where
/// that is set, hung up on otherwise.
struct Listener {
    url: String,
    heard: Arc<Mutex<Vec<String>>>,
}

impl Listener {
    fn new(latest: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let url = format!("http://{}", listener.local_addr().expect("its address"));
        let heard = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&heard);
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
                let line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                kept.lock().expect("the lines").push(line.clone());
                let Some(tag) = &latest else {
                    continue;
                };
                let (status, body) = if line.starts_with("GET /releases/latest ") {
                    ("200 OK", format!(r#"{{"tag_name":"{tag}"}}"#))
                } else {
                    ("404 Not Found", String::new())
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Self { url, heard }
    }

    /// A release source naming `tag` as the newest release.
    fn source(tag: &str) -> Self {
        Self::new(Some(tag.to_owned()))
    }

    /// A proxy that hangs up on everything.
    fn proxy() -> Self {
        Self::new(None)
    }

    fn heard(&self) -> Vec<String> {
        self.heard.lock().expect("the lines").clone()
    }
}

/// What the built binary answers to `crucible update` with `args`, run in
/// `scratch`'s home with every proxy pointed at `proxy`, and with
/// `CRUCIBLE_CODE_UPDATE_SOURCE` set to `source` where it is given.
fn update(scratch: &Scratch, proxy: &Listener, source: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crucible"));
    command
        .arg("update")
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("HOME", scratch.home())
        .env("CRUCIBLE_CODE_HOME", scratch.home().join(".crucible"))
        .current_dir(scratch.home())
        .stdin(Stdio::null());
    // Windows finds its socket providers through `SystemRoot`, and a process
    // started without it cannot open a connection to anything.
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    for variable in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(variable, &proxy.url);
    }
    if let Some(source) = source {
        command.env("CRUCIBLE_CODE_UPDATE_SOURCE", source);
    }
    command.output().expect("the built binary runs")
}

fn said(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_configuration_env_block_does_not_name_the_release_source() {
    let scratch = Scratch::new("env-block");
    let source = Listener::source("v99.0.0");
    let proxy = Listener::proxy();
    scratch.configured(&format!(
        r#"{{ "env": {{ "CRUCIBLE_CODE_UPDATE_SOURCE": "{}" }} }}"#,
        source.url
    ));

    let output = update(&scratch, &proxy, None, &["--check"]);

    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert_eq!(
        output.stderr,
        b"crucible: could not ask the release source which release is newest\n",
        "{}",
        said(&output)
    );
    assert!(source.heard().is_empty(), "{:?}", source.heard());
    assert_eq!(
        proxy.heard(),
        ["CONNECT api.github.com:443 HTTP/1.1"],
        "the check went to GitHub, through the proxy"
    );
}

#[test]
fn the_environment_names_a_loopback_source_whatever_updates_check_says() {
    let scratch = Scratch::new("loopback");
    scratch.configured(r#"{ "updates": { "check": "never" } }"#);
    let proxy = Listener::proxy();

    let later = Listener::source("v99.0.0");
    let output = update(&scratch, &proxy, Some(&later.url), &["--check"]);

    assert_eq!(output.status.code(), Some(3), "{}", said(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "crucible 99.0.0 is out; this is {}. `crucible update` installs it\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(later.heard(), ["GET /releases/latest HTTP/1.1"]);

    let same = Listener::source(&format!("v{}", env!("CARGO_PKG_VERSION")));
    let output = update(&scratch, &proxy, Some(&same.url), &["--check"]);

    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert_eq!(same.heard(), ["GET /releases/latest HTTP/1.1"]);
    assert!(proxy.heard().is_empty(), "{:?}", proxy.heard());
}

#[test]
fn a_source_off_this_machine_is_refused_before_anything_is_asked() {
    let scratch = Scratch::new("elsewhere");
    let proxy = Listener::proxy();

    let output = update(
        &scratch,
        &proxy,
        Some("http://example.com/releases"),
        &["--check"],
    );

    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("CRUCIBLE_CODE_UPDATE_SOURCE may only"),
        "{}",
        said(&output)
    );
    assert!(proxy.heard().is_empty(), "{:?}", proxy.heard());
}

#[test]
fn a_build_cargo_made_is_not_replaced() {
    let scratch = Scratch::new("cargo");
    let proxy = Listener::proxy();
    let source = Listener::source("v99.0.0");
    // Every install on Windows is updated by its own installer, which is the
    // refusal given there before anything asks what made this one.
    let route = if cfg!(windows) {
        "install.ps1"
    } else {
        "`cargo install`"
    };

    for asked in [&["--dry-run"][..], &[]] {
        let output = update(&scratch, &proxy, Some(&source.url), asked);

        assert_eq!(output.status.code(), Some(1), "{}", said(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(route),
            "{}",
            said(&output)
        );
    }
    assert!(source.heard().is_empty(), "{:?}", source.heard());
}
