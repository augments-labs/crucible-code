//! A small tree on disk, for the tools to work on.
//!
//! These tools are about the filesystem, so testing them against a fake one
//! would test the fake. Each fixture gets its own directory under the system
//! temporary directory and removes it when it drops.

use std::fs;
use std::path::PathBuf;

use crucible_core::{
    Ask, Grant, Permission, Sensitivity, ToolArgs, ToolCall, ToolId, Verdict, Workspace,
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
        path.display().to_string()
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

/// Arguments as the model would have written them.
pub(crate) fn call(args: &str) -> ToolArgs {
    ToolArgs::new(args)
}

/// A grant, minted the only way one can be.
///
/// There is no constructor outside the permission engine, so even a test has
/// to go through a verdict — which is the property the token exists to have.
pub(crate) fn allowed() -> Grant {
    struct Yes;

    impl Ask for Yes {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: Sensitivity) -> Verdict {
            Verdict::AllowOnce
        }
    }

    let call = ToolCall {
        id: ToolId::new("sample"),
        name: "sample".into(),
        args: ToolArgs::new("{}"),
    };

    Permission::new()
        .decide(&call, Sensitivity::ReadOnly, &mut Yes)
        .expect("a read-only call is always granted")
}
