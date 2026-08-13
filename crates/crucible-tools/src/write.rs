//! Putting a whole file down.

use std::fs;
use std::io::Write as _;

use crucible_core::{
    Approved, PathError, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace,
};

use crate::args::Args;
use crate::target;

/// The name the model calls.
const NAME: &str = "write";

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Writes a file in the workspace, replacing it if it is already there. Creates any parent directories it needs.",
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "The file to write, relative to the workspace root."
    },
    "content": {
      "type": "string",
      "description": "The complete new contents of the file."
    }
  },
  "required": ["path", "content"]
}"#;

/// Writes a file inside the workspace.
#[derive(Debug)]
pub struct Write {
    workspace: Workspace,
}

impl Write {
    /// Writes inside `workspace`, and nowhere else.
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for Write {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile {
            target: target::creatable(&self.workspace, NAME, args, "path"),
        }
    }

    fn run(&self, approved: Approved) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let requested = args.text("path")?;
        let content = args.exact("content")?;

        // The parent has to exist before the path can be contained, because
        // containment is decided on a resolved path and only a directory that
        // is really there can be resolved. So the directories are made first,
        // through a parent that has itself been checked.
        if let Some(problem) = self.prepare(requested)? {
            return Ok(problem);
        }

        let path = match self.workspace.creatable(requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        // What is at the name now, asked about the name itself rather than
        // through it: `creatable` proved the last component was not a symbolic
        // link, so one there now arrived since, and `symlink_metadata` is the
        // question that sees it rather than the far end. All this decides is
        // which of the two opens the write is — both of them refuse a name that
        // has become a link, so nothing rests on getting it right.
        let already = fs::symlink_metadata(&path);
        if already.as_ref().is_ok_and(fs::Metadata::is_dir) {
            return Ok(ToolOutput::failed(format!("{requested} is a directory")));
        }

        let replaced = already.is_ok();
        let opened = if replaced {
            path.open_to_change()
        } else {
            path.create()
        };

        let mut file = match opened {
            Ok(file) => file,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        // Both of these are about the descriptor rather than the name, so the
        // file that was opened is the file that gets the text — there is no
        // second lookup here for anything to arrive in.
        file.set_len(0)
            .and_then(|()| file.write_all(content.as_bytes()))
            .map_err(|source| ToolError::Io {
                tool: NAME,
                problem: format!("could not write {requested}").into(),
                source,
            })?;

        let lines = content.lines().count();
        let what = if replaced { "replaced" } else { "created" };
        Ok(ToolOutput::ok(format!("{what} {requested}, {lines} lines")))
    }
}

impl Write {
    /// Makes the directories the path needs, one contained level at a time.
    ///
    /// Returns the failure the model should see, if there is one. Walking down
    /// rather than calling `create_dir_all` on the whole path is what keeps
    /// every level inside the workspace. The file itself is contained either
    /// way, by the check that follows this; what one call would leave behind is
    /// the *directories* — `../stray/one.txt` is refused at the end, after
    /// `stray` has already been made outside the tree.
    ///
    /// Making a level is the one step here that a second writer cannot race:
    /// the operating system refuses `create_dir` when anything at all is
    /// already at the name, a symbolic link included, so a link planted at a
    /// level between the check and the make is an error rather than a way out
    /// of the tree.
    fn prepare(&self, requested: &str) -> Result<Option<ToolOutput>, ToolError> {
        let Some(parent) = std::path::Path::new(requested).parent() else {
            return Ok(None);
        };

        let mut so_far = std::path::PathBuf::new();
        for part in parent.components() {
            so_far.push(part);

            let Some(step) = so_far.to_str() else {
                return Ok(Some(ToolOutput::failed(format!(
                    "{requested} is not valid text"
                ))));
            };

            // A level that is already there is not one to make, and there are
            // two ways of being there. `Ok` is one inside the tree. `Escapes`
            // is one above it: an absolute path names every directory between
            // the filesystem root and the workspace on the way down, and those
            // exist without the workspace reaching them. Refusing that pair
            // was refusing every absolute path — with a message naming `/`,
            // which the caller never sent.
            if let Ok(_) | Err(PathError::Escapes { .. }) = self.workspace.existing(step) {
                continue;
            }

            let at = match self.workspace.creatable(step) {
                Ok(at) => at,
                Err(problem) => return Ok(Some(ToolOutput::failed(problem.to_string()))),
            };

            fs::create_dir(&at).map_err(|source| ToolError::Io {
                tool: NAME,
                problem: format!("could not create {step}").into(),
                source,
            })?;
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests;
