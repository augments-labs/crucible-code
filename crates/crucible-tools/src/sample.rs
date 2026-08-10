//! A small tree on disk, for the tools to work on.
//!
//! These tools are about the filesystem, so testing them against a fake one
//! would test the fake. Each fixture gets its own directory under the system
//! temporary directory and removes it when it drops.

use std::fs;
use std::path::{Path, PathBuf};

use crucible_core::{
    Approved, Ask, Disposition, Permission, Remember, Rules, Sensitivity, Settled, Tool, ToolArgs,
    ToolCall, ToolId, Verdict, Workspace,
};

/// A workspace with a directory beside it that is deliberately outside.
pub(crate) struct Sample {
    base: PathBuf,
    root: PathBuf,
}

impl Sample {
    /// A fresh, empty workspace. `name` only has to be unique within the
    /// crate's tests, which run in one process.
    pub(crate) fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("crucible-{name}-{}", std::process::id()));
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
pub(crate) fn allowed(tool: &dyn Tool, args: &str) -> Approved {
    permitted(tool, args, &[])
}

/// The same, with rules standing — for the tools that reach more files than
/// the one they were decided about, and have to refuse the rest themselves.
pub(crate) fn under(tool: &dyn Tool, args: &str, rules: &[(Disposition, &str)]) -> Approved {
    permitted(tool, args, rules)
}

/// Decides one call the only way a call can be decided.
fn permitted(tool: &dyn Tool, args: &str, written: &[(Disposition, &str)]) -> Approved {
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
