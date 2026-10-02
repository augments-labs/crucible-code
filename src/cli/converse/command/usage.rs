//! `/usage`: what the session has used, and how much of each plan window is
//! gone.
//!
//! Three blocks under a title naming who is answering and with which
//! credential. The session's cost, its time waiting on the API and on the
//! wall, the lines its edits changed and its tokens; one bar for how much of
//! the next request's window is used, the figure `/context` shows as free
//! turned round; and one bar for each plan window the vendor reported on the
//! responses crucible already received, always in the order five-hour, weekly,
//! monthly, so a window is found in the same place from one provider to the
//! next. A vendor that reported none is said to have reported none: no
//! request is ever sent to find out.
//!
//! A cost nobody priced reads `not priced`, never `$0.00`. A reset time is the
//! reader's own wall clock, read in the system's zone as the panel opens; a
//! machine whose zone cannot be read is shown UTC, and the times say so. A
//! reset the clock is already past says `since passed`: the window has
//! started again since the reading its figure is from.
//!
//! [`body`] draws the blocks at a width and nothing else, so the panel here
//! and anything that embeds the same figures draw them alike. Below
//! [`WIDE`] columns each label stands over its value rather than beside it.
//!
//! It reads and changes nothing, so it stands over a running turn too, with
//! the figures that turn last reported.

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::providers::{CredentialSource, credential_source, in_use, offered};
use crucible_app::startup::ProviderAuth;
use crucible_client_api::{self as api, Cost, Window};
use crucible_tui::{Bar, Fill, Glyphs, Part, Renderer, Row, Slot, Terminal};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::cli::Fatal;
use crate::cli::client::astray;

use super::context::closing;
use super::region::{self, Ended};
use super::{Counted, HUNG, Still, Terms};

/// The narrowest window a label is drawn beside its value in; below it a
/// window's bar beside its label would be under thirty cells, so each label
/// stands over its value instead.
const WIDE: usize = 60;

/// The column values start at, beside their labels and under them.
const BESIDE: usize = 19;
const UNDER: usize = 17;

/// What a bar leaves at its end, for the `100% used` beside it.
const FIGURE: usize = 12;

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
        let (zone, guessed) =
            TimeZone::try_system().map_or((TimeZone::UTC, true), |zone| (zone, false));
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
    let shown = Shown::of(terms, conversation.serving(), &usage);
    if keys && stood(renderer, terms, &shown, |_| Ok(()))? != Ended::Cramped {
        return Ok(());
    }
    // Hung under the line that asked, so laid out short of the mark.
    let columns = renderer.transcript_columns().saturating_sub(HUNG);
    Ok(renderer.present(&shown.body(columns, terms.style().glyphs()))?)
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
    let shown = Shown::of(terms, counted.serving, &counted.usage);
    if stood(renderer, terms, &shown, while_waiting)? == Ended::Cramped {
        let columns = renderer.transcript_columns();
        renderer.present(&shown.body(columns, terms.style().glyphs()))?;
    }
    Ok(())
}

/// What the panel draws, read once as it opens: the figures, who is
/// answering, and the clock its reset times are read against.
struct Shown<'a> {
    heading: String,
    usage: &'a api::Usage,
    clock: Clock,
}

impl<'a> Shown<'a> {
    fn of(terms: &Terms, serving: Option<&'static str>, usage: &'a api::Usage) -> Self {
        Self {
            heading: heading(terms, serving, terms.style().glyphs()),
            usage,
            clock: Clock::system(),
        }
    }

    fn body(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        body(&self.heading, self.usage, columns, glyphs, &self.clock)
    }
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
        .map_or_else(
            || name.to_owned(),
            |words| format!("{name} {} {words}", glyphs.dot()),
        )
}

/// What the heading calls a key read from a variable.
const KEYED: &str = "API key";

/// Stands the panel until it is closed, or says there was no room.
fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    shown: &Shown<'_>,
    while_waiting: impl FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Ended, Fatal> {
    let style = terms.style();
    region::stand_while(
        renderer,
        |_| style,
        &mut Still,
        |_, columns, _| {
            let glyphs = style.glyphs();
            let rows = panel(&shown.heading, shown.usage, columns, glyphs, &shown.clock);
            (rows, None)
        },
        |pressed, _| closing(&pressed),
        while_waiting,
    )
}

/// The panel's rows at `columns`: a rule, the body, and how to close it.
fn panel(
    heading: &str,
    usage: &api::Usage,
    columns: usize,
    glyphs: Glyphs,
    clock: &Clock,
) -> Vec<Row> {
    let mut rows = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
    ];
    rows.extend(body(heading, usage, columns, glyphs, clock));
    rows.push(Row::new());
    rows.push(
        Row::new()
            .then(Slot::Quiet, "esc to close")
            .clipped(columns),
    );
    rows
}

/// What the session has used, at `columns`: the title, the session's figures,
/// the context bar, and a bar for each plan window reported.
///
/// `heading` names who is answering, after the word `Usage`; reset times are
/// read against `clock`. Every row fits `columns`.
pub(crate) fn body(
    heading: &str,
    usage: &api::Usage,
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
    if usage.limits.is_empty() {
        rows.push(
            Row::new()
                .then(Slot::Plain, "  ")
                .then(Slot::Quiet, "limits not reported"),
        );
    }
    for window in Window::EVERY {
        let Some(limit) = usage.limits.of(window) else {
            continue;
        };
        let used = limit.used.get();
        let resets = limit.resets_at.and_then(|at| clock.resets(at));
        if wide {
            let cells = columns.saturating_sub(BESIDE + FIGURE);
            rows.push(gauge(labelled(named(window), at), used, cells, glyphs));
        } else {
            rows.push(Row::new().then(Slot::Plain, format!("  {}", named(window))));
            rows.push(gauge(
                Row::new().then(Slot::Plain, "  "),
                used,
                gauge_cells,
                glyphs,
            ));
        }
        if let Some(resets) = resets {
            let indent = if wide { at } else { 2 };
            rows.push(
                Row::new()
                    .then(Slot::Plain, " ".repeat(indent))
                    .then(Slot::Quiet, resets),
            );
        }
    }
    rows.into_iter().map(|row| row.clipped(columns)).collect()
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

/// What a window is called on its row: crucible's name for it, never words a
/// response chose.
const fn named(window: Window) -> &'static str {
    match window {
        Window::FiveHour => "5-hour window",
        Window::Weekly => "Weekly window",
        Window::Monthly => "Monthly window",
    }
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
