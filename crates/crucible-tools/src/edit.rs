//! Changing part of a file.
//!
//! Exact text in, exact text out. No patch format and no line numbers: a model
//! that has just read a file can quote from it, and quoting is the one thing it
//! can do without counting.

use std::io::{Read as _, Seek as _, Write as _};

use crucible_core::{Approved, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};

use crate::args::Args;
use crate::target;

/// The name the model calls.
const NAME: &str = "edit";

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Replaces exact text in a file in the workspace. The text to find must appear exactly once unless all is true.",
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "The file to change, relative to the workspace root."
    },
    "find": {
      "type": "string",
      "description": "The exact text to replace, copied from the file including its indentation."
    },
    "replace": {
      "type": "string",
      "description": "The text to put in its place. Empty to delete the text found."
    },
    "all": {
      "type": "boolean",
      "description": "Replace every occurrence instead of requiring exactly one. Defaults to false."
    }
  },
  "required": ["path", "find", "replace"]
}"#;

/// Replaces exact text in a file inside the workspace.
#[derive(Debug)]
pub struct Edit {
    workspace: Workspace,
}

impl Edit {
    /// Edits inside `workspace`, and nowhere else.
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for Edit {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile {
            target: target::existing(&self.workspace, NAME, args, "path"),
        }
    }

    fn run(&self, approved: Approved) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let requested = args.text("path")?;
        let find = args.text("find")?;
        let replace = args.exact("replace")?;
        let all = args.flag("all", false)?;

        if find == replace {
            return Ok(ToolOutput::failed(
                "find and replace are the same text, so there is nothing to change",
            ));
        }

        let path = match self.workspace.existing(requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        // One open file for the read and the write. Naming the path twice would
        // leave a gap between them in which the name could be made to lead
        // somewhere else, and the text put down here is decided from the text
        // read a moment ago — so if those are two files, the change is being
        // applied to one nobody looked at.
        let mut file = match path.open_to_change() {
            Ok(file) => file,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        let mut before = String::new();
        if file.read_to_string(&mut before).is_err() {
            // Something that is not text. The model sent the wrong path and can
            // send a different one.
            return Ok(ToolOutput::failed(format!(
                "{requested} is not a text file"
            )));
        }

        let found = before.matches(find).count();
        if let Some(problem) = trouble(found, all, requested) {
            return Ok(problem);
        }

        let after = if all {
            before.replace(find, replace)
        } else {
            before.replacen(find, replace, 1)
        };

        file.set_len(0)
            .and_then(|()| file.rewind())
            .and_then(|()| file.write_all(after.as_bytes()))
            .map_err(|source| ToolError::Io {
                tool: NAME,
                problem: format!("could not write {requested}").into(),
                source,
            })?;

        let changed = if all { found } else { 1 };
        Ok(ToolOutput::ok(format!(
            "changed {requested}, {changed} replacements"
        )))
    }
}

/// Why a count of occurrences is not one the call can act on.
///
/// Ambiguity is a failure rather than a guess. Replacing the first of several
/// identical fragments changes a line the model did not look at, and it has no
/// way to find out which one it got.
fn trouble(found: usize, all: bool, requested: &str) -> Option<ToolOutput> {
    match found {
        0 => Some(ToolOutput::failed(format!(
            "that text does not appear in {requested}"
        ))),
        1 => None,
        _ if all => None,
        many => Some(ToolOutput::failed(format!(
            "that text appears {many} times in {requested}: \
             include more of the surrounding lines, or pass all"
        ))),
    }
}

#[cfg(test)]
mod tests;
