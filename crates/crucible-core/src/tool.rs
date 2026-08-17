//! What a tool is, from the runner's side.
//!
//! Tools are an open set: adding one must not edit this crate. The runner
//! dispatches over `dyn Tool` and never names `read`, `grep` or `bash`.
//!
//! Arguments arrive from the model as JSON text and stay text until the tool
//! that owns them parses them. That keeps core free of every tool's argument
//! shape, and it means an argument is validated exactly once, by the code that
//! knows what it means.

use std::fmt;

use crate::diff::Diff;
use crate::ids::ToolId;
use crate::permission::{Approved, Sensitivity};

/// Why a tool call did not produce a result.
///
/// A tool that ran and decided the answer is "no such file" returns a failed
/// [`ToolOutput`] instead — that is a result the model should see and act on,
/// not a breakdown of the mechanism.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The model asked for a tool that is not registered.
    #[error("no tool named {0}")]
    Unknown(Box<str>),

    /// The arguments were not the shape this tool takes.
    #[error("{tool}: {problem}")]
    Arguments {
        /// Which tool rejected them.
        tool: &'static str,
        /// What was wrong, in words the model can act on.
        problem: Box<str>,
    },

    /// The operating system refused.
    #[error("{tool}: {problem}")]
    Io {
        /// Which tool was running.
        tool: &'static str,
        /// What failed, without the underlying path if it is sensitive.
        problem: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// The user cancelled while the tool was running.
    #[error("{0} cancelled")]
    Cancelled(&'static str),
}

/// The model asking to run a tool.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The provider's identifier, used to match the result back to the call.
    pub id: ToolId,
    /// Which tool.
    pub name: Box<str>,
    /// The arguments, still as the model wrote them.
    pub args: ToolArgs,
}

impl fmt::Debug for ToolCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolCall")
            .field("id", &"[redacted]")
            .field("name", &"[redacted]")
            .field("args", &"[redacted]")
            .finish()
    }
}

/// Tool arguments as JSON text.
///
/// Deliberately not a parsed value: core has no JSON dependency and no opinion
/// about any tool's schema.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolArgs(Box<str>);

impl ToolArgs {
    /// Takes the argument text a provider streamed.
    #[must_use]
    pub fn new(json: impl Into<Box<str>>) -> Self {
        Self(json.into())
    }

    /// The JSON text, for the owning tool to parse.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ToolArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ToolArgs([redacted])")
    }
}

/// What a call is about, in the words the transcript shows beside the tool's
/// name.
///
/// A type of its own rather than a `String`, for the reason [`ToolArgs`] is
/// one: it is made out of a call's arguments, and a `bash` call's arguments are
/// a command line somebody may have typed a token into. Redacting the arguments
/// and then carrying a copy of part of them under another name would be no
/// redaction at all.
#[derive(Clone, PartialEq, Eq)]
pub struct Summary(Box<str>);

impl Summary {
    /// Takes the words a tool worked out from its own arguments.
    #[must_use]
    pub fn new(said: impl Into<Box<str>>) -> Self {
        Self(said.into())
    }

    /// The words, for whatever is drawing the row.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the call said nothing that could be summarised.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Summary([redacted])")
    }
}

/// What the model said this call is for, on its way to whoever decides it.
///
/// The panel a call waits behind is drawn by a thread with no provider behind
/// it: it sends the question and blocks on the answer, so it cannot go back and
/// ask what the command was for. What the model says about a call therefore
/// arrives *with* the call, as an argument the schema invites and the tool
/// itself never reads.
///
/// Empty is the ordinary case and not a failure. A tool whose schema invites no
/// account, and a call that declined to give one, both come through here, and
/// the panel they get is the panel there was before this existed.
///
/// A [`Summary`] is the neighbouring type and answers a different question:
/// that one is *what* the call is, taken out of the arguments the tool acts on,
/// and it goes in the transcript. This is what the model says *about* the call,
/// in its own words, and it is shown only while somebody is deciding.
#[derive(Clone, PartialEq, Eq)]
pub struct Account(Box<str>);

impl Account {
    /// Takes the one line a call gave about itself.
    #[must_use]
    pub fn new(description: impl Into<Box<str>>) -> Self {
        Self(description.into())
    }

    /// A call that said nothing about itself.
    #[must_use]
    pub fn none() -> Self {
        Self::new("")
    }

    /// The line, for whatever is drawing the question.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Account {
    /// Redacted for the reason [`Summary`] is: this is the model's prose about
    /// arguments this crate refuses to show, and prose about a command line
    /// quotes it as often as not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Account([redacted])")
    }
}

/// What a tool produced, on its way back to the model.
///
/// And on its way to the reader, which is not the same journey. The text is
/// what both are shown; a [`Diff`] is what only the reader is, and it comes off
/// at [`ToolOutput::forget_diff`] where the two copies part company.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolOutput {
    text: Box<str>,
    failed: bool,
    diff: Option<Diff>,
}

impl fmt::Debug for ToolOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolOutput")
            .field("text", &"[redacted]")
            .field("failed", &self.failed)
            .field("diff", &self.diff)
            .finish()
    }
}

impl ToolOutput {
    /// A successful result.
    #[must_use]
    pub fn ok(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: false,
            diff: None,
        }
    }

    /// A result the model should treat as a failure it can react to — a
    /// missing file, a non-zero exit status, a pattern that matched nothing.
    #[must_use]
    pub fn failed(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: true,
            diff: None,
        }
    }

    /// The same result, with the change it made for the reader to look at.
    ///
    /// What a tool that rewrote a file adds on its way out. It is the one thing
    /// here the model is not sent, so it carries no meaning the text does not
    /// also carry — a result whose words depended on it would say different
    /// things to the two readers of the same call.
    #[must_use]
    pub fn showing(mut self, diff: Diff) -> Self {
        self.diff = Some(diff);
        self
    }

    /// The text the model sees.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the provider should mark this result as an error.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// What the call changed, where it changed a file and said so.
    #[must_use]
    pub fn diff(&self) -> Option<&Diff> {
        self.diff.as_ref()
    }

    /// Drops it, for the copy that is kept rather than drawn.
    ///
    /// A diff is drawn once and a transcript is replayed every turn for the
    /// rest of the session, so the copy going into one keeps only what the
    /// model was told. Otherwise the transcript would grow with what had been
    /// *shown*, where what bounds it is what was *said*.
    pub fn forget_diff(&mut self) {
        self.diff = None;
    }
}

/// One tool the agent can call.
pub trait Tool: Send + Sync {
    /// The name the model uses. Must match the `name` in [`Tool::schema`].
    fn name(&self) -> &'static str;

    /// The JSON Schema for this tool's arguments, as sent to the provider.
    fn schema(&self) -> &'static str;

    /// How dangerous this particular call is.
    ///
    /// Takes the arguments because it is not a property of the tool: `bash`
    /// running `ls` and `bash` running `rm -rf` are the same tool.
    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity;

    /// What this call is about — the [`Summary`] a transcript row shows.
    ///
    /// A different question from [`Tool::sensitivity`], asked of the same
    /// arguments: that one answers what is at risk, this one answers what the
    /// reader is looking at. `grep` is where the two come apart — what is at
    /// risk is the directory about to be walked, and what the call is about is
    /// the pattern.
    ///
    /// Owned by the tool for the reason the arguments are text everywhere else
    /// in this crate: the tool is the only code that knows which field carries
    /// the answer. Read anywhere else, that field name would be a second reading
    /// of a schema this trait keeps opaque on purpose, and the two would drift
    /// apart the first time one of them was renamed.
    ///
    /// Empty where the arguments cannot be read at all: that call is refused by
    /// [`Tool::run`] a moment later, and words invented for it would describe
    /// something that never happened.
    fn summary(&self, args: &ToolArgs) -> Summary;

    /// Runs the call.
    ///
    /// An [`Approved`] cannot be constructed outside the permission engine, so
    /// a call site that has not obtained a verdict cannot reach this function.
    /// It carries the tool and the arguments as well as the proof, which is
    /// what makes the arguments a tool runs on *the* arguments a verdict was
    /// reached about, and this tool *the* tool it was reached about — a
    /// separate `args` parameter, and a handle found beside the call, both
    /// left that to the caller's care.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when the call could not be carried out at all. A result
    /// the model should see, including a failure, comes back as a failed
    /// [`ToolOutput`].
    fn run(&self, approved: Approved) -> Result<ToolOutput, ToolError>;
}

impl fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tool({})", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Change, Line};

    #[test]
    fn output_carries_whether_the_model_should_treat_it_as_a_failure() {
        assert!(!ToolOutput::ok("done").is_failed());
        assert!(ToolOutput::failed("no such file").is_failed());
        assert_eq!(ToolOutput::failed("no such file").text(), "no such file");
    }

    #[test]
    fn arguments_are_kept_as_written() {
        let args = ToolArgs::new(r#"{"path":"src/main.rs"}"#);
        assert_eq!(args.as_str(), r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn argument_debug_never_shows_the_arguments() {
        let args = ToolArgs::new(r#"{"token":"debug-canary"}"#);
        let shown = format!("{args:?}");
        assert!(!shown.contains("debug-canary"), "{shown}");
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn call_debug_never_shows_provider_output() {
        let call = ToolCall {
            id: ToolId::new("id-debug-canary"),
            name: "name-debug-canary".into(),
            args: ToolArgs::new(r#"{"token":"args-debug-canary"}"#),
        };
        let shown = format!("{call:?}");
        for canary in ["id-debug-canary", "name-debug-canary", "args-debug-canary"] {
            assert!(!shown.contains(canary), "{shown}");
        }
    }

    #[test]
    fn output_debug_never_shows_workspace_content() {
        let output = ToolOutput::ok("output-debug-canary").showing(Diff::new([Line::new(
            1,
            Change::Added,
            "diff-debug-canary",
        )]));
        let shown = format!("{output:?}");
        for canary in ["output-debug-canary", "diff-debug-canary"] {
            assert!(!shown.contains(canary), "{shown}");
        }
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn a_result_shown_a_diff_hands_it_over_until_it_is_asked_to_forget_it() {
        // The two ends of the one journey that is not shared. Everything else on
        // a result goes to the model and the reader alike; this goes to the
        // reader, and the same value carries it as far as the split and no
        // further.
        let mut output = ToolOutput::ok("changed 1, 1 replacements")
            .showing(Diff::new([Line::new(315, Change::Added, "budgets:")]));

        assert_eq!(output.diff().map(Diff::added), Some(1));

        output.forget_diff();

        assert!(output.diff().is_none());
        // And what the model was told is untouched, because it never depended on
        // the diff in the first place.
        assert_eq!(output.text(), "changed 1, 1 replacements");
    }

    #[test]
    fn a_result_that_changed_nothing_carries_no_diff_to_begin_with() {
        assert!(ToolOutput::ok("done").diff().is_none());
        assert!(ToolOutput::failed("no such file").diff().is_none());
    }

    #[test]
    fn an_unknown_tool_names_the_tool_the_model_asked_for() {
        let err = ToolError::Unknown("frobnicate".into());
        assert_eq!(err.to_string(), "no tool named frobnicate");
    }
}
