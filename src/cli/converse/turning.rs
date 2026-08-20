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
//! pulses, and it lives here rather than in scrollback because a line the
//! renderer moves back over cannot also be committed. When the tool answers the
//! line is handed back — [`Turning::saw`] returns it — and whoever drives this
//! commits it still, so what reaches scrollback is the same words with the
//! motion gone.
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

use crucible_core::{Compacting, Event};
#[cfg(test)]
use crucible_tui::Theme;
use crucible_tui::{Row, Slot, Working};

use super::super::draw;
use super::super::style::Style;
use super::planning::Planning;

/// How the turn is asked to stop, said after the clock.
const STOPS: &str = "esc to interrupt";

/// What the row under the working row calls the prompt waiting behind the turn.
const NEXT: &str = "Next:";

/// The rows this puts above the box, blanks included.
const ROWS: usize = 3;

/// And with a prompt waiting behind the turn, the row that names it.
///
/// One row rather than two: it goes directly under the row it belongs to, the
/// way a call's result goes directly under the call. A blank between them would
/// make it a second thing on the screen rather than a second line of the first.
const QUEUED: usize = ROWS + 1;

/// And with a call standing over it: the blank that parts them, the call, and the
/// row under it offering to leave the command running.
///
/// Three rather than two, because the offer is drawn from the moment the call is
/// out. It is one row and it is the only thing on screen that says what a key
/// would do about the command in front of you, so it belongs with the call rather
/// than with the sample the call's output gives way as.
///
/// The call line is what a narrow window gives up first, because it is the one
/// of the four that a second look gets back anyway: the tool answers and the
/// line is committed to scrollback either way. The prompt waiting goes next,
/// since it is still in the queue and its own turn will say it. Then the plan,
/// which gives up its own rows a state at a time before it goes entirely — and
/// it is measured before either of the other two, so those are what a window
/// short of rows drops on its behalf. The row saying a turn is running exists
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

/// How wide the bar under the word is, in columns.
///
/// The same figure the panel that offers to make room uses, because the two
/// pictures are one picture at two moments and a bar that changed width between
/// them would read as a different thing.
const BAR: usize = 28;

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
    /// How much of the model's window is left, or `None` where no window is
    /// known and while room is being made.
    left: Option<u8>,
    /// Why room is being made, and `None` when it is not.
    ///
    /// What the row under the word says while it happens — the reason rather
    /// than a fixed sentence, because a window that filled and a provider that
    /// refused are different things to be told, and neither is true when
    /// somebody simply asked.
    making: Option<Compacting>,
    /// How much of the notes has been written, as a percentage.
    ///
    /// What the bar under the word fills to. A fraction of the room the notes
    /// were given rather than of how long it will take — the answer is arriving
    /// and this is how much of it has, which is the only thing here that is
    /// actually known.
    part: u8,
    /// The words of the call whose tool is out, or `None` where none is. What
    /// the tool said the call was about, without the mark: the mark is the part
    /// that moves, and it is drawn per frame.
    calling: Option<String>,
    /// The prompt waiting behind this turn, already cut to a row, or `None`
    /// where none is. Cut on the way in rather than on the way out for the
    /// reason [`Turning::queueing`] gives.
    queued: Option<String>,
    /// What the call whose tool is out has printed, kept to the last rows of
    /// it. Emptied when the call comes back, so it holds one call's worth and
    /// never a turn's.
    printing: Printing,
    /// What the footing was last drawn from, so a redraw that would draw the
    /// same rows again can be skipped. `None` before the first.
    drawn: Option<Drawn>,
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

    /// Forgets it, which is what the call coming back means.
    ///
    /// The count moves only where something was on screen to take back. Every
    /// turn ends, and a turn that never ran a command would otherwise get a frame
    /// out of this — a redraw with nothing behind it, at the moment the region is
    /// being handed back, which scrolls the terminal by a row nobody asked for.
    fn clear(&mut self) {
        let shown = !self.is_empty();

        *self = Self {
            changed: if shown {
                self.changed.wrapping_add(1)
            } else {
                self.changed
            },
            ..Self::default()
        };
    }

    /// Whether the command has printed anything at all.
    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.partial.is_empty()
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
    fn rows(&self, columns: usize, spare: usize, style: Style) -> Vec<Row> {
        // One row for the offer, whatever else there is room for. It is drawn from
        // the moment the call is out, because a command that has printed nothing
        // for half a minute is the one somebody most wants to put down — and a
        // command that has printed something is the one whose counts go in front
        // of the same offer.
        if spare == 0 {
            return Vec::new();
        }

        let held = spare.saturating_sub(1).min(SAMPLE);

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
        shown.truncate(shown.len().min(held.max(1)));

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
        let said = match self.lines() {
            0 => BACKGROUND.to_owned(),
            1 => format!("1 line {} {} {BACKGROUND}", glyphs.dot(), sized(self.bytes)),
            lines => format!(
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
    /// The reading against the far end of it.
    ///
    /// Part of what decides a redraw, because anything the row says and this
    /// value does not carry reaches the screen only when something else on the
    /// row happens to change with it — a stale number, arriving late, on the
    /// row somebody is reading to find out what is going on.
    left: Option<u8>,
    /// The clock, and the face both marks are wearing, coarsened to the one
    /// number every unit of them divides.
    beat: u64,
    /// The call standing over the row, where one is.
    calling: Option<String>,
    /// The prompt named under it, where one is waiting.
    queued: Option<String>,
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
    /// A turn that starts now.
    pub(super) fn started() -> Self {
        Self {
            since: Instant::now(),
            doing: Doing::Thinking,
            left: None,
            making: None,
            part: 0,
            spent: None,
            calling: None,
            queued: None,
            printing: Printing::default(),
            drawn: None,
        }
    }

    /// Takes the prompt waiting behind the turn, for the row that names it.
    ///
    /// Cut to the window here rather than where the row is drawn, because what
    /// is kept is compared against the last frame's copy sixty times a second
    /// and a prompt is bounded by the editor's ceiling rather than by a row —
    /// that ceiling is a megabyte. What is held is a row's worth of it, which
    /// is what the call line beside it holds too.
    ///
    /// Called again when the window changes, so a terminal made wider is not
    /// left reading a line cut for a narrower one. Cutting twice costs nothing
    /// and says the same thing: the second cut is the narrower of the two.
    pub(super) fn queueing(&mut self, waiting: Option<&str>, columns: usize, style: Style) {
        self.queued = waiting.map(|said| draw::clipped(said, columns, style.glyphs()));
    }

    /// Takes the word from one event on its way to the screen, and hands back
    /// the call line that has stopped being live, where one has.
    ///
    /// Every variant is named rather than caught by a rest arm: an event added
    /// later either changes what the turn is doing or does not, and that is a
    /// decision to make here rather than one to inherit.
    pub(super) fn saw(&mut self, event: &Event) -> Option<String> {
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
        if let Event::Wrote { text, .. } = event {
            self.printing.took(text.as_str());
        }

        // Before it as well, and for a sharper reason. A turn asked to stop
        // still has its tool out, and that tool still answers; a turn that ends
        // or fails with one out never gets an answer at all. Either way the
        // line has to come back, or a call that was made leaves no record —
        // which is the one thing a transcript may not do.
        let returned = match event {
            Event::ToolRequested { call, summary } => {
                self.calling = Some(draw::called(call, summary));
                None
            }
            Event::ToolFinished { .. } | Event::TurnFinished { .. } | Event::Failed { .. } => {
                // The sample goes with the line it stood under. What the command
                // printed is in the result about to be committed, and behind the
                // key that stands a result whole — held here as well, it would be
                // rows above the box describing a call that has answered.
                self.printing.clear();
                self.calling.take()
            }
            Event::TurnStarted { .. }
            | Event::Delta { .. }
            | Event::Spent { .. }
            | Event::Carried { .. }
            | Event::Compacting { .. }
            | Event::Compacted { .. }
            | Event::Wrote { .. }
            | Event::Retrying => None,
        };

        // A turn that has been asked to stop is stopping whatever else it is
        // still reporting. The deltas already in flight arrive after the key,
        // and a row that went back to `writing` would be saying the key missed.
        if self.doing == Doing::Interrupting {
            return returned;
        }

        // How full the window is, kept whether or not a turn is running: the
        // reading is on screen from the first frame of the session to the last,
        // and the one moment it is not is while the number it would show is the
        // one being replaced.
        match event {
            Event::Carried { left } => self.left = *left,
            // Reported again as the notes are written, so what the row shows
            // moves rather than sitting still for one request. The reading is
            // taken away for the duration: the number it would show is the one
            // being replaced.
            Event::Compacting { why, part } => {
                self.making = Some(*why);
                self.part = *part;
                self.left = None;
            }
            Event::Compacted { .. } => self.making = None,
            _ => {}
        }

        self.doing = match event {
            Event::Delta { .. } => Doing::Writing,
            Event::ToolRequested { .. } | Event::Wrote { .. } => Doing::Running,
            // Room having been made puts the turn back where a finished tool
            // does: waiting on the model, with the next request not yet asked.
            Event::ToolFinished { .. } | Event::Compacted { .. } => Doing::Thinking,
            Event::Retrying => Doing::Retrying,
            Event::Compacting { .. } => Doing::Compacting,
            Event::TurnStarted { .. }
            | Event::Spent { .. }
            | Event::Carried { .. }
            | Event::TurnFinished { .. }
            | Event::Failed { .. } => self.doing,
        };

        returned
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
            doing: self.doing,
            left: self.left,
            spent: self.spent,
            beat: Working::beat(self.running()),

            // Cloned rather than borrowed, because it is kept until the next
            // frame to be compared against. A tool's name and a path is tens of
            // bytes, taken at most four times a second, and it is freed the
            // moment the tool answers — nothing here grows with the transcript.
            calling: self.calling.clone(),

            // And the same for the prompt waiting, which is why it was cut
            // before it was kept: what is cloned here is a row of it rather
            // than a megabyte of it.
            queued: self.queued.clone(),

            // A number rather than the rows themselves. The rows change on every
            // piece a command prints, so comparing them would mean holding a
            // copy of the sample for the length of every frame — and a counter
            // that moves whenever they do answers the only question the loop is
            // asking.
            printed: self.printing.changed,

            making: self.making,
            part: self.part,
        };
        let moved = self.drawn.as_ref() != Some(&now);

        self.drawn = Some(now);
        moved
    }

    /// The rows to put above the box, or none where the window has no room.
    ///
    /// A blank either side, so the rows belong to neither the turn's own output
    /// above them nor the box below, and a blank between the call and the row
    /// under it for the same reason: the call is a thing that is happening and
    /// the row is what is happening to the turn. The prompt waiting takes no
    /// blank above it, because it is a second line of the row rather than a
    /// second thing beside it. `room` is what is left of the window once the
    /// box has taken its share: dropped whole rather than squeezed, because a
    /// footing taller than the window is a region the renderer cannot rewind
    /// over, and one row of turn output is worth more than a clock.
    ///
    /// The plan goes under all of it, directly over the box, and it is laid out
    /// here rather than by the caller so that the whole of that arithmetic is
    /// one function. What it stands under is the turn; what it stands over is
    /// the line being typed while the turn runs — and the panel between them
    /// says which of the plan's tasks that turn is on.
    pub(super) fn rows(
        &self,
        planning: &Planning,
        columns: usize,
        style: Style,
        room: usize,
    ) -> Vec<Row> {
        if room <= ROWS {
            return Vec::new();
        }

        // Offered what is left once the row saying a turn is running has taken
        // its three and the footing has left the window its one. That is what
        // puts the plan last in the order things give way: it is measured
        // first, so the call line and the prompt waiting are the two measured
        // against what it left, and a window short of rows drops those before
        // the panel gives up a task.
        let mut panel = planning.rows(columns, room - ROWS - 1, style.glyphs());
        let room = room - panel.len();

        let working = Working {
            doing: self.doing.word(),
            running: self.running(),
            spent: self.spent,
            stops: (self.doing != Doing::Interrupting).then_some(STOPS),
            left: self.left,
        };

        // What the call has to clear is a row taller where the prompt below is
        // being drawn, since the two are standing in the same window.
        let queued = self.queued.as_deref().filter(|_| room > QUEUED);
        let standing = if queued.is_some() {
            CALLING + 1
        } else {
            CALLING
        };

        let mut rows = Vec::new();

        if let Some(said) = self.calling.as_deref().filter(|_| room > standing) {
            rows.push(Row::new());
            rows.push(self.call(said, columns, style));

            // Measured last, against whatever every other row left. It is the one
            // thing here a second look gets back whatever the window did — the
            // key that stands a result whole stands this too — so it is the first
            // to give way and it gives way without saying so.
            // What is left after every row that never gives way has taken its
            // own is the sample's, and the sample's alone: the offer under the
            // call is counted in `standing` above, because a call with no way to
            // put it down is the one thing this row must never be.
            rows.extend(
                self.printing
                    .rows(columns, room.saturating_sub(standing), style),
            );
        }

        rows.push(Row::new());
        rows.push(working.row(columns, style.glyphs()));

        // Under the word and with no blank between them, because it is a second
        // line of the same thing rather than a second thing beside it — the
        // rule the prompt waiting behind a turn already keeps.
        if let Some(why) = self.making
            && let Some(row) = making(why, self.part, columns, style)
        {
            rows.push(row);
        }

        if let Some(said) = queued {
            rows.push(next(said, columns, style));
        }

        rows.append(&mut panel);
        rows.push(Row::new());

        rows
    }
}

/// The row under the word while room is being made, or nothing where there is
/// no room for one.
///
/// It says why, because the three reasons are three different things to be told
/// and only one of them is the window having filled.
fn making(why: Compacting, part: u8, columns: usize, style: Style) -> Option<Row> {
    let glyphs = style.glyphs();
    let gutter = Working::gutter(glyphs);
    let said = match why {
        Compacting::Full => "the window was full",
        Compacting::Refused => "the model would not take another request this size",
        Compacting::Asked | Compacting::Resumed => "you asked for this",
    };

    let row = Row::new().then(Slot::Quiet, " ".repeat(gutter));

    // The bar where there is room for one and something to draw in it, and the
    // words alone otherwise: what the reader needs is why this is happening,
    // and the bar is what says how far along it is.
    //
    // Nothing has arrived while `part` is 0, and nothing is what the request is
    // doing — the model is reading a session it has not begun writing down,
    // which on a full window is seconds. An empty bar held through all of them
    // is what a reader calls stuck, and it is claiming a length it does not
    // have; the words claim nothing and say the same thing.
    let row = if part > 0 && columns >= gutter + BAR + 8 {
        let full = usize::from(part) * BAR / 100;
        row.then(Slot::Plain, glyphs.filled().repeat(full))
            .then(Slot::Quiet, glyphs.hollow().repeat(BAR - full))
            .then(Slot::Quiet, format!("  {part}%  {said}"))
    } else {
        row.then(Slot::Quiet, said.to_owned())
    };

    (row.columns() <= columns).then_some(row)
}

impl Turning {
    /// The line for the call whose tool is out.
    ///
    /// The mark pulses rather than turning: a call is one thing waiting on one
    /// answer, and the row below it is already carrying the mark that says the
    /// turn as a whole is moving. Two marks cycling through four faces beside
    /// each other read as two independent things rather than as one inside the
    /// other. On the same beat as that one, so the footing moves as one picture.
    ///
    /// The words go through the same clipping the committed line uses, so the
    /// line does not change shape at the moment it stops moving.
    fn call(&self, said: &str, columns: usize, style: Style) -> Row {
        let lit = Working::beat(self.running()).is_multiple_of(2);
        let slot = if lit { Slot::Accent } else { Slot::Quiet };

        let row = Row::new().then(slot, style.glyphs().called());

        // The mark alone where the window left no room for the words, as the
        // committed line does: the space after it would be the one column a
        // window that narrow has not got.
        match draw::words(said, columns, style) {
            words if words.is_empty() => row,
            words => row.then(Slot::Plain, " ").join(words),
        }
    }

    /// How long the turn has been running.
    fn running(&self) -> Duration {
        self.since.elapsed()
    }
}

/// The row naming the prompt that will be sent once this turn is over.
///
/// It starts in the column the word above it starts in and opens with no mark
/// of its own, because a mark opening a line is what says a thing is happening
/// and this is a thing that has not started. What it is is a second line of the
/// row above rather than a second thing beside it.
///
/// Cut at the right where the prompt is wider than the window, which is the only
/// answer available to it: a prompt is whatever somebody typed, and wrapping it
/// would make the footing's height a function of that.
fn next(said: &str, columns: usize, style: Style) -> Row {
    let glyphs = style.glyphs();
    let gutter = Working::gutter(glyphs);
    let room = columns.saturating_sub(gutter);

    // A window narrower than the column this row starts in. Nothing it could
    // say would be anything but the terminal wrapping it, and a row of spaces
    // is a row the reader is owed an explanation for.
    if room == 0 {
        return Row::new();
    }

    let row = Row::new()
        .then(Slot::Plain, " ".repeat(gutter))
        .then(Slot::Quiet, draw::clipped(NEXT, room, glyphs));

    // The label is what the row is; the prompt is what it says. So a window
    // that has room for one of the two keeps the label, the way the row above
    // keeps its word and drops what is said after it.
    match room.saturating_sub(crucible_tui::columns(NEXT)) {
        left if left > 1 => {
            let said = draw::clipped(said, left - 1, glyphs);
            row.then(Slot::Plain, format!(" {said}"))
        }
        _ => row,
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        Spend, StopReason, Summary, ToolArgs, ToolCall, ToolId, ToolOutput, TurnError, TurnId,
    };
    use crucible_tui::{Glyphs, Palette};

    use super::*;

    /// No plan at all, which is what a session has until the agent writes one
    /// and is what every test here but the last two is about.
    fn nothing() -> Planning {
        Planning::new(crucible_tools::Plan::new())
    }

    /// A plan of `count` open tasks, each named after where it is in the list.
    ///
    /// Written through the tool the way the model writes one, because that is
    /// the only way anything gets into a plan and the panel is drawn from what
    /// came out the other side.
    fn planned(count: usize) -> Planning {
        let said = (0..count)
            .map(|at| format!(r#"{{"task":"Task {at}","state":"open"}}"#))
            .collect::<Vec<_>>()
            .join(",");

        let plan = crucible_tools::Plan::new();
        plan.replay(&ToolArgs::new(format!(r#"{{"tasks":[{said}]}}"#)));

        Planning::new(plan)
    }

    /// The word the row says after `event`, from a turn that just started.
    fn after(event: &Event) -> &'static str {
        let mut turning = Turning::started();
        turning.saw(event);
        turning.doing.word()
    }

    fn requested() -> Event {
        Event::ToolRequested {
            call: ToolCall {
                id: ToolId::new("a"),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            },
            summary: Summary::new("src/main.rs"),
        }
    }

    #[test]
    fn the_word_says_which_of_the_two_things_a_turn_does_is_happening() {
        // Waiting on the model and waiting on a tool are the two, and they are
        // the two because they fail differently: a turn stuck thinking is a
        // provider that has gone quiet, and one stuck running is a command that
        // has not come back. A single word for both would hide which.
        assert_eq!(
            after(&Event::TurnStarted {
                turn: TurnId::FIRST
            }),
            "thinking"
        );
        assert_eq!(after(&Event::Delta { text: "hi".into() }), "writing");
        assert_eq!(after(&requested()), "running");
        assert_eq!(
            after(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            "thinking"
        );
    }

    #[test]
    fn a_response_being_asked_for_again_says_so_until_the_new_one_speaks() {
        // The span it covers is the whole of the second ask — the pause and the
        // request after it — and `thinking` over that span would be a row saying
        // the first answer is still on its way.
        let mut turning = Turning::started();
        turning.saw(&Event::Retrying);

        assert_eq!(turning.doing.word(), "retrying");

        turning.saw(&Event::Delta { text: "hi".into() });
        assert_eq!(turning.doing.word(), "writing");
    }

    #[test]
    fn a_turn_asked_to_stop_goes_on_saying_so_whatever_arrives_after() {
        // The deltas already in flight land after the key. A row that read them
        // and went back to `writing` would be saying the key was missed, at the
        // one moment somebody is watching the row to find out whether it was.
        let mut turning = Turning::started();
        turning.interrupting();
        turning.saw(&Event::Delta { text: "hi".into() });

        assert_eq!(turning.doing.word(), "interrupting");

        // And stops offering the key that has already been pressed.
        let rows = turning.rows(&nothing(), 80, Style::plain(), 24);
        let said = rows.iter().map(Row::text).collect::<String>();

        assert!(said.contains("interrupting"), "{said:?}");
        assert!(!said.contains(STOPS), "{said:?}");
    }

    #[test]
    fn the_row_says_what_the_turn_has_spent_once_the_provider_has_said() {
        // And says nothing in its place until then, which is what every turn
        // looks like until its first response comes back.
        let mut turning = Turning::started();
        let said = |turning: &Turning| {
            turning
                .rows(&nothing(), 80, Style::plain(), 24)
                .iter()
                .map(Row::text)
                .collect::<String>()
        };

        assert!(!said(&turning).contains('↓'), "{:?}", said(&turning));

        turning.saw(&Event::Spent {
            spend: Spend::new(12_800),
        });

        assert!(said(&turning).contains("↓ 12.8k"), "{:?}", said(&turning));
    }

    #[test]
    fn a_turn_asked_to_stop_goes_on_counting_what_it_spends() {
        // The word stops moving when the key is pressed; the count does not.
        // The response already in flight goes on arriving and goes on costing,
        // and that stretch is the one somebody is most likely to be watching
        // the number through.
        let mut turning = Turning::started();
        turning.interrupting();
        turning.saw(&Event::Spent {
            spend: Spend::new(2_900),
        });

        assert_eq!(turning.spent, Some(2_900));
    }

    #[test]
    fn a_row_that_would_be_drawn_the_same_again_is_not_drawn_again() {
        // The whole cost of an animated row on a sixty-times-a-second tick.
        // Without this the box under it is laid out and written on every one of
        // them, to produce the bytes that were already on the screen.
        let mut turning = Turning::started();

        assert!(turning.moved(), "the first row was never drawn");
        assert!(!turning.moved(), "the same row was drawn twice");

        turning.saw(&Event::Delta { text: "hi".into() });
        assert!(turning.moved(), "the word changed and the row did not");

        // And the count is on the row, so it is on the value the loop keys on.
        // Left off, it would reach the screen only on the beat some other
        // segment happened to change — a stale number, arriving late, on the
        // row somebody is reading to find out what is going on.
        turning.saw(&Event::Spent {
            spend: Spend::new(1_400),
        });
        assert!(turning.moved(), "the count changed and the row did not");
    }

    #[test]
    fn the_bar_moves_on_the_notes_rather_than_on_whatever_else_changes() {
        // The bar is a segment of the row, so it belongs to the value the loop
        // keys a redraw on. Left out of it, it reaches the screen only when
        // something else on the row happens to change with it — and on a
        // request that draws nothing else for a minute, the something else is
        // the clock.
        let mut turning = Turning::started();
        assert!(turning.moved(), "the first row was never drawn");

        turning.saw(&Event::Compacting {
            why: Compacting::Asked,
            part: 0,
        });
        assert!(
            turning.moved(),
            "room was asked for and the row did not say"
        );

        turning.saw(&Event::Compacting {
            why: Compacting::Asked,
            part: 12,
        });
        assert!(turning.moved(), "the bar moved and the row did not");
    }

    #[test]
    fn the_bar_arrives_with_the_notes_rather_than_standing_at_nothing() {
        // Nothing is measurable until the first word of the recap arrives: the
        // request is out and the model is reading the session it is about to
        // write down, which on a full window is seconds. A bar at nothing for
        // all of it is what a reader calls stuck, and the words beside it say
        // the same thing without claiming a length.
        let style = Style::plain();
        let glyphs = style.glyphs();

        let before = making(Compacting::Full, 0, 80, style)
            .expect("a row")
            .text();

        assert!(!before.contains(glyphs.hollow()), "{before:?}");
        assert!(before.contains("the window was full"), "{before:?}");

        let under = making(Compacting::Full, 12, 80, style)
            .expect("a row")
            .text();

        assert!(under.contains(glyphs.filled()), "{under:?}");
        assert!(under.contains("12%"), "{under:?}");
    }

    #[test]
    fn a_window_with_no_room_for_the_row_keeps_the_turn_s_own_output_instead() {
        let turning = Turning::started();

        for room in 0..=ROWS {
            assert!(
                turning
                    .rows(&nothing(), 80, Style::plain(), room)
                    .is_empty(),
                "{room}"
            );
        }

        assert_eq!(
            turning.rows(&nothing(), 80, Style::plain(), ROWS + 1).len(),
            ROWS
        );
    }

    #[test]
    fn a_call_stands_over_the_row_for_as_long_as_its_tool_is_out() {
        // Here rather than in scrollback, because the mark on it moves: a line
        // the renderer rewinds over every frame cannot also be a line it never
        // rewinds over. It is committed when the tool answers and not before.
        let mut turning = Turning::started();
        let said = |turning: &Turning| {
            turning
                .rows(&nothing(), 80, Style::plain(), 24)
                .iter()
                .map(Row::text)
                .collect::<Vec<_>>()
        };

        assert!(!said(&turning).iter().any(|row| row.contains("Read")));

        turning.saw(&requested());
        let standing = said(&turning);

        assert_eq!(standing.len(), CALLING, "{standing:?}");

        // By position, since what is under test is the order: the call over the
        // row that says a turn is running, and a blank above the call and
        // another between the two, so neither belongs to the output above nor
        // to the box under them.
        let at = |row: usize| standing.get(row).cloned().unwrap_or_default();

        assert!(at(0).is_empty(), "{standing:?}");
        assert!(at(1).contains("Read(src/main.rs)"), "{standing:?}");
        // Directly under the call with no blank between them: it is a caption on
        // the call rather than a second thing beside it.
        assert!(at(2).contains("(ctrl+b to background)"), "{standing:?}");
        assert!(at(3).is_empty(), "{standing:?}");
        assert!(at(4).contains("running"), "{standing:?}");
        assert!(at(5).is_empty(), "{standing:?}");
    }

    /// What a running call has printed, as an event.
    fn printed(text: &str) -> Event {
        Event::Wrote {
            call: ToolId::new("a"),
            text: crucible_core::Wrote::new(text),
        }
    }

    #[test]
    fn a_command_shows_its_last_lines_and_says_how_many_there_have_been() {
        let mut turning = Turning::started();
        turning.saw(&requested());

        for line in 1..=41 {
            turning.saw(&printed(&format!("Compiling crate-{line} v0.5.0\n")));
        }

        let rows: Vec<String> = turning
            .rows(&nothing(), 80, Style::plain(), 24)
            .iter()
            .map(Row::text)
            .collect();
        let sample: Vec<&String> = rows
            .iter()
            .filter(|row| row.contains("Compiling"))
            .collect();

        assert_eq!(sample.len(), SAMPLE, "{rows:?}");
        // The last of them, not the first: what a build is doing now is the
        // question, and the first five lines answered it a minute ago.
        assert!(
            sample.last().is_some_and(|row| row.contains("crate-41")),
            "{rows:?}"
        );
        assert!(
            sample.first().is_some_and(|row| row.contains("crate-37")),
            "{rows:?}"
        );

        // And the count row is what keeps five rows from reading as everything
        // the command has said. Indented with the sample, because it is a caption
        // on those rows rather than a row of its own — the one thing here a row
        // test can check and a reader would notice first.
        let counted = rows
            .iter()
            .find(|row| row.contains("41 lines"))
            .expect("the sample never said how much of it was not shown");

        assert!(counted.starts_with("    41 lines"), "{counted:?}");
        assert!(
            counted.contains(" B") || counted.contains("kB") || counted.contains("MB"),
            "the count never said how many bytes: {counted:?}"
        );
    }

    #[test]
    fn the_row_under_a_call_offers_to_leave_it_running_before_it_has_printed_anything() {
        // A command silent for thirty-eight seconds is the one most worth putting
        // down, so the row that offers it cannot wait for output to justify
        // itself. It gains the counts in front of the offer once there are any.
        let mut turning = Turning::started();
        turning.saw(&requested());

        let rows = |turning: &Turning| {
            turning
                .rows(&nothing(), 80, Style::plain(), 24)
                .iter()
                .map(Row::text)
                .collect::<Vec<_>>()
        };

        let quiet = rows(&turning);
        assert!(
            quiet
                .iter()
                .any(|row| row.contains("(ctrl+b to background)")),
            "{quiet:?}"
        );
        assert!(
            !quiet.iter().any(|row| row.contains("lines")),
            "a command that has printed nothing was given a count: {quiet:?}"
        );

        turning.saw(&printed("Compiling one\n"));
        let printing = rows(&turning);
        let counted = printing
            .iter()
            .find(|row| row.contains("(ctrl+b to background)"))
            .expect("the offer went away when the command spoke");

        assert!(counted.contains("1 line"), "{counted:?}");
        assert!(counted.starts_with("    1 line"), "{counted:?}");
    }

    #[test]
    fn what_a_command_printed_is_handed_back_when_its_tool_answers() {
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.saw(&printed("Compiling one\n"));

        turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
        });

        let rows: Vec<String> = turning
            .rows(&nothing(), 80, Style::plain(), 24)
            .iter()
            .map(Row::text)
            .collect();

        assert!(
            !rows.iter().any(|row| row.contains("Compiling")),
            "the sample outlived the call it belonged to: {rows:?}"
        );
    }

    #[test]
    fn a_window_short_of_rows_drops_the_sample_before_the_call_line() {
        // The order things give way. The sample is the one of them a second look
        // gets back whatever the window did, so it goes first.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.saw(&printed("Compiling one\n"));

        let rows: Vec<String> = turning
            .rows(&nothing(), 80, Style::plain(), CALLING + 1)
            .iter()
            .map(Row::text)
            .collect();

        assert!(
            rows.iter().any(|row| row.contains("Read(src/main.rs)")),
            "the call line gave way before the sample: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("Compiling")),
            "the sample took room the call line needed: {rows:?}"
        );
    }

    #[test]
    fn the_sample_is_on_the_value_the_loop_keys_a_redraw_on() {
        // Otherwise a command's output reaches the screen only on the frames
        // something else on the footing happens to change — a second at a time,
        // when the clock ticks.
        let mut turning = Turning::started();
        turning.saw(&requested());
        assert!(turning.moved());

        turning.saw(&printed("Compiling one\n"));
        assert!(
            turning.moved(),
            "output arrived and the footing did not think it had changed"
        );

        assert!(!turning.moved(), "a frame nobody could tell from the last");
    }

    #[test]
    fn a_turn_that_ran_no_command_gets_no_frame_out_of_the_sample() {
        // Every turn ends, and the end empties the sample. A turn that never had
        // a command running must not be redrawn for that: the region is being
        // handed back at that moment, and a frame with nothing behind it scrolls
        // the terminal by a row nobody asked for.
        let mut turning = Turning::started();
        turning.saw(&Event::Delta { text: "hi".into() });
        assert!(turning.moved());

        turning.saw(&Event::TurnFinished {
            turn: TurnId::FIRST,
            stop: StopReason::Yielded,
        });

        assert!(
            !turning.moved(),
            "the end of a turn with no command invented a frame"
        );
    }

    #[test]
    fn a_line_rewritten_in_place_replaces_the_row_rather_than_adding_one() {
        // What a progress bar does: a carriage return and the line again. Kept as
        // one row, because that is what the terminal it was written for would do.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.saw(&printed("Building [==>    ] 41/128\r"));
        turning.saw(&printed("Building [====>  ] 96/128\r"));

        let rows: Vec<String> = turning
            .rows(&nothing(), 80, Style::plain(), 24)
            .iter()
            .map(Row::text)
            .collect();
        let building: Vec<&String> = rows.iter().filter(|row| row.contains("Building")).collect();

        assert_eq!(building.len(), 1, "{rows:?}");
        assert!(
            building.first().is_some_and(|row| row.contains("96/128")),
            "{rows:?}"
        );
    }

    #[test]
    fn the_call_line_comes_back_when_its_tool_answers_and_only_then() {
        let mut turning = Turning::started();

        assert_eq!(turning.saw(&requested()), None);
        assert_eq!(turning.saw(&Event::Delta { text: "hi".into() }), None);
        assert_eq!(
            turning.saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            Some("Read(src/main.rs)".to_owned())
        );

        // And once only. A second reading would commit the same line twice.
        assert_eq!(
            turning.saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            None
        );
    }

    #[test]
    fn a_turn_that_ends_with_a_tool_still_out_hands_its_call_back_anyway() {
        // Otherwise a call that was made leaves no record of having been made:
        // the line was never committed, and the turn it was standing in is
        // over. That is the one thing a transcript may not do -- and it is
        // reached by every turn that fails or is stopped mid-call, which is
        // exactly when somebody goes looking for what ran.
        for ending in [
            Event::TurnFinished {
                turn: TurnId::FIRST,
                stop: StopReason::Cancelled,
            },
            Event::Failed {
                error: TurnError::Refused("read".into()),
            },
        ] {
            let mut turning = Turning::started();
            turning.saw(&requested());

            assert_eq!(
                turning.saw(&ending),
                Some("Read(src/main.rs)".to_owned()),
                "{ending:?}"
            );
        }
    }

    #[test]
    fn a_turn_asked_to_stop_still_lets_the_call_it_had_out_come_back() {
        // The word freezes at `interrupting` when the key is pressed. The line
        // of the call still out is not a word, and freezing it too would lose
        // the record of the call at the one moment there is most to explain.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.interrupting();

        assert_eq!(
            turning.saw(&Event::ToolFinished {
                call: ToolId::new("a"),
                output: ToolOutput::ok("done"),
            }),
            Some("Read(src/main.rs)".to_owned())
        );
    }

    #[test]
    fn the_mark_on_a_live_call_pulses_and_the_words_beside_it_do_not_move() {
        // Two frames, half a beat apart. The mark is painted one way and then
        // the other; everything after it is the same string in the same
        // columns, because a call line that changed width four times a second
        // would be unreadable next to the row it stands over.
        // Against a palette that writes colour, because the pulse *is* colour:
        // on a terminal without any, the two faces are the same mark and the
        // row is still and correct. What is under test is the beat reaching the
        // slot, so the instrument has to be one that can tell two slots apart.
        let style = Style::plain();
        let palette = Palette::resolve(true, Theme::Dark, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        });
        let now = Instant::now();

        // One beat apart to the microsecond, rather than two readings of the
        // clock a beat apart in wall time: what is under test is that the face
        // changes from one beat to the next, and a machine that stalled between
        // two readings would be testing how long the stall was.
        let face = |beat: Duration| {
            let moment = Turning {
                since: now.checked_sub(beat).expect("a clock past its own epoch"),
                ..Turning::started()
            };

            moment.call("Read(src/main.rs)", 80, style)
        };
        let (lit, dim) = (face(Duration::ZERO), face(Duration::from_millis(250)));

        assert_ne!(
            lit.paint(&palette),
            dim.paint(&palette),
            "the mark did not pulse"
        );

        // What the row says rather than what it is painted as, because the words
        // are two spans now — the tool's name in the accent and its arguments in
        // the quieter colour — and a sequence between them is bytes rather than
        // a column the words moved by.
        for face in [&lit, &dim] {
            assert!(
                face.text().ends_with("Read(src/main.rs)"),
                "{}",
                face.text()
            );
        }
    }

    #[test]
    fn the_call_line_is_on_the_value_the_loop_keys_a_redraw_on() {
        // Left off it, a call would appear on screen only on the beat some
        // other segment happened to change -- so the line naming what is
        // running would arrive after the tool it names had already answered.
        let mut turning = Turning::started();
        turning.moved();

        turning.saw(&requested());
        assert!(turning.moved(), "the call appeared and the footing did not");

        turning.saw(&Event::ToolFinished {
            call: ToolId::new("a"),
            output: ToolOutput::ok("done"),
        });
        assert!(turning.moved(), "the call went and the footing did not");
    }

    #[test]
    fn a_prompt_finished_while_the_turn_runs_is_named_under_the_row() {
        // The gap this closes: Return during a turn takes the line out of the
        // box, and until this row nothing on the screen said where it went.
        // The next acknowledgement it gets is its own turn starting, which is
        // however long the turn in front of it takes.
        let mut turning = Turning::started();
        turning.queueing(Some("fix the failing test"), 80, Style::plain());

        let rows = turning.rows(&nothing(), 80, Style::plain(), 24);
        let said = rows.iter().map(Row::text).collect::<Vec<_>>();

        assert_eq!(said.len(), QUEUED, "{said:?}");

        // By position, since the position is the point: directly under the row
        // it belongs to, with no blank between them, and in the column that
        // row's own word starts in.
        let at = |row: usize| said.get(row).cloned().unwrap_or_default();

        assert!(at(0).is_empty(), "{said:?}");
        assert!(at(1).starts_with("✳ thinking"), "{said:?}");
        assert_eq!(at(2), "  Next: fix the failing test", "{said:?}");
        assert!(at(3).is_empty(), "{said:?}");
    }

    #[test]
    fn a_turn_with_nothing_waiting_behind_it_draws_no_row_for_it() {
        // Absent rather than blank. A row that says nothing is a row of the
        // window spent, and what it is spent against is the turn's own output
        // above it.
        let turning = Turning::started();
        let rows = turning.rows(&nothing(), 80, Style::plain(), 24);

        assert_eq!(rows.len(), ROWS, "{:?}", rows.iter().map(Row::text));
    }

    #[test]
    fn a_prompt_wider_than_the_window_is_cut_at_the_right() {
        // Cut rather than wrapped: the footing's height is what the renderer
        // rewinds by, and a height that depended on how much somebody typed
        // would be a rewind that depended on it too.
        let mut turning = Turning::started();
        turning.queueing(Some(&"a".repeat(200)), 40, Style::plain());

        let rows = turning.rows(&nothing(), 40, Style::plain(), 24);
        let said = rows.get(2).map(Row::text).unwrap_or_default();

        assert!(said.ends_with('…'), "{said:?}");
        assert!(crucible_tui::columns(&said) <= 40, "{said:?}");
    }

    #[test]
    fn what_is_held_of_a_waiting_prompt_is_a_row_of_it_rather_than_all_of_it() {
        // It is cloned into the value the redraw is keyed on, sixty times a
        // second, and the box lets a prompt reach a megabyte. Cutting it where
        // it is taken is what keeps that clone the size of a row.
        let mut turning = Turning::started();
        turning.queueing(Some(&"a".repeat(1024 * 1024)), 80, Style::plain());

        let held = turning.queued.clone().unwrap_or_default();
        assert!(crucible_tui::columns(&held) <= 80, "{}", held.len());
    }

    #[test]
    fn the_prompt_waiting_is_on_the_value_the_loop_keys_a_redraw_on() {
        // Left off it, a line finished into the queue would reach the screen
        // on the beat some other segment happened to change -- a box emptied by
        // Return with nothing anywhere saying the line was kept, for as long as
        // a quarter of a second after the press.
        let mut turning = Turning::started();
        turning.moved();

        turning.queueing(Some("fix the failing test"), 80, Style::plain());
        assert!(
            turning.moved(),
            "the prompt appeared and the footing did not"
        );

        turning.queueing(None, 80, Style::plain());
        assert!(turning.moved(), "the prompt went and the footing did not");
    }

    #[test]
    fn the_row_naming_a_waiting_prompt_is_never_drawn_past_the_last_column() {
        // The mark that says a line was cut is columns of the row rather than
        // columns past it, and the ascii set spells it with three -- so a row
        // that reserved one column for it would be committed two past the
        // window, and the terminal would wrap it into a row nothing counted.
        for wide in [0, 1, 2, 3, 5, 6, 7, 8, 20, 80] {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                let said = next("fix the failing test", wide, Style::drawn(glyphs)).text();

                assert!(
                    crucible_tui::columns(&said) <= wide,
                    "{wide} {glyphs:?}: {said:?}"
                );
            }
        }
    }

    #[test]
    fn a_window_too_short_for_all_three_drops_the_call_before_the_waiting_prompt() {
        // In that order, because that is the order they stop being worth the
        // room. The call reaches scrollback the moment its tool answers and
        // the prompt is still in the queue with its own turn to come; the row
        // saying a turn is running exists nowhere else, so it goes last.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.queueing(Some("fix the failing test"), 80, Style::plain());

        let said = |room: usize| {
            turning
                .rows(&nothing(), 80, Style::plain(), room)
                .iter()
                .map(Row::text)
                .collect::<Vec<_>>()
        };

        let whole = said(CALLING + 2);
        assert_eq!(whole.len(), CALLING + 1, "{whole:?}");
        assert!(whole.concat().contains("Read"), "{whole:?}");
        assert!(whole.concat().contains("Next:"), "{whole:?}");

        let shorter = said(CALLING + 1);
        assert_eq!(shorter.len(), QUEUED, "{shorter:?}");
        assert!(!shorter.concat().contains("Read"), "{shorter:?}");
        assert!(shorter.concat().contains("Next:"), "{shorter:?}");

        let shortest = said(QUEUED);
        assert_eq!(shortest.len(), ROWS, "{shortest:?}");
        assert!(!shortest.concat().contains("Next:"), "{shortest:?}");
    }

    #[test]
    fn a_window_too_short_for_both_drops_the_call_before_the_row() {
        // The call is written to scrollback the moment its tool answers, so a
        // window that drops it loses nothing a second look does not return.
        // The row saying a turn is running exists nowhere else.
        let mut turning = Turning::started();
        turning.saw(&requested());

        let rows = turning.rows(&nothing(), 80, Style::plain(), CALLING);
        let said = rows.iter().map(Row::text).collect::<String>();

        assert_eq!(rows.len(), ROWS, "{said:?}");
        assert!(said.contains("running"), "{said:?}");
        assert!(!said.contains("Read"), "{said:?}");
    }

    #[test]
    fn the_plan_stands_under_everything_the_turn_says_and_over_the_box() {
        // The only place it can go. What it stands under is the turn — the call
        // out and the row saying one is running — and what it stands over is
        // the line being typed while that happens. The blank at the end parts
        // it from the box, so the panel is the last thing above one.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.queueing(Some("fix the failing test"), 80, Style::plain());

        let rows = turning.rows(&planned(3), 80, Style::plain(), 40);
        let said = rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");

        assert!(said.contains("Task 0"), "{said:?}");
        assert!(said.find("Read") < said.find("Task 0"), "{said:?}");
        assert!(said.find("running") < said.find("Task 0"), "{said:?}");
        assert!(said.find("Next:") < said.find("Task 0"), "{said:?}");
        assert_eq!(rows.last().map(Row::text).as_deref(), Some(""), "{said:?}");
    }

    #[test]
    fn a_window_short_of_rows_drops_the_call_and_the_waiting_prompt_before_a_task() {
        // What measuring the panel first buys. The call line and the row naming
        // the prompt behind the turn are the two measured against what the plan
        // left, so they are the two a narrow window drops on its behalf: a call
        // is committed to scrollback the moment its tool answers and a queued
        // prompt has its own turn coming, while what the agent is working to is
        // on screen nowhere else.
        let mut turning = Turning::started();
        turning.saw(&requested());
        turning.queueing(Some("fix the failing test"), 80, Style::plain());

        let planning = planned(3);
        let panel = planning.rows(80, 40, Style::plain().glyphs()).len();

        let said = |room: usize| {
            turning
                .rows(&planning, 80, Style::plain(), room)
                .iter()
                .map(Row::text)
                .collect::<String>()
        };

        // One taller than it used to be, because the call now carries the row
        // offering to leave its command running.
        let whole = said(panel + 8);
        assert!(whole.contains("Read"), "{whole:?}");
        assert!(whole.contains("Next:"), "{whole:?}");
        assert!(whole.contains("Task 2"), "{whole:?}");

        let shorter = said(panel + 7);
        assert!(!shorter.contains("Read"), "{shorter:?}");
        assert!(shorter.contains("Next:"), "{shorter:?}");
        assert!(shorter.contains("Task 2"), "{shorter:?}");

        let shortest = said(panel + 4);
        assert!(!shortest.contains("Next:"), "{shortest:?}");
        assert!(shortest.contains("Task 2"), "{shortest:?}");
    }
}
