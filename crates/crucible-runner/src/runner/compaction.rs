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
//! question the user is still waiting on with a stop. The one turn that does
//! end here is one somebody stopped, and it ends saying so.
//!
//! **The log is not touched.** Compaction rewrites what the model is sent; what
//! happened is what the session log holds, and it keeps every message this
//! drops. The two stop being the same thing here, and that is the point: one is
//! a working set and the other is the record.
//!
//! **A stop replaces nothing.** The transcript is rebuilt once the notes are
//! whole, so a recap somebody stopped part way through — or one a failed
//! request never finished — leaves the session exactly as it was. Half a
//! session's memory is not one, and standing it in place of the messages it
//! was meant to replace would lose the rest of them for good.

use crucible_core::{
    Cancel, Compacted, Compacting, Delta, Message, Post, Request, Room, StopReason, Transcript,
    TurnError,
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
/// with something nearly as large as the part it replaced.
const ROOM: u32 = 4_096;

/// How far a recap has run when the row that says so reads half done, in bytes.
///
/// The row cannot measure against the ceiling above, because a recap never
/// approaches it: notes come in at a fraction of the room they are given, so a
/// fraction of that room is a bar that crawls to a tenth of its length and then
/// vanishes — which is what a reader watching it called stuck.
///
/// Nor can it measure against the end, because nobody knows where the end is
/// until the model stops. So it measures against how far a recap usually gets:
/// `len / (len + HALFWAY)`, which moves from the first line, slows as the notes
/// run long, and never claims to have finished something still arriving. What
/// finishes it is the notes finishing, and the row goes with them.
const HALFWAY: u64 = 2_048;

impl Runner {
    /// Makes room, and says what it took.
    ///
    /// [`Room::Nothing`] where there was nothing worth compacting — a session
    /// too short to have a middle — and [`Room::Stopped`] where somebody
    /// stopped the recap while it was being written. The turn carries on after
    /// the first; what it must not do is loop, and a compaction that freed
    /// nothing is what the caller checks for.
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
    ) -> Result<Room, TurnError> {
        let Some(replacing) = self.replacing() else {
            return Ok(Room::Nothing);
        };

        let before = self.load.tokens();
        events.post(crucible_core::Event::Compacting { why, part: 0 });

        let Some(recap) = self.recap(why, events, cancel)? else {
            return Ok(Room::Stopped);
        };
        if recap.is_empty() {
            return Ok(Room::Nothing);
        }

        let mut messages = std::mem::take(&mut self.transcript).into_messages();

        // Drained rather than collected into a second transcript: this is the
        // one value here that grows with the session, and holding two of them
        // at once is what the peak-memory budget is set to refuse.
        let standing: Vec<Message> = messages.drain(replacing..).collect();
        drop(messages);

        let standing_as = format!("{}{recap}", crucible_core::RECAP);

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

        // Turns kept whole rather than messages, because that is the shape a
        // reader thinks in: the recap stands in for the front, and what is left
        // standing is the last few things they asked for.
        let kept = self.transcript.turns();

        let compacted = Compacted {
            why,
            replaced: replacing,
            before,
            after: self.load.tokens(),
            kept,
        };
        events.post(crucible_core::Event::Compacted { compacted });

        Ok(Room::Made(compacted))
    }

    /// How many messages from the front the recap stands in place of, or
    /// `None` where there is not enough behind to be worth replacing.
    ///
    /// Counted back from the end in **estimated tokens**, not in turns: a turn
    /// can be enormous, so a fixed number of them can be most of the window,
    /// and the kept tail is the thing that has to fit beside the recap. The
    /// current turn is always kept, whatever it has cost so far — cutting into
    /// it would drop work the model is in the middle of. Each turn before it is
    /// kept while the running byte estimate stays under the budget, and the cut
    /// lands on a user prompt, which is the boundary that never parts a call
    /// from the result answering it. That boundary is a rule, not a
    /// coincidence: a message kind added later that does not open a turn has to
    /// be counted into the turn it belongs to here, or the cut would land
    /// between a call and its answer.
    ///
    /// The estimate uses the model's own calibrated bytes-per-token where the
    /// load has one, and the pessimistic uncalibrated rate before any response
    /// has been seen — the same figure the load is measured by, so a full
    /// window is judged by the number that decided it was full.
    fn replacing(&self) -> Option<usize> {
        let budget = self.compacting.keep_tokens.max(1);
        let messages = self.transcript.messages();

        // The newest turn is kept whole. Starting one user prompt back from the
        // end puts the boundary before whatever the model is doing now, and a
        // session with a single turn has nothing behind it to replace.
        let mut marks = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| matches!(message, Message::User(_)))
            .map(|(at, _)| at);
        let mut cut = marks.next_back()?;

        // Walk the earlier turns newest-first, spending the budget on each.
        // `cut` only ever moves to a user prompt, so it is always a boundary.
        let mut tokens = 0_u64;
        for (at, message) in messages.iter().enumerate().rev() {
            if at >= cut {
                continue;
            }
            tokens = tokens.saturating_add(self.estimated(message));
            if matches!(message, Message::User(_)) {
                if tokens >= budget {
                    break;
                }
                cut = at;
            }
        }

        // Nothing to do where the budget swallowed everything behind the
        // current turn: compacting would spend a request to replace nothing.
        (cut > 0).then_some(cut)
    }

    /// What one message is estimated to cost the window, in tokens.
    ///
    /// Measured in bytes and converted at the load's own rate, so the tail is
    /// bounded in the unit the window is bounded in and at the rate the last
    /// response proved. Before any response has calibrated the rate, the
    /// pessimistic figure over-counts — which errs toward keeping less, the
    /// direction that costs context rather than the turn.
    fn estimated(&self, message: &Message) -> u64 {
        let bytes = match message {
            Message::User(said) => said.len(),
            Message::Agent { text, .. } => text.len(),
            Message::ToolResults(results) => results
                .iter()
                .map(|result| result.output.text().len())
                .sum::<usize>(),
        } as u64;

        self.load.bytes_to_tokens(bytes)
    }

    /// Asks the model to write down what is worth keeping.
    ///
    /// The instruction is pushed onto the transcript and taken off again rather
    /// than copied alongside it, because a copy of the transcript is the one
    /// allocation this crate may not make.
    ///
    /// `None` where the answer was stopped part way. What has arrived by then
    /// is notes that break off mid-sentence, and the caller may not stand them
    /// in place of anything: a stop is somebody saying leave the session alone.
    fn recap(
        &mut self,
        why: Compacting,
        events: &dyn Post,
        cancel: &Cancel,
    ) -> Result<Option<String>, TurnError> {
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

        // Counted from the text rather than from what the provider says it has
        // produced: one of them reports that only in its last chunk, so a bar
        // following it would sit at nothing for the whole request and then be
        // over — which is what a reader watching it called broken.
        let said = asked.map(|mut stream| {
            let mut said = String::new();
            let mut part = 0;
            let mut stopped = false;

            while let Some(delta) = stream.next() {
                match delta {
                    Ok(Delta::Text(text)) => {
                        said.push_str(&text);

                        // Posted only when the number it would draw has moved,
                        // which for a row redrawn on a beat is the difference
                        // between reporting a hundred times and a hundred
                        // thousand.
                        let now = reached(said.len() as u64);
                        if now != part {
                            part = now;
                            events.post(crucible_core::Event::Compacting { why, part });
                        }
                    }
                    Ok(Delta::Stopped(StopReason::Cancelled)) => {
                        stopped = true;
                        break;
                    }
                    Err(_) => break,
                    Ok(_) => {}
                }
            }
            (!stopped).then_some(said)
        });

        self.transcript.pop();
        Ok(said?)
    }
}

/// How far along the notes read, given how far they have run.
///
/// A curve rather than a ratio, for the reason [`HALFWAY`] gives: there is no
/// end to be a fraction of until the model has stopped, and the one number
/// available is how much has arrived.
fn reached(bytes: u64) -> u8 {
    let part = bytes.saturating_mul(100) / bytes.saturating_add(HALFWAY);

    u8::try_from(part).unwrap_or(99)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_moves_from_the_first_line_and_never_reads_finished_early() {
        // The two failures a reader sees: a bar that sits at nothing while the
        // notes are being written, and one that reads done while they are still
        // arriving.
        assert_eq!(reached(0), 0);
        assert!(reached(120) > 0, "nothing had moved after a line of notes");
        assert!(reached(HALFWAY * 8) < 100, "it read finished before it was");

        // And it only ever goes one way.
        let mut before = 0;
        for bytes in (0..16_384).step_by(97) {
            let now = reached(bytes);
            assert!(now >= before, "{bytes}: {now} after {before}");
            before = now;
        }
    }

    #[test]
    fn a_recap_the_size_recaps_actually_come_in_at_fills_most_of_it() {
        // Measured against real ones: what these have to stay clear of is the
        // low corner, where every recap ever written reads as barely started.
        assert!(reached(1_447) > 25, "{}", reached(1_447));
        assert!(reached(5_766) > 60, "{}", reached(5_766));
    }
}
