//! One model answer, put back together from the deltas that carried it.
//!
//! Every provider splits an answer differently — prose in fragments, a tool
//! name announced before its arguments, arguments in pieces of JSON that are
//! not valid JSON on their own — and the reassembly is the same whichever way
//! it was split. So it happens once, here, rather than in each provider.

use crucible_core::{ProviderError, StopReason, ToolArgs, ToolCall, ToolId};

/// An answer still arriving.
#[derive(Debug)]
pub(crate) struct Answer {
    /// Named so a protocol failure can say which provider produced it.
    provider: &'static str,
    text: String,
    calls: Vec<Building>,
    stop: Option<StopReason>,
}

/// One tool call, still arriving.
#[derive(Debug)]
struct Building {
    id: ToolId,
    name: Box<str>,
    args: String,
}

impl Answer {
    /// Nothing said yet.
    pub(crate) fn new(provider: &'static str) -> Self {
        Self {
            provider,
            text: String::new(),
            calls: Vec::new(),
            stop: None,
        }
    }

    /// Takes prose.
    pub(crate) fn say(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Takes the start of a tool call. Everything after it belongs to this one
    /// until the next start.
    pub(crate) fn calling(&mut self, id: ToolId, name: Box<str>) {
        self.calls.push(Building {
            id,
            name,
            args: String::new(),
        });
    }

    /// Takes a fragment of the current call's arguments.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when there is no call for the fragment to
    /// belong to. Dropping it instead would send the model's own call back to
    /// it with arguments missing, and the tool would report a problem the
    /// model has no way to fix.
    pub(crate) fn arguments(&mut self, fragment: &str) -> Result<(), ProviderError> {
        let Some(building) = self.calls.last_mut() else {
            return Err(ProviderError::Protocol {
                provider: self.provider,
                problem: "tool arguments arrived before the call they belong to".into(),
            });
        };

        building.args.push_str(fragment);
        Ok(())
    }

    /// Takes the reason the model stopped.
    pub(crate) fn stopped(&mut self, stop: StopReason) {
        self.stop = Some(stop);
    }

    /// Why the model stopped, or `None` if it never said.
    pub(crate) fn stop(&self) -> Option<StopReason> {
        self.stop
    }

    /// What it said, and what it asked to run.
    pub(crate) fn finish(self) -> (Box<str>, Vec<ToolCall>) {
        let calls = self
            .calls
            .into_iter()
            .map(|building| ToolCall {
                id: building.id,
                name: building.name,
                args: ToolArgs::new(building.args),
            })
            .collect();

        (self.text.into(), calls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Answer {
        Answer::new("test")
    }

    #[test]
    fn text_arriving_in_fragments_becomes_one_message() {
        let mut answer = answer();

        answer.say("Hello");
        answer.say(", ");
        answer.say("world");

        let (text, _) = answer.finish();
        assert_eq!(&*text, "Hello, world");
    }

    #[test]
    fn a_tool_call_is_assembled_from_its_name_and_its_fragments() {
        // The fragments are not valid JSON on their own, which is the whole
        // reason they are joined before anything tries to read them.
        let mut answer = answer();

        answer.calling(ToolId::new("call_1"), "read".into());
        answer.arguments("{\"path\":").unwrap();
        answer.arguments("\"src/main.rs\"}").unwrap();

        let (_, calls) = answer.finish();

        assert_eq!(
            calls,
            vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"src/main.rs"}"#),
            }]
        );
    }

    #[test]
    fn two_calls_keep_their_own_arguments() {
        let mut answer = answer();

        answer.calling(ToolId::new("a"), "read".into());
        answer.arguments("{\"path\":\"one\"}").unwrap();
        answer.calling(ToolId::new("b"), "read".into());
        answer.arguments("{\"path\":\"two\"}").unwrap();

        let (_, calls) = answer.finish();
        let args: Vec<&str> = calls.iter().map(|call| call.args.as_str()).collect();

        assert_eq!(args, [r#"{"path":"one"}"#, r#"{"path":"two"}"#]);
    }

    #[test]
    fn a_tool_call_with_no_arguments_at_all_still_becomes_a_call() {
        let mut answer = answer();

        answer.calling(ToolId::new("a"), "pwd".into());

        let (_, calls) = answer.finish();
        assert_eq!(calls.first().map(|call| call.args.as_str()), Some(""));
    }

    #[test]
    fn arguments_with_no_call_to_belong_to_are_a_protocol_failure() {
        let mut answer = answer();

        let problem = answer.arguments("{\"path\":").unwrap_err();

        assert!(
            matches!(problem, ProviderError::Protocol { provider, .. } if provider == "test"),
            "expected a protocol failure naming the provider, got {problem:?}"
        );
    }

    #[test]
    fn an_answer_that_never_stopped_says_so() {
        // The runner decides what to do about it; the assembly does not guess.
        let mut answer = answer();
        assert_eq!(answer.stop(), None);

        answer.stopped(StopReason::WantsTools);
        assert_eq!(answer.stop(), Some(StopReason::WantsTools));
    }

    #[test]
    fn prose_and_a_call_in_the_same_answer_are_both_kept() {
        // The model saying what it is about to do, then doing it.
        let mut answer = answer();

        answer.say("let me look");
        answer.calling(ToolId::new("a"), "read".into());
        answer.arguments("{}").unwrap();

        let (text, calls) = answer.finish();

        assert_eq!(&*text, "let me look");
        assert_eq!(calls.len(), 1);
    }
}
