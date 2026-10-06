//! What stands above the box while a turn is running.
//!
//! The component draws a mark, a word, a clock and a count; this is where all
//! four come from. The clock starts as the turn leaves for its own thread and
//! never pauses — not for a permission question, which is time somebody is
//! waiting just as much. The word and the count are read off the events the
//! turn reports as they go past, which is why they are read here rather than by
//! the component: the events are this program's, and the component knows
//! nothing about them.
//!
//! The key is named here for the second time on the screen — the row under the
//! box names it too — and that is deliberate. It is the third segment of this
//! row and the first thing a narrow window drops, and the key that stops a turn
//! is the wrong thing for a narrow window to take away.
//!
//! A call standing above that row is the other thing held here. A tool that has
//! been asked for and has not answered gets its line drawn with a mark that
//! pulses, and it lives here rather than in the transcript because a line drawn
//! again every frame cannot also be one the record holds. When the tool answers
//! the line is handed back — [`Turning::saw`] returns it — and whoever drives
//! this commits it still, so what the transcript keeps is the same words with
//! the motion gone.
//!
//! A held call is not drawn the moment it is asked for. Most commands are over
//! in a fraction of a second, and a row put up for each and taken down a moment
//! later is the screen blinking on every call; a call stands over the row only
//! once it has been out for as long as `output.pinAfterSeconds` says
//! ([`Turning::pinning`]), and one that answers sooner goes
//! to the transcript without ever having been drawn here. Elapsed time alone
//! decides it, because nothing about a command's text says how long it will
//! take. The delay is about drawing and nothing else: the background key reads
//! the call at the front of the queue whether or not it stands yet, so somebody
//! who knows a command will run long can put it down the moment it starts, and
//! it leaves for the transcript the way a drawn one does.
//!
//! Not every call is held. One that is the only call of its pass, cannot be
//! backgrounded and only looks up something elsewhere has nothing to draw
//! again: no output arrives under it, no key points at it, and its words are
//! settled the moment it is asked for. [`Turning::saw`] hands that one back
//! where it was requested, and it is written where every other row of the turn
//! is. What holding it was buying was the promise that a result is drawn under
//! the call it answers, and the transcript only grows at the end, so that
//! promise needs holding exactly when a second call could answer first.
//!
//! Which is what a pass of several is, and there the row says how many of what
//! rather than naming one of them. One response asking for eight fetches used
//! to put the first of the eight over the box and leave it there until it
//! answered — one URL in front of the reader for as long as the whole batch
//! took, and no word about the seven behind it. A call that can be
//! backgrounded or has started printing is named as it always was: the key
//! points at a row, and a sample belongs to a call rather than to a number.
//!
//! Over that call, where the turn has been looking around rather than doing one
//! thing, the line counting what it has looked at so far. It is not held here
//! either — the words are read off the run each frame and handed in, since a
//! count from the frame before would say the run had stalled — but it wears the
//! same mark as the call under it, read once for the pair, so the two blink
//! together and read as one turn doing one thing.
//!
//! Under the row, where a prompt was finished while this turn was running, the
//! line that will be sent once it ends. A line typed into the box mid-turn
//! leaves the box the moment Return is pressed, and until it is named here the
//! only acknowledgement it ever got was its own turn starting, minutes later.
//! It is a second row of the same thing rather than a thing of its own, so no
//! blank parts the two.
//!
//! Under all of that, the plan the agent is working to. Nothing about it is
//! held here — it is read from the tool that writes it, and this module never
//! learns there is a tool — but it is laid out here, because a window with room
//! for some of this and not all of it has to be divided somewhere, and dividing
//! it in two places is how a footing comes to be a row taller than the window.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crucible_runner::{Breakdown, Event, Totals};
use crucible_tools::Looking;
use crucible_tui::{Row, Slot, Working};
use crucible_types::{Compacting, PlanWindows, ToolId};

use super::super::draw;
use super::super::style::Style;
use super::planning::Planning;
use super::queueing::{self, Prompts};

/// How the turn is asked to stop, said after the clock.
const STOPS: &str = "esc to interrupt";

/// The rows this puts above the box, blanks included.
const ROWS: usize = 3;

/// And with a backgroundable call standing over it: the blank that parts them,
/// the call, and the row under it offering to leave the command running.
///
/// Three rather than two, because the offer is drawn from the moment a call that
/// supports it stands. It is one row and it is the only thing on screen that says
/// what a key would do about the command in front of you, so it belongs with the
/// call rather than with the sample the call's output gives way as. A call that
/// cannot be left running needs one fewer row.
///
/// The call line is what a narrow window gives up first, because it is the one
/// of the four that a second look gets back anyway: the tool answers and the
/// line is written into the transcript either way. The queue's panel goes next,
/// a line at a time and then entirely, since what it names is still in the
/// queue and its own turn will say it. Then the plan, which gives up its own
/// rows a state at a time before it goes entirely — and it is measured before
/// either of the other two, so those are what a window short of rows drops on
/// its behalf. The row saying a turn is running exists
/// nowhere else and never gives way.
const CALLING: usize = ROWS + 3;

/// What the row under a call offers to do about it.
///
/// Named beside the sample rather than in the key's own module, because it is the
/// one place the offer is spelled and the row is where somebody reads it. The key
/// itself is documented once in the keys page, which is why this says what it
/// does about *this command* rather than repeating what the key is for.
const BACKGROUND: &str = "(ctrl+b to background)";

/// The most rows of a running command's output the footing shows at once.
///
/// Enough to see what a compiler is working through, and few enough that a
/// twenty-four-row window still belongs to the conversation. It is the first
/// thing the ladder below gives up, so on a short window this is a ceiling
/// rather than a promise.
const SAMPLE: usize = 5;

/// The least unscaled room the bar needs before it is shown.
///
/// Below this the whole row gives way rather than drawing a progress sliver. On
/// a wider window the bar uses two thirds of the room available up to
/// [`BAR_MAX`], so the extra columns become useful resolution without letting
/// the progress row dominate the prompt.
const BAR: usize = 28;

/// The most available progress cells considered on a wide window, before the
/// two-thirds scale is applied.
///
/// A larger source width would repeat percentage values rather than add
/// information, and this keeps every frame bounded by a small constant even on
/// an unusually wide terminal.
const BAR_MAX: usize = 100;

/// The fraction of its former available width the progress bar keeps.
const BAR_NUMERATOR: usize = 2;
const BAR_DENOMINATOR: usize = 3;

/// Columns beside the progress cells: the gap and the longest percentage.
const BAR_TAIL: usize = 6;

/// How long a completed bar remains visible.
///
/// A single render frame can be replaced by the completion events already
/// waiting behind it, which made a recap appear to stop around ninety percent.
/// Half a second is long enough to perceive and short enough not to hold up the
/// next activity; only the drawing side waits, never the worker.
const COMPLETE_FOR: Duration = Duration::from_millis(500);

/// How far in the sample stands, in columns.
///
/// The depth the panel that asks about a command already indents the command
/// under its subject. It reads as the call's own output rather than as a second
/// thing beside it.
const INSET: usize = 4;

/// The most one line of a command's output is held as, in bytes.
///
/// A row shows what fits in the window and no more, so what is held past that is
/// held to be thrown away. A command printing a megabyte without a newline —
/// which is one `find` over a large tree with its output on one line — must not
/// make this grow while it does it.
const LINE: usize = 1024;

/// The one word for what a turn is doing at this moment.
///
/// Four, because four is what the events can tell apart. A fifth read off the
/// same events would be a word the screen changed to and nothing else did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Doing {
    /// Asked, and nothing has come back yet.
    Thinking,
    /// Prose is arriving.
    Writing,
    /// A tool was asked for and has not answered.
    Running,
    /// Esc has been pressed and the turn is stopping.
    Interrupting,
    /// The response failed before it said anything and is being asked for again.
    Retrying,
    /// The window filled and room is being made. The turn has not ended.
    Compacting,
}

impl Doing {
    /// The word, as the row says it.
    fn word(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::Writing => "writing",
            Self::Running => "running",
            Self::Interrupting => "interrupting",
            Self::Retrying => "retrying",
            Self::Compacting => "compacting",
        }
    }
}

/// The row above the box, and what it last said.
#[derive(Debug)]
pub(super) struct Turning {
    /// When the turn left, which is what the clock counts from.
    since: Instant,
    /// What it is doing now.
    doing: Doing,
    /// What it has spent so far, or `None` until the provider says.
    spent: Option<u64>,
    /// The latest request divided by what holds the window, which is also
    /// where the usable room left before compaction is read from.
    breakdown: Breakdown,
    /// What the session has used, as the turn last reported it: what `/usage`
    /// shows while the runner is away on the turn.
    totals: Totals,
    /// The plan windows the vendor last reported, for the same panel.
    limits: Option<PlanWindows>,
    /// Why room is being made, and `None` when no progress row remains.
    ///
    /// Kept briefly after [`Event::Compacted`] with `part` at 100, so completed
    /// work remains perceptible before the live footing returns to the turn.
    /// [`Self::finished_frame`] removes it after the dwell; no transcript row is
    /// committed for this live state.
    making: Option<Compacting>,
    /// How much of the notes has been written, as a percentage.
    ///
    /// What the bar under the word fills to. A fraction of the room the notes
    /// were given rather than of how long it will take — the answer is arriving
    /// and this is how much of it has, which is the only thing here that is
    /// actually known.
    part: u8,
    /// When completion became visible, until its short dwell has elapsed.
    completed: Option<Instant>,
    /// The calls whose tools are out, in the order they were requested.
    ///
    /// A response may ask for several before any of them runs. Each keeps its
    /// provider-assigned identity here so an answer can take back the line it
    /// belongs under rather than whichever request happened to arrive last.
    calling: VecDeque<Calling>,
    /// What the footing was last drawn from, so a redraw that would draw the
    /// same rows again can be skipped. `None` before the first.
    drawn: Option<Drawn>,
    /// How long a call is out before it stands over the row: `output.pinAfterSeconds`,
    /// handed in by [`Turning::pinning`]. Nought for a turn told nothing, which is
    /// every call drawn the moment it is asked for, as nought in the file says.
    pinned_after: Duration,
}

/// One call still waiting for its result.
#[derive(Debug)]
struct Calling {
    /// The identity its result and live output carry.
    id: ToolId,
    /// When it was asked for, which is what [`Turning::pinning`] counts from.
    asked: Instant,
    /// The words the call row says, without its moving mark.
    said: draw::Called,
    /// What this call has printed while it runs.
    printing: Printing,
    /// Whether the call can be left to finish after its tool answers the turn.
    backgroundable: bool,
    /// What kind of looking-around this call is, where it is only that.
    looking: Option<Looking>,
    /// The tool's name as a row writes it, kept apart from [`Calling::said`]
    /// so a count of what is out does not have to read it back out of a
    /// sentence that has already had the arguments put into it.
    name: String,
}

/// A call line that has stopped being live and is the transcript's to commit.
///
/// Three things rather than a pair, because what the transcript does with the
/// line depends on the third: a call that only looked around joins the run
/// being counted above it, and one that did something is named on a row of its
/// own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Settled {
    /// Which call it was.
    pub(super) call: ToolId,
    /// The words its row says, without the moving mark it wore while it ran.
    pub(super) said: draw::Called,
    /// What kind of looking-around it was, where it was only that.
    pub(super) looking: Option<Looking>,
}

/// The line a call stopped being live on, out of what was held while it ran.
fn settled(calling: Calling) -> Settled {
    Settled {
        call: calling.id,
        said: calling.said,
        looking: calling.looking,
    }
}

/// What a running command has printed, as much of it as is shown.
///
/// Two things at once, and they are counted separately on purpose: the last few
/// lines, which are what the reader is watching, and how much there has been,
/// which is the only thing that keeps five rows from reading as everything the
/// command has said.
///
/// Bounded in all three directions — the rows held, the length of the line still
/// arriving, and nothing at all kept about the lines that have scrolled past
/// except that they were counted. A command emitting megabytes a second is the
/// one this has to cost nothing for.
#[derive(Debug, Default)]
struct Printing {
    /// The last whole lines, oldest first, at most [`SAMPLE`] of them.
    lines: VecDeque<String>,
    /// The line still arriving, shown as the sample's last row. A command that
    /// rewrites this line rather than ending it — a progress bar — replaces it.
    partial: String,
    /// How many lines there have been, including every one no longer held.
    counted: usize,
    /// How many bytes there have been, for the same reason.
    bytes: usize,
    /// Whether a carriage return is standing, so the next character written
    /// starts the line again. A progress bar is the reason: it returns to the
    /// start of the line and writes over what is there.
    rewriting: bool,
    /// Bumped by every piece that arrives, so the loop above can key a redraw on
    /// it without holding a copy of the rows to compare against.
    changed: u64,
}

impl Printing {
    /// Takes what the call has printed since the last piece.
    fn took(&mut self, text: &str) {
        self.changed = self.changed.wrapping_add(1);
        self.bytes = self.bytes.saturating_add(text.len());

        for character in text.chars() {
            match character {
                '\n' => {
                    self.counted = self.counted.saturating_add(1);
                    self.rewriting = false;
                    if self.lines.len() >= SAMPLE {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(std::mem::take(&mut self.partial));
                }
                // Not the line ending and not the line gone: a carriage return
                // puts the next character back at the start of the line, and
                // what is on the line stays there until something writes over
                // it. That is what a progress bar does, and a return read as
                // "clear it" would leave the sample blank between two frames of
                // one.
                '\r' => self.rewriting = true,
                _ => {
                    if self.rewriting {
                        self.partial.clear();
                        self.rewriting = false;
                    }

                    // Past the bound the rest of the line is dropped rather than
                    // the front of it: the front is what a row shows.
                    if self.partial.len() < LINE {
                        self.partial.push(character);
                    }
                }
            }
        }
    }

    /// How many lines there have been, the one still arriving included.
    fn lines(&self) -> usize {
        self.counted
            .saturating_add(usize::from(!self.partial.is_empty()))
    }

    /// The sample and the count under it, in `spare` rows or not at all.
    ///
    /// Never the sample without the count: five rows out of forty-one, with
    /// nothing saying so, is a reader who believes they are looking at the whole
    /// of what the command has printed.
    fn rows(&self, columns: usize, spare: usize, style: Style, backgroundable: bool) -> Vec<Row> {
        // One row for the offer, whatever else there is room for. It is drawn from
        // the moment the call stands, because a command that has printed nothing
        // for half a minute is the one somebody most wants to put down — and a
        // command that has printed something is the one whose counts go in front
        // of the same offer.
        if spare == 0 {
            return Vec::new();
        }

        let has_caption = backgroundable || self.lines() > 0;
        let held = spare.saturating_sub(usize::from(has_caption)).min(SAMPLE);

        let glyphs = style.glyphs();
        let room = columns.saturating_sub(INSET);
        let inset = " ".repeat(INSET.min(columns));

        // The line still arriving is the last row, where there is one: a command
        // part way through writing a line has written it, and a reader watching
        // wants what is on screen now rather than the last line it finished.
        let mut shown: Vec<&String> = self.lines.iter().collect();
        if !self.partial.is_empty() {
            shown.push(&self.partial);
        }
        shown.truncate(shown.len().min(held));

        // The last of them. What a build is doing now is the question the sample
        // answers, and its first five lines answered it a minute ago.
        let from = shown.len().saturating_sub(held);
        let mut rows: Vec<Row> = shown
            .get(from..)
            .unwrap_or_default()
            .iter()
            .map(|line| {
                Row::new().then(
                    Slot::Quiet,
                    format!("{inset}{}", draw::indented(line, room, glyphs)),
                )
            })
            .collect();

        // What the row says, and what it always says: the counts where there are
        // any, and the offer either way. A command silent for half a minute is the
        // one somebody most wants to put down, so the offer cannot wait for output
        // to justify itself.
        let said = match (self.lines(), backgroundable) {
            (0, false) => return rows,
            (0, true) => BACKGROUND.to_owned(),
            (1, false) => format!("1 line {} {}", glyphs.dot(), sized(self.bytes)),
            (1, true) => format!("1 line {} {} {BACKGROUND}", glyphs.dot(), sized(self.bytes)),
            (lines, false) => format!("{lines} lines {} {}", glyphs.dot(), sized(self.bytes)),
            (lines, true) => format!(
                "{lines} lines {} {} {BACKGROUND}",
                glyphs.dot(),
                sized(self.bytes)
            ),
        };

        // Indented with the sample, and cut the same way it is: it is a caption on
        // the rows above it rather than a row of its own, so it starts in the
        // column they start in. Clipped before the inset is put in front of it,
        // because what tidies a row's ends would take the inset for one of them.
        rows.push(Row::new().then(
            Slot::Quiet,
            format!("{inset}{}", draw::clipped(said, room, glyphs)),
        ));

        rows
    }
}

/// A count of bytes, in the units somebody reads it in.
///
/// Deliberately the shape of the token count on the row below — exact while it
/// is small, one decimal of the larger unit after that — because the two are
/// read together and a pair of numbers written two ways reads as two kinds of
/// fact. Not that function: this one carries a unit, and a space before it.
fn sized(bytes: usize) -> String {
    for (unit, over) in [("MB", 1_000_000), ("kB", 1_000)] {
        if bytes >= over {
            let (whole, tenth) = (bytes / over, (bytes % over) * 10 / over);

            return match tenth {
                0 => format!("{whole} {unit}"),
                _ => format!("{whole}.{tenth} {unit}"),
            };
        }
    }

    format!("{bytes} B")
}

/// Everything the footing is drawn from, coarsened to what it is drawn *by*.
///
/// The clock counts and the marks move while nothing arrives, so the loop above
/// redraws on its own — and this is what it asks about first. A value that has
/// not moved is a frame nobody would be able to tell from the last, so anything
/// the rows say has to be in here: a segment left out is one that changes on
/// screen only when something else happens to change with it.
#[derive(Debug, PartialEq, Eq)]
struct Drawn {
    /// The word the row says.
    doing: Doing,
    /// The count beside it.
    spent: Option<u64>,
    /// The remaining-window fact in the prompt belonging to this live foot.
    ///
    /// Part of what decides a redraw, because anything the row says and this
    /// value does not carry reaches the screen only when something else on the
    /// candidate happens to change with it — a stale number, arriving late, in
    /// the prompt somebody is reading to find out what is going on.
    left: Option<u8>,
    /// The clock, and the face both marks are wearing, coarsened to the one
    /// number every unit of them divides.
    beat: u64,
    /// The call standing over the row, where one is.
    calling: Option<String>,
    /// Whether that call carries the background offer under it.
    backgroundable: bool,
    /// How many pieces the running command had printed, which is what stands in
    /// for the sample's rows.
    printed: u64,
    /// Why room is being made, where it is, and how far the notes have got.
    ///
    /// Both, because both are on the row under the word: one decides whether
    /// that row is there at all and the other is the length of the bar on it.
    /// A compaction draws nothing else for as long as it runs, so the something
    /// else these would otherwise wait for is the clock — a bar that moves in
    /// steps of a beat, on the one row that is telling the reader anything.
    making: Option<Compacting>,
    /// How far the notes have got, as the row reads it.
    part: u8,
}

impl Turning {
    /// A turn that starts now, from the session's last request divided by
    /// what holds the window.
    pub(super) fn started(breakdown: Breakdown) -> Self {
        Self {
            since: Instant::now(),
            doing: Doing::Thinking,
            breakdown,
            totals: Totals::new(),
            limits: None,
            making: None,
            part: 0,
            completed: None,
            spent: None,
            calling: VecDeque::new(),
            drawn: None,
            pinned_after: Duration::ZERO,
        }
    }

    /// The same turn, from what the session had used when it left and the
    /// plan windows last reported.
    pub(super) fn using(self, totals: Totals, limits: Option<PlanWindows>) -> Self {
        Self {
            totals,
            limits,
            ..self
        }
    }

    /// The same turn, holding each call back from the row over the working
    /// row until it has been out for `after`.
    ///
    /// Most commands are over in well under a second, and some reaching a
    /// server in two; a row put up for each and taken down again is the screen
    /// blinking on every call. Held back, a call that answers in time goes to
    /// the transcript and nowhere else, and the band above the box stays the
    /// height it was. The working row says `running` throughout, so the wait is
    /// not a stretch of saying nothing.
    pub(super) fn pinning(self, after: Duration) -> Self {
        Self {
            pinned_after: after,
            ..self
        }
    }

    /// Takes the word from one event on its way to the screen, and hands back
    /// the call line that has stopped being live, where one has.
    ///
    /// Every variant is named rather than caught by a rest arm: an event added
    /// later either changes what the turn is doing or does not, and that is a
    /// decision to make here rather than one to inherit.
    pub(super) fn saw(&mut self, event: &Event) -> Vec<Settled> {
        // Before the guard below, because what a turn spent is true whether it
        // is stopping or not — and a turn asked to stop goes on spending until
        // the response in flight has finished arriving. That is the stretch
        // somebody is most likely to be watching the number.
        if let Event::Spent { spend } = event {
            self.spent = Some(spend.tokens());
        }

        // Before the guard below as well, and for the same reason: a turn asked
        // to stop still has a command running, and what it prints while it is
        // being stopped is the last thing anybody wants hidden.
        if let Event::Wrote { call, text } = event
            && let Some(calling) = self.calling.iter_mut().find(|one| one.id == *call)
        {
            calling.printing.took(text.as_str());
        }

        // Before it as well, and for a sharper reason. A turn asked to stop
        // still has its tool out, and that tool still answers; a turn that ends
        // or fails with one out never gets an answer at all. Either way the
        // line has to come back, or a call that was made leaves no record —
        // which is the one thing a transcript may not do.
        let returned = match event {
            Event::ToolRequested {
                call,
                summary,
                backgroundable,
                looking,
                alone,
            } => {
                let said = draw::called(call, summary);
                if let Some(calling) = self.calling.iter_mut().find(|one| one.id == call.id) {
                    calling.said = said;
                    calling.backgroundable = *backgroundable;
                    calling.looking = *looking;
                    Vec::new()
                } else if *alone && !*backgroundable && looking.is_none() {
                    // Nothing about this one moves, so the footing has nothing
                    // to draw again: it prints no output to watch, no key
                    // points at it, and the words on its row are settled here
                    // and say the same thing when the tool answers. What the
                    // footing was buying was the promise that a result is
                    // drawn under the call it answers, and a transcript that
                    // only grows at the end can keep that promise for one
                    // outstanding call by writing the call now and the answer
                    // after it. So it goes down where it was asked for, and a
                    // reader watching a fetch that takes half a minute reads
                    // it in the transcript rather than off the bottom of the
                    // screen.
                    //
                    // Only when it is the only call of its pass. Four calls
                    // announced together answer in whatever order they
                    // finish, and four rows written up front would take the
                    // four results in the wrong order under the wrong calls.
                    // Those still stand until each is answered.
                    //
                    // And never a call that only looked around: that one is
                    // owed to the run being counted, which counts what has
                    // come back rather than what has gone out.
                    vec![Settled {
                        call: call.id.clone(),
                        said,
                        looking: *looking,
                    }]
                } else {
                    self.calling.push_back(Calling {
                        id: call.id.clone(),
                        asked: Instant::now(),
                        said,
                        printing: Printing::default(),
                        backgroundable: *backgroundable,
                        looking: *looking,
                        name: draw::pascal(&call.name),
                    });
                    Vec::new()
                }
            }
            Event::ToolFinished { call, output, .. } => self
                .calling
                .iter()
                .position(|one| one.id == *call)
                .and_then(|at| self.calling.remove(at))
                .map_or_else(Vec::new, |calling| {
                    let mut returned = settled(calling);
                    // A failed lookup needs its own result row, even in a batch
                    // of successful reads. The grouping promise covers work
                    // that came back, not an error the reader needs to see.
                    if output.is_failed() {
                        returned.looking = None;
                    }
                    vec![returned]
                }),
            // Terminal events cannot leave live rows behind. Drain every call in
            // request order: none has a result to consume another call's line.
            Event::TurnFinished { .. } | Event::Failed { .. } => {
                self.calling.drain(..).map(settled).collect()
            }
            // A steered line is committed to the transcript by the loop that
            // reads it, not here: the queue it leaves is that loop's to draw.
            Event::TurnStarted { .. }
            | Event::PromptCache { .. }
            | Event::Sandbox { .. }
            | Event::Delta { .. }
            | Event::Spent { .. }
            | Event::Carried { .. }
            | Event::Compacting { .. }
            | Event::Compacted { .. }
            | Event::Wrote { .. }
            | Event::Steered { .. }
            | Event::Aged { .. }
            | Event::Unread { .. }
            | Event::FastRefused { .. }
            | Event::Used { .. }
            | Event::PlanLimits { .. }
            | Event::Retrying => Vec::new(),
        };

        // These are facts rather than the activity word below. A turn asked to
        // stop keeps reporting them until the response in flight is actually
        // over, so freezing them would leave the next prompt with stale room.
        match event {
            Event::Carried { breakdown } => {
                self.breakdown = *breakdown;
            }
            Event::Used { totals } => self.totals = *totals,
            Event::PlanLimits { windows } => self.limits = Some(windows.clone()),
            Event::Compacting { why, part } => {
                self.making = Some(*why);
                self.part = (*part).min(99);
                self.completed = None;
            }
            // Keep completion in the footing long enough to be perceived. The
            // runner posts that fact first; this assignment is defensive against
            // an older or synthetic producer that sends only `Compacted`, and has
            // no effect without a progress row to complete.
            Event::Compacted { .. } => {
                self.part = 100;
                self.completed = self.making.map(|_| Instant::now());
            }
            _ => {}
        }

        // A turn that has been asked to stop is stopping whatever else it is
        // still reporting. The deltas already in flight arrive after the key,
        // and a row that went back to `writing` would be saying the key missed.
        if self.doing == Doing::Interrupting {
            return returned;
        }

        self.doing = match event {
            Event::Delta { .. } => Doing::Writing,
            Event::ToolRequested { .. } | Event::Wrote { .. } => Doing::Running,
            // Room having been made puts the turn back where a finished tool
            // does: waiting on the model, with the next request not yet asked.
            Event::ToolFinished { .. } if !self.calling.is_empty() => Doing::Running,
            Event::ToolFinished { .. } | Event::Compacted { .. } => Doing::Thinking,
            // The same request going out again, this time at standard speed.
            Event::Retrying | Event::FastRefused { .. } => Doing::Retrying,
            Event::Compacting { .. } => Doing::Compacting,
            Event::TurnStarted { .. }
            | Event::PromptCache { .. }
            | Event::Sandbox { .. }
            | Event::Spent { .. }
            | Event::Carried { .. }
            | Event::Steered { .. }
            | Event::Aged { .. }
            | Event::Unread { .. }
            | Event::Used { .. }
            | Event::PlanLimits { .. }
            | Event::TurnFinished { .. }
            | Event::Failed { .. } => self.doing,
        };

        returned
    }

    /// Whether the call at the front of the turn can be left running.
    ///
    /// The key reader asks the same live call the hint is drawn for, so a key
    /// pressed with nothing backgroundable out cannot leave a request behind
    /// for a later Bash call to consume. The live call rather than the
    /// [standing](Self::standing) one: the hint waits out
    /// `output.pinAfterSeconds`, and the key does not.
    pub(super) fn can_background(&self) -> bool {
        self.calling
            .front()
            .is_some_and(|calling| calling.backgroundable)
    }

    /// Says the turn has been asked to stop.
    pub(super) fn interrupting(&mut self) {
        self.doing = Doing::Interrupting;
    }

    /// Whether the row would now be drawn differently from the last one drawn,
    /// recording this one as drawn.
    ///
    /// What the loop above redraws on between events. The beat is the coarsest
    /// thing the clock is read by, so it stands in for the clock and for the
    /// face the mark is wearing; the other two are what the row says beside
    /// them.
    pub(super) fn moved(&mut self) -> bool {
        let now = Drawn {
            doing: self.shown_doing(),
            left: self.breakdown.left(),
            spent: self.spent,
            beat: Working::beat(self.running()),

            // Cloned rather than borrowed, because it is kept until the next
            // frame to be compared against. A tool's name and a path is tens of
            // bytes, taken at most four times a second, and it is freed the
            // moment the tool answers — nothing here grows with the transcript.
            //
            // What the row will say rather than which call is at the front of
            // the queue. A pass of eight that has had three answered still has
            // the same call in front and says a different number, and a key
            // that carried the front call's name would leave that number to
            // reach the screen on the next beat: right twice a second by
            // accident, which is not the same as right.
            calling: self.standing().map(|calling| {
                if self.folds() {
                    self.outstanding()
                } else {
                    calling.said.text().to_owned()
                }
            }),
            backgroundable: self
                .standing()
                .is_some_and(|calling| calling.backgroundable),

            // A number rather than the rows themselves. The rows change on every
            // piece a command prints, so comparing them would mean holding a
            // copy of the sample for the length of every frame — and a counter
            // that moves whenever they do answers the only question the loop is
            // asking.
            printed: self
                .standing()
                .map_or(0, |calling| calling.printing.changed),

            making: self.making,
            part: self.part,
        };
        let moved = self.drawn.as_ref() != Some(&now);

        self.drawn = Some(now);
        moved
    }

    /// Clears live completion after its 100% dwell has elapsed.
    ///
    /// Called by the rendering owner, not by the event handler: receiving
    /// [`Event::Compacted`] and removing its bar in one pass would leave no
    /// perceptible completion. Idempotent so timeout frames and terminal failures
    /// cannot resurrect or advance anything.
    pub(super) fn finished_frame(&mut self, now: Instant) {
        let Some(completed) = self.completed else {
            return;
        };
        if now.saturating_duration_since(completed) < COMPLETE_FOR {
            return;
        }

        self.making = None;
        self.completed = None;
        // `moved` stored the 100% picture immediately before this state was
        // cleared. Invalidate it so the next beat cannot compare the no-bar
        // state to an older equal beat and leave completion standing.
        self.drawn = None;
    }

    /// Says the last frame drawn is no longer what the footing would draw.
    ///
    /// For what changes the footing without changing the turn: the queue under
    /// the working row is the loop's, and nothing [`Turning::moved`] compares
    /// says it has a line more or one fewer.
    pub(super) fn redraw(&mut self) {
        self.drawn = None;
    }

    /// How much longer a completed bar must remain on screen.
    pub(super) fn completion_wait(&self, now: Instant) -> Option<Duration> {
        self.completed
            .map(|completed| COMPLETE_FOR.saturating_sub(now.saturating_duration_since(completed)))
    }

    /// The activity word to draw while completion has its short dwell.
    fn shown_doing(&self) -> Doing {
        if self.doing == Doing::Interrupting {
            Doing::Interrupting
        } else if self.completed.is_some() {
            Doing::Compacting
        } else {
            self.doing
        }
    }

    /// The usable room left before compaction, read off [`Turning::breakdown`]
    /// so the prompt line and `/context` cannot disagree.
    pub(super) const fn left(&self) -> Option<u8> {
        self.breakdown.left()
    }

    /// The last request divided by what holds the window: the one the turn
    /// started from, then each [`Event::Carried`] since. What `/context`
    /// shows while the runner is away on the turn.
    pub(super) const fn breakdown(&self) -> Breakdown {
        self.breakdown
    }

    /// What the session has used: what it had when the turn started, then
    /// each [`Event::Used`] since.
    pub(super) const fn totals(&self) -> Totals {
        self.totals
    }

    /// The plan windows last reported: those the turn started with, then each
    /// [`Event::PlanLimits`] since.
    pub(super) fn limits(&self) -> Option<PlanWindows> {
        self.limits.clone()
    }

    /// The row that says the turn is running, as the footing draws it.
    pub(super) fn working(&self, columns: usize, style: Style) -> Row {
        Working {
            doing: self.shown_doing().word(),
            running: self.running(),
            spent: self.spent,
            stops: (self.doing != Doing::Interrupting).then_some(STOPS),
        }
        .row(columns, style.glyphs())
    }

    /// The rows to put above the box, or none where the window has no room:
    /// the turn's, laid out at the transcript's width, then those over them,
    /// laid out at the window's.
    ///
    /// A blank either side, so the rows belong to neither the turn's own output
    /// above them nor the box below, and a blank between the call and the row
    /// under it for the same reason: the call is a thing that is happening and
    /// the row is what is happening to the turn. `room` is what is left of the
    /// window once the box has taken its share: dropped whole rather than
    /// squeezed, because a footing taller than the band it is drawn in writes
    /// into the band underneath, and one row of turn output is worth more than
    /// a clock.
    ///
    /// Where prompts are waiting, the working row stands over the queue's panel
    /// and both stand across the whole window, the rule over the panel from
    /// edge to edge: the row and the panel are about what comes next, not lines
    /// of the transcript, so they take no rail. The blank over the row is still
    /// the transcript's, and is drawn with it.
    ///
    /// The plan goes under all of it, directly over the box, and it is laid out
    /// here rather than by the caller so that the whole of that arithmetic is
    /// one function. What it stands under is the turn; what it stands over is
    /// the line being typed while the turn runs — and the panel between them
    /// says which of the plan's tasks that turn is on.
    // Seven with the turn itself is two over clippy's limit, and each is a
    // distinct thing the layout is measured against: the plan, the run of
    // lookups, the queue, the widths, the dress and the rows left. The two
    // widths already travel as one, because they always arrive together;
    // bundling any other pair would make a type that exists only to satisfy
    // the count.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rows(
        &self,
        planning: &Planning,
        counting: &str,
        queued: &Prompts,
        widths: Widths,
        style: Style,
        room: usize,
    ) -> (Vec<Row>, Vec<Row>) {
        let Widths { columns, window } = widths;
        if room <= ROWS {
            return (Vec::new(), Vec::new());
        }

        // Offered what is left once the row saying a turn is running has taken
        // its three and the footing has left the window its one. That is what
        // puts the plan last in the order things give way: it is measured
        // first, so the call line and the prompt waiting are the two measured
        // against what it left, and a window short of rows drops those before
        // the panel gives up a task.
        let mut panel = planning.rows(columns, room - ROWS - 1, style.glyphs());
        let room = room - panel.len();

        // What the call has to clear is taller where the queue's panel below is
        // being drawn, since the two are standing in the same window. The panel
        // is measured against what the working row and the bar under it leave,
        // with the blank over them and the one row the footing leaves the
        // window. Its own last blank stands for the footing's where no plan is
        // under it. What it cannot fit it does not draw — a queue is still in
        // the queue, and its own turn will say it.
        let spare = room
            .saturating_sub(ROWS + 1 + usize::from(self.making.is_some()))
            .saturating_add(usize::from(panel.is_empty()));
        let panel_rows = queueing::panel(queued, window, spare, style);
        let calling = self.standing();
        let standing = if calling
            .is_some_and(|calling| calling.backgroundable || calling.printing.lines() > 0)
        {
            CALLING
        } else {
            CALLING.saturating_sub(1)
        } + panel_rows.len();

        // And taller again by the one row a live run of calls that only looked
        // around stands in. It is counted with the call rather than measured
        // against it, because the two are one turn doing one thing: a window
        // that can hold only one of them holds neither, since the pair is what
        // says what is going on.
        let run = (!counting.is_empty()).then(|| self.counted(counting, columns, style));
        let standing = standing + usize::from(run.is_some());

        let mut rows = Vec::new();

        if room > standing
            && let Some(run) = run
        {
            rows.push(Row::new());
            rows.push(run);
        }

        if let Some(calling) = calling.filter(|_| room > standing) {
            if rows.is_empty() {
                rows.push(Row::new());
            }

            // One row either way. A pass that asked for one tool names it, and
            // a pass that asked for eight says how many of what: the first of
            // the eight would otherwise stand here until it answered, which is
            // one URL held in front of a reader for as long as the whole batch
            // takes and no word about the seven behind it.
            //
            // Not where anything out can be backgrounded or has printed. The
            // key points at the row that names a command, and a count is not
            // something it can act on; the sample under it belongs to a call
            // rather than to a number.
            if self.folds() {
                rows.push(self.counted(&self.outstanding(), columns, style));
            } else {
                rows.push(self.call(&calling.said, columns, style));

                // Measured last, against whatever every other row left. It is
                // the one thing here a second look gets back whatever the
                // window did — the key that stands a result whole stands this
                // too — so it is the first to give way and it gives way
                // without saying so.
                // What is left after every row that never gives way has taken
                // its own is the sample's, and the sample's alone: the caption
                // under the call is counted in `standing` above whenever there
                // is one. For a backgroundable call that is the offer; for any
                // call that printed it is the bounded count that keeps a
                // sample from reading as the whole.
                rows.extend(calling.printing.rows(
                    columns,
                    room.saturating_sub(standing),
                    style,
                    calling.backgroundable,
                ));
            }
        }

        rows.push(Row::new());

        // The rest stand over the transcript's band where the panel does, and
        // in it where nothing is waiting — or where the window had no room to
        // name anything that is.
        let across = if panel_rows.is_empty() {
            columns
        } else {
            window
        };
        let mut over = Vec::new();
        let under = if panel_rows.is_empty() {
            &mut rows
        } else {
            &mut over
        };
        under.push(self.working(across, style));

        // Under the word and with no blank between them, because it is a second
        // line of the same thing rather than a second thing beside it.
        if self.making.is_some()
            && let Some(row) = making(self.part, across, style)
        {
            under.push(row);
        }

        // The blank under the footing parts it from the line below it. The
        // panel ends in a blank of its own, so it needs no second one; a plan
        // under it is a thing beside it, so that blank stays.
        let parted = panel_rows.is_empty() || !panel.is_empty();
        under.extend(panel_rows);
        under.append(&mut panel);
        if parted {
            under.push(Row::new());
        }

        (rows, over)
    }
}

/// The two widths the rows over the box are laid out at.
///
/// Two because they stand in two bands: the turn's own rows read as the
/// transcript's last rows, with the scroll rail beside them, and what stands
/// over the transcript — the queue's panel, and the row over it — takes the
/// whole window.
#[derive(Debug, Clone, Copy)]
pub(super) struct Widths {
    /// The transcript's.
    pub(super) columns: usize,
    /// The window's.
    pub(super) window: usize,
}

/// The bar under the word while room is being made, or nothing where there is
/// no room for one or nothing to measure yet.
///
/// The bar is the whole row, starting in the column the word above it starts
/// in, so it reads as a second line of the same thing. It says how far the
/// notes have got, which is the one thing the reader cannot see for themselves.
/// Nothing has arrived while `part` is 0 — the model is reading a session it
/// has not begun writing down, which on a full window is seconds — so there is
/// no bar to draw and no row.
fn making(part: u8, columns: usize, style: Style) -> Option<Row> {
    let glyphs = style.glyphs();
    let gutter = Working::gutter(glyphs);
    let available = columns
        .saturating_sub(gutter)
        .saturating_sub(BAR_TAIL)
        .min(BAR_MAX);

    if part == 0 || available < BAR {
        return None;
    }

    // Integer cells have no exact third where the available width is not a
    // multiple of three, so the remainder is deliberately rounded down: the row
    // is never wider than two thirds of the one it replaces.
    let bar = available * BAR_NUMERATOR / BAR_DENOMINATOR;
    let full = usize::from(part.min(100)) * bar / 100;
    let row = Row::new()
        .then(Slot::Quiet, " ".repeat(gutter))
        .then(Slot::Plain, glyphs.filled().repeat(full))
        .then(Slot::Quiet, glyphs.hollow().repeat(bar - full))
        .then(Slot::Quiet, format!("  {part}%"));

    (row.columns() <= columns).then_some(row)
}

impl Turning {
    /// The line for the call whose tool is out.
    ///
    /// The dot appears and disappears on the same beat as the turn's own mark.
    /// Visibility supplies the motion; when visible it stays in the reader's own
    /// foreground, as the committed line's mark does, instead of cycling through
    /// colours. Its empty face is a space in
    /// that same one-column field, so the command does not move between frames
    /// or when the live call becomes a committed one.
    ///
    /// The words go through the same clipping the committed line uses, so no
    /// face can make the terminal wrap a row the renderer counted as one.
    /// Whether what is out is better said as a count than as one of its rows.
    ///
    /// Two or more, and nothing among them the footing is holding for its own
    /// sake: a command the background key points at, or a call part way
    /// through printing something worth watching.
    fn folds(&self) -> bool {
        self.calling.len() > 1
            && !self
                .calling
                .iter()
                .any(|calling| calling.backgroundable || calling.printing.lines() > 0)
    }

    /// What is out right now, counted by tool.
    ///
    /// In the order the calls were asked for rather than by how many there are
    /// of each, because that order is the model's sentence about what it is
    /// doing and a reader who saw the turn start recognises it.
    ///
    /// Tool names rather than a verb for each, unlike the run that settles into
    /// the transcript. That run counts the four kinds of looking-around the
    /// tools declare, and this counts whatever is out — a tool added tomorrow
    /// has a name here and would have no sentence.
    fn outstanding(&self) -> String {
        let mut counted: Vec<(&str, usize)> = Vec::new();

        for calling in &self.calling {
            match counted
                .iter_mut()
                .find(|(name, _)| *name == calling.name.as_str())
            {
                Some((_, count)) => *count += 1,
                None => counted.push((calling.name.as_str(), 1)),
            }
        }

        let said: Vec<String> = counted
            .into_iter()
            .map(|(name, count)| format!("{count} {name}"))
            .collect();

        match said.split_last() {
            None => String::new(),
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        }
    }

    fn call(&self, said: &draw::Called, columns: usize, style: Style) -> Row {
        let row = Row::new()
            .then(Slot::Plain, self.mark(style))
            .clipped(columns);

        match draw::words(said, columns, style) {
            words if words.is_empty() || row.is_empty() => row,
            words => row.then(Slot::Plain, " ").join(words),
        }
    }

    /// The line saying what a live run of calls that only looked around has
    /// come to so far.
    ///
    /// It wears the mark of the call under it, on the same beat, because the
    /// two are one turn doing one thing: this row says what the run has
    /// amassed and the row below says which of it is out right now.
    ///
    /// The words are the run's own sentence rather than a call's name, so they
    /// go in the plain slot and not the one [`draw::words`] puts a tool in.
    /// Nothing is behind this row to open yet — the results it counts are
    /// still being collected — and the line the transcript writes when the run
    /// settles is the one that offers them.
    fn counted(&self, counting: &str, columns: usize, style: Style) -> Row {
        let glyphs = style.glyphs();
        let room = columns.saturating_sub(crucible_tui::columns(glyphs.called()) + 1);
        let row = Row::new()
            .then(Slot::Plain, self.mark(style))
            .clipped(columns);

        match draw::clipped(counting, room, glyphs) {
            words if words.is_empty() || row.is_empty() => row,
            words => row.then(Slot::Plain, " ").then(Slot::Plain, words),
        }
    }

    /// The mark a live row wears this beat, or the one column of space it
    /// stands in when the beat has it hidden.
    ///
    /// Read once per layout and handed to every row that wants it, so the rows
    /// of one frame are all of one instant. Two rows blinking against each
    /// other would read as two turns rather than one.
    fn mark(&self, style: Style) -> &'static str {
        let visible = Working::beat(self.running()).is_multiple_of(2);
        if visible {
            style.glyphs().called()
        } else {
            " "
        }
    }

    /// The call drawn over the row, where one has been out long enough to be.
    ///
    /// The front of the queue, because that is the call asked for first and so
    /// the one out longest: where it is too young to stand, every call behind
    /// it is younger still. Read off the clock rather than kept, so the frame
    /// at which it comes due is one [`Turning::moved`] sees change, and the
    /// loop that asks that sixty times a second draws it without waiting for
    /// anything else to happen.
    fn standing(&self) -> Option<&Calling> {
        self.calling
            .front()
            .filter(|calling| calling.asked.elapsed() >= self.pinned_after)
    }

    /// How long the turn has been running.
    fn running(&self) -> Duration {
        self.since.elapsed()
    }
}

#[cfg(test)]
mod tests;
