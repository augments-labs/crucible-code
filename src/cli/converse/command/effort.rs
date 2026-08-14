//! `/effort`: how hard to think, taken off a ladder or named on the line.
//!
//! The ladder is crucible's and it is the same everywhere — five rungs, in the
//! same order, whichever vendor this session is on. Which of them a model
//! actually serves is the vendor's answer and it differs between models of one
//! vendor, so the ladder that gets stood up holds the rungs this build has
//! written down for the model in force. Several models serve none at all, and
//! those are told rather than offered a ladder with nothing on it.
//!
//! Narrowed rather than greyed. A rung drawn but unreachable is a row the
//! arrows have to step over, and one nobody can act on is one the panel is
//! better off not holding — what is left is short enough to read at a glance,
//! which is the whole argument for a ladder over a list.
//!
//! What is typed is not narrowed. `/effort max` and `--effort max` go to the
//! vendor whatever the table here says, because the table is read off somebody
//! else's documentation and goes stale between releases: a rung it has not
//! caught up with should cost a missing row in a panel, never a refusal from
//! the program that is not the one serving the model.
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
use crate::cli::{Fatal, remember, rungs, unasked};

use super::{Terms, say};

/// What escape leaves behind, in place of the listing it used to write.
const LEFT: &str = "cancelled, no rung taken";

/// What a model that serves no rung is said with, after its name.
///
/// The name rather than "this model", because the name is the half that can be
/// acted on: it is what `/model` changes, and what the vendor's own
/// documentation is indexed by.
const NO_RUNG: &str =
    "takes no rung: its vendor serves none for it. /model picks a model that does";

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

    // Every rung for a model this build has not heard of, which is why this is
    // read before the word typed after the command rather than instead of it:
    // what is typed goes to the vendor either way, and this decides only what
    // gets offered and what gets listed back.
    let rungs = rungs(provider, runner.model());

    if !said.is_empty() {
        return match said.parse() {
            Ok(effort) => taken(effort, provider, renderer, runner, terms),
            Err(refused) => mistyped(&refused, renderer, terms, rungs),
        };
    }

    if rungs.is_empty() {
        return say(renderer, terms, &format!("{} {NO_RUNG}", runner.model()));
    }

    if keys {
        match chosen(renderer, runner, terms, rungs)? {
            Taken::Took(effort) => return taken(effort, provider, renderer, runner, terms),
            // Escape asked for the screen that was there before the panel. A
            // listing under it would be the same question put a second time.
            Taken::Left => return say(renderer, terms, LEFT),
            Taken::Cramped => {}
        }
    }

    listed(renderer, runner, terms, rungs)
}

/// Stands the ladder where the prompt box was, and says which rung came off it.
///
/// A window with no room to stand one in comes out as the listing below.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
    served: &[Effort],
) -> Result<Taken<Effort>, Fatal> {
    let words: Vec<&str> = served.iter().map(|one| one.as_str()).collect();

    // The model by name, because a rung is asked of one model and what it buys
    // differs between them. It is also the fact that stops this reading as a
    // setting of crucible's — the ladder is what is sent, and this is who to.
    let title = format!("Effort · {}", runner.model());

    let ladder = Ladder {
        title: &title,
        rungs: &words,
        chosen: rung(runner.effort().unwrap_or(OPENS_ON), served),
        ends: ENDS,
        footer: FOOTER,
    };

    Ok(picking::adjust(renderer, terms.style, ladder)?.of(served))
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
    served: &[Effort],
) -> Result<(), Fatal> {
    // The word came off the line and was never shape-checked — anything at all
    // can follow `/effort ` — so it goes out the way arrived text goes out.
    renderer.commit(&format!("! {refused}"))?;

    listing(renderer, terms, served)
}

/// What is being asked now, and the lines that would ask for something else.
fn listed<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
    served: &[Effort],
) -> Result<(), Fatal> {
    let saying = match runner.effort() {
        Some(effort) => effort.as_str(),
        // Not a rung, because none was asked for. Naming one here would be this
        // program guessing at a vendor's default and printing the guess.
        None => "the vendor's own default",
    };
    let row = Row::new().then(Slot::Plain, clip(saying, renderer.columns()));

    renderer.present(&[row], terms.style.palette())?;

    listing(renderer, terms, served)
}

/// The rungs the model serves, as the line that asks for each one.
///
/// The same set the ladder would have stood over. A pipe gets the panel's
/// answer written out, and two answers to one question that differ by which
/// surface asked it are two answers.
fn listing<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    served: &[Effort],
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let rows: Vec<Row> = served
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
/// The weakest rung where it stands on none. A session already asking for a
/// rung its model does not serve is one the mark has nowhere true to stand, and
/// the end of the track is a better place to be put than the middle: it is
/// visibly not where the session was, so the first arrow key reads as a move
/// rather than as the panel having agreed with what was already in force.
fn rung(effort: Effort, served: &[Effort]) -> usize {
    served.iter().position(|one| *one == effort).unwrap_or(0)
}
