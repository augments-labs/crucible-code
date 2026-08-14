//! `/logout`: the providers a key was written down for, and forgetting one.
//!
//! The list is what is *there* rather than what could be. `/login` offers every
//! provider this build serves, because any of them can be given a key; this one
//! offers the ones a key was already written down for, because the rest have
//! nothing to forget. That is the whole difference between the two panels, and
//! it is why this reads the store before it draws anything.
//!
//! What it reaches is that file and only that file. A key exported into the
//! shell belongs to the shell, wins over what is written down, and is not
//! something a session could take away — so what is said afterwards names which
//! of the two moved rather than saying "logged out" and leaving the rest to be
//! assumed.

use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip};

use crate::cli::converse::picking::{self, Taken};
use crate::cli::{Fatal, PROVIDERS, Served};

use super::{Terms, say};

/// The sentence under the panel's title: which of the two places a key can be
/// this reaches, since a panel of names cannot say it.
const SAID: &str = concat!(
    "Choose the provider to forget the key for. It is the key crucible wrote ",
    "down; one exported into your shell is untouched and goes on winning."
);

/// What is left to say once a key has been forgotten.
const KEPT: &str = "only what was written down; a key in the environment still wins";

/// What escape leaves behind, in place of the rows it used to write.
const LEFT: &str = "cancelled, nothing signed out";

/// Runs it: the one named forgotten, one chosen off the panel forgotten, or
/// what there is to choose from.
///
/// `keys` is whether there is a keyboard to walk a panel with. Down a pipe
/// there is not, and a panel waiting for a key nobody can press is a session
/// that stopped — so what a piped run gets is the names as rows.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let stored = terms.logins.read();
    let names: Vec<&str> = stored.providers().collect();
    let held = held(&names);

    // Before anything is drawn: a panel of nothing has no entry to take and no
    // reason to be stood up, and the rows underneath would be none as well.
    if held.is_empty() {
        return say(renderer, terms, "nothing is logged in");
    }

    if let Some(named) = held.iter().copied().find(|one| one.name == said) {
        return forgetting(named, renderer, terms);
    }

    // Nobody named and a keyboard to walk a list with: the panel, and what comes
    // off it is the same fact as a name typed on the line.
    if keys && said.is_empty() {
        match chosen(&held, renderer, terms)? {
            Taken::Took(taken) => return forgetting(taken, renderer, terms),
            // Escape asked for the screen that was there before the panel. The
            // rows under it would be the same question put a second time.
            Taken::Left => return say(renderer, terms, LEFT),
            Taken::Cramped => {}
        }
    }

    // The word came off the line and was never shape-checked — anything at all
    // can follow `/logout ` — so it goes out the way arrived text goes out. It
    // is one sentence for a name this build never heard of and for a provider
    // holding no key here, because it is true of both and the difference is not
    // one the answer underneath leaves anybody guessing at.
    if !said.is_empty() {
        renderer.commit(&format!("! not logged in to {said}"))?;
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = held
        .into_iter()
        .map(|one| {
            let said = format!("/logout {} — {}", one.name, reaches(one));

            Row::new().then(Slot::Quiet, clip(&said, columns))
        })
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}

/// The providers this build serves that `stored` holds a key for.
///
/// In [`PROVIDERS`] order rather than the store's, so this list and `/login`'s
/// read the same way down. A name in the file that this build does not serve is
/// not among them: it names no provider here, so there is nothing to draw it as
/// and no session that could have asked with it. The file is where it came from
/// and the file is where it stays.
fn held(stored: &[&str]) -> Vec<Served> {
    PROVIDERS
        .into_iter()
        .filter(|one| stored.contains(&one.name))
        .collect()
}

/// What forgetting one provider's key reaches, and what it leaves.
///
/// The one thing that differs between entries that would otherwise read
/// identically, and the thing worth knowing before pressing Enter: the variable
/// is named so it can be seen not to be in this.
fn reaches(one: Served) -> String {
    format!("the key written down, not {}", one.key)
}

/// Stands the panel where the prompt box was, and says what was taken off it.
///
/// `None` is a panel that was left and a window with no room to stand one in
/// alike. Neither is a provider, and both come out as the rows above — which is
/// the answer `/logout` always has, and the only one a short window can be
/// given.
fn chosen<T: Terminal>(
    held: &[Served],
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<Taken<Served>, Fatal> {
    let says: Vec<String> = held.iter().copied().map(reaches).collect();

    let shown: Vec<Offered<'_>> = held
        .iter()
        .zip(&says)
        .map(|(one, says)| Offered {
            name: one.shown,
            says,
        })
        .collect();

    let panel = Panel {
        title: "Log out",
        said: Some(SAID),
        shown: &shown,
        chosen: 0,
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    Ok(picking::pick(renderer, terms.style, panel)?.of(held))
}

/// Forgets `named`'s key, and says what that reached.
fn forgetting<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Whether there was still one there to forget is not the question being
    // answered. Another crucible having taken it between the read above and
    // this line leaves the key gone, which is what was asked for.
    if let Err(problem) = terms.logins.forget(named.name) {
        return say(renderer, terms, &format!("! {problem}"));
    }

    let columns = renderer.columns();
    let said = format!("logged out of {}", named.name);
    let rows = [
        Row::new().then(Slot::Plain, clip(&said, columns)),
        Row::new().then(Slot::Quiet, clip(KEPT, columns)),
    ];

    Ok(renderer.present(&rows, terms.style.palette())?)
}

#[cfg(test)]
mod tests;
