//! `/model`: which model answers, taken off a panel or named on the line.
//!
//! The name is the whole of what this command carries, and unlike a key it is
//! meant to be seen — so the line and the panel are two ways to the same place
//! rather than two halves of one thing. Naming it is what a script does and what
//! somebody who already knows the spelling does; the panel is for the rest,
//! which is most first runs, because a model name is a string a vendor chose and
//! there is no guessing it.
//!
//! The shelf holds every provider beside the models it serves, with a line to
//! narrow both by and the rungs the marked model takes on a strip beneath. A
//! key says a provider can be reached; it does not choose one. Taking a row is
//! the explicit point where the provider, the model and the rung change
//! together — one stop rather than three, because a rung is asked of a model
//! and picking the model first only to be sent elsewhere for the rung is the
//! same question asked twice.
//!
//! What is taken is written down, because the answer to "which model" is the
//! same answer every time this directory is opened and asking it once a session
//! is asking it for ever.

use crucible_core::Effort;
use crucible_runner::Runner;
use crucible_tui::{
    Editor, Glyphs, Offered, Pane, Panel, Renderer, Row, Serving, Shelf, Slot, Stocked, Terminal,
    clip, fold,
};

use crate::cli::choice::Choice;
use crate::cli::converse::picking::{self, Shelved, Standing, Taken};
use crate::cli::{Fatal, Model, NO_MODEL_CHOSEN, Served, offered, remember, served};

use super::{Terms, about, say};

mod narrowing;

/// What escape leaves behind, in place of the listing it used to write.
const LEFT: &str = "cancelled, no model taken";

/// The row at the top of the pane of providers: every one of them at once.
const ALL: &str = "All";

/// What the search line says with nothing typed into it.
///
/// Both halves named, because the line reads all four names a row has and
/// nothing on screen says which one a match came off. Somebody who only knew it
/// searched models would never try a vendor's name in it.
const HINT: &str = "a model, or a vendor";

/// What the pane of models says where the query left nothing on it.
///
/// The way out is named beside the fact, because an empty pane under a line
/// with words in it is the one place here where a reader can be stuck without
/// knowing which key gets them out. Built from the glyph set for the dash: a
/// terminal without one draws a hollow square in the middle of the sentence.
fn nothing(glyphs: Glyphs) -> String {
    format!("nothing matches {} backspace to widen it", glyphs.dash())
}

/// What the row of a model whose provider serves no rung says at its end.
const NO_RUNG: &str = "no rung";

/// What the strip says where the marked model serves no rung.
///
/// Whose doing it is, and not the shelf's: a rung is offered by whoever serves
/// the model, so a strip that only said *none* would read as something this
/// panel had decided.
fn serves_none(glyphs: Glyphs) -> String {
    format!("no rung {} its vendor serves none", glyphs.dash())
}

/// What the strip says while a turn runs.
///
/// A rung is what the running turn was started under, so there is nothing here
/// to change about it — the same answer `/effort` itself gives mid-turn, said
/// where somebody is looking for the strip rather than where they typed the
/// other command.
const HELD: &str = "set by /effort between turns";

/// What the title says on its right where no model has been chosen yet.
const NOTHING_ASKED: &str = "nothing asked yet";

/// What the strip under the panes offers, and which rung is on it now.
///
/// Two answers rather than one because mid-turn there is a third thing to say.
/// `/effort` is refused while a turn runs and a pick made over one is held
/// rather than applied, so the strip is drawn empty with the reason on it —
/// offering a rung this command could not then apply would be the panel
/// promising something the loop underneath it refuses.
#[derive(Clone, Copy)]
enum Track {
    /// Rungs may be taken, and this is the one in force.
    Offered(Option<Effort>),
    /// None may be taken here.
    Refused,
}

#[derive(Clone, Copy)]
pub(super) struct Selected {
    provider: Served,
    model: Model,
}

impl Selected {
    /// The provider and the model's name, which is what a held pick is stored
    /// as.
    pub(super) fn parts(&self) -> (Served, String) {
        (self.provider, self.model.name.to_owned())
    }
}

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
    if !said.is_empty() {
        return named(said, renderer, runner, terms);
    }

    if keys {
        let track = Track::Offered(runner.effort());
        match stood(renderer, terms, runner.model(), track, &mut |_| Ok(()))? {
            Shelved::Took(selected, rung) => {
                return applied(selected, rung, renderer, runner, terms);
            }
            // Escape asked for the screen that was there before the shelf. A
            // listing under it would be the same question put a second time.
            Shelved::Left => return say(renderer, LEFT),
            Shelved::Cramped => {}
        }
    }

    listed(renderer, runner, terms)
}

/// The shelf, stood while a turn runs behind it.
///
/// The runner is on the worker, so which model is in force is handed in by name
/// rather than read off it, and the drain is run once a pass so the turn goes
/// on rendering under the shelf. What comes back is the pick, not applied — the
/// runner cannot be reached mid-turn, so it is held for the turn the loop
/// starts next, and the strip of rungs is drawn empty for the same reason.
pub(super) fn picked_while<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    current: &str,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Taken<Selected>, Fatal> {
    Ok(
        match stood(renderer, terms, current, Track::Refused, while_waiting)? {
            Shelved::Took(selected, _) => Taken::Took(selected),
            Shelved::Left => Taken::Left,
            Shelved::Cramped => Taken::Cramped,
        },
    )
}

/// Whether a switch is confirmed, with the consequence said first.
///
/// The pick is held for the next turn rather than applied now, and the next
/// turn is the one that re-reads the transcript against the new model — so that
/// is said, and agreed to, before anything is held. `false` where the reader
/// goes back to the picker instead.
pub(super) fn confirmed<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    selected: Selected,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<bool, Fatal> {
    // The display name, not the slug: a person reads "Fable 5", and the
    // provider/model spelling is what `--model` and the config are for.
    let name = selected.model.shown;
    let says = format!(
        "This session is cached for the current model. Switching to {name} means the full transcript gets re-read on your next message."
    );
    let switch = format!("switch to {name}");
    let rows = [
        Offered {
            name: "Yes",
            says: &switch,
        },
        Offered {
            name: "No",
            says: "go back",
        },
    ];

    let panel = Panel {
        title: "Switch model?",
        said: Some(&says),
        shown: &rows,
        chosen: 0,
        footer: "esc to go back",
    };

    Ok(matches!(
        picking::pick_while(renderer, terms.style(), panel, while_waiting)?,
        picking::Picked::Took(0)
    ))
}

/// The model named on the line, with its provider named, settled from the
/// session, or found as the one provider in the catalog that serves it.
fn named<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let Some(choice) = Choice::parse(said) else {
        return say(renderer, "! a model cannot have an empty provider");
    };
    let Some(model) = choice.model else {
        return say(renderer, "! no model was named after the provider");
    };
    let providers = terms.providers.snapshot();
    let provider = if let Some(provider) = choice.provider {
        match served(&providers, &provider) {
            Ok(provider) => provider,
            Err(problem) => return say(renderer, &format!("! {problem}")),
        }
    } else if let Some(provider) = terms.provider.get() {
        match served(&providers, provider) {
            Ok(provider) => provider,
            Err(problem) => return say(renderer, &format!("! {problem}")),
        }
    } else {
        let mut matching = offered(&providers).filter(|provider| {
            provider
                .models
                .iter()
                .any(|offered| offered.name == model.as_ref())
        });
        let Some(provider) = matching.next() else {
            return say(
                renderer,
                "! use provider/model for a model outside the picker",
            );
        };
        if matching.next().is_some() {
            return say(
                renderer,
                "! more than one provider serves that name; use provider/model",
            );
        }
        provider
    };

    // Dropped for the same reason `apply` drops it: `/model provider/name`
    // names one thing and takes it or says why not, and there is no second half
    // waiting behind this one.
    taken(provider, (&model, None), renderer, runner, terms).map(drop)
}

/// The keys, under the panes they work on, long and short.
///
/// Built rather than written down, because the four arrows in it are the
/// setting's: a terminal without them draws hollow squares on the one row that
/// exists to be read by somebody who does not yet know. The short form is what
/// a window with no room for the long one gets — the same keys, without the
/// words saying what each of them moves.
fn keys(glyphs: Glyphs) -> (String, String) {
    let (up, down) = glyphs.walking();
    let (left, right) = glyphs.stepping();
    let dot = glyphs.dot();

    (
        format!(
            "tab pane {dot} {up}{down} model {dot} {left}{right} effort {dot} enter takes both {dot} esc to cancel"
        ),
        format!("tab {dot} {up}{down} {dot} {left}{right} {dot} enter {dot} esc"),
    )
}

/// Stands the shelf over the whole window, and says what came off it.
///
/// One loop for both ways in. Between turns the runner is this side's and the
/// track carries the rung in force; mid-turn it is on the worker, the model in
/// force is handed in by name, and the track carries nothing at all.
///
/// The shelf is narrowed inside the frame rather than before it, because what
/// it holds is decided by what has been typed and by which provider the mark
/// stands on, and both of those change under the keys. So the frame that
/// narrows is the frame that writes down what the keys will walk next — marks
/// included, since a query that emptied the shelf under one leaves it standing
/// past the end.
fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    current: &str,
    track: Track,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Shelved<Selected>, Fatal> {
    let providers = terms.providers.snapshot();
    let all = narrowing::every(&providers);
    let glyphs = terms.style().glyphs();
    let (long, short) = keys(glyphs);

    // Which model is in force goes on the title row rather than beside an
    // entry: it is one fact about the session, and a pane whose rows all read
    // the same way is one that can be walked without reading each of them.
    // Labelled, because a slug on its own at the far end of the title row is a
    // name with nothing saying what it is the name of. The rung rides with it:
    // both are what the next turn would be asked under, and the shelf below
    // offers to change either.
    let asked = match current {
        "" => NOTHING_ASKED.to_owned(),
        name => {
            let slug = format!("{}/{name}", terms.provider.get().unwrap_or("unselected"));
            match track {
                Track::Offered(Some(effort)) => {
                    format!("{slug} {} {}", glyphs.dot(), effort.as_str())
                }
                _ => slug,
            }
        }
    };
    let now = format!("now  {asked}");
    let nothing = nothing(glyphs);
    let norung = match track {
        Track::Offered(_) => serves_none(glyphs),
        Track::Refused => HELD.to_owned(),
    };

    // Opened on the one in force, so the first key moves off a known place
    // rather than towards one. A model chosen elsewhere is on no row here, and
    // the title above is where it is named.
    let at = all
        .iter()
        .position(|one| {
            Some(one.provider.name) == terms.provider.get() && one.model.name == current
        })
        .unwrap_or(0);
    let rung = match track {
        Track::Offered(Some(effort)) => all
            .get(at)
            .and_then(|one| one.model.rungs.iter().position(|one| *one == effort))
            .unwrap_or(0),
        _ => 0,
    };

    let mut standing = Standing {
        query: Editor::new(),
        // Opened on the models, which is what somebody typing `/model` came
        // for. The pane beside them is how the shelf is narrowed rather than
        // what is taken off it, and tab is what says so.
        pane: Pane::Models,
        provider: 0,
        model: at,
        rung,
        models: all.clone(),
        providers: 0,
        rungs: 0,
        pointer: None,
        lit: None,
    };

    picking::shelve(
        renderer,
        terms.style(),
        &mut standing,
        |standing, columns, room| {
            let counts = narrowing::counted(&providers, &all, standing.query.text());
            let only = standing
                .provider
                .checked_sub(1)
                .and_then(|at| counts.get(at))
                .map(|(provider, _)| provider.name);

            standing.models = narrowing::shelved(&all, standing.query.text(), only);
            standing.providers = counts.len() + 1;
            standing.provider = standing.provider.min(counts.len());
            standing.model = standing.model.min(standing.models.len().saturating_sub(1));

            let rungs: Vec<&str> = match track {
                Track::Refused => Vec::new(),
                Track::Offered(_) => standing
                    .models
                    .get(standing.model)
                    .map(|one| one.model.rungs.iter().map(|rung| rung.as_str()).collect())
                    .unwrap_or_default(),
            };
            standing.rungs = rungs.len();
            standing.rung = standing.rung.min(rungs.len().saturating_sub(1));

            let total: usize = counts.iter().filter_map(|(_, count)| *count).sum();
            let serving: Vec<Serving<'_>> = std::iter::once(Serving {
                name: ALL,
                count: (total > 0).then_some(total),
            })
            .chain(counts.iter().map(|(provider, count)| Serving {
                name: provider.shown,
                count: *count,
            }))
            .collect();

            let windows: Vec<String> = standing
                .models
                .iter()
                .map(|one| {
                    let window = crate::cli::startup::window(
                        &providers,
                        one.provider,
                        one.model.name,
                        &terms.settings,
                    );
                    crate::cli::draw::tokens(u64::from(window))
                })
                .collect();

            let stocked: Vec<Stocked<'_>> = standing
                .models
                .iter()
                .zip(&windows)
                .map(|(one, window)| Stocked {
                    name: one.model.shown,
                    // Who serves it, until the shelf is one provider's — at
                    // which point the pane beside it is already saying so, once
                    // rather than on every row.
                    by: if only.is_none() {
                        one.provider.shown
                    } else {
                        ""
                    },
                    window,
                    note: if one.model.rungs.is_empty() {
                        NO_RUNG
                    } else {
                        ""
                    },
                    now: Some(one.provider.name) == terms.provider.get()
                        && one.model.name == current,
                })
                .collect();

            let shelf = Shelf {
                title: "Model",
                now: &now,
                query: standing.query.text(),
                typed: standing.query.column(),
                hint: HINT,
                providers: &serving,
                provider: standing.provider,
                models: &stocked,
                held: all.len(),
                model: standing.model,
                rungs: &rungs,
                rung: standing.rung,
                nothing: &nothing,
                pane: standing.pane,
                keys: (&long, &short),
                norung: &norung,
                pointer: standing.pointer,
            };

            let rows = shelf.within(columns, room, glyphs);
            let caret = shelf.caret(columns, glyphs);
            // Read off the shelf that was just drawn rather than worked out
            // again when a click arrives: what the pointer is over is a fact
            // about a picture, and this is the moment there is one.
            standing.lit = shelf.resting(columns, room);

            (rows, Some(caret))
        },
        while_waiting,
    )
}

/// Asks for the model, and then for the rung marked under it.
///
/// In that order, and each through the command that already owns it: the model
/// here, the rung through `/effort`'s own path. A rung taken on this shelf is
/// then written down and said back exactly as one taken there, which is what
/// keeps two ways to one answer from being two answers.
///
/// A model whose provider serves no rung is taken with the rung left exactly as
/// it was. That is not a failure and says nothing on screen beyond the `no rung`
/// its row already carried.
fn applied<T: Terminal>(
    selected: Selected,
    rung: Option<usize>,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let effort = rung.and_then(|at| selected.model.rungs.get(at).copied());
    // A rung is asked of a model, so a model that was refused has no rung to
    // ask for. Going on would reach `/effort`, which finds the session still
    // without a provider and says it has no model at all -- a second warning,
    // about a second missing thing, under the one that named the real one.
    if !taken(
        selected.provider,
        (selected.model.name, effort),
        renderer,
        runner,
        terms,
    )? {
        return Ok(());
    }

    let Some(effort) = effort else {
        return Ok(());
    };

    // Through `/effort` itself rather than through a copy of its two lines.
    // With no keyboard asked for, because the rung is already chosen: what it
    // does with a word is take it, write it down and say so, which is the whole
    // of what is owed here.
    super::effort::run(effort.as_str(), renderer, runner, terms, false)
}

/// Asks it from the next turn on, and writes it down for the next run.
///
/// A row off another provider's half of the panel moves the session there
/// first: a model belongs to the vendor that serves it, and a name written
/// under the wrong one is the mismatch this command exists to stop.
///
/// A failure to write does not undo the switch. What is lost is the part that
/// outlives the process, and the line drawn says so.
/// Applies a pick held from mid-turn, when the runner is this side's again.
///
/// The same applying as `taken`, named for the caller that has a provider and a
/// model rather than a row off the panel: a pick made while the runner was on
/// the worker is applied at the next turn's start through here.
pub(super) fn apply<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    selected: Served,
    name: &str,
) -> Result<(), Fatal> {
    // The answer is dropped rather than passed on: there is no rung behind this
    // caller to stop, and the line saying what went wrong has already been
    // drawn by the time it comes back.
    taken(selected, (name, None), renderer, runner, terms).map(drop)
}

/// Whether the model is the one the next turn will be asked for.
///
/// `false` is a provider that could not be reached, said in one line and
/// nothing applied. It is not an error to the caller -- the reader has been
/// told, and the session is exactly where it was -- but it is the difference
/// between a model taken and a model refused, and only the caller knows what it
/// was about to do next.
/// The model and optional explicit rung travel together; an absent rung keeps
/// the effort already selected by the session.
fn taken<T: Terminal>(
    selected: Served,
    (name, effort): (&str, Option<Effort>),
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<bool, Fatal> {
    let provider = selected.name;
    // Validate before retiring a cache or replacing the provider. The picker
    // may supply a compatible rung together with the model; a typed model name
    // cannot silently carry xhigh/max into Gemini's narrower ladder.
    let catalogue = terms.providers.snapshot();
    if provider == "google"
        && let Some(effort) = effort.or(runner.effort())
        && !crate::cli::rungs(&catalogue, provider, name).contains(&effort)
    {
        say(
            renderer,
            &format!(
                "! {name} does not support {} effort; choose a supported rung in /model or change /effort before switching",
                effort.as_str()
            ),
        )?;
        return Ok(false);
    }
    let provider_changed = terms.provider.get() != Some(provider);
    if provider_changed {
        let set = match (terms.serving)(selected, &terms.logins.read()) {
            Ok(set) => set,
            Err(problem) => return refused(renderer, &problem).map(|()| false),
        };
        if !super::cache::retire(renderer, runner)? {
            return Ok(false);
        }
        runner.serve(set.provider);
        terms.provider.set(Some(provider));
    } else if runner.model() != name && !super::cache::retire(renderer, runner)? {
        return Ok(false);
    }
    // One generation, read once: the ceiling, the window and what the model
    // reads are three answers about the same model, and three separate reads
    // could take them from three different generations of the registry.
    runner.ask(
        name,
        crate::cli::startup::ceiling(&catalogue, provider, name),
        Some(crate::cli::startup::window(
            &catalogue,
            selected,
            name,
            &terms.settings,
        )),
        crate::cli::startup::accepts(&catalogue, provider, name),
    );

    // The word may have come off the line and was never shape-checked — anything
    // at all can follow `/model ` — so it goes out the way arrived text goes out.
    renderer.commit(&format!("{provider}/{name}"))?;

    // Both halves written, and the row above already says what to. Where they
    // went is not news: it is the same file every time, chosen by crucible
    // rather than by the reader, and naming it on every model is a session
    // reading its own bookkeeping out loud.
    //
    // The provider first, because it is the half a machine holding two keys
    // needs — a model written under a provider says what to ask that provider
    // for and never which provider to ask, so writing only that would leave the
    // next run here asking the same question this command just answered.
    let written = remember::asking(&terms.choosing, provider)
        .and_then(|()| remember::choosing(&terms.choosing, provider, name));
    let Err(problem) = written else {
        return Ok(true);
    };

    renderer.commit(&format!("! {problem}"))?;

    // Wrapped rather than clipped: short as this row is, a narrow enough window
    // would still cut it, and half of it says nothing about what was lost.
    let rows: Vec<Row> = fold("asked for this session only", renderer.columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Quiet, row))
        .collect();

    renderer.present(&rows)?;
    Ok(true)
}

/// Says why nothing was taken, in the one colour this program keeps for that.
///
/// Louder than the quiet line a command answers with, because the two say
/// opposite things: a quiet line is a command that did what was asked, and this
/// is one that did not. Wrapped rather than clipped -- a provider's name and the
/// two ways out of this are the whole sentence, and half of it is advice to
/// nowhere.
fn refused<T: Terminal>(renderer: &mut Renderer<T>, problem: &Fatal) -> Result<(), Fatal> {
    let rows: Vec<Row> = fold(&format!("! {problem}"), renderer.columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Trouble, row))
        .collect();

    Ok(renderer.present(&rows)?)
}

/// What is being asked now, and the lines that would ask for something else.
fn listed<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Read out of a configuration file or off the command line either way, so
    // it goes out the way arrived text goes out.
    match runner.model() {
        "" => renderer.commit(NO_MODEL_CHOSEN)?,
        name => renderer.commit(&format!(
            "{}/{name}",
            terms.provider.get().unwrap_or("unselected")
        ))?,
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = offered(&terms.providers.snapshot())
        .flat_map(|provider| provider.models.iter().map(move |model| (provider, model)))
        .map(|(provider, model)| {
            let named = if model.shown == model.name {
                format!("/model {}/{}", provider.name, model.name)
            } else {
                about(
                    &format!("/model {}/{}", provider.name, model.name),
                    model.shown,
                    terms.style().glyphs(),
                )
            };
            Row::new().then(Slot::Quiet, clip(&named, columns))
        })
        .collect();

    Ok(renderer.present(&rows)?)
}

#[cfg(test)]
mod tests {
    use crucible_core::AgentId;
    use crucible_runner::{AgentSpec, Model as RunnerModel, Session, Tools};
    use crucible_tui::{Glyphs, Recording, Renderer};

    use crate::cli::converse::tests::plain;
    use crate::cli::fake::Script;
    use crate::cli::sample::Sample;

    use crate::cli::Providers;

    use super::{Effort, Selected, applied, keys, offered, taken};

    /// The built-in providers, as one generation the rows are read off.
    fn catalogue() -> Providers {
        crate::cli::providers()
            .expect("the built-in providers register")
            .snapshot()
    }

    #[test]
    fn the_keys_under_the_panes_come_out_of_the_glyph_set() {
        // The row naming the keys is the whole of what teaches somebody
        // standing at the shelf how to walk it and how to leave it. A terminal
        // without the arrows draws four hollow squares on the one row that
        // exists to be read by somebody who does not yet know.
        assert_eq!(
            keys(Glyphs::Unicode),
            (
                "tab pane \u{b7} \u{2191}\u{2193} model \u{b7} \u{2190}\u{2192} effort \u{b7} enter takes both \u{b7} esc to cancel"
                    .to_owned(),
                "tab \u{b7} \u{2191}\u{2193} \u{b7} \u{2190}\u{2192} \u{b7} enter \u{b7} esc".to_owned(),
            )
        );
        assert_eq!(
            keys(Glyphs::Ascii),
            (
                "tab pane - ^v model - <> effort - enter takes both - esc to cancel".to_owned(),
                "tab - ^v - <> - enter - esc".to_owned(),
            )
        );
    }

    /// A runner asking for `old` and nothing else, to take a row against.
    fn asking() -> crucible_runner::Runner {
        crucible_runner::Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            AgentSpec::new(
                AgentId::new("test"),
                RunnerModel {
                    name: "old".into(),
                    max_tokens: 17,
                    window: Some(99),
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_runner::ContextInputs::new(std::env::temp_dir()),
            Session::nowhere(),
        )
    }

    /// A runner with nothing to ask, as a run with no credential anywhere gets.
    fn unasked() -> crucible_runner::Runner {
        crucible_runner::Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            AgentSpec::new(
                AgentId::new("test"),
                RunnerModel {
                    name: "".into(),
                    max_tokens: 17,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_runner::ContextInputs::new(std::env::temp_dir()),
            Session::nowhere(),
        )
    }

    /// The row for one model of one provider, by both names.
    fn row(provider: &str, model: &str) -> Selected {
        let provider = offered(&catalogue())
            .find(|one| one.name == provider)
            .expect("a served provider");
        let model = provider
            .models
            .iter()
            .find(|one| one.name == model)
            .copied()
            .expect("a served model");

        Selected { provider, model }
    }

    #[test]
    fn a_row_whose_provider_cannot_be_reached_takes_nothing_and_says_it_once() {
        // One sentence, and the one that names what is actually missing. Going
        // on to the rung reaches `/effort`, which finds no provider set and
        // says the session has no model at all -- a second warning, about a
        // different missing thing, printed under the first and contradicting
        // the model still in force.
        // The machine the reader is on: no key for anything, so no provider was
        // resolved and no model was ever asked for.
        let terms = plain();
        terms.provider.set(None);
        let mut runner = unasked();
        let mut renderer = Renderer::new(Recording::new(80, 24));

        applied(
            row("moonshot", "k3"),
            Some(0),
            &mut renderer,
            &mut runner,
            &terms,
        )
        .expect("the row to be answered");

        let written = renderer.terminal().written().to_string();
        assert!(written.contains("! "), "{written}");
        assert!(!written.contains("No model selected"), "{written}");
        assert!(!written.contains("No models available"), "{written}");
        assert!(runner.model().is_empty(), "{}", runner.model());
    }

    #[test]
    fn taking_a_row_asks_for_the_model_and_then_the_rung_marked_under_it() {
        // Both halves, in that order. A rung is asked of a model, so a shelf
        // that applied the rung first would be asking it of the model being
        // left behind.
        let terms = plain();
        let mut runner = asking();
        let mut renderer = Renderer::new(Recording::new(80, 24));
        let selected = row("anthropic", "claude-sonnet-5");
        let at = selected
            .model
            .rungs
            .iter()
            .position(|rung| *rung == Effort::Xhigh)
            .expect("a model that serves xhigh");

        applied(selected, Some(at), &mut renderer, &mut runner, &terms)
            .expect("the row to be taken");

        assert_eq!(runner.model(), "claude-sonnet-5");
        assert_eq!(runner.effort(), Some(Effort::Xhigh));
    }

    #[test]
    fn google_model_switch_requires_an_explicit_compatible_effort() {
        let mut terms = plain();
        terms.serving = Box::new(|_, _| {
            Ok(crate::cli::Resolved {
                provider: Box::new(Script::new(Vec::new())),
                source: crate::cli::CredentialSource::StoredKey,
            })
        });
        let mut runner = asking();
        runner.think(Effort::Max);
        let mut renderer = Renderer::new(Recording::new(100, 24));
        let google = row("google", "gemini-3.8-flash");
        super::run(
            "google/gemini-3.8-flash",
            &mut renderer,
            &mut runner,
            &terms,
            false,
        )
        .unwrap();
        assert_eq!(
            runner.model(),
            "old",
            "an incompatible inherited rung must not silently cross providers"
        );
        assert_eq!(runner.effort(), Some(Effort::Max));
        assert_eq!(terms.provider.get(), Some("anthropic"));
        assert!(renderer.terminal().written().contains("effort"));
        applied(google, Some(2), &mut renderer, &mut runner, &terms).unwrap();
        assert_eq!(runner.model(), "gemini-3.8-flash");
        assert_eq!(runner.effort(), Some(Effort::High));
        assert_eq!(terms.provider.get(), Some("google"));
        super::super::effort::run("xhigh", &mut renderer, &mut runner, &terms, false).unwrap();
        assert_eq!(
            runner.effort(),
            Some(Effort::High),
            "an unsupported typed rung must leave the selected rung unchanged"
        );
    }

    #[test]
    fn taking_a_model_that_serves_no_rung_leaves_the_rung_exactly_as_it_was() {
        // Not an error and nothing said about it. The row carried `no rung`
        // and the strip carried the same sentence, so a session that took it
        // has already been told.
        let terms = plain();
        let mut runner = asking();
        runner.think(Effort::High);
        let mut renderer = Renderer::new(Recording::new(80, 24));
        let selected = row("anthropic", "claude-haiku-4-5");
        assert!(selected.model.rungs.is_empty());

        applied(selected, None, &mut renderer, &mut runner, &terms).expect("the row to be taken");

        assert_eq!(runner.model(), "claude-haiku-4-5");
        assert_eq!(runner.effort(), Some(Effort::High));
    }

    #[test]
    fn taking_a_model_replaces_name_output_and_startup_resolved_window_together() {
        let sample = Sample::new("model-runtime-limits");
        let mut terms = plain();
        terms.settings = sample.settings(
            r#"{"providers":{"anthropic":{"contextWindow":{"claude-haiku-4-5":345678}}}}"#,
        );
        let mut runner = crucible_runner::Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            AgentSpec::new(
                AgentId::new("test"),
                RunnerModel {
                    name: "old".into(),
                    max_tokens: 17,
                    window: Some(99),
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_runner::ContextInputs::new(std::env::temp_dir()),
            Session::nowhere(),
        );
        let mut renderer = Renderer::new(Recording::new(80, 24));
        let anthropic = offered(&catalogue())
            .find(|provider| provider.name == "anthropic")
            .expect("anthropic is served");

        taken(
            anthropic,
            ("claude-haiku-4-5", None),
            &mut renderer,
            &mut runner,
            &terms,
        )
        .expect("the model to be taken");

        assert_eq!(runner.model(), "claude-haiku-4-5");
        assert_eq!(runner.maximum_output(), 16_000);
        assert_eq!(runner.context_window(), Some(345_678));
    }
}
