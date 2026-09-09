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
//!   `<home>`, `<workspace>` or `<root>`;
//! - the separators inside a path that begins at one of those, which are the
//!   platform's rather than the answer's, become `/`.
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

use crucible_config::{Extensions, HOME, Home, Settings};
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

/// Replaces the values that differ between two correct runs.
fn settled(text: &str, root: &Path, stands_for: &str) -> String {
    let here = root.display().to_string();
    let escaped = here.replace('\\', "\\\\");

    let named = text
        .replace(&escaped, stands_for)
        .replace(&here, stands_for);
    slashed(&named, stands_for).replace(env!("CARGO_PKG_VERSION"), "<version>")
}

/// Turns the separators inside a path that begins at `mark` into `/`.
///
/// Only inside one. A backslash anywhere else in a rendering — an escape in a
/// quoted string, a character class in a pattern — is part of the answer, and
/// rewriting those would be the probe agreeing that two different answers are
/// the same. What is not part of the answer is which separator the machine
/// running the probe writes its own temporary directory with.
fn slashed(text: &str, mark: &str) -> String {
    let mut rendered = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(at) = rest.find(mark) {
        let after = at + mark.len();
        rendered.push_str(&rest[..after]);
        rest = &rest[after..];

        let ends = |one: char| one.is_whitespace() || matches!(one, '"' | ',' | ')' | ':');
        let end = rest.find(ends).unwrap_or(rest.len());
        rendered.push_str(&rest[..end].replace("\\\\", "/").replace('\\', "/"));
        rest = &rest[end..];
    }

    rendered.push_str(rest);
    rendered
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

// ------------------------------------------------------------------ settings

/// A home directory found the way the binary finds one, touching no disk.
fn homed(at: &Path) -> Home {
    let named = at.as_os_str().to_owned();
    Home::find(&move |wanted| (wanted == HOME).then(|| named.clone())).expect("an absolute path")
}

/// Writes one fixture file, making whatever directory it needs.
fn wrote(root: &Path, at: &str, text: &str) {
    let path = root.join(at);
    fs::create_dir_all(path.parent().expect("a file has a directory"))
        .expect("a writable temporary directory");
    fs::write(&path, text).expect("a writable temporary directory");
}

/// Renders what every public accessor answers, provider by provider.
///
/// Names that no layer mentioned are asked for too. What a setting nobody wrote
/// resolves to is as much a precedence answer as what a contested one does, and
/// it is the half that changes silently when a default moves.
fn resolved(settings: &Settings) -> String {
    let mut rendered = String::new();
    let _ = writeln!(rendered, "provider           {:?}", settings.provider());

    for provider in ["anthropic", "google", "openai", "unmentioned"] {
        let _ = writeln!(rendered, "providers.{provider}");
        let _ = writeln!(
            rendered,
            "  model            {:?}",
            settings.model(provider)
        );
        let _ = writeln!(
            rendered,
            "  baseUrl          {:?}",
            settings.base_url(provider)
        );
        let _ = writeln!(
            rendered,
            "  effort           {:?}",
            settings.effort(provider)
        );
        let _ = writeln!(
            rendered,
            "  apiKeyEnv        {:?}",
            settings.api_key_env(provider)
        );
    }

    let _ = writeln!(
        rendered,
        "env                {:?}",
        settings.env().collect::<Vec<_>>()
    );

    for id in ["acme.reviewer", "acme.unmentioned"] {
        let _ = writeln!(rendered, "extensions.{id}");
        let _ = writeln!(
            rendered,
            "  enabled          {:?}",
            settings.extension_enabled(id)
        );
        let _ = writeln!(
            rendered,
            "  digest           {:?}",
            settings.extension_digest(id)
        );
        let _ = writeln!(
            rendered,
            "  config           {:?}",
            settings.extension_settings(id)
        );
    }

    let _ = writeln!(
        rendered,
        "sandbox enabled    {:?}",
        settings.sandbox_enabled()
    );
    rendered
}

/// Renders whichever a read produced, so a refusal is frozen like an answer.
fn read_settings(home: &Home, workspace: &Path) -> String {
    match Settings::read(home, workspace) {
        Ok(settings) => format!("{}{settings:#?}\n", resolved(&settings)),
        Err(refusal) => format!("refused: {refusal}\n"),
    }
}

/// The layer outside the checkout, which is the only one that may widen.
const USER_LAYER: &str = r#"{
  "provider": "anthropic",
  "providers": {
    "anthropic": {
      "model": "user-model",
      "baseUrl": "https://user.example/v1",
      "effort": "high",
      "apiKeyEnv": "EXAMPLE_KEY_VARIABLE"
    },
    "openai": { "model": "user-openai-model" }
  },
  "env": {
    "CRUCIBLE_CODE_SHARED": "user layer",
    "CRUCIBLE_CODE_USER_ONLY": "user layer"
  },
  "extensions": {
    "acme.reviewer": {
      "enabled": true,
      "digest": "sha256:0101010101010101010101010101010101010101010101010101010101010101",
      "config": { "verbosity": "brief", "audience": "team" }
    }
  },
  "permissions": { "allow": ["read(src/**)"], "deny": ["bash(rm -rf /)"] }
}
"#;

/// The checked-in project layer, nearer and unable to widen.
const PROJECT_LAYER: &str = r#"{
  "providers": {
    "anthropic": { "model": "project-model" },
    "google": { "model": "project-google-model" }
  },
  "env": { "CRUCIBLE_CODE_SHARED": "project layer" },
  "permissions": { "deny": ["bash(curl)"] }
}
"#;

/// The nearest layer of all, conventionally not committed.
const LOCAL_LAYER: &str = r#"{
  "providers": { "anthropic": { "effort": "low" } },
  "env": { "CRUCIBLE_CODE_SHARED": "project-local layer" },
  "permissions": { "ask": ["write(**)"] }
}
"#;

/// A project layer reaching for the key a request is sent with.
const WIDENING_LAYER: &str = r#"{
  "providers": { "anthropic": { "apiKeyEnv": "PLANTED_KEY_VARIABLE" } }
}
"#;

/// A project layer naming a variable that is not crucible's own.
const PLANTING_LAYER: &str = r#"{
  "env": { "PATH_HELPER": "planted" }
}
"#;

#[test]
fn the_configuration_layers_answer_which_one_won() {
    let scratch = Scratch::new("config");
    let root = scratch.path();
    let home = homed(&root.join("home"));
    let workspace = root.join("project");

    wrote(root, "home/config.json", USER_LAYER);
    let mut rendered = String::from("=== the user layer alone ===\n");
    rendered.push_str(&read_settings(&home, &workspace));

    wrote(root, "project/.crucible/config.json", PROJECT_LAYER);
    rendered.push_str("=== the project layer over it ===\n");
    rendered.push_str(&read_settings(&home, &workspace));

    wrote(root, "project/.crucible/config.local.json", LOCAL_LAYER);
    rendered.push_str("=== the project-local layer over both ===\n");
    rendered.push_str(&read_settings(&home, &workspace));

    // The two refusals are frozen beside the answers because they are the same
    // question: what a workspace layer may decide. A key that stopped being
    // refused would otherwise look like a settings change rather than the loss
    // of a boundary.
    wrote(root, "project/.crucible/config.json", WIDENING_LAYER);
    rendered.push_str("=== a project layer reaching for the key ===\n");
    rendered.push_str(&read_settings(&home, &workspace));

    wrote(root, "project/.crucible/config.json", PLANTING_LAYER);
    rendered.push_str("=== a project layer naming a variable of its own ===\n");
    rendered.push_str(&read_settings(&home, &workspace));

    same("configuration-layers", &settled(&rendered, root, "<root>"));
}

// -------------------------------------------------------------- cache policy

/// Renders the prompt-cache policy the given two layers resolve to.
fn cached(scratch: &Scratch, user: &str, project: Option<&str>) -> String {
    let root = scratch.path();
    let workspace = root.join("project");
    let checked_in = workspace.join(".crucible").join("config.json");

    wrote(root, "home/config.json", user);
    let _ = fs::remove_file(&checked_in);
    if let Some(project) = project {
        wrote(root, "project/.crucible/config.json", project);
    } else {
        fs::create_dir_all(checked_in.parent().expect("a file has a directory"))
            .expect("a writable temporary directory");
    }

    match Settings::read(&homed(&root.join("home")), &workspace) {
        Ok(settings) => format!("{:#?}\n", settings.prompt_cache()),
        Err(refusal) => format!("refused: {refusal}\n"),
    }
}

/// Everything the block can state, in one layer that is allowed to state it.
const CACHE_USER: &str = r#"{
  "promptCaching": {
    "mode": "require",
    "allowedMechanisms": ["automaticPrefix", "explicitBreakpoints", "persistentContent"],
    "isolationScope": "user",
    "requestedRetention": { "class": "extended", "maxSeconds": 3600 },
    "persistentResources": { "mode": "create" },
    "namespace": "example-namespace"
  }
}
"#;

#[test]
fn the_prompt_cache_policy_answers_what_each_layer_was_allowed_to_say() {
    let scratch = Scratch::new("cache");

    let cells: [(&str, &str, Option<&str>); 5] = [
        ("nothing stated anywhere", "{}\n", None),
        ("the user layer states all of it", CACHE_USER, None),
        (
            "a workspace layer narrowing the mechanisms",
            CACHE_USER,
            Some("{\"promptCaching\":{\"allowedMechanisms\":[\"automaticPrefix\"]}}\n"),
        ),
        (
            "a workspace layer reaching past the ceiling",
            CACHE_USER,
            Some("{\"promptCaching\":{\"persistentResources\":{\"mode\":\"require\"}}}\n"),
        ),
        (
            "a workspace layer speaking with no user layer",
            "{}\n",
            Some("{\"promptCaching\":{\"mode\":\"observeOnly\"}}\n"),
        ),
    ];

    let mut rendered = String::new();
    for (about, user, project) in cells {
        let _ = writeln!(rendered, "=== {about} ===");
        rendered.push_str(&cached(&scratch, user, project));
    }

    same(
        "prompt-cache-policy",
        &settled(&rendered, scratch.path(), "<root>"),
    );
}

// ---------------------------------------------------------- source discovery

/// One installable manifest, as an extension author would write it.
fn manifest(id: &str) -> String {
    format!(
        r#"{{
  "id": "{id}",
  "version": "1.4.0",
  "protocol": "1.3",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.35.0",
  "capabilities": ["registerTools"],
  "contributions": ["tools"]
}}
"#
    )
}

#[test]
fn the_installed_extension_sweep_answers_what_it_read_and_refused() {
    let scratch = Scratch::new("sources");
    let root = scratch.path();

    // Named so that the directory order and the read order disagree: what the
    // sweep answers is the sorted order, and a sweep that started reporting the
    // filesystem's own would answer differently on two machines holding the
    // same extensions.
    wrote(
        root,
        "home/extensions/zeta/manifest.json",
        &manifest("acme.zeta"),
    );
    wrote(
        root,
        "home/extensions/alpha/manifest.json",
        &manifest("acme.alpha"),
    );
    wrote(
        root,
        "home/extensions/repeat/manifest.json",
        &manifest("acme.alpha"),
    );
    wrote(root, "home/extensions/broken/manifest.json", "{ not json\n");
    wrote(
        root,
        "home/extensions/unqualified/manifest.json",
        &manifest("reviewer"),
    );
    // Not a half-installed extension, and must stay out of both lists.
    wrote(root, "home/extensions/README.md", "not an extension\n");

    let found = Extensions::discover(&homed(&root.join("home")));

    let mut rendered = String::new();
    let _ = writeln!(rendered, "at        {}", found.at().display());
    let _ = writeln!(rendered, "stopped   {}", found.stopped());
    let _ = writeln!(rendered, "found     {}", found.found().len());
    for one in found.found() {
        let _ = writeln!(rendered, "{one:#?}");
    }
    let _ = writeln!(rendered, "refused   {}", found.refused().len());
    for one in found.refused() {
        // Said rather than shown: a refusal that could not open a file carries
        // the operating system's own wording, and `Display` is the rendering
        // this program puts in front of a person.
        let _ = writeln!(rendered, "{one}");
    }

    // The ceiling, counted rather than listed. What matters about it is that a
    // planted tree is cut off at a fixed number and says so, and sixty-five
    // renderings of one refusal would bury that in the fixture.
    let crowded = Scratch::new("sources-crowded");
    for one in 0..=64 {
        wrote(
            crowded.path(),
            &format!("home/extensions/one-{one:03}/manifest.json"),
            "{ not json\n",
        );
    }
    let swept = Extensions::discover(&homed(&crowded.path().join("home")));
    let _ = writeln!(rendered, "=== sixty-five installed directories ===");
    let _ = writeln!(rendered, "stopped   {}", swept.stopped());
    let _ = writeln!(rendered, "found     {}", swept.found().len());
    let _ = writeln!(rendered, "refused   {}", swept.refused().len());

    same("installed-extensions", &settled(&rendered, root, "<root>"));
}
