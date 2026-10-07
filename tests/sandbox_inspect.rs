//! What `crucible sandbox inspect` and `--sandbox` answer on this machine, and
//! the exit each answer ends with; and, beside them, what `crucible config
//! check` makes of the same hostile configuration key.
//!
//! The built binary is run in a directory and a home of the test's own, with
//! an environment cleared down to what it needs, so nothing the machine running
//! this keeps is read and nothing is written anywhere a test did not make.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crucible_client_api::inspection::{Inspection, KIND};

/// A directory under the system temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-sandbox-inspect-{probe}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("work")).expect("a temporary directory");
        fs::create_dir_all(path.join("home")).expect("a temporary directory");
        Self(path)
    }

    fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the built binary answers to `args`, run in `scratch`, with crucible's
/// home named outright unless `homeless`.
fn asked(scratch: &Scratch, args: &[&str], homeless: bool) -> Output {
    asked_of(
        Path::new(env!("CARGO_BIN_EXE_crucible")),
        scratch,
        args,
        homeless,
    )
}

/// [`asked`], of the binary at `program`.
fn asked_of(program: &Path, scratch: &Scratch, args: &[&str], homeless: bool) -> Output {
    asked_in(program, scratch, &scratch.work(), args, homeless)
}

/// [`asked_of`], started in `here` rather than in the scratch's work directory.
fn asked_in(
    program: &Path,
    scratch: &Scratch,
    here: &Path,
    args: &[&str],
    homeless: bool,
) -> Output {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(here)
        .stdin(Stdio::null());
    if !homeless {
        command
            .env("HOME", scratch.home())
            .env("CRUCIBLE_CODE_HOME", scratch.home().join(".crucible"));
    }
    command.output().expect("the built binary runs")
}

/// Every entry under `root`, with its bytes where it is a file.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut seen = BTreeMap::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        for entry in fs::read_dir(&directory).expect("a directory this test made") {
            let at = entry.expect("an entry").path();
            if at.is_dir() {
                seen.insert(at.clone(), None);
                left.push(at);
            } else {
                seen.insert(at.clone(), Some(fs::read(&at).expect("its bytes")));
            }
        }
    }
    seen
}

#[test]
fn the_alias_and_the_command_write_one_report_and_succeed() {
    let scratch = Scratch::new("report");
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"sandbox":{"enabled":true}}"#,
    )
    .expect("a project file");
    let before = tree(&scratch.0);

    // Whatever this machine can confine with, and whether or not it would
    // take the policy, a report was made, and a report made is a success.
    let alias = asked(&scratch, &["--sandbox"], false);
    let command = asked(&scratch, &["sandbox", "inspect"], false);
    for answered in [&alias, &command] {
        assert_eq!(answered.status.code(), Some(0), "{answered:?}");
        assert!(answered.stderr.is_empty(), "{answered:?}");
    }
    let said = String::from_utf8_lossy(&alias.stdout);
    assert!(said.starts_with("sandbox enabled in "), "{said}");
    assert!(
        said.contains("  mode      required by project configuration"),
        "{said}"
    );
    assert!(said.ends_with('\n'), "{said}");
    // The alias is the command, word for word.
    assert_eq!(alias.stdout, command.stdout);

    let json = asked(&scratch, &["sandbox", "inspect", "--json"], false);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    assert!(json.stderr.is_empty(), "{json:?}");
    let line = json
        .stdout
        .strip_suffix(b"\n")
        .expect("one document, ending in a newline");
    assert!(!line.contains(&b'\n'), "more than one line");
    let written = String::from_utf8_lossy(line);
    assert!(written.contains(r#""format_version":1"#), "{written}");
    assert!(
        written.contains(&format!(r#""kind":"{KIND}""#)),
        "{written}"
    );
    let read = Inspection::decode(&json.stdout).expect("a document that reads back");
    assert!(
        ["ready", "refused", "unavailable"].contains(&read.status()),
        "{written}"
    );
    // No path reaches the document, not even the one asked about.
    assert!(
        !written.contains(&*scratch.work().to_string_lossy()),
        "{written}"
    );

    // Asking three times wrote nothing, here or in crucible's home.
    assert_eq!(tree(&scratch.0), before);
}

#[test]
fn an_inspection_that_could_not_be_made_fails_in_both_forms() {
    let scratch = Scratch::new("homeless");

    // With no home to read configuration from, there is no policy to report
    // on. Text goes where a failure always goes; the document still comes, so
    // a script reading standard output is never left with nothing.
    for args in [&["--sandbox"][..], &["sandbox", "inspect"]] {
        let answered = asked(&scratch, args, true);
        assert_eq!(answered.status.code(), Some(1), "{answered:?}");
        assert!(answered.stdout.is_empty(), "{answered:?}");
        assert!(answered.stderr.starts_with(b"crucible: "), "{answered:?}");
    }

    let json = asked(&scratch, &["sandbox", "inspect", "--json"], true);
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    assert!(json.stderr.starts_with(b"crucible: "), "{json:?}");
    let read = Inspection::decode(&json.stdout).expect("a document that reads back");
    assert_eq!(read.status(), "failed");
}

#[test]
fn a_command_line_that_does_not_parse_is_a_usage_error() {
    let scratch = Scratch::new("usage");
    for args in [
        &["sandbox", "inspect", "--bogus"][..],
        &["sandbox", "inspect", "--json", "extra"],
        &["--sandbox", "sandbox", "inspect"],
    ] {
        let answered = asked(&scratch, args, false);
        assert_eq!(answered.status.code(), Some(2), "{args:?}: {answered:?}");
        assert!(answered.stdout.is_empty(), "{answered:?}");
    }
}

/// Every place `said` names a path under `scratch`.
fn paths_in(said: &str, scratch: &Scratch) -> Vec<String> {
    let under = scratch.0.to_string_lossy().into_owned();
    said.match_indices(&under)
        .map(|(at, _)| {
            said.get(at..)
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect()
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn a_backend_that_was_not_found_is_reported_without_the_paths_looked_at() {
    // A copy of the binary, so the broker crucible looks for beside itself is
    // looked for in this test's directory, where none was ever built.
    let scratch = Scratch::new("unavailable");
    let program = scratch.0.join("bin/crucible");
    fs::create_dir_all(scratch.0.join("bin")).expect("a directory for the binary");
    fs::copy(env!("CARGO_BIN_EXE_crucible"), &program).expect("a copy of the binary");
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"sandbox":{"enabled":true}}"#,
    )
    .expect("a project file");

    let json = asked_of(&program, &scratch, &["sandbox", "inspect", "--json"], false);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    let written = String::from_utf8_lossy(&json.stdout);
    let read = Inspection::decode(&json.stdout).expect("a document that reads back");
    assert_eq!(read.status(), "unavailable", "{written}");
    assert_eq!(
        paths_in(&written, &scratch),
        Vec::<String>::new(),
        "{written}"
    );

    // The text names the directory asked about on its first line and no path
    // after it.
    let text = asked_of(&program, &scratch, &["sandbox", "inspect"], false);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let said = String::from_utf8_lossy(&text.stdout);
    let (first, rest) = said.split_once('\n').expect("a first line");
    assert!(first.starts_with("sandbox enabled in "), "{said}");
    assert!(rest.contains("no sandbox backend was found"), "{said}");
    assert_eq!(paths_in(rest, &scratch), Vec::<String>::new(), "{said}");
}

#[test]
fn a_report_that_could_not_be_made_does_not_name_the_file_that_stopped_it() {
    let scratch = Scratch::new("unreadable-config");
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"not_a_setting":true}"#,
    )
    .expect("a project file");

    let json = asked(&scratch, &["sandbox", "inspect", "--json"], false);
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let written = String::from_utf8_lossy(&json.stdout);
    let read = Inspection::decode(&json.stdout).expect("a document that reads back");
    assert_eq!(read.status(), "failed", "{written}");
    assert_eq!(
        paths_in(&written, &scratch),
        Vec::<String>::new(),
        "{written}"
    );
    // The run's own failure still says which file, where a failure is said,
    // spelled the way this platform spells it.
    let file = Path::new(".crucible").join("config.json");
    assert!(
        String::from_utf8_lossy(&json.stderr).contains(&*file.to_string_lossy()),
        "{json:?}"
    );

    // As text there is no report at all, only the failure.
    let text = asked(&scratch, &["sandbox", "inspect"], false);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    assert!(text.stdout.is_empty(), "{text:?}");
}

/// Every character in `written` a terminal would act on rather than draw: ESC,
/// BEL, the C1 controls and the rest, a line break aside.
fn controls_in(written: &[u8]) -> Vec<char> {
    String::from_utf8_lossy(written)
        .chars()
        .filter(|character| character.is_control() && *character != '\n')
        .collect()
}

#[cfg(unix)]
#[test]
fn a_directory_named_to_retitle_the_terminal_is_named_with_its_escapes_shown() {
    let scratch = Scratch::new("hostile-root");
    let here = scratch.work().join("sub\u{1b}]0;T\u{7}");
    fs::create_dir_all(&here).expect("a directory with a hostile name");

    let text = asked_in(
        Path::new(env!("CARGO_BIN_EXE_crucible")),
        &scratch,
        &here,
        &["sandbox", "inspect"],
        false,
    );
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert_eq!(controls_in(&text.stdout), Vec::<char>::new(), "{text:?}");
    let said = String::from_utf8_lossy(&text.stdout);
    let (first, _) = said.split_once('\n').expect("a first line");
    assert!(first.ends_with(r"sub\u{1b}]0;T\u{7}"), "{said}");
}

#[test]
fn a_configuration_key_that_carries_escapes_is_named_with_them_shown() {
    let scratch = Scratch::new("hostile-key");
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"\u001b]0;PWNED\u0007\u001b[31mred\u009b2J":true}"#,
    )
    .expect("a project file");

    let text = asked(&scratch, &["sandbox", "inspect"], false);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    assert_eq!(controls_in(&text.stderr), Vec::<char>::new(), "{text:?}");
    let said = String::from_utf8_lossy(&text.stderr);
    assert!(
        said.contains(r"\u{1b}]0;PWNED\u{7}\u{1b}[31mred\u{9b}2J"),
        "{said}"
    );
}

#[test]
fn a_configuration_check_names_a_key_that_carries_escapes_with_them_shown() {
    let scratch = Scratch::new("hostile-check");
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"\u001b]0;PWNED\u0007\u001b[31mred\u009b2J":true}"#,
    )
    .expect("a project file");

    let text = asked(&scratch, &["config", "check"], false);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    assert_eq!(controls_in(&text.stdout), Vec::<char>::new(), "{text:?}");
    let said = String::from_utf8_lossy(&text.stdout);
    assert!(said.starts_with("configuration invalid\n"), "{said}");
    assert!(
        said.contains(r"\u{1b}]0;PWNED\u{7}\u{1b}[31mred\u{9b}2J"),
        "{said}"
    );
}

/// A project configuration whose one key carries an 8-bit CSI, an ESC, a line
/// break that would open a report line of its own, and a right-to-left
/// override: everything a checkout could use to make a report say what it does
/// not hold.
const HOSTILE_KEY: &str = "a\u{9b}2J\u{1b}[31m\n  schema: forged\u{202e}b";

/// [`asked`] of `config check`, `args` after it, in a project whose
/// configuration is [`HOSTILE_KEY`] alone.
fn checked_hostile(probe: &str, args: &[&str]) -> Output {
    checked_key(probe, HOSTILE_KEY, args)
}

/// [`asked`] of `config check`, `args` after it, in a project whose
/// configuration is `key` alone.
fn checked_key(probe: &str, key: &str, args: &[&str]) -> Output {
    let scratch = Scratch::new(probe);
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    let document = serde_json::json!({ key: true }).to_string();
    fs::write(scratch.work().join(".crucible/config.json"), document).expect("a project file");
    let mut asking = vec!["config", "check"];
    asking.extend_from_slice(args);
    asked(&scratch, &asking, false)
}

/// Every character in `written` that a terminal would act on, or that would
/// reorder or hide what is drawn, other than the line breaks in `breaks`.
fn unshown_in(written: &str, breaks: bool) -> Vec<char> {
    written
        .chars()
        .filter(|&character| {
            (character.is_control() && !(breaks && character == '\n'))
                || matches!(character, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

#[test]
fn a_configuration_check_written_as_json_escapes_what_a_terminal_would_act_on() {
    let json = checked_hostile("hostile-check-json", &["--json"]);
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let said = String::from_utf8(json.stdout).expect("UTF-8");
    let (document, rest) = said.split_once('\n').expect("one line");
    assert_eq!(rest, "", "{said:?}");
    assert_eq!(unshown_in(document, false), Vec::<char>::new(), "{said:?}");

    let read: serde_json::Value = serde_json::from_str(document).expect("one JSON document");
    let message = read
        .pointer("/failures/0/message")
        .and_then(serde_json::Value::as_str)
        .expect("a failure");
    assert!(message.contains(HOSTILE_KEY), "{message:?}");
}

#[test]
fn a_configuration_check_shows_a_hostile_key_on_its_own_line() {
    let text = checked_hostile("hostile-check-text", &[]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let said = String::from_utf8(text.stdout).expect("UTF-8");
    assert_eq!(unshown_in(&said, true), Vec::<char>::new(), "{said:?}");
    let schemas = said
        .lines()
        .filter(|line| line.trim_start().starts_with("schema:"))
        .count();
    assert_eq!(schemas, 1, "{said}");
    assert!(
        said.contains(r"a\u{9b}2J\u{1b}[31m\n  schema: forged\u{202e}b"),
        "{said}"
    );
}

#[test]
fn a_configuration_check_that_fails_says_why_with_the_override_shown() {
    let text = checked_hostile("hostile-check-stderr", &[]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let said = String::from_utf8(text.stderr).expect("UTF-8");
    assert_eq!(unshown_in(&said, true), Vec::<char>::new(), "{said:?}");
    assert!(said.contains(r"forged\u{202e}b"), "{said}");
}

#[test]
fn a_configuration_check_shows_a_line_or_paragraph_separator_in_a_key_as_its_escape() {
    // Some terminals and viewers end a line at either separator, so one left
    // raw would start a report line the key was never given.
    const KEY: &str = "a\u{2028}  schema: forged\u{2029}b";
    let separators = |said: &str| {
        said.chars()
            .filter(|character| matches!(character, '\u{2028}' | '\u{2029}'))
            .collect::<Vec<_>>()
    };

    let text = checked_key("separator-check-text", KEY, &[]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let said = String::from_utf8(text.stdout).expect("UTF-8");
    let told = String::from_utf8(text.stderr).expect("UTF-8");
    assert_eq!(separators(&said), Vec::<char>::new(), "{said:?}");
    assert_eq!(separators(&told), Vec::<char>::new(), "{told:?}");
    assert!(
        said.contains(r"a\u{2028}  schema: forged\u{2029}b"),
        "{said}"
    );

    let json = checked_key("separator-check-json", KEY, &["--json"]);
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let said = String::from_utf8(json.stdout).expect("UTF-8");
    assert_eq!(separators(&said), Vec::<char>::new(), "{said:?}");
    assert!(said.contains(r"a\u2028  schema: forged\u2029b"), "{said}");
    let read: serde_json::Value = serde_json::from_str(said.trim_end()).expect("one JSON document");
    let message = read
        .pointer("/failures/0/message")
        .and_then(serde_json::Value::as_str)
        .expect("a failure");
    assert!(message.contains(KEY), "{message:?}");
}

#[cfg(unix)]
#[test]
fn a_configuration_check_names_a_hostile_directory_with_its_escapes_shown() {
    let scratch = Scratch::new("hostile-check-root");
    let here = scratch.work().join("sub\u{1b}]0;T\u{7}\u{202e}");
    fs::create_dir_all(&here).expect("a directory with a hostile name");

    let text = asked_in(
        Path::new(env!("CARGO_BIN_EXE_crucible")),
        &scratch,
        &here,
        &["config", "check"],
        false,
    );
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let said = String::from_utf8(text.stdout).expect("UTF-8");
    assert_eq!(unshown_in(&said, true), Vec::<char>::new(), "{said:?}");
    assert!(
        said.contains(r"sub\u{1b}]0;T\u{7}\u{202e}/.crucible/config.json: absent"),
        "{said}"
    );
}
