//! `/model`: which model answers, taken off a panel or named on the line.
//!
//! The name is the whole of what this command carries, and unlike a key it is
//! meant to be seen — so the line and the panel are two ways to the same place
//! rather than two halves of one thing. Naming it is what a script does and what
//! somebody who already knows the spelling does; the panel is for the rest,
//! which is most first runs, because a model name is a string a vendor chose and
//! there is no guessing it.
//!
//! Only the models of the provider this run is set up for are offered. A name
//! reaches whichever vendor the key belongs to, so one from another vendor comes
//! back as a refusal with somebody else's model name in it — a mistake this
//! program would have spelled out for them.
//!
//! What is taken is written down, because the answer to "which model" is the
//! same answer every time this directory is opened and asking it once a session
//! is asking it for ever.

use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::converse::picking;
use crate::cli::{Fatal, NO_MODEL_CHOSEN, NOTHING_TO_ASK, PROVIDERS, remember};

use super::Terms;

/// The sentence under the panel's title: what standing there cannot show, which
/// is that this outlives the session it was chosen in.
const SAID: &str = concat!(
    "Choose the model crucible asks from the next turn on. It is written down ",
    "for this workspace, so the next run here asks the same one."
);

/// Runs it: the model named, one taken off the panel, or what is being asked
/// now and what else could be.
///
/// `keys` is whether there is a keyboard to walk a panel with. Down a pipe there
/// is not, and a panel waiting for a key nobody can press is a session that
/// stopped — so what a piped run gets is the list, written where it can be
/// scrolled and typed back a line at a time.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    // Nothing holds a key, so there is no provider to write a name under and no
    // vendor to send it to. Answering "chosen" here would be a session that says
    // it is set up and refuses every turn.
    let Some(provider) = terms.provider.get() else {
        return renderer.commit(NOTHING_TO_ASK).map_err(Fatal::from);
    };

    if !said.is_empty() {
        return taken(said, provider, renderer, runner, terms);
    }

    if keys && let Some(name) = chosen(provider, renderer, runner, terms)? {
        return taken(name, provider, renderer, runner, terms);
    }

    listed(provider, renderer, runner, terms)
}

/// Stands the panel where the prompt box was, and says which model came off it.
///
/// `None` is a panel that was left and a window with no room to stand one in
/// alike. Neither is a model, and both come out as the listing below — the
/// answer `/model` always had, and the only one a short window can be given.
fn chosen<T: Terminal>(
    provider: &str,
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<Option<&'static str>, Fatal> {
    let Some(served) = PROVIDERS.into_iter().find(|one| one.name == provider) else {
        return Ok(None);
    };

    // Which model is in force goes here rather than beside an entry: it is one
    // fact about the session, and a list that says it once is a list whose rows
    // all read the same way.
    let saying = match runner.model() {
        "" => format!("Nothing is being asked yet. {SAID}"),
        name => format!("{name} is what is asked now. {SAID}"),
    };

    // The other way to the same model, which is the one thing that differs
    // between rows that would otherwise be a name and nothing else.
    let says: Vec<String> = served
        .models
        .iter()
        .map(|model| format!("--model {provider}/{model}"))
        .collect();

    let shown: Vec<Offered<'_>> = served
        .models
        .iter()
        .zip(&says)
        .map(|(model, says)| Offered { name: model, says })
        .collect();

    let panel = Panel {
        title: "Model",
        said: Some(&saying),
        shown: &shown,
        // Opened on the one in force, so the first key moves off a known place
        // rather than towards one. A model chosen elsewhere is on no row here,
        // and the sentence above is where it is named.
        chosen: served
            .models
            .iter()
            .position(|model| *model == runner.model())
            .unwrap_or(0),
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    let Some(at) = picking::pick(renderer, terms.style, panel)? else {
        return Ok(None);
    };

    // The index is into the list the panel was handed, built from the provider's
    // models in order — so a lookup that cannot miss, written as one that can
    // rather than as an assertion nobody would read again.
    Ok(served.models.get(at).copied())
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
