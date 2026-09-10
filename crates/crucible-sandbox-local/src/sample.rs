//! A small tree on disk, for the backends to confine.
//!
//! These backends are about what a process may reach on a real filesystem, so
//! testing them against a fake one would test the fake. Each fixture gets its
//! own directory under the system temporary directory and removes it when it
//! drops.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_workspace::{Workspace, written};

/// The variable a continuous-integration job sets so that a test needing the
/// enforcing Linux backend fails when that backend is unavailable, instead of
/// quietly passing over nothing.
///
/// This crate publishes no test harness, so the name is spelled again in
/// `.github/workflows/rust-ci.yml`, which sets it, and in
/// `tests/sandbox_conformance.rs`, which cannot reach in here. Changing the
/// string here reddens the test that pins it, which is where those two are
/// named.
pub(crate) const REQUIRE_ENFORCING_SANDBOX: &str = "CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX";

/// Whether a test that needs the enforcing backend has to stop here.
///
/// On a developer machine without a usable Bubblewrap the test is skipped, which
/// is the honest answer for a boundary nobody can exercise there. Where the job
/// has declared that the backend must exist, an unavailable backend is a failure
/// naming the reason, so a suite that measured nothing cannot report green.
pub(crate) fn skipped_without_enforcement(service: &crate::LocalSandbox) -> bool {
    match crucible_sandbox::SandboxService::probe(service) {
        Ok(_) => false,
        Err(problem) => {
            assert!(
                std::env::var_os(REQUIRE_ENFORCING_SANDBOX).is_none(),
                "the enforcing sandbox backend is required by this job but unavailable: {problem}"
            );
            true
        }
    }
}

/// A workspace with a directory beside it that is deliberately outside.
pub(crate) struct Sample {
    base: PathBuf,
    root: PathBuf,
}

impl Sample {
    /// A fresh, empty workspace.
    ///
    /// `name` is a label, so a directory left behind by a run that crashed says
    /// which test made it. It is deliberately **not** what makes the directory
    /// unique: two fixtures asked for under one name used to share a tree, and
    /// this constructor empties the tree it is about to use — so the second
    /// test's setup deleted the first test's workspace out from under it while
    /// it ran. A command spawned with a working directory that has been removed
    /// cannot start, which surfaced as a test that failed under load and passed
    /// on its own.
    ///
    /// So the count is what separates them, and it cannot be forgotten the way
    /// a unique name can. The tests of one crate run in one process, which is
    /// what makes a counter enough.
    pub(crate) fn new(name: &str) -> Self {
        Self::below(name, &std::env::temp_dir())
    }

    /// Unix socket names must fit Darwin's 104-byte sockaddr field. Its usual
    /// per-user temporary directory alone can consume most of that field.
    #[cfg(unix)]
    pub(crate) fn socket(name: &str) -> Self {
        let temporary = Path::new("/tmp")
            .canonicalize()
            .expect("canonical temporary root");
        Self::below(name, &temporary)
    }

    fn below(name: &str, temporary: &Path) -> Self {
        /// How many fixtures this process has made.
        static MADE: AtomicU64 = AtomicU64::new(0);

        let made = MADE.fetch_add(1, Ordering::Relaxed);
        let base = temporary.join(format!("crucible-{name}-{}-{made}", std::process::id()));

        // A previous run that crashed leaves its tree behind, and a process
        // identifier is reused eventually. Cheap, and the only thing that can
        // still be in the way.
        let _ = fs::remove_dir_all(&base);

        let root = base.join("inside");
        fs::create_dir_all(&root).expect("a temporary directory");
        fs::create_dir_all(base.join("outside")).expect("a temporary directory");

        Self { base, root }
    }

    /// The workspace a command is confined to.
    pub(crate) fn workspace(&self) -> Workspace {
        Workspace::open(&self.root).expect("the root exists")
    }

    /// A directory beside the workspace, made and returned as the text of a
    /// call names it. The counterpart to [`Self::outside`], for the tests about
    /// a directory the workspace was widened to reach on purpose rather than
    /// one it must refuse.
    pub(crate) fn beside(&self, name: &str) -> String {
        let path = self.base.join(name);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        written(&path)
    }

    /// Writes a text file, creating the directories above it.
    pub(crate) fn write(&self, at: &str, text: &str) {
        self.write_bytes(at, text.as_bytes());
    }

    /// Writes a file that need not be text.
    pub(crate) fn write_bytes(&self, at: &str, bytes: &[u8]) {
        let path = self.root.join(at);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("a directory in the workspace");
        }
        fs::write(path, bytes).expect("a writable temporary directory");
    }

    /// Writes a file outside the workspace and returns its absolute path, for
    /// the tests that check a command cannot reach it.
    pub(crate) fn outside(&self, name: &str, text: &str) -> String {
        let path = self.base.join("outside").join(name);
        fs::write(&path, text).expect("a writable temporary directory");
        written(&path)
    }

    /// The workspace root, for the tests that need an absolute path into it.
    pub(crate) fn root(&self) -> &PathBuf {
        &self.root
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// A symbolic link at `link` pointing at `target`, which need not exist.
///
/// Every link here stands in for one a cloned repository shipped, so the far end
/// is always a file. Windows has a call per kind and no way to make one for a
/// target that is not there yet, which is why the kind is in the name rather
/// than read off the target.
///
/// Making one on Windows is a privilege: developer mode, or an elevated shell.
/// Failing loudly is right — a link that was not made turns a containment test
/// into one that passes because there was nothing to escape through.
pub(crate) fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, link);

    made.expect("a symbolic link: on Windows this needs developer mode");
}
