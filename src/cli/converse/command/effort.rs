//! `/effort`: how hard to think, taken off a panel or named on the line.
//!
//! The ladder is crucible's and it is the same everywhere — five rungs, in the
//! same order, whichever vendor this session is on. Which of them a model
//! actually serves, and whether it takes a rung at all, is the vendor's answer
//! and it differs between models of one vendor, so all five are offered and the
//! vendor is left to refuse what it does not serve. That is the bargain a model
//! name is already on: what is offered is a shortcut past somebody's
//! documentation, not a claim about what is behind it.
//!
//! What is taken is written down beside the model, under the same provider, for
//! the same reason: how hard to think is the same answer every time this
//! machine is used, and asking it once a session is asking it for ever.

use crucible_core::{Effort, EffortError};
use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::converse::picking;
use crate::cli::{Fatal, NOTHING_TO_ASK, remember};

use super::Terms;

/// The sentence under the panel's title: what standing there cannot show.
const SAID: &str = concat!(
    "Choose how hard crucible asks the model to think, from the next turn on. ",
    "It is written down, so the next run here asks for the same. Which rungs a ",
    "model serves is up to its vendor."
);

/// The rung the panel opens on where nothing has chosen one.
///
/// A list has to open somewhere, and the rung most sessions want is a better
/// place to start from than the bottom. It is not what a run with no rung asks
/// for — nothing is asked until a row is taken with Enter, and leaving the panel
/// leaves the session asking for no rung at all.
const OPENS_ON: Effort = Effort::High;

/// What each rung is for, in the few words a row has room for.
///
/// Written about the cost rather than the amount, because the amount is the
/// word already on the row. What somebody standing here is choosing between is
/// waiting and thinking, and nothing else on the screen says so.
const fn says(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "answers soonest, thinks least",
        Effort::Medium => "some thought, little waiting",
        Effort::High => "thinks before answering",
        Effort::Xhigh => "long thought on hard questions",
        Effort::Max => "as hard as this model thinks",
    }
}

/// Runs it: the rung named, one taken off the panel, or the rung in force and
/// the lines that ask for the others.
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
    // Nothing holds a key, so there is no provider to write a rung under and no
    // vendor to send it to. The rung would be real and would reach nothing.
    let Some(provider) = terms.provider.get() else {
        return renderer.commit(NOTHING_TO_ASK).map_err(Fatal::from);
    };

    if !said.is_empty() {
        return match said.parse() {
            Ok(effort) => taken(effort, provider, renderer, runner, terms),
            Err(refused) => mistyped(&refused, renderer, terms),
        };
    }

    if keys && let Some(effort) = chosen(renderer, runner, terms)? {
        return taken(effort, provider, renderer, runner, terms);
    }

    listed(renderer, runner, terms)
}

/// Stands the panel where the prompt box was, and says which rung came off it.
///
/// `None` is a panel that was left and a window with no room to stand one in
/// alike. Neither is a rung, and both come out as the listing below.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<Option<Effort>, Fatal> {
    let asking = runner.effort();

    // Which rung is in force goes here rather than beside a row: it is one fact
    // about the session, and a list that says it once is a list whose rows all
    // read the same way.
    let saying = match asking {
        Some(effort) => format!("{} is what is asked now. {SAID}", effort.as_str()),
        None => format!("The vendor's own default is in force. {SAID}"),
    };

    let shown: Vec<Offered<'_>> = Effort::LADDER
        .iter()
        .map(|effort| Offered {
            name: effort.as_str(),
            says: says(*effort),
        })
        .collect();

    let panel = Panel {
        title: "Effort",
        said: Some(&saying),
        shown: &shown,
        chosen: rung(asking.unwrap_or(OPENS_ON)),
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    let Some(at) = picking::pick(renderer, terms.style, panel)? else {
        return Ok(None);
    };

    Ok(Effort::LADDER.get(at).copied())
}

/// Asks for it from the next turn on, and writes it down for the next run.
///
/// A failure to write does not undo the rung. What is lost is the part that
/// outlives the process, and the line drawn says so — the same bargain `/model`
/// and an answer of `always` are both on.
fn taken<T: Terminal>(
    effort: Effort,
    provider: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    runner.think(effort);

    let said = match remember::thinking(&terms.choosing, provider, effort) {
        // Where it went is not news. It is the same file every time, chosen by
        // crucible rather than by the reader, and naming it on every rung is a
        // session reading its own bookkeeping out loud. What the reader asked
        // for is the rung, so the rung is the answer.
        Ok(()) => format!("{} effort", effort.as_str()),
        Err(problem) => {
            renderer.commit(&format!("! {problem}"))?;
            format!("{} effort, this session only", effort.as_str())
        }
    };

    // Wrapped rather than clipped: short as this row is, a narrow enough window
    // would still cut it, and half of it says nothing about what was asked for.
    let rows: Vec<Row> = fold(&said, renderer.columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Quiet, row))
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}

/// A word that is not a rung, said back with the rungs that are.
///
/// The refusal is the type's own, which is the same sentence `--effort` is
/// refused with before the session starts. One question asked twice deserves
/// one answer, and somebody who mistyped it here has already read that answer
/// somewhere else.
fn mistyped<T: Terminal>(
    refused: &EffortError,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    // The word came off the line and was never shape-checked — anything at all
    // can follow `/effort ` — so it goes out the way arrived text goes out.
    renderer.commit(&format!("! {refused}"))?;

    listing(renderer, terms)
}

/// What is being asked now, and the lines that would ask for something else.
fn listed<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let saying = match runner.effort() {
        Some(effort) => effort.as_str(),
        // Not a rung, because none was asked for. Naming one here would be this
        // program guessing at a vendor's default and printing the guess.
        None => "the vendor's own default",
    };
    let row = Row::new().then(Slot::Plain, clip(saying, renderer.columns()));

    renderer.present(&[row], terms.style.palette())?;

    listing(renderer, terms)
}

/// The whole ladder, as the line that asks for each rung.
fn listing<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let rows: Vec<Row> = Effort::LADDER
        .iter()
        .map(|effort| {
            Row::new().then(
                Slot::Quiet,
                clip(&format!("/effort {}", effort.as_str()), columns),
            )
        })
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}

/// Where on the ladder this rung stands.
///
/// The list the panel is handed is the ladder in order, so this is a lookup
/// that cannot miss — written as one that can rather than as an assertion
/// nobody would read again.
fn rung(effort: Effort) -> usize {
    Effort::LADDER
        .iter()
        .position(|one| *one == effort)
        .unwrap_or(0)
}
