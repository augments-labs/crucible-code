//! The call a model makes and the arguments it wrote.
//!
//! A call is data on its way between a provider and the tool that answers it.
//! Neither end owns it, so it lives here rather than beside either, and it
//! carries the argument text unparsed: validation belongs to the tool whose
//! schema describes it.

use std::fmt;

use crate::ids::ToolId;

/// The most bytes a provider-visible tool name may retain.
///
/// Equal to the existing inbound call-name boundary. A descriptor the provider
/// can be shown but whose returned name the runner would refuse is not a usable
/// descriptor.
pub const TOOL_NAME_BYTES: usize = 4 * 1024;

/// The most bytes retained for one provider tool-call identifier.
pub const TOOL_CALL_ID_BYTES: usize = 16 * 1024;

/// The most bytes retained for one tool call's argument text.
pub const TOOL_ARGUMENT_BYTES: usize = 1024 * 1024;
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
/// Deliberately not a parsed value: argument validation and interpretation
/// belong to the tool whose schema describes them, not to the execution core.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ToolId;

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
}
