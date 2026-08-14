//! `/model`: which model answers, named on the line or read off the list.
//!
//! The name is the whole of what this command carries, and unlike a key it is
//! meant to be seen. Naming it is what a script does and what somebody who
//! already knows the spelling does; on its own it says what is being asked now
//! and what else this provider could be, because a model name is a string a
//! vendor chose and there is no guessing it.
//!
//! Only the models of the provider this run is set up for are listed. A name
//! reaches whichever vendor the key belongs to, so one from another vendor comes
//! back as a refusal with somebody else's model name in it — a mistake this
//! program would have spelled out for them.
//!
//! What is taken is written down, because the answer to "which model" is the
//! same answer every time this directory is opened and asking it once a session
//! is asking it for ever.

use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::{Fatal, NO_MODEL_CHOSEN, NOTHING_TO_ASK, PROVIDERS, remember};

use super::Terms;

/// Runs it: the model named, or what is being asked now and what else could be.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Nothing holds a key, so there is no provider to write a name under and no
    // vendor to send it to. Answering "chosen" here would be a session that says
    // it is set up and refuses every turn.
    let Some(provider) = terms.provider else {
        return renderer.commit(NOTHING_TO_ASK).map_err(Fatal::from);
    };

    if said.is_empty() {
        return listed(provider, renderer, runner, terms);
    }

    taken(said, provider, renderer, runner, terms)
}

/// Asks it from the next turn on, and writes it down for the next run.
///
/// It goes to the file at home under the provider this run is set up for — a
/// model belongs to the vendor that serves it, and a name written under the
/// wrong one is the mismatch this release exists to stop.
///
/// A failure to write does not undo the switch. What is lost is the part that
/// outlives the process, and the line drawn says so, which is the same bargain
/// an answer of `always` is on.
fn taken<T: Terminal>(
    name: &str,
    provider: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    runner.ask(name);

    // The word may have come off the line and was never shape-checked — anything
    // at all can follow `/model ` — so it goes out the way arrived text goes out.
    renderer.commit(&format!("{provider}/{name}"))?;

    let said = match remember::choosing(&terms.choosing, provider, name) {
        Ok(()) => format!("written to {}", terms.choosing.display()),
        Err(problem) => {
            renderer.commit(&format!("! {problem}"))?;
            "asked for this session only".to_owned()
        }
    };

    // Wrapped rather than clipped: a path is most of this row and half of one
    // says nothing about where to look.
    let rows: Vec<Row> = fold(&said, renderer.columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Quiet, row))
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}

/// What is being asked now, and the lines that would ask for something else.
fn listed<T: Terminal>(
    provider: &str,
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Read out of a configuration file or off the command line either way, so
    // it goes out the way arrived text goes out.
    match runner.model() {
        "" => renderer.commit(NO_MODEL_CHOSEN)?,
        name => renderer.commit(name)?,
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = PROVIDERS
        .into_iter()
        .filter(|one| one.name == provider)
        .flat_map(|one| one.models)
        .map(|model| Row::new().then(Slot::Quiet, clip(&format!("/model {model}"), columns)))
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}
