//! Changing part of a file.
//!
//! Exact text in, exact text out. No patch format and no line numbers: a model
//! that has just read a file can quote from it, and quoting is the one thing it
//! can do without counting. The file and its replacement are each capped at
//! one megabyte: an exact whole-file transformation needs both in memory, so a
//! larger file belongs in a streaming tool rather than in this one.
//!
//! A call carries one replacement or a list of them. The list is read once,
//! applied in order in memory, and written once, and any one of them that
//! cannot be made leaves the file exactly as it was — a file holding half of
//! what was asked for is a state nobody chose, and the model cannot see which
//! half it got without reading the file back.

use std::io::{self, Read as _};

use crucible_core::{
    Approved, Cancel, Remembered, Sensitivity, Summary, Tool, ToolArgs, ToolError, ToolOutput,
    Watch, Workspace,
};

use std::sync::LazyLock;

use crate::args::Args;
use crate::atomic;
use crate::changed;
use crate::schema::{Field, Schema, Shape};
use crate::summary;
use crate::target;

/// The name the model calls.
const NAME: &str = "edit";

/// The file to change.
const PATH: &str = "path";

/// The exact text to replace.
const FIND: &str = "find";

/// What to put in its place.
const REPLACE: &str = "replace";

/// Whether every occurrence is replaced.
const ALL: &str = "all";

/// Several changes in one call.
const EDITS: &str = "edits";

/// The most source or resulting text one call holds for a whole-file edit.
const FILE_LIMIT: usize = 1_000_000;

/// One change, described once — the same three fields stand at the top level
/// for a single change and inside each element of `edits` for several.
fn change(within: &str) -> Vec<Field> {
    vec![
        Field {
            name: FIND,
            about: format!(
                "The exact text to replace, copied from the file including its indentation.\
                 {within}"
            ),
            needed: true,
            shape: Shape::Text,
        },
        Field {
            name: REPLACE,
            about: "The text to put in its place. Empty to delete the text found.".into(),
            needed: true,
            shape: Shape::Text,
        },
        Field {
            name: ALL,
            about: "Replace every occurrence instead of requiring exactly one. Defaults to false."
                .into(),
            needed: false,
            shape: Shape::Flag,
        },
    ]
}

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
///
/// The account fields declared last are not for this tool and are never read
/// here. They are drawn on the panel where somebody decides whether this call
/// may run, and they arrive with the call because the thread holding the
/// terminal has no provider to ask when the panel opens. Neither is required:
/// a call that says nothing about itself gets the panel it would have got
/// before either existed. [`crate::account`] is what reads them.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    let mut fields = vec![Field {
        name: PATH,
        about: "The file to change, relative to the workspace root.".into(),
        needed: true,
        shape: Shape::Text,
    }];
    let mut single = change(" Send this and replace for a single change, or edits for several.");
    for one in &mut single {
        one.needed = false;
    }
    fields.append(&mut single);
    fields.push(Field {
        name: EDITS,
        about: "Several changes to make to the file in one call, instead of find and replace. \
                They are made in order, each one looking at what the one before it left. If any \
                of them cannot be made, none of them is made and the file is left as it was."
            .into(),
        needed: false,
        shape: Shape::List {
            of: Box::new(Shape::Fields(change(""))),
            fewest: None,
            most: None,
        },
    });
    fields.extend(crate::account::fields(
        "path",
        "What the change does",
        "Where the call makes several changes, or replaces every occurrence, account for each \
         place it lands.",
    ));
    Schema {
        about: format!(
            "Replaces exact text in a file in the workspace, either once or several times in one \
             call. The text to find must appear exactly once unless all is true. Source and \
             result must each be no larger than {FILE_LIMIT} bytes."
        ),
        fields,
    }
    .text()
});

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
        SCHEMA.as_str()
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile {
            target: target::existing(&self.workspace, NAME, args, PATH),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, PATH)
    }

    fn remember(&self, args: &ToolArgs) -> Option<Remembered> {
        summary::remembered(NAME, args, true)
    }

    fn run(&self, approved: Approved, _watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let requested = args.text(PATH)?;
        let listed = args.list(EDITS)?;
        let wanted = changes(&args, listed.as_deref())?;

        if let Some(at) = wanted
            .iter()
            .position(|change| change.find == change.replace)
        {
            return Ok(refused(
                at,
                wanted.len(),
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

        // Every change is made to the text in memory, and the file is written
        // only once they all have been. A list that fails part-way through has
        // touched nothing.
        // Both versions are kept, because two readers are owed different
        // things: the model is told how many replacements were made, and the
        // person watching is shown which lines moved, which cannot be worked
        // out from the result alone. Each is bounded by the ceiling above, and
        // both are gone when this call returns.
        let mut after = before.clone();
        let mut replaced = 0_usize;
        for (at, change) in wanted.iter().enumerate() {
            if self.cancel.requested() {
                return Err(ToolError::Cancelled(NAME));
            }

            let found = after.matches(change.find).count();
            if let Some(problem) = trouble(found, change.all, requested) {
                return Ok(refused(at, wanted.len(), &problem));
            }

            let made = if change.all { found } else { 1 };
            if grown(after.len(), made, change).is_none_or(|length| length > FILE_LIMIT) {
                return Ok(too_large(requested));
            }

            after = if change.all {
                after.replace(change.find, change.replace)
            } else {
                after.replacen(change.find, change.replace, 1)
            };
            replaced = replaced.saturating_add(made);
        }

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

        Ok(
            ToolOutput::ok(format!("changed {requested}, {replaced} replacements"))
                .showing(changed::between(&before, &after)),
        )
    }
}

/// One replacement a call asks for.
struct Replacement<'a> {
    find: &'a str,
    replace: &'a str,
    all: bool,
}

/// The replacements a call asks for, in whichever of the two shapes it sent.
///
/// A call that sent both is refused rather than read as one of them: taking
/// `edits` and dropping `find` would do half of what the call said and report
/// that it worked.
fn changes<'a>(
    args: &'a Args,
    listed: Option<&'a [Args]>,
) -> Result<Vec<Replacement<'a>>, ToolError> {
    let Some(each) = listed else {
        return Ok(vec![Replacement {
            find: args.text(FIND)?,
            replace: args.exact(REPLACE)?,
            all: args.flag(ALL, false)?,
        }]);
    };

    if args.holds(FIND) || args.holds(REPLACE) || args.holds(ALL) {
        return Err(args.wrong("send find and replace, or edits, but not both"));
    }
    if each.is_empty() {
        return Err(args.wrong("edits is empty"));
    }

    each.iter()
        .map(|one| {
            Ok(Replacement {
                find: one.text(FIND)?,
                replace: one.exact(REPLACE)?,
                all: one.flag(ALL, false)?,
            })
        })
        .collect()
}

/// How long the text is once a change has been made to it, or `None` where the
/// arithmetic leaves what a `usize` can hold.
fn grown(length: usize, made: usize, change: &Replacement<'_>) -> Option<usize> {
    let removed = made.checked_mul(change.find.len())?;
    let added = made.checked_mul(change.replace.len())?;
    length.checked_sub(removed)?.checked_add(added)
}

/// A failure naming which of several changes stopped the call.
///
/// The position is what the model needs to fix the call, and the rest of the
/// sentence is what it needs in order not to re-read the file first: a list is
/// made whole or not at all.
fn refused(at: usize, total: usize, problem: &str) -> ToolOutput {
    if total == 1 {
        return ToolOutput::failed(problem.to_owned());
    }

    ToolOutput::failed(format!(
        "edit {} of {total} could not be made, so nothing was changed: {problem}",
        at.saturating_add(1)
    ))
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
fn trouble(found: usize, all: bool, requested: &str) -> Option<String> {
    match found {
        0 => Some(format!("that text does not appear in {requested}")),
        1 => None,
        _ if all => None,
        many => Some(format!(
            "that text appears {many} times in {requested}: \
             include more of the surrounding lines, or pass all"
        )),
    }
}

#[cfg(test)]
mod tests;
