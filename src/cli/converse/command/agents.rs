//! `/agents`: the coding agents the agent catalog offers, searched by name or
//! licence, each with where it comes from, and one added after a yes.
//!
//! Opening it asks the agents door for the catalog through
//! [`Terms::carry`], which reads it again whatever `updates.check` says, and
//! stands the answer as a [`Picker`]: one entry an agent, in the door's order,
//! its key and newest version over its licence and what the install store
//! holds of it; where the window is wide enough for a preview pane, it says
//! the marked agent's licence, terms, caution and store whole, which the list
//! pane beside it is too narrow to. What is typed narrows the entries to the
//! agents whose key or licence holds it, ASCII case aside. The drawing is the picker's and the
//! search line is an [`Editor`]; what is here is which entries a query keeps
//! and what each key moves, in [`Listing`].
//!
//! Taking an entry opens the agent's own screen, which is its provenance: the
//! versions offered, the licence, the terms link and the caution, the versions
//! held with a receipt, and whether a sign-in made under the sandbox is kept.
//! **Add** asks the door to install the newest version offered and write one
//! more instance of the agent, the one question it stops on answered and the
//! download stood as `/login`'s agent screen answers and stands them; the line
//! after names the instance written, and the catalog is asked for again so the
//! entry reads as the store now stands. **Verify**, offered where a version is
//! installed, hashes every file of each against its receipt, only when chosen,
//! and says on the same screen which files differ. Installing another version,
//! updating, rolling back, removing and signing in stay on `/login`'s agent
//! screen.
//!
//! Every string the catalog, the store or the configuration gave is hostile:
//! each is drawn with its control and format characters written as escapes
//! and cut where a line would be pushed out of sight, so none can draw a line
//! of its own or reach the terminal as a control sequence. The agent's key is
//! an [`AgentKey`](crucible_types::AgentKey), which holds nothing that needs
//! it.

use crucible_app::Conversation;
use crucible_app::catalog_fetch::FetchError;
use crucible_app::client::agents::{self as door, AgentsError, How};
use crucible_client_api::{Command, Name};
use crucible_tui::{
    Caret, Editor, Glyphs, Kept, Key, Offered, Panel, Picker, Pressed, Renderer, Row, Slot,
    Terminal, Typed, fold,
};

use crate::cli::Fatal;
use crate::cli::converse::picking::{self, Picked};
use crate::cli::converse::region::{self, Ended, Moved, step};

use super::login::{Overseeing, named, names, refused, sentence, shown, within};
use super::{Terms, say};

#[cfg(test)]
mod tests;

/// The word every screen here opens with.
const TITLE: &str = "Agents";

/// What the search line says while nothing is typed into it.
const HINT: &str = "an agent, or a licence";

/// What the preview pane says where the query left nothing to preview.
const NOVIEW: &str = "nothing to show";

/// What Enter and Escape do, under the marked agent.
const TAKES: &str = "Enter to open · Esc to cancel";

/// What the agent's screen says its Escape does.
const BACK: &str = "esc to go back";

/// What the panel over the catalog being read says is under way.
const READING: &str = "reading the agent catalog";

/// What a catalog offering nothing says, in place of a list.
const NONE_OFFERED: &str = "no agent is offered by the catalog";

/// The closing line of a list the door cut.
const MORE: &str = "more agents are offered than are shown here";

/// What an agent the store holds no version of says, on its entry.
const UNINSTALLED: &str = "not installed";

/// What an agent whose directory in the store could not be read says, on its
/// entry; the reason is on its own screen.
const UNREAD: &str = "the store could not be read";

/// What an agent's screen says of an agent whose entry names no place it keeps
/// its sign-in in.
const UNKEPT_SIGN_IN: &str = "A sign-in made under the sandbox is not kept: its entry names no place the agent keeps one in.";

/// What a door answer that is not the one asked for says.
const ASTRAY: &str = "! the agents door answered with something other than what was asked";

/// What the agent's screen says where it has no room to stand.
const CRAMPED: &str =
    "the window has no room for the agent's screen; make it taller and try /agents again";

/// What an add whose question had no room to stand says.
const CRAMPED_ADD: &str =
    "! adding needs an answer first; make the window taller and try /agents again";

/// The most of a reason the door, the store or the fetch gave that a line
/// repeats, in characters: as much as a refusal at the agents door is.
const REASON: usize = 1024;

/// The most versions, or files that differ, a line names one by one before
/// saying how many more there are.
const NAMED_AT_MOST: usize = 8;

/// What runs `/agents`: the catalog stood as a list where a key can be read,
/// else printed, and each agent taken opened on its own screen.
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
    // What was typed into the search line, kept across an add, which asks for
    // the catalog again and stands a new list over the answer.
    let mut query = Editor::new();
    loop {
        let Some(read) = carried(
            renderer,
            conversation,
            terms,
            Command::AgentCatalog,
            READING.to_owned(),
        )?
        else {
            return say(renderer, CRAMPED);
        };
        let mut listing = match read {
            Ok(door::Outcome::Catalog {
                agents,
                more,
                unrefreshed,
            }) => Listing::new(agents, more, unrefreshed.as_ref()),
            Ok(door::Outcome::Refused(refusal)) => return say(renderer, &format!("! {refusal}")),
            Ok(_) => return say(renderer, ASTRAY),
            Err(refusal) => return say(renderer, &refused(&refusal)),
        };
        if listing.agents.is_empty() || !keys {
            return printed(renderer, &listing);
        }
        listing.query = std::mem::take(&mut query);
        loop {
            let glyphs = terms.style().glyphs();
            let ended = region::stand(
                renderer,
                |_| terms.style(),
                &mut listing,
                |listing, columns, room| listing.laid(columns, room, glyphs),
                |pressed, listing| listing.pressed(pressed),
            )?;
            match ended {
                Ended::Took => {}
                Ended::Left => return Ok(()),
                Ended::Cramped => return printed(renderer, &listing),
            }
            let Some(entry) = listing.marked() else {
                continue;
            };
            match screen(&entry.listed, renderer, conversation, terms)? {
                Screened::Back => {}
                Screened::Added => break,
                Screened::Over => return Ok(()),
            }
        }
        query = std::mem::take(&mut listing.query);
    }
}

/// `command` sent to the agents door, the one question it stops on answered
/// and the panel saying `doing` stood between, as `/login`'s agent screen
/// does: nothing where a question had no room to stand.
fn carried<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &Conversation,
    terms: &Terms,
    command: Command,
    doing: String,
) -> Result<Option<Result<door::Outcome, AgentsError>>, Fatal> {
    let mut overseer = Overseeing {
        renderer: &mut *renderer,
        style: terms.style(),
        doing,
        cramped: false,
    };
    let carried = terms.carry(conversation, command, &mut overseer)?;
    Ok((!overseer.cramped).then_some(carried))
}

/// The catalog as lines, for a window with no keys to read or no room to
/// stand the list: each agent on one, then the closing lines.
fn printed<T: Terminal>(renderer: &mut Renderer<T>, listing: &Listing) -> Result<(), Fatal> {
    if listing.agents.is_empty() {
        say(renderer, NONE_OFFERED)?;
    }
    for entry in &listing.agents {
        say(
            renderer,
            &format!("{} · {} · {}", entry.title, entry.licence, entry.installed),
        )?;
    }
    for line in &listing.notice {
        say(renderer, line)?;
    }
    Ok(())
}

/// The list `/agents` stands: every agent the catalog offered, as its entry
/// draws it, what the search line holds, and where the mark is.
///
/// Narrowed on every frame rather than once a key is pressed, because the
/// query is the editor's and the frame is where it is read; the frame that
/// narrows writes down what the keys will walk next, the mark pulled back onto
/// the list where the query left it standing past the end.
struct Listing {
    /// Each agent the catalog offered, in the order the door gave them.
    agents: Vec<Entry>,
    /// The closing lines: that more are offered than are shown, and that the
    /// catalog could not be refreshed and why.
    notice: Vec<String>,
    /// What has been typed into the search line.
    query: Editor,
    /// Which of `found` the mark is on.
    marked: usize,
    /// Where the pointer rests, for the picker to light what is under it.
    pointer: Option<(usize, usize)>,
    /// Which of `agents` the query keeps, in order, as the last frame found.
    found: Vec<usize>,
}

/// One agent on the list, its words drawn once.
struct Entry {
    /// What the door said of it.
    listed: door::Listed,
    /// Its key and its newest version.
    title: String,
    /// Its licence.
    licence: String,
    /// Which versions the store holds of it, or that it could not be read.
    installed: String,
}

impl Listing {
    /// The list over what the door listed: `agents`, whether there were
    /// `more`, and why the catalog was `unrefreshed`, where it was.
    fn new(agents: Vec<door::Listed>, more: bool, unrefreshed: Option<&FetchError>) -> Self {
        let mut notice = Vec::new();
        if more {
            notice.push(MORE.to_owned());
        }
        if let Some(why) = unrefreshed {
            notice.push(unrefreshed_line(why));
        }
        Self {
            agents: agents.into_iter().map(Entry::new).collect(),
            notice,
            query: Editor::new(),
            marked: 0,
            pointer: None,
            found: Vec::new(),
        }
    }

    /// The entry the mark is on, where the query left any.
    fn marked(&self) -> Option<&Entry> {
        self.found
            .get(self.marked)
            .and_then(|&at| self.agents.get(at))
    }

    /// The list at `columns` in `room` rows, and where the cursor stands in
    /// its search line: nothing where the picker has no room to stand, and no
    /// cursor then, since the picker places one only in a list it drew.
    fn laid(&mut self, columns: usize, room: usize, glyphs: Glyphs) -> (Vec<Row>, Option<Caret>) {
        let query = self.query.text();
        self.found = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, entry)| sought(&entry.listed, query))
            .map(|(at, _)| at)
            .collect();
        self.marked = self.marked.min(self.found.len().saturating_sub(1));

        let kept: Vec<Kept<'_>> = self
            .found
            .iter()
            .filter_map(|&at| self.agents.get(at))
            .map(|entry| Kept {
                title: &entry.title,
                when: &entry.licence,
                branch: &entry.installed,
                place: "",
            })
            .collect();
        let marked = self.marked();
        let preview: Vec<Row> = match (marked, Picker::previewing(columns)) {
            (Some(entry), Some(pane)) => previewed(&entry.listed, pane),
            _ => Vec::new(),
        };
        let meta = marked
            .map(|entry| offered_count(&entry.listed))
            .unwrap_or_default();
        let heading = format!(
            "{TITLE} {} {} of {} offered",
            glyphs.dot(),
            self.found.len(),
            self.agents.len()
        );
        let nothing = format!("no agent matches \"{}\"", shown(query));
        let forms = keys(glyphs, !self.found.is_empty());
        let forms: Vec<&str> = forms.iter().map(String::as_str).collect();
        let notice: Vec<&str> = self
            .notice
            .iter()
            .flat_map(|line| fold(line, columns.saturating_sub(2).max(1)))
            .collect();
        let picker = Picker {
            heading: &heading,
            query,
            typed: self.query.column(),
            hint: HINT,
            sessions: &kept,
            marked: self.marked,
            renaming: None,
            refused: None,
            preview: &preview,
            preview_meta: &meta,
            takes: TAKES,
            nothing: &nothing,
            noview: NOVIEW,
            keys: &forms,
            notice: &notice,
            pointer: self.pointer,
        };
        let rows = picker.within(columns, room, glyphs);
        let caret = (!rows.is_empty()).then(|| picker.caret(columns, room, glyphs));
        (rows, caret)
    }

    /// What one key does to the list: typing and pasting land in the search
    /// line, the arrows walk the marks, Enter takes the marked agent and is
    /// refused on an empty list, and Escape clears a typed query before it
    /// leaves.
    fn pressed(&mut self, pressed: Pressed) -> Moved {
        match pressed {
            Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
            Pressed::Escape if self.query.is_empty() => Moved::Left,
            Pressed::Escape => {
                self.query.clear();
                self.marked = 0;
                Moved::Redraw
            }
            Pressed::Key(Key::Enter) if self.found.is_empty() => Moved::Still,
            Pressed::Key(Key::Enter) => Moved::Took,
            Pressed::Resized => Moved::Redraw,
            Pressed::Up => {
                let next = self.marked.checked_sub(1);
                step(&mut self.marked, next)
            }
            Pressed::Down => {
                let next = Some(self.marked + 1).filter(|next| *next < self.found.len());
                step(&mut self.marked, next)
            }
            // Where the pointer is, so the picker lights what it rests on.
            Pressed::Hovered { row, column } => {
                let next = (row != usize::MAX).then_some((row, column));
                if self.pointer == next {
                    return Moved::Still;
                }
                self.pointer = next;
                Moved::Redraw
            }
            Pressed::Pasted(pasted) => {
                let answered = self.query.paste(&pasted);
                self.queried(answered, true)
            }
            Pressed::Key(key) => {
                let rewrites = picking::rewrites(key);
                let answered = self.query.press(key);
                self.queried(answered, rewrites)
            }
            _ => Moved::Still,
        }
    }

    /// What the search line's answer does to the mark: a key that changed the
    /// query narrowed to another list, so the mark walks back to its top.
    fn queried(&mut self, answered: Typed, rewrites: bool) -> Moved {
        if answered == Typed::Ignored {
            return Moved::Still;
        }
        if rewrites {
            self.marked = 0;
        }
        Moved::Redraw
    }
}

impl Entry {
    /// The entry `listed` draws.
    fn new(listed: door::Listed) -> Self {
        Self {
            title: titled(&listed),
            licence: shown(&listed.license),
            installed: held(&listed),
            listed,
        }
    }
}

/// Whether `listed` is one `query` keeps: its key or its licence holds it,
/// ASCII case aside. An empty query keeps every agent.
fn sought(listed: &door::Listed, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    [listed.agent.as_str(), &listed.license]
        .into_iter()
        .any(|said| said.to_ascii_lowercase().contains(&query))
}

/// An agent's key and the newest version it is offered at.
fn titled(listed: &door::Listed) -> String {
    match listed.offered.first() {
        Some(newest) => format!("{} {}", listed.agent, shown(newest)),
        None => listed.agent.to_string(),
    }
}

/// Which versions the store holds of an agent, on its entry.
fn held(listed: &door::Listed) -> String {
    match &listed.installed {
        Ok(versions) if versions.is_empty() => UNINSTALLED.to_owned(),
        Ok(versions) => format!("installed {}", counted(versions)),
        Err(_) => UNREAD.to_owned(),
    }
}

/// `versions`, each escaped and cut, one after another: the first
/// [`NAMED_AT_MOST`] by name and how many more after them.
fn counted(versions: &[Box<str>]) -> String {
    let mut said: Vec<String> = versions
        .iter()
        .take(NAMED_AT_MOST)
        .map(|version| shown(version))
        .collect();
    let more = versions.len().saturating_sub(NAMED_AT_MOST);
    if more > 0 {
        said.push(format!("and {more} more"));
    }
    said.join(", ")
}

/// How many versions an agent is offered at, under the preview.
fn offered_count(listed: &door::Listed) -> String {
    match listed.offered.len() {
        1 => "1 version offered".to_owned(),
        offered => format!("{offered} versions offered"),
    }
}

/// The line saying the catalog could not be refreshed, and so is what was
/// believed before.
fn unrefreshed_line(why: &FetchError) -> String {
    format!(
        "the catalog could not be refreshed, so this is what was believed before: {}",
        within(&why.to_string(), REASON)
    )
}

/// The marked agent's licence, terms and caution and what the store holds of
/// it, as its screen tells them, folded at `pane` columns: the list pane
/// beside a preview is narrower than an entry's second line can be, so the
/// preview says it whole.
fn previewed(listed: &door::Listed, pane: usize) -> Vec<Row> {
    described(listed)
        .iter()
        .flat_map(|line| fold(line, pane.max(1)))
        .map(|part| Row::new().then(Slot::Plain, part))
        .collect()
}

/// Every form of the keys row, longest first, for a list with something on it
/// or without.
fn keys(glyphs: Glyphs, listed: bool) -> Vec<String> {
    let (up, down) = glyphs.walking();
    let dot = format!(" {} ", glyphs.dot());
    if listed {
        vec![
            [
                &format!("{up}{down} to walk"),
                "enter to open",
                "type to search",
                "esc to cancel",
            ]
            .join(&dot),
            [&format!("{up}{down}"), "enter", "esc"].join(&dot),
            "esc".to_owned(),
        ]
    } else {
        vec![
            ["type to narrow", "esc to cancel"].join(&dot),
            "esc".to_owned(),
        ]
    }
}

/// How an agent's screen closed.
enum Screened {
    /// Back to the list, nothing added.
    Back,
    /// An instance was added, so the list is asked for again.
    Added,
    /// `/agents` is over.
    Over,
}

/// What an agent's screen offers.
#[derive(Clone, Copy)]
enum Choice {
    /// Install the newest version offered and add an instance of it.
    Add,
    /// Check the versions installed against their receipts.
    Verify,
    /// Back to the list.
    Back,
}

/// Stands `listed`'s screen, its provenance over what can be done with it,
/// until it is left or an instance is added.
fn screen<T: Terminal>(
    listed: &door::Listed,
    renderer: &mut Renderer<T>,
    conversation: &Conversation,
    terms: &Terms,
) -> Result<Screened, Fatal> {
    let installed = listed
        .installed
        .as_ref()
        .is_ok_and(|versions| !versions.is_empty());
    let newest = listed.offered.first().map(|version| shown(version));
    let add = newest.as_ref().map(|version| {
        format!(
            "Install {} {version} and add an instance of it to your configuration.",
            listed.agent
        )
    });
    let mut offers: Vec<(Choice, Offered<'_>)> = Vec::new();
    if let Some(add) = &add {
        offers.push((
            Choice::Add,
            Offered {
                name: "Add",
                says: add,
            },
        ));
    }
    if installed {
        offers.push((
            Choice::Verify,
            Offered {
                name: "Verify",
                says: "Check its installed files against their receipts.",
            },
        ));
    }
    offers.push((
        Choice::Back,
        Offered {
            name: "Go back",
            says: "Choose another agent.",
        },
    ));
    let shown_offers: Vec<Offered<'_>> = offers.iter().map(|(_, offered)| *offered).collect();
    let title = format!("{TITLE} {} {}", terms.style().glyphs().dot(), listed.agent);
    let mut verified: Option<Vec<String>> = None;
    loop {
        let told = provenance(listed, verified.as_deref());
        let told: Vec<&str> = told.iter().map(String::as_str).collect();
        let panel = Panel {
            title: &title,
            said: None,
            source: None,
            told: &told,
            shown: &shown_offers,
            chosen: 0,
            footer: BACK,
        };
        let choice = match picking::pick(renderer, terms.style(), panel)? {
            Picked::Took(at) => offers.get(at).map_or(Choice::Back, |(choice, _)| *choice),
            Picked::Left => Choice::Back,
            Picked::Cramped => {
                say(renderer, CRAMPED)?;
                return Ok(Screened::Over);
            }
        };
        match choice {
            Choice::Back => return Ok(Screened::Back),
            Choice::Add => match add_one(listed, renderer, conversation, terms)? {
                Some(true) => return Ok(Screened::Added),
                Some(false) => {}
                None => return Ok(Screened::Over),
            },
            Choice::Verify => {
                let doing = format!("checking {} against its receipts", listed.agent);
                let command = match Name::new(listed.agent.as_str()) {
                    Ok(agent) => Command::VerifyAgent { agent },
                    Err(refusal) => {
                        say(renderer, &format!("! {refusal}"))?;
                        continue;
                    }
                };
                let Some(read) = carried(renderer, conversation, terms, command, doing)? else {
                    return Ok(Screened::Over);
                };
                match read {
                    Ok(outcome) => match checked(&outcome) {
                        Some(lines) => verified = Some(lines),
                        None => say(renderer, ASTRAY)?,
                    },
                    Err(refusal) => say(renderer, &refused(&refusal))?,
                }
            }
        }
    }
}

/// What an agent's screen tells of it, each line escaped and cut: the
/// versions offered, the licence, the terms and caution where there are any,
/// what the store holds of it, whether a sign-in made under the sandbox is
/// kept, and what a check against its receipts found, where one was made.
fn provenance(listed: &door::Listed, verified: Option<&[String]>) -> Vec<String> {
    let mut told = vec![if listed.offered.is_empty() {
        "Offered: no version for this platform.".to_owned()
    } else {
        format!("Offered: {}.", counted(&listed.offered))
    }];
    told.extend(described(listed));
    if listed.keeps_no_sign_in {
        told.push(UNKEPT_SIGN_IN.to_owned());
    }
    told.extend(verified.unwrap_or_default().iter().cloned());
    told
}

/// The licence, the terms and caution where there are any, and what the
/// store holds of an agent, each line escaped and cut.
fn described(listed: &door::Listed) -> Vec<String> {
    let mut told = vec![format!("Licence: {}.", shown(&listed.license))];
    if let Some(terms) = &listed.terms {
        told.push(format!("Terms: {}", shown(terms)));
    }
    if let Some(caution) = &listed.caution {
        told.push(format!("Caution: {}", shown(caution)));
    }
    told.push(match &listed.installed {
        Ok(versions) if versions.is_empty() => "Not installed.".to_owned(),
        Ok(versions) => format!("Installed, each with a receipt: {}.", counted(versions)),
        Err(why) => format!(
            "The install store could not be read: {}",
            within(why, REASON)
        ),
    });
    told
}

/// What a check against the receipts found, a line a version: nothing where
/// `outcome` is not a check's.
fn checked(outcome: &door::Outcome) -> Option<Vec<String>> {
    let door::Outcome::Verified { versions, .. } = outcome else {
        return None;
    };
    Some(
        versions
            .iter()
            .map(|(version, mismatches)| {
                let version = shown(version);
                if mismatches.is_empty() {
                    return format!("{version}: every file is as recorded at install.");
                }
                let paths: Vec<Box<str>> = mismatches
                    .iter()
                    .map(|mismatch| mismatch.path.clone())
                    .collect();
                let differ = match paths.len() {
                    1 => "1 file differs".to_owned(),
                    many => format!("{many} files differ"),
                };
                format!(
                    "{version}: {differ} from what was recorded at install: {}.",
                    quoted_counted(&paths)
                )
            })
            .collect(),
    )
}

/// `paths`, each quoted, escaped and cut: the first [`NAMED_AT_MOST`] by
/// name and how many more after them.
fn quoted_counted(paths: &[Box<str>]) -> String {
    let said = names(paths.get(..NAMED_AT_MOST).unwrap_or(paths));
    match paths.len().saturating_sub(NAMED_AT_MOST) {
        0 => said,
        more => format!("{said} and {more} more"),
    }
}

/// Adds an instance of `listed` at its newest version: whether one was added,
/// or nothing where `/agents` is over.
fn add_one<T: Terminal>(
    listed: &door::Listed,
    renderer: &mut Renderer<T>,
    conversation: &Conversation,
    terms: &Terms,
) -> Result<Option<bool>, Fatal> {
    let Some(newest) = listed.offered.first() else {
        return Ok(Some(false));
    };
    let command = match (Name::new(listed.agent.as_str()), Name::new(newest)) {
        (Ok(agent), Ok(version)) => Command::AddAgent { agent, version },
        (Err(refusal), _) | (_, Err(refusal)) => {
            say(renderer, &format!("! {refusal}"))?;
            return Ok(Some(false));
        }
    };
    let doing = format!("adding {} {}", listed.agent, shown(newest));
    let Some(carried) = carried(renderer, conversation, terms, command, doing)? else {
        say(renderer, CRAMPED_ADD)?;
        return Ok(None);
    };
    let outcome = match carried {
        Ok(outcome) => outcome,
        Err(refusal) => {
            say(renderer, &refused(&refusal))?;
            return Ok(Some(false));
        }
    };
    if let door::Outcome::Undecided = outcome {
        return Ok(Some(false));
    }
    if let door::Outcome::Refused(refusal) = &outcome {
        say(renderer, &format!("! {refusal}"))?;
        return Ok(Some(false));
    }
    let Some(line) = added(&outcome) else {
        say(renderer, ASTRAY)?;
        return Ok(Some(false));
    };
    say(renderer, &line)?;
    Ok(Some(true))
}

/// What an add that was carried out says: the instance written, which is the
/// last the door names; the others the door moved onto the version with it;
/// those left as they were; and why it was not smoke-tested or pruned, where it was
/// not. Nothing where `outcome` is not an add's.
fn added(outcome: &door::Outcome) -> Option<String> {
    let door::Outcome::Moved {
        agent,
        how: How::Added,
        version,
        instances,
        left,
        unsmoked,
        unpruned,
        ..
    } = outcome
    else {
        return None;
    };
    let (written, moved) = instances.split_last()?;
    let version = shown(version);
    let mut said = vec![format!(
        "Added {agent} {version} as the instance {}.",
        named(written)
    )];
    if !moved.is_empty() {
        said.push(format!("{} moved onto {version}.", names(moved)));
    }
    if !left.is_empty() {
        said.push(format!(
            "{} changed meanwhile and were left as they are.",
            names(left)
        ));
    }
    if let Some(why) = unsmoked {
        said.push(format!("It was not smoke-tested: {}.", within(why, REASON)));
    }
    if let Some(unpruned) = unpruned {
        said.push(format!("Older versions were kept: {}.", sentence(unpruned)));
    }
    Some(said.join(" "))
}
