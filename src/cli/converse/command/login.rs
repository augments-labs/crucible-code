//! `/login`: a provider taken off a panel, and a key given to a box that does
//! not echo it.
//!
//! The provider is named on the line or chosen from the panel, and the key is
//! neither. A key typed after a command is a key in the shell's history file,
//! in the process listing while the command runs, and in the scrollback
//! afterwards — three places it was never meant to be, none of which this
//! program could clear. So the two halves are asked for separately: who the key
//! is for, out loud, and the key itself into a box.
//!
//! What that writes is then read straight back, and the session is handed the
//! provider it buys. A key is given by somebody who wants to type at the screen
//! in front of them, and the file it lands in is what makes the run after this
//! one ask the same thing rather than what makes this one ask at all.
//!
//! The same two halves are what a run with no key for anything opens on, before
//! any prompt is read. That screen is this command standing without being asked
//! — the warning under the welcome names it, and somebody meeting crucible for
//! the first time has no reason to know it is a thing they can type.
//!
//! Naming somebody this build has never heard of, a panel that was left, and a
//! window with no room to stand one in all come out the same way: which names
//! crucible knows and which variable each of them signs a request from, written
//! into the scrollback where it can be scrolled. Every one of those halves comes
//! off [`PROVIDERS`], so a provider this build serves and cannot be logged in to
//! is not a state that exists.

use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip};

use crate::cli::converse::picking;
use crate::cli::converse::secret;
use crate::cli::{Fatal, PROVIDERS, Served};

use super::{Terms, say};

/// The sentence under the panel's title: what standing there cannot show, which
/// is that the key is asked for next and where it goes.
const SAID: &str = concat!(
    "Choose the provider to give crucible a key for. It is typed into a box ",
    "that does not echo it, and asked from the next turn on."
);

/// The same, on the screen a run opens with when nothing at all is set up.
///
/// Different because the reader did not ask for this one and the first thing it
/// owes them is why it is standing there. The rest is the sentence above, which
/// is true wherever the panel is.
const FIRST: &str = concat!(
    "crucible holds no key for any provider, so there is nothing it can ask ",
    "yet. Choose one to give a key for. It is typed into a box that does not ",
    "echo it, and asked from the next turn on."
);

/// What escape does where the panel was asked for.
const CANCEL: &str = "esc to cancel";

/// What it does on the screen a run opens with. Not `esc to cancel`: there is no
/// prompt behind that screen to cancel back to, and leaving it takes nothing
/// back — the offer stands, under the name of the command that opens it again.
const SKIP: &str = "esc to skip; /login opens this again";

/// Runs it: a key taken for the one named, one chosen off the panel, or where
/// each of them reads a key from.
///
/// `keys` is whether there is a keyboard to take one from. Down a pipe there is
/// not, and a panel or a box waiting for something nobody can type is a session
/// that stopped — so what a piped run gets is the rows naming the variables
/// instead.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let named = PROVIDERS.into_iter().find(|one| one.name == said);

    if keys {
        if let Some(named) = named {
            return given(named, renderer, runner, terms);
        }

        // Nobody named and a keyboard to walk a list with: the panel, and what
        // comes off it is the same fact as a name typed on the line.
        if said.is_empty()
            && let Some(chose) = chosen(renderer, terms, SAID, CANCEL)?
        {
            return given(chose, renderer, runner, terms);
        }
    }

    // The word came off the line and was never shape-checked — anything at all
    // can follow `/login ` — so it goes out the way arrived text goes out, and
    // the names that would have worked go under it.
    if named.is_none() && !said.is_empty() {
        renderer.commit(&format!("! no provider called {said}"))?;
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = PROVIDERS
        .into_iter()
        .filter(|one| named.is_none_or(|only| only.name == one.name))
        .map(|one| {
            let said = format!("/login {} — a key from {}", one.name, one.key);

            Row::new().then(Slot::Quiet, clip(&said, columns))
        })
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}

/// The screen a run with no key for any provider opens on.
///
/// The warning under the welcome names this command, and somebody meeting
/// crucible for the first time has no reason to know that is a thing they can
/// type. So the same panel stands before the first prompt is read, unasked —
/// which is the one difference, and it is what `said` and the footer are for.
///
/// Only where there is a keyboard, and that is the caller's to know: a panel
/// waiting for something nobody can press is a session that stopped.
pub(in crate::cli::converse) fn first<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    match chosen(renderer, terms, FIRST, SKIP)? {
        Some(named) => given(named, renderer, runner, terms),

        // Left, and nothing said about it. The warning the welcome drew is
        // still standing above the box this returns to, naming both ways out of
        // a run with no key — so a line here would be that sentence again,
        // written under it.
        None => Ok(()),
    }
}

/// Stands the panel where the prompt box was, and says what was taken off it.
///
/// `said` is the sentence under the title and `footer` the row under the
/// entries. They are the caller's because the two screens this panel stands on
/// differ in nothing else: one was asked for and one was not.
///
/// `None` is a panel that was left and a window with no room to stand one in
/// alike. Neither is a provider, and both come out as the rows above — the
/// answer `/login` always has, and the only one a short window can be given.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    said: &str,
    footer: &str,
) -> Result<Option<Served>, Fatal> {
    // Where else the same key can come from — and the one thing that differs
    // between rows that would otherwise read identically.
    let says: Vec<String> = PROVIDERS
        .iter()
        .map(|one| format!("typed here, or set in {}", one.key))
        .collect();

    let shown: Vec<Offered<'_>> = PROVIDERS
        .iter()
        .zip(&says)
        .map(|(one, says)| Offered {
            name: one.shown,
            says,
        })
        .collect();

    let panel = Panel {
        title: "Log in",
        said: Some(said),
        shown: &shown,
        chosen: 0,
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer,
    };

    let Some(at) = picking::pick(renderer, terms.style, panel)? else {
        return Ok(None);
    };

    // The index is into the list the panel was handed, built from `PROVIDERS`
    // in order — so a lookup that cannot miss, written as one that can rather
    // than as an assertion nobody would read again.
    Ok(PROVIDERS.get(at).copied())
}

/// Asks for a key, writes it down, and sets this session up with it.
fn given<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let Some(key) = secret::ask(renderer, terms.style, &format!("{} api key", named.name))? else {
        return say(renderer, terms, "nothing was written");
    };

    match terms.logins.keep(named.name, &key) {
        Ok(()) => taken(named, renderer, runner, terms),

        // The key is still in hand and the box is gone, so there is nothing to
        // retry from — which is why this says what stopped rather than only
        // that something did.
        Err(problem) => say(renderer, terms, &format!("! {problem}")),
    }
}

/// Hands this session the provider the key that was just written down buys.
///
/// The key is on disk, so this run is now the run the next launch would be, and
/// reading it back through the same resolution is what makes that true here
/// instead of only at the next start. What it costs is a second read of a file
/// written a line ago; what it buys is that somebody who has just logged in can
/// type at the session in front of them.
///
/// A model and a rung come with it, and only where nothing has answered yet: a
/// flag or a command that named one is somebody's own answer, and a file that
/// said nothing about this run while there was no key does not get to overrule
/// it now that there is. Where nothing names a model either, the line says so —
/// `/model` is the other half of a first minute, and a session that stopped at
/// "logged in" would leave the reader to find that out from the next refusal.
fn taken<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let set = match (terms.serving)(named, &terms.logins.read()) {
        Ok(set) => set,

        // Written and unusable, which is exactly what the next run would meet.
        // Said now rather than left for it: a session that looked logged in and
        // refused every turn is the state this whole command exists to end.
        Err(problem) => return say(renderer, terms, &format!("! {problem}")),
    };

    runner.serve(set.provider);
    terms.provider.set(Some(named.name));

    if runner.model().is_empty()
        && let Some(model) = set.model
    {
        runner.ask(&model);
    }

    if runner.effort().is_none()
        && let Some(effort) = set.effort
    {
        runner.think(effort);
    }

    let said = if runner.model().is_empty() {
        format!("logged in to {}; choose a model with /model", named.name)
    } else {
        format!("logged in to {}, asking {}", named.name, runner.model())
    };

    say(renderer, terms, &said)
}
