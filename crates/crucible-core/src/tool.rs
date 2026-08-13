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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The provider's identifier, used to match the result back to the call.
    pub id: ToolId,
    /// Which tool.
    pub name: Box<str>,
    /// The arguments, still as the model wrote them.
    pub args: ToolArgs,
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
    /// Shown in full. Arguments are the agent's reasoning made visible and are
    /// what a user is deciding about at the permission prompt; a redacted one
    /// would make that decision impossible.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ToolArgs({})", self.0)
    }
}

/// What a tool produced, on its way back to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    text: Box<str>,
    failed: bool,
}

impl ToolOutput {
    /// A successful result.
    #[must_use]
    pub fn ok(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: false,
        }
    }

    /// A result the model should treat as a failure it can react to — a
    /// missing file, a non-zero exit status, a pattern that matched nothing.
    #[must_use]
    pub fn failed(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: true,
        }
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
    fn argument_debug_shows_the_arguments() {
        // The permission prompt shows these. Redacting them would ask the user
        // to approve something they cannot see.
        let args = ToolArgs::new(r#"{"command":"rm -rf /"}"#);
        assert!(format!("{args:?}").contains("rm -rf /"));
    }

    #[test]
    fn an_unknown_tool_names_the_tool_the_model_asked_for() {
        let err = ToolError::Unknown("frobnicate".into());
        assert_eq!(err.to_string(), "no tool named frobnicate");
    }
}
