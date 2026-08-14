//! `/login`: which providers there are, and what each one reads a key from.
//!
//! The welcome sends people here — a run with no key says to use `/login` — so
//! this answers the question that sentence leaves: which names crucible knows,
//! and which variable each of them signs a request from. Both halves come off
//! [`PROVIDERS`], so a provider this build serves and cannot be logged in to is
//! not a state that exists.
//!
//! The key itself is never on the line. A key typed after a command is a key in
//! the shell's history file, in the process listing while the command runs, and
//! in the scrollback afterwards — three places it was never meant to be, none
//! of which this program could clear.

use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::{Fatal, PROVIDERS};

use super::Terms;

/// Runs it: the one named, or all of them where the line named none.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let named = PROVIDERS.into_iter().find(|one| one.name == said);

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
