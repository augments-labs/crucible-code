//! `/settings`: what is in force, every setting a menu can change, and what the
//! session has used, as three tabs of one panel.
//!
//! **Status** reads and changes nothing: the version, who answers and with
//! which credential, where the session works and is kept, and the sandbox and
//! permission mode. Those last two loosen what runs unasked, and a menu is not
//! a permission decision, so they are shown here and changed only by `/sandbox`
//! and `/mode`. **Config** is one row per setting the configuration declares
//! a row for — the list and the reasons for every key without one belong to
//! the configuration crate, which holds the two together. **Usage** is the
//! body of `/usage`, drawn at the panel's width; turning to it between turns
//! asks the plan as opening `/usage` does, and draws the answer when it comes;
//! closed before it came, the panel gives the question up as `/usage` does.
//!
//! **A change is a request.** Enter or space asks the application, through the
//! client contract, to write one key to the user's own file — the file and the
//! splice `/theme` writes with. What a running session can take is put in
//! force the moment it is written: the theme, the syntax theme, the glyphs,
//! the tool detail, the scroll rail, the wheel's speed and the key that sends.
//! What only a start reads says so on its row, changed or not, and [`takes`]
//! is where each row is given one answer or the other. A file that cannot be
//! written leaves a live change in force for this session and says so on the
//! panel.
//!
//! **What somebody else decided is locked.** A row a project file or the shell
//! states is shown with who set it and changes nothing: writing the user's
//! file under it would be an answer that looks taken and does nothing.
//!
//! What the start read is not read again, so what this panel wrote is kept on
//! the terms, one entry a row, and a panel opened later shows it.

use std::borrow::Cow;

use crucible_app::Conversation;
use crucible_app::client::{Performed, Setting};
use crucible_client_api::{self as api, Name};
use crucible_config::{Forced, Row as Setting_, RowId, Values};
use crucible_tools::Mode;
use crucible_tui::{
    Caret, Glyphs, Key, Pressed, Renderer, Row, Slot, TabRow, Terminal, columns, fold,
};

use crate::cli::client::{Out, astray};
use crate::cli::style::glyph_set;
use crate::cli::{Fatal, sends};

use super::region::{self, Ended, Moved};
use super::usage::{self, Clock};
use super::{Counted, HUNG, Terms, about, resume, theme};

#[cfg(test)]
mod tests;

/// The narrowest window the panel stands in. Below it the tab row and the
/// search box no longer read as what they are, and the values are listed
/// instead.
const NARROWEST: usize = 20;

/// The most a search holds. A label is a few words, so anything longer
/// matches nothing and is only kept.
const QUERY: usize = 64;

/// How wide a Status label is drawn, so the values start in one column.
const LABEL: usize = 21;

/// The fewest columns a path is drawn beside its label in. Narrower than
/// this, a shortened path is a mark and a name cut short, and it goes under
/// the label instead.
const PATH: usize = 16;

/// The tabs, in the order the row draws them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Config,
    Usage,
}

impl Tab {
    const EVERY: [Self; 3] = [Self::Status, Self::Config, Self::Usage];

    const fn name(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Config => "Config",
            Self::Usage => "Usage",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Status => Self::Config,
            Self::Config => Self::Usage,
            Self::Usage => Self::Status,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Status => Self::Usage,
            Self::Config => Self::Status,
            Self::Usage => Self::Config,
        }
    }
}

/// One setting as the panel holds it.
struct Line {
    row: &'static Setting_,
    /// Spelled as a document spells it: `true`, `dark`, `6`.
    value: String,
    /// Who decided it, where the user's own file does not.
    forced: Option<Forced>,
    /// Read only at the next start, which its row says.
    later: bool,
}

/// One row of the Status tab.
struct Fact {
    label: &'static str,
    value: String,
    /// A path, kept to its root and the name it ends in where it does not
    /// fit, rather than folded: a folded path is several rows of directories
    /// nobody came to read.
    path: bool,
}

impl Fact {
    fn said(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            path: false,
        }
    }

    /// `path` with the home directory written `~`, as the resume picker
    /// writes it.
    fn path(label: &'static str, path: &std::path::Path, home: Option<&std::path::Path>) -> Self {
        Self {
            label,
            value: resume::homed(path, home),
            path: true,
        }
    }
}

/// A tab's rows before they are windowed: each with whether it counts toward
/// `↓ N more`, and the first and last of the rows the mark stands on.
struct Body {
    rows: Vec<(Row, bool)>,
    anchor: Option<(usize, usize)>,
}

impl Body {
    /// Rows that each count, with no mark to keep in sight.
    fn counted(rows: impl IntoIterator<Item = Row>) -> Self {
        Self {
            rows: rows.into_iter().map(|row| (row, true)).collect(),
            anchor: None,
        }
    }
}

/// A choice opened under its row: the words it offers and the one marked.
struct Opened {
    options: Vec<String>,
    at: usize,
}

/// Everything the panel shows and where the keys have left it.
struct Panel {
    tab: Tab,
    lines: Vec<Line>,
    /// Which of the rows the search leaves is marked.
    marked: usize,
    query: String,
    searching: bool,
    opened: Option<Opened>,
    /// The row and the word a key asked for, until the caller writes it.
    asked: Option<(usize, String)>,
    /// One sentence in place of the footer, until the next key.
    note: Option<String>,
    /// The first row of the tab's rows on screen, and the furthest it goes.
    scrolled: usize,
    furthest: usize,
    status: Vec<Fact>,
    heading: String,
    usage: api::Usage,
    clock: Clock,
    /// The question out to the plan, until it is answered.
    out: Option<Out>,
    /// The tab the panel was last looked at on with no key pressed, so that
    /// turning to Usage is seen once.
    looked: Option<Tab>,
    /// The syntax themes this build reads code in.
    themes: Vec<String>,
}

/// Opens the panel between turns, or lists the values where there is no
/// keyboard to walk it with.
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
    if !keys {
        return listed(renderer, terms);
    }
    let usage = match terms.perform(conversation, api::Command::Usage) {
        Performed::Usage(usage) => *usage,
        other => return Ok(renderer.commit(&astray(&other))?),
    };
    let counted = Counted {
        usage,
        serving: conversation.serving(),
        mode: conversation.runner().mode(),
        session: conversation.session().id().cloned(),
    };
    let mut watch = |panel: &mut Panel| panel.watched(terms, conversation);
    let out = stood(renderer, terms, &counted, |_| Ok(()), Some(&mut watch))?;
    usage::closed(out, terms, conversation);
    Ok(())
}

/// Opens the panel over a running turn, with what that turn last reported.
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
    // Nothing is asked over a running turn, so nothing is out once it closes.
    stood(renderer, terms, counted, while_waiting, None).map(drop)
}

/// Stands the panel until it is closed, writing each change as it is asked
/// for, and leaves a row for each change behind it. With `watch`, the panel
/// is looked at again every [`usage::BEAT`] with no key pressed; mid-turn
/// there is no watch, and `while_waiting` keeps the turn's text moving.
///
/// Hands back the question the Usage tab put to the plan, where it was still
/// out as the panel closed, for the caller to end with [`usage::closed`].
fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    counted: &Counted,
    mut while_waiting: impl FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
    mut watch: Option<&mut dyn FnMut(&mut Panel) -> Moved>,
) -> Result<Option<Out>, Fatal> {
    let mut panel = Panel::new(terms, counted);
    let mut changed: Vec<String> = Vec::new();
    loop {
        let laid =
            |panel: &mut Panel, columns, room| panel.laid(columns, room, terms.style().glyphs());
        let ended = match watch.as_mut() {
            Some(watch) => region::stand_watching(
                renderer,
                |_| terms.style(),
                &mut panel,
                laid,
                |pressed, panel| panel.walked(pressed),
                usage::BEAT,
                |panel| watch(panel),
            )?,
            None => region::stand_while(
                renderer,
                |_| terms.style(),
                &mut panel,
                laid,
                |pressed, panel| panel.walked(pressed),
                &mut while_waiting,
            )?,
        };
        match ended {
            Ended::Took => {
                if let Some(said) = settle(renderer, terms, &mut panel) {
                    // One row a setting, however often it was changed.
                    let label = said.split(" set to ").next().unwrap_or_default();
                    changed.retain(|row| !row.starts_with(&format!("{label} set to ")));
                    changed.push(said);
                }
            }
            Ended::Left => {
                left(renderer, &changed)?;
                return Ok(panel.out.take());
            }
            Ended::Cramped => {
                left(renderer, &changed)?;
                listed(renderer, terms)?;
                return Ok(panel.out.take());
            }
        }
    }
}

/// The rows a closed panel leaves: one for each setting it changed.
fn left<T: Terminal>(renderer: &mut Renderer<T>, changed: &[String]) -> Result<(), Fatal> {
    let columns = renderer.transcript_columns().saturating_sub(HUNG);
    let rows: Vec<Row> = changed
        .iter()
        .flat_map(|said| fold(said, columns))
        .map(|part| Row::new().then(Slot::Quiet, part))
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    Ok(renderer.present(&rows)?)
}

/// Every setting and its value, written where no panel can stand.
fn listed<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<(), Fatal> {
    let glyphs = terms.style().glyphs();
    let columns = renderer.transcript_columns().saturating_sub(HUNG);
    let rows: Vec<Row> = crucible_config::rows()
        .iter()
        .map(|row| Line::read(terms, row))
        .flat_map(|line| {
            let said = about(line.row.label(), &line.worded(glyphs), glyphs);
            fold(&said, columns)
                .into_iter()
                .map(Row::plain)
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(renderer.present(&rows)?)
}

/// Writes what the panel asked for, through the client contract, and puts it
/// in force where the running session can take it. The row the transcript is
/// left, where something was changed.
fn settle<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    panel: &mut Panel,
) -> Option<String> {
    let (at, word) = panel.asked.take()?;
    let glyphs = terms.style().glyphs();
    let line = panel.lines.get_mut(at)?;
    let row = line.row;
    let asked = match (Name::new(row.key()), Name::new(&word)) {
        (Ok(name), Ok(value)) => terms.keep(api::Command::Setting { name, value }),
        (Err(refused), _) | (_, Err(refused)) => Performed::Refused(refused),
    };
    match asked {
        Performed::Setting(Setting::Remembered) => {
            line.later = match takes(row.id()) {
                Takes::Now => !worn(row, &word, renderer, terms),
                Takes::NextStart => true,
                Takes::NextUse => false,
            };
            line.value.clone_from(&word);
            kept(terms, row, word);
            let said = format!("{} set to {}", row.label(), line.worded(glyphs));
            Some(if line.later {
                format!("{said} {} applies at next start", glyphs.dot())
            } else {
                said
            })
        }
        Performed::Setting(Setting::Unwritten(problem)) => {
            let note = format!("! not written: {problem}");
            panel.note = Some(if worn(row, &word, renderer, terms) {
                line.value.clone_from(&word);
                kept(terms, row, word);
                format!("{note} {} in force for this session only", glyphs.dot())
            } else {
                note
            });
            None
        }
        Performed::Setting(Setting::Forced(by)) => {
            line.forced = Some(by);
            panel.note = Some(locked(by).to_owned());
            None
        }
        other => {
            panel.note = Some(astray(&other));
            None
        }
    }
}

/// When a change to a row reaches the running session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// At once: the running session reads it again, and [`worn`] puts it there.
    Now,
    /// Only a new start reads it, and its row says so.
    NextStart,
    /// Read only when it is next used, which is at a start anyway, so there is
    /// nothing to wait for and its row says nothing.
    NextUse,
}

/// When a change to the row `id` reaches the running session.
///
/// [`worn`] matches the same identity to put a change in force, and a test
/// holds the two to one answer. Colour is a new start's: the palette's depth
/// is settled from the terminal and the environment, and the syntax theme's
/// colours are only read where colour is on, so turning it on or off is a
/// palette resolved again rather than a value set on the one in force.
const fn takes(id: RowId) -> Takes {
    match id {
        RowId::Theme
        | RowId::SyntaxTheme
        | RowId::Glyphs
        | RowId::ToolDetail
        | RowId::ScrollRail
        | RowId::ScrollSpeed
        | RowId::Send => Takes::Now,
        RowId::Colour
        | RowId::Tone
        | RowId::Compaction
        | RowId::CacheMode
        | RowId::CacheIsolation
        | RowId::CacheRetention
        | RowId::CachePersistent => Takes::NextStart,
        RowId::UpdateCheck => Takes::NextUse,
    }
}

/// Puts `word` in force for `row` now, where the running session reads it
/// again; `false` for a setting only a new start reads.
fn worn<T: Terminal>(
    row: &Setting_,
    word: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> bool {
    match row.id() {
        RowId::Theme => theme::wear(word, renderer, terms),
        RowId::SyntaxTheme => theme::read_in(word, renderer, terms),
        RowId::ScrollRail => {
            renderer.rails(word == "true");
            true
        }
        RowId::ScrollSpeed => word
            .parse::<u16>()
            .map(|rows| renderer.rolls(i32::from(rows)))
            .is_ok(),
        RowId::Glyphs => crucible_config::Glyphs::read(word)
            .map(|wanted| {
                let glyphs = glyph_set(Some(wanted));
                terms.style.set(terms.style().drawing(glyphs));
                renderer.draws(glyphs);
            })
            .is_some(),
        RowId::ToolDetail => crucible_config::ToolDetail::read(word)
            .map(|detail| terms.style.set(terms.style().detailing(detail)))
            .is_some(),
        // The editor is the loop's, which hands it this once the panel closes.
        RowId::Send => crucible_config::Sending::read(word)
            .map(|said| terms.sending.set(sends(Some(said))))
            .is_some(),
        RowId::Colour
        | RowId::Tone
        | RowId::Compaction
        | RowId::UpdateCheck
        | RowId::CacheMode
        | RowId::CacheIsolation
        | RowId::CacheRetention
        | RowId::CachePersistent => false,
    }
}

/// Keeps `word` as what `row` was set to this session.
fn kept(terms: &Terms, row: &'static Setting_, word: String) {
    let mut settled = terms.settled.borrow_mut();
    settled.retain(|(key, _)| *key != row.key());
    settled.push((row.key(), word));
}

/// The sentence a locked row answers a key with.
const fn locked(by: Forced) -> &'static str {
    match by {
        Forced::Project => "this value is set by the project config",
        Forced::Environment => "this value is set by the environment",
    }
}

impl Line {
    /// `row` as this session has it.
    fn read(terms: &Terms, row: &'static Setting_) -> Self {
        let from = |name: &str| (terms.environment)(name);
        // The two a running session can say it has in force, since `/theme`
        // changes them too; every other row's running value is what was
        // settled here, or what the start read.
        let shown = match row.id() {
            RowId::Theme => theme::worn(terms).map(str::to_owned),
            RowId::SyntaxTheme => terms.reading.borrow().clone(),
            RowId::Glyphs
            | RowId::Colour
            | RowId::ToolDetail
            | RowId::ScrollRail
            | RowId::ScrollSpeed
            | RowId::Send
            | RowId::Tone
            | RowId::Compaction
            | RowId::UpdateCheck
            | RowId::CacheMode
            | RowId::CacheIsolation
            | RowId::CacheRetention
            | RowId::CachePersistent => None,
        };
        let settled = terms
            .settled
            .borrow()
            .iter()
            .find(|(key, _)| *key == row.key())
            .map(|(_, word)| word.clone());
        let value = shown
            .or(settled)
            .or_else(|| terms.settings.stated(row, &from))
            .or_else(|| row.usual().map(str::to_owned))
            .unwrap_or_default();
        Self {
            row,
            value,
            forced: terms.settings.forced(row, &from),
            later: takes(row.id()) == Takes::NextStart,
        }
    }

    /// The value as a row shows it: `on`, `‹ 6 ›`, `dark`.
    fn worded(&self, glyphs: Glyphs) -> String {
        match self.row.values() {
            Values::Flag => if self.value == "true" { "on" } else { "off" }.to_owned(),
            Values::Whole { .. } => {
                let (open, close) = glyphs.bracketing();
                format!("{open} {} {close}", self.value)
            }
            Values::Choice(_) | Values::Named => self.value.clone(),
        }
    }

    /// What is said after the value: who set it, or when it is read.
    fn aside(&self) -> Option<&'static str> {
        match self.forced {
            Some(Forced::Project) => Some("set by project config"),
            Some(Forced::Environment) => Some("set by the environment"),
            None => self.later.then_some("applies at next start"),
        }
    }
}

impl Panel {
    fn new(terms: &Terms, counted: &Counted) -> Self {
        let glyphs = terms.style().glyphs();
        let serving = counted.serving;
        let signed = serving.and_then(|name| usage::signed(terms, name));
        let heading = serving.map_or_else(String::new, |name| match &signed {
            Some(words) => format!("{name} {} {words}", glyphs.dot()),
            None => name.to_owned(),
        });
        Self {
            tab: Tab::Config,
            lines: crucible_config::rows()
                .iter()
                .map(|row| Line::read(terms, row))
                .collect(),
            marked: 0,
            query: String::new(),
            searching: false,
            opened: None,
            asked: None,
            note: None,
            scrolled: 0,
            furthest: 0,
            status: status(
                terms,
                counted,
                signed,
                std::env::home_dir().as_deref(),
                glyphs,
            ),
            heading,
            usage: counted.usage.clone(),
            clock: Clock::system(),
            out: None,
            looked: None,
            themes: crucible_tui::syntax::every_theme(),
        }
    }

    /// The lines the search leaves, by their place in [`Panel::lines`].
    fn shown(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.row.label().to_lowercase().contains(&query))
            .map(|(at, _)| at)
            .collect()
    }

    /// The line the mark stands on.
    fn marked_line(&self) -> Option<usize> {
        self.shown().get(self.marked).copied()
    }

    #[cfg(test)]
    fn marked_label(&self) -> Option<&'static str> {
        let at = self.marked_line()?;
        self.lines.get(at).map(|line| line.row.label())
    }

    #[cfg(test)]
    fn asked_word(&self) -> Option<&str> {
        self.asked.as_ref().map(|(_, word)| word.as_str())
    }

    /// What happened with no key pressed: the Usage tab turned to, which
    /// asks the plan, or the plan's answer come.
    fn watched(&mut self, terms: &Terms, conversation: &mut Conversation) -> Moved {
        let turned = self.tab == Tab::Usage && self.looked != Some(Tab::Usage);
        self.looked = Some(self.tab);
        if turned && self.out.is_none() {
            self.out = terms.ask_limits(conversation);
            if self.out.is_some() {
                return Moved::Redraw;
            }
        }
        usage::answered_in(&mut self.out, &mut self.usage, terms, conversation, false)
    }

    /// What one key does.
    fn walked(&mut self, pressed: Pressed) -> Moved {
        let noted = self.note.take().is_some();
        let moved = if self.opened.is_some() {
            self.choosing(&pressed)
        } else if self.searching {
            self.typing(pressed)
        } else {
            self.walking(&pressed)
        };
        if noted && moved == Moved::Still {
            Moved::Redraw
        } else {
            moved
        }
    }

    /// A key while a choice is open under its row.
    fn choosing(&mut self, pressed: &Pressed) -> Moved {
        let Some(opened) = self.opened.as_mut() else {
            return Moved::Still;
        };
        match pressed {
            Pressed::Up => {
                let next = opened.at.checked_sub(1);
                region::step(&mut opened.at, next)
            }
            Pressed::Down => {
                let next = opened.at.saturating_add(1);
                region::step(
                    &mut opened.at,
                    (next < opened.options.len()).then_some(next),
                )
            }
            Pressed::Key(Key::Enter | Key::Char(' ')) => {
                let picked = opened.options.get(opened.at).cloned();
                self.opened = None;
                match (self.marked_line(), picked) {
                    (Some(at), Some(word))
                        if self.lines.get(at).is_some_and(|line| line.value != word) =>
                    {
                        self.asked = Some((at, word));
                        Moved::Took
                    }
                    _ => Moved::Redraw,
                }
            }
            Pressed::Escape => {
                self.opened = None;
                Moved::Redraw
            }
            Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
            Pressed::Resized => Moved::Redraw,
            _ => Moved::Still,
        }
    }

    /// A key while the search box has it.
    fn typing(&mut self, pressed: Pressed) -> Moved {
        match pressed {
            Pressed::Key(Key::Char(letter)) => {
                if self.query.len() < QUERY && !letter.is_control() {
                    self.query.push(letter);
                }
                self.marked = 0;
                Moved::Redraw
            }
            Pressed::Pasted(text) => {
                for letter in text.chars().filter(|letter| !letter.is_control()) {
                    if self.query.len() >= QUERY {
                        break;
                    }
                    self.query.push(letter);
                }
                self.marked = 0;
                Moved::Redraw
            }
            Pressed::Key(Key::Backspace) => {
                self.query.pop();
                self.marked = 0;
                Moved::Redraw
            }
            Pressed::Down | Pressed::Key(Key::Enter) => {
                self.searching = false;
                Moved::Redraw
            }
            Pressed::Escape => {
                self.query.clear();
                self.searching = false;
                self.marked = 0;
                Moved::Redraw
            }
            Pressed::Tab | Pressed::Cycle => {
                self.searching = false;
                self.walking(&pressed)
            }
            Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
            Pressed::Resized => Moved::Redraw,
            _ => Moved::Still,
        }
    }

    /// A key while the rows, or a tab other than Config, have it.
    fn walking(&mut self, pressed: &Pressed) -> Moved {
        let config = self.tab == Tab::Config;
        match pressed {
            Pressed::Tab => self.turn(self.tab.next()),
            Pressed::Cycle => self.turn(self.tab.previous()),
            Pressed::Key(Key::Left) => self
                .stepped(false)
                .unwrap_or_else(|| self.turn(self.tab.previous())),
            Pressed::Key(Key::Right) => self
                .stepped(true)
                .unwrap_or_else(|| self.turn(self.tab.next())),
            Pressed::Up if config => {
                let next = self.marked.checked_sub(1);
                region::step(&mut self.marked, next)
            }
            Pressed::Down if config => {
                let next = self.marked.saturating_add(1);
                let next = (next < self.shown().len()).then_some(next);
                region::step(&mut self.marked, next)
            }
            Pressed::Up => {
                let next = self.scrolled.checked_sub(1);
                region::step(&mut self.scrolled, next)
            }
            Pressed::Down => {
                let next = self.scrolled.saturating_add(1);
                region::step(&mut self.scrolled, (next <= self.furthest).then_some(next))
            }
            Pressed::Key(Key::Char('/')) if config => {
                self.searching = true;
                Moved::Redraw
            }
            Pressed::Key(Key::Enter | Key::Char(' ')) if config => self.changed(),
            Pressed::Escape if config && !self.query.is_empty() => {
                self.query.clear();
                self.marked = 0;
                Moved::Redraw
            }
            Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
            Pressed::Resized => Moved::Redraw,
            _ => Moved::Still,
        }
    }

    fn turn(&mut self, tab: Tab) -> Moved {
        self.tab = tab;
        self.scrolled = 0;
        Moved::Redraw
    }

    /// The arrows on the number row step it, one at a time and no further
    /// than its bounds; `None` where the mark is on any other row.
    fn stepped(&mut self, up: bool) -> Option<Moved> {
        if self.tab != Tab::Config {
            return None;
        }
        let at = self.marked_line()?;
        let line = self.lines.get(at)?;
        let Values::Whole { least, most } = line.row.values() else {
            return None;
        };
        if let Some(by) = line.forced {
            self.note = Some(locked(by).to_owned());
            return Some(Moved::Redraw);
        }
        let now: u16 = line.value.parse().unwrap_or(least);
        let next = if up {
            now.checked_add(1)
        } else {
            now.checked_sub(1)
        };
        Some(match next.filter(|next| (least..=most).contains(next)) {
            Some(next) => {
                self.asked = Some((at, next.to_string()));
                Moved::Took
            }
            None => Moved::Still,
        })
    }

    /// Enter or space on the marked row.
    fn changed(&mut self) -> Moved {
        let Some(at) = self.marked_line() else {
            return Moved::Still;
        };
        let Some(line) = self.lines.get(at) else {
            return Moved::Still;
        };
        if let Some(by) = line.forced {
            self.note = Some(locked(by).to_owned());
            return Moved::Redraw;
        }
        let word = match line.row.values() {
            Values::Flag => if line.value == "true" {
                "false"
            } else {
                "true"
            }
            .to_owned(),
            Values::Choice(words) if words.len() <= 3 => {
                let now = words.iter().position(|word| *word == line.value);
                let next = now.map_or(0, |now| now.saturating_add(1) % words.len());
                words.get(next).copied().unwrap_or_default().to_owned()
            }
            Values::Whole { least, most } => {
                let now: u16 = line.value.parse().unwrap_or(least);
                if now >= most {
                    least
                } else {
                    now.saturating_add(1)
                }
                .to_string()
            }
            Values::Choice(words) => {
                return self.open(words.iter().map(|word| (*word).to_owned()).collect(), at);
            }
            Values::Named => return self.open(self.themes.clone(), at),
        };
        self.asked = Some((at, word));
        Moved::Took
    }

    fn open(&mut self, options: Vec<String>, at: usize) -> Moved {
        let now = self.lines.get(at).map(|line| line.value.as_str());
        let marked = options.iter().position(|word| Some(word.as_str()) == now);
        self.opened = Some(Opened {
            options,
            at: marked.unwrap_or(0),
        });
        Moved::Redraw
    }

    /// The panel at `columns` by `room`, or nothing where it does not fit.
    fn laid(&mut self, columns: usize, room: usize, glyphs: Glyphs) -> (Vec<Row>, Option<Caret>) {
        if columns < NARROWEST {
            return (Vec::new(), None);
        }
        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
            tabs(self.tab, columns, glyphs),
            Row::new(),
        ];
        let mut caret = None;
        // An opened choice has the keys, and the room the box would take.
        if self.tab == Tab::Config && self.opened.is_none() {
            let (top, middle, bottom) = self.search(columns, glyphs);
            if self.searching {
                caret = Some(Caret {
                    row: rows.len().saturating_add(1),
                    column: 2usize
                        .saturating_add(columns_of(&middle))
                        .min(columns.saturating_sub(2)),
                });
            }
            rows.extend([
                top,
                boxed(&middle, self.query.is_empty(), columns, glyphs),
                bottom,
                Row::new(),
            ]);
        }
        let footer = self.footer(columns, glyphs);
        let Some(left) = room
            .checked_sub(rows.len().saturating_add(1).saturating_add(footer.len()))
            .filter(|left| *left > 0)
        else {
            return (Vec::new(), None);
        };
        let body = match self.tab {
            Tab::Config => self.config(columns, glyphs),
            Tab::Status => Body {
                rows: self.status_rows(columns, glyphs),
                anchor: None,
            },
            Tab::Usage => Body::counted(usage::body(
                &self.heading,
                &self.usage,
                self.out.as_ref().map(Out::provider),
                columns,
                glyphs,
                &self.clock,
            )),
        };
        let Some(window) = self.window(body, left, columns, glyphs) else {
            return (Vec::new(), None);
        };
        rows.extend(window);
        rows.push(Row::new());
        rows.extend(footer);
        (rows, caret)
    }

    /// The search box's three rows, the middle one as its words alone.
    fn search(&self, columns: usize, glyphs: Glyphs) -> (Row, String, Row) {
        let across = glyphs.horizontal().repeat(columns.saturating_sub(2));
        let (left, right) = glyphs.top();
        let top = Row::new().then(Slot::Quiet, format!("{left}{across}{right}"));
        let (left, right) = glyphs.bottom();
        let bottom = Row::new().then(Slot::Quiet, format!("{left}{across}{right}"));
        let inside = columns.saturating_sub(4);
        let words = if self.query.is_empty() {
            "/ to search".to_owned()
        } else {
            crucible_tui::clip(&self.query, inside).to_owned()
        };
        (top, words, bottom)
    }

    /// The Config rows, one entry for each setting the search leaves.
    fn config(&self, columns: usize, glyphs: Glyphs) -> Body {
        let shown = self.shown();
        if shown.is_empty() {
            let row = Row::new()
                .then(Slot::Quiet, "  no setting is called that")
                .clipped(columns);
            return Body {
                rows: vec![(row, false)],
                anchor: None,
            };
        }
        let mut rows = Vec::new();
        let mut anchor = None;
        for (place, at) in shown.iter().enumerate() {
            let Some(line) = self.lines.get(*at) else {
                continue;
            };
            let marked = place == self.marked;
            let first = rows.len();
            let opened = self.opened.as_ref().filter(|_| marked);
            for (number, row) in entry(line, marked, opened.is_some(), columns, glyphs)
                .into_iter()
                .enumerate()
            {
                rows.push((row, number == 0));
            }
            let mut last = rows.len().saturating_sub(1);
            if let Some(opened) = opened {
                for (number, option) in opened.options.iter().enumerate() {
                    let row = if number == opened.at {
                        last = rows.len();
                        Row::new()
                            .then(Slot::Plain, "    ")
                            .then(Slot::Accent, glyphs.caret())
                            .then(Slot::Plain, " ")
                            .then(Slot::Strong, option.as_str())
                    } else {
                        Row::new().then(Slot::Plain, format!("      {option}"))
                    };
                    rows.push((row.clipped(columns), true));
                }
            }
            if marked {
                anchor = Some((first, last));
            }
        }
        Body { rows, anchor }
    }

    /// The Status rows: a label and its value beside it, or under it where the
    /// two do not fit.
    fn status_rows(&self, columns: usize, glyphs: Glyphs) -> Vec<(Row, bool)> {
        let beside = 2 + LABEL;
        let mut rows = Vec::new();
        for Fact { label, value, path } in &self.status {
            let room = columns.saturating_sub(beside);
            let value = if *path && room >= PATH {
                crucible_tui::shorten(value, room, glyphs)
            } else {
                Cow::Borrowed(value.as_str())
            };
            if beside.saturating_add(columns_of(&value)) <= columns {
                let mut row = Row::new().then(Slot::Quiet, format!("  {label}"));
                row.pad(beside);
                rows.push((row.then(Slot::Plain, value), true));
                continue;
            }
            rows.push((
                Row::new()
                    .then(Slot::Quiet, format!("  {label}"))
                    .clipped(columns),
                true,
            ));
            let under = columns.saturating_sub(4).max(1);
            let parts: Vec<Cow<'_, str>> = if *path {
                vec![crucible_tui::shorten(&value, under, glyphs)]
            } else {
                fold(&value, under).into_iter().map(Cow::Borrowed).collect()
            };
            for part in parts {
                rows.push((
                    Row::new()
                        .then(Slot::Plain, format!("    {part}"))
                        .clipped(columns),
                    false,
                ));
            }
        }
        rows
    }

    /// As many of the body's rows as `room` holds, kept around its anchor
    /// where it has one, with a row saying how many more are below.
    fn window(
        &mut self,
        body: Body,
        room: usize,
        columns: usize,
        glyphs: Glyphs,
    ) -> Option<Vec<Row>> {
        let Body { rows, anchor } = body;
        if rows.len() <= room {
            self.scrolled = 0;
            self.furthest = 0;
            return Some(rows.into_iter().map(|(row, _)| row).collect());
        }
        let fit = room.checked_sub(1).filter(|fit| *fit > 0)?;
        let furthest = rows.len().saturating_sub(fit);
        if let Some((first, last)) = anchor {
            if last >= self.scrolled.saturating_add(fit) {
                self.scrolled = last.saturating_add(1).saturating_sub(fit);
            }
            if first < self.scrolled {
                self.scrolled = first;
            }
        }
        self.scrolled = self.scrolled.min(furthest);
        self.furthest = furthest;
        let below = rows
            .iter()
            .skip(self.scrolled.saturating_add(fit))
            .filter(|(_, counts)| *counts)
            .count();
        let mut shown: Vec<Row> = rows
            .into_iter()
            .skip(self.scrolled)
            .take(fit)
            .map(|(row, _)| row)
            .collect();
        if below > 0 {
            let (_, down) = glyphs.walking();
            shown.push(
                Row::new()
                    .then(Slot::Quiet, format!("  {down} {below} more"))
                    .clipped(columns),
            );
        }
        Some(shown)
    }

    /// The rows under the panel naming the keys, or the note in their place.
    fn footer(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let dot = glyphs.dot();
        let (up, down) = glyphs.walking();
        let (back, on) = glyphs.stepping();
        let folded = |said: &str| -> Vec<Row> {
            fold(said, columns)
                .into_iter()
                .map(|part| Row::new().then(Slot::Quiet, part))
                .collect()
        };
        if let Some(note) = &self.note {
            return folded(note);
        }
        // The words in full where they fit on one row, else their short form.
        let either = |wide: String, short: String| {
            folded(if columns_of(&wide) <= columns {
                &wide
            } else {
                &short
            })
        };
        let tabs = format!("{back}{on} tabs {dot} esc to close");
        match self.tab {
            Tab::Status | Tab::Usage => folded(&tabs),
            Tab::Config if self.opened.is_some() => either(
                format!("{up}{down} to walk {dot} enter picks {dot} esc to leave it as it was"),
                format!("{up}{down} walk {dot} enter picks {dot} esc leaves it"),
            ),
            Tab::Config if self.searching => either(
                format!("type to filter {dot} {down} to the rows {dot} esc to clear"),
                format!("type to filter {dot} {down} rows {dot} esc clears"),
            ),
            Tab::Config => {
                let wide = format!(
                    "{up}{down} to walk {dot} enter changes {dot} / to search {dot} {tabs}"
                );
                if columns_of(&wide) <= columns {
                    return folded(&wide);
                }
                let mut rows = folded(&format!(
                    "{up}{down} walk {dot} enter changes {dot} / search"
                ));
                rows.extend(folded(&tabs));
                rows
            }
        }
    }
}

/// The width of `text` in columns.
fn columns_of(text: &str) -> usize {
    columns(text)
}

/// The search box's middle row: its words, quiet where they are the prompt.
fn boxed(words: &str, prompt: bool, columns: usize, glyphs: Glyphs) -> Row {
    let side = glyphs.vertical();
    let slot = if prompt { Slot::Quiet } else { Slot::Plain };
    let mut row = Row::new()
        .then(Slot::Quiet, side)
        .then(Slot::Plain, " ")
        .then(slot, words);
    row.pad(columns.saturating_sub(1));
    row.then(Slot::Quiet, side).clipped(columns)
}

/// The row of tabs, the one in front in brackets so it reads without colour.
///
/// Where every tab does not fit, the others give way before the open one: a
/// reader who cannot see which tab is open cannot tell what is under it.
fn tabs(tab: Tab, columns: usize, glyphs: Glyphs) -> Row {
    TabRow {
        heading: Some("Settings"),
        names: &Tab::EVERY.map(Tab::name),
        open: Tab::EVERY
            .iter()
            .position(|one| *one == tab)
            .unwrap_or_default(),
        marks: Some(glyphs.bracketing()),
    }
    .row(columns)
}

/// One setting's rows: its label and value on one row where they fit, and
/// the value, or what is said after it, under the label where they do not.
fn entry(line: &Line, marked: bool, opened: bool, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut head = if marked && !opened {
        Row::new()
            .then(Slot::Accent, glyphs.caret())
            .then(Slot::Plain, " ")
    } else {
        Row::new().then(Slot::Plain, "  ")
    };
    head.push(
        if marked { Slot::Strong } else { Slot::Plain },
        line.row.label(),
    );
    if opened {
        return vec![head.clipped(columns)];
    }
    let value = line.worded(glyphs);
    let tone = if marked { Slot::Accent } else { Slot::Quiet };
    let aside = line
        .aside()
        .map(|aside| format!(" {} {aside}", glyphs.dot()));
    let lead = head.columns().saturating_add(2);
    let whole = columns_of(&value).saturating_add(aside.as_deref().map_or(0, columns_of));

    if lead.saturating_add(whole) <= columns {
        head.pad(columns.saturating_sub(whole));
        head.push(tone, value);
        if let Some(aside) = aside {
            head.push(Slot::Quiet, aside);
        }
        return vec![head];
    }
    let mut rows = Vec::new();
    if lead.saturating_add(columns_of(&value)) <= columns {
        head.pad(columns.saturating_sub(columns_of(&value)));
        head.push(tone, value);
        rows.push(head);
    } else {
        rows.push(head.clipped(columns));
        rows.push(
            Row::new()
                .then(Slot::Plain, "    ")
                .then(tone, value)
                .clipped(columns),
        );
    }
    if let Some(aside) = line.aside() {
        rows.push(
            Row::new()
                .then(Slot::Quiet, format!("    {aside}"))
                .clipped(columns),
        );
    }
    rows
}

/// The Status tab's rows, read once as the panel opens.
fn status(
    terms: &Terms,
    counted: &Counted,
    signed: Option<String>,
    home: Option<&std::path::Path>,
    glyphs: Glyphs,
) -> Vec<Fact> {
    let dot = glyphs.dot();
    let model = counted
        .usage
        .context
        .model
        .as_ref()
        .map_or("none", |model| model.text().as_str());
    let model = match counted.serving {
        Some(provider) => format!("{model} {dot} {provider}"),
        None => model.to_owned(),
    };
    let sandbox = terms.settings.sandbox();
    let on = if sandbox.enabled() { "on" } else { "off" };
    let confined = if sandbox.required_by_project() {
        format!("{on} {dot} required by project config")
    } else {
        on.to_owned()
    };
    vec![
        Fact::said("Version", env!("CARGO_PKG_VERSION").to_owned()),
        Fact::said("Model", model),
        Fact::said("Sign-in", signed.unwrap_or_else(|| "none".to_owned())),
        Fact::path("Working directory", terms.workspace.root(), home),
        Fact::path("Config file", &terms.choosing, home),
        // Its first eight, as a reader names one: enough to tell the sessions
        // of one directory apart, and short enough to stand beside its label.
        Fact::said(
            "Session",
            counted.session.as_ref().map_or_else(
                || "not recorded".to_owned(),
                |id| id.as_str().chars().take(8).collect(),
            ),
        ),
        Fact::said("Sandbox", confined),
        Fact::said("Permission mode", mode(counted.mode).to_owned()),
    ]
}

/// The mode as configuration spells it.
const fn mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Ask => "ask",
        Mode::AllowEdits => "allowEdits",
        Mode::FullAccess => "fullAccess",
    }
}
