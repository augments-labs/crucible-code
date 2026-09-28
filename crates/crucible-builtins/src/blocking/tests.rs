//! The search tools on a lent worker: they wait there for room, stop with
//! their call, give their place back, and answer as they do without one.

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use crucible_runtime::{Cancel, NOTICED};
use crucible_tools::{
    DescribeTool, Disposition, Tool, ToolContext, ToolError, ToolOutput, ToolWorker, Unrun,
    Unwatched,
};
use crucible_types::{Ancestry, ToolId};
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

use crate::sample::{Sample, allowed, context, under};
use crate::{Glob, Grep, Held, ToolSearch};
use crucible_tools::Revealed;

/// A tool these tests can both run and be granted a call to.
trait Searching: Tool + DescribeTool {}

impl<T: Tool + DescribeTool> Searching for T {}

/// How long a test waits for something the code under test should do at once,
/// before deciding it never will. Far longer than [`NOTICED`], so a pass is a
/// fact about the code rather than about how busy the machine is.
const PATIENCE: Duration = Duration::from_secs(5);

/// How often a job held on a gate looks at it.
const TICK: Duration = Duration::from_millis(1);

/// The longest a holding job runs, whether or not its gate is raised: dropping
/// a runtime waits for its blocking threads, so a job outliving a failed
/// assertion would turn the failure into a hang.
const LIFETIME: Duration = Duration::from_secs(10);

/// What a grep for `needle` answers when it stopped before it reached a file.
const GREP_STOPPED: &str = "nothing matched needle\n[stopped before the walk finished: a match \
                            in a file it did not reach is not here]";

/// What a glob for `**/*.rs` answers when it stopped before it reached a file.
const GLOB_STOPPED: &str = "no path matched **/*.rs\n[stopped before the walk finished: these \
                            are the lowest paths it reached, not the lowest there are]";

/// A runtime of the test's own, with the timer a worker's wait for room looks
/// at its cancellation on.
fn runtime() -> Runtime {
    Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime for the test")
}

/// A tree with something for both tools to find, at two depths.
fn tree(name: &str) -> Sample {
    let sample = Sample::new(name);
    sample.write("src/main.rs", "fn main() {\n    let needle = 1;\n}\n");
    sample.write("src/lib.rs", "// needle in a comment\npub fn other() {}\n");
    sample.write(
        "src/deep/inner.rs",
        "no match\nNEEDLE shouted\nneedle again\n",
    );
    sample.write("notes.md", "a needle in the notes\n");
    sample.write(".gitignore", "ignored.rs\n");
    sample.write("ignored.rs", "needle nobody should see\n");
    sample
}

/// A call stopped by `cancel` and lent `worker`.
fn lent<'a>(cancel: &Cancel, worker: &'a ToolWorker) -> ToolContext<'a> {
    ToolContext::new(
        Ancestry::new(),
        ToolId::new("sample"),
        cancel,
        None,
        &Unwatched,
    )
    .with_worker(worker)
}

/// Waits on a blocking thread until `gate` is raised, or [`LIFETIME`] passes.
fn held_until(gate: &Cancel) {
    let began = std::time::Instant::now();
    while !gate.requested() && began.elapsed() < LIFETIME {
        std::thread::sleep(TICK);
    }
}

/// Waits until `done` holds, or fails the test once [`PATIENCE`] has passed.
async fn until(what: &str, done: impl Fn() -> bool) {
    let began = tokio::time::Instant::now();
    while !done() {
        assert!(began.elapsed() < PATIENCE, "{what} never happened");
        tokio::time::sleep(TICK).await;
    }
}

/// Fills every place on `worker` with a job that runs until `gate` is raised,
/// and waits until every one of them has started — which is also the proof
/// that every place was free.
async fn occupy(worker: &ToolWorker, gate: &Cancel) -> Vec<JoinHandle<Result<(), Unrun>>> {
    let started = Arc::new(AtomicUsize::new(0));
    let holding = (0..ToolWorker::CAPACITY)
        .map(|_| {
            let worker = worker.clone();
            let gate = gate.clone();
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                worker
                    .run(&Cancel::new(), move |_| {
                        started.fetch_add(1, Ordering::AcqRel);
                        held_until(&gate);
                    })
                    .await
            })
        })
        .collect();
    until("every place on the worker being taken", || {
        started.load(Ordering::Acquire) == ToolWorker::CAPACITY
    })
    .await;
    holding
}

/// Raises `gate` and waits for every job it held.
async fn release(gate: &Cancel, holding: Vec<JoinHandle<Result<(), Unrun>>>) {
    gate.request();
    for held in holding {
        let ended = tokio::time::timeout(PATIENCE, held).await;
        assert!(
            matches!(ended, Ok(Ok(Ok(())))),
            "a holding job did not end: {ended:?}"
        );
    }
}

/// Every place on `worker` is free: as many jobs as it has places all start.
async fn every_place_free(worker: &ToolWorker) {
    let gate = Cancel::new();
    let holding = occupy(worker, &gate).await;
    release(&gate, holding).await;
}

/// Starts `tool` on a worker whose every place is taken, and says what the
/// call answered once it was cancelled while it waited for room.
fn cancelled_while_waiting(tool: &dyn Searching, args: &str, name: &str) -> ToolOutput {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    let context = lent(&cancel, &worker);

    runtime.block_on(async {
        let gate = Cancel::new();
        let holding = occupy(&worker, &gate).await;

        let mut running = tool.run(allowed(tool, args), &context);
        let first = poll_fn(|asked| Poll::Ready(running.as_mut().poll(asked))).await;
        assert!(
            first.is_pending(),
            "{name} answered on the thread polling it rather than waiting for room on the worker \
             it was lent"
        );
        // Long enough for the call to be well into its wait for room.
        tokio::time::sleep(NOTICED * 2).await;

        cancel.request();
        let answered = tokio::time::timeout(PATIENCE, running).await;
        let output = answered
            .unwrap_or_else(|_| panic!("the cancelled {name} was still waiting for room"))
            .unwrap_or_else(|problem| panic!("the cancelled {name} failed: {problem}"));

        release(&gate, holding).await;
        every_place_free(&worker).await;
        output
    })
}

#[test]
fn a_grep_cancelled_while_it_waits_for_room_on_the_worker_searches_nothing() {
    let sample = tree("blocking-grep-waiting");
    let grep = Grep::new(sample.workspace());

    let output = cancelled_while_waiting(&grep, r#"{"pattern":"needle"}"#, "grep");

    assert!(output.is_failed());
    assert_eq!(output.text(), GREP_STOPPED);
}

#[test]
fn a_glob_cancelled_while_it_waits_for_room_on_the_worker_lists_nothing() {
    let sample = tree("blocking-glob-waiting");
    let glob = Glob::new(sample.workspace());

    let output = cancelled_while_waiting(&glob, r#"{"pattern":"**/*.rs"}"#, "glob");

    assert!(output.is_failed());
    assert_eq!(output.text(), GLOB_STOPPED);
}

/// A tree far larger than a walk gets through in the moment between handing
/// it to the worker and the call being cancelled.
fn wide(name: &str) -> Sample {
    let sample = Sample::new(name);
    for directory in 0..40 {
        for file in 0..25 {
            sample.write(
                &format!("d{directory:02}/f{file:02}.rs"),
                "fn main() {}\nlet needle = 1;\n",
            );
        }
    }
    sample
}

#[test]
fn a_search_cancelled_while_it_walks_on_the_worker_stops_answers_and_gives_its_place_back() {
    let sample = wide("blocking-running");
    let grep = Grep::new(sample.workspace());
    let glob = Glob::new(sample.workspace());
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());

    for (tool, args) in [
        (&grep as &dyn Searching, r#"{"pattern":"needle"}"#),
        (&glob as &dyn Searching, r#"{"pattern":"**/*.rs"}"#),
    ] {
        let cancel = Cancel::new();
        let context = lent(&cancel, &worker);
        runtime.block_on(async {
            let mut running = tool.run(allowed(tool, args), &context);
            // The first poll finds room and hands the walk to the worker.
            let first = poll_fn(|asked| Poll::Ready(running.as_mut().poll(asked))).await;
            cancel.request();

            let answered = match first {
                Poll::Ready(answered) => Ok(answered),
                Poll::Pending => tokio::time::timeout(PATIENCE, running).await,
            };
            let output = answered
                .unwrap_or_else(|_| panic!("{args}: the cancelled search did not answer"))
                .unwrap_or_else(|problem| panic!("{args}: the cancelled search failed: {problem}"));
            // The walk heard its call's cancel through the token the worker
            // handed it, and said that it stopped rather than finishing.
            assert!(
                output.text().contains("[stopped before the walk finished"),
                "{args}: the walk went on after its call was cancelled: {}",
                output.text()
            );
            every_place_free(&worker).await;
        });
    }
}

#[test]
fn a_search_dropped_while_it_walks_on_the_worker_gives_its_place_back() {
    let sample = tree("blocking-dropped");
    let grep = Grep::new(sample.workspace());
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    let context = lent(&cancel, &worker);

    runtime.block_on(async {
        let mut running = grep.run(allowed(&grep, r#"{"pattern":"needle"}"#), &context);
        let _ = poll_fn(|asked| Poll::Ready(running.as_mut().poll(asked))).await;
        drop(running);

        every_place_free(&worker).await;
    });
}

#[test]
fn a_search_on_a_worker_that_has_stopped_is_an_error_rather_than_an_answer() {
    let stopped = runtime();
    let worker = ToolWorker::new(stopped.handle().clone());
    stopped.shutdown_background();
    let sample = tree("blocking-stopped");
    let grep = Grep::new(sample.workspace());
    let glob = Glob::new(sample.workspace());
    let cancel = Cancel::new();
    let context = lent(&cancel, &worker);

    for (tool, args, name) in [
        (&grep as &dyn Searching, r#"{"pattern":"needle"}"#, "grep"),
        (&glob as &dyn Searching, r#"{"pattern":"**/*.rs"}"#, "glob"),
    ] {
        let answered = runtime().block_on(tool.run(allowed(tool, args), &context));

        assert!(
            matches!(&answered, Err(ToolError::Io { tool, .. }) if &**tool == name),
            "{name} answered without the worker it was lent: {answered:?}"
        );
    }
}

/// The calls both tools are compared on: every shape of answer each gives.
const GREP_CALLS: &[&str] = &[
    r#"{"pattern":"needle"}"#,
    r#"{"pattern":"needle","ignore_case":true}"#,
    r#"{"pattern":"needle","mode":"files"}"#,
    r#"{"pattern":"needle","context":1}"#,
    r#"{"pattern":"needle","limit":2}"#,
    r#"{"pattern":"needle","glob":"**/*.rs"}"#,
    r#"{"pattern":"needle","path":"src/deep"}"#,
    r#"{"pattern":"n.edle","fixed":true}"#,
    r#"{"pattern":"absent from every file"}"#,
    r#"{"pattern":"("}"#,
    r#"{"pattern":"needle","path":"../outside"}"#,
];

const GLOB_CALLS: &[&str] = &[
    r#"{"pattern":"**/*.rs"}"#,
    r#"{"pattern":"src/*.rs"}"#,
    r#"{"pattern":"**/*","limit":2}"#,
    r#"{"pattern":"**/*.rs","sort":"modified"}"#,
    r#"{"pattern":"**/*.rs","path":"src/deep"}"#,
    r#"{"pattern":"**/*.toml"}"#,
    r#"{"pattern":"[","path":"src"}"#,
];

/// The rules the comparison is repeated under, so a file a rule names is
/// passed over on the worker as it is on the calling thread.
fn rules() -> [(Disposition, &'static str); 2] {
    [
        (Disposition::Deny, "grep(src/lib.rs)"),
        (Disposition::Deny, "glob(src/lib.rs)"),
    ]
}

#[test]
fn a_search_on_the_worker_answers_exactly_as_it_does_on_the_calling_thread() {
    let sample = tree("blocking-same");
    let grep = Grep::new(sample.workspace());
    let glob = Glob::new(sample.workspace());
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    let on_worker = lent(&cancel, &worker);

    let calls = GREP_CALLS
        .iter()
        .map(|args| (&grep as &dyn Searching, *args))
        .chain(
            GLOB_CALLS
                .iter()
                .map(|args| (&glob as &dyn Searching, *args)),
        );
    for (tool, args) in calls {
        for ruled in [false, true] {
            let approved = || {
                if ruled {
                    under(tool, args, &rules())
                } else {
                    allowed(tool, args)
                }
            };
            let here = crucible_runtime::answered!(tool.run(approved(), &context()))
                .map_err(|e| e.to_string());
            let there = runtime
                .block_on(tool.run(approved(), &on_worker))
                .map_err(|e| e.to_string());

            let seen = |answered: &Result<ToolOutput, String>| {
                answered
                    .as_ref()
                    .map(|output| (output.is_failed(), output.text().to_owned()))
                    .map_err(Clone::clone)
            };
            assert_eq!(
                seen(&there),
                seen(&here),
                "{args} (ruled: {ruled}) answered differently on the worker"
            );
        }
    }
}

/// A tool search over two held-back tools, revealing into the set it hands
/// back.
fn looking() -> (ToolSearch, Revealed) {
    let revealed = Revealed::new();
    let held = vec![
        Held {
            name: "web_search".into(),
            about: "Searches the web and returns titles and extracts.".into(),
        },
        Held {
            name: "todo_write".into(),
            about: "Writes down the plan for the work in hand.".into(),
        },
    ];
    (ToolSearch::new(held, revealed.clone()), revealed)
}

#[test]
fn a_tool_search_cancelled_while_it_waits_for_room_on_the_worker_reveals_nothing() {
    let (search, revealed) = looking();
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    let context = lent(&cancel, &worker);

    let answered = runtime.block_on(async {
        let gate = Cancel::new();
        let holding = occupy(&worker, &gate).await;

        let mut running = search.run(allowed(&search, r#"{"query":"web search"}"#), &context);
        let first = poll_fn(|asked| Poll::Ready(running.as_mut().poll(asked))).await;
        assert!(
            first.is_pending(),
            "tool_search answered on the thread polling it rather than waiting for room on the \
             worker it was lent"
        );
        tokio::time::sleep(NOTICED * 2).await;

        cancel.request();
        let answered = tokio::time::timeout(PATIENCE, running)
            .await
            .unwrap_or_else(|_| panic!("the cancelled tool_search was still waiting for room"));

        release(&gate, holding).await;
        every_place_free(&worker).await;
        answered
    });

    assert!(
        matches!(&answered, Err(ToolError::Cancelled(tool)) if &**tool == "tool_search"),
        "{answered:?}"
    );
    assert!(
        !revealed.holds("web_search"),
        "a search that never ran revealed a tool"
    );
}

#[test]
fn a_tool_search_on_the_worker_answers_and_reveals_as_it_does_on_the_calling_thread() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    let on_worker = lent(&cancel, &worker);

    for query in [
        "web search",
        "todo_write",
        "plan",
        "nothing like it",
        "",
        "web",
    ] {
        let args = format!(
            r#"{{"query":{}}}"#,
            serde_json::to_string(query).expect("a query that encodes")
        );
        let (here_tool, here_revealed) = looking();
        let (there_tool, there_revealed) = looking();

        let here =
            crucible_runtime::answered!(here_tool.run(allowed(&here_tool, &args), &context()))
                .map(|output| (output.is_failed(), output.text().to_owned()))
                .map_err(|problem| problem.to_string());
        let there = runtime
            .block_on(there_tool.run(allowed(&there_tool, &args), &on_worker))
            .map(|output| (output.is_failed(), output.text().to_owned()))
            .map_err(|problem| problem.to_string());

        assert_eq!(there, here, "{query} answered differently on the worker");
        for name in ["web_search", "todo_write"] {
            assert_eq!(
                there_revealed.holds(name),
                here_revealed.holds(name),
                "{query} revealed {name} differently on the worker"
            );
        }
    }
}

/// A job that comes apart on the worker comes apart in the call too, as it
/// does on the calling thread: the runner then contains it and records the
/// call as having panicked, which is what it records for a search that came
/// apart on the thread polling it.
#[test]
fn a_job_that_comes_apart_on_the_worker_comes_apart_in_the_call_as_it_does_inline() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    let on_worker = lent(&cancel, &worker);
    let inline = context();

    for (where_, context) in [("inline", &inline), ("on the worker", &on_worker)] {
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime.block_on(super::run("grep", context, |_| -> () {
                panic!("the search came apart")
            }))
        }));
        assert!(
            unwound.is_err(),
            "a job that came apart {where_} was answered rather than unwound: {unwound:?}"
        );
    }
    runtime.block_on(every_place_free(&worker));
}
