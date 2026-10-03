//! `/usage`: what the session has used, and how much of each plan limit is
//! gone.
//!
//! Three blocks under a title naming who is answering and with which
//! credential. The session's cost, its time waiting on the API and on the
//! wall, the lines its edits changed and its tokens; one bar for how much of
//! the next request's window is used, the figure `/context` shows as free
//! turned round; and one bar for each plan limit, the plan-wide ones first and
//! then each group a vendor keeps for one model under the vendor's name for
//! it. Within a group the windows stand shortest first, so a window is found
//! in the same place from one provider to the next. A window is named in
//! crucible's words by its length; the group's name is the one thing a vendor
//! wrote that is drawn, as text, cut to fit.
//!
//! What is known stands at once: what the responses so far reported, or what
//! the plan last answered. Opening the panel between turns also asks the
//! plan, where the application will ask it — a sign-in whose vendor keeps a
//! source, once a minute at most — and the block's last row says so until the
//! answer comes and the block is drawn again. A plan that never answers, with
//! nothing known, is said to have reported none. Printed rather than stood,
//! the panel waits for the answer first, since a printed block cannot be
//! drawn again: where keys are read, only until a key is pressed, after which
//! the question is given up and what is known is printed. A panel closed with
//! the question still out gives it up the same way, so the request is
//! answered with what is known. A question given up still counts toward the
//! minute, so opening the panel again within it asks nothing and shows what
//! is known.
//!
//! A cost nobody priced reads `not priced`, never `$0.00`. A reset time is the
//! reader's own wall clock, read in the system's zone as the panel opens; a
//! machine whose zone cannot be read is shown UTC, and the times say so, as is
//! one whose `TZ`, or with `TZ` unset whose `/etc/localtime`, names anything
//! on disk but a regular file of a zone file's size: the zone is read on the
//! drawing thread, which a pipe or an endless file would hold. A reset the
//! clock is already past says `since passed`: the window has
//! started again since the reading its figure is from.
//!
//! [`body`] draws the blocks at a width and nothing else, so the panel here
//! and anything that embeds the same figures draw them alike. Below
//! [`WIDE`] columns each label stands over its value rather than beside it.
//!
//! Over a running turn it stands with the figures that turn last reported and
//! asks nothing: the turn has the conversation.

use std::ffi::OsStr;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::providers::{CredentialSource, credential_source, in_use, offered};
use crucible_app::startup::ProviderAuth;
use crucible_client_api::{self as api, Cost, Limit, Reading, Window};
use crucible_tui::{Bar, Fill, Glyphs, Part, Pressed, Renderer, Row, Slot, Terminal};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::cli::Fatal;
use crate::cli::client::{Out, astray};

use super::context::closing;
use super::region::{self, Ended, Moved};
use super::{Counted, HUNG, Terms};

/// The narrowest window a label is drawn beside its value in; below it a
/// window's bar beside its label would be under thirty cells, so each label
/// stands over its value instead.
const WIDE: usize = 60;

/// The column values start at, beside their labels and under them.
const BESIDE: usize = 19;
const UNDER: usize = 17;

/// What a bar leaves at its end, for the `100% used` beside it.
const FIGURE: usize = 12;

/// What a count's figure column leaves beyond the widest count: a gap before
/// it wider than a percent's, and the margin after it.
const COUNT_MARGIN: usize = 4;

/// How often a panel with a question out looks again for the answer with no
/// key pressed.
pub(super) const BEAT: std::time::Duration = std::time::Duration::from_millis(250);

/// The largest zone file the system's zone may be read from; a real one is a
/// few kilobytes.
const ZONE_FILE: u64 = 1 << 20;

/// Where the system's zone is read from when `TZ` is unset.
const LOCALTIME: &str = "/etc/localtime";

/// Whether the system's zone may be read with `tz` as `TZ` and `localtime`
/// as `/etc/localtime`: whether `TZ` names a zone, a rule or nothing on disk,
/// or else a [`zone_sized`] file, and, with `TZ` unset, whether `localtime`
/// is one. The zone is read on the drawing thread, and what a path names is
/// read whole, so a pipe would hold the panel until something wrote to it and
/// an endless file until memory ran out.
fn readable_zone(tz: Option<&OsStr>, localtime: &Path) -> bool {
    let Some(tz) = tz else {
        return zone_sized(localtime);
    };
    let Some(name) = tz.to_str() else {
        return true;
    };
    let name = name.strip_prefix(':').unwrap_or(name);
    if name.is_empty() || TimeZone::get(name).is_ok() {
        return true;
    }
    zone_sized(Path::new(name))
}

/// Whether `path` names nothing, or a regular file no larger than
/// [`ZONE_FILE`] once every symlink is followed, as `/etc/localtime` usually
/// is one into the zone database.
fn zone_sized(path: &Path) -> bool {
    std::fs::metadata(path).map_or(true, |found| found.is_file() && found.len() <= ZONE_FILE)
}

/// The wall clock reset times are read against: now, in the reader's zone.
pub(crate) struct Clock {
    now: Timestamp,
    zone: TimeZone,
    /// Whether the zone is UTC because the system's could not be read.
    guessed: bool,
}

impl Clock {
    /// Now, in the zone this machine says it is in, or in UTC where it says
    /// nothing readable.
    pub(crate) fn system() -> Self {
        let readable = readable_zone(std::env::var_os("TZ").as_deref(), Path::new(LOCALTIME));
        let (zone, guessed) = readable
            .then(|| TimeZone::try_system().ok())
            .flatten()
            .map_or((TimeZone::UTC, true), |zone| (zone, false));
        Self {
            now: Timestamp::now(),
            zone,
            guessed,
        }
    }

    /// `now`, in `zone`.
    #[cfg(test)]
    pub(crate) fn at(now: i64, zone: TimeZone) -> Self {
        Self {
            now: Timestamp::from_second(now).unwrap_or(Timestamp::UNIX_EPOCH),
            zone,
            guessed: false,
        }
    }

    /// The phrase a window's reset is drawn with: `resets 15:40` for one to
    /// come, and `reset 09:00, since passed` for one the clock is already
    /// past.
    ///
    /// A reading is kept until a response brings another, so after an idle
    /// stretch its reset can be behind the clock: the window has started
    /// again since, and its figure is what it was as of the reading, which
    /// the phrase says rather than promising a reset that already happened.
    fn resets(&self, at: u64) -> Option<String> {
        let read = self.reads(at)?;
        let passed = i64::try_from(at).is_ok_and(|at| at <= self.now.as_second());
        Some(if passed {
            format!("reset {read}, since passed")
        } else {
            format!("resets {read}")
        })
    }

    /// The phrase a turn stopped on a used-up plan names its reset with:
    /// `resets Mon 09:00` for one still to come, and `resets soon` for one the
    /// clock has reached. The stop happened against a reset still ahead, so
    /// one behind the clock now says the plan is about to be usable again
    /// rather than that it already is. `None` where the time cannot be read.
    pub(crate) fn reset_by(&self, at: SystemTime) -> Option<String> {
        let Ok(since) = at.duration_since(UNIX_EPOCH) else {
            return Some("resets soon".to_owned());
        };
        let at = since.as_secs();
        if i64::try_from(at).is_ok_and(|at| at <= self.now.as_second()) {
            return Some("resets soon".to_owned());
        }
        self.reads(at).map(|read| format!("resets {read}"))
    }

    /// When a window starts again, as a wall clock reads it: the time alone
    /// today, with the weekday within the week, and with the date beyond or
    /// before.
    fn reads(&self, at: u64) -> Option<String> {
        let at = Timestamp::from_second(i64::try_from(at).ok()?)
            .ok()?
            .to_zoned(self.zone.clone());
        let today: Date = self.now.to_zoned(self.zone.clone()).date();
        let days = today.until(at.date()).ok()?.get_days();
        let shape = match days {
            0 => "%H:%M",
            1..=6 => "%a %H:%M",
            _ => "%-d %b %H:%M",
        };
        let read = at.strftime(shape).to_string();
        Some(if self.guessed {
            format!("{read} UTC")
        } else {
            read
        })
    }
}

/// Asks for what the session has used and stands it, or prints it where no
/// keys can close a panel or there is no room to stand one.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let usage = match terms.perform(conversation, api::Command::Usage) {
        Performed::Usage(usage) => *usage,
        other => return Ok(renderer.commit(&astray(&other))?),
    };
    let mut shown = Shown::of(terms, conversation.serving(), usage);
    shown.out = terms.ask_limits(conversation);
    if keys {
        let mut watch = |shown: &mut Shown| shown.watched(terms, conversation, false);
        if stood(renderer, terms, &mut shown, |_| Ok(()), Some(&mut watch))? != Ended::Cramped {
            closed(shown.out.take(), terms, conversation);
            return Ok(());
        }
        // Printed once, so with the plan's answer in it rather than a promise
        // of one; but no longer than until a key says not to wait.
        awaited(&mut shown, terms, conversation, |beat| {
            keyed_within(renderer, beat)
        })?;
    } else {
        // Printed once, so with the plan's answer in it rather than a promise
        // of one. No key is read here, so none is kept waiting.
        shown.watched(terms, conversation, true);
    }
    // Hung under the line that asked, so laid out short of the mark.
    let columns = renderer.transcript_columns().saturating_sub(HUNG);
    Ok(renderer.present(&shown.body(columns, terms.style().glyphs()))?)
}

/// Waits for the plan's answer before a block is printed where keys are read,
/// looking for it every [`BEAT`] and reading keys between. A key ends the
/// wait: the question is given up and what is known is printed, so the
/// keyboard is never held by a plan that is slow to answer. `pressed` says
/// whether a key came within the time it is handed.
fn awaited(
    shown: &mut Shown,
    terms: &Terms,
    conversation: &mut Conversation,
    mut pressed: impl FnMut(std::time::Duration) -> Result<bool, Fatal>,
) -> Result<(), Fatal> {
    while shown.out.as_ref().is_some_and(|out| !out.ended()) {
        if pressed(BEAT)? {
            if let Some(out) = shown.out.take_if(|out| !out.ended()) {
                terms.abandon(conversation, out);
            }
            break;
        }
    }
    // An answer that came in the meantime is taken in; one given up is gone.
    shown.watched(terms, conversation, false);
    Ok(())
}

/// Ends the question `out` as a panel that stood is closed. An answer that
/// came is taken back to the conversation, so the next opening shows it; one
/// still to come is given up, as a key ends a wait for it, so the request is
/// answered with what is known; it still counts toward the minute.
pub(super) fn closed(out: Option<Out>, terms: &Terms, conversation: &mut Conversation) {
    match out {
        Some(out) if out.ended() => drop(terms.asked(conversation, out)),
        Some(out) => terms.abandon(conversation, out),
        None => {}
    }
}

/// Whether a key is pressed within `beat`. The key is read, and is spent on
/// ending the wait; what the pointer or the selection took, a resize and what
/// means nothing are not keys.
fn keyed_within<T: Terminal>(
    renderer: &mut Renderer<T>,
    beat: std::time::Duration,
) -> Result<bool, Fatal> {
    if !renderer.waiting(beat)? {
        return Ok(false);
    }
    Ok(match renderer.took(crucible_tui::pressed()?)? {
        None | Some(Pressed::Ignored) => false,
        Some(Pressed::Resized) => {
            renderer.resized()?;
            false
        }
        Some(_) => true,
    })
}

/// Stands the panel over a running turn, with the figures that turn last
/// reported, or prints them where there is no room to stand it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn live<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    counted: &Counted,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<(), Fatal> {
    let mut shown = Shown::of(terms, counted.serving, counted.usage.clone());
    if stood(renderer, terms, &mut shown, while_waiting, None)? == Ended::Cramped {
        let columns = renderer.transcript_columns();
        renderer.present(&shown.body(columns, terms.style().glyphs()))?;
    }
    Ok(())
}

/// What the panel draws, read once as it opens: the figures, who is
/// answering, and the clock its reset times are read against; and the
/// question out to the plan, until it is answered.
struct Shown {
    heading: String,
    usage: api::Usage,
    clock: Clock,
    out: Option<Out>,
}

impl Shown {
    fn of(terms: &Terms, serving: Option<&'static str>, usage: api::Usage) -> Self {
        Self {
            heading: heading(terms, serving, terms.style().glyphs()),
            usage,
            clock: Clock::system(),
            out: None,
        }
    }

    fn body(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let asking = self.out.as_ref().map(Out::provider);
        body(
            &self.heading,
            &self.usage,
            asking,
            columns,
            glyphs,
            &self.clock,
        )
    }

    /// Takes the plan's answer in, where it has come or `wait` says to wait
    /// for it, and says whether the picture changed.
    fn watched(&mut self, terms: &Terms, conversation: &mut Conversation, wait: bool) -> Moved {
        answered_in(&mut self.out, &mut self.usage, terms, conversation, wait)
    }
}

/// Takes the answer to the question `out` back to `conversation` and into
/// `usage`, where it has come or `wait` says to wait for it, and says whether
/// what is drawn changed. The question is over either way it went: a refusal
/// or a failure leaves what was known.
pub(super) fn answered_in(
    out: &mut Option<Out>,
    usage: &mut api::Usage,
    terms: &Terms,
    conversation: &mut Conversation,
    wait: bool,
) -> Moved {
    let Some(question) = out.take_if(|question| wait || question.ended()) else {
        return Moved::Still;
    };
    if let Performed::Usage(answered) = terms.asked(conversation, question) {
        *usage = *answered;
    }
    Moved::Redraw
}

/// Who is answering, and with which credential.
///
/// Named as `/model` names it, except that a key read from a variable is
/// `API key` rather than the variable's name: the heading says what pays, and
/// where the key was read from is `/model`'s to say. A sign-in keeps its name.
/// Read off the stored credentials as the panel opens; a provider that
/// answers with none is named alone, and none answering names nothing.
fn heading(terms: &Terms, serving: Option<&'static str>, glyphs: Glyphs) -> String {
    let Some(name) = serving else {
        return String::new();
    };
    signed(terms, name).map_or_else(
        || name.to_owned(),
        |words| format!("{name} {} {words}", glyphs.dot()),
    )
}

/// The credential the provider `name` answers with, worded as the heading
/// words it, where it answers with one.
pub(super) fn signed(terms: &Terms, name: &str) -> Option<String> {
    let providers = terms.providers.snapshot();
    let stored = terms.logins.read();
    let auth = ProviderAuth {
        settings: &terms.settings,
        from: &*terms.environment,
        stored: &stored,
        subscriptions: &terms.subscriptions,
    };
    offered(&providers)
        .find(|one| one.name == name)
        .and_then(|one| match credential_source(one, auth)? {
            CredentialSource::Environment(_) => Some(KEYED.to_owned()),
            CredentialSource::StoredKey | CredentialSource::Subscription => {
                in_use(one, auth).map(|used| used.words)
            }
        })
}

/// What the heading calls a key read from a variable.
const KEYED: &str = "API key";

/// Stands the panel until it is closed, or says there was no room. With
/// `watch`, the panel is looked at again every [`BEAT`] with no key pressed,
/// for a question out to the plan; mid-turn there is none, and
/// `while_waiting` keeps the turn's text moving instead.
fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    shown: &mut Shown,
    while_waiting: impl FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
    watch: Option<&mut dyn FnMut(&mut Shown) -> Moved>,
) -> Result<Ended, Fatal> {
    let style = terms.style();
    let laid = |shown: &mut Shown, columns, _| {
        let glyphs = style.glyphs();
        let asking = shown.out.as_ref().map(Out::provider);
        let rows = panel(
            &shown.heading,
            &shown.usage,
            asking,
            columns,
            glyphs,
            &shown.clock,
        );
        (rows, None)
    };
    match watch {
        Some(watch) => region::stand_watching(
            renderer,
            |_| style,
            shown,
            laid,
            |pressed, _| closing(&pressed),
            BEAT,
            watch,
        ),
        None => region::stand_while(
            renderer,
            |_| style,
            shown,
            laid,
            |pressed, _| closing(&pressed),
            while_waiting,
        ),
    }
}

/// The panel's rows at `columns`: a rule, the body, and how to close it.
// What is drawn, whose plan is being asked, and how wide, in which glyphs and
// against which clock, are independent inputs, as for `body`.
#[allow(clippy::too_many_arguments)]
fn panel(
    heading: &str,
    usage: &api::Usage,
    asking: Option<&str>,
    columns: usize,
    glyphs: Glyphs,
    clock: &Clock,
) -> Vec<Row> {
    let mut rows = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
    ];
    rows.extend(body(heading, usage, asking, columns, glyphs, clock));
    rows.push(Row::new());
    rows.push(
        Row::new()
            .then(Slot::Quiet, "esc to close")
            .clipped(columns),
    );
    rows
}

/// What the session has used, at `columns`: the title, the session's figures,
/// the context bar, and a bar for each plan limit known.
///
/// `heading` names who is answering, after the word `Usage`; `asking` names
/// the provider whose plan is being asked, while it is. Reset times are read
/// against `clock`. Every row fits `columns`.
// What is drawn, whose plan is being asked, and how wide, in which glyphs and
// against which clock, are independent inputs: the panel, `/settings` and the
// tests each hold them apart.
#[allow(clippy::too_many_arguments)]
pub(crate) fn body(
    heading: &str,
    usage: &api::Usage,
    asking: Option<&str>,
    columns: usize,
    glyphs: Glyphs,
    clock: &Clock,
) -> Vec<Row> {
    let wide = columns >= WIDE;
    let at = if wide { BESIDE } else { UNDER };
    let title = if heading.is_empty() {
        "Usage".to_owned()
    } else {
        format!("Usage {} {heading}", glyphs.dot())
    };
    let minus = match glyphs {
        Glyphs::Unicode => "\u{2212}",
        Glyphs::Ascii => "-",
    };
    let (cost_slot, cost) = self::cost(&usage.used.cost);

    let mut rows = vec![
        Row::new().then(Slot::Strong, title),
        Row::new(),
        Row::new().then(Slot::Plain, "Session"),
        labelled("Total cost", at).then(cost_slot, cost),
        labelled("API time", at).then(Slot::Plain, took(usage.used.api_ms)),
        labelled("Wall time", at).then(Slot::Plain, took(usage.used.wall_ms)),
        labelled("Lines changed", at).then(
            Slot::Plain,
            format!("+{} {minus}{}", usage.used.added, usage.used.removed),
        ),
    ];
    let tokens = [
        format!("{} in", count(usage.used.input)),
        format!("{} out", count(usage.used.output)),
        format!("{} cache read", count(usage.used.cache_read)),
        format!("{} cache write", count(usage.used.cache_write)),
    ];
    let lines = wrapped(&tokens, glyphs.dot(), columns.saturating_sub(at));
    for (line, words) in lines.into_iter().enumerate() {
        let lead = if line == 0 {
            labelled("Tokens", at)
        } else {
            Row::new().then(Slot::Plain, " ".repeat(at))
        };
        rows.push(lead.then(Slot::Plain, words));
    }

    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Plain, "Context"));
    let gauge_cells = columns.saturating_sub(2 + FIGURE);
    rows.push(match usage.context.left {
        Some(left) => gauge(
            Row::new().then(Slot::Plain, "  "),
            100_u8.saturating_sub(left.get()),
            gauge_cells,
            glyphs,
        ),
        None => Row::new()
            .then(Slot::Plain, "  ")
            .then(Slot::Quiet, "window not known"),
    });

    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Plain, "Plan limits"));
    rows.extend(limits(&usage.limits, asking, columns, glyphs, clock));
    rows.into_iter().map(|row| row.clipped(columns)).collect()
}

/// The rows under `Plan limits` at `columns`: the plan-wide windows, then a
/// group for each model the plan limits on its own, then a row saying more
/// were reported than crossed, where they were, then a row saying whose plan
/// is being asked, while it is.
fn limits(
    limits: &api::Limits,
    asking: Option<&str>,
    columns: usize,
    glyphs: Glyphs,
    clock: &Clock,
) -> Vec<Row> {
    // One column for every figure in the block, wide enough for its widest
    // count, so the bars end together.
    let figure = limits
        .groups
        .iter()
        .flat_map(|group| &group.limits)
        .filter_map(|limit| match limit.reading {
            Reading::Counted { .. } => {
                figured(&limit.reading).map(|said| crucible_tui::columns(&said))
            }
            Reading::Percent(_) | Reading::Unlimited => None,
        })
        .map(|widest| widest + COUNT_MARGIN)
        .fold(FIGURE, usize::max);
    let mut rows = Vec::new();
    for group in &limits.groups {
        let drawn: Vec<Row> = group
            .limits
            .iter()
            .flat_map(|limit| self::limit(limit, figure, columns, glyphs, clock))
            .collect();
        if drawn.is_empty() {
            continue;
        }
        if let Some(model) = &group.model {
            if !rows.is_empty() {
                rows.push(Row::new());
            }
            rows.push(Row::new().then(
                Slot::Strong,
                format!(
                    "  {}",
                    cut(model.as_str(), columns.saturating_sub(2), glyphs)
                ),
            ));
        }
        rows.extend(drawn);
    }
    // What was cut at a ceiling is said to be missing, so what is drawn is
    // not read as every limit the plan has.
    if limits.more && !rows.is_empty() {
        rows.push(
            Row::new()
                .then(Slot::Plain, "  ")
                .then(Slot::Quiet, "more limits not reported"),
        );
    }
    match asking {
        Some(provider) => rows.push(Row::new().then(Slot::Plain, "  ").then(
            Slot::Quiet,
            format!("asking {provider}{}", glyphs.ellipsis()),
        )),
        None if rows.is_empty() => rows.push(
            Row::new()
                .then(Slot::Plain, "  ")
                .then(Slot::Quiet, "limits not reported"),
        ),
        None => {}
    }
    rows
}

/// The rows of one window: its name, its bar and figure, and when it resets.
/// None for a count of nothing, which limits nothing.
fn limit(limit: &Limit, figure: usize, columns: usize, glyphs: Glyphs, clock: &Clock) -> Vec<Row> {
    let wide = columns >= WIDE;
    let name = named(limit.window);
    let mut rows = Vec::new();
    let lead = if wide {
        labelled(&name, BESIDE)
    } else {
        rows.push(Row::new().then(Slot::Plain, format!("  {name}")));
        Row::new().then(Slot::Plain, "  ")
    };
    let cells = columns.saturating_sub(if wide { BESIDE } else { 2 } + figure);
    let (used, total) = match limit.reading {
        Reading::Unlimited => (None, 0),
        Reading::Counted { total: 0, .. } => return Vec::new(),
        Reading::Percent(used) => (Some(u64::from(used.get().min(100))), 100),
        Reading::Counted { used, total } => (Some(used.min(total)), total),
    };
    rows.push(match (used, figured(&limit.reading)) {
        (Some(used), Some(said)) => {
            // The figure right-aligned in its column, short of the margin
            // after it.
            let room = figure.saturating_sub(2);
            bar(used, total, cells, glyphs)
                .spans()
                .fold(lead, |row, (slot, text)| row.then(slot, text))
                .then(Slot::Plain, format!("{said:>room$}"))
        }
        _ => lead.then(Slot::Quiet, "unlimited"),
    });
    if let Some(resets) = limit.resets_at.and_then(|at| clock.resets(at)) {
        let indent = if wide { BESIDE } else { 2 };
        rows.push(
            Row::new()
                .then(Slot::Plain, " ".repeat(indent))
                .then(Slot::Quiet, resets),
        );
    }
    rows
}

/// What a window's figure says: `31% used`, `412 of 1,500 used`. None for a
/// window with no bar.
fn figured(reading: &Reading) -> Option<String> {
    match reading {
        Reading::Percent(used) => Some(format!("{}% used", used.get().min(100))),
        Reading::Counted { used, total } => {
            Some(format!("{} of {} used", grouped(*used), grouped(*total)))
        }
        Reading::Unlimited => None,
    }
}

/// A bar `cells` wide with `used` of `total` solid.
fn bar(used: u64, total: u64, cells: usize, glyphs: Glyphs) -> Row {
    let parts = [
        Part {
            slot: Slot::Accent,
            fill: Fill::Solid,
            size: used,
        },
        Part {
            slot: Slot::Quiet,
            fill: Fill::Shaded,
            size: total.saturating_sub(used),
        },
    ];
    Bar { parts: &parts }.row(cells, glyphs)
}

/// A count with its thousands set apart: `1,500`, `1,000,000`.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// `text`, cut to `columns` with the ellipsis where any of it was left out.
fn cut(text: &str, columns: usize, glyphs: Glyphs) -> String {
    if crucible_tui::columns(text) <= columns {
        return text.to_owned();
    }
    let mark = glyphs.ellipsis();
    let kept = crucible_tui::clip(text, columns.saturating_sub(crucible_tui::columns(mark)));
    format!("{kept}{mark}")
}

/// A row opening with `label`, indented, its value to start at column `at`.
fn labelled(label: &str, at: usize) -> Row {
    let room = at.saturating_sub(2);
    Row::new().then(Slot::Plain, format!("  {label:<room$}"))
}

/// `lead`, then a bar `cells` wide with `used` percent of it solid, then the
/// figure.
fn gauge(lead: Row, used: u8, cells: usize, glyphs: Glyphs) -> Row {
    let used = used.min(100);
    let parts = [
        Part {
            slot: Slot::Accent,
            fill: Fill::Solid,
            size: u64::from(used),
        },
        Part {
            slot: Slot::Quiet,
            fill: Fill::Shaded,
            size: u64::from(100 - used),
        },
    ];
    let bar = Bar { parts: &parts }.row(cells, glyphs);
    bar.spans()
        .fold(lead, |row, (slot, text)| row.then(slot, text))
        .then(Slot::Plain, format!(" {used:>3}% used"))
}

/// What a window is called on its row: the name a sentence calls it by, so the
/// two can never drift apart, standing first on the row and so capitalised.
///
/// The contract's window is mapped back to the domain's, whose name is the one
/// the notice and the client's sentence say: a rename there reaches this row
/// too, and the mapping names no words of its own.
fn named(window: Window) -> String {
    let window = match window {
        Window::FiveHour => crucible_types::Window::FiveHour,
        Window::Daily => crucible_types::Window::Daily,
        Window::Weekly => crucible_types::Window::Weekly,
        Window::Monthly => crucible_types::Window::Monthly,
        Window::Yearly => crucible_types::Window::Yearly,
        Window::Lasting { minutes } => crucible_types::Window::Lasting(minutes),
    };
    let named = window.named();
    let mut name = named.chars();
    name.next()
        .map(|first| first.to_uppercase().chain(name).collect())
        .unwrap_or_default()
}

/// What the session cost, and the tone it is said in.
fn cost(cost: &Cost) -> (Slot, String) {
    match cost {
        Cost::Unspent => (Slot::Quiet, "nothing asked yet".to_owned()),
        Cost::NotPriced => (Slot::Quiet, "not priced".to_owned()),
        Cost::Priced { currency, micros } => (Slot::Plain, money(currency.as_str(), *micros)),
        // An answer stopped before the provider said what it cost.
        Cost::AtLeast { currency, micros } => (
            Slot::Plain,
            format!("at least {}", money(currency.as_str(), *micros)),
        ),
    }
}

/// `micros` millionths of `currency`, to the cent: `$1.84`, `1.84 EUR`.
fn money(currency: &str, micros: u64) -> String {
    let cents = micros.saturating_add(5_000) / 10_000;
    let amount = format!("{}.{:02}", cents / 100, cents % 100);
    match (currency, cents) {
        // A sum that rounds to nothing was still spent.
        ("USD", 0) if micros > 0 => "<$0.01".to_owned(),
        ("USD", _) => format!("${amount}"),
        (code, _) => format!("{amount} {code}"),
    }
}

/// A span of milliseconds, to the second: `12s`, `4m 12s`, `2h 05m`.
fn took(millis: u64) -> String {
    let seconds = millis / 1_000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60),
    }
}

/// A count of tokens, to three figures: `0`, `38.1k`, `1.42M`.
fn count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{}.{}k", tokens / 1_000, tokens % 1_000 / 100),
        _ => format!("{}.{:02}M", tokens / 1_000_000, tokens % 1_000_000 / 10_000),
    }
}

/// `parts` joined by `dot`, a new line started wherever the next would run
/// past `room`. A part is never split: one wider than `room` stands alone and
/// is clipped with its row.
fn wrapped(parts: &[String], dot: &str, room: usize) -> Vec<String> {
    let joint = format!(" {dot} ");
    let mut lines: Vec<String> = Vec::new();
    for part in parts {
        match lines.last_mut() {
            Some(line)
                if crucible_tui::columns(line)
                    + crucible_tui::columns(&joint)
                    + crucible_tui::columns(part)
                    <= room =>
            {
                line.push_str(&joint);
                line.push_str(part);
            }
            _ => lines.push(part.clone()),
        }
    }
    lines
}

#[cfg(test)]
mod tests;
