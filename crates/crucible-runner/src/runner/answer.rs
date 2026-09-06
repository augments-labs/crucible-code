//! One model answer, put back together from the deltas that carried it.
//!
//! Every provider splits an answer differently — prose in fragments, a tool
//! name announced before its arguments, arguments in pieces of JSON that are
//! not valid JSON on their own — and the reassembly is the same whichever way
//! it was split. So it happens once, here, rather than in each provider.

use std::fmt;

use crucible_core::{
    Continuation, ProviderContinuation, ProviderError, ProviderLimit, StopReason, ToolArgs,
    ToolCall, ToolId,
};

const MAX_RESPONSE_TEXT: usize = 8 * 1024 * 1024;
const MAX_TOOL_ARGUMENTS: usize = 1024 * 1024;
const MAX_TOOL_CALL_ID: usize = 16 * 1024;
const MAX_TOOL_CALL_NAME: usize = 4 * 1024;
const MAX_TOOL_CALL_METADATA: usize = 256 * 1024;
const MAX_TOOL_CALLS: usize = 128;

/// An answer still arriving.
pub(crate) struct Answer {
    /// Named so a protocol failure can say which provider produced it.
    provider: &'static str,
    text: String,
    calls: Vec<Building>,
    stop: Option<StopReason>,
    argument_bytes: usize,
    metadata_bytes: usize,
    retained_bytes: usize,
    turn_held: usize,
    turn_maximum: usize,
    continuation: Option<Continuation>,
    finalized: Option<ProviderContinuation>,
    progress: bool,
}

/// One tool call, still arriving.
struct Building {
    id: ToolId,
    name: Box<str>,
    args: String,
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answer")
            .field("provider", &self.provider)
            .field("text", &"[redacted]")
            .field("calls", &self.calls.len())
            .field("stop", &self.stop)
            .field("retained_bytes", &self.retained_bytes)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Building {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Building")
            .field("id", &"[redacted]")
            .field("name", &"[redacted]")
            .field("args", &"[redacted]")
            .finish()
    }
}

impl Answer {
    /// Nothing said yet.
    #[cfg(test)]
    pub(crate) fn new(provider: &'static str) -> Self {
        Self::within(provider, 0, usize::MAX)
    }

    /// Nothing said yet, with the room this response has left in its turn.
    pub(crate) fn within(provider: &'static str, turn_held: usize, turn_maximum: usize) -> Self {
        Self {
            provider,
            text: String::new(),
            calls: Vec::new(),
            stop: None,
            argument_bytes: 0,
            metadata_bytes: 0,
            retained_bytes: 0,
            turn_held,
            turn_maximum,
            continuation: None,
            finalized: None,
            progress: false,
        }
    }

    /// Takes prose.
    pub(crate) fn say(&mut self, text: &str) -> Result<(), ProviderError> {
        self.open()?;
        self.room(
            self.text.len(),
            text.len(),
            MAX_RESPONSE_TEXT,
            ProviderLimit::Text,
        )?;
        self.turn_room(text.len())?;
        self.text.push_str(text);
        self.retained_bytes += text.len();
        Ok(())
    }

    /// Takes the start of a tool call. Everything after it belongs to this one
    /// until the next start.
    pub(crate) fn calling(&mut self, id: ToolId, name: Box<str>) -> Result<(), ProviderError> {
        self.open()?;
        self.room(
            0,
            id.as_str().len(),
            MAX_TOOL_CALL_ID,
            ProviderLimit::ToolCallId,
        )?;
        self.room(
            0,
            name.len(),
            MAX_TOOL_CALL_NAME,
            ProviderLimit::ToolCallName,
        )?;
        let incoming = id.as_str().len().saturating_add(name.len());
        self.room(
            self.metadata_bytes,
            incoming,
            MAX_TOOL_CALL_METADATA,
            ProviderLimit::ToolCallMetadata,
        )?;
        self.room(
            self.calls.len(),
            1,
            MAX_TOOL_CALLS,
            ProviderLimit::ToolCalls,
        )?;
        self.turn_room(incoming)?;
        self.calls.push(Building {
            id,
            name,
            args: String::new(),
        });
        self.metadata_bytes += incoming;
        self.retained_bytes += incoming;
        Ok(())
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
        self.open()?;
        self.room(
            self.argument_bytes,
            fragment.len(),
            MAX_TOOL_ARGUMENTS,
            ProviderLimit::ToolArguments,
        )?;
        self.turn_room(fragment.len())?;
        let Some(building) = self.calls.last_mut() else {
            return Err(ProviderError::Protocol {
                provider: self.provider,
                problem: "tool arguments arrived before the call they belong to".into(),
            });
        };

        building.args.push_str(fragment);
        self.argument_bytes += fragment.len();
        self.retained_bytes += fragment.len();
        Ok(())
    }

    /// Admits private response state within the remaining turn and history room.
    pub(crate) fn continuing(
        &mut self,
        state: Continuation,
        room: usize,
    ) -> Result<(), ProviderError> {
        self.open()?;
        if self.continuation.is_some() {
            return Err(self.invalid("duplicate provider continuation"));
        }
        let bytes = state.retained_bytes();
        if bytes > room {
            return Err(self.invalid("provider continuation exceeds the retained-history limit"));
        }
        self.turn_room(bytes)?;
        self.retained_bytes += bytes;
        self.progress = true;
        self.continuation = Some(state);
        Ok(())
    }

    /// Private progress changes retry safety, never token or visible-text counts.
    pub(crate) fn progressed(&mut self) -> Result<(), ProviderError> {
        self.open()?;
        self.progress = true;
        Ok(())
    }

    pub(crate) const fn has_progress(&self) -> bool {
        self.progress
    }

    fn invalid(&self, problem: &'static str) -> ProviderError {
        ProviderError::Protocol {
            provider: self.provider,
            problem: problem.into(),
        }
    }

    /// Called only after the entire stream ended without error or cancellation.
    pub(crate) fn finalize(&mut self) -> Result<StopReason, ProviderError> {
        let stop = self.reached()?;
        if let Some(state) = self.continuation.take()
            && matches!(stop, StopReason::Yielded | StopReason::WantsTools)
        {
            self.finalized = Some(
                state
                    .finish(&self.text, self.calls.len(), Some(stop))
                    .map_err(|_| {
                        self.invalid("provider continuation does not match the completed answer")
                    })?,
            );
        }
        Ok(stop)
    }

    /// Never available on a failed response, even after an individually valid part.
    pub(crate) fn take_continuation(&mut self) -> Option<ProviderContinuation> {
        self.finalized.take()
    }

    /// Takes the reason the model stopped.
    pub(crate) fn stopped(&mut self, stop: StopReason) -> Result<(), ProviderError> {
        self.open()?;
        self.stop = Some(stop);
        Ok(())
    }

    fn open(&self) -> Result<(), ProviderError> {
        if self.stop.is_some() {
            return Err(ProviderError::Protocol {
                provider: self.provider,
                problem: "a delta arrived after the response stopped".into(),
            });
        }
        Ok(())
    }

    fn room(
        &self,
        held: usize,
        incoming: usize,
        maximum: usize,
        limit: ProviderLimit,
    ) -> Result<(), ProviderError> {
        if incoming > maximum.saturating_sub(held) {
            return Err(ProviderError::Limit {
                provider: self.provider,
                limit,
                maximum,
            });
        }
        Ok(())
    }

    fn turn_room(&self, incoming: usize) -> Result<(), ProviderError> {
        self.room(
            self.turn_held.saturating_add(self.retained_bytes),
            incoming,
            self.turn_maximum,
            ProviderLimit::TurnResponseBytes,
        )
    }

    /// Provider-controlled bytes this response retained.
    pub(crate) const fn retained(&self) -> usize {
        self.retained_bytes
    }

    /// Why the model stopped, or `None` if it never said.
    ///
    /// What goes on the transcript, including where the answer broke off: the
    /// message has to say the turn did not reach an ending, and `None` is how
    /// it says so.
    pub(crate) fn stop(&self) -> Option<StopReason> {
        self.stop
    }

    /// Why the model stopped, as a turn cannot go on without.
    ///
    /// A stream that ends having said nothing is a truncated answer with
    /// nothing to mark it as one — see [`crucible_core::DeltaStream::next`],
    /// which forbids it. Both providers here prevent it and prove it; a third
    /// that forgot would produce half an answer that reads as a whole one, and
    /// this is what stops that being silent.
    pub(crate) fn reached(&self) -> Result<StopReason, ProviderError> {
        self.stop.ok_or_else(|| ProviderError::Protocol {
            provider: self.provider,
            problem: "the response ended without saying why the model stopped".into(),
        })
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

        answer.say("Hello").unwrap();
        answer.say(", ").unwrap();
        answer.say("world").unwrap();

        let (text, _) = answer.finish();
        assert_eq!(&*text, "Hello, world");
    }

    #[test]
    fn a_tool_call_is_assembled_from_its_name_and_its_fragments() {
        // The fragments are not valid JSON on their own, which is the whole
        // reason they are joined before anything tries to read them.
        let mut answer = answer();

        answer
            .calling(ToolId::new("call_1"), "read".into())
            .unwrap();
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

        answer.calling(ToolId::new("a"), "read".into()).unwrap();
        answer.arguments("{\"path\":\"one\"}").unwrap();
        answer.calling(ToolId::new("b"), "read".into()).unwrap();
        answer.arguments("{\"path\":\"two\"}").unwrap();

        let (_, calls) = answer.finish();
        let args: Vec<&str> = calls.iter().map(|call| call.args.as_str()).collect();

        assert_eq!(args, [r#"{"path":"one"}"#, r#"{"path":"two"}"#]);
    }

    #[test]
    fn a_tool_call_with_no_arguments_at_all_still_becomes_a_call() {
        let mut answer = answer();

        answer.calling(ToolId::new("a"), "pwd".into()).unwrap();

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

        answer.stopped(StopReason::WantsTools).unwrap();
        assert_eq!(answer.stop(), Some(StopReason::WantsTools));
    }

    #[test]
    fn an_answer_that_never_stopped_is_not_carried_on_as_a_finished_one() {
        // A stream that ends having said nothing looks exactly like one that
        // finished. Guessed at, the guess is "finished", and the user is handed
        // half an answer with nothing saying it is half.
        let mut answer = answer();

        let problem = answer.reached().unwrap_err();
        assert!(
            matches!(problem, ProviderError::Protocol { provider, .. } if provider == "test"),
            "expected a protocol failure naming the provider, got {problem:?}"
        );

        answer.stopped(StopReason::Yielded).unwrap();
        assert_eq!(answer.reached().unwrap(), StopReason::Yielded);
    }

    #[test]
    fn prose_and_a_call_in_the_same_answer_are_both_kept() {
        // The model saying what it is about to do, then doing it.
        let mut answer = answer();

        answer.say("let me look").unwrap();
        answer.calling(ToolId::new("a"), "read".into()).unwrap();
        answer.arguments("{}").unwrap();

        let (text, calls) = answer.finish();

        assert_eq!(&*text, "let me look");
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn text_is_refused_before_it_can_grow_past_its_bound() {
        let mut answer = answer();
        answer.say(&"x".repeat(MAX_RESPONSE_TEXT)).unwrap();

        let problem = answer.say("x").unwrap_err();

        assert!(matches!(
            problem,
            ProviderError::Limit {
                limit: ProviderLimit::Text,
                maximum: MAX_RESPONSE_TEXT,
                ..
            }
        ));
    }

    #[test]
    fn response_bytes_are_bounded_across_the_turn_before_append() {
        let mut answer = Answer::within("test", 8, 10);
        answer.say("xx").unwrap();

        let problem = answer.say("x").unwrap_err();

        assert!(matches!(
            problem,
            ProviderError::Limit {
                limit: ProviderLimit::TurnResponseBytes,
                maximum: 10,
                ..
            }
        ));
        assert_eq!(answer.retained(), 2, "the refused byte was not retained");
    }

    #[test]
    fn tool_arguments_are_bounded_across_all_calls_in_the_response() {
        let mut answer = answer();
        answer.calling(ToolId::new("a"), "write".into()).unwrap();
        answer.arguments(&"x".repeat(MAX_TOOL_ARGUMENTS)).unwrap();
        answer.calling(ToolId::new("b"), "write".into()).unwrap();

        let problem = answer.arguments("x").unwrap_err();

        assert!(matches!(
            problem,
            ProviderError::Limit {
                limit: ProviderLimit::ToolArguments,
                maximum: MAX_TOOL_ARGUMENTS,
                ..
            }
        ));
    }

    #[test]
    fn tool_call_count_and_each_metadata_field_are_bounded() {
        let mut answer = answer();
        for number in 0..MAX_TOOL_CALLS {
            answer
                .calling(ToolId::new(number.to_string()), "read".into())
                .unwrap();
        }
        assert!(matches!(
            answer.calling(ToolId::new("last"), "read".into()),
            Err(ProviderError::Limit {
                limit: ProviderLimit::ToolCalls,
                ..
            })
        ));

        let mut fresh = Answer::new("test");
        assert!(matches!(
            fresh.calling(ToolId::new("x".repeat(MAX_TOOL_CALL_ID + 1)), "read".into()),
            Err(ProviderError::Limit {
                limit: ProviderLimit::ToolCallId,
                ..
            })
        ));
        assert!(matches!(
            fresh.calling(ToolId::new("a"), "x".repeat(MAX_TOOL_CALL_NAME + 1).into()),
            Err(ProviderError::Limit {
                limit: ProviderLimit::ToolCallName,
                ..
            })
        ));

        let mut metadata = Answer::new("test");
        for _ in 0..12 {
            metadata
                .calling(
                    ToolId::new("i".repeat(MAX_TOOL_CALL_ID)),
                    "n".repeat(MAX_TOOL_CALL_NAME).into(),
                )
                .unwrap();
        }
        assert!(matches!(
            metadata.calling(
                ToolId::new("i".repeat(MAX_TOOL_CALL_ID)),
                "n".repeat(MAX_TOOL_CALL_NAME).into()
            ),
            Err(ProviderError::Limit {
                limit: ProviderLimit::ToolCallMetadata,
                ..
            })
        ));
    }

    #[test]
    fn stop_is_terminal_for_every_later_delta() {
        let mut answer = answer();
        answer.stopped(StopReason::Yielded).unwrap();

        assert!(matches!(
            answer.say("late"),
            Err(ProviderError::Protocol { .. })
        ));
        assert!(matches!(
            answer.calling(ToolId::new("a"), "read".into()),
            Err(ProviderError::Protocol { .. })
        ));
        assert!(matches!(
            answer.arguments("{}"),
            Err(ProviderError::Protocol { .. })
        ));
        assert!(matches!(
            answer.stopped(StopReason::Yielded),
            Err(ProviderError::Protocol { .. })
        ));
    }

    #[test]
    fn debug_output_never_contains_response_content() {
        let mut answer = answer();
        answer.say("answer-canary").unwrap();
        answer
            .calling(ToolId::new("id-canary"), "name-canary".into())
            .unwrap();
        answer.arguments("args-canary").unwrap();

        let shown = format!("{answer:?}");

        for canary in ["answer-canary", "id-canary", "name-canary", "args-canary"] {
            assert!(!shown.contains(canary), "Debug leaked {canary}: {shown}");
        }
    }
}
