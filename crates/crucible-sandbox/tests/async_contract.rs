//! A sandbox backend written outside this crate, driven through its whole
//! lifecycle as trait objects.
//!
//! This file is a crate of its own and sees only what `crucible-sandbox`
//! exports: a service that probes and prepares, a session that materializes
//! and stages, a launch that is released and a process that is stopped. Every
//! step that waits hands back a future, and a step that consumes what it was
//! called on hands back one that owns it.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_runtime::{BoxFuture, answered};
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCleanup, SandboxCommand, SandboxEnvironment, SandboxError, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection, SandboxLaunch,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRequest, SandboxResourceLimits, SandboxService, SandboxSession, SandboxUsage,
    SandboxViolation, inspection,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

/// An absolute path spelled the way the running platform's path type accepts.
#[cfg(unix)]
const ROOT: &str = "/workspace";
#[cfg(windows)]
const ROOT: &str = r"C:\workspace";

/// An absolute program, which is the only kind a sandbox command accepts;
/// nothing here runs it.
#[cfg(unix)]
const PROGRAM: &str = "/bin/true";
#[cfg(windows)]
const PROGRAM: &str = r"C:\true.exe";

/// How far every lifecycle one service handed out has gone.
#[derive(Default)]
struct Counts {
    prepared: AtomicUsize,
    materialized: AtomicUsize,
    staged: AtomicUsize,
    released: AtomicUsize,
    stopped: AtomicUsize,
}

impl Counts {
    fn count(counter: &AtomicUsize) {
        counter.fetch_add(1, Ordering::SeqCst);
    }

    fn read(&self) -> [usize; 5] {
        [
            &self.prepared,
            &self.materialized,
            &self.staged,
            &self.released,
            &self.stopped,
        ]
        .map(|counter| counter.load(Ordering::SeqCst))
    }
}

/// A backend that confines nothing, and says so in every inspection.
struct Unconfined {
    identity: SandboxBackendIdentity,
    counts: Arc<Counts>,
}

impl SandboxService for Unconfined {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move { Ok((self.identity.clone(), SandboxCapabilities::none())) })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let inspection = inspection(
                request.id(),
                self.identity.clone(),
                SandboxCapabilities::none(),
                request.policy(),
                request.manifest(),
                false,
                Some("an external fake confines nothing"),
                SandboxCleanup::Pending,
            )?;
            Counts::count(&self.counts.prepared);
            let session: Box<dyn SandboxSession> = Box::new(Staging {
                inspection,
                counts: Arc::clone(&self.counts),
            });
            Ok(session)
        })
    }
}

/// A prepared session, waiting for its command.
struct Staging {
    inspection: SandboxInspection,
    counts: Arc<Counts>,
}

impl SandboxSession for Staging {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move {
            Counts::count(&self.counts.materialized);
            Ok(())
        })
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            if !command.program().is_absolute() {
                return Err(SandboxError::InvalidCommand);
            }
            let Self { inspection, counts } = *self;
            Counts::count(&counts.staged);
            let launch: Box<dyn SandboxLaunch> = Box::new(Held { inspection, counts });
            Ok(launch)
        })
    }
}

/// A staged command nothing has let run yet.
struct Held {
    inspection: SandboxInspection,
    counts: Arc<Counts>,
}

impl SandboxLaunch for Held {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let Self { inspection, counts } = *self;
            Counts::count(&counts.released);
            let process: Box<dyn SandboxProcess> = Box::new(Running {
                inspection,
                counts,
                stopped: false,
            });
            Ok(process)
        })
    }
}

/// A command that runs until it is stopped.
struct Running {
    inspection: SandboxInspection,
    counts: Arc<Counts>,
    stopped: bool,
}

impl SandboxProcess for Running {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        None
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(self.stopped.then(ExitStatus::default))
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            if !self.stopped {
                self.stopped = true;
                Counts::count(&self.counts.stopped);
            }
            Ok(())
        })
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        None
    }
}

#[allow(clippy::expect_used)] // Every word the identity is given is valid.
fn service(counts: &Arc<Counts>) -> Arc<dyn SandboxService> {
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("external").expect("a valid backend id"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("a valid identity");
    Arc::new(Unconfined {
        identity,
        counts: Arc::clone(counts),
    })
}

/// A request under a policy that confines nothing, which is all this backend
/// may truthfully report.
#[allow(clippy::expect_used)] // Every path the policy is given is valid.
fn request() -> SandboxRequest {
    let workspace = SandboxFilesystemRule::new(
        ROOT,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("a valid rule");
    let policy = SandboxPolicy::new(
        false,
        [workspace],
        ROOT,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a valid policy");
    SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("call"),
        policy,
        SandboxManifest::empty(),
    )
}

#[allow(clippy::expect_used)] // An absolute program with no arguments is valid.
fn command() -> SandboxCommand {
    SandboxCommand::new(PROGRAM, std::iter::empty(), SandboxEnvironment::empty())
        .expect("a valid command")
}

#[test]
fn an_external_backend_runs_a_command_through_its_whole_lifecycle() {
    let counts = Arc::new(Counts::default());
    let service = service(&counts);

    let (identity, _) = answered!(service.probe()).expect("the backend answers");
    let mut session = answered!(service.prepare(request())).expect("the session is prepared");
    assert_eq!(session.inspection().backend(), &identity);
    answered!(session.materialize()).expect("nothing to materialize");
    let launch = answered!(session.stage(command())).expect("the command is staged");
    let mut process = answered!(launch.release()).expect("the command is released");

    assert!(!process.ended());
    answered!(process.stop()).expect("the command stops");
    answered!(process.stop()).expect("stopping twice is stopping once");
    assert!(process.ended());
    assert_eq!(counts.read(), [1, 1, 1, 1, 1]);
}

#[test]
fn a_foreground_start_is_asked_on_another_thread() {
    let counts = Arc::new(Counts::default());
    let service = service(&counts);
    let session = answered!(service.prepare(request())).expect("the session is prepared");

    // The provided `start` stages and releases in one future, which owns the
    // session it consumed and so may be asked wherever it is sent.
    let starting = session.start(command());
    let started = std::thread::scope(|scope| {
        scope
            .spawn(move || answered!(starting))
            .join()
            .expect("the poll ends")
    });

    let mut process = started.expect("the command starts");
    answered!(process.stop()).expect("the command stops");
    assert_eq!(counts.read(), [1, 0, 1, 1, 1]);
}
