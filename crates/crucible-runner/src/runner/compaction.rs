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
//! **The log is the record.** Compaction rewrites what the model is sent; what
//! happened is what the session log holds, and it keeps every message this
//! drops. The two stop being the same thing here, and that is the point: one is
//! a working set and the other is the record. Pruning is the same split — an
//! old tool result is cleared from what the model is sent and kept whole in the
//! log, with a line written where it happened so a continued session clears it
//! again.
//!
//! **A stop replaces nothing.** The transcript is rebuilt once the notes are
//! whole, so a recap somebody stopped part way through — or one a failed
//! request never finished — leaves the session exactly as it was. Half a
//! session's memory is not one, and standing it in place of the messages it
//! was meant to replace would lose the rest of them for good.

use std::fmt::Write as _;

use crucible_core::{
    Compacted, Compacting, Delta, Message, Request, Room, Spend, StopReason, ToolId, ToolOutput,
    Transcript, TurnError,
};

use crate::context::RunContext;

use super::{Load, Runner};

/// How much recent tool output is never cleared, in bytes.
///
/// The newest results are the ones the model is still working from, and a turn
/// that lost the output it just asked for would be flying blind. Beyond this a
/// result is old enough that the recap — which says what it found — is the
/// better thing to keep.
const PROTECT: u64 = 60_000;

/// The least a pruning has to recover to be worth recording, in bytes.
///
/// Under it the placeholders and the log line cost more of the record than the
/// clearing saved, and a transcript that was mostly prose gains nothing.
/// Pruning is for the sessions where tool output is the bulk; where it is not,
/// the recap alone is the answer.
const MINIMUM: u64 = 30_000;

/// What the model is asked for, in place of the next turn.
///
/// Written as an instruction to carry on rather than as a request for prose: an
/// answer that reads as a report about a session is one the model then has to
/// re-read as its own memory. What this asks for is the memory itself.
///
/// The shape is fixed rather than free-form because a fixed one is harder to
/// drop a category from: left to its own wording, a recap quietly omits the one
/// decision that mattered. Each heading is a thing the next turn cannot do
/// without. The files are not the model's to recall — they are collected from
/// the calls being replaced and appended by code after validation, so the list
/// survives a second compaction instead of going out with the first recap.
const RECAP: &str = "\
Before anything else, create a structured context checkpoint from everything \
above so another model pass can continue the work. Use exactly every heading \
and subheading below, in this order. Keep each section concise. Write `(none)` \
where a section has nothing to say rather than omitting it.\n\n\
## Goal\n\
## Constraints & Preferences\n\
## Progress\n\
### Done\n\
### In Progress\n\
### Blocked\n\
## Decisions\n\
## Next Steps\n\
## Critical Context\n\n\
Preserve exact file paths, function and type names, commands, error messages, \
requirements, decisions and unfinished state. Write operational notes for \
yourself, not a report to the user. End after the content of \
`## Critical Context`; the exact `Files so far` list is appended by the program. \
Output nothing before `## Goal` or after that final section.";

/// The line the tracked files stand under in a recap.
///
/// Read back on replay to pull the list a previous recap carried into the next
/// one, so the record of which files a session touched survives being compacted
/// twice. The log line is the only copy — the runner keeps no state of its own
/// across compactions, and the recap it already wrote is the record.
const FILES: &str = "Files so far:";

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

/// The most recap text retained from one stream, in bytes.
///
/// The recap request is the one response of a turn that does not pass through
/// the loop's own bounds, so it holds this ceiling itself — the same figure a
/// regular response's visible text is held to. A recap this large is not notes,
/// and past it the stream is provider-controlled growth with nothing to show
/// for it.
const MAX_RECAP_TEXT: usize = 8 * 1024 * 1024;

/// What the standalone recap request produced.
enum Recap {
    Complete(String),
    Stopped,
    Incomplete,
}

impl Runner {
    /// Makes room, and says what it took.
    ///
    /// [`Room::Nothing`] where there was nothing worth compacting — a session
    /// too short to have a middle — and [`Room::Stopped`] where somebody
    /// stopped the recap while it was being written. The turn carries on after
    /// the first; what it must not do is loop, and a compaction that freed
    /// nothing is what the caller checks for.
    ///
    /// What the recap request itself produces joins `spent`, because the recap
    /// is a response of the turn like any other: a caller with no turn running
    /// hands in a reading of its own and reads what the request cost off it.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where the request for the recap failed. The transcript is
    /// untouched in that case: it is replaced once the answer is whole, so a
    /// failure part way through leaves the session exactly as it was.
    pub fn compact(
        &mut self,
        why: Compacting,
        run: &RunContext<'_>,
        spent: &mut Spend,
    ) -> Result<Room, TurnError> {
        // Held to this session's policy for the reason [`Runner::turn`] is:
        // the recap boundary below is read off the run, and a run asking for
        // more than the session allows does not get it.
        let run = &run.held_to(self.policy);

        let events = run.reporting();

        // Choose the recap boundary before pruning. Clearing output can make
        // old turns look cheap enough to keep, but where a recap was possible
        // before it remains useful afterwards; only the previously fruitless
        // no-middle case should turn into prune-only progress.
        let replacing = self.replacing(run.policy().compaction.keep_tokens);

        // Measured before anything moves, because this is what the compaction
        // is judged against: pruning can be all the room a current turn needs,
        // and a `before` taken after it would read that progress as none.
        let before = self.load.tokens();

        // The lightest touch first. Tool results can fill the current turn by
        // themselves, and that is exactly where there is no older middle for a
        // recap to replace. Prune before giving up on finding one so those
        // results do not become untouchable merely because the turn is active.
        self.prune();

        let replacing = if let Some(replacing) = replacing {
            replacing
        } else {
            let after = self.load.tokens();
            if after < before {
                let compacted = Compacted {
                    why,
                    replaced: 0,
                    before,
                    after,
                    kept: self.transcript.turns(),
                };
                events.post(crucible_core::Event::Compacted { compacted });
                events.post(crucible_core::Event::Carried {
                    left: self.left_under(run.policy().compaction),
                });
                return Ok(Room::Made(compacted));
            }

            // A turn runs compaction only between passes, after every call has
            // its result. If that complete active turn is all that is left and
            // still does not fit, recap it too. Keeping it whole forever is the
            // dead end that used to report NoRoom even though the session log
            // still held everything being replaced.
            let completed_pass = self
                .transcript
                .messages()
                .iter()
                .rev()
                .take_while(|message| !matches!(message, Message::User { .. }))
                .any(|message| matches!(message, Message::ToolResults(_)));
            match (why, completed_pass) {
                (Compacting::Full | Compacting::Refused, true) => self.transcript.len(),
                (Compacting::Asked | Compacting::Resumed, _)
                | (Compacting::Full | Compacting::Refused, false) => {
                    return Ok(Room::Nothing);
                }
            }
        };

        // The files the replaced span touched, and the files every recap before
        // it already carried. Collected before anything is recorded, while the
        // whole transcript is still there to walk: the model is told to carry
        // this list forward, so a second compaction extends it rather than
        // losing what the first one kept.
        let touched = self.tracked(replacing);
        events.post(crucible_core::Event::Compacting { why, part: 0 });

        let recap = match self.recap(why, &touched, run, spent)? {
            Recap::Complete(recap) => recap,
            Recap::Incomplete => return Err(TurnError::RecapIncomplete),
            Recap::Stopped => return Ok(Room::Stopped),
        };

        // Completion is a fact only once a structured recap is whole. It goes
        // immediately before Compacted below, preserving event order without
        // sleeping the worker; the renderer gives it a short visible dwell.
        events.post(crucible_core::Event::Compacting { why, part: 100 });

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
        rebuilt.push(Message::said(standing_as));
        for message in standing {
            rebuilt.push(message);
        }
        self.transcript = rebuilt;

        self.load.replaced();
        for message in self.transcript.messages() {
            self.load.recounted(message);
        }
        self.load
            .requesting(self.spec.instructions(), &self.tools.advertised());

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
        events.post(crucible_core::Event::Carried {
            left: self.left_under(run.policy().compaction),
        });

        Ok(Room::Made(compacted))
    }

    /// How many messages from the front the recap stands in place of, or
    /// `None` where there is not enough behind to be worth replacing.
    ///
    /// Counted back from the end in **estimated tokens**, not in turns: a turn
    /// can be enormous, so a fixed number of them can be most of the window,
    /// and the kept tail is the thing that has to fit beside the recap. The
    /// current turn is always kept by this ordinary boundary. Automatic
    /// compaction has one fallback above it: after pruning has failed and no
    /// older middle remains, a complete active turn may be recapped too. That
    /// happens only between passes, where no tool call is in flight.
    ///
    /// Earlier turns are kept while the running byte estimate stays under the
    /// budget, and every ordinary cut lands on a user prompt. That boundary is a rule,
    /// not a coincidence: a message kind added later that does not open a turn
    /// has to be counted into the turn it belongs to here, or the cut would land
    /// between a call and its answer.
    ///
    /// The estimate uses the model's own calibrated bytes-per-token where the
    /// load has one, and the pessimistic uncalibrated rate before any response
    /// has been seen — the same figure the load is measured by, so a full
    /// window is judged by the number that decided it was full.
    fn replacing(&self, keep_tokens: u64) -> Option<usize> {
        let budget = keep_tokens.max(1);
        let messages = self.transcript.messages();

        // The newest turn is kept whole. Starting one user prompt back from the
        // end puts the ordinary boundary before whatever the model is doing now,
        // and a session with a single turn has no older middle to replace.
        let mut marks = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| matches!(message, Message::User { .. }))
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
            if matches!(message, Message::User { .. }) {
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
            Message::User { text: said, .. } => said.len(),
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
    /// The files to carry into the recap, read and modified apart.
    ///
    /// Two sources, in the order they happened: the lists every previous recap
    /// already stands over, then what the calls in the span about to be
    /// replaced volunteer through [`Tool::remember`]. A file in both collapses
    /// to one mention, and a modified one stays modified — a file that was
    /// changed is one a later turn may need the state of, however many times it
    /// was only read afterwards. The runner keeps no list of its own across
    /// compactions; the recaps already written are the record, and this reads
    /// them back rather than hold a second copy that could drift from it.
    fn tracked(&self, replacing: usize) -> (Vec<String>, Vec<String>) {
        let mut files = Files::default();

        for message in self.transcript.messages() {
            // A recap from a compaction before this one. The span being
            // replaced starts after the latest of them, but the files it kept
            // are still this session's to remember.
            if let Message::User { text: said, .. } = message
                && let Some(recap) = said.strip_prefix(crucible_core::RECAP)
            {
                for (path, changed) in listed(recap) {
                    files.note(path, changed);
                }
            }
        }

        // The calls in the span being replaced. Only an agent message carries
        // them, and only a tool that has a file to name answers `remember` —
        // the rest return `None` and are nobody's to track. `take` rather than a
        // slice, because `replacing` was just decided from this length and a
        // panic on the way back through it is a bug, not a bound to re-check.
        for message in self.transcript.messages().iter().take(replacing) {
            if let Message::Agent { calls, .. } = message {
                for call in calls {
                    if let Some(tool) = self.tools.find(&call.name)
                        && let Some(file) = tool.remember(&call.args)
                    {
                        files.note(file.path(), file.is_modified());
                    }
                }
            }
        }

        (files.read, files.modified)
    }

    fn recap(
        &mut self,
        why: Compacting,
        touched: &(Vec<String>, Vec<String>),
        run: &RunContext<'_>,
        spent: &mut Spend,
    ) -> Result<Recap, TurnError> {
        let events = run.reporting();
        let cancel = run.cancel();
        let asking = RECAP.to_owned();
        let asking_bytes = asking.len() as u64;
        self.transcript.push(Message::said(asking));

        // The recap is a standalone request: no ordinary system prompt or tool
        // schemas, and its output has to fit beside the request itself. This is
        // a bound on what may be produced, not a promise that a recap is long.
        // `tokens` may include ordinary system/tool overhead that this request
        // omits. Keeping it is the conservative direction; adding only the new
        // instruction avoids estimating the existing transcript a second time.
        let request_tokens = self
            .load
            .tokens()
            .saturating_add(Load::cautious(asking_bytes));
        let safe = self.spec.model.window.map_or(u32::MAX, |window| {
            u32::try_from(u64::from(window).saturating_sub(request_tokens)).unwrap_or(u32::MAX)
        });
        let room = run
            .policy()
            .compaction
            .recap_tokens
            .min(self.spec.model.max_tokens)
            .min(safe);
        if room == 0 {
            self.transcript.pop();
            return Ok(Recap::Incomplete);
        }

        let asked = self.provider.stream(
            Request {
                model: &self.spec.model.name,
                transcript: &self.transcript,
                tools: &[],
                max_tokens: room,
                system: None,
                effort: self.spec.model.effort,
                // Nothing, deliberately. This request exists to turn a
                // transcript into a recap, and a recap is text; re-sending
                // megabytes of pictures to write one would spend the whole
                // ceiling on the code path that fires when the window is
                // already full. What the recap can say about a file is what
                // the prompt naming it said.
                attached: &[],
            },
            cancel,
        );

        // Counted from the text rather than from what the provider says it has
        // produced: one of them reports that only in its last chunk, so a bar
        // following it would sit at nothing for the whole request and then be
        // over — which is what a reader watching it called broken.
        let said = asked.and_then(|mut stream| {
            let mut said = String::new();
            let mut part = 0;
            let mut stopped = None;

            // What the turn had spent before this request opened. A provider's
            // readings are this response's total so far, so each lands on this
            // fixed figure rather than on the reading before it — the same
            // arithmetic every other response of the turn is counted by.
            let before = *spent;

            while let Some(delta) = stream.next() {
                match delta {
                    Ok(Delta::Text(text)) => {
                        said.push_str(&text);
                        if said.len() > MAX_RECAP_TEXT {
                            return Ok(Recap::Incomplete);
                        }

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
                    // The recap is a response of the turn like any other, and
                    // what it produces counts: a reading that skipped it would
                    // hold the ceiling and the row both under what the turn
                    // actually ran to.
                    Ok(Delta::Spent(reading)) => {
                        *spent = before.and(reading);
                        events.post(crucible_core::Event::Spent { spend: *spent });
                    }
                    // No tools are advertised, so no call can arrive; and what
                    // this request carried measures a transcript that is about
                    // to be replaced, so there is nothing standing for the
                    // reading to correct.
                    Ok(Delta::ToolStarted { .. } | Delta::ToolArgs(_) | Delta::Carried(_)) => {}
                    Ok(Delta::Stopped(reason)) => {
                        stopped = Some(reason);
                        break;
                    }
                    // The provider's own failure, handed on as itself: a
                    // recap whose connection broke did not produce a recap
                    // that fell short, and calling it one would hide the only
                    // fact the caller can act on.
                    Err(problem) => return Err(problem),
                }
            }
            Ok(match stopped {
                Some(StopReason::Yielded) if structured(&said) => {
                    append_files(&mut said, touched);
                    Recap::Complete(said)
                }
                Some(StopReason::Cancelled) => Recap::Stopped,
                Some(
                    StopReason::Yielded
                    | StopReason::OutOfTokens
                    | StopReason::WindowExceeded
                    | StopReason::Filtered
                    | StopReason::Paused
                    | StopReason::Unknown
                    | StopReason::WantsTools,
                )
                | None => Recap::Incomplete,
            })
        });

        self.transcript.pop();
        Ok(said?)
    }

    /// Clears old tool results from what the model is sent, and records it.
    ///
    /// Walked newest-first, protecting the most recent [`PROTECT`] bytes of
    /// output the model is still working from. Each result past that is a
    /// candidate, and the candidates are cleared together only where they cross
    /// [`MINIMUM`] — a pruning that recovered less would cost the record more
    /// than it saved. Cleared together rather than one at a time so the log
    /// holds one line for the pass, and so the transcript and the log move in
    /// the same step: the line names the results, and replay clears the same
    /// ones on a continue.
    ///
    /// Nothing is asked of the model and nothing is removed from history. The
    /// calls stay, the prose stays, and the placeholder keeps the shape of a
    /// result that answered — only the bulk is gone, and only from what the
    /// model is sent.
    fn prune(&mut self) {
        // The newest output is protected: a result the model just read is not
        // one to pull out from under it. Counted in bytes, the figure the
        // results are actually measured in.
        let mut recent = 0_u64;
        let mut clearing: Vec<ToolId> = Vec::new();
        let mut savings = 0_u64;

        for message in self.transcript.messages().iter().rev() {
            let Message::ToolResults(results) = message else {
                continue;
            };

            for result in results.iter().rev() {
                let bytes = result.output.text().len() as u64;

                // A result small enough that clearing it buys nothing is
                // skipped whole, and does not spend the protected window: the
                // window is for output worth keeping, and this is neither.
                if recent < PROTECT {
                    recent = recent.saturating_add(bytes);
                    continue;
                }

                if bytes >= ToolOutput::MIN_PRUNE_BYTES as u64 {
                    clearing.push(result.id.clone());
                    savings = savings.saturating_add(bytes);
                }
            }
        }

        if savings < MINIMUM {
            return;
        }

        // The transcript first, because the log line names what was cleared:
        // writing it before the clearing would let a crash between the two
        // leave a log claiming results were cleared that the transcript still
        // holds. The line goes out once the transcript has moved, and replay
        // reads it to make the same move again.
        let freed = self.transcript.prune(&clearing);
        self.session.pruned(freed, &clearing);

        // The load drops by what was freed: the transcript is smaller, and the
        // next request is the thing that is measured. Recounted rather than
        // adjusted, because the estimate's rate is the provider's and this is
        // the moment it is known to be exact.
        self.load.replaced();
        for message in self.transcript.messages() {
            self.load.recounted(message);
        }
        self.load
            .requesting(self.spec.instructions(), &self.tools.advertised());
    }
}

/// Whether every required checkpoint section is present, ordered and filled.
///
/// `Progress` is a container; its three subsections carry the content. Every
/// other section must say something, including `(none)`, so a clean provider
/// stop cannot make a structurally truncated checkpoint look complete.
fn structured(said: &str) -> bool {
    const SECTIONS: &[(&str, bool)] = &[
        ("## Goal", true),
        ("## Constraints & Preferences", true),
        ("## Progress", false),
        ("### Done", true),
        ("### In Progress", true),
        ("### Blocked", true),
        ("## Decisions", true),
        ("## Next Steps", true),
        ("## Critical Context", true),
    ];

    if !said.starts_with("## Goal\n") || said.lines().any(|line| line == FILES) {
        return false;
    }

    let lines: Vec<&str> = said.lines().collect();
    let headings: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| line.starts_with("##").then_some(at))
        .collect();
    if headings.len() != SECTIONS.len()
        || headings
            .iter()
            .zip(SECTIONS)
            .any(|(at, (expected, _))| lines.get(*at) != Some(expected))
    {
        return false;
    }

    SECTIONS.iter().enumerate().all(|(index, (_, required))| {
        if !required {
            return true;
        }
        let Some(start) = headings.get(index).map(|heading| heading + 1) else {
            return false;
        };
        let end = headings.get(index + 1).copied().unwrap_or(lines.len());
        lines
            .get(start..end)
            .is_some_and(|section| section.iter().any(|line| !line.trim().is_empty()))
    })
}

/// Appends the file record derived from calls and prior checkpoints.
fn append_files(recap: &mut String, touched: &(Vec<String>, Vec<String>)) {
    while recap.ends_with(char::is_whitespace) {
        recap.pop();
    }
    let _ = write!(recap, "\n\n{FILES}\n");
    if touched.0.is_empty() && touched.1.is_empty() {
        recap.push_str("(none yet)");
        return;
    }
    for path in &touched.0 {
        let _ = writeln!(recap, "{path} (read)");
    }
    for path in &touched.1 {
        let _ = writeln!(recap, "{path} (modified)");
    }
    recap.pop();
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

/// The files a session has touched, read and modified kept apart.
///
/// The two lists a recap carries, accumulated as the span being replaced is
/// walked. Each file appears once, in the order it was first noted; a file that
/// was ever changed is on the modified list and nowhere else, because that is
/// the fact a later turn cannot do without.
#[derive(Default)]
struct Files {
    read: Vec<String>,
    modified: Vec<String>,
}

impl Files {
    /// Notes one file, read or changed.
    ///
    /// A change wins over a read: the same file may be opened a dozen times and
    /// edited once, and the edit is what the next session needs to know about.
    /// A file already changed stays changed however many reads follow, and one
    /// already listed is not listed again.
    fn note(&mut self, path: &str, changed: bool) {
        if changed {
            self.read.retain(|kept| kept != path);
            if !self.modified.iter().any(|kept| kept == path) {
                self.modified.push(path.to_owned());
            }
        } else if !self.modified.iter().any(|kept| kept == path)
            && !self.read.iter().any(|kept| kept == path)
        {
            self.read.push(path.to_owned());
        }
    }
}

/// The files a prior recap carried, read back off the text it left.
///
/// Everything from the `Files so far:` line to the end, one `path (read)` or
/// `path (modified)` per line. Anything that does not parse as one of those is
/// left out rather than guessed at: a line from an older recap written some
/// other way is not a file this session touched. New recaps receive this list
/// from code, not from the model.
fn listed(recap: &str) -> Vec<(&str, bool)> {
    let Some((_, files)) = recap.split_once(FILES) else {
        return Vec::new();
    };

    files
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(path) = line.strip_suffix("(modified)") {
                Some((path.trim(), true))
            } else {
                line.strip_suffix("(read)").map(|path| (path.trim(), false))
            }
        })
        .filter(|(path, _)| !path.is_empty())
        .collect()
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

    #[test]
    fn a_structured_recap_has_every_heading_once_in_exact_order() {
        let complete = "## Goal\ngoal\n## Constraints & Preferences\n(none)\n## Progress\n### Done\ndone\n### In Progress\n(none)\n### Blocked\n(none)\n## Decisions\n(none)\n## Next Steps\nnext\n## Critical Context\n(none)";
        assert!(structured(complete));

        let extra = complete.replace("## Decisions", "## Surprise\nextra\n## Decisions");
        assert!(!structured(&extra));

        let duplicate = complete.replace("## Next Steps", "## Decisions\nagain\n## Next Steps");
        assert!(!structured(&duplicate));

        let empty = complete.replace("## Critical Context\n(none)", "## Critical Context");
        assert!(!structured(&empty));
    }

    #[test]
    fn a_recap_without_a_file_list_carries_none_forward() {
        // A recap written before this existed, or by a model that left the list
        // out, has nothing to carry — and that is an answer, not a failure.
        assert!(listed("## Goal\nbuild the thing").is_empty());
    }

    #[test]
    fn the_files_a_recap_kept_are_read_back_the_way_they_were_written() {
        let recap =
            "## State\nnext: ship it\n\nFiles so far:\nsrc/main.rs (modified)\nREADME.md (read)\n";

        assert_eq!(listed(recap), [("src/main.rs", true), ("README.md", false)]);
    }

    #[test]
    fn a_line_that_is_not_a_file_is_not_read_as_one() {
        // Older recap text may have carried this list itself. A line it wrote
        // some other way is not a file the session touched and is left out
        // rather than guessed at; new recaps receive the list from code.
        let recap = "Files so far:\nsrc/main.rs (read)\nnot a file line\n(modified)\n";

        assert_eq!(listed(recap), [("src/main.rs", false)]);
    }

    #[test]
    fn a_file_is_listed_once_and_a_change_outranks_a_read() {
        // The rules the recap's accuracy rests on: no file twice, and the edit
        // is the fact a later turn needs about a file it also only read.
        let mut files = Files::default();
        files.note("src/main.rs", false);
        files.note("src/main.rs", false);
        assert_eq!(files.read, ["src/main.rs".to_owned()]);

        // Read first, then changed: it moves, because the read is no longer the
        // truest thing to say about it.
        files.note("src/main.rs", true);
        assert!(
            files.read.is_empty(),
            "a changed file is still listed as read"
        );
        assert_eq!(files.modified, ["src/main.rs".to_owned()]);

        // And changed first, then read: it stays changed, however many reads
        // follow.
        files.note("src/lib.rs", true);
        files.note("src/lib.rs", false);
        assert_eq!(
            files.modified,
            ["src/main.rs".to_owned(), "src/lib.rs".to_owned()]
        );
        assert!(!files.read.iter().any(|kept| kept == "src/lib.rs"));
    }
}
