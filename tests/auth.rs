//! What `crucible auth status`, `auth login` and `auth logout` do to a home of
//! the test's own, and what they say while doing it.
//!
//! The built binary is run in a directory and a home of the test's own, with
//! an environment cleared down to what it needs and every proxy pointed at a
//! loopback listener of the test's own, so nothing the machine running this
//! keeps is read, nothing is written anywhere a test did not make, and any
//! attempt to reach out lands where the test can hear it. Every credential
//! here is made up, and the one each test hands over is a sentinel searched
//! for afterwards in everything the run could have put it in.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A made-up key nothing may repeat: not standard output, not standard
/// error, not a file but the store, and not a process started on the way.
const SENTINEL: &str = "not-a-real-key-auth-sentinel-0f9e8d7c6b5a49382716";

/// A directory under the system temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-auth-{probe}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("work")).expect("a temporary directory");
        fs::create_dir_all(path.join("home/.crucible")).expect("a temporary directory");
        fs::create_dir_all(path.join("bin")).expect("a temporary directory");
        Self(path)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }

    fn crucible(&self) -> PathBuf {
        self.home().join(".crucible")
    }

    fn store(&self) -> PathBuf {
        self.crucible().join("auth.json")
    }

    fn bin(&self) -> PathBuf {
        self.0.join("bin")
    }

    /// The store, written as `text`, private to its owner.
    fn holding(&self, text: &str) {
        fs::write(self.store(), text).expect("a store this test writes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(self.store(), fs::Permissions::from_mode(0o600))
                .expect("a store this test owns");
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A loopback listener nothing should ever connect to.
struct Sentinel(TcpListener);

impl Sentinel {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        listener
            .set_nonblocking(true)
            .expect("a listener that can be polled");
        Self(listener)
    }

    fn url(&self) -> String {
        format!(
            "http://{}",
            self.0.local_addr().expect("the listener's address")
        )
    }

    /// Whether anything connected since it was made.
    fn heard(&self) -> bool {
        match self.0.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::WouldBlock => false,
            Err(error) => panic!("the listener failed: {error}"),
        }
    }
}

/// What the built binary answers to `args` with `input` on its standard
/// input, run in `scratch` with every proxy pointed at `sentinel` and
/// `variables` set beside the few it needs.
fn asked(
    scratch: &Scratch,
    sentinel: &Sentinel,
    args: &[&str],
    input: Option<&[u8]>,
    variables: &[(&str, &str)],
) -> Output {
    let mut path = std::ffi::OsString::from(scratch.bin());
    if let Some(inherited) = std::env::var_os("PATH") {
        path.push(":");
        path.push(inherited);
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_crucible"));
    command
        .args(args)
        .env_clear()
        .env("PATH", path)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("HOME", scratch.home())
        .env("CRUCIBLE_CODE_HOME", scratch.crucible())
        .current_dir(scratch.work())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for proxy in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(proxy, sentinel.url());
    }
    for (name, value) in variables {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("the built binary runs");
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("a piped standard input");
        // A run that refuses early closes its end first; what it refused is
        // what the test reads, not the broken pipe.
        let _ = stdin.write_all(input);
    }
    child.wait_with_output().expect("the built binary ends")
}

/// What one entry under a scratch directory is, as far as a test compares it.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Directory,
    /// The store's lock file, recorded by presence alone.
    Lock,
    File(Vec<u8>),
}

/// The name of the lock file a store change takes.
const LOCK: &str = "auth.lock";

/// Every entry under `root`, with its bytes where it is a file.
///
/// The lock file is recorded by presence only, on every platform: a test that
/// holds the lock while it runs would otherwise read a file Windows refuses to
/// read while another handle holds a lock on its bytes.
fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut seen = BTreeMap::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        for entry in fs::read_dir(&directory).expect("a directory this test made") {
            let at = entry.expect("an entry").path();
            if at.is_dir() {
                seen.insert(at.clone(), Entry::Directory);
                left.push(at);
            } else if at.file_name().is_some_and(|name| name == LOCK) {
                seen.insert(at, Entry::Lock);
            } else {
                let bytes = fs::read(&at).expect("its bytes");
                seen.insert(at, Entry::File(bytes));
            }
        }
    }
    seen
}

/// Whether `needle` is anywhere in `haystack`.
fn carries(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Panics where `answered` repeated the sentinel on either stream.
fn unrepeated(answered: &Output) {
    assert!(
        !carries(&answered.stdout, SENTINEL),
        "standard output repeated the key"
    );
    assert!(
        !carries(&answered.stderr, SENTINEL),
        "standard error repeated the key"
    );
}

#[test]
fn a_key_given_on_the_command_line_is_refused_and_never_repeated() {
    let scratch = Scratch::new("argv");
    let sentinel = Sentinel::new();
    let before = tree(scratch.root());
    let equals = format!("--api-key={SENTINEL}");
    // A word where a value is not taken is the parser's, which ends 2; a key
    // given as the provider is a provider nobody serves, which ends 1. Neither
    // is said back.
    for (args, code) in [
        (vec!["auth", "login", "anthropic", SENTINEL], 2),
        (vec!["auth", "login", "anthropic", "--api-key", SENTINEL], 2),
        (vec!["auth", "login", "anthropic", equals.as_str()], 2),
        (vec!["auth", "logout", "anthropic", SENTINEL], 2),
        (vec!["auth", "status", "anthropic", SENTINEL], 2),
        (vec!["auth", "login", SENTINEL], 1),
        (vec!["auth", "logout", SENTINEL], 1),
        (vec!["auth", "status", SENTINEL], 1),
    ] {
        let answered = asked(&scratch, &sentinel, &args, None, &[]);

        assert_eq!(
            answered.status.code(),
            Some(code),
            "{} was not refused as it should be",
            args.len()
        );
        unrepeated(&answered);
    }
    assert_eq!(tree(scratch.root()), before, "something was written");
    assert!(!sentinel.heard(), "something reached out");
}

/// A store holding an Anthropic key, OpenAI's key, a Kimi account login
/// whose access lapsed long ago, and a key under a name no row of this
/// build's stands for. Every value is made up.
const HELD: &str = r#"{"version":2,"keys":{"anthropic":"fabricated-anthropic-key","openai":"fabricated-openai-key","custom@example.com":"fabricated-custom-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{"device_id":"01234567-89ab-4cde-8fab-0123456789ab","expires_in":"3600"},"expires_at":1000000000,"refreshed_at":1}},"identities":{"moonshot@kimi.ai":"00000000-0000-4000-8000-000000000000"}}"#;

/// The programs a login could start on the way, each replaced in `scratch`'s
/// own directory first on the path by one that writes what it was started
/// with to `started.log` there and does nothing else.
#[cfg(unix)]
fn shimmed(scratch: &Scratch) {
    use std::os::unix::fs::PermissionsExt as _;

    let log = scratch.bin().join("started.log");
    for program in [
        "xdg-open",
        "open",
        "gio",
        "sensible-browser",
        "x-www-browser",
        "firefox",
        "git",
        "sh",
        "bash",
        "security",
        "secret-tool",
    ] {
        let at = scratch.bin().join(program);
        fs::write(
            &at,
            format!(
                "#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> '{}'\n",
                log.display()
            ),
        )
        .expect("a shim");
        fs::set_permissions(&at, fs::Permissions::from_mode(0o755)).expect("an executable shim");
    }
}

/// Whether anything a login started was handed the sentinel.
#[cfg(unix)]
fn started_with_it(scratch: &Scratch) -> bool {
    fs::read(scratch.bin().join("started.log")).is_ok_and(|log| carries(&log, SENTINEL))
}

/// The lock a store change takes, held by this process until dropped.
fn locked(scratch: &Scratch) -> fs::File {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(scratch.crucible().join(LOCK))
        .expect("the lock file");
    lock.try_lock().expect("the lock is free");
    lock
}

#[cfg(unix)]
#[test]
fn a_key_piped_in_is_stored_under_its_row_and_repeated_nowhere_else() {
    let scratch = Scratch::new("piped");
    let sentinel = Sentinel::new();
    shimmed(&scratch);
    let input = format!("{SENTINEL}\n");

    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "login", "anthropic", "--api-key-stdin"],
        Some(input.as_bytes()),
        &[],
    );

    assert_eq!(answered.status.code(), Some(0), "the piped login failed");
    unrepeated(&answered);
    let said = String::from_utf8_lossy(&answered.stdout);
    assert!(
        said.contains("under anthropic"),
        "the login did not say the row it stored under"
    );
    let store = fs::read_to_string(scratch.store()).expect("the store was written");
    assert!(
        store.contains(&format!(r#""anthropic":"{SENTINEL}""#)),
        "stored under its row's name"
    );
    for (path, entry) in tree(scratch.root()) {
        if path == scratch.store() {
            continue;
        }
        // Nothing holds the lock once the run has ended, so its bytes are
        // read here, where the comparison above records it by presence alone.
        let bytes = match entry {
            Entry::Directory => continue,
            Entry::Lock => fs::read(&path).expect("its bytes"),
            Entry::File(bytes) => bytes,
        };
        assert!(
            !carries(&bytes, SENTINEL),
            "{} holds the key",
            path.display()
        );
    }
    assert!(!started_with_it(&scratch), "a process was handed the key");
    assert!(!sentinel.heard(), "something reached out");
}

#[test]
fn a_key_longer_than_the_bound_is_refused_and_nothing_is_stored() {
    let scratch = Scratch::new("oversized");
    let sentinel = Sentinel::new();
    let before = tree(scratch.root());
    let mut input = SENTINEL.as_bytes().to_vec();
    input.resize(16 * 1024 * 4, b'k');

    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "login", "anthropic", "--api-key-stdin"],
        Some(&input),
        &[],
    );

    assert_eq!(answered.status.code(), Some(1));
    unrepeated(&answered);
    let said = String::from_utf8_lossy(&answered.stderr);
    assert!(
        said.contains("longer than 16384 bytes"),
        "the refusal did not say the bound"
    );
    assert!(
        said.contains("nothing was stored"),
        "the refusal did not say nothing was stored"
    );
    assert_eq!(tree(scratch.root()), before, "something was written");
}

#[test]
fn a_login_with_no_terminal_and_no_pipe_says_what_to_run_instead() {
    let scratch = Scratch::new("unasked");
    let sentinel = Sentinel::new();
    let before = tree(scratch.root());

    // A key row, a row that is a key or an account, and an account sign-in:
    // none of them has a terminal to ask on, and none waits for one.
    for provider in ["anthropic", "openai", "moonshot@kimi.ai"] {
        let answered = asked(&scratch, &sentinel, &["auth", "login", provider], None, &[]);

        assert_eq!(answered.status.code(), Some(1), "{provider}");
        let said = String::from_utf8_lossy(&answered.stderr);
        assert!(said.contains("--api-key-stdin"), "{provider}: {said}");
        assert!(said.contains("terminal"), "{provider}: {said}");
        assert!(answered.stdout.is_empty(), "{provider}");
    }
    assert_eq!(tree(scratch.root()), before, "something was written");
    assert!(!sentinel.heard(), "something reached out");
}

#[test]
fn a_provider_nobody_serves_is_refused_by_every_command() {
    let scratch = Scratch::new("unknown");
    let sentinel = Sentinel::new();
    scratch.holding(HELD);
    let before = tree(scratch.root());

    for args in [
        vec!["auth", "status", "nonesuch"],
        vec!["auth", "login", "nonesuch"],
        vec!["auth", "login", "nonesuch", "--api-key-stdin"],
        vec!["auth", "logout", "nonesuch"],
    ] {
        let answered = asked(&scratch, &sentinel, &args, Some(b"made-up"), &[]);

        assert_eq!(answered.status.code(), Some(1), "{args:?}");
        let said = String::from_utf8_lossy(&answered.stderr);
        assert!(
            said.contains("this build serves anthropic"),
            "{args:?}: the names served were not said"
        );
        assert!(
            !said.contains("nonesuch"),
            "{args:?}: the word was repeated"
        );
    }
    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "status", "nonesuch", "--json"],
        None,
        &[],
    );
    assert_eq!(answered.status.code(), Some(1));
    let document = String::from_utf8_lossy(&answered.stdout);
    assert!(
        document
            .starts_with(r#"{"acceptance":"unchecked","format_version":1,"kind":"auth-status","#),
        "the document does not open with its header"
    );
    assert!(
        document.contains(r#""status":"failed""#),
        "the document does not say it failed"
    );
    assert!(
        document.contains(r#""providers":[]"#),
        "the document lists a provider"
    );
    assert_eq!(tree(scratch.root()), before, "something was written");
}

#[test]
fn status_writes_one_document_and_neither_writes_renews_nor_waits_on_the_lock() {
    let scratch = Scratch::new("status");
    let sentinel = Sentinel::new();
    scratch.holding(HELD);
    // Held the whole run: a status that took the store's lock, as a renewal
    // or a write does, would wait out its bound and say the store was busy.
    let lock = locked(&scratch);
    let before = tree(scratch.root());
    let started = std::time::Instant::now();

    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "status", "--json"],
        None,
        &[("XAI_API_KEY", SENTINEL)],
    );

    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "the status waited on the lock"
    );
    assert_eq!(answered.status.code(), Some(0), "the status failed");
    assert!(
        answered.stderr.is_empty(),
        "the status wrote to standard error"
    );
    unrepeated(&answered);
    let document = String::from_utf8(answered.stdout).expect("UTF-8");
    assert!(!document.contains("fabricated"), "a stored value was said");
    assert_eq!(
        document.matches('\n').count(),
        1,
        "the document is not one line"
    );
    assert!(
        document.ends_with("}\n"),
        "the document does not end its line"
    );
    for fragment in [
        r#""format_version":1"#,
        r#""kind":"auth-status""#,
        r#""status":"complete""#,
        r#""acceptance":"unchecked""#,
        r#""truncated":false"#,
        r#""expires_at":1000000000,"kind":"account","name":"moonshot@kimi.ai""#,
        r#""state":"expired""#,
        r#""source":"environment","state":"configured""#,
        r#""variable":"XAI_API_KEY","variable_configured":false,"variable_set":true"#,
    ] {
        assert!(
            document.contains(fragment),
            "{fragment} is not in the document"
        );
    }
    // The lapsed login is said to be lapsed, not renewed: the store is as it
    // was, and nothing was asked of anyone.
    assert_eq!(tree(scratch.root()), before, "something was written");
    assert!(!sentinel.heard(), "something reached out");
    drop(lock);
}

#[test]
fn a_logout_while_the_store_is_locked_changes_nothing_and_says_so() {
    let scratch = Scratch::new("busy");
    let sentinel = Sentinel::new();
    scratch.holding(HELD);
    let lock = locked(&scratch);
    let before = tree(scratch.root());

    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "logout", "moonshot"],
        None,
        &[],
    );

    assert_eq!(
        answered.status.code(),
        Some(1),
        "the logout under the lock did not end 1"
    );
    let said = String::from_utf8_lossy(&answered.stderr);
    assert!(
        said.contains("another crucible is writing"),
        "the logout did not say the store was busy"
    );
    assert_eq!(
        tree(scratch.root()),
        before,
        "the store changed under the lock"
    );
    drop(lock);
}

#[test]
fn a_logout_takes_out_one_provider_and_keeps_every_other_name() {
    let scratch = Scratch::new("logout");
    let sentinel = Sentinel::new();
    scratch.holding(HELD);

    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "logout", "moonshot"],
        None,
        &[],
    );

    assert_eq!(answered.status.code(), Some(0), "the logout failed");
    let said = String::from_utf8_lossy(&answered.stdout);
    assert!(
        said.contains(
            "removed the credential stored for Kimi Code · kimi.ai under moonshot@kimi.ai"
        ),
        "the logout did not say what it removed"
    );
    assert!(
        !said.contains("a launch still"),
        "nothing else signs it in, and the logout said something does"
    );
    let store = fs::read_to_string(scratch.store()).expect("the store");
    for kept in [
        r#""anthropic":"fabricated-anthropic-key""#,
        r#""openai":"fabricated-openai-key""#,
        r#""custom@example.com":"fabricated-custom-key""#,
    ] {
        assert!(store.contains(kept), "{kept} went");
    }
    assert!(
        !store.contains("fabricated-access"),
        "the account's access token is still stored"
    );
    assert!(
        !store.contains("fabricated-refresh"),
        "the account's refresh token is still stored"
    );
    assert!(!sentinel.heard(), "something reached out");
}

#[test]
fn a_logout_says_which_variable_still_signs_the_provider_in() {
    let scratch = Scratch::new("remaining");
    let sentinel = Sentinel::new();
    scratch.holding(HELD);

    let answered = asked(
        &scratch,
        &sentinel,
        &["auth", "logout", "anthropic"],
        None,
        &[("ANTHROPIC_API_KEY", SENTINEL)],
    );

    assert_eq!(answered.status.code(), Some(0), "the logout failed");
    unrepeated(&answered);
    let said = String::from_utf8_lossy(&answered.stdout);
    assert!(
        said.contains("removed the credential stored for Anthropic"),
        "the logout did not say what it removed"
    );
    assert!(
        said.contains("a launch still signs anthropic in with ANTHROPIC_API_KEY"),
        "the logout did not name the variable that still signs it in"
    );
    assert!(
        said.contains("unset it there"),
        "the logout did not say where to unset it"
    );
    let store = fs::read_to_string(scratch.store()).expect("the store");
    assert!(
        !store.contains("fabricated-anthropic-key"),
        "the Anthropic key is still stored"
    );
    assert!(
        store.contains("fabricated-openai-key"),
        "the OpenAI key went with it"
    );

    // The variable is still the shell's, and status still says it is used.
    let after = asked(
        &scratch,
        &sentinel,
        &["auth", "status", "anthropic"],
        None,
        &[("ANTHROPIC_API_KEY", SENTINEL)],
    );
    assert_eq!(after.status.code(), Some(0));
    let said = String::from_utf8_lossy(&after.stdout);
    assert!(
        said.contains("configured anthropic: ANTHROPIC_API_KEY is set"),
        "the status did not say the variable is used"
    );
    unrepeated(&after);
}
