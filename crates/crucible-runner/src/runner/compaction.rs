//! Making room, without ending the turn.
//!
//! One request, asked of the same model, whose answer replaces the middle of
//! the transcript. What is left standing is a recap and the last few turns
//! word for word — enough for the model to know what it was doing, at a
//! fraction of what it was carrying.
//!
//! **The turn does not end.** This runs inside the loop, between one request
//! and the next, and the loop carries on afterwards against a transcript that
//! now fits. A session that ended its turn to make room would answer the
//! question the user is still waiting on with a stop.
//!
//! **The log is not touched.** Compaction rewrites what the model is sent; what
//! happened is what the session log holds, and it keeps every message this
//! drops. The two stop being the same thing here, and that is the point: one is
//! a working set and the other is the record.

use crucible_core::{
    Cancel, Compacted, Compacting, Delta, Message, Post, Request, StopReason, Transcript, TurnError,
};

use super::Runner;

/// What the model is asked for, in place of the next turn.
///
/// Written as an instruction to carry on rather than as a request for prose: an
/// answer that reads as a report about a session is one the model then has to
/// re-read as its own memory. What this asks for is the memory itself.
const RECAP: &str = "\
Before anything else, write down what is worth keeping from everything above, \
for yourself, so that you can carry on with it and nothing else. Include what \
was asked for, what has been decided, what has been changed and where, what \
failed and why, and what you were about to do next. Leave out anything you \
would not need again. Write it as notes to yourself, not as a report to \
somebody else, and write nothing before or after it.";

/// The most a recap may be, in tokens.
///
/// Notes rather than an essay, and far under what an answer may be: a recap
/// that ran to the model's whole answer ceiling would be replacing a session
/// with something nearly as large as the part it replaced. It is also what the
/// row saying so measures against, which is the other reason it is a figure
/// this program chooses rather than the model's own.
const ROOM: u32 = 4_096;

/// Roughly how many bytes a token comes to.
///
/// Only ever used to turn a length into a fraction for the row that says room
/// is being made. Nothing decided on it, and deliberately low — a bar that
/// reaches its end a moment early is better than one that stalls short of it.
const BYTES: u64 = 3;

/// What a recap is marked with where it stands in a transcript.
///
/// The model reads its own notes back under a heading that says whose they are
/// and why they are short — without it, a recap reads as something the user
/// said, which is the one voice it must not be mistaken for.
const STANDS: &str = "[everything before this was compacted to make room; \
these are your own notes on it]\n\n";

impl Runner {
    /// Makes room, and says what it took.
    ///
    /// `Ok(None)` where there was nothing worth compacting — a session too
    /// short to have a middle. The turn carries on either way; what it must not
    /// do is loop, and a compaction that freed nothing is what the caller
    /// checks for.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where the request for the recap failed. The transcript is
    /// untouched in that case: it is replaced once the answer is whole, so a
    /// failure part way through leaves the session exactly as it was.
    pub fn compact(
        &mut self,
        why: Compacting,
        events: &dyn Post,
        cancel: &Cancel,
    ) -> Result<Option<Compacted>, TurnError> {
        let kept = self.keeping();
        let Some(replacing) = self.replacing(kept) else {
            return Ok(None);
        };

        let before = self.load.tokens();
        events.post(crucible_core::Event::Compacting { why, part: 0 });

        let recap = self.recap(why, events, cancel)?;
        if recap.is_empty() {
            return Ok(None);
        }

        let tail = self.transcript.len() - replacing;
        let mut messages = std::mem::take(&mut self.transcript).into_messages();

        // Drained rather than collected into a second transcript: this is the
        // one value here that grows with the session, and holding two of them
        // at once is what the peak-memory budget is set to refuse.
        let standing: Vec<Message> = messages.drain(replacing..).collect();
        drop(messages);

        let standing_as = format!("{STANDS}{recap}");

        // Written to the log before the transcript is replaced, so a crash
        // between the two leaves a log that says what happened rather than one
        // that quietly lost the messages.
        self.session.compacted(replacing, &standing_as);

        let mut rebuilt = Transcript::new();
        rebuilt.push(Message::User(standing_as.into()));
        for message in standing {
            rebuilt.push(message);
        }
        self.transcript = rebuilt;

        self.load.replaced();
        for message in self.transcript.messages() {
            self.load.recorded(message);
        }

        let compacted = Compacted {
            why,
            replaced: replacing,
            before,
            after: self.load.tokens(),
            kept: tail,
        };
        events.post(crucible_core::Event::Compacted { compacted });

        Ok(Some(compacted))
    }

    /// How many turns are kept word for word after the recap.
    fn keeping(&self) -> usize {
        self.compacting.keep.max(1)
    }

    /// How many messages from the front the recap stands in place of, or
    /// `None` where there is not enough behind to be worth replacing.
    ///
    /// Counted back from the end by user prompts, because a turn begins at one:
    /// keeping a whole number of turns is what stops a recap landing between a
    /// call and the result that answers it, which no provider will accept.
    fn replacing(&self, keep: usize) -> Option<usize> {
        let starts: Vec<usize> = self
            .transcript
            .messages()
            .iter()
            .enumerate()
            .filter(|(_, message)| matches!(message, Message::User(_)))
            .map(|(at, _)| at)
            .collect();

        // Nothing to do until there is at least one whole turn behind the ones
        // being kept. Compacting a session that is all tail would spend a
        // request to replace nothing.
        let at = starts.len().checked_sub(keep)?;
        let front = *starts.get(at)?;

        (front > 0).then_some(front)
    }

    /// Asks the model to write down what is worth keeping.
    ///
    /// The instruction is pushed onto the transcript and taken off again rather
    /// than copied alongside it, because a copy of the transcript is the one
    /// allocation this crate may not make.
    fn recap(
        &mut self,
        why: Compacting,
        events: &dyn Post,
        cancel: &Cancel,
    ) -> Result<String, TurnError> {
        self.transcript.push(Message::User(RECAP.into()));

        let asked = self.provider.stream(
            Request {
                model: &self.model.name,
                transcript: &self.transcript,
                tools: &[],
                max_tokens: ROOM.min(self.model.max_tokens),
                system: None,
                effort: self.model.effort,
            },
            cancel,
        );

        // The bytes the notes would run to if they filled the room they were
        // given. Counted from the text rather than from what the provider says
        // it has produced: one of them reports that only in its last chunk, so
        // a bar following it would sit at nothing for the whole request and
        // then be over — which is what a reader watching it called broken.
        let full = u64::from(ROOM.min(self.model.max_tokens)).saturating_mul(BYTES) | 1;
        let said = asked.map(|mut stream| {
            let mut said = String::new();
            let mut part = 0;

            while let Some(delta) = stream.next() {
                match delta {
                    Ok(Delta::Text(text)) => {
                        said.push_str(&text);

                        // Posted only when the number it would draw has moved,
                        // which for a row redrawn on a beat is the difference
                        // between reporting a hundred times and a hundred
                        // thousand.
                        let now =
                            u8::try_from((said.len() as u64 * 100 / full).min(100)).unwrap_or(100);
                        if now != part {
                            part = now;
                            events.post(crucible_core::Event::Compacting { why, part });
                        }
                    }
                    Ok(Delta::Stopped(StopReason::Cancelled)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            said
        });

        self.transcript.pop();
        Ok(said?)
    }
}
