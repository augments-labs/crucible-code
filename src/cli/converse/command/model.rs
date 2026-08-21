//! `/model`: which model answers, taken off a panel or named on the line.
//!
//! The name is the whole of what this command carries, and unlike a key it is
//! meant to be seen — so the line and the panel are two ways to the same place
//! rather than two halves of one thing. Naming it is what a script does and what
//! somebody who already knows the spelling does; the panel is for the rest,
//! which is most first runs, because a model name is a string a vendor chose and
//! there is no guessing it.
//!
//! The panel holds every provider beside the models it serves. A key says a
//! provider can be reached; it does not choose one. Taking a row is the
//! explicit point where the provider and model change together.
//!
//! What is taken is written down, because the answer to "which model" is the
//! same answer every time this directory is opened and asking it once a session
//! is asking it for ever.

use crucible_runner::Runner;
use crucible_tui::{Glyphs, Offered, Panel, Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::choice::Choice;
use crate::cli::converse::picking::{self, Taken};
use crate::cli::{Fatal, Model, NO_MODEL_CHOSEN, PROVIDERS, Served, remember, served};

use super::{Terms, about, say};

/// The sentence under the panel's title: what standing there cannot show, which
/// is that this outlives the session it was chosen in.
const SAID: &str = concat!(
    "Choose the provider and model crucible asks from the next turn on. The ",
    "choice is written down for every run from now on; effort stays unset until chosen."
);

/// What escape leaves behind, in place of the listing it used to write.
const LEFT: &str = "cancelled, no model taken";

#[derive(Clone, Copy)]
struct Selected {
    provider: Served,
    model: Model,
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
        match chosen(renderer, runner, terms)? {
            Taken::Took(selected) => {
                return taken(
                    selected.provider,
                    selected.model.name,
                    renderer,
                    runner,
                    terms,
                );
            }
            // Escape asked for the screen that was there before the panel. A
            // listing under it would be the same question put a second time.
            Taken::Left => return say(renderer, LEFT),
            Taken::Cramped => {}
        }
    }

    listed(renderer, runner, terms)
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
    let provider = if let Some(provider) = choice.provider {
        match served(&provider) {
            Ok(provider) => provider,
            Err(problem) => return say(renderer, &format!("! {problem}")),
        }
    } else if let Some(provider) = terms.provider.get() {
        match served(provider) {
            Ok(provider) => provider,
            Err(problem) => return say(renderer, &format!("! {problem}")),
        }
    } else {
        let mut matching = PROVIDERS.into_iter().filter(|provider| {
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

    taken(provider, &model, renderer, runner, terms)
}

/// Stands the panel where the prompt box was, and says which model came off it.
///
/// A window with no room to stand one in comes out as the listing below — the
/// answer `/model` always had, and the only one a short window can be given.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    terms: &Terms,
) -> Result<Taken<Selected>, Fatal> {
    let offered: Vec<_> = PROVIDERS
        .into_iter()
        .flat_map(|provider| {
            provider
                .models
                .iter()
                .copied()
                .map(move |model| Selected { provider, model })
        })
        .collect();

    // Which model is in force goes here rather than beside an entry: it is one
    // fact about the session, and a list that says it once is a list whose rows
    // all read the same way.
    let saying = match runner.model() {
        "" => format!("Nothing is being asked yet. {SAID}"),
        name => format!(
            "{}/{name} is what is asked now. {SAID}",
            terms.provider.get().unwrap_or("unselected")
        ),
    };

    let says: Vec<String> = offered
        .iter()
        .map(|selected| beside(*selected, terms.style().glyphs()))
        .collect();

    let shown: Vec<Offered<'_>> = offered
        .iter()
        .zip(&says)
        .map(|(selected, says)| Offered {
            name: selected.model.shown,
            says,
        })
        .collect();

    let panel = Panel {
        title: "Model",
        said: Some(&saying),
        shown: &shown,
        // Opened on the one in force, so the first key moves off a known place
        // rather than towards one. A model chosen elsewhere is on no row here,
        // and the sentence above is where it is named.
        chosen: offered
            .iter()
            .position(|selected| {
                Some(selected.provider.name) == terms.provider.get()
                    && selected.model.name == runner.model()
            })
            .unwrap_or(0),
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    Ok(picking::pick(renderer, terms.style(), panel)?.of(&offered))
}

/// What a row says beside the model's name: who serves it, and the other way
/// to the same model.
///
/// The one thing that differs between rows that would otherwise be a name and
/// nothing else. The mark between the two halves is the setting's, since a
/// terminal that cannot draw it would otherwise be given a question mark in the
/// middle of a line somebody is reading a flag off.
fn beside(selected: Selected, glyphs: Glyphs) -> String {
    format!(
        "{} {} --model {}/{}",
        selected.provider.shown,
        glyphs.dot(),
        selected.provider.name,
        selected.model.name
    )
}

/// Asks it from the next turn on, and writes it down for the next run.
///
/// A row off another provider's half of the panel moves the session there
/// first: a model belongs to the vendor that serves it, and a name written
/// under the wrong one is the mismatch this command exists to stop.
///
/// A failure to write does not undo the switch. What is lost is the part that
/// outlives the process, and the line drawn says so.
fn taken<T: Terminal>(
    selected: Served,
    name: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let provider = selected.name;
    if terms.provider.get() != Some(provider) {
        let set = match (terms.serving)(selected, &terms.logins.read()) {
            Ok(set) => set,
            Err(problem) => return say(renderer, &format!("! {problem}")),
        };
        runner.serve(set.provider);
        terms.provider.set(Some(provider));
    }
    runner.ask(
        name,
        crate::cli::startup::ceiling(provider, name),
        crate::cli::startup::window(provider, name, &terms.settings),
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
        return Ok(());
    };

    renderer.commit(&format!("! {problem}"))?;

    // Wrapped rather than clipped: short as this row is, a narrow enough window
    // would still cut it, and half of it says nothing about what was lost.
    let rows: Vec<Row> = fold("asked for this session only", renderer.columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Quiet, row))
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
    let rows: Vec<Row> = PROVIDERS
        .into_iter()
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
    use crucible_runner::{Model as RunnerModel, Session, Tools};
    use crucible_tui::{Glyphs, Recording, Renderer};

    use crate::cli::converse::tests::plain;
    use crate::cli::fake::Script;
    use crate::cli::sample::Sample;

    use super::{PROVIDERS, Selected, beside, taken};

    #[test]
    fn what_stands_between_the_vendor_and_the_flag_comes_out_of_the_glyph_set() {
        // The row names who serves the model and the flag that asks for it
        // without the panel, and what says they are two is the mark between
        // them. A terminal that cannot draw that mark gets the one the setting
        // names rather than a question mark in the middle of a flag.
        let provider = PROVIDERS.into_iter().next().expect("a served provider");
        let model = provider.models.first().copied().expect("a served model");
        let selected = Selected { provider, model };

        assert_eq!(
            beside(selected, Glyphs::Unicode),
            format!(
                "{} · --model {}/{}",
                provider.shown, provider.name, model.name
            )
        );
        assert_eq!(
            beside(selected, Glyphs::Ascii),
            format!(
                "{} - --model {}/{}",
                provider.shown, provider.name, model.name
            )
        );
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
            RunnerModel {
                name: "old".into(),
                max_tokens: 17,
                window: Some(99),
                system: None,
                effort: None,
            },
            Session::nowhere(),
        );
        let mut renderer = Renderer::new(Recording::new(80, 24));
        let anthropic = PROVIDERS
            .into_iter()
            .find(|provider| provider.name == "anthropic")
            .expect("anthropic is served");

        taken(
            anthropic,
            "claude-haiku-4-5",
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
