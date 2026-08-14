//! `/login`: a key given once, to a box that does not echo it.
//!
//! The provider is named on the line and the key is not. A key typed after a
//! command is a key in the shell's history file, in the process listing while
//! the command runs, and in the scrollback afterwards — three places it was
//! never meant to be, none of which this program could clear. So the line names
//! who the key is for and the box below asks for the key itself. What it writes
//! is read by the next launch, which is where a stored key becomes the provider
//! a run is set up for.
//!
//! Naming nobody, and naming somebody this build has never heard of, both fall
//! through to which names crucible knows and which variable each of them signs
//! a request from. Both halves come off [`PROVIDERS`], so a provider this build
//! serves and cannot be logged in to is not a state that exists.

use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::converse::secret;
use crate::cli::{Fatal, PROVIDERS, Served};

use super::Terms;

/// Runs it: a key taken for the one named, or where each reads one from.
///
/// `keys` is whether there is a keyboard to take one from. Down a pipe there is
/// not, and a box waiting for a key nobody can type is a session that stopped —
/// so what a piped run gets is the row naming the variable instead.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let named = PROVIDERS.into_iter().find(|one| one.name == said);

    if let Some(named) = named
        && keys
    {
        return given(named, renderer, terms);
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
