//! A small tree on disk, for the tools to work on.
//!
//! These tools are about the filesystem, so testing them against a fake one
//! would test the fake. Each fixture gets its own directory under the system
//! temporary directory and removes it when it drops.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_core::{
    Ancestry, Approved, Ask, Cancel, DescribeTool, Disposition, Permission, Remember, Rules,
    Sensitivity, Settled, Tool, ToolArgs, ToolCall, ToolContext, ToolId, Unwatched, Verdict, Watch,
    Workspace,
};

/// A fresh run context for a direct tool test that watches nothing.
pub(crate) fn context() -> ToolContext<'static> {
    cancelled_by(&Cancel::new())
}

/// A direct-test context stopped by `cancel` and otherwise unwatched.
pub(crate) fn cancelled_by(cancel: &Cancel) -> ToolContext<'static> {
    ToolContext::new(
        Ancestry::new(),
        ToolId::new("sample"),
        cancel,
        None,
        &Unwatched,
    )
}

/// A direct-test context that forwards incremental output to `watch`.
pub(crate) fn watching(watch: &dyn Watch) -> ToolContext<'_> {
    ToolContext::new(
        Ancestry::new(),
        ToolId::new("sample"),
        &Cancel::new(),
        None,
        watch,
    )
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
    /// cannot start, which surfaced as a `bash` test that failed under load and
    /// passed on its own.
    ///
    /// So the count is what separates them, and it cannot be forgotten the way
    /// a unique name can. The tests of one crate run in one process, which is
    /// what makes a counter enough.
    pub(crate) fn new(name: &str) -> Self {
        /// How many fixtures this process has made.
        static MADE: AtomicU64 = AtomicU64::new(0);

        let made = MADE.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("crucible-{name}-{}-{made}", std::process::id()));

        // A previous run that crashed leaves its tree behind, and a process
        // identifier is reused eventually. Cheap, and the only thing that can
        // still be in the way.
        let _ = fs::remove_dir_all(&base);

        let root = base.join("inside");
        fs::create_dir_all(&root).expect("a temporary directory");
        fs::create_dir_all(base.join("outside")).expect("a temporary directory");

        Self { base, root }
    }

    /// The workspace the tools are given.
    pub(crate) fn workspace(&self) -> Workspace {
        Workspace::open(&self.root).expect("the root exists")
    }

    /// The same workspace, widened to reach a directory outside the root —
    /// what `extraDirectories` does to the one the tools are given.
    pub(crate) fn reaching(&self, directory: &str) -> Workspace {
        self.workspace()
            .reaching([directory])
            .expect("a directory that is there")
    }

    /// A directory beside the workspace, made and returned as the text of a
    /// call names it. The counterpart to [`Self::outside`], for the tests about
    /// a directory the workspace was widened to reach on purpose rather than
    /// one it must refuse.
    pub(crate) fn beside(&self, name: &str) -> String {
        let path = self.base.join(name);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        crucible_core::written(&path)
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
    /// the tests that check a tool refuses to reach it.
    pub(crate) fn outside(&self, name: &str, text: &str) -> String {
        let path = self.base.join("outside").join(name);
        fs::write(&path, text).expect("a writable temporary directory");
        crucible_core::written(&path)
    }

    /// The workspace root, for the tests that need an absolute path into it.
    pub(crate) fn root(&self) -> &PathBuf {
        &self.root
    }

    /// The workspace root as the text of a call names it.
    ///
    /// Spelled the way [`Self::outside`] is, and for a reason that is about the
    /// test rather than about paths: this goes into a JSON string literal, and a
    /// backslash is what JSON escapes with. A Windows path dropped into one is
    /// not a path but a parse error a few characters in, so the tool refuses the
    /// arguments and never reaches the refusal the test is about.
    pub(crate) fn named(&self) -> String {
        crucible_core::written(&self.root)
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

/// A call the tool may run, permitted the only way one can be.
///
/// There is no constructor outside the permission engine, so even a test goes
/// through a verdict — which is the property the token exists to have. It also
/// means a test runs a tool on the arguments a verdict was reached about, by
/// the same construction the runner uses, rather than on a pair that only
/// happened to be assembled together.
pub(crate) fn allowed<T>(tool: &T, args: &str) -> Approved
where
    T: DescribeTool + Tool + ?Sized,
{
    permitted(tool, args, &[])
}

/// The same, with rules standing — for the tools that reach more files than
/// the one they were decided about, and have to refuse the rest themselves.
pub(crate) fn under<T>(tool: &T, args: &str, rules: &[(Disposition, &str)]) -> Approved
where
    T: DescribeTool + Tool + ?Sized,
{
    permitted(tool, args, rules)
}

/// Decides one call the only way a call can be decided.
fn permitted<T>(tool: &T, args: &str, written: &[(Disposition, &str)]) -> Approved
where
    T: DescribeTool + Tool + ?Sized,
{
    struct Yes;

    impl Ask for Yes {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            (Verdict::Allow, Remember::Never)
        }
    }

    let mut rules = Rules::new();
    for (kind, text) in written {
        rules.add(*kind, text).expect("a readable rule");
    }

    let call = ToolCall {
        id: ToolId::new("sample"),
        name: tool.name().into(),
        args: ToolArgs::new(args),
    };

    let sensitivity = tool.sensitivity(&call.args);
    match Permission::with(crucible_core::Mode::default(), rules).decide(
        &call,
        &sensitivity,
        &mut Yes,
    ) {
        Settled::Approved(approved) => approved,
        Settled::Forbidden | Settled::Refused => {
            panic!("the rules here name files below the call, and the answer above is yes")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_fixtures_asked_for_under_one_name_are_still_two_directories() {
        // The defect this catches: a fixture's directory was named after the
        // string a test passed, so two tests passing the same one shared a
        // tree — and `Sample::new` empties the tree it is about to use. The
        // second test's setup therefore deleted the first test's workspace
        // while it was running, and a command spawned with a working directory
        // that has been removed fails with "no such file or directory". It
        // showed up as a flaky `bash` test, which is the one place a missing
        // working directory has anything to say.
        let first = Sample::new("same-name");
        first.write("kept.txt", "still here");

        let second = Sample::new("same-name");
        second.write("theirs.txt", "and so is this");

        assert!(
            first.root().join("kept.txt").is_file(),
            "the second fixture emptied the first one's workspace"
        );
        assert!(second.root().join("theirs.txt").is_file());
        assert_ne!(first.root(), second.root());
    }
}
