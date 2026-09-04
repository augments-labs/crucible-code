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
        if self.denied.swap(false, Ordering::Relaxed) {
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

/// An explicit capture entrypoint: the parent supplies a controlling PTY and
/// sends `x`, observes the retry notice, then sends `x` again. The ordinary test
/// suite exercises the same behavior without borrowing the operator's terminal.
#[test]
#[ignore = "manual PTY capture; send x twice and retain the before/error/recovered frames"]
fn capture_cleanup_retry_unicode() {
    capture(crucible_config::Glyphs::Unicode);
}

#[test]
#[ignore = "manual PTY capture; send x twice and retain the before/error/recovered frames"]
fn capture_cleanup_retry_ascii() {
    capture(crucible_config::Glyphs::Ascii);
}

fn capture(glyphs: crucible_config::Glyphs) {
    use crucible_tui::{Ground, Raw, Renderer, Screen, SystemTerminal};

    use crate::cli::style::{Output, Style};

    let (sandbox, _) = sandbox();
    let (left, _workspace) = super::running_with("cleanup-visual", 1, sandbox);
    let _raw = Raw::enter()
        .expect("raw terminal")
        .expect("a controlling PTY");
    let _screen = Screen::take()
        .expect("alternate screen")
        .expect("a controlling PTY");
    let mut renderer = Renderer::new(SystemTerminal::stdout());
    let style = Style::resolve(
        Output {
            color: Some(crucible_config::Color::Always),
            glyphs: Some(glyphs),
            ..Output::default()
        },
        true,
        None,
        Some(Ground::Dark),
        &|_| None,
    );
    let ended = super::Leaving::default()
        .stand(&mut renderer, style, &left)
        .expect("interactive cleanup panel");
    assert_eq!(ended, super::Ended::Left);
    assert_eq!(
        left.count(),
        0,
        "capture must finish by successfully retrying stop"
    );
}
