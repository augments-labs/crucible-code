//! `/effort`: how hard to think, taken off a ladder or named on the line.
//!
//! The ladder is crucible's and it is the same everywhere — five rungs, in the
//! same order, whichever vendor this session is on. Which of them a model
//! actually serves, and whether it takes a rung at all, is the vendor's answer
//! and it differs between models of one vendor, so all five are offered and the
//! vendor is left to refuse what it does not serve. That is the bargain a model
//! name is already on: what is offered is a shortcut past somebody's
//! documentation, not a claim about what is behind it.
//!
//! Which is why the ladder is stood over a model by name, and why a session
//! with no model chosen is sent to `/model` instead of being asked this. A rung
//! is not a setting of crucible's that a vendor happens to read — it is one
//! word in one request, and what it buys is the model's to say.
//!
//! What is taken is written down beside the model, under the same provider, for
//! the same reason: how hard to think is the same answer every time this
//! machine is used, and asking it once a session is asking it for ever.

use crucible_core::{Effort, EffortError};
use crucible_runner::Runner;
use crucible_tui::{Ladder, Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::converse::picking::{self, Taken};
use crate::cli::{Fatal, remember, unasked};

use super::{Terms, say};

/// What escape leaves behind, in place of the listing it used to write.
const LEFT: &str = "cancelled, no rung taken";

/// What the two ends of the track are called.
///
/// The trade rather than the amount: the amount is the word on each rung, and
/// what somebody standing here is choosing between is waiting and thinking.
const ENDS: (&str, &str) = ("Faster", "Smarter");

/// The keys, under the track they work on.
const FOOTER: &str = "←/→ to adjust · enter to confirm · esc to cancel";

/// The rung the ladder opens on where nothing has chosen one.
///
/// A mark has to stand somewhere, and the rung most sessions want is a better
/// place to start from than an end. It is not what a run with no rung asks for —
/// nothing is asked until Enter, and leaving the ladder leaves the session
/// asking for no rung at all.
const OPENS_ON: Effort = Effort::High;

/// Runs it: the rung named, one taken off the ladder, or the rung in force and
/// the lines that ask for the others.
///
/// `keys` is whether there is a keyboard to walk a ladder with. Down a pipe
/// there is not, and a ladder waiting for a key nobody can press is a session
/// that stopped — so what a piped run gets is the list, written where it can be
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
    // A rung is asked of a model, so the ladder is stood over one by name and
    // there has to be one. Without a provider there is nobody to write it under
    // either, and the two want different sentences: one says set a key, the
    // other says the key is fine and pick a model.
    let named = terms.provider.get();
    let Some(provider) = named.filter(|_| !runner.model().is_empty()) else {
        return renderer.commit(unasked(named)).map_err(Fatal::from);
    };

    if !said.is_empty() {
        return match said.parse() {
            Ok(effort) => taken(effort, provider, renderer, runner, terms),
            Err(refused) => mistyped(&refused, renderer, terms),
        };
    }

    if keys {
        match chosen(renderer, runner, terms)? {
            Taken::Took(effort) => return taken(effort, provider, renderer, runner, terms),
            // Escape asked for the screen that was there before the panel. A
            // listing under it would be the same question put a second time.
            Taken::Left => return say(renderer, terms, LEFT),
            Taken::Cramped => {}
        }
    }

    listed(renderer, runner, terms)
}

/// Stands the ladder where the prompt box was, and says which rung came off it.
///
/// A window with no room to stand one in comes out as the listing below.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<Taken<Effort>, Fatal> {
    let rungs: Vec<&str> = Effort::LADDER.iter().map(|one| one.as_str()).collect();

    // The model by name, because a rung is asked of one model and what it buys
    // differs between them. It is also the fact that stops this reading as a
    // setting of crucible's — the ladder is what is sent, and this is who to.
    let title = format!("Effort · {}", runner.model());

    let ladder = Ladder {
        title: &title,
        rungs: &rungs,
        chosen: rung(runner.effort().unwrap_or(OPENS_ON)),
        ends: ENDS,
        footer: FOOTER,
    };

    Ok(picking::adjust(renderer, terms.style, ladder)?.of(&Effort::LADDER))
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
/// The rungs the ladder is handed are [`Effort::LADDER`] in order, so this is a
/// lookup that cannot miss — written as one that can rather than as an
/// assertion nobody would read again.
fn rung(effort: Effort) -> usize {
    Effort::LADDER
        .iter()
        .position(|one| *one == effort)
        .unwrap_or(0)
}
