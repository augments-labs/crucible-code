//! Putting a whole file down.

use std::fs;

use crucible_core::{Grant, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};

use crate::args::Args;

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

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile
    }

    fn run(&self, args: ToolArgs, _grant: Grant) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, &args)?;
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

        if path.as_path().is_dir() {
            return Ok(ToolOutput::failed(format!("{requested} is a directory")));
        }

        let replaced = path.as_path().exists();

        fs::write(&path, content).map_err(|source| ToolError::Io {
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
            if self.workspace.existing(step).is_ok() {
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
