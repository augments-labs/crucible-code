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
use crucible_runtime::BoxFuture;
use crucible_sandbox_local::LocalSandbox;

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
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move { self.inner.probe().await })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            Ok(Box::new(Fallible {
                inner: self.inner.prepare(request).await?,
                denied: Arc::clone(&self.denied),
            }) as Box<dyn SandboxSession>)
        })
    }
}

impl SandboxSession for Fallible<dyn SandboxSession> {
    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { self.inner.materialize().await })
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            Ok(Box::new(Fallible {
                inner: self.inner.stage(command).await?,
                denied: self.denied,
            }) as Box<dyn SandboxLaunch>)
        })
    }
}

impl SandboxLaunch for Fallible<dyn SandboxLaunch> {
    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn transfer_owner(&mut self) -> Result<(), SandboxError> {
        self.inner.transfer_owner()
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            Ok(Box::new(Fallible {
                inner: self.inner.release().await?,
                denied: self.denied,
            }) as Box<dyn SandboxProcess>)
        })
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

    fn ended(&mut self) -> bool {
        self.inner.ended()
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            if self.denied.swap(false, Ordering::Relaxed) {
                Err(io::Error::other(PRIVATE_ERROR))
            } else {
                self.inner.stop().await
            }
        })
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

    fn begin_background_acceptance(
        &mut self,
        key: CallResultKey,
    ) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { self.inner.begin_background_acceptance(key).await })
    }

    fn complete_background_acceptance(
        &mut self,
        receipt: CallResultReceipt,
    ) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { self.inner.complete_background_acceptance(receipt).await })
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
