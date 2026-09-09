//! Frozen answers from the surfaces a restructuring is most likely to change.
//!
//! Each probe drives one public entry point and writes what it observed as
//! plain text, which is compared against a file frozen beside this one. The
//! files are read and compared here rather than through the snapshot library,
//! so no environment variable can rewrite an expectation into agreement.
//!
//! What a probe renders is deliberately more than an assertion would: a moved
//! field, a reordered list, a dropped clause and a changed exit code all reach
//! the comparison, because the thing being preserved is the whole answer and
//! not the part somebody thought to assert.
//!
//! Only these substitutions are applied before comparison, and each one stands
//! for a value that differs between two correct runs:
//!
//! - the crate version, which every release changes, becomes `<version>`;
//! - the probe's own temporary directory, which is new every run, becomes
//!   `<home>` or `<workspace>`.
//!
//! Nothing else is normalized. Ordering, error text, absent fields and the
//! exact wording of a message are the answer, and a probe that hid them would
//! be agreeing with whatever it was shown.

#![allow(clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use crucible_core::{Answered, Fetch, Put, Search};
use crucible_core::{
    Cancel, DescribeTool, Host, Page, Question, SearchResponse, SourceError, ToolProvenance,
    Workspace,
};
use crucible_tools::{
    AskUser, Bash, Edit, Glob, Grep, Held, Ledger, Plan, Read, TodoWrite, ToolSearch, WebFetch,
    WebSearch, Write,
};

/// The frozen answer for `name`, as a path.
fn frozen(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("differential")
        .join(format!("{name}.txt"))
}

/// Compares `observed` against the frozen answer, and says where both are.
///
/// The mismatch is not printed. These renderings run to thousands of bytes and
/// a diff of two of them in a test failure is unreadable; the two paths are
/// what somebody actually needs, and the second one is written where a test's
/// own scratch directory already is.
fn same(name: &str, observed: &str) {
    assert!(
        observed.len() > 32,
        "{name} rendered almost nothing, so its comparison would prove nothing"
    );

    let expected = frozen(name);
    let frozen_text = fs::read_to_string(&expected)
        .unwrap_or_else(|problem| panic!("{} could not be read: {problem}", expected.display()));
    if frozen_text == observed {
        return;
    }

    let spilled = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.txt"));
    fs::write(&spilled, observed)
        .unwrap_or_else(|problem| panic!("{} could not be written: {problem}", spilled.display()));
    panic!(
        "{name} no longer answers what was frozen for it\n  frozen:   {}\n  observed: {}",
        expected.display(),
        spilled.display()
    );
}

/// A directory of this probe's own, removed when the probe ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-differential-{probe}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Replaces the two values that differ between two correct runs.
fn settled(text: &str, root: &Path, stands_for: &str) -> String {
    text.replace(&root.display().to_string(), stands_for)
        .replace(env!("CARGO_PKG_VERSION"), "<version>")
}

// ---------------------------------------------------------------- command line

/// Runs the shipped binary with `args` and renders everything it answered.
///
/// The child gets an environment built from nothing rather than inherited, so
/// the run cannot read the person's own configuration, colour preference or
/// terminal size — and cannot answer differently on two machines because of
/// one. Only `PATH` is passed through, which none of these invocations reads.
///
/// Every invocation here exits on its arguments alone. Standard input is empty
/// so that a form which did wait for a person ends instead of hanging.
fn asked(home: &Path, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crucible"));
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("CRUCIBLE_CODE_HOME", home.join(".crucible"))
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("COLUMNS", "80")
        .current_dir(home)
        .stdin(Stdio::null());

    let answered = command.output().expect("the built binary runs");
    let mut rendered = String::new();
    let _ = writeln!(rendered, "$ crucible {}", args.join(" "));
    let _ = writeln!(
        rendered,
        "exit {}",
        answered
            .status
            .code()
            .map_or_else(|| "(signalled)".to_owned(), |code| code.to_string())
    );
    let _ = writeln!(rendered, "--- stdout ---");
    rendered.push_str(&String::from_utf8_lossy(&answered.stdout));
    let _ = writeln!(rendered, "--- stderr ---");
    rendered.push_str(&String::from_utf8_lossy(&answered.stderr));
    rendered.push('\n');
    rendered
}

#[test]
fn the_argument_only_command_surface_answers_what_it_did() {
    let scratch = Scratch::new("cli");
    let home = scratch.path();

    // Only forms whose whole answer is the command tree itself. What a run
    // reports about this machine — the extensions it found, the confinement it
    // would use — is a different question with a different right answer on
    // each platform, and is asked of the library below rather than frozen here.
    let forms: [&[&str]; 6] = [
        &["--version"],
        &["-h"],
        &["--help"],
        &["sandbox", "--help"],
        &["sandbox", "setup", "--help"],
        &["--no-such-flag"],
    ];

    let mut rendered = String::new();
    for args in forms {
        rendered.push_str(&asked(home, args));
    }

    same("command-line", &settled(&rendered, home, "<home>"));
}

// ------------------------------------------------------------------ tool schemas

/// A questioner that is never asked, so that the tool can be described.
struct Silent;
impl Put for Silent {
    fn put(&self, _: &[Question]) -> Option<Vec<Answered>> {
        None
    }
}

/// A source that is never reached, so that the two web tools can be described.
struct Unreached;
impl Search for Unreached {
    fn name(&self) -> &'static str {
        "unreached"
    }
    fn reaches(&self) -> Host {
        Host::Opaque("nothing is asked of this source".into())
    }
    fn search(&self, _: &str, _: &Cancel) -> Result<SearchResponse, SourceError> {
        panic!("the schema probe never searches")
    }
}
impl Fetch for Unreached {
    fn name(&self) -> &'static str {
        "unreached"
    }
    fn reaches(&self, _: &str) -> Host {
        Host::Opaque("nothing is asked of this source".into())
    }
    fn fetch(&self, _: &str, _: &Cancel) -> Result<Page, SourceError> {
        panic!("the schema probe never fetches")
    }
}

/// Renders one tool's whole advertised contract, schema text included.
fn described(tool: &dyn DescribeTool) -> String {
    let provenance = ToolProvenance::builtin(tool.name()).expect("a built-in name");
    let descriptor = tool
        .descriptor(provenance)
        .expect("a describable built-in tool");

    let mut rendered = String::new();
    let _ = writeln!(rendered, "## {}", descriptor.name());
    let _ = writeln!(rendered, "effect      {:?}", descriptor.effect());
    let _ = writeln!(rendered, "execution   {:?}", descriptor.execution());
    let _ = writeln!(rendered, "timeout     {:?}", descriptor.timeout());
    let _ = writeln!(rendered, "result      {:?}", descriptor.result_bytes());
    let _ = writeln!(rendered, "provenance  {:?}", descriptor.provenance());
    let _ = writeln!(rendered, "schema");
    rendered.push_str(descriptor.schema());
    rendered.push_str("\n\n");
    rendered
}

#[test]
fn every_built_in_tool_advertises_what_it_did() {
    let scratch = Scratch::new("tools");
    let workspace = Workspace::open(scratch.path()).expect("a workspace");
    let ledger = Ledger::new();
    let unreached = Arc::new(Unreached);

    let held = vec![Held {
        name: "held".into(),
        about: "one held tool, so the search has something to offer".into(),
    }];

    let tools: [Box<dyn DescribeTool>; 11] = [
        Box::new(AskUser::new(Arc::new(Silent))),
        Box::new(Bash::new(workspace.clone())),
        Box::new(Edit::new(workspace.clone())),
        Box::new(Glob::new(workspace.clone())),
        Box::new(Grep::new(workspace.clone())),
        Box::new(Read::new(workspace.clone(), ledger.clone())),
        Box::new(TodoWrite::new(Plan::new())),
        Box::new(ToolSearch::new(held, crucible_core::Revealed::new())),
        Box::new(WebFetch::new(unreached.clone())),
        Box::new(WebSearch::new(unreached)),
        Box::new(Write::new(workspace, ledger)),
    ];

    let mut rendered = String::new();
    for tool in &tools {
        rendered.push_str(&described(tool.as_ref()));
    }

    same(
        "tool-schemas",
        &settled(&rendered, scratch.path(), "<workspace>"),
    );
}
