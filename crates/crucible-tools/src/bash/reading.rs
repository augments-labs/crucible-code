//! Asking a command that is still running what it has printed so far.
//!
//! The registry has held this answer since it was built — it is what the panel
//! behind <kbd>Ctrl</kbd>+<kbd>O</kbd> stands whole. The reader could reach it
//! and the model could not, and the gap between those two is where a model
//! goes looking for another way: running a second command to ask the question
//! the first one is already answering.
//!
//! So this is the affordance behind the sentence a backgrounded command comes
//! back with. That sentence asks the model not to poll, and the ending it
//! promises arrives only when the command ends — which for a dev server, a
//! watcher or a `--follow` is never. This is the other half: the answer, on
//! demand, for as long as the command is running.
//!
//! It touches no file and starts no process. What it reads is a value inside
//! this process, so there is no path a rule could be written about and nobody
//! is asked — the same permission story `todo_write` has, for the same reason.

use std::sync::LazyLock;

use crucible_core::{
    Approved, DescribeTool, Sensitivity, Summary, Target, Tool, ToolArgs, ToolContext, ToolError,
    ToolOutput,
};

use super::background::Background;
use crate::args::Args;
use crate::schema::{Field, Schema, Shape, Whole};

/// The name the model calls.
const NAME: &str = "bash_output";

/// The field the command's number arrives under.
const NUMBER: &str = "number";

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Answers with what a command left running has printed so far. Use it instead of \
                running something else to find out how one is going: the number is the one the \
                call that left it running was answered with. A command that has already ended \
                is not here — what it printed arrives on its own when it ends."
            .into(),
        fields: vec![Field {
            name: NUMBER,
            about: "The number the command is running as, as the call that left it running \
                    answered with."
                .into(),
            needed: true,
            shape: Shape::Count(Whole {
                least: 1,
                most: None,
            }),
        }],
    }
    .text()
});

/// Reads what a command left running has printed.
#[derive(Debug)]
pub struct BashOutput {
    left: Background,
}

impl BashOutput {
    /// Reads from `left`, the registry the tool that starts them keeps.
    #[must_use]
    pub fn new(left: Background) -> Self {
        Self { left }
    }
}

impl DescribeTool for BashOutput {
    fn name(&self) -> &str {
        NAME
    }

    fn schema(&self) -> &str {
        SCHEMA.as_str()
    }
}

impl Tool for BashOutput {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        let args = Args::parse(NAME, args)?;
        asked(&args).map(drop)
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        // Not a file and not a process. What this reads is a value inside this
        // process, and a target that resolves to nothing is the honest answer
        // to what a rule could be written about.
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        // The number, because it is the whole of the call and the row is
        // otherwise four identical words whichever command was asked about.
        Args::parse(NAME, args)
            .and_then(|args| asked(&args))
            .map_or_else(
                |_| Summary::new(""),
                |number| Summary::new(format!("#{number}")),
            )
    }

    fn run(&self, approved: Approved, _context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let number = asked(&args)?;

        let Some(printed) = self.left.wrote(number) else {
            return Ok(ToolOutput::failed(missing(number, &self.left)));
        };

        // Both of a command's streams are kept to the retained ceiling apiece,
        // so what the registry hands over can be twice what one answer may
        // carry. Cut here, where it becomes an answer.
        let printed = super::output::excerpt(&printed, super::output::CAPTURE_TEXT);
        if printed.trim().is_empty() {
            return Ok(ToolOutput::ok(format!(
                "[#{number} is running and has printed nothing yet]"
            )));
        }

        Ok(ToolOutput::ok(printed))
    }
}

/// What the model is told when nothing is running as the number it asked about.
///
/// Naming what is running rather than only refusing, because the ordinary way
/// to arrive here is a number that was right a moment ago: a command that ended
/// leaves the registry, and its output arrives on its own in the note about the
/// ending. A refusal that said only "no" would send the model looking for
/// another way to ask — which is the thing this tool exists to make unnecessary.
fn missing(number: usize, left: &Background) -> String {
    let running = left.running();
    if running.is_empty() {
        return format!(
            "nothing is running as #{number}, and nothing is left running at all. A command that \
             has ended is not here; what it printed arrives on its own when it ends."
        );
    }

    let names = running
        .iter()
        .map(|one| format!("#{} {}", one.number, one.called))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "nothing is running as #{number}. Left running: {names}. A command that has ended is not \
         here; what it printed arrives on its own when it ends."
    )
}

/// The number a call asked about.
///
/// # Errors
///
/// [`ToolError::Arguments`] where the call did not name one.
fn asked(args: &Args) -> Result<usize, ToolError> {
    if !args.holds(NUMBER) {
        return Err(args.wrong(format!("{NUMBER} is required")));
    }

    // The default cannot be reached past the line above, and `count` refuses a
    // zero, so nothing here can answer with a number no command ever runs as.
    args.count(NUMBER, 0)
}

#[cfg(test)]
mod tests;
