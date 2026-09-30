//! `/login`: a subscription or console credential, selected without exposing
//! a secret on the command line.
//!
//! With no words, the first panel asks how usage is paid for: an account whose
//! plan includes it, or a key billed by what is sent. Each leads to a list of
//! the rows of that kind, read off the row registry, and a row that holds the
//! credential serving its provider says `signed in`. Only which row holds one
//! is read: the store is asked for names and kinds before anything is drawn,
//! and a store that is there and cannot be parsed, is not text or is past its
//! size limit is said instead of a panel.
//! Row names are drawn with the glyph set's dot. Below the first
//! screen Escape goes back one level, with the mark where it was; at the first
//! screen, and at one opened directly by words, it cancels.
//!
//! Words after the command narrow the rows: the list's own word, a site, or
//! the start of a word of a row's name or of its site, and the provider's typed
//! name. Several rows left stand under the headings of their kinds; one opens
//! its screen at once; none is said in one line naming the words.
//!
//! A row whose provider holds a credential says on its next screen what will be
//! replaced. Nothing is replaced until the new one is stored, so a sign-in that
//! fails, is refused or expires leaves the store as it was and says what is
//! unchanged. A sign-in stopped by Escape while its credential is being written
//! is waited for, and taken as signed in where the write went through, or said
//! as still being stored where the write outlives the wait.
//!
//! A real subscription implementation starts its login as a task on the
//! application's runtime and reports the page the user must visit; the terminal
//! thread continues serving resize and cancellation, but for the moment a stop
//! waits for a credential already being written, and leaving the view stops
//! the login, closing a browser login's callback port.
//!
//! A key typed after a command is a key in the shell's history file, in the
//! process listing while the command runs, and in that shell's own scrollback
//! afterwards — three places it was never meant to be, none of which this
//! program could clear. So the two halves are asked for separately: who the key
//! is for, out loud, and the key itself into a box.
//!
//! What that writes is then read straight back, and the session is handed the
//! provider it buys — unless another provider is already answering, in which
//! case the session keeps the provider and the model it has and the line points
//! at `/model`, where switching is chosen rather than implied. A key is given
//! by somebody who wants to type at the screen in front of them, and the file
//! it lands in is what makes the run after this one ask the same thing rather
//! than what makes this one ask at all.
//!
//! A run with no key for anything is left alone to draw its prompt. The warning
//! under the welcome names this command and `/model` both, which is the whole of
//! what somebody meeting crucible for the first time has to read; a panel
//! standing in front of that prompt would be this program answering a question
//! nobody asked it, on the one screen where the reader is still finding out
//! where they are.
//!
//! A window with no room to stand a panel in, and a run with no keyboard, are
//! given each row as the whole line to type: `/login` and the words that leave
//! that row alone. Every row comes off the registry, so a way in this build
//! offers and cannot be typed is not a state that exists.

use std::borrow::Cow;
use std::time::Duration;

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::content_use;
use crucible_app::providers::{List, Providers, Row as Way, Rows, Served, key_variables, offered};
use crucible_app::subscription::Route;
use crucible_app::switching::LoggedIn;
use crucible_auth::{AuthError, Kind, LoginAttempt, LoginUpdate, Stopped};
use crucible_client_api::Command;
use crucible_tui::{
    Caret, Glyphs, Heading, Key, Offered, Panel, Pressed, Renderer, Row, Slot, Terminal,
    characters, clip, fold, pressed,
};

use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::converse::picking::{self, Picked};
use crate::cli::converse::secret::{self, Asked};
use crate::cli::converse::warning::{self, Answer, Put};

use super::{Terms, about, say};

/// The few words at the top of every `/login` panel.
const TITLE: &str = "Log in";

/// The row that leads to the accounts, and what it says.
const ACCOUNT_ROUTE_SHOWN: &str = "Your account with subscription";
const ACCOUNT_ROUTE_SAYS: &str = "Usage included in your paid plan";

/// The row that leads to a key of the reader's own, and what it says.
pub(super) const KEY_ROUTE_SHOWN: &str = "Provide your own API key";
pub(super) const KEY_ROUTE_SAYS: &str = "API usage billing";

/// The first panel: two ways to pay, whatever the store holds.
const FIRST: [Offered<'static>; 2] = [
    Offered {
        name: ACCOUNT_ROUTE_SHOWN,
        says: ACCOUNT_ROUTE_SAYS,
    },
    Offered {
        name: KEY_ROUTE_SHOWN,
        says: KEY_ROUTE_SAYS,
    },
];

/// The sentence under the first panel.
const HOW: &str = "Choose how usage is paid for.";

/// The sentence under the list of accounts.
const ACCOUNTS: &str = "Choose the account whose plan pays for usage.";

/// The sentence under the list of keys.
const SAID: &str = "Choose the provider whose API key you have.";

/// The sentence over rows narrowed by words, which may be of either kind.
const NARROWED: &str = "Choose the account or key to sign in with.";

/// The headings rows narrowed by words stand under.
const ACCOUNT_HEADING: &str = "Subscription";
const KEY_HEADING: &str = "API key";

/// What opens the description of a row holding its provider's sign-in.
const SIGNED_IN: &str = "signed in";

/// What a row holding its provider's key says.
const SIGNED_IN_WITH_KEY: &str = "signed in with a stored key";

/// The one key worth naming on a screen a command opened: the arrows and Enter
/// are what a list with a mark on it is already saying.
const CANCEL: &str = "esc to cancel";

/// The same, on a screen chosen from a list.
const BACK: &str = "esc to go back";

/// What a sign-in stopped while its credential was still being written says:
/// the write outlived the wait, so what it left is for `/login` to show.
const UNSETTLED: &str =
    "! the sign-in was being stored when it was stopped; /login shows what is stored";

/// What the sign-in view says while a stop waits for a credential being
/// written.
const STOPPING: &str = "stopping; waiting for anything being stored…";

/// What escape leaves behind, in place of the rows it used to write.
const LEFT: &str = "cancelled, nothing signed in";

/// What the box's absence leaves behind, when the window had no room for it.
///
/// Not [`LEFT`]: nothing was asked, so nothing was cancelled, and the way in
/// is the one thing this can say that the reader does not already know.
const CRAMPED: &str = "the window has no room for the key box; make it taller and try /login again";

/// What stopped, when the key could not be written down.
pub(super) const STORE_FAILED: &str = "the key could not be saved";

/// The way back in, when the store's directory or file would not open.
///
/// Fixed words: the error behind it names the path and what the operating
/// system said, and neither belongs on a row under a command.
pub(super) const STORE_UNWRITABLE: &str =
    "crucible cannot write its login store; try /login again after fixing the permissions";

/// The way back in, when another crucible held the store's lock too long.
const STORE_BUSY: &str =
    "another crucible is writing its login store; try /login again in a moment";

/// The way back in, when the store is there and cannot be read.
///
/// Writing over it would replace the only copy of whatever else it holds, so
/// the store declines; moving it aside is what makes the next write safe.
const STORE_UNREADABLE: &str =
    "crucible cannot read its login store; move it aside and try /login again";

/// The way back in, when the yes that goes with the credential being replaced
/// could not be taken out of the configuration file first.
const STORE_UNRELEASED: &str =
    "crucible cannot change its configuration file; fix it and try /login again";

/// Manual callback input is transient credential material. It has the same
/// bound as the key box and is never committed or echoed.
const MAX_MANUAL: usize = 16 * 1024;

/// Where a screen was opened from, which decides what Escape does on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opened {
    /// By the command, or by words that left one row: Escape cancels.
    Directly,
    /// From a list or a panel before it: Escape goes back there.
    Below,
}

impl Opened {
    /// What the screen's last row says Escape does.
    const fn leaves(self) -> &'static str {
        match self {
            Self::Directly => CANCEL,
            Self::Below => BACK,
        }
    }
}

/// How a screen of the walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Closed {
    /// It said its last line: the command is over.
    Done,
    /// Escape, on a screen below another: that one stands again.
    Back,
    /// No room for it: the caller prints the lines to type.
    Cramped,
}

/// What every screen of the walk is drawn on and acts for, and which row
/// holds each provider's credential as the walk began.
struct Walk<'w, T: Terminal> {
    renderer: &'w mut Renderer<T>,
    conversation: &'w mut Conversation,
    terms: &'w Terms,
    held: &'w [Way],
}

/// Runs it: the walk from the first panel, the rows words narrow it to, or
/// where no panel can stand, the lines to type.
///
/// `keys` is whether there is a keyboard to take one from. Down a pipe there is
/// not, and a panel or a box waiting for something nobody can type is a session
/// that stopped, so what a piped run gets is the lines instead.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let rows = Rows::production();
    let held = match holding(&rows, terms) {
        Ok(held) => held,
        Err(failed) => return say(renderer, &format!("! {}", remedy(&failed))),
    };

    let words: Vec<&str> = said.split_whitespace().collect();
    let matched = matching(&words, &rows);
    if matched.is_empty() {
        // The words came off the line and were never shape-checked (anything
        // at all can follow `/login `), so they go out the way arrived text
        // goes out.
        let words = words.join(" ");
        renderer.commit(&format!(
            "! no sign-in matches \"{words}\"; /login lists them"
        ))?;
        return Ok(());
    }

    if keys {
        let mut walk = Walk {
            renderer: &mut *renderer,
            conversation,
            terms,
            held: &held,
        };
        let stood = if words.is_empty() {
            walked(&mut walk, &rows)?
        } else if let [only] = matched.as_slice() {
            screen(only, Opened::Directly, &mut walk)?
        } else {
            narrowed(&matched, &mut walk)?
        };
        if stood != Closed::Cramped {
            return Ok(());
        }
    }

    typed(&matched, &rows, renderer, terms)
}

/// Which row holds the credential serving each provider: names and kinds, read
/// before anything is drawn, and no key or token.
///
/// A store that is there and cannot be parsed, holds something other than
/// text or is past its size limit is one a write would refuse to replace, so
/// it comes back to be said before anything is asked. One the system will not
/// open, or whose home cannot be reached, marks no row, and the write that
/// meets it says why, as it always has.
fn holding(rows: &Rows, terms: &Terms) -> Result<Vec<Way>, AuthError> {
    let held = match terms.logins.holding() {
        Ok(held) => held,
        Err(failed @ (AuthError::Unreadable { .. } | AuthError::TooLarge { .. })) => {
            return Err(failed);
        }
        Err(
            AuthError::Unwritable { .. } | AuthError::Busy { .. } | AuthError::Unreleased { .. },
        ) => Vec::new(),
    };
    Ok(held
        .iter()
        .filter_map(|held| rows.of(held.kind, &held.name))
        .cloned()
        .collect())
}

/// The rows `words` leave, in the order the lists show them: all of them where
/// there are none.
///
/// Without regard to case, a row stays when every word matches it: the words
/// `subscription` and `key` match the rows of that list, a word that is
/// exactly a site the rows of that site, and any other word a row whose
/// provider it names, or the start of a word of whose name or of whose site it
/// is.
fn matching<'r>(words: &[&str], rows: &'r Rows) -> Vec<&'r Way> {
    let sites: Vec<&str> = rows.all().iter().filter_map(|way| way.site).collect();
    rows.all()
        .iter()
        .filter(|way| words.iter().all(|word| matches(word, way, &sites)))
        .collect()
}

/// Whether one word leaves `way`.
fn matches(word: &str, way: &Way, sites: &[&str]) -> bool {
    let word = word.to_lowercase();
    match word.as_str() {
        "subscription" => return way.list == List::Subscription,
        "key" => return way.list == List::Key,
        _ => {}
    }
    if sites.iter().any(|site| site.to_lowercase() == word) {
        return way.site.is_some_and(|site| site.to_lowercase() == word);
    }
    way.provider.to_lowercase() == word
        || way
            .shown
            .split_whitespace()
            .any(|part| part.to_lowercase().starts_with(&word))
        || way
            .site
            .is_some_and(|site| site.to_lowercase().starts_with(&word))
}

/// The words after `/login` that leave `way` alone: its provider's name, then
/// its list's word, then its site, as few of them as do it.
fn reaching(way: &Way, rows: &Rows) -> String {
    let list = match way.list {
        List::Subscription => "subscription",
        List::Key => "key",
    };
    let alone = |words: &[&str]| matching(words, rows) == [way];
    let mut words = vec![way.provider];
    for more in [Some(list), way.site].into_iter().flatten() {
        if alone(&words) {
            break;
        }
        words.push(more);
    }
    words.join(" ")
}

/// The first panel, the list of the kind chosen on it, and the screen of the
/// row chosen from that, until one of them is done.
fn walked<T: Terminal>(walk: &mut Walk<'_, T>, rows: &Rows) -> Result<Closed, Fatal> {
    let mut first = 0;
    loop {
        let panel = Panel {
            source: None,
            title: TITLE,
            said: Some(HOW),
            shown: &FIRST,
            chosen: first,
            footer: CANCEL,
        };
        let (list, said) = match picking::pick(walk.renderer, walk.terms.style(), panel)? {
            Picked::Took(0) => (List::Subscription, ACCOUNTS),
            Picked::Took(_) => (List::Key, SAID),
            Picked::Left => {
                say(walk.renderer, LEFT)?;
                return Ok(Closed::Done);
            }
            Picked::Cramped => return Ok(Closed::Cramped),
        };
        first = usize::from(list == List::Key);

        let listed = listed(rows, list);
        let listing = Listing {
            said,
            ways: &listed,
            headings: &[],
            footer: BACK,
        };
        match listing.stand(walk)? {
            Closed::Back => {}
            closed => return Ok(closed),
        }
    }
}

/// The rows of one list, in the order the registry gives them.
fn listed(rows: &Rows, list: List) -> Vec<&Way> {
    rows.listed(list).collect()
}

/// The rows words left, under the headings of their kinds, opened by the
/// command: Escape cancels.
fn narrowed<T: Terminal>(matched: &[&Way], walk: &mut Walk<'_, T>) -> Result<Closed, Fatal> {
    let (ordered, headings) = kinds(matched);

    let listing = Listing {
        said: NARROWED,
        ways: &ordered,
        headings: &headings,
        footer: CANCEL,
    };
    match listing.stand(walk)? {
        Closed::Back => {
            say(walk.renderer, LEFT)?;
            Ok(Closed::Done)
        }
        closed => Ok(closed),
    }
}

/// `matched` with each kind's rows together, and the headings that stand over
/// them.
fn kinds<'r>(matched: &[&'r Way]) -> (Vec<&'r Way>, Vec<Heading<'static>>) {
    let mut ordered = matched.to_vec();
    // Stable: within a kind, the registry's order stands.
    ordered.sort_by_key(|way| way.list == List::Key);
    let accounts = ordered
        .iter()
        .filter(|way| way.list == List::Subscription)
        .count();
    let headings = [
        (0, ACCOUNT_HEADING, accounts > 0),
        (accounts, KEY_HEADING, accounts < ordered.len()),
    ]
    .into_iter()
    .filter(|(_, _, any)| *any)
    .map(|(before, name, _)| Heading { before, name })
    .collect();
    (ordered, headings)
}

/// A list of rows to choose one from.
struct Listing<'a> {
    said: &'a str,
    ways: &'a [&'a Way],
    headings: &'a [Heading<'a>],
    footer: &'a str,
}

impl Listing<'_> {
    /// Stands the list, and the screen of each row chosen from it, until one
    /// is done or the list is left.
    fn stand<T: Terminal>(&self, walk: &mut Walk<'_, T>) -> Result<Closed, Fatal> {
        let providers = walk.terms.providers.snapshot();
        let glyphs = walk.terms.style().glyphs();
        let entries = entries(self.ways, walk.held, &providers, glyphs);
        let shown: Vec<Offered<'_>> = entries
            .iter()
            .map(|(name, says)| Offered { name, says })
            .collect();

        let mut at = 0;
        loop {
            let panel = Panel {
                source: None,
                title: TITLE,
                said: Some(self.said),
                shown: &shown,
                chosen: at,
                footer: self.footer,
            };
            let style = walk.terms.style();
            let picked =
                picking::pick_under(walk.renderer, style, panel, self.headings, &mut |_| Ok(()))?;
            match picked {
                Picked::Took(chosen) => {
                    at = chosen;
                    let Some(way) = self.ways.get(chosen) else {
                        return Ok(Closed::Cramped);
                    };
                    match screen(way, Opened::Below, walk)? {
                        Closed::Back => {}
                        closed => return Ok(closed),
                    }
                }
                Picked::Left => return Ok(Closed::Back),
                Picked::Cramped => return Ok(Closed::Cramped),
            }
        }
    }
}

/// Each row's name and what it says beneath it, as a list draws them.
fn entries(
    ways: &[&Way],
    held: &[Way],
    providers: &Providers,
    glyphs: Glyphs,
) -> Vec<(String, String)> {
    ways.iter()
        .map(|way| {
            (
                drawn(way.shown, glyphs).into_owned(),
                described(way, held, providers, glyphs),
            )
        })
        .collect()
}

/// A row's name as the glyph set draws it.
///
/// The registry writes a name the way the design does, `Kimi Code · kimi.ai`;
/// a terminal set to ASCII is sent the set's own dot in its place.
fn drawn(shown: &str, glyphs: Glyphs) -> Cow<'_, str> {
    const DOT: &str = "·";
    if glyphs.dot() == DOT || !shown.contains(DOT) {
        return Cow::Borrowed(shown);
    }
    Cow::Owned(shown.replace(DOT, glyphs.dot()))
}

/// What a row says beneath its name.
///
/// `signed in` first, where the row holds the credential serving its
/// provider, then the caution of a route whose vendor uses what is sent, so a
/// narrow window cuts the row's own words rather than either. A key
/// row holding nothing names the variable its key can be set in instead, where
/// the provider's key is read from one for it.
fn described(way: &Way, held: &[Way], providers: &Providers, glyphs: Glyphs) -> String {
    let holds = held.contains(way);
    let says = way.says.unwrap_or_default();
    // The vendor's words about what it does with what is sent come after
    // `signed in` and before the row's own, so a narrow window cuts those.
    let caution = content_use::Routes::production()
        .warned(&content_use::row_route(way))
        .map(|warned| warned.warning.caution);
    // A key row beside a caution says `signed in` alone, so the caution
    // stands whole at forty columns: the list it is in already says a key.
    let signed = holds.then_some(match (way.kind, caution) {
        (Kind::Key, None) => SIGNED_IN_WITH_KEY,
        (Kind::Key, Some(_)) | (Kind::Account, _) => SIGNED_IN,
    });
    let own = match (way.kind, holds) {
        (Kind::Key, true) => None,
        (Kind::Key, false) => Some(
            read_from(way, providers).map_or_else(|| says.to_owned(), |one| variable_row(&one)),
        ),
        (Kind::Account, _) => Some(says.to_owned()),
    };
    [signed.map(str::to_owned), caution.map(str::to_owned), own]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(&format!(" {} ", glyphs.dot()))
}

/// The provider whose variable a key row's key can be set in instead, where
/// it is that provider's row for one.
fn read_from(way: &Way, providers: &Providers) -> Option<Served> {
    way.environment
        .then(|| offered(providers).find(|one| one.name == way.provider))
        .flatten()
}

/// What a provider row says: the variable the same key can be set in instead,
/// which is the one thing that differs between rows that would otherwise read
/// identically.
fn variable_row(one: &Served) -> String {
    format!("set {}", one.key)
}

/// What choosing `way` replaces: the credential its provider holds, on any row
/// of it, that one included.
fn replaced(way: &Way, held: &[Way], glyphs: Glyphs) -> Option<String> {
    held.iter()
        .find(|one| one.provider == way.provider)
        .map(|one| {
            format!(
                "the {} held for {}",
                credential(one),
                drawn(one.shown, glyphs)
            )
        })
}

/// What a sign-in that did not complete says: nothing was replaced.
fn unchanged(way: &Way, held: &[Way], glyphs: Glyphs) -> String {
    held.iter()
        .find(|one| one.provider == way.provider)
        .map_or_else(
            || "! sign-in did not complete; nothing was stored".to_owned(),
            |one| {
                format!(
                    "! sign-in did not complete; the {} stored for {} is unchanged",
                    credential(one),
                    drawn(one.shown, glyphs)
                )
            },
        )
}

/// What a row's credential is called in a sentence.
const fn credential(way: &Way) -> &'static str {
    match way.kind {
        Kind::Key => "API key",
        Kind::Account => "sign-in",
    }
}

/// The screen of one row: its key box, or its sign-in.
fn screen<T: Terminal>(way: &Way, opened: Opened, walk: &mut Walk<'_, T>) -> Result<Closed, Fatal> {
    let route = content_use::row_route(way);
    if let Some(warned) = walk.terms.consent.asks(&route).copied() {
        let style = walk.terms.style();
        match warning::ask(walk.renderer, style, &warned, Put::Choice)? {
            Answer::Yes => walk.terms.consent.give(&route),
            Answer::Back if opened == Opened::Below => return Ok(Closed::Back),
            Answer::Back => {
                say(walk.renderer, LEFT)?;
                return Ok(Closed::Done);
            }
            Answer::Cramped => {
                walk.renderer.commit(warning::CRAMPED_CHOICE)?;
                return Ok(Closed::Done);
            }
        }
    }
    let closed = opening(way, opened, walk);
    // A yes given here and not written down with a stored credential goes:
    // a sign-in that failed or was left agreed to nothing.
    walk.terms.consent.withdraw(&route);
    closed
}

/// The screen of `way`, once nothing is left to ask before it.
fn opening<T: Terminal>(
    way: &Way,
    opened: Opened,
    walk: &mut Walk<'_, T>,
) -> Result<Closed, Fatal> {
    let served = walk.terms.providers.snapshot();
    let Some(named) = offered(&served).find(|one| one.name == way.provider) else {
        say(
            walk.renderer,
            &format!(
                "! {} is not served by this build",
                drawn(way.shown, walk.terms.style().glyphs())
            ),
        )?;
        return Ok(Closed::Done);
    };
    match way.kind {
        // The row a provider's key has always been read for, opened by the
        // command: the way `/login anthropic` comes in.
        Kind::Key if opened == Opened::Directly && way.environment => {
            given(named, walk.renderer, walk.conversation, walk.terms)?;
            Ok(Closed::Done)
        }
        Kind::Key => keyed(way, named, opened, walk),
        Kind::Account => signed(way, opened, walk),
    }
}

/// The key box for `way`, and the key written down under its name.
fn keyed<T: Terminal>(
    way: &Way,
    named: Served,
    opened: Opened,
    walk: &mut Walk<'_, T>,
) -> Result<Closed, Fatal> {
    let glyphs = walk.terms.style().glyphs();
    let replaces = replaced(way, walk.held, glyphs);
    let asked = secret::ask(
        walk.renderer,
        walk.terms.style(),
        &drawn(way.shown, glyphs),
        replaces.as_deref(),
        opened.leaves(),
    )?;
    match asked {
        Asked::Key(key) => kept(way.stored, named, &key, walk)?,
        Asked::Left if opened == Opened::Below => return Ok(Closed::Back),
        Asked::Left => say(walk.renderer, LEFT)?,
        Asked::Cramped => say(walk.renderer, CRAMPED)?,
    }
    Ok(Closed::Done)
}

/// The sign-in of `way`: its methods where it has more than one, then the
/// view the sign-in runs under.
fn signed<T: Terminal>(way: &Way, opened: Opened, walk: &mut Walk<'_, T>) -> Result<Closed, Fatal> {
    let routes = walk.terms.subscriptions.routes(way.stored);
    match routes.as_slice() {
        [] => {
            let glyphs = walk.terms.style().glyphs();
            let named = drawn(way.shown, glyphs);
            say(
                walk.renderer,
                &format!("! no subscription login for {named}"),
            )?;
            return Ok(Closed::Done);
        }
        [route] => return subscribed(*route, way, opened, walk),
        _ => {}
    }

    let shown: Vec<Offered<'_>> = routes
        .iter()
        .map(|route| Offered {
            name: route.shown,
            says: route.says,
        })
        .collect();
    let title = drawn(way.shown, walk.terms.style().glyphs());
    let mut at = 0;
    loop {
        let panel = Panel {
            source: None,
            title: &title,
            said: Some("Choose where to finish account authorization."),
            shown: &shown,
            chosen: at,
            footer: opened.leaves(),
        };
        match picking::pick(walk.renderer, walk.terms.style(), panel)? {
            Picked::Took(chosen) => {
                at = chosen;
                let Some(route) = routes.get(chosen) else {
                    return Ok(Closed::Cramped);
                };
                match subscribed(*route, way, Opened::Below, walk)? {
                    Closed::Back => {}
                    closed => return Ok(closed),
                }
            }
            Picked::Left if opened == Opened::Below => return Ok(Closed::Back),
            Picked::Left => {
                say(walk.renderer, LEFT)?;
                return Ok(Closed::Done);
            }
            Picked::Cramped => return Ok(Closed::Cramped),
        }
    }
}

/// Each row in `ways` as the whole line to type, for a window with no room for
/// a panel and a run with no keyboard.
fn typed<T: Terminal>(
    ways: &[&Way],
    rows: &Rows,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let providers = terms.providers.snapshot();
    let glyphs = terms.style().glyphs();
    let columns = renderer.columns();
    // Folded rather than cut: a line to type with its last word cut off is a
    // line that reaches some other row, or none.
    let lines: Vec<Row> = ways
        .iter()
        .flat_map(|way| {
            let line = line(way, rows, &providers, glyphs);
            fold(&line, columns)
                .into_iter()
                .map(|part| Row::new().then(Slot::Quiet, part))
                .collect::<Vec<_>>()
        })
        .collect();

    Ok(renderer.present(&lines)?)
}

/// One row as the line to type, and what typing it reaches: the words name
/// the row, so what follows says only what it takes.
fn line(way: &Way, rows: &Rows, providers: &Providers, glyphs: Glyphs) -> String {
    let what = match (way.kind, read_from(way, providers)) {
        (Kind::Key, Some(one)) => format!("a key, or set {}", one.key),
        (Kind::Key, None) => "a key".to_owned(),
        (Kind::Account, _) => "a sign-in".to_owned(),
    };
    about(&format!("/login {}", reaching(way, rows)), &what, glyphs)
}

/// Runs a registered subscription flow and switches this session on success.
///
/// Escape stops the flow. Opened below another screen it goes back there;
/// opened by the command it cancels it, as Escape on any screen the command
/// opened does; and where its credential was being written as it was
/// stopped, the write is waited for and a sign-in that went through is taken,
/// or said as still being stored where it outlives the wait.
/// A flow that fails, is refused or expires says what went wrong and that
/// nothing held was replaced.
fn subscribed<T: Terminal>(
    route: Route,
    way: &Way,
    opened: Opened,
    walk: &mut Walk<'_, T>,
) -> Result<Closed, Fatal> {
    let Walk {
        renderer,
        conversation,
        terms,
        held,
    } = walk;
    let (terms, held) = (*terms, *held);
    let glyphs = terms.style().glyphs();
    let provider = route.provider();
    if !terms.subscriptions.supports(provider) {
        say(renderer, &format!("! no subscription login for {provider}"))?;
        return Ok(Closed::Done);
    }
    let failed = |renderer: &mut Renderer<T>, problem: &dyn std::fmt::Display| {
        say(renderer, &format!("! {problem}"))?;
        say(renderer, &unchanged(way, held, glyphs))?;
        Ok(Closed::Done)
    };
    let attempt = match terms.subscriptions.start(route, terms.logins.clone()) {
        Ok(attempt) => attempt,
        Err(problem) => return failed(renderer, &problem),
    };

    // Read once: nothing a login does moves where a key is read from.
    let providers = terms.providers.snapshot();
    let withheld: Vec<&str> = key_variables(&providers, &terms.settings).collect();
    let mut view =
        LoginView::new(glyphs).opened(opened.leaves(), replaced(way, held, glyphs), glyphs);
    view.show(renderer, terms, route.title())?;
    loop {
        match attempt.wait(Duration::from_millis(50)) {
            Ok(Some(update)) => {
                if view.apply(update, &withheld) {
                    warning::stored(renderer, terms, &content_use::row_route(way))?;
                    let Some(named) =
                        offered(&terms.providers.snapshot()).find(|one| one.name == provider)
                    else {
                        say(renderer, "! the signed-in provider is unavailable")?;
                        return Ok(Closed::Done);
                    };
                    taken(named, renderer, conversation, terms)?;
                    return Ok(Closed::Done);
                }
                view.show(renderer, terms, route.title())?;
            }
            Ok(None) => {}
            Err(problem) => return failed(renderer, &problem),
        }

        let Some(arrived) = view.key(renderer)? else {
            continue;
        };
        match arrived {
            Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => {
                // A write already begun cannot be taken back: stopping waits
                // for it, which the view says first, and then says whether the
                // credential was written, and that answer is what is said.
                view.stopping();
                view.show(renderer, terms, route.title())?;
                match after_stop(attempt.cancel(), opened) {
                    AfterStop::Take => {
                        warning::stored(renderer, terms, &content_use::row_route(way))?;
                        let Some(named) =
                            offered(&terms.providers.snapshot()).find(|one| one.name == provider)
                        else {
                            say(renderer, "! the signed-in provider is unavailable")?;
                            return Ok(Closed::Done);
                        };
                        taken(named, renderer, conversation, terms)?;
                        return Ok(Closed::Done);
                    }
                    AfterStop::Unsettled => {
                        say(renderer, UNSETTLED)?;
                        return Ok(Closed::Done);
                    }
                    AfterStop::Back => return Ok(Closed::Back),
                    AfterStop::Left => {
                        say(renderer, LEFT)?;
                        return Ok(Closed::Done);
                    }
                }
            }
            Pressed::Resized => renderer.resized()?,
            pressed => {
                if !view.press(pressed, &attempt)? {
                    continue;
                }
            }
        }
        view.show(renderer, terms, route.title())?;
    }
}

/// What `/login` does after stopping a sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterStop {
    /// The credential was written as it was stopped: the session takes it.
    Take,
    /// The write outlived the wait: said, and left to `/login` to show.
    Unsettled,
    /// Nothing was written, below another screen: that one stands again.
    Back,
    /// Nothing was written, on a screen the command opened: cancelled.
    Left,
}

/// What a sign-in opened as `opened` does once stopping it answered
/// `stopped`: only what the store holds decides it.
const fn after_stop(stopped: Stopped, opened: Opened) -> AfterStop {
    match (stopped, opened) {
        (Stopped::Written, _) => AfterStop::Take,
        (Stopped::Unsettled, _) => AfterStop::Unsettled,
        (Stopped::Unwritten, Opened::Below) => AfterStop::Back,
        (Stopped::Unwritten, Opened::Directly) => AfterStop::Left,
    }
}

struct LoginView {
    /// What the sign-in replaces once it completes, under the title.
    notice: Option<String>,
    page: Option<(Box<str>, Option<Box<str>>)>,
    status: Cow<'static, str>,
    browser_failed: bool,
    accepts_manual: bool,
    manual: String,
    limited: bool,
    following: Option<Pressed>,
}

impl LoginView {
    fn new(glyphs: Glyphs) -> Self {
        Self {
            notice: None,
            page: None,
            status: Cow::Owned(about("waiting", CANCEL, glyphs)),
            browser_failed: false,
            accepts_manual: false,
            manual: String::new(),
            limited: false,
            following: None,
        }
    }

    /// The view of a sign-in opened as `leaves` says, replacing `replaces`
    /// where the provider holds a credential.
    fn opened(mut self, leaves: &str, replaces: Option<String>, glyphs: Glyphs) -> Self {
        self.status = Cow::Owned(about("waiting", leaves, glyphs));
        self.notice =
            replaces.map(|replaces| format!("Signing in replaces {replaces} once it completes."));
        self
    }

    /// Says the sign-in is being stopped, which can take a moment where its
    /// credential is being written.
    fn stopping(&mut self) {
        self.status = Cow::Borrowed(STOPPING);
    }

    /// Takes one update from the login, opening the browser without the
    /// `withheld` variables when the update is the page to visit.
    fn apply(&mut self, update: LoginUpdate, withheld: &[&str]) -> bool {
        match update {
            LoginUpdate::Authorize {
                browser_uri,
                shown_uri,
                user_code,
                manual,
            } => {
                self.browser_failed =
                    crate::cli::browser::open(&browser_uri, withheld.iter().copied()).is_err();
                self.page = Some((shown_uri, user_code));
                self.accepts_manual = manual;
                self.status = Cow::Borrowed("a browser should open; waiting for authorization…");
                false
            }
            LoginUpdate::Progress { message } => {
                self.accepts_manual = false;
                self.manual.clear();
                self.status = Cow::Borrowed(message);
                false
            }
            LoginUpdate::Complete => true,
        }
    }

    fn key<T: Terminal>(&mut self, renderer: &mut Renderer<T>) -> Result<Option<Pressed>, Fatal> {
        if let Some(following) = self.following.take() {
            return Ok(renderer.took(following)?);
        }
        if !renderer.waiting(Duration::ZERO)? {
            return Ok(None);
        }
        Ok(renderer.took(pressed()?)?)
    }

    // An event token is handed over, not lent: the handler takes the one thing
    // the reader produced, and a reference would say the caller kept a say in it.
    #[allow(clippy::needless_pass_by_value)]
    fn press(&mut self, pressed: Pressed, attempt: &LoginAttempt) -> Result<bool, Fatal> {
        match pressed {
            Pressed::Key(Key::Char(first)) if self.accepts_manual => {
                let room = MAX_MANUAL.saturating_sub(self.manual.len());
                let (text, refused, after) = characters(first, room)?.into_parts();
                self.manual.push_str(&text);
                self.limited = refused;
                self.following = after;
                Ok(true)
            }
            Pressed::Pasted(text) => Ok(self.pasted(&text)),
            Pressed::Key(Key::Backspace) if self.accepts_manual => {
                self.limited = false;
                Ok(self.manual.pop().is_some())
            }
            Pressed::Key(Key::Enter) if self.accepts_manual && !self.manual.trim().is_empty() => {
                match attempt.submit(&self.manual) {
                    Ok(()) => {
                        self.manual.clear();
                        self.accepts_manual = false;
                        self.status = Cow::Borrowed("checking pasted authorization…");
                    }
                    Err(problem) => self.status = Cow::Owned(format!("! {problem}")),
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Takes a pasted line into the box the way typed characters go in, and
    /// says whether the picture changed.
    ///
    /// Whole or not at all: half a callback is worth less than none, and the
    /// row beneath the box says why nothing grew.
    fn pasted(&mut self, text: &str) -> bool {
        if !self.accepts_manual {
            return false;
        }
        let text = secret::pasted(text);
        let room = MAX_MANUAL.saturating_sub(self.manual.len());
        self.limited = text.len() > room;
        if !self.limited {
            self.manual.push_str(&text);
        }
        true
    }

    fn show<T: Terminal>(
        &self,
        renderer: &mut Renderer<T>,
        terms: &Terms,
        title: &str,
    ) -> Result<(), Fatal> {
        let (rows, caret) = self.frame(renderer.columns(), title, terms.style().glyphs());
        renderer.live(&rows, caret, terms.style().palette())?;
        Ok(())
    }

    fn frame(&self, columns: usize, title: &str, glyphs: Glyphs) -> (Vec<Row>, Caret) {
        let mut rows = Vec::with_capacity(7);
        rows.push(Row::new().then(Slot::Strong, clip(title, columns)));
        if let Some(notice) = &self.notice {
            rows.push(Row::new().then(Slot::Plain, clip(notice, columns)));
        }
        if let Some((url, code)) = &self.page {
            rows.push(Row::new().then(Slot::Plain, clip(&format!("Open {url}"), columns)));
            if let Some(code) = code {
                rows.push(
                    Row::new().then(Slot::Strong, clip(&format!("Enter code {code}"), columns)),
                );
            }
            if self.browser_failed {
                rows.push(Row::new().then(
                    Slot::Quiet,
                    clip("browser did not open; use the page above", columns),
                ));
            }
        } else {
            rows.push(Row::new().then(Slot::Quiet, clip("starting sign-in…", columns)));
        }
        if !self.accepts_manual {
            rows.push(Row::new().then(Slot::Quiet, clip(&self.status, columns)));
            let caret = Caret {
                row: rows.len().saturating_sub(1),
                column: 0,
            };
            return (rows, caret);
        }

        rows.push(Row::new().then(
            Slot::Quiet,
            clip("Paste the callback URL or code, then press Enter.", columns),
        ));
        // Both marks are one column in either set, so the mark and the space
        // after it take two columns wherever this is drawn, and the caret below
        // sits one column past as many of them as there are characters.
        let mark = format!("{} ", glyphs.caret());
        let room = columns.saturating_sub(2);
        let dots = glyphs
            .hidden()
            .repeat(self.manual.chars().count().min(room));
        rows.push(
            Row::new()
                .then(Slot::Accent, clip(&mark, columns))
                .then(Slot::Plain, &dots),
        );
        let hint = if self.limited {
            "authorization input is limited to 16 KiB"
        } else {
            &self.status
        };
        rows.push(Row::new().then(Slot::Quiet, clip(hint, columns)));
        let caret = Caret {
            row: rows.len().saturating_sub(2),
            column: (2 + dots.chars().count()).min(columns.saturating_sub(1)),
        };
        (rows, caret)
    }
}

/// Asks for a key for `named`'s own row, the one its variable is read for,
/// writes it down, and sets this session up with it, or says why nothing was
/// written: the box was left, or never stood.
///
/// Opened by the command, so Escape cancels. The box says what the key
/// replaces where the provider holds a credential, which is read here: a
/// store that cannot be read is said instead of the box.
fn given<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let rows = Rows::production();
    let held = match holding(&rows, terms) {
        Ok(held) => held,
        Err(failed) => return say(renderer, &format!("! {}", remedy(&failed))),
    };
    let glyphs = terms.style().glyphs();
    let way = rows.environment(named.name);
    let replaces = way.and_then(|way| replaced(way, &held, glyphs));
    // The row's name, which says the site where the provider has two.
    let shown = way.map_or(Cow::Borrowed(named.shown), |way| drawn(way.shown, glyphs));
    let asked = secret::ask(renderer, terms.style(), &shown, replaces.as_deref(), CANCEL)?;
    match asked {
        Asked::Key(key) => written(named, &key, renderer, conversation, terms),
        Asked::Left => say(renderer, LEFT),
        Asked::Cramped => say(renderer, CRAMPED),
    }
}

/// Writes `key` down as `named`'s and sets this session up with it, or says
/// what stopped.
///
/// The error names the path, and for a file that would not open what the
/// operating system said about it; the row under the command carries neither.
/// The path is in the reader's own home, and what the system said is what
/// looking at it will say again. The
/// key is still in hand and the box is gone, so there is nothing to retry
/// from — which is why the row says the way back in, chosen by what stopped
/// ([`remedy`]). Nothing else hears the error: this session has no channel a
/// command's failure is logged through.
fn written<T: Terminal>(
    named: Served,
    key: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let mut walk = Walk {
        renderer,
        conversation,
        terms,
        held: &[],
    };
    kept(named.name, named, key, &mut walk)
}

/// [`written`], under `stored`: the name of the row the key was given on.
fn kept<T: Terminal>(
    stored: &str,
    named: Served,
    key: &str,
    walk: &mut Walk<'_, T>,
) -> Result<(), Fatal> {
    let Walk {
        renderer,
        conversation,
        terms,
        ..
    } = walk;
    let terms = *terms;
    match terms.logins.keep(stored, key) {
        Ok(_) => {
            if let Some(way) = Rows::production().of(Kind::Key, stored) {
                warning::stored(renderer, terms, &content_use::row_route(way))?;
            }
            taken(named, renderer, conversation, terms)
        }
        Err(failed) => say(
            renderer,
            &format!(
                "! {}",
                about(STORE_FAILED, remedy(&failed), terms.style().glyphs())
            ),
        ),
    }
}

/// The way back in, by what stopped the store.
///
/// Four sentences for five causes: a store too large to parse and one that
/// will not parse are the same thing to the reader, a file crucible cannot
/// read and will not write over.
fn remedy(failed: &AuthError) -> &'static str {
    match failed {
        AuthError::Unwritable { .. } => STORE_UNWRITABLE,
        AuthError::Busy { .. } => STORE_BUSY,
        AuthError::Unreadable { .. } | AuthError::TooLarge { .. } => STORE_UNREADABLE,
        AuthError::Unreleased { .. } => STORE_UNRELEASED,
    }
}

/// Hands this session the provider whose credential was just written down —
/// unless another is already answering, which keeps everything it has.
///
/// The credential is on disk, so this run is now the run the next launch would
/// be, and reading it back through the same resolution is what makes that true
/// here instead of only at the next start. What it costs is a second read of a
/// file written a line ago; what it buys is that somebody who has just stored a
/// credential can type at the session in front of them.
///
/// Authentication never chooses a model or effort, and never retires the pair
/// in force. Where nothing names a model, the line says so; where a provider is
/// already answering, the line points at `/model` instead — that is the other
/// half of a first minute, and a session that stopped at "credential stored"
/// would leave the reader to find that out from the next refusal.
fn taken<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let asking = |provider| Command::Login { provider };
    let logged_in = match terms.perform_naming(conversation, named.name, asking) {
        Performed::Login(logged_in) => logged_in,
        other => return say(renderer, &astray(&other)),
    };
    let unwritten = match logged_in {
        // Written and unusable, which is exactly what the next run would meet.
        // Said now rather than left for it: a session that looked configured and
        // refused every turn is the state this whole command exists to end.
        LoggedIn::Unusable(problem) => return say(renderer, &format!("! {problem}")),

        // A credential says a provider can be reached and never which to ask, so
        // where another provider is already answering, it keeps the session it
        // has: the model in force belongs to that vendor, and pulling both out
        // from under the reader to honour a stored key would turn "add a second
        // credential" into "lose the conversation's setup". `/model` is the one
        // place the provider and the model change, and they change together there.
        LoggedIn::Elsewhere => {
            return say(
                renderer,
                &format!("login successful; /model switches to {}", named.name),
            );
        }
        LoggedIn::CacheHeld(problem) => return super::cache::held(renderer, &problem),
        LoggedIn::Serving {
            retained,
            unwritten,
        } => {
            super::cache::retained(renderer, retained)?;
            unwritten
        }
    };

    // Written down as well as switched to, so the next run here opens on the
    // provider whose credential was stored instead of asking again. A failure
    // loses the half that outlives the process and not the session in front of
    // the reader, which is the bargain `/model` is on.
    if let Some(problem) = unwritten {
        say(renderer, &format!("! {problem}"))?;
    }

    let model = conversation.runner().model();
    let said = if model.is_empty() {
        "login successful; choose a model with /model".to_owned()
    } else {
        format!("login successful; asking {model}")
    };

    say(renderer, &said)
}

#[cfg(test)]
mod tests;
