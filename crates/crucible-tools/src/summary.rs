//! What a call is about, in the words the transcript shows beside a tool's
//! name.
//!
//! One argument per tool, and which one is the only thing that differs between
//! them: `read` is about the path it opens, `grep` about the pattern it looks
//! for, `bash` about the command it runs. So the reading lives here once and
//! each tool says which field to read.
//!
//! Nothing is validated on the way through. What comes back is shown and never
//! acted on, and a call whose arguments do not hold up is refused a moment
//! later by the tool that owns them — this is the one reading in the crate that
//! is allowed to shrug, because the alternative is a transcript row that
//! reports an error the next row reports properly.

use crucible_core::{Remembered, Summary, ToolArgs};

use crate::args::Args;

/// The text `field` carries, or nothing where the call cannot be read.
///
/// Optional whatever the schema says, because the schema is not what arrived. A
/// required field the model left out is a call that will be refused, and it
/// reaches here first.
pub(crate) fn field(tool: &'static str, args: &ToolArgs, field: &str) -> Summary {
    let said = Args::parse(tool, args)
        .ok()
        .and_then(|args| args.optional_text(field).ok().flatten().map(str::to_owned));

    Summary::new(said.unwrap_or_default())
}

/// The path a file tool's call names, volunteered for a compaction to track.
///
/// Read off the same field [`field`] reads, for the same reason: the tool is the
/// only code that knows which argument is the path. `modified` is whether the
/// call changes the file rather than only opens it. A call whose arguments do
/// not hold up names nothing — it is refused a moment later, and a compaction
/// that tracked it would be remembering something that never happened.
pub(crate) fn remembered(
    tool: &'static str,
    args: &ToolArgs,
    modified: bool,
) -> Option<Remembered> {
    let path = Args::parse(tool, args)
        .ok()
        .and_then(|args| args.optional_text("path").ok().flatten().map(str::to_owned))?;

    if path.is_empty() {
        return None;
    }

    Some(if modified {
        Remembered::modified(path)
    } else {
        Remembered::read(path)
    })
}

#[cfg(test)]
mod tests {
    use crucible_core::Tool;

    use super::*;
    use crate::sample::Sample;
    use crate::{Bash, Edit, Glob, Grep, Ledger, Read, Write};

    #[test]
    fn each_tool_names_the_argument_its_call_is_about() {
        // One test rather than six, in the module the six share, because what
        // it pins is the map: `grep` answering with its path instead of its
        // pattern would be a row about the directory rather than about the
        // search, and only a list of them together says that is wrong.
        let sample = Sample::new("summary-fields");
        let workspace = sample.workspace();
        let seen = Ledger::new();

        let tools: [(Box<dyn Tool>, &str, &str, &str); 6] = [
            (
                Box::new(Read::new(workspace.clone(), seen.clone())),
                "read",
                r#"{"path":"src/main.rs"}"#,
                "src/main.rs",
            ),
            (
                Box::new(Write::new(workspace.clone(), seen)),
                "write",
                r#"{"path":"notes.md","content":"hello"}"#,
                "notes.md",
            ),
            (
                Box::new(Edit::new(workspace.clone())),
                "edit",
                r#"{"path":"src/lib.rs","find":"a","replace":"b"}"#,
                "src/lib.rs",
            ),
            (
                Box::new(Grep::new(workspace.clone())),
                "grep",
                r#"{"pattern":"fn main","path":"src"}"#,
                "fn main",
            ),
            (
                Box::new(Glob::new(workspace.clone())),
                "glob",
                r#"{"pattern":"**/*.rs","path":"src"}"#,
                "**/*.rs",
            ),
            (
                Box::new(Bash::new(workspace)),
                "bash",
                r#"{"command":"cargo test"}"#,
                "cargo test",
            ),
        ];

        for (tool, name, args, expected) in tools {
            assert_eq!(
                tool.summary(&ToolArgs::new(args)).as_str(),
                expected,
                "{name} summarised the wrong argument"
            );
        }
    }

    #[test]
    fn a_call_nobody_can_read_is_summarised_as_nothing() {
        // The tool refuses this call a moment later and says why. A row that
        // guessed at words for it would be the wrong explanation arriving
        // first.
        for unreadable in ["not json", "{}", r#"{"path":7}"#] {
            let said = field("read", &ToolArgs::new(unreadable), "path");
            assert!(said.is_empty(), "{unreadable} was summarised as something");
        }
    }
}
