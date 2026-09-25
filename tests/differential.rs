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
//!   platform's rather than the answer's, become `/`;
//! - the file name the built binary was started as, which carries the suffix
//!   the platform gives an executable, becomes the one name `crucible`;
//! - the stable prefix a caching cell sends, which is one fixed constant long
//!   enough to cross every reviewed threshold, becomes `<stable prefix>`. It is
//!   the one substitution that stands for a value which does not vary: it
//!   stands for a value too large to read, and a request that truncated,
//!   reordered or split it would no longer match it and would be rendered
//!   whole.
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
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_builtins::{
    AskUser, Bash, Edit, Glob, Grep, Held, Ledger, Plan, Read, TodoWrite, ToolSearch, WebFetch,
    WebSearch, Write,
};
use crucible_config::{HOME, Home, Settings};
use crucible_core::{
    Ancestry, Calibration, Carried, ContextSnapshot, Fragment, RunId, RunItem, Spend,
};
use crucible_core::{Answered, Fetch, Put, Search};
use crucible_core::{
    Authorization, Cancel, Credential, CredentialScopeId, DescribeTool, Host, Message, Outgoing,
    Page, PromptCacheFingerprint, PromptCacheIdentity, PromptCacheKey, PromptCacheMechanism,
    PromptCacheMechanisms, PromptCachePlan, PromptCachePolicy, PromptCacheProjection,
    PromptCacheRequest, PromptCacheRetention, PromptCacheScopeDigest, PromptCacheSelected,
    PromptCacheSelection, Provider, ProviderAttemptId, Question, RecordedToolOutput, Request,
    RequestPurpose, SearchResponse, SourceError, StopReason, ToolArgs, ToolCall, ToolId,
    ToolProvenance, ToolResult, ToolSchema, Transcript, Workspace,
};
use crucible_extension::Extensions;
use crucible_provider::{
    Anthropic, Google, Moonshot, OpenAi, PostResponse, Transport, TransportError,
};
use crucible_runtime::BoxFuture;
use crucible_session::Session;

/// The frozen answer for `name`, as a path.
fn frozen(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("differential")
        .join(format!("{name}.txt"))
}

/// Compares `observed` against the frozen answer, and says where both are.
///
/// The whole mismatch is not printed. These renderings run to thousands of
/// bytes and a diff of two of them in a test failure is unreadable, so the
/// observed answer is written beside the test's own scratch directory and the
/// two paths are named.
///
/// The first differing line is printed with them. A failure on a machine
/// somebody is sitting at is answered by the paths; a failure on a build runner
/// is not, because the file the second path names is gone before anyone can
/// open it. One line is what makes a remote failure readable without making a
/// local one unreadable.
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
        "{name} no longer answers what was frozen for it\n{}\n  frozen:   {}\n  observed: {}",
        parted(&frozen_text, observed),
        expected.display(),
        spilled.display()
    );
}

/// The first line the two answers disagree on, both sides bounded.
///
/// Bounded because one of these lines can be a whole rendered request. A line
/// that runs past the limit is cut and says so, which is enough to tell a
/// reordering from a rewording without printing the rest of the answer to find
/// out.
fn parted(frozen_text: &str, observed: &str) -> String {
    /// The most of one line either side prints.
    const WIDTH: usize = 200;

    let cut = |line: &str| {
        let kept: String = line.chars().take(WIDTH).collect();
        if kept.len() < line.len() {
            format!("{kept}… (line is {} bytes)", line.len())
        } else {
            kept
        }
    };

    for (number, (was, now)) in frozen_text.lines().zip(observed.lines()).enumerate() {
        if was != now {
            return format!(
                "  first difference at line {}\n    frozen:   {}\n    observed: {}",
                number + 1,
                cut(was),
                cut(now)
            );
        }
    }

    let (frozen_lines, observed_lines) = (frozen_text.lines().count(), observed.lines().count());
    format!(
        "  every shared line agrees; the answers are {frozen_lines} and {observed_lines} lines long"
    )
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
    let _ = writeln!(rendered, "$ {PROGRAM} {}", args.join(" "));
    let _ = writeln!(
        rendered,
        "exit {}",
        answered
            .status
            .code()
            .map_or_else(|| "(signalled)".to_owned(), |code| code.to_string())
    );
    // The substitution is applied to what the program wrote and not to the
    // whole rendering, so that the lines this function writes itself cannot be
    // rewritten by it.
    let named = |raw: &[u8]| String::from_utf8_lossy(raw).replace(invoked(), PROGRAM);
    let _ = writeln!(rendered, "--- stdout ---");
    rendered.push_str(&named(&answered.stdout));
    let _ = writeln!(rendered, "--- stderr ---");
    rendered.push_str(&named(&answered.stderr));
    rendered.push('\n');
    rendered
}

/// The one name a usage line is frozen under.
const PROGRAM: &str = "crucible";

/// The file name of the built binary, which is what a usage line reports.
///
/// The command line names itself from the file it was started as, so on Windows
/// every usage line says `crucible.exe`. That is the same program under the
/// spelling that platform gives an executable, not a second answer, so it is
/// read from the built path rather than written down — a probe that spelled the
/// suffix itself would be asserting the platform instead of asking it.
fn invoked() -> &'static str {
    Path::new(env!("CARGO_BIN_EXE_crucible"))
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the built binary has a name")
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
    fn put<'a>(
        &'a self,
        _: &'a [Question],
    ) -> crucible_runtime::BoxFuture<'a, Option<Vec<Answered>>> {
        Box::pin(async { None })
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
    fn search<'a>(
        &'a self,
        _: &'a str,
        _: &'a Cancel,
    ) -> BoxFuture<'a, Result<SearchResponse, SourceError>> {
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
    fn fetch<'a>(&'a self, _: &'a str, _: &'a Cancel) -> BoxFuture<'a, Result<Page, SourceError>> {
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
        Box::new(Bash::new(
            workspace.clone(),
            Arc::new(crucible_sandbox_local::LocalSandbox::new()),
        )),
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

    let found = Extensions::discover(homed(&root.join("home")).path());

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
    // planted tree is refused whole and says so, rather than being answered
    // from the part of it a sweep reached first.
    let crowded = Scratch::new("sources-crowded");
    for one in 0..=64 {
        wrote(
            crowded.path(),
            &format!("home/extensions/one-{one:03}/manifest.json"),
            "{ not json\n",
        );
    }
    let swept = Extensions::discover(homed(&crowded.path().join("home")).path());
    let _ = writeln!(rendered, "=== sixty-five installed directories ===");
    let _ = writeln!(rendered, "stopped   {}", swept.stopped());
    let _ = writeln!(rendered, "found     {}", swept.found().len());
    let _ = writeln!(rendered, "refused   {}", swept.refused().len());

    same("installed-extensions", &settled(&rendered, root, "<root>"));
}

// ------------------------------------------------------------------ provider

/// The secret this probe's credential applies, so redaction can be shown.
///
/// A value no vendor would send back and no schema would contain, so its
/// absence from a frozen body means the body never carried it, rather than
/// that nothing looked.
const KEY: &str = "differential-probe-key-never-a-real-one";

/// A credential that authorizes the way a key does, and protects what it wrote.
#[derive(Debug)]
struct Keyed;

impl Credential for Keyed {
    fn scope(&self) -> CredentialScopeId {
        // Fixed rather than minted. A fresh scope every run would put a value
        // in the rendering that differs between two correct runs, and there is
        // no identity material here to derive a stable one from.
        CredentialScopeId::from_digest([7; 32])
    }

    fn authorize<'a>(&'a self, request: &'a mut Outgoing) -> Authorization<'a> {
        request.set_header("authorization", format!("Bearer {KEY}"));
        request.set_header("x-api-key", KEY);
        request.protect(KEY);
        Box::pin(std::future::ready(Ok(())))
    }
}

/// One request a provider made, kept where the probe can read it.
#[derive(Clone)]
struct Posted {
    url: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// A transport that answers nothing and keeps what it was asked to send.
///
/// The response is deliberately empty. What is frozen here is the request, and
/// a recorded reply would be a second thing to keep current for no gain. Every
/// provider therefore fails to read an answer, which each cell ignores: the
/// bytes were on the wire before that, and they are the whole question.
#[derive(Clone, Default)]
struct Recorder(Arc<std::sync::Mutex<Vec<Posted>>>);

impl std::fmt::Debug for Recorder {
    /// By hand, and saying nothing. The derived one would print every request
    /// this has kept, and a request carries the header a credential wrote.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recorder")
    }
}

impl Recorder {
    /// What was posted, in the order it was posted.
    fn posted(&self) -> Vec<Posted> {
        self.0.lock().map(|kept| kept.clone()).unwrap_or_default()
    }
}

impl Transport for Recorder {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
        if let Ok(mut kept) = self.0.lock() {
            kept.push(Posted {
                url: url.to_owned(),
                headers: headers
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                body,
            });
        }

        Box::pin(std::future::ready(Ok(PostResponse::recorded(
            200,
            tokio::io::empty(),
        ))))
    }
}

/// The conversation every cell sends, so two vendors differ only by protocol.
///
/// It holds each shape a body module needs a form for: a prompt, an answer
/// that called a tool, that tool's result, and a second prompt with the first
/// exchange behind it. A transcript of one message would freeze the easiest
/// quarter of every vendor's rendering.
fn spoken() -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::User {
            text: "rename the field and show me what moved".into(),
            attachments: Box::new([]),
        })
        .expect("a first prompt");
    transcript
        .push(Message::Agent {
            text: "Looking at where it is read.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("probe-call-1"),
                name: "grep".into(),
                args: ToolArgs::new(r#"{"pattern":"nearness","path":"crates"}"#),
            }],
            stop: Some(StopReason::WantsTools),
            continuation: None,
        })
        .expect("an answer that called a tool");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("probe-call-1"),
            output: RecordedToolOutput::ok("crates/crucible-config/src/document.rs:41"),
        }]))
        .expect("the tool's result");
    transcript
        .push(Message::User {
            text: "good, now the other caller".into(),
            attachments: Box::new([]),
        })
        .expect("a second prompt");
    transcript
}

/// The tools every cell offers, as the runner would hand them over.
fn offered() -> [ToolSchema<'static>; 2] {
    [
        ToolSchema {
            name: "grep",
            schema: r#"{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}"#,
        },
        ToolSchema {
            name: "write",
            schema: r#"{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"}},"required":["path","text"]}"#,
        },
    ]
}

/// Renders exactly what one provider put on the wire for one request.
///
/// `build` is handed the transport rather than the provider being handed back,
/// because the probe needs a second handle on the recorder and a provider owns
/// the one it sends through.
///
/// The body is frozen as the provider wrote it, never re-serialized through a
/// JSON value: key order, spacing and a repeated key are part of the answer,
/// and a round trip through a map would agree with any reordering of them.
fn wired(
    cell: &str,
    purpose: RequestPurpose,
    model: &str,
    build: impl FnOnce(Box<dyn Transport>) -> Box<dyn Provider>,
) -> String {
    let recorder = Recorder::default();
    let provider = build(Box::new(recorder.clone()));
    let transcript = spoken();
    let tools = offered();

    let _ = crucible_runtime::answered!(provider.stream(
        Request {
            purpose,
            model,
            transcript: &transcript,
            tools: &tools,
            max_tokens: 4096,
            system: Some("You are a probe. Answer nothing."),
            effort: None,
            attached: &[],
            prompt_cache: None,
        },
        &Cancel::new(),
    ));

    let posted = recorder.posted();
    assert!(
        posted.len() == 1,
        "{cell} made {} requests, and a cell that made none or several is not \
         the one request this freezes",
        posted.len()
    );
    let Some(sent) = posted.first() else {
        panic!("{cell} recorded nothing")
    };

    // The header values and the address are shown with the credential removed,
    // because a probe that printed a secret would put one in a file that is
    // committed. The body is shown whole, and the assertion below is what says
    // the secret is not in it — a redaction there could hide a body that had
    // started carrying the key.
    let redactions = {
        let mut outgoing = Outgoing::new();
        outgoing.protect(KEY);
        outgoing.redactions()
    };

    assert!(
        !sent.body.contains(KEY),
        "{cell} put the credential in the request body"
    );

    let mut rendered = String::new();
    let _ = writeln!(rendered, "=== {cell} ===");
    let _ = writeln!(rendered, "url     {}", redactions.redact(&sent.url));
    for (name, value) in &sent.headers {
        let _ = writeln!(rendered, "header  {name}: {}", redactions.redact(value));
    }
    let _ = writeln!(rendered, "body");
    rendered.push_str(&sent.body);
    rendered.push_str("\n\n");
    rendered
}

/// The stable prefix every caching cell sends.
///
/// Sized from the reviewed thresholds rather than by eye. The lowest minimum a
/// provider here declares is 257 tokens and the highest is 4096, and the
/// projection estimates one token to four bytes, so a shorter prefix would
/// leave the strictest of them selecting nothing — and a cell that selected
/// nothing would freeze the same request as one that never asked to cache,
/// while reading as though it had proved something.
fn prefix() -> String {
    "You are a probe. Answer nothing, and keep answering nothing. ".repeat(300)
}

/// The same conversation one exchange further on.
///
/// A turn projects its own plan from the request it is about to send, so the
/// second request is never the first one's plan sent again. Growing the
/// transcript moves the last stable message, and where the cache marker lands
/// once it has moved is the part a restructuring silently changes.
fn continued() -> Transcript {
    let mut transcript = spoken();
    transcript
        .push(Message::Agent {
            text: "The other caller reads it in the runner.".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
            continuation: None,
        })
        .expect("a second answer");
    transcript
        .push(Message::User {
            text: "rename that one too".into(),
            attachments: Box::new([]),
        })
        .expect("a third prompt");
    transcript
}

/// The attempt identity every caching cell carries.
///
/// Read from a fixed spelling rather than minted, because a fresh one every run
/// would differ between two correct runs and there is nothing here to derive a
/// stable one from.
const ATTEMPT: &str = "01900000-0000-7000-8000-000000000001";

/// Renders what one provider puts on the wire across two turns of one cached
/// conversation, under the policy a caller chose.
///
/// The plan is built the way a turn builds one: projected from the request that
/// is about to be sent. Both the encoded request and the provider's own account
/// of what it encoded are rendered, because they can disagree — a vendor that
/// supports no marker answers with an unchanged body, and only its verdict says
/// whether that body was a refusal or a mechanism that needs no field.
fn caching(
    cell: &str,
    model: &str,
    policy: PromptCachePolicy,
    build: impl FnOnce(Box<dyn Transport>) -> Box<dyn Provider>,
) -> String {
    let recorder = Recorder::default();
    let provider = build(Box::new(recorder.clone()));
    let capabilities = provider.prompt_cache_capabilities(model);
    let tools = offered();
    let system = prefix();
    let attempt = ProviderAttemptId::parse(ATTEMPT).expect("a canonical attempt identity");
    let redactions = {
        let mut outgoing = Outgoing::new();
        outgoing.protect(KEY);
        outgoing.redactions()
    };

    let mut rendered = String::new();
    for (turn, (label, transcript, fingerprint)) in [
        ("first turn", spoken(), [0x11; 32]),
        ("second turn", continued(), [0x22; 32]),
    ]
    .into_iter()
    .enumerate()
    {
        let projection = PromptCacheProjection::inspect(&Request {
            purpose: RequestPurpose::Turn,
            model,
            transcript: &transcript,
            tools: &tools,
            max_tokens: 4096,
            system: Some(&system),
            effort: None,
            attached: &[],
            prompt_cache: None,
        })
        .expect("a projection of the request about to be sent");
        let plan = PromptCachePlan::new(&projection, PromptCacheFingerprint::new(fingerprint));
        let selection = PromptCacheSelection::prepare(policy, &capabilities, &plan, false)
            .expect("a selection under a policy with no conflict");
        assert!(
            selection.selected().is_some(),
            "{cell} {label} selected no mechanism ({:?}), and a cell that \
             selects none freezes the same request as one that never asked to \
             cache at all",
            selection.eligibility()
        );

        let cache = PromptCacheRequest {
            attempt,
            policy,
            capabilities: &capabilities,
            plan: &plan,
            identity: PromptCacheIdentity::new(
                PromptCacheScopeDigest::new([0x33; 32]),
                PromptCacheFingerprint::new(fingerprint),
                "differential-probe-request-v1",
            ),
            selection,
            routing_key: Some(PromptCacheKey::from_digest([0x44; 32], 64)),
            resource: None,
        };
        let request = Request {
            purpose: RequestPurpose::Turn,
            model,
            transcript: &transcript,
            tools: &tools,
            max_tokens: 4096,
            system: Some(&system),
            effort: None,
            attached: &[],
            prompt_cache: Some(&cache),
        };
        let encoded = provider.prompt_cache_encoding(&request);
        let _ = crucible_runtime::answered!(provider.stream(request, &Cancel::new()));

        let posted = recorder.posted();
        assert!(
            posted.len() == turn + 1,
            "{cell} had made {} requests by {label}, and a turn that sent none \
             or several is not the one request this freezes",
            posted.len()
        );
        let Some(sent) = posted.last() else {
            panic!("{cell} recorded nothing for its {label}")
        };
        assert!(
            !sent.body.contains(KEY),
            "{cell} put the credential in the {label} request body"
        );

        let _ = writeln!(rendered, "=== {cell}, {label} ===");
        let _ = writeln!(
            rendered,
            "prefix  {} bytes, an estimated {} tokens",
            plan.stable_bytes(),
            plan.estimated_tokens()
        );
        let _ = writeln!(
            rendered,
            "chose   {:?}",
            selection.selected().map(PromptCacheSelected::mechanism)
        );
        let _ = writeln!(rendered, "encoded {encoded:?}");
        let _ = writeln!(rendered, "url     {}", redactions.redact(&sent.url));
        for (name, value) in &sent.headers {
            let _ = writeln!(rendered, "header  {name}: {}", redactions.redact(value));
        }
        let _ = writeln!(rendered, "body");
        rendered.push_str(&sent.body.replace(&system, "<stable prefix>"));
        rendered.push_str("\n\n");
    }
    rendered
}

#[test]
fn every_provider_answers_what_it_puts_on_the_wire() {
    let mut rendered = String::new();

    // Anthropic, twice over the model and once over the projection. The named
    // model takes a branch of its own in the headers, so a cell that only sent
    // the ordinary one would freeze half of what this vendor writes.
    rendered.push_str(&wired(
        "anthropic turn claude-opus-5",
        RequestPurpose::Turn,
        "claude-opus-5",
        |transport| Box::new(Anthropic::at(Anthropic::VENDOR, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "anthropic turn claude-fable-5-1",
        RequestPurpose::Turn,
        "claude-fable-5-1",
        |transport| Box::new(Anthropic::at(Anthropic::VENDOR, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "anthropic recap claude-opus-5",
        RequestPurpose::Recap,
        "claude-opus-5",
        |transport| Box::new(Anthropic::at(Anthropic::VENDOR, Box::new(Keyed), transport)),
    ));

    rendered.push_str(&wired(
        "google turn gemini-3.8-flash",
        RequestPurpose::Turn,
        "gemini-3.8-flash",
        |transport| Box::new(Google::at(Google::VENDOR, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "google recap gemini-3.8-flash",
        RequestPurpose::Recap,
        "gemini-3.8-flash",
        |transport| Box::new(Google::at(Google::VENDOR, Box::new(Keyed), transport)),
    ));

    // Moonshot over both of its own addresses. Which one it is sent to decides
    // what the provider considers a custom endpoint, and that answer is in the
    // request rather than only in the configuration that chose it.
    rendered.push_str(&wired(
        "moonshot turn kimi-for-coding at the coding address",
        RequestPurpose::Turn,
        "kimi-for-coding",
        |transport| Box::new(Moonshot::at(Moonshot::CODING, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "moonshot turn kimi-for-coding at the platform address",
        RequestPurpose::Turn,
        "kimi-for-coding",
        |transport| Box::new(Moonshot::at(Moonshot::PLATFORM, Box::new(Keyed), transport)),
    ));

    // OpenAI over its two addresses and over the model that is treated apart
    // from the rest, for the same reason the Anthropic cells go twice.
    rendered.push_str(&wired(
        "openai turn gpt-5.6-sol",
        RequestPurpose::Turn,
        "gpt-5.6-sol",
        |transport| Box::new(OpenAi::at(OpenAi::VENDOR, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "openai turn gpt-6-astra",
        RequestPurpose::Turn,
        "gpt-6-astra",
        |transport| Box::new(OpenAi::at(OpenAi::VENDOR, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "openai turn gpt-5.6-sol at the subscription address",
        RequestPurpose::Turn,
        "gpt-5.6-sol",
        |transport| Box::new(OpenAi::at(OpenAi::SUBSCRIPTION, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&wired(
        "openai recap gpt-5.6-sol",
        RequestPurpose::Recap,
        "gpt-5.6-sol",
        |transport| Box::new(OpenAi::at(OpenAi::VENDOR, Box::new(Keyed), transport)),
    ));

    // The same providers again with a cache plan attached, over two turns of
    // one conversation. Every cell above sends none, so without these nothing
    // freezes what a cache marker looks like, where it sits, or which of them
    // a vendor refuses.
    rendered.push_str(&caching(
        "anthropic cached turn claude-opus-5",
        "claude-opus-5",
        PromptCachePolicy::default(),
        |transport| Box::new(Anthropic::at(Anthropic::VENDOR, Box::new(Keyed), transport)),
    ));

    // The same vendor forced onto its other mechanism, and asked to hold the
    // prefix for longer than a default. The boundary walk that places an
    // explicit marker and the retention that decides whether a time to live is
    // written are the two parts of this vendor's encoding that a default policy
    // never reaches.
    rendered.push_str(&caching(
        "anthropic cached turn claude-fable-5-1 at an explicit breakpoint, held longer",
        "claude-fable-5-1",
        PromptCachePolicy::default()
            .allowing(PromptCacheMechanisms::one(
                PromptCacheMechanism::ExplicitBreakpoints,
            ))
            .with_retention(
                PromptCacheRetention::extended(3_600).expect("an hour is a legal retention"),
            ),
        |transport| Box::new(Anthropic::at(Anthropic::VENDOR, Box::new(Keyed), transport)),
    ));

    rendered.push_str(&caching(
        "google cached turn gemini-3.8-flash",
        "gemini-3.8-flash",
        PromptCachePolicy::default(),
        |transport| Box::new(Google::at(Google::VENDOR, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&caching(
        "moonshot cached turn kimi-for-coding at the coding address",
        "kimi-for-coding",
        PromptCachePolicy::default(),
        |transport| Box::new(Moonshot::at(Moonshot::CODING, Box::new(Keyed), transport)),
    ));
    rendered.push_str(&caching(
        "openai cached turn gpt-5.6-sol",
        "gpt-5.6-sol",
        PromptCachePolicy::default(),
        |transport| Box::new(OpenAi::at(OpenAi::VENDOR, Box::new(Keyed), transport)),
    ));

    // The version reaches the wire in a header, so it is substituted the way
    // every other rendering here substitutes it. There is no path to normalize:
    // nothing in a request body is a temporary directory.
    same(
        "provider-messages",
        &rendered.replace(env!("CARGO_PKG_VERSION"), "<version>"),
    );
}

// ------------------------------------------------------------------- session

/// A log kept in memory, so what a session wrote can be read back exactly.
#[derive(Clone, Default)]
struct Kept(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut kept) => {
                kept.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            Err(_) => Err(std::io::Error::other("a poisoned log")),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_session_writes_down_the_same_record_of_the_same_turn() {
    // A fixed name rather than a minted one: the stem is what the session reads
    // its own identity back from, and a new identity every run would differ
    // between two correct runs for no reason a reader could use.
    let kept = Kept::default();
    let path = PathBuf::from("/nowhere/01900000-0000-7000-8000-0000000000aa.jsonl");
    let session = Session::onto(path, kept.clone());

    // The conversation, one message of each kind the log has a shape for.
    for message in spoken().messages() {
        session.append(message);
    }
    session.append(&Message::Context(Fragment::new(
        "workspace",
        "one checked-out tree",
    )));

    // The framework record beside it. `append_item` is the pair — the message
    // line the conversation reads and the versioned ancestry line beside it —
    // and freezing only one of the two would leave a restructuring free to drop
    // the other.
    let run = RunId::parse("01900000-0000-7000-8000-0000000000b1").expect("a run identity");
    let ancestry = Ancestry::restore(run, None, run, 0).expect("a top-level ancestry");
    session.append_item(
        &RunItem::message(
            ancestry,
            Message::said("the framework's own copy of a prompt"),
        )
        .expect("a message inside its ceilings"),
    );
    session.append_journal(
        &RunItem::message(
            ancestry,
            Message::said("journal only, no conversation line"),
        )
        .expect("a message inside its ceilings"),
    );

    // The bounded summaries. Each stands for messages that stay in the file and
    // leave the transcript, so what one says is the whole difference between a
    // session continued correctly and one continued with the wrong history.
    session.compacted(3, "they renamed a field and found its readers");
    session.pruned(
        2,
        &[ToolId::new("probe-call-1"), ToolId::new("probe-call-2")],
    );
    session.measured(&Calibration {
        carried: Carried::new(4_725),
        spent: Spend::new(128),
        sent: 18_898,
        overhead: 1_024,
    });

    let snapshot = ContextSnapshot::from_value(serde_json::json!({
        "workspace": { "root": "/work", "trees": 1 }
    }))
    .expect("a snapshot of one section");
    let established = snapshot
        .patch_from(&ContextSnapshot::new())
        .expect("a first snapshot to be a patch against an empty one");
    session.contextual(&established).expect("a legal patch");

    let trouble = session.finish();
    assert!(
        trouble.is_none(),
        "the probe's own log failed while it was being written: {trouble:?}"
    );

    let written = kept.0.lock().expect("a lock").clone();
    let written = String::from_utf8(written).expect("a log of text");
    same("session-record", &written);
}

#[test]
fn a_session_written_through_its_store_writes_down_the_same_record() {
    // The runner writes through the store contract, whose writes wait for the
    // log to take each line; the calls above do not wait. Both have to leave
    // the same bytes, or a session written by a turn would stop being the one
    // every build before it reads back.
    let kept = Kept::default();
    let path = PathBuf::from("/nowhere/01900000-0000-7000-8000-0000000000aa.jsonl");
    let session = Session::onto(path, kept.clone());
    // Named as the contract, which is how the runner holds it: the session's
    // own methods of the same names are the calls that do not wait.
    let store: &dyn crucible_core::JournalStore = &session;
    let run = RunId::parse("01900000-0000-7000-8000-0000000000b1").expect("a run identity");
    let ancestry = Ancestry::restore(run, None, run, 0).expect("a top-level ancestry");
    let item = |said: &str| {
        RunItem::message(ancestry, Message::said(said)).expect("a message inside its ceilings")
    };
    let snapshot = ContextSnapshot::from_value(serde_json::json!({
        "workspace": { "root": "/work", "trees": 1 }
    }))
    .expect("a snapshot of one section");
    let established = snapshot
        .patch_from(&ContextSnapshot::new())
        .expect("a first snapshot to be a patch against an empty one");

    let written = async {
        for message in spoken().messages() {
            store.append_message(message).await;
        }
        let context = Message::Context(Fragment::new("workspace", "one checked-out tree"));
        store.append_message(&context).await;
        let framework = item("the framework's own copy of a prompt");
        let said = framework.model_message().expect("a conversation item");
        store.append_message(said).await;
        store.append_run_item(&framework);
        store.append_run_item(&item("journal only, no conversation line"));
        store
            .compacted(3, "they renamed a field and found its readers")
            .await;
        let pruned = [ToolId::new("probe-call-1"), ToolId::new("probe-call-2")];
        store.pruned(2, &pruned).await;
        let reading = Calibration {
            carried: Carried::new(4_725),
            spent: Spend::new(128),
            sent: 18_898,
            overhead: 1_024,
        };
        store.measured(&reading).await;
        store.contextual(&established).await
    };
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime to wait on the log with")
        .block_on(written)
        .expect("a legal patch");

    let trouble = session.finish();
    assert!(
        trouble.is_none(),
        "the probe's own log failed while it was being written: {trouble:?}"
    );

    let written = kept.0.lock().expect("a lock").clone();
    let written = String::from_utf8(written).expect("a log of text");
    same("session-record", &written);
}

// ---------------------------------------------------------------------- turn

/// Whether every step of the probed turn answers when first asked, as every
/// service here did while a turn could not wait, or waits once first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pace {
    AtOnce,
    Waiting,
}

/// How many times each kind of step of the probed turn said it was not ready.
#[derive(Debug, Default)]
struct Waited {
    provider: AtomicUsize,
    tool: AtomicUsize,
    toolset: AtomicUsize,
}

/// `future`, saying once that it is not ready first where the pace is
/// `Waiting`, and counting it on `waited`.
struct Paced<'a, F> {
    waits: bool,
    waited: &'a AtomicUsize,
    future: std::pin::Pin<Box<F>>,
}

impl<'a, F> Paced<'a, F> {
    fn new(pace: Pace, waited: &'a AtomicUsize, future: F) -> Self {
        Self {
            waits: pace == Pace::Waiting,
            waited,
            future: Box::pin(future),
        }
    }
}

impl<F: std::future::Future> std::future::Future for Paced<'_, F> {
    type Output = F::Output;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        if std::mem::replace(&mut self.waits, false) {
            self.waited.fetch_add(1, Ordering::Relaxed);
            cx.waker().wake_by_ref();
            return std::task::Poll::Pending;
        }
        self.future.as_mut().poll(cx)
    }
}

/// A model that asks for the probe tool once and then answers, at `pace`.
struct Probed {
    pace: Pace,
    waited: Arc<Waited>,
    rounds: std::sync::Mutex<std::collections::VecDeque<Vec<crucible_core::Delta>>>,
    scope: CredentialScopeId,
}

impl Provider for Probed {
    fn name(&self) -> &'static str {
        "probed"
    }

    fn spells(&self) -> crucible_core::Modalities {
        crucible_core::Modalities::empty().insert(crucible_core::Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> crucible_core::PromptCacheCapabilities {
        crucible_core::PromptCacheCapabilities::unknown("differential-turn-probe-v1")
    }

    fn prompt_cache_route(&self) -> crucible_core::PromptCacheRoute<'_> {
        crucible_core::PromptCacheRoute {
            protocol: "probed",
            endpoint: "probed",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "differential-turn-probe-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        crucible_core::PromptCacheEncoding::NoControlIntended
    }

    fn stream<'a>(
        &'a self,
        _request: Request<'a>,
        _cancel: &'a Cancel,
    ) -> crucible_runtime::BoxFuture<
        'a,
        Result<Box<dyn crucible_core::DeltaStream>, crucible_core::ProviderError>,
    > {
        let round = self
            .rounds
            .lock()
            .expect("a script")
            .pop_front()
            .unwrap_or_default();
        let stream = Recited {
            pace: self.pace,
            waited: Arc::clone(&self.waited),
            deltas: round.into(),
        };
        Box::pin(Paced::new(self.pace, &self.waited.provider, async move {
            Ok(Box::new(stream) as Box<dyn crucible_core::DeltaStream>)
        }))
    }
}

/// One round of deltas, each read at the probe's pace.
struct Recited {
    pace: Pace,
    waited: Arc<Waited>,
    deltas: std::collections::VecDeque<crucible_core::Delta>,
}

impl crucible_core::DeltaStream for Recited {
    fn next(
        &mut self,
    ) -> crucible_runtime::BoxFuture<
        '_,
        Option<Result<crucible_core::Delta, crucible_core::ProviderError>>,
    > {
        let next = self.deltas.pop_front().map(Ok);
        Box::pin(Paced::new(
            self.pace,
            &self.waited.provider,
            async move { next },
        ))
    }
}

/// The one tool the probed turn calls, running at the probe's pace.
struct Probe {
    pace: Pace,
    waited: Arc<Waited>,
}

impl DescribeTool for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl crucible_core::Tool for Probe {
    fn validate(&self, _args: &ToolArgs) -> Result<(), crucible_core::ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> crucible_core::Sensitivity {
        crucible_core::Sensitivity::ReadOnly {
            target: crucible_core::Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> crucible_core::Summary {
        crucible_core::Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        _approved: crucible_core::Approved,
        _context: &'a crucible_core::ToolContext<'_>,
    ) -> crucible_runtime::BoxFuture<'a, Result<crucible_core::ToolOutput, crucible_core::ToolError>>
    {
        Box::pin(Paced::new(self.pace, &self.waited.tool, async {
            Ok(crucible_core::ToolOutput::ok("the probe found one reader"))
        }))
    }
}

/// A live toolset offering the probe, preparing and disposing of itself at
/// the probe's pace.
struct Offering {
    pace: Pace,
    waited: Arc<Waited>,
    snapshot: crucible_core::ToolSnapshot,
}

impl crucible_core::Toolset for Offering {
    fn prepare<'a>(
        &'a self,
        _context: &'a crucible_core::ToolsetContext,
    ) -> crucible_runtime::BoxFuture<'a, Result<(), crucible_core::ToolsetError>> {
        Box::pin(Paced::new(self.pace, &self.waited.toolset, async {
            Ok(())
        }))
    }

    fn snapshot<'a>(
        &'a self,
        _context: &'a crucible_core::ToolsetContext,
    ) -> crucible_runtime::BoxFuture<
        'a,
        Result<crucible_core::ToolSnapshot, crucible_core::ToolsetError>,
    > {
        Box::pin(async { Ok(self.snapshot.clone()) })
    }

    fn refresh<'a>(
        &'a self,
        _context: &'a crucible_core::ToolsetContext,
    ) -> crucible_runtime::BoxFuture<
        'a,
        Result<crucible_core::ToolSnapshot, crucible_core::ToolsetError>,
    > {
        Box::pin(async { Ok(self.snapshot.clone()) })
    }

    fn dispose<'a>(
        &'a self,
        _context: &'a crucible_core::ToolsetContext,
    ) -> crucible_runtime::BoxFuture<'a, Result<(), crucible_core::ToolsetError>> {
        Box::pin(Paced::new(self.pace, &self.waited.toolset, async {
            Ok(())
        }))
    }
}

/// Allows every call it is asked about, once.
struct Allowing;

impl crucible_core::Ask for Allowing {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a crucible_core::Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (crucible_core::Verdict, crucible_core::Remember)> {
        Box::pin(async {
            (
                crucible_core::Verdict::Allow,
                crucible_core::Remember::Never,
            )
        })
    }
}

/// `text` with what is minted afresh on each run renumbered: each UUID in its
/// hyphenated spelling becomes `<id-N>`, and each 64-digit hexadecimal digest
/// — a prompt-cache fingerprint, which binds the run's own identity — becomes
/// `<digest-N>`, numbered in the order each first appears, so the same value
/// reads the same wherever it recurs and two runs compare on everything else.
fn renumbered(text: &str) -> String {
    const UUID: [usize; 5] = [8, 4, 4, 4, 12];
    const DIGEST: usize = 64;
    let bytes = text.as_bytes();
    let uuid_length: usize = UUID.iter().sum::<usize>() + UUID.len() - 1;
    let hex = |at: usize, width: usize| {
        bytes
            .get(at..at + width)
            .is_some_and(|run| run.iter().all(u8::is_ascii_hexdigit))
    };
    let bounded = |at: usize, width: usize| {
        !bytes.get(at + width).is_some_and(u8::is_ascii_hexdigit)
            && (at == 0 || !bytes.get(at - 1).is_some_and(u8::is_ascii_hexdigit))
    };
    let is_uuid = |at: usize| {
        let mut offset = at;
        UUID.iter().enumerate().all(|(group, &width)| {
            let digits = hex(offset, width);
            offset += width;
            let joined = group + 1 == UUID.len() || bytes.get(offset) == Some(&b'-');
            offset += 1;
            digits && joined
        }) && bounded(at, uuid_length)
    };
    let is_digest = |at: usize| hex(at, DIGEST) && bounded(at, DIGEST);
    let mut ids: Vec<&str> = Vec::new();
    let mut digests: Vec<&str> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let (kind, width, seen) = if is_uuid(at) {
            ("id", uuid_length, &mut ids)
        } else if is_digest(at) {
            ("digest", DIGEST, &mut digests)
        } else {
            let next = text[at..].chars().next().expect("a character");
            out.push(next);
            at += next.len_utf8();
            continue;
        };
        let value = &text[at..at + width];
        let number = seen
            .iter()
            .position(|one| *one == value)
            .unwrap_or_else(|| {
                seen.push(value);
                seen.len() - 1
            });
        let _ = write!(out, "<{kind}-{number}>");
        at += width;
    }
    out
}

/// Takes one turn through the application's own crossing, on the
/// application's own runtime, over a model, a tool and a toolset that answer
/// at `pace`, and renders what came of it: how the turn ended, every event it
/// posted in order, and every line the session wrote in order, the framework
/// journal's invocation records among them.
fn turn_record(pace: Pace) -> (String, Arc<Waited>) {
    let waited = Arc::new(Waited::default());
    let provenance = ToolProvenance::new(
        crucible_core::ToolSourceKind::Other,
        "differential:probe",
        "the differential turn's tool",
    )
    .expect("a provenance");
    let probe = Probe {
        pace,
        waited: Arc::clone(&waited),
    };
    let descriptor = crucible_core::ToolDescriptor::new("probe", probe.schema(), provenance)
        .expect("a descriptor");
    let snapshot = crucible_core::ToolSnapshot::new([crucible_core::ToolEntry::new(
        descriptor,
        Arc::new(probe),
    )])
    .expect("a snapshot");
    let offering = Offering {
        pace,
        waited: Arc::clone(&waited),
        snapshot,
    };
    let provider = Probed {
        pace,
        waited: Arc::clone(&waited),
        rounds: std::sync::Mutex::new(
            [
                vec![
                    crucible_core::Delta::Text("Looking for its readers.".into()),
                    crucible_core::Delta::ToolStarted {
                        id: ToolId::new("probe-call-1"),
                        name: "probe".into(),
                    },
                    crucible_core::Delta::ToolArgs("{}".into()),
                    crucible_core::Delta::Stopped(StopReason::WantsTools),
                ],
                vec![
                    crucible_core::Delta::Text("One reader, in the runner.".into()),
                    crucible_core::Delta::Stopped(StopReason::Yielded),
                ],
            ]
            .into(),
        ),
        scope: CredentialScopeId::new(),
    };

    let kept = Kept::default();
    let session = Arc::new(Session::onto(
        PathBuf::from("/nowhere/01900000-0000-7000-8000-0000000000cc.jsonl"),
        kept.clone(),
    ));
    let (rendered, stopped) = crucible_app::services::serving(|services| {
        let runtime = services
            .runtime()
            .handle()
            .expect("the application's runtime");
        let agent = crucible_runner::Agent::new(
            crucible_core::AgentId::new("probe"),
            crucible_runner::Model {
                name: "probed".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                effort: None,
            },
        );
        let mut conversation =
            crucible_app::Conversation::recording(session, Some("probed"), |session| {
                crucible_runner::Runner::with_toolset(
                    Box::new(provider),
                    offering,
                    agent,
                    crucible_context::ContextInputs::new(PathBuf::from("/nowhere/work"))
                        .dated(std::time::UNIX_EPOCH + std::time::Duration::from_hours(496_704)),
                    session,
                )
            })
            .on(runtime);
        let (events, seen) = std::sync::mpsc::channel::<crucible_runner::EventEnvelope>();
        let (cancel, steer, aside) = (
            Cancel::new(),
            crucible_core::Steer::new(),
            crucible_core::Aside::new(),
        );
        let turned = {
            let run = conversation
                .runner()
                .starting(&events, &cancel, &steer, &aside);
            conversation.turn("probe the tree", Box::default(), &mut Allowing, &run)
        };
        drop(events);
        let trouble = conversation.session().finish();
        assert!(trouble.is_none(), "the probe's own log failed: {trouble:?}");

        let mut rendered = String::new();
        let _ = writeln!(rendered, "=== turn ===\n{turned:?}\n=== events ===");
        for envelope in seen.try_iter() {
            let _ = writeln!(rendered, "{:?}", envelope.into_event());
        }
        let _ = writeln!(rendered, "=== session ===");
        rendered.push_str(
            &String::from_utf8(kept.0.lock().expect("a lock").clone()).expect("a log of text"),
        );
        rendered
    });
    assert!(stopped.is_ok(), "the runtime did not stop: {stopped:?}");
    (renumbered(&rendered), waited)
}

#[test]
fn a_turn_whose_steps_wait_records_what_one_answered_at_once_records() {
    // What changed is that a turn can wait for its model, a call that runs
    // alone and its toolset's preparation and disposal. The same turn, with
    // every one of those waiting once, has to come out exactly as it did when
    // nothing could: the same ending, the same events in the same order, and
    // the same session lines, each admitted call's invocation records once
    // each and in order among them.
    let (at_once, idle) = turn_record(Pace::AtOnce);
    let (waiting, waited) = turn_record(Pace::Waiting);

    assert_eq!(
        (
            idle.provider.load(Ordering::Relaxed),
            idle.tool.load(Ordering::Relaxed),
            idle.toolset.load(Ordering::Relaxed),
        ),
        (0, 0, 0),
        "a step of the at-once turn waited"
    );
    assert!(
        waited.provider.load(Ordering::Relaxed) > 0
            && waited.tool.load(Ordering::Relaxed) == 1
            && waited.toolset.load(Ordering::Relaxed) == 2,
        "the waiting turn's model, tool run, preparation and disposal did not each wait: {waited:?}"
    );
    assert!(
        at_once.contains("Ran(") && at_once.contains("the probe found one reader"),
        "the probed turn did not run its call:\n{at_once}"
    );
    let invocations: Vec<&str> = at_once
        .lines()
        .filter(|line| line.contains("\"invocation\"") && line.contains("probe-call-1"))
        .collect();
    assert_eq!(
        invocations.len(),
        3,
        "the admitted call was not recorded prepared, started and finished once each:\n{at_once}"
    );
    if waiting != at_once {
        let spilled = Path::new(env!("CARGO_TARGET_TMPDIR")).join("turn-record.txt");
        fs::write(&spilled, &waiting).expect("the waiting record to be written");
        panic!(
            "a turn whose steps waited recorded something else\n{}\n  waiting: {}",
            parted(&at_once, &waiting),
            spilled.display()
        );
    }
}
