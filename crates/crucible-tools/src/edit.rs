//! Changing part of a file.
//!
//! Exact text in, exact text out. No patch format and no line numbers: a model
//! that has just read a file can quote from it, and quoting is the one thing it
//! can do without counting. The file and its replacement are each capped at
//! one megabyte: an exact whole-file transformation needs both in memory, so a
//! larger file belongs in a streaming tool rather than in this one.

use std::io::{self, Read as _};

use crucible_core::{
    Approved, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace,
};

use crate::args::Args;
use crate::atomic;
use crate::target;

/// The name the model calls.
const NAME: &str = "edit";

/// The most source or resulting text one call holds for a whole-file edit.
const FILE_LIMIT: usize = 1_000_000;

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Replaces exact text in a file in the workspace. The text to find must appear exactly once unless all is true. Source and result must each be no larger than 1000000 bytes.",
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
    cancel: Cancel,
}

impl Edit {
    /// Edits inside `workspace`, and nowhere else.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self { workspace, cancel }
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

        // Read through a descriptor-relative open. If the last component or a
        // directory above it became a link after resolution, the open refuses
        // it rather than bringing outside bytes into this transformation. The
        // commit below is likewise relative to the proven parent and renames
        // over a newly planted link rather than following it.
        let mut file = match path.open_regular_to_change() {
            Ok(file) => file,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        // Fixed-size reads put a cancellation point inside the scan and keep
        // retained source bytes below the declared whole-file ceiling. A large
        // sparse or minified input is therefore bounded both in memory and in
        // how long a stopped turn keeps reading it.
        let before = match source(&mut file, &self.cancel) {
            Ok(Source::Text(before)) => before,
            Ok(Source::TooLarge) => return Ok(too_large(requested)),
            Ok(Source::Cancelled) => return Err(ToolError::Cancelled(NAME)),
            Ok(Source::Binary) => {
                return Ok(ToolOutput::failed(format!(
                    "{requested} is not a text file"
                )));
            }
            Err(source) => {
                return Err(ToolError::Io {
                    tool: NAME,
                    problem: format!("could not read {requested}").into(),
                    source,
                });
            }
        };

        let found = before.matches(find).count();
        if let Some(problem) = trouble(found, all, requested) {
            return Ok(problem);
        }

        let changed = if all { found } else { 1 };
        let after_len = changed
            .checked_mul(find.len())
            .and_then(|removed| before.len().checked_sub(removed))
            .and_then(|without| {
                changed
                    .checked_mul(replace.len())
                    .and_then(|added| without.checked_add(added))
            });
        if after_len.is_none_or(|length| length > FILE_LIMIT) {
            return Ok(too_large(requested));
        }

        let after = if all {
            before.replace(find, replace)
        } else {
            before.replacen(find, replace, 1)
        };

        let permissions = file
            .metadata()
            .map_err(|source| ToolError::Io {
                tool: NAME,
                problem: format!("could not inspect {requested}").into(),
                source,
            })?
            .permissions();
        if self.cancel.requested() {
            return Err(ToolError::Cancelled(NAME));
        }
        // The replacement is prepared beside the old file, flushed, and
        // renamed only after it is whole. At no point can a reader observe the
        // empty or partially-written interval that truncating in place creates;
        // an identity change detected at the final pre-commit check is refused
        // as well.
        if let Err(problem) =
            atomic::replace(&path, after.as_bytes(), Some(permissions), Some(&file))
        {
            return Ok(ToolOutput::failed(problem.to_string()));
        }

        Ok(ToolOutput::ok(format!(
            "changed {requested}, {changed} replacements"
        )))
    }
}

/// The bounded outcomes of reading a source file.
enum Source {
    Text(String),
    TooLarge,
    Binary,
    Cancelled,
}

/// Reads one edit source with a stop check between fixed-size reads.
fn source(file: &mut std::fs::File, cancel: &Cancel) -> io::Result<Source> {
    let mut bytes = Vec::new();
    let mut block = [0_u8; 8 * 1024];

    loop {
        if cancel.requested() {
            return Ok(Source::Cancelled);
        }
        let read = file.read(&mut block)?;
        if cancel.requested() {
            return Ok(Source::Cancelled);
        }
        if read == 0 {
            break;
        }
        let Some(arrived) = block.get(..read) else {
            return Err(io::Error::other("a file read exceeded its buffer"));
        };
        if bytes.len().saturating_add(arrived.len()) > FILE_LIMIT {
            return Ok(Source::TooLarge);
        }
        bytes.extend_from_slice(arrived);
    }

    match String::from_utf8(bytes) {
        Ok(text) => Ok(Source::Text(text)),
        Err(_) => Ok(Source::Binary),
    }
}

/// A bounded edit the caller can split or perform another way.
fn too_large(requested: &str) -> ToolOutput {
    ToolOutput::failed(format!(
        "{requested} is too large to edit safely: source and result must each be at most {FILE_LIMIT} bytes"
    ))
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
