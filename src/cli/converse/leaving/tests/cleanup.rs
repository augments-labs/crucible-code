//! A real background process whose explicit cleanup can fail before retry.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crucible_core::{
    CallResultKey, CallResultReceipt, SandboxBackendIdentity, SandboxCapabilities, SandboxCommand,
    SandboxError, SandboxInspection, SandboxLaunch, SandboxOutput, SandboxProcess, SandboxRequest,
    SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
};
use crucible_tools::LocalSandbox;

pub(super) const PRIVATE_ERROR: &str = "synthetic-private-cleanup-details";

pub(super) fn sandbox() -> (Arc<dyn SandboxService>, Arc<AtomicBool>) {
    let denied = Arc::new(AtomicBool::new(true));
    (
        Arc::new(Fallible {
            inner: Box::new(LocalSandbox::new()),
            denied: Arc::clone(&denied),
        }),
        denied,
    )
}

struct Fallible<T: ?Sized> {
    inner: Box<T>,
    denied: Arc<AtomicBool>,
}

impl SandboxService for Fallible<LocalSandbox> {
    fn probe(&self) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
        self.inner.probe()
    }

    fn prepare(&self, request: SandboxRequest) -> Result<Box<dyn SandboxSession>, SandboxError> {
        Ok(Box::new(Fallible {
            inner: self.inner.prepare(request)?,
            denied: Arc::clone(&self.denied),
        }))
    }
}

impl SandboxSession for Fallible<dyn SandboxSession> {
    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        self.inner.materialize()
    }

    fn stage(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxLaunch>, SandboxError> {
        Ok(Box::new(Fallible {
            inner: self.inner.stage(command)?,
            denied: self.denied,
        }))
    }
}

impl SandboxLaunch for Fallible<dyn SandboxLaunch> {
    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn transfer_owner(&mut self) -> Result<(), SandboxError> {
        self.inner.transfer_owner()
    }

    fn release(self: Box<Self>) -> Result<Box<dyn SandboxProcess>, SandboxError> {
        Ok(Box::new(Fallible {
            inner: self.inner.release()?,
            denied: self.denied,
        }))
    }
}

impl SandboxProcess for Fallible<dyn SandboxProcess> {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        self.inner.take_stdin()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.inner.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.inner.take_stderr()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.denied.load(Ordering::Relaxed) {
            Err(io::Error::other(PRIVATE_ERROR))
        } else {
            self.inner.stop()
        }
    }

    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.inner.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.inner.violation()
    }

    fn begin_background_acceptance(&mut self, key: CallResultKey) -> Result<(), SandboxError> {
        self.inner.begin_background_acceptance(key)
    }

    fn complete_background_acceptance(
        &mut self,
        receipt: CallResultReceipt,
    ) -> Result<(), SandboxError> {
        self.inner.complete_background_acceptance(receipt)
    }
}
