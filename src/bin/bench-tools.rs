//! Latency of the real deterministic tools over a production-shaped workspace.
//!
//! Read, glob, edit, write and a foreground compatibility sandbox command are
//! invoked through their public `Tool` and permission contracts. Every sample
//! uses a fresh path where mutation is involved, verifies the effect, and times
//! only the call: fixture construction happens outside the measured region.
//! The budget is the slowest median rather than the sum, with every operation
//! retained as numeric evidence in the performance artifact.
//!
//! Each call is lent a tool worker, on a runtime this probe builds shaped as
//! the application's, so a tool that hands its blocking work over is timed
//! doing so; the probe waits on that runtime for the answer.

use std::fmt::Write as _;
use std::fs;
use std::future::Future;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crucible_builtins::{Bash, Edit, Glob, Ledger, Read, Write};
use crucible_core::{
    Ancestry, Ask, Cancel, Mode, Permission, Remember, Sensitivity, Settled, Tool, ToolArgs,
    ToolCall, ToolContext, ToolError, ToolId, ToolOutput, Unwatched, Verdict, Workspace,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::ToolWorker;
use tokio::runtime::{Builder, Runtime};

/// Median invocations retained for each operation.
const RUNS: usize = 31;
/// A deliberately generous shared-runner ceiling, in milliseconds.
const LIMIT: f64 = 40.0;

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("bench-tools: {0}")]
    Io(#[from] io::Error),
    #[error("bench-tools: {0}")]
    Workspace(#[from] crucible_core::PathError),
    #[error("bench-tools: {0}")]
    Tool(#[from] ToolError),
    #[error("bench-tools: permission did not approve {0}")]
    Permission(Box<str>),
    #[error("bench-tools: {0} reported failure: {1}")]
    Failed(&'static str, Box<str>),
    #[error("bench-tools: {0}")]
    Wrong(Box<str>),
}

/// Full access should settle every benchmark call before asking.
struct Unasked;

impl Ask for Unasked {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Deny, Remember::Never) })
    }
}

/// The runtime each call is waited on, and the tool worker it is lent.
///
/// Shaped as the application's in what these calls reach: four workers, a
/// clock for the worker's wait for room and a command's limits, and at most
/// eight blocking threads. No I/O driver, which nothing timed here waits on.
/// Built before the measured calls, as the application builds its own before
/// a turn. A sandboxed command's status is watched on it too.
struct Driver {
    runtime: Runtime,
    worker: ToolWorker,
}

impl Driver {
    fn new() -> Result<Self, ProbeError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(4)
            .max_blocking_threads(8)
            .enable_time()
            .build()?;
        let worker = ToolWorker::new(runtime.handle().clone());
        Ok(Self { runtime, worker })
    }

    /// Waits on the probe's own thread for `future` to answer.
    fn answer<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

struct Scratch {
    base: PathBuf,
    workspace: Workspace,
}

impl Scratch {
    fn new() -> Result<Self, ProbeError> {
        let base =
            std::env::temp_dir().join(format!("crucible-bench-tools-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("tree/nested"))?;
        for number in 0..256 {
            fs::write(
                base.join(format!("tree/nested/file-{number:03}.txt")),
                format!("fixture {number}\nneedle {number}\n"),
            )?;
        }
        fs::write(base.join("read.txt"), "alpha\nbeta\ngamma\n")?;
        let workspace = Workspace::open(&base)?;
        Ok(Self { base, workspace })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn invoke(
    driver: &Driver,
    tool: &dyn Tool,
    name: &'static str,
    args: String,
) -> Result<(ToolOutput, Duration), ProbeError> {
    let call = ToolCall {
        id: ToolId::new(format!("bench-{name}")),
        name: name.into(),
        args: ToolArgs::new(args),
    };
    tool.validate(&call.args)?;
    let sensitivity = tool.sensitivity(&call.args);
    let mut permission = Permission::with(Mode::FullAccess, crucible_core::Rules::new());
    let Settled::Approved(approved) =
        driver.answer(permission.decide(&call, &sensitivity, &mut Unasked))
    else {
        return Err(ProbeError::Permission(name.into()));
    };
    let cancel = Cancel::new();
    let context = ToolContext::new(Ancestry::new(), call.id, &cancel, None, &Unwatched)
        .with_worker(&driver.worker);

    let started = Instant::now();
    let output = driver.answer(tool.run(approved, &context))?;
    let elapsed = started.elapsed();
    if output.is_failed() {
        return Err(ProbeError::Failed(name, output.into_text()));
    }
    Ok((output, elapsed))
}

fn median(mut readings: Vec<Duration>) -> Result<f64, ProbeError> {
    readings.sort_unstable();
    readings
        .get(readings.len() / 2)
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .ok_or_else(|| ProbeError::Wrong("an operation produced no readings".into()))
}

fn read_latency(driver: &Driver, scratch: &Scratch, ledger: &Ledger) -> Result<f64, ProbeError> {
    let tool = Read::new(scratch.workspace.clone(), ledger.clone());
    let mut readings = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let (output, elapsed) = invoke(driver, &tool, "read", r#"{"path":"read.txt"}"#.to_owned())?;
        if !output.text().contains("     2\tbeta") {
            return Err(ProbeError::Wrong("read omitted the planted line".into()));
        }
        readings.push(elapsed);
    }
    median(readings)
}

fn glob_latency(driver: &Driver, scratch: &Scratch) -> Result<f64, ProbeError> {
    let tool = Glob::new(scratch.workspace.clone());
    let mut readings = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let (output, elapsed) = invoke(
            driver,
            &tool,
            "glob",
            r#"{"pattern":"tree/**/*.txt","limit":300}"#.to_owned(),
        )?;
        if !output.text().contains("file-255.txt") {
            return Err(ProbeError::Wrong("glob omitted the planted tail".into()));
        }
        readings.push(elapsed);
    }
    median(readings)
}

fn edit_latency(driver: &Driver, scratch: &Scratch) -> Result<f64, ProbeError> {
    let tool = Edit::new(scratch.workspace.clone());
    let mut readings = Vec::with_capacity(RUNS);
    for number in 0..RUNS {
        let path = format!("edit-{number:02}.txt");
        fs::write(scratch.base.join(&path), "before\n")?;
        let args = format!(r#"{{"path":"{path}","find":"before","replace":"after"}}"#);
        let (output, elapsed) = invoke(driver, &tool, "edit", args)?;
        if output.diff().is_none_or(crucible_core::Diff::is_empty)
            || fs::read_to_string(scratch.base.join(path))? != "after\n"
        {
            return Err(ProbeError::Wrong(
                "edit did not make the planted replacement".into(),
            ));
        }
        readings.push(elapsed);
    }
    median(readings)
}

fn write_latency(driver: &Driver, scratch: &Scratch, ledger: &Ledger) -> Result<f64, ProbeError> {
    let tool = Write::new(scratch.workspace.clone(), ledger.clone());
    let mut readings = Vec::with_capacity(RUNS);
    for number in 0..RUNS {
        let path = format!("write-{number:02}.txt");
        let args = format!(r#"{{"path":"{path}","content":"written {number}\\n"}}"#);
        let (output, elapsed) = invoke(driver, &tool, "write", args)?;
        if output.diff().is_none_or(crucible_core::Diff::is_empty)
            || !scratch.base.join(path).is_file()
        {
            return Err(ProbeError::Wrong(
                "write did not create the planted file".into(),
            ));
        }
        readings.push(elapsed);
    }
    median(readings)
}

fn sandbox_latency(driver: &Driver, scratch: &Scratch) -> Result<f64, ProbeError> {
    let sandbox = LocalSandbox::new().watching_on(driver.runtime.handle().clone());
    let tool = Bash::new(scratch.workspace.clone(), Arc::new(sandbox)).sandboxing(false);
    let mut readings = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let (output, elapsed) = invoke(
            driver,
            &tool,
            "bash",
            r#"{"command":"printf sandbox-ready"}"#.to_owned(),
        )?;
        if !output.text().contains("sandbox-ready") {
            return Err(ProbeError::Wrong(
                "sandbox command omitted its marker".into(),
            ));
        }
        readings.push(elapsed);
    }
    median(readings)
}

fn measure() -> Result<[f64; 5], ProbeError> {
    let scratch = Scratch::new()?;
    let ledger = Ledger::new();
    let driver = Driver::new()?;
    Ok([
        read_latency(&driver, &scratch, &ledger)?,
        glob_latency(&driver, &scratch)?,
        edit_latency(&driver, &scratch)?,
        write_latency(&driver, &scratch, &ledger)?,
        sandbox_latency(&driver, &scratch)?,
    ])
}

fn report(measured: [f64; 5]) -> Result<(), io::Error> {
    let [read, glob, edit, write, sandbox] = measured;
    let slowest = measured.into_iter().fold(0.0_f64, f64::max);
    let mut line = String::new();
    let _ = write!(
        line,
        "{slowest:.3} ms {LIMIT:.0} read={read:.3} glob={glob:.3} edit={edit:.3} write={write:.3} sandbox={sandbox:.3}"
    );
    line.push('\n');
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

fn explain(problem: &ProbeError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL {problem}");
    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let measured = match measure() {
        Ok(measured) => measured,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };
    if report(measured).is_err() {
        return ExitCode::FAILURE;
    }
    if measured.into_iter().any(|reading| reading > LIMIT) {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
