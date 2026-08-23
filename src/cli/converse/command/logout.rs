//! `/logout`: the providers a credential was written down for, and forgetting
//! one.
//!
//! The list is what is *there* rather than what could be. `/login` offers the
//! account and key routes this build can store; this one offers providers with
//! a stored credential, because the rest have nothing this command can remove.
//! That is the whole difference between the two panels, and it is why this
//! reads the store before it draws anything.
//!
//! What it reaches is that file and only that file. A key inherited from the
//! launching environment is not something a child process can take away from
//! its parent shell. The command therefore names the source that remains and
//! tells the user where it has to be removed; it never says the provider is
//! signed out while that source is active.

use crucible_provider::Unavailable;
use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip};

use crate::cli::converse::picking::{self, Taken};
use crate::cli::{
    CredentialSource, Fatal, NO_PROVIDER_CHOSEN, NOTHING_TO_ASK, PROVIDERS, Served, served,
};

use super::{Terms, about, say};

/// The sentence under the panel's title: which of the two places a key can be
/// this reaches, since a panel of names cannot say it.
const SAID: &str = concat!(
    "Choose a credential stored by Crucible to remove. Environment credentials ",
    "are inherited from the launching shell and cannot be removed here."
);

/// What is left to say once a key has been forgotten.
const KEPT: &str = "the active provider is unchanged";

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
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let stored = terms.logins.read();
    let names: Vec<&str> = stored.providers().collect();
    let held = held(&names);

    // Before anything is drawn: a panel of nothing has no entry to take and no
    // reason to be stood up, and the rows underneath would be none as well.
    if held.is_empty() {
        let remaining = terms
            .provider
            .get()
            .and_then(|provider| served(provider).ok())
            .and_then(|provider| (terms.serving)(provider, &stored).ok());
        if let Some(remaining) = remaining {
            return say(
                renderer,
                &not_stored(terms.provider.get().unwrap_or_default(), &remaining.source),
            );
        }
        return say(
            renderer,
            "nothing is stored by Crucible; unset any environment keys in the shell",
        );
    }

    if let Some(named) = held.iter().copied().find(|one| one.name == said) {
        return forgetting(named, renderer, runner, terms);
    }

    // Nobody named and a keyboard to walk a list with: the panel, and what comes
    // off it is the same fact as a name typed on the line.
    if keys && said.is_empty() {
        match chosen(&held, renderer, terms)? {
            Taken::Took(taken) => return forgetting(taken, renderer, runner, terms),
            // Escape asked for the screen that was there before the panel. The
            // rows under it would be the same question put a second time.
            Taken::Left => return say(renderer, LEFT),
            Taken::Cramped => {}
        }
    }

    // The word came off the line and was never shape-checked — anything at all
    // can follow `/logout ` — so it goes out the way arrived text goes out. It
    // is one sentence for a name this build never heard of and for a provider
    // holding no key here, because it is true of both and the difference is not
    // one the answer underneath leaves anybody guessing at.
    if !said.is_empty() {
        renderer.commit(&format!("! no credential for {said} is stored by Crucible"))?;
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = held
        .into_iter()
        .map(|one| {
            let said = about(
                &format!("/logout {}", one.name),
                &reaches(one),
                terms.style().glyphs(),
            );

            Row::new().then(Slot::Quiet, clip(&said, columns))
        })
        .collect();

    Ok(renderer.present(&rows)?)
}

/// The providers this build serves that `stored` holds a credential for.
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
    format!("stored by Crucible; not inherited from {}", one.key)
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
        title: "Remove stored credential",
        said: Some(SAID),
        shown: &shown,
        chosen: 0,
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    Ok(picking::pick(renderer, terms.style(), panel)?.of(held))
}

/// Forgets `named`'s key, and says what that reached.
fn forgetting<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Whether there was still one there to forget is not the question being
    // answered. Another crucible having taken it between the read above and
    // this line leaves the key gone, which is what was asked for.
    if let Err(problem) = terms.logins.forget(named.name) {
        return say(renderer, &format!("! {problem}"));
    }

    let columns = renderer.columns();
    let said = format!("removed the stored credential for {}", named.name);
    let status = if terms.provider.get() == Some(named.name) {
        let stored = terms.logins.read();
        if let Ok(remaining) = (terms.serving)(named, &stored) {
            runner.serve(remaining.provider);
            not_stored(named.name, &remaining.source)
        } else {
            let warning = if PROVIDERS
                .into_iter()
                .any(|provider| (terms.serving)(provider, &stored).is_ok())
            {
                NO_PROVIDER_CHOSEN
            } else {
                NOTHING_TO_ASK
            };
            runner.ask("", crate::cli::startup::UNKNOWN_CEILING, None, None);
            runner.serve(Box::new(Unavailable::new(warning)));
            terms.provider.set(None);
            "the active session is now signed out".to_owned()
        }
    } else {
        KEPT.to_owned()
    };
    let rows = [
        Row::new().then(Slot::Plain, clip(&said, columns)),
        Row::new().then(Slot::Quiet, clip(&status, columns)),
    ];

    Ok(renderer.present(&rows)?)
}

/// What remains after the stored credential moved, without ever carrying the
/// credential value itself.
fn not_stored(provider: &str, source: &CredentialSource) -> String {
    match source {
        CredentialSource::Environment(variable) => {
            format!("{provider} still uses {variable}; unset it in the shell before restarting")
        }
        CredentialSource::StoredKey => {
            format!("{provider} is still authenticated by another stored API key")
        }
        CredentialSource::Subscription => {
            format!("{provider} is still authenticated by another stored account")
        }
    }
}

#[cfg(test)]
mod tests;
