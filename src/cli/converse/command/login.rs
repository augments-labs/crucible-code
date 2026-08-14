//! `/login`: a provider taken off a panel, and a key given to a box that does
//! not echo it.
//!
//! The provider is named on the line or chosen from the panel, and the key is
//! neither. A key typed after a command is a key in the shell's history file,
//! in the process listing while the command runs, and in the scrollback
//! afterwards — three places it was never meant to be, none of which this
//! program could clear. So the two halves are asked for separately: who the key
//! is for, out loud, and the key itself into a box. What that writes is read by
//! the next launch, which is where a stored key becomes the provider a run is
//! set up for.
//!
//! Naming somebody this build has never heard of, a panel that was left, and a
//! window with no room to stand one in all come out the same way: which names
//! crucible knows and which variable each of them signs a request from, written
//! into the scrollback where it can be scrolled. Every one of those halves comes
//! off [`PROVIDERS`], so a provider this build serves and cannot be logged in to
//! is not a state that exists.

use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip};

use crate::cli::converse::picking;
use crate::cli::converse::secret;
use crate::cli::{Fatal, PROVIDERS, Served};

use super::Terms;

/// The sentence under the panel's title: what standing there cannot show, which
/// is that the key is asked for next and that this run is not the one it
/// changes.
const SAID: &str = concat!(
    "Choose the provider to give crucible a key for. It is typed into a box ",
    "that does not echo it, and written down for the next run to read."
);

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
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let named = PROVIDERS.into_iter().find(|one| one.name == said);

    if keys {
        if let Some(named) = named {
            return given(named, renderer, terms);
        }

        // Nobody named and a keyboard to walk a list with: the panel, and what
        // comes off it is the same fact as a name typed on the line.
        if said.is_empty()
            && let Some(taken) = chosen(renderer, terms)?
        {
            return given(taken, renderer, terms);
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

/// Stands the panel where the prompt box was, and says what was taken off it.
///
/// `None` is a panel that was left and a window with no room to stand one in
/// alike. Neither is a provider, and both come out as the rows above — the
/// answer `/login` always has, and the only one a short window can be given.
fn chosen<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<Option<Served>, Fatal> {
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
        said: Some(SAID),
        shown: &shown,
        chosen: 0,
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    let Some(at) = picking::pick(renderer, terms.style, panel)? else {
        return Ok(None);
    };

    // The index is into the list the panel was handed, built from `PROVIDERS`
    // in order — so a lookup that cannot miss, written as one that can rather
    // than as an assertion nobody would read again.
    Ok(PROVIDERS.get(at).copied())
}

/// Asks for a key and writes it down, saying which of those happened.
fn given<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let Some(key) = secret::ask(renderer, terms.style, &format!("{} api key", named.name))? else {
        return say(renderer, terms, "nothing was written");
    };

    match terms.logins.keep(named.name, &key) {
        // What the key changes is which provider the *next* run is set up for.
        // This one was built against whatever was there when it started, and a
        // line saying only "logged in" would leave somebody typing at a session
        // that goes on refusing every turn.
        Ok(()) => say(
            renderer,
            terms,
            &format!(
                "logged in to {}; crucible asks it from the next run",
                named.name
            ),
        ),

        // The key is still in hand and the box is gone, so there is nothing to
        // retry from — which is why this says what stopped rather than only
        // that something did.
        Err(problem) => say(renderer, terms, &format!("! {problem}")),
    }
}

/// Says one line back, quietly, clipped to the window it is said in.
fn say<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms, said: &str) -> Result<(), Fatal> {
    let row = Row::new().then(Slot::Quiet, clip(said, renderer.columns()));

    Ok(renderer.present(&[row], terms.style.palette())?)
}
