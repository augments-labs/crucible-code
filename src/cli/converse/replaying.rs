//! Putting a session picked up back on the screen.
//!
//! Original conversation and compaction notices come from the protected log,
//! independently of the smaller transcript sent to the model. Replay calls no
//! provider or tool and reads no workspace file to reconstruct old changes.
//!
//! The live draw functions render prompts, markdown, grouped tool calls and
//! expandable results. New logs also retain bounded private diff previews;
//! older logs still restore their recorded change counts without inventing
//! file contents. Display history is streamed a message batch at a time so it
//! does not create a second session-sized transcript in memory.

use std::collections::HashMap;

use crucible_core::{Diff, Message, RECAP, ToolId};
use crucible_runner::Runner;
use crucible_session::{DisplayHistory, DisplayItem, Pruned, Session, SessionError};
use crucible_tui::{Recording, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::converse::Terms;
use crate::cli::draw;
use crate::cli::gathering::Gathering;
use crate::cli::kept::Kept;
use crate::cli::style::Style;

/// What stands over the notes a compaction left.
///
/// They ride a user message because the closed set of messages has no variant
/// for them and no provider would know what to do with one — so without a line
/// saying otherwise they would go down behind the mark a typed line wears,
/// which would say the user wrote them.
const NOTES: &str = "notes on everything before this";

/// Puts what a session already said back on the screen.
///
/// Original log records are committed to scrollback through the live builders.
/// The model transcript is only a fallback for sessions without a log.
///
/// # Errors
///
/// A session storage error if history cannot be read, or [`Fatal::Terminal`]
/// if the terminal could not be written to.
pub(super) fn replayed<T: Terminal>(
    renderer: &mut Renderer<T>,
    against: &Replay<'_>,
    session: &Session,
    kept: &mut Kept,
) -> Result<(), Fatal> {
    if let Some(history) = session.display_history()? {
        let pruned = Pruned::default();
        let original = Replay {
            runner: against.runner,
            style: against.style,
            pruned: &pruned,
        };
        return streamed(renderer, history, &original, session, kept);
    }
    walked(
        renderer,
        against.runner.transcript().messages(),
        &Previews::new(),
        against,
        kept,
    )
}

/// The change lines a log kept for one batch of results, for the reader alone.
///
/// Empty for a transcript replayed without its log, and for a log written
/// before previews were kept: both draw the recorded counts instead, which is
/// what those sessions were shown.
type Previews = HashMap<ToolId, Diff>;

/// Keeps only the current message and its following result batch in memory.
/// Grouping needs that one-message lookahead to distinguish failed calls.
fn streamed<T: Terminal>(
    renderer: &mut Renderer<T>,
    history: DisplayHistory,
    against: &Replay<'_>,
    session: &Session,
    kept: &mut Kept,
) -> Result<(), Fatal> {
    let mut pending: Option<(Message, Previews)> = None;
    for item in history {
        let item = item.map_err(|source| SessionError::Log {
            at: session.path().display().to_string().into(),
            source,
        })?;
        match item {
            DisplayItem::Message { message, previews } => {
                if let Some((earlier, mut kept_previews)) = pending.take() {
                    if matches!(&earlier, Message::Agent { calls, .. } if !calls.is_empty())
                        && matches!(&message, Message::ToolResults(_))
                    {
                        // The call line and the results under it go down as one
                        // batch, so the previews the results carry travel with
                        // it: only one of the two messages ever holds any.
                        kept_previews.extend(previews);
                        walked(renderer, &[earlier, message], &kept_previews, against, kept)?;
                        continue;
                    }
                    walked(renderer, &[earlier], &kept_previews, against, kept)?;
                }
                pending = Some((message, previews));
            }
            DisplayItem::Compacted(details) => {
                if let Some((earlier, previews)) = pending.take() {
                    walked(renderer, &[earlier], &previews, against, kept)?;
                }
                renderer.apart()?;
                renderer.present(&draw::compacted_rows(
                    details,
                    renderer.columns(),
                    against.style.glyphs(),
                ))?;
            }
            DisplayItem::LegacyCompacted { replaced } => {
                if let Some((earlier, previews)) = pending.take() {
                    walked(renderer, &[earlier], &previews, against, kept)?;
                }
                renderer.apart()?;
                let detail = if replaced == 0 {
                    "old tool output was cleared".to_owned()
                } else {
                    format!("{replaced} earlier messages became a recap")
                };
                renderer.commit(&format!("compacted · {detail}"))?;
            }
            DisplayItem::ContextReset => {
                // Old /clear reset model context but left terminal scrollback
                // and its result offers visible. Only message grouping ends.
                if let Some((earlier, previews)) = pending.take() {
                    walked(renderer, &[earlier], &previews, against, kept)?;
                }
            }
        }
    }
    if let Some((earlier, previews)) = pending {
        walked(renderer, &[earlier], &previews, against, kept)?;
    }
    renderer.settle()?;
    Ok(())
}

/// The tail of a session nobody has picked up, drawn into rows `columns` wide.
///
/// The picker uses the same message builders at its own width. Its bounded
/// message tail omits supplemental display metadata; selecting the session
/// streams the complete history and restores those details. What comes back
/// here is only the last `most` rows of the supplied messages.
///
/// Bounded by `most` for the same reason the log is read from its end: a
/// preview is a glance, and one that kept every row of a long session would
/// spend a session's memory answering it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the rows could not be drawn, which a recording does
/// not do.
pub(super) fn glimpsed(
    messages: &[Message],
    against: &Replay<'_>,
    columns: usize,
    most: usize,
) -> Result<Vec<Row>, Fatal> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }

    // Redirected, because nobody is looking at this screen: a recording that
    // claims to be a terminal repaints every row it holds after every message
    // put back, which turns a long session's preview into the length of that
    // session squared. What is wanted is the rows, and those are recorded
    // either way.
    let mut renderer = Renderer::new(Recording::redirected(columns, most.max(1)));
    renderer.wears(against.style.palette());
    renderer.draws(against.style.glyphs());

    // A held of its own, dropped with the renderer: what a key would open is
    // the business of the session on the screen, and this one is not on it.
    walked(
        &mut renderer,
        messages,
        &Previews::new(),
        against,
        &mut Kept::default(),
    )?;

    Ok(renderer.tail(most))
}

/// The walk itself: every message put back, then the tail ended.
fn walked<T: Terminal>(
    renderer: &mut Renderer<T>,
    messages: &[Message],
    previews: &Previews,
    against: &Replay<'_>,
    kept: &mut Kept,
) -> Result<(), Fatal> {
    // Counted before a row goes down, because the first row of a run is the
    // one that says how long the run is.
    let mut batch = Batch {
        folded: Folded::of(messages, against.runner),
        previews,
    };

    for message in messages {
        said(renderer, against, kept, &mut batch, message)?;
    }

    // A call the log never answered -- the session ended while it was out --
    // still went down as a line the reader watched, so it goes down here too.
    batch.folded.unanswered(renderer, against.style, kept)?;

    // Whatever the last message left live, ended: a session whose last turn was
    // the model talking leaves a tail in the region the renderer owns, and what
    // is said next belongs under it rather than in the middle of it.
    renderer.settle()?;

    Ok(())
}

/// What a whole replay is drawn against, and what does not change while it
/// runs: the session being put back, what a pruning cleared out of it, and the
/// dress the renderer is already wearing.
///
/// One value rather than three parameters carried down the walk — what changes
/// from one call to the next is the message, and this is everything that does
/// not. It is what [`glimpsed`] is handed too, for the same reason: a caller
/// drawing a session it is not in still has to say which build's tools are
/// being named.
pub(super) struct Replay<'a> {
    /// The session in hand, for what each tool's call line reads as.
    pub(super) runner: &'a Runner,
    /// What the results a pruning cleared said, so a row a reader watched come
    /// back says it again. Empty where nothing was ever cleared, which is every
    /// session short enough not to have needed the room.
    pub(super) pruned: &'a Pruned,
    /// The dress the rows are drawn in.
    pub(super) style: Style,
}

impl<'a> Replay<'a> {
    /// What a session this run is in is drawn against.
    ///
    /// The two ways into a session — the command line and `/resume` — both put
    /// it back on the screen, and this is what keeps them putting it back
    /// against the same three things. A preview builds its own, because the
    /// session it draws is not the one this run is in.
    pub(super) fn of(runner: &'a Runner, terms: &'a Terms, pruned: &'a Pruned) -> Self {
        Self {
            runner,
            pruned,
            style: terms.style(),
        }
    }
}

/// One message, put back the way it went down.
///
/// The arms are in the order a turn produces them, which is the order the
/// transcript holds them in — so walking it hands the renderer the same calls
/// in the same order the turn did, and the picture is the picture.
fn said<T: Terminal>(
    renderer: &mut Renderer<T>,
    against: &Replay<'_>,
    kept: &mut Kept,
    batch: &mut Batch<'_>,
    message: &Message,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let style = against.style;

    // Whatever the batch before this one never answered goes down first, where
    // the live path would have left it: a call line with nothing under it.
    if !matches!(message, Message::ToolResults(_)) {
        batch.folded.unanswered(renderer, style, kept)?;
    }

    match message {
        // Harness facts belong in what the model reads, not in the transcript
        // attributed to either participant on screen.
        Message::Context(_) => {}

        // The notes a compaction left standing, under a line saying whose words
        // they are, and through the same door the model's prose goes through —
        // because that is what they are.
        Message::User { text: said, .. } if said.starts_with(RECAP) => {
            renderer.apart()?;
            renderer.present(&[Row::new().then(Slot::Quiet, clip(NOTES, columns))])?;
            renderer.stream(said.strip_prefix(RECAP).unwrap_or(said))?;
            renderer.settle()?;
        }

        // What was asked, in the row the box commits when it is typed: the mark,
        // the ground behind it, the break at the column rather than at a space.
        // A reader finds their own words the way they left them.
        Message::User {
            text: said,
            attachments,
        } => {
            draw::queued(renderer, said, style)?;
            draw::attached(renderer, attachments, style)?;
        }

        Message::Agent {
            continuation: _,
            text,
            calls,
            stop,
        } => {
            if !text.trim().is_empty() {
                renderer.apart()?;
                renderer.stream(text)?;
            }

            // Settled whether or not anything was said, because what follows is
            // presented, and a line still open is one the row under it would be
            // written into the middle of.
            renderer.settle()?;

            // The line the footing was drawing while the tool was out, with the
            // motion gone — which is the line that joined the transcript when it
            // answered. What the call was about is asked of the tool that owns
            // the arguments, the same way it was asked the first time.
            for call in calls {
                let line = draw::called(call, &against.runner.about(call));

                // Named before the row that answers it goes down, the same way
                // the turn named it: the expansion carries the call's line, and
                // a result whose call was never named would open under a heading
                // nobody wrote.
                kept.calling(call.id.clone(), line.clone());

                // A call in a folded run has no row of its own: the line the
                // run came to stands where the first of those rows would have,
                // and every call after it adds nothing to the screen. What each
                // of them said is still reachable, from that one line.
                if batch.folded.holds(&call.id) {
                    if let Some(said) = batch.folded.opens(&call.id).map(ToOwned::to_owned) {
                        let at = draw::gathered(renderer, &said, style)?;
                        batch.folded.went(&call.id, at);
                    }

                    continue;
                }

                // Not drawn yet. Live, a call's line joins the transcript when
                // the call answers, with the result directly under it -- so a
                // batch of three reads as three pairs, not as three lines and
                // then three results. Held until the answer comes past.
                batch.folded.named(call.id.clone(), line);
            }

            // An answer that did not end the way the model meant it to is worth
            // the same line here it got the first time: a half answer read back
            // as a whole one is the one thing a transcript may not do.
            if let Some(said) = stop.and_then(draw::notice) {
                renderer.apart()?;
                renderer.commit(said)?;
            }
        }

        // Under the call line above it, which is where a reader asking what a
        // call did is already looking.
        Message::ToolResults(results) => {
            for result in results {
                // Through the door the turn drew it through, which is what makes
                // the row that comes back the row that went down: lit where it
                // was cut, and holding the lines it was cut from where the key
                // over it can reach them.
                // Copied rather than moved, which is the one thing this path
                // does that the turn's did not: the transcript owns this result
                // and goes on being sent, so what is held for the key to open is
                // a second copy of one result. Bounded by the tool that made it
                // and by the ceiling the record keeps, so what a replay costs is
                // the same after four hundred messages as after four.
                // And saying what it said, where a pruning has since cleared it.
                // The transcript keeps the placeholder, because that is what the
                // model is being sent; the row gets the words back, because that
                // is what the reader was shown. Neither is told about the other.
                let shown = draw::Shown::replayed(
                    result.output.clone(),
                    batch.previews.get(&result.id).cloned(),
                );
                let output = match against.pruned.showed(&result.id) {
                    Some(showed) => shown.saying(showed),
                    None => shown,
                };

                // A call the line above it counted. There is no row of its
                // own to hang this under, so it is kept whole against the line
                // that stands for the run — which is the door to all of them.
                if batch.folded.holds(&result.id) {
                    kept.gathered(&result.id, output.into_text(), batch.folded.at(&result.id));
                    continue;
                }

                // The call's own line, directly over its result: the pair the
                // turn drew when the answer came in.
                if let Some(line) = batch.folded.answering(&result.id) {
                    draw::returned(renderer, &line, style)?;
                }

                draw::came_back(renderer, kept, &result.id, output, style)?;
            }
        }
    }

    Ok(())
}

/// What one batch of messages is drawn with beside the transcript itself.
///
/// Both halves are worked out per batch, which is what keeps them out of
/// [`Replay`]: that value is everything a replay does not change while it runs,
/// and neither of these survives the batch it was read for.
struct Batch<'a> {
    /// Which of the batch's calls share a line, settled before the first row.
    folded: Folded,
    /// The change lines the log kept for the batch's results, for the reader.
    previews: &'a Previews,
}

/// Which calls a walk folds into one line, worked out before it draws a row.
///
/// A turn cannot know how long a run of calls is until it ends, so it holds the
/// first call of one back until a second arrives. A walk has the whole
/// transcript in front of it and needs no such trick: it counts the runs first,
/// so by the time a row goes down it already knows whether the call it belongs
/// to has a row of its own or a share of a line.
///
/// Which is what makes a resumed session the session it was. The picture a
/// reader left is the one they come back to, and a turn that folded three reads
/// into a line may not put three rows back on the screen a day later.
#[derive(Default)]
struct Folded {
    /// Which run each folded call is in. A call in no run is not in here, and
    /// a run of one is no run.
    run: HashMap<ToolId, usize>,
    /// The call that opens each run, and what that run's line says.
    opens: Vec<(ToolId, String)>,
    /// The record row each run's line went down on, once it has.
    rows: Vec<Option<usize>>,
    /// The calls with a row of their own whose row has not gone down yet, with
    /// the line each will be drawn as. Drawn when the answer arrives, or when
    /// the walk moves on without one.
    waiting: Vec<(ToolId, String)>,
}

impl Folded {
    /// The runs in a transcript, in the order the walk will meet them.
    ///
    /// A run ends where the live path would have ended it: at a prompt, at a
    /// fact put in front of one, at prose between two calls, at a stop worth a
    /// line of its own, at any call that did more than look around, and at the
    /// end of a round trip. So a run is one batch of calls at most, which is
    /// what the reader watched: the line went down when the batch was answered
    /// rather than when the turn was.
    fn of(messages: &[Message], runner: &Runner) -> Self {
        let mut folded = Self::default();
        let mut run = Gathering::default();

        for (index, message) in messages.iter().enumerate() {
            match message {
                // A prompt, a fact put in front of one, and the end of a round
                // trip. The last is where the live path closes a run too:
                // every call of the batch above has been answered and the
                // agent is going back for more. Closing there is what keeps
                // the two paths drawing the same rows — a walk that folded a
                // whole turn into one line would put back a session nobody
                // watched.
                Message::User { .. } | Message::Context(_) | Message::ToolResults(_) => {
                    folded.close(&mut run);
                }
                Message::Agent {
                    continuation: _,
                    text,
                    calls,
                    stop,
                } => {
                    if !text.trim().is_empty() {
                        folded.close(&mut run);
                    }

                    for call in calls {
                        // Results follow their request batch. Missing and failed
                        // answers keep individual headings, just as they did live.
                        let answered = match messages.get(index + 1) {
                            Some(Message::ToolResults(results)) => results
                                .iter()
                                .any(|result| result.id == call.id && !result.output.is_failed()),
                            _ => false,
                        };
                        match runner.folds(call).filter(|_| answered) {
                            Some(looking) => run.counted(call.id.clone(), looking),
                            None => folded.close(&mut run),
                        }
                    }

                    if stop.and_then(draw::notice).is_some() {
                        folded.close(&mut run);
                    }
                }
            }
        }

        folded.close(&mut run);
        folded
    }

    /// Ends the run in hand, keeping it where it came to more than one call.
    fn close(&mut self, run: &mut Gathering) {
        let run = run.taken();
        let Some(opening) = run.calls().first().filter(|_| run.folds()).cloned() else {
            return;
        };

        let index = self.opens.len();
        for call in run.calls() {
            self.run.insert(call.clone(), index);
        }

        self.opens.push((opening, run.did()));
        self.rows.push(None);
    }

    /// Whether this call is one of a run, and so has no row of its own.
    fn holds(&self, call: &ToolId) -> bool {
        self.run.contains_key(call)
    }

    /// What the run this call opens says, or nothing where it opens none.
    fn opens(&self, call: &ToolId) -> Option<&str> {
        let (opening, said) = self.opens.get(*self.run.get(call)?)?;
        (opening == call).then_some(said.as_str())
    }

    /// Remembers the record row this call's run went down on.
    fn went(&mut self, call: &ToolId, at: usize) {
        let Some(&index) = self.run.get(call) else {
            return;
        };

        if let Some(row) = self.rows.get_mut(index) {
            *row = Some(at);
        }
    }

    /// The row this call's result answers to, once its run has been drawn.
    fn at(&self, call: &ToolId) -> Option<usize> {
        self.rows.get(*self.run.get(call)?).copied().flatten()
    }

    /// Holds a call's line back until its answer comes past.
    fn named(&mut self, call: ToolId, line: String) {
        self.waiting.push((call, line));
    }

    /// The line held for this call, taken out to be drawn over its answer.
    fn answering(&mut self, call: &ToolId) -> Option<String> {
        let at = self.waiting.iter().position(|(held, _)| held == call)?;
        Some(self.waiting.remove(at).1)
    }

    /// Draws every line still held, in the order the calls were made.
    ///
    /// What the live path leaves where a batch is not answered: the call lines
    /// stand, with nothing under them, and the reader can see what was asked.
    fn unanswered<T: Terminal>(
        &mut self,
        renderer: &mut Renderer<T>,
        style: Style,
        kept: &mut Kept,
    ) -> Result<(), Fatal> {
        for (call, line) in self.waiting.drain(..) {
            kept.abandoned(&call);
            draw::returned(renderer, &line, style)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
