//! Putting a whole file down.
//!
//! Whatever was at the name is gone afterwards, which makes this the one call
//! here that can destroy work. So it refuses a file nobody has looked at: the
//! agent may replace what it has read and what it wrote itself, and has to go
//! and read anything else first. The refusal is a result rather than an error,
//! so the turn continues and the model can do exactly that.

use std::fs;
use std::io::Read as _;

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
use crate::ledger::{Ledger, Shown};
use crate::schema::{Field, Schema, Shape};
use crate::summary;
use crate::target;

/// The name the model calls.
const NAME: &str = "write";

/// The file to write.
const PATH: &str = "path";

/// What to put in it.
const CONTENT: &str = "content";

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
    let mut fields = vec![
        Field {
            name: PATH,
            about: "The file to write, relative to the workspace root.".into(),
            needed: true,
            shape: Shape::Text,
        },
        Field {
            name: CONTENT,
            about: "The complete new contents of the file.".into(),
            needed: true,
            shape: Shape::Text,
        },
    ];
    fields.extend(crate::account::fields(
        "path",
        "What the file is becoming",
        "Where a file is being replaced, account for what is in it now.",
    ));
    Schema {
        about: "Writes a file in the workspace, replacing it if it is already there. Creates \
                missing parent directories on Unix; on Windows the parent directory must already \
                exist."
            .into(),
        fields,
    }
    .text()
});

/// Writes a file inside the workspace.
#[derive(Debug)]
pub struct Write {
    workspace: Workspace,
    seen: Ledger,
}

impl Write {
    /// Writes inside `workspace`, and nowhere else, replacing only files `seen`
    /// says have been looked at.
    #[must_use]
    pub fn new(workspace: Workspace, seen: Ledger) -> Self {
        Self { workspace, seen }
    }
}

impl DescribeTool for Write {
    fn name(&self) -> &str {
        NAME
    }

    fn schema(&self) -> &str {
        SCHEMA.as_str()
    }
}

impl Tool for Write {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        let args = Args::parse(NAME, args)?;
        args.text(PATH)?;
        args.exact(CONTENT).map(drop)
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile {
            target: target::creatable(&self.workspace, NAME, args, PATH),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, PATH)
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
        let putting = crate::blocking::run(NAME, context, move |cancel| {
            put(&workspace, &seen, &approved, cancel)
        });
        Box::pin(async move {
            // A call cancelled while it waited for room on the worker did
            // nothing, and answers as one cancelled at its first look does.
            let put = putting
                .await?
                .unwrap_or_else(|| Err(ToolError::Cancelled(NAME.into())))?;
            Ok(self.seen.shown(put))
        })
    }
}

/// The whole of a call's work, which is file work from its first step to its
/// last, and so is done where [`crate::blocking::run`] says: on the worker the
/// call was lent, or in place.
///
/// `cancel` is looked at before each step whose effect outlives the process:
/// before each directory is made and before the replacement is renamed into
/// place. A call stopped anywhere before the rename leaves the file as it
/// was, never half-written or half-renamed. The directories it had already
/// made for the file by then stay where they are, and the call answers only
/// that it was cancelled.
fn put(
    workspace: &Workspace,
    seen: &Ledger,
    approved: &Approved,
    cancel: &Cancel,
) -> Result<Shown, ToolError> {
    let args = Args::parse(NAME, approved.args())?;
    let requested = args.text(PATH)?;
    let content = args.exact(CONTENT)?;

    // The parent has to exist before the path can be contained, because
    // containment is decided on a resolved path and only a directory that
    // is really there can be resolved. So the directories are made first,
    // through a parent that has itself been checked.
    if let Some(problem) = prepare(workspace, requested, cancel)? {
        return Ok(problem.into());
    }

    let path = match workspace.creatable(requested) {
        Ok(path) => path,
        Err(problem) => return Ok(ToolOutput::failed(problem.to_string()).into()),
    };

    // What is at the name now, asked about the name itself rather than
    // through it: `creatable` proved the last component was not a symbolic
    // link, so one there now arrived since, and `symlink_metadata` is the
    // question that sees it rather than the far end. All this decides is
    // which of the two opens the write is — both of them refuse a name that
    // has become a link, so nothing rests on getting it right.
    let already = fs::symlink_metadata(&path);
    if already.as_ref().is_ok_and(fs::Metadata::is_dir) {
        return Ok(ToolOutput::failed(format!("{requested} is a directory")).into());
    }

    let replaced = already.is_ok();

    // Asked before anything is opened, because the answer is about what is
    // already there rather than about the write. A file the agent has not
    // read is one it cannot know it is discarding — including one another
    // program wrote a moment ago, which is the case a model has no way at
    // all to see.
    if replaced && !seen.holds(path.as_path()) {
        return Ok(ToolOutput::failed(format!(
            "{requested} has not been read, so replacing it would discard what is in it: read it first"
        ))
        .into());
    }

    let mut original = if replaced {
        let file = match path.open_regular_to_change() {
            Ok(file) => file,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string()).into()),
        };
        Some(file)
    } else {
        None
    };
    let permissions = original
        .as_ref()
        .map(|file| file.metadata().map(|metadata| metadata.permissions()))
        .transpose()
        .map_err(|source| ToolError::Io {
            tool: NAME.into(),
            problem: format!("could not inspect {requested}").into(),
            source,
        })?;

    // What is about to go, read back so that whoever is watching can see
    // what went. Nothing else will ever hold both versions: the model is
    // sent a line count, and by the time anything downstream reads that,
    // the old file is gone.
    let before = if replaced {
        original.as_mut().and_then(discarded)
    } else {
        Some(String::new())
    };

    // The last look before the one step that cannot be taken back.
    heeded(cancel)?;

    // Prepared beside the destination and flushed before the namespace
    // changes atomically. Unix also flushes the directory; Windows flushes
    // the renamed file because its handle-relative rename has no
    // write-through form. A failure before commit leaves the old file
    // whole, and a file whose identity changed before the final pre-commit
    // check is refused rather than overwritten.
    if let Err(problem) = atomic::replace(&path, content.as_bytes(), permissions, original.as_ref())
    {
        return Ok(ToolOutput::failed(problem.to_string()).into());
    }

    let lines = content.lines().count();
    let what = if replaced { "replaced" } else { "created" };
    let answer = ToolOutput::ok(format!("{what} {requested}, {lines} lines"));

    // No block rather than a wrong one. A file that could not be read back
    // is not one that was empty, and a diff drawn from an empty string would
    // say every line here is new when the truth is that nobody can say.
    let output = match before {
        Some(before) => answer.showing(changed::between(&before, content)),
        None => answer,
    };

    // What the agent just put down it has by definition seen, so the next
    // call may replace it. Without this a file has to be created and then
    // read back before it can be corrected, which is a round trip spent
    // learning what the same turn wrote.
    Ok(Shown {
        output,
        file: Some(path.as_path().to_path_buf()),
    })
}

/// Whether the call should stop before its next step, answered as the error
/// it then ends with.
///
/// A job cannot be stopped from outside once it has started, and one whose
/// call was dropped runs on until it next looks at its token, so this is
/// asked before each step whose effect outlives the process.
fn heeded(cancel: &Cancel) -> Result<(), ToolError> {
    if cancel.requested() {
        return Err(ToolError::Cancelled(NAME.into()));
    }
    Ok(())
}

/// The most of a file being replaced that is read back to show what went.
///
/// The figure is the one `edit` holds a whole-file transformation to, because
/// this is that transformation with the finding step left out: both versions
/// have to be in memory at once for either to be worked out.
const DISCARDED: usize = 1_000_000;

/// What was at the name, where it is small enough to hold and is text.
///
/// `None` where it is neither, which is why the caller carries an `Option` all
/// the way to the answer rather than an empty string. A block is an extra and
/// not a promise — the call's job is putting the new file down, and it does that
/// whether or not the old one could be read — but an extra that guesses is worse
/// than one that is absent.
fn discarded(file: &mut fs::File) -> Option<String> {
    let mut bytes = Vec::new();
    file.take(DISCARDED as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > DISCARDED {
        return None;
    }

    String::from_utf8(bytes).ok()
}

/// Makes the directories the path needs, one contained level at a time.
///
/// Returns the failure the model should see, if there is one, and
/// [`ToolError::Cancelled`] where `cancel` was raised before a directory that
/// was about to be made. Walking down rather than calling `create_dir_all` on
/// the whole path is what keeps every level inside the workspace. The file
/// itself is contained either way, by the check that follows this; what one
/// call would leave behind is the *directories* — `../stray/one.txt` is
/// refused at the end, after `stray` has already been made outside the tree.
///
/// Creation itself refuses when anything is already at the leaf, a symbolic
/// link included. A link planted there between the check and creation is
/// therefore an error rather than a path redirection.
fn prepare(
    workspace: &Workspace,
    requested: &str,
    cancel: &Cancel,
) -> Result<Option<ToolOutput>, ToolError> {
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
        if let Ok(_) | Err(PathError::Escapes { .. }) = workspace.existing(step) {
            continue;
        }

        let at = match workspace.creatable(step) {
            Ok(at) => at,
            Err(problem) => return Ok(Some(ToolOutput::failed(problem.to_string()))),
        };

        heeded(cancel)?;
        if let Err(problem) = at.create_directory() {
            return Ok(Some(ToolOutput::failed(problem.to_string())));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests;
