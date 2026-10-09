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
//!
//! A file the session has read is held to what the session saw: an edit of
//! one whose content changed since is refused, because the text it quotes
//! may now mean something else. A file the session never read can still be
//! edited, since the text it quotes has to be found in it. Either way the
//! file is held to the content the edit read until the replacement is
//! committed, so another writer's change in between is kept and the edit
//! refused rather than the change overwritten. Once it is committed, the
//! file is held to what the edit made, read first or not, so the next `edit`
//! or `write` of it needs no read; the ledger says why that counts for a
//! file the session never read.

use std::io::{self, Read as _};

use crucible_runtime::{BoxFuture, Cancel};
use crucible_tools::{
    Approved, DescribeTool, Remembered, Sensitivity, Summary, Tool, ToolContext, ToolError,
    ToolOutput,
};
use crucible_types::ToolArgs;
use crucible_workspace::{PathError, Workspace};

use std::sync::LazyLock;

use crate::args::Args;
use crate::atomic;
use crate::changed;
use crate::ledger::{Fingerprint, Ledger, Shown};
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

/// The most changes one call may list. The count is an argument like any
/// other, and each entry costs a scan of the whole file — so a list with no
/// ceiling would let one bounded call buy unbounded work. Far above any list a
/// call has a reason to send, and far below the point the scans add up.
const MOST_EDITS: usize = 256;

/// One change, described once — the same three fields stand at the top level
/// for a single change and inside each element of `edits` for several.
fn change(within: &str) -> Vec<Field> {
    vec![
        Field {
            name: FIND,
            about: format!("The exact text to replace, with its indentation.{within}"),
            needed: true,
            shape: Shape::Text,
        },
        Field {
            name: REPLACE,
            about: "Its replacement; empty deletes the text.".into(),
            needed: true,
            shape: Shape::Text,
        },
        Field {
            name: ALL,
            about: "Replace every occurrence. Defaults to false.".into(),
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
/// before either existed. [`fn@crate::account`] is what reads them.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    let mut fields = vec![Field {
        name: PATH,
        about: "The file, relative to the workspace root.".into(),
        needed: true,
        shape: Shape::Text,
    }];
    let mut single = change(" Pair with replace.");
    for one in &mut single {
        one.needed = false;
    }
    fields.append(&mut single);
    fields.push(Field {
        name: EDITS,
        about: "Several changes instead of find and replace, each made on what the one before \
                left. If any fails, none is made."
            .into(),
        needed: false,
        shape: Shape::List {
            of: Box::new(Shape::Fields(change(""))),
            fewest: None,
            most: Some(MOST_EDITS),
        },
    });
    fields.extend(crate::account::fields(
        "path",
        "What the change does",
        "Account for each place a change lands.",
    ));
    Schema {
        about: format!(
            "Replaces exact text in a workspace file. The text to find must appear exactly once \
             unless all is true. Source and result are each at most {FILE_LIMIT} bytes."
        ),
        fields,
    }
    .text()
});

/// Replaces exact text in a file inside the workspace.
#[derive(Debug)]
pub struct Edit {
    workspace: Workspace,
    seen: Ledger,
}

impl Edit {
    /// Edits inside `workspace`, and nowhere else, holding a file to what
    /// `seen` says the session last saw in it.
    #[must_use]
    pub fn new(workspace: Workspace, seen: Ledger) -> Self {
        Self { workspace, seen }
    }
}

impl DescribeTool for Edit {
    fn name(&self) -> &str {
        NAME
    }

    fn schema(&self) -> &str {
        SCHEMA.as_str()
    }
}

impl Tool for Edit {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        let args = Args::parse(NAME, args)?;
        args.text(PATH)?;
        let listed = args.list(EDITS)?;
        changes(&args, listed.as_deref()).map(drop)
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile {
            target: target::existing(&self.workspace, NAME, args, PATH),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, PATH, crucible_tools::Argument::Path)
    }

    fn remember(&self, args: &ToolArgs) -> Option<Remembered> {
        summary::remembered(NAME, args, true)
    }

    fn run<'a>(
        &'a self,
        approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let workspace = self.workspace.clone();
        let seen = self.seen.clone();
        let editing = crate::blocking::run(NAME, context, move |cancel| {
            edited(&workspace, &seen, &approved, cancel)
        });
        Box::pin(async move {
            // A call cancelled while it waited for room on the worker did
            // nothing, and answers as one cancelled at its first look does.
            let edited = editing
                .await?
                .unwrap_or_else(|| Err(ToolError::Cancelled(NAME.into())))?;
            Ok(self.seen.shown(edited))
        })
    }
}

/// The whole of a call's work, which is file work from its first step to its
/// last, and so is done where [`crate::blocking::run`] says: on the worker the
/// call was lent, or in place.
///
/// `cancel` is looked at between the reads of the file, before each change is
/// made in memory, and before the replacement is renamed into place, the one
/// step here whose effect outlives the process.
fn edited(
    workspace: &Workspace,
    seen: &Ledger,
    approved: &Approved,
    cancel: &Cancel,
) -> Result<Shown, ToolError> {
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
        )
        .into());
    }

    let path = match workspace.existing(requested) {
        Ok(path) => path,
        Err(problem) => return Ok(ToolOutput::failed(problem.to_string()).into()),
    };

    // The file the verdict was reached about, or nothing is read: a name
    // that leads elsewhere now is not the file anybody agreed to change.
    if let Err(problem) = target::held(workspace, approved, requested, &path) {
        return Ok(ToolOutput::failed(problem).into());
    }

    // Read through a descriptor-relative open. If the last component or a
    // directory above it became a link after resolution, the open refuses
    // it rather than bringing outside bytes into this transformation. The
    // commit below is likewise relative to the proven parent and renames
    // over a newly planted link rather than following it.
    let mut file = match path.open_regular_to_change() {
        Ok(file) => file,
        Err(problem) => return Ok(ToolOutput::failed(problem.to_string()).into()),
    };

    // Fixed-size reads put a cancellation point inside the scan and keep
    // retained source bytes below the declared whole-file ceiling. A large
    // sparse or minified input is therefore bounded both in memory and in
    // how long a stopped turn keeps reading it.
    let before = match source(&mut file, cancel) {
        Ok(Source::Text(before)) => before,
        Ok(Source::TooLarge) => return Ok(too_large(requested).into()),
        Ok(Source::Cancelled) => return Err(ToolError::Cancelled(NAME.into())),
        Ok(Source::Binary) => {
            return Ok(ToolOutput::failed(format!("{requested} is not a text file")).into());
        }
        Err(source) => {
            return Err(ToolError::Io {
                tool: NAME.into(),
                problem: format!("could not read {requested}").into(),
                source,
            });
        }
    };

    // What the edit is derived from, held against what the session last saw
    // of the file where it saw it, and carried to the commit either way.
    let read = Fingerprint::of(before.as_bytes());
    let held = seen.fingerprint(path.as_path());
    if held.is_some_and(|held| held != read) {
        return Ok(ToolOutput::failed(format!(
            "{requested} changed since it was read, so the edit was not made: read it again"
        ))
        .into());
    }

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
        if cancel.requested() {
            return Err(ToolError::Cancelled(NAME.into()));
        }

        let found = after.matches(change.find).count();
        if let Some(problem) = trouble(found, change.all, requested) {
            return Ok(refused(at, wanted.len(), &problem).into());
        }

        let made = if change.all { found } else { 1 };
        if grown(after.len(), made, change).is_none_or(|length| length > FILE_LIMIT) {
            return Ok(too_large(requested).into());
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
            tool: NAME.into(),
            problem: format!("could not inspect {requested}").into(),
            source,
        })?
        .permissions();
    if cancel.requested() {
        return Err(ToolError::Cancelled(NAME.into()));
    }
    // The replacement is prepared beside the old file, flushed, and
    // renamed only after it is whole. At no point can a reader observe the
    // empty or partially-written interval that truncating in place creates;
    // a change of identity or of content detected at the final pre-commit
    // check is refused as well.
    let expected = Some((&file, read));
    match atomic::replace(&path, after.as_bytes(), Some(permissions), expected) {
        Ok(()) => {}
        // Named as the model asked for it, as every other answer here is.
        Err(PathError::Changed { .. }) => {
            return Ok(ToolOutput::failed(format!(
                "{requested} changed while its replacement was prepared, so it was not replaced"
            ))
            .into());
        }
        Err(problem) => return Ok(ToolOutput::failed(problem.to_string()).into()),
    }

    let output = ToolOutput::ok(format!("changed {requested}, {replaced} replacements"))
        .showing(changed::between(&before, &after));

    // The file is now held to what this edit made, so the next change needs
    // no read first. That holds for a file the session never read too: what
    // is there now is content the session produced, though the agent was
    // shown only the text it quoted, and a later `write` may discard the
    // rest unseen. A change made to it after this is still refused.
    let file = Some((
        path.as_path().to_path_buf(),
        Fingerprint::of(after.as_bytes()),
    ));
    Ok(Shown { output, file })
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
    if each.len() > MOST_EDITS {
        return Err(args.wrong(format!(
            "edits lists {} changes, and at most {MOST_EDITS} fit one call",
            each.len()
        )));
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
