//! The registry-wide contract for the tools a run composes.
//!
//! The roster is read from the composition owner rather than copied into a
//! second list. Each registered name is then given one synthetic call whose
//! schema, sensitivity and result are checked against the contract this base
//! already has. A name without a case stops the fixture, so adding a tool to
//! the registry cannot quietly add an unexercised capability.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_auth::{Renewals, StoredCredentials};
use crucible_builtins::{Background, Ledger, Plan};
use crucible_config::Settings;
use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::{
    Approved, Ask, DescribeTool, Fetch, Host, Page, Permission, Put, Remember, Search,
    SearchResponse, SearchResult, Sensitivity, Settled, SourceError, Target, Tool, ToolContext,
    ToolEntry, ToolError, ToolOutput, Unwatched, Verdict,
};
use crucible_types::{Ancestry, Answered, Question, ToolArgs, ToolCall, ToolId};
use crucible_workspace::Workspace;
use tokio::runtime::Runtime;

use crate::services::Services;
use crate::startup::{Reaching, Resuming, Startup, tools};
use crate::subscription::Subscriptions;

const FROZEN_SCHEMAS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/differential/tool-schemas.txt"
));

/// A directory owned by one fixture, removed when the fixture ends.
struct Tree(PathBuf);

impl Tree {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "crucible-tool-conformance-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).map_err(|problem| problem.to_string())?;
        let path = fs::canonicalize(&path).map_err(|problem| problem.to_string())?;
        Ok(Self(path))
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The resources a registry needs while its tools run.
struct Fixture {
    _tree: Tree,
    runtime: tokio::runtime::Runtime,
}

/// The composition owner's complete registry, with both web capabilities and
/// somebody available to answer an ask.
fn registry() -> Result<(crucible_runner::Tools, Fixture), String> {
    let tree = Tree::new()?;
    let workspace = Workspace::open(tree.root()).map_err(|problem| problem.to_string())?;
    fs::write(
        workspace.root().join("fixture.txt"),
        "before\nconformance\n",
    )
    .map_err(|problem| problem.to_string())?;

    let providers = crate::providers::providers()
        .map_err(|problem| problem.to_string())?
        .snapshot();
    let settings = Settings::default();
    let services = Services::new();
    let stored = StoredCredentials::default();
    let renewals = Renewals::new();
    let subscriptions = Subscriptions::production(&renewals);
    let asking: Arc<dyn Put> = Arc::new(Answers);
    let source = Arc::new(Source);
    let reaching = Reaching {
        searching: Some(source.clone()),
        fetching: Some(source),
    };
    let leaving = Background::new();
    let revealed = crucible_tools::Revealed::new();
    let plan = Plan::new();
    let ledger = Ledger::new();
    let sessions = tree.root().join("sessions");
    let startup = Startup {
        services: &services,
        providers: &providers,
        provider: None,
        unasked: "nothing to ask",
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: crucible_tools::Mode::Ask,
        settings: &settings,
        sessions: &sessions,
        workspace: &workspace,
        ledger: &ledger,
        revealed: &revealed,
        plan: &plan,
        leaving: &leaving,
        asking,
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &stored,
        subscriptions: &subscriptions,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|problem| problem.to_string())?;

    let tools = tools(
        &startup,
        &settings,
        reaching,
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone())),
    )
    .map_err(|problem| problem.to_string())?;
    Ok((
        tools,
        Fixture {
            _tree: tree,
            runtime,
        },
    ))
}

/// Every registered entry, whether it is advertised now or held for lookup.
fn registered(tools: &crucible_runner::Tools) -> Result<Vec<ToolEntry>, String> {
    let mut entries = BTreeMap::new();

    for schema in tools.advertised() {
        let entry = tools
            .registered(schema.name)
            .ok_or_else(|| format!("the registry lost {} while it was being read", schema.name))?;
        if entry.descriptor().advertised() != schema {
            return Err(format!(
                "the registry advertised {} with a different schema",
                schema.name
            ));
        }
        if let Some(previous) = entries.insert(schema.name.to_owned(), entry.clone())
            && previous.descriptor() != entry.descriptor()
        {
            return Err(format!(
                "the registry listed {} with different descriptors",
                schema.name
            ));
        }
    }

    for descriptor in tools.deferred() {
        let entry = tools.registered(descriptor.name()).ok_or_else(|| {
            format!(
                "the registry lost {} while it was being read",
                descriptor.name()
            )
        })?;
        if entry.descriptor() != descriptor {
            return Err(format!(
                "the registry deferred {} with a different descriptor",
                descriptor.name()
            ));
        }
        if let Some(previous) = entries.insert(descriptor.name().to_owned(), entry.clone())
            && previous.descriptor() != entry.descriptor()
        {
            return Err(format!(
                "the registry listed {} with different descriptors",
                descriptor.name()
            ));
        }
    }

    Ok(entries.into_values().collect())
}

/// The frozen schema rendering, in the same name order as the fixture.
fn rendered(entries: &[ToolEntry]) -> String {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| left.descriptor().name().cmp(right.descriptor().name()));

    let mut rendered = String::new();
    for entry in entries {
        let descriptor = entry.descriptor();
        let _ = writeln!(rendered, "## {}", descriptor.name());
        let _ = writeln!(rendered, "effect      {:?}", descriptor.effect());
        let _ = writeln!(rendered, "execution   {:?}", descriptor.execution());
        let _ = writeln!(rendered, "timeout     {:?}", descriptor.timeout());
        let _ = writeln!(rendered, "result      {:?}", descriptor.result_bytes());
        let _ = writeln!(rendered, "provenance  {:?}", descriptor.provenance());
        let _ = writeln!(rendered, "schema");
        rendered.push_str(descriptor.schema());
        rendered.push_str("\n\n");
    }
    rendered
}

fn first_difference(expected: &str, observed: &str) -> String {
    for (number, (was, now)) in expected.lines().zip(observed.lines()).enumerate() {
        if was != now {
            return format!(
                "line {}: expected {was:?}, observed {now:?}",
                number.saturating_add(1)
            );
        }
    }
    format!(
        "the shared lines agree; expected {} lines and observed {}",
        expected.lines().count(),
        observed.lines().count()
    )
}

fn check(tools: &crucible_runner::Tools, runtime: &Runtime) -> Result<(), String> {
    let entries = registered(tools)?;
    let observed = rendered(&entries);
    if observed != FROZEN_SCHEMAS {
        return Err(format!(
            "the registry's schemas no longer equal the frozen fixture: {}",
            first_difference(FROZEN_SCHEMAS, &observed)
        ));
    }
    check_cases(&entries, runtime)
}

fn check_cases(entries: &[ToolEntry], runtime: &Runtime) -> Result<(), String> {
    for entry in entries {
        let case = case(entry.descriptor().name())?;
        exercise(entry, &case, runtime)?;
    }
    Ok(())
}

/// What a call is expected to say about risk and what it should return.
struct Case {
    args: &'static str,
    kind: &'static str,
    detail: &'static str,
    outcome: Outcome,
}

enum Outcome {
    Succeeded(&'static str),
    Failed(&'static str),
}

fn case(name: &str) -> Result<Case, String> {
    let case = match name {
        "ask_user" => Case {
            args: r#"{"questions":[{"heading":"Choice","question":"Pick one","answers":[{"answer":"yes"},{"answer":"no"}]}]}"#,
            kind: "read-only",
            detail: "read a path it could not resolve",
            outcome: Outcome::Succeeded("yes"),
        },
        "bash" => Case {
            args: r#"{"command":"printf conformance"}"#,
            kind: "spawn-process",
            detail: "run printf conformance",
            outcome: Outcome::Succeeded("conformance"),
        },
        "bash_output" => Case {
            args: r#"{"number":1}"#,
            kind: "read-only",
            detail: "read a path it could not resolve",
            outcome: Outcome::Failed("nothing is running as #1"),
        },
        "edit" => Case {
            args: r#"{"path":"fixture.txt","find":"before","replace":"after"}"#,
            kind: "mutates-file",
            detail: "change fixture.txt",
            outcome: Outcome::Succeeded("changed fixture.txt, 1 replacements"),
        },
        "glob" => Case {
            args: r#"{"pattern":"*.txt"}"#,
            kind: "read-only",
            detail: "read .",
            outcome: Outcome::Succeeded("fixture.txt\n"),
        },
        "grep" => Case {
            args: r#"{"pattern":"conformance"}"#,
            kind: "read-only",
            detail: "read .",
            outcome: Outcome::Succeeded("fixture.txt:2:conformance\n"),
        },
        "read" => Case {
            args: r#"{"path":"fixture.txt"}"#,
            kind: "read-only",
            detail: "read fixture.txt",
            outcome: Outcome::Succeeded("2\tconformance"),
        },
        "todo_write" => Case {
            args: r#"{"tasks":[{"task":"conformance","state":"open"}]}"#,
            kind: "read-only",
            detail: "read a path it could not resolve",
            outcome: Outcome::Succeeded("open: conformance"),
        },
        "tool_search" => Case {
            args: r#"{"query":"web"}"#,
            kind: "read-only",
            detail: "read a path it could not resolve",
            outcome: Outcome::Succeeded("web_search"),
        },
        "web_fetch" => Case {
            args: r#"{"url":"https://example.test/page"}"#,
            kind: "reaches-network",
            detail: "reach example.test",
            outcome: Outcome::Succeeded("fetched conformance"),
        },
        "web_search" => Case {
            args: r#"{"query":"conformance","limit":1}"#,
            kind: "reaches-network",
            detail: "reach search.example",
            outcome: Outcome::Succeeded("search extract"),
        },
        "write" => Case {
            args: r#"{"path":"created.txt","content":"written\n"}"#,
            kind: "mutates-file",
            detail: "change created.txt",
            outcome: Outcome::Succeeded("created created.txt, 1 lines"),
        },
        other => {
            return Err(format!("registered tool {other:?} has no conformance case"));
        }
    };
    Ok(case)
}

fn classification(sensitivity: &Sensitivity) -> (&'static str, String) {
    let kind = match sensitivity {
        Sensitivity::ReadOnly { .. } => "read-only",
        Sensitivity::ReadsOutside { .. } => "reads-outside",
        Sensitivity::MutatesFile { .. } => "mutates-file",
        Sensitivity::SpawnsProcess { .. } => "spawn-process",
        Sensitivity::ReachesNetwork { .. } => "reaches-network",
    };
    (kind, sensitivity.to_string())
}

fn exercise(entry: &ToolEntry, case: &Case, runtime: &Runtime) -> Result<(), String> {
    let descriptor = entry.descriptor();
    serde_json::from_str::<serde_json::Value>(descriptor.schema())
        .map_err(|problem| format!("{} schema is not JSON: {problem}", descriptor.name()))?;
    if descriptor.schema().is_empty() {
        return Err(format!("{} has an empty schema", descriptor.name()));
    }

    let call = ToolCall {
        id: ToolId::new(format!("conformance-{}", descriptor.name())),
        name: descriptor.name().into(),
        args: ToolArgs::new(case.args),
    };
    entry.tool().validate(&call.args).map_err(|problem| {
        format!(
            "{} rejected its conformance call: {problem}",
            descriptor.name()
        )
    })?;

    let sensitivity = entry.tool().sensitivity(&call.args);
    let (kind, detail) = classification(&sensitivity);
    if kind != case.kind || detail != case.detail {
        return Err(format!(
            "{} classified as {kind:?} {detail:?}, expected {:?} {:?}",
            descriptor.name(),
            case.kind,
            case.detail
        ));
    }

    let approved = approved(&call, &sensitivity, runtime)?;
    let cancel = Cancel::new();
    let context = ToolContext::new(Ancestry::new(), call.id.clone(), &cancel, None, &Unwatched);
    let output = runtime
        .block_on(entry.tool().run(approved, &context))
        .map_err(|problem| format!("{} did not run: {problem}", descriptor.name()))?;

    match &case.outcome {
        Outcome::Succeeded(marker) if output.is_failed() || !output.text().contains(marker) => {
            Err(format!(
                "{} did not succeed with {marker:?}: failed={} text={:?}",
                descriptor.name(),
                output.is_failed(),
                output.text()
            ))
        }
        Outcome::Failed(marker) if !output.is_failed() || !output.text().contains(marker) => {
            Err(format!(
                "{} did not fail with {marker:?}: failed={} text={:?}",
                descriptor.name(),
                output.is_failed(),
                output.text()
            ))
        }
        Outcome::Succeeded(_) | Outcome::Failed(_) => Ok(()),
    }
}

fn approved(
    call: &ToolCall,
    sensitivity: &Sensitivity,
    runtime: &Runtime,
) -> Result<Approved, String> {
    let mut ask = Allows;
    match runtime.block_on(Permission::new().decide(call, sensitivity, &mut ask)) {
        Settled::Approved(approved) => Ok(approved),
        Settled::Forbidden => Err(format!("{} was forbidden", call.name)),
        Settled::Refused => Err(format!("{} was refused", call.name)),
    }
}

/// A caller that approves only this synthetic conformance call.
struct Allows;

impl Ask for Allows {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Allow, Remember::Never) })
    }
}

/// A person who answers every synthetic question with the first option.
struct Answers;

impl Put for Answers {
    fn put<'a>(&'a self, questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>> {
        let answers = questions
            .iter()
            .map(|_| Answered::new(["yes"]))
            .collect::<Vec<_>>();
        Box::pin(async move { Some(answers) })
    }
}

/// A source that answers both web capabilities from memory.
struct Source;

impl Search for Source {
    fn name(&self) -> &'static str {
        "conformance"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn search<'a>(
        &'a self,
        _query: &'a str,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<SearchResponse, SourceError>> {
        Box::pin(async move {
            Ok(SearchResponse::results(vec![SearchResult {
                title: "Example result".into(),
                url: "https://search.example/result".into(),
                extract: "search extract".into(),
            }]))
        })
    }
}

impl Fetch for Source {
    fn name(&self) -> &'static str {
        "conformance"
    }

    fn reaches(&self, url: &str) -> Host {
        Host::Named {
            sent: url.into(),
            host: "example.test".into(),
        }
    }

    fn fetch<'a>(
        &'a self,
        url: &'a str,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Page, SourceError>> {
        Box::pin(async move {
            Ok(Page {
                url: url.into(),
                title: Some("Example page".into()),
                text: "fetched conformance".into(),
            })
        })
    }
}

/// A tool absent from the production roster, temporarily registered only to
/// prove the coverage guard.
struct Throwaway;

impl DescribeTool for Throwaway {
    fn name(&self) -> &'static str {
        "throwaway"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Throwaway {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> crucible_tools::Summary {
        crucible_tools::Summary::new("throwaway")
    }

    fn run<'a>(
        &'a self,
        _approved: crucible_tools::Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async { Ok(ToolOutput::ok("throwaway")) })
    }
}

#[test]
fn every_registered_tool_conforms_to_the_composition() -> Result<(), String> {
    let (tools, fixture) = registry()?;
    check(&tools, &fixture.runtime)
}

#[test]
fn a_revealed_deferred_tool_is_enumerated_once() -> Result<(), String> {
    let (tools, fixture) = registry()?;
    check(&tools, &fixture.runtime)?;
    registered(&tools).map(|_| ())
}

#[test]
fn a_registered_tool_without_a_case_is_refused() -> Result<(), String> {
    let (mut tools, fixture) = registry()?;
    tools
        .add_builtin(Throwaway)
        .map_err(|problem| problem.to_string())?;
    let entries = registered(&tools)?;
    let problem = check_cases(&entries, &fixture.runtime)
        .err()
        .ok_or("the throwaway was accepted")?;
    if !problem.contains("throwaway") {
        return Err(format!(
            "the coverage guard named the wrong problem: {problem}"
        ));
    }
    Ok(())
}
