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

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::switching::Switched;
use crucible_client_api::{Command, Name};
use crucible_models::{Effort, FastForm};
use crucible_tui::{
    Editor, Glyphs, Offered, Pane, Panel, Renderer, Row, Serving, Shelf, Slot, Stocked, Terminal,
    fold, label,
};

use crate::cli::Fatal;
use crate::cli::choice::Choice;
use crate::cli::client::astray;
use crate::cli::converse::picking::{self, Shelved, Standing, Taken};
use crate::cli::converse::warning::{self, Chosen};
use std::collections::BTreeMap;

use crucible_app::providers::{
    InUse, Model, NO_MODEL_CHOSEN, Providers, Served, StoredCredentials, offered,
};
use crucible_app::startup::served;

use super::{Asked, Laid, Terms, about, relaid, say, say_at};

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

/// What the row of a model with a fast form says at its end.
const FAST: &str = "fast";

/// The fast form `model` has on the route `served` is served on: none at a
/// configured address, which is not the vendor's to answer for, and the
/// sign-in's where a stored sign-in serves the provider.
fn routed(served: Served, model: &str, based: bool, signed_in: bool) -> FastForm {
    if based {
        return FastForm::None;
    }
    match served.fast_signed_in.filter(|_| signed_in) {
        Some(signed) => signed(model),
        None => (served.fast)(model),
    }
}

/// The fast form a row's model has on its provider's route, as the settings
/// and the credential store say that route is.
fn row_form(
    served: Served,
    model: &str,
    settings: &crucible_config::Settings,
    stored: &crucible_app::providers::StoredCredentials,
) -> FastForm {
    routed(
        served,
        model,
        settings.base_url(served.name).is_some(),
        stored.has_subscription(served.name),
    )
}

/// The one note a model's row has room for: `no rung` before `fast`.
fn note(rungs: &[Effort], form: FastForm) -> &'static str {
    if rungs.is_empty() {
        NO_RUNG
    } else if form == FastForm::None {
        ""
    } else {
        FAST
    }
}

/// What a model that is itself warned says in the note column.
const TRAINS: &str = "trains";

/// The note of a row whose model is itself warned, `warned`: `trains` before
/// anything about its rungs or its speed.
fn noted(warned: bool, rungs: &[Effort], form: FastForm) -> &'static str {
    if warned { TRAINS } else { note(rungs, form) }
}

/// The models of `all` each provider's credential in use serves, from
/// `using`, the credential each provider is sent with.
fn narrowed(all: Vec<Selected>, using: &BTreeMap<&str, InUse>) -> Vec<Selected> {
    all.into_iter()
        .filter(|one| {
            using
                .get(one.provider.name)
                .and_then(|in_use| in_use.serves)
                .is_none_or(|serves| serves.contains(&one.model.name))
        })
        .collect()
}

/// What heads the pane narrowed to `provider`: the provider and the words of
/// the credential its models are served by, or nothing where it has none.
fn headed(provider: &str, using: &BTreeMap<&str, InUse>, glyphs: Glyphs) -> Option<String> {
    let dot = glyphs.dot();
    using
        .get(provider)
        .map(|in_use| format!("{provider} {dot} {}", in_use.words.replace('·', dot)))
}

/// The quiet row under the pane narrowed to `provider`: how many more of its
/// models another credential would serve, and where it is given.
fn closing(provider: Served, using: &BTreeMap<&str, InUse>, glyphs: Glyphs) -> Option<String> {
    let serves = using.get(provider.name)?.serves?;
    let more = provider
        .models
        .iter()
        .filter(|model| !serves.contains(&model.name))
        .count();
    (more > 0).then(|| format!("{more} more with an API key {} /login", glyphs.dot()))
}

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
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    if !said.is_empty() {
        return named(said, renderer, conversation, terms, keys);
    }

    if keys {
        let runner = conversation.runner();
        let track = Track::Offered(runner.effort());
        let current = Asked {
            provider: conversation.serving(),
            model: runner.model(),
            effort: runner.effort().map(Effort::as_str),
            pace: super::Pace::of(runner),
        };
        let mut on = None;
        loop {
            match stood(renderer, terms, (current, on), track, &mut |_| Ok(()))? {
                Shelved::Took(selected, rung) => {
                    let chose = (selected.provider.name, selected.model.name);
                    match warning::choosing(renderer, terms, chose, keys, &mut |_| Ok(()))? {
                        Chosen::Take => {
                            return applied(selected, rung, renderer, conversation, terms);
                        }
                        // Back to the shelf, with the mark where it was.
                        Chosen::Back => on = Some(chose),
                        Chosen::Stop(said) => return say(renderer, &said),
                    }
                }
                // Escape asked for the screen that was there before the shelf. A
                // listing under it would be the same question put a second time.
                Shelved::Left => return say(renderer, LEFT),
                Shelved::Cramped => break,
            }
        }
    }

    listed(renderer, conversation, terms)
}

/// The shelf, stood while a turn runs behind it.
///
/// The runner is on the worker, so who is answering and for which model are
/// handed in by name rather than read off it, and the drain is run once a pass so the turn goes
/// on rendering under the shelf. What comes back is the pick, not applied — the
/// runner cannot be reached mid-turn, so it is held for the turn the loop
/// starts next, and the strip of rungs is drawn empty for the same reason.
pub(super) fn picked_while<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    current: Asked<'_>,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Taken<Selected>, Fatal> {
    Ok(
        match stood(
            renderer,
            terms,
            (current, None),
            Track::Refused,
            while_waiting,
        )? {
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
        source: None,
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
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
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
    } else if let Some(provider) = conversation.serving() {
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

    // Words name one thing, so going back from the question is leaving it.
    match warning::choosing(renderer, terms, (provider.name, &model), keys, &mut |_| {
        Ok(())
    })? {
        Chosen::Take => {}
        Chosen::Back => return say(renderer, LEFT),
        Chosen::Stop(said) => return say(renderer, &said),
    }

    // Dropped for the same reason `apply` drops it: `/model provider/name`
    // names one thing and takes it or says why not, and there is no second half
    // waiting behind this one.
    taken(
        provider,
        (&model, None),
        (renderer, Laid::Hung),
        conversation,
        terms,
    )
    .map(drop)
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
    (current, on): (Asked<'_>, Option<(&str, &str)>),
    track: Track,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Shelved<Selected>, Fatal> {
    let providers = terms.providers.snapshot();
    let glyphs = terms.style().glyphs();
    let (long, short) = keys(glyphs);
    // Read once for the shelf, not once a frame: which providers a stored
    // sign-in serves decides the fast form a row's model has, and which of
    // its models each provider's credential in use serves decides its list.
    let stored = terms.logins.read();
    let using = in_use(terms, &providers, &stored);
    let all = narrowed(narrowing::every(&providers), &using);
    let routes = terms.consent.routes();

    // Which model is in force goes on the title row rather than beside an
    // entry: it is one fact about the session, and a pane whose rows all read
    // the same way is one that can be walked without reading each of them.
    // Labelled, because a model's label on its own at the far end of the title
    // row is a name with nothing saying what it is the name of. The rung rides with it:
    // both are what the next turn would be asked under, and the shelf below
    // offers to change either.
    let now = titled(current, glyphs);
    let nothing = nothing(glyphs);
    let norung = match track {
        Track::Offered(_) => serves_none(glyphs),
        Track::Refused => HELD.to_owned(),
    };

    // Opened on the one in force, so the first key moves off a known place
    // rather than towards one. A model chosen elsewhere is on no row here, and
    // the title above is where it is named.
    // Or on the one a question just went back from.
    let (provider, model) = on.map_or((current.provider, current.model), |(provider, model)| {
        (Some(provider), model)
    });
    let at = all
        .iter()
        .position(|one| Some(one.provider.name) == provider && one.model.name == model)
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
                    let window = crucible_app::startup::window(
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
                .map(|(one, window)| {
                    let now = Some(one.provider.name) == current.provider
                        && one.model.name == current.model;
                    // The model in force by the provider set up for it, which
                    // knows the credential; the rest by their provider's route.
                    let form = if now {
                        current.pace.form
                    } else {
                        row_form(one.provider, one.model.name, &terms.settings, &stored)
                    };
                    (one, window, now, form)
                })
                .map(|(one, window, now, form)| Stocked {
                    name: one.model.name,
                    // Who serves it, until the shelf is one provider's — at
                    // which point the pane beside it is already saying so, once
                    // rather than on every row.
                    by: if only.is_none() {
                        one.provider.shown
                    } else {
                        ""
                    },
                    window,
                    note: noted(
                        routes
                            .warned(&crucible_app::content_use::model_route(
                                one.provider.name,
                                one.model.name,
                            ))
                            .is_some(),
                        one.model.rungs,
                        form,
                    ),
                    now,
                })
                .collect();

            let heading = only.and_then(|name| headed(name, &using, glyphs));
            let closing = only
                .and_then(|name| served(&providers, name).ok())
                .and_then(|provider| closing(provider, &using, glyphs));
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
                heading: heading.as_deref(),
                closing: closing.as_deref(),
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
    conversation: &mut Conversation,
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
        (renderer, Laid::Hung),
        conversation,
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
    super::effort::run(effort.as_str(), renderer, conversation, terms, false)
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
/// the worker is applied through here as that turn ends. What it says stands
/// at the left edge, since nothing here is hung under a line that asked.
pub(super) fn apply<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    selected: Served,
    name: &str,
) -> Result<(), Fatal> {
    // The answer is dropped rather than passed on: there is no rung behind this
    // caller to stop, and the line saying what went wrong has already been
    // drawn by the time it comes back.
    taken(
        selected,
        (name, None),
        (renderer, Laid::Flush),
        conversation,
        terms,
    )
    .map(drop)
}

/// Whether the model is the one the next turn will be asked for.
///
/// `false` is a provider that could not be reached, said in one line and
/// nothing applied. It is not an error to the caller -- the reader has been
/// told, and the session is exactly where it was -- but it is the difference
/// between a model taken and a model refused, and only the caller knows what it
/// was about to do next.
/// The model and optional explicit rung travel together; an absent rung keeps
/// the effort already selected by the session. The renderer travels with how
/// its rows are laid: hung under the line that asked, or at the left edge.
fn taken<T: Terminal>(
    selected: Served,
    (name, effort): (&str, Option<Effort>),
    (renderer, laid): (&mut Renderer<T>, Laid),
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<bool, Fatal> {
    let provider = selected.name;
    // What is checked, reached, retired and written, and in which order, is
    // the conversation's. What is here is what each way it can end is said as.
    let asked = match (Name::new(provider), Name::new(name)) {
        (Ok(provider), Ok(model)) => Command::SelectModel {
            provider,
            model,
            effort: effort.map(crucible_app::client::rung),
        },
        // A word no front end may name a model by: empty, or longer than any
        // vendor's. Said rather than sent, and nothing is applied.
        (Err(refusal), _) | (_, Err(refusal)) => {
            return say_at(renderer, laid, &format!("! {refusal}")).map(|()| false);
        }
    };
    let switched = match terms.perform(conversation, asked) {
        Performed::Model(switched) => switched,
        other => return say_at(renderer, laid, &astray(&other)).map(|()| false),
    };
    let unwritten = match switched {
        // The picker may supply a compatible rung together with the model; a
        // typed model name cannot silently carry xhigh/max into Gemini's
        // narrower ladder.
        Switched::Unsupported(effort) => {
            say_at(
                renderer,
                laid,
                &format!(
                    "! {name} does not support {} effort; choose a supported rung in /model or change /effort before switching",
                    effort.as_str()
                ),
            )?;
            return Ok(false);
        }
        Switched::Unreachable(problem) => return refused(renderer, &problem).map(|()| false),
        Switched::CacheHeld(problem) => {
            return super::cache::held(renderer, laid, &problem).map(|()| false);
        }
        Switched::Taken {
            retained,
            unwritten,
        } => {
            super::cache::retained(renderer, laid, retained)?;
            unwritten
        }
    };

    // The word may have come off the line and was never shape-checked — anything
    // at all can follow `/model ` — so it goes out the way arrived text goes out.
    // The rung asked for with the model where the shelf marked one, which is
    // put on the runner just after this; otherwise the one kept across it.
    renderer.commit(&answered(
        provider,
        name,
        effort.or(conversation.runner().effort()),
        terms.style().glyphs(),
    ))?;

    // Both halves written, and the row above already says what to. Where they
    // went is not news: it is the same file every time, chosen by crucible
    // rather than by the reader, and naming it on every model is a session
    // reading its own bookkeeping out loud.
    let Some(problem) = unwritten else {
        return Ok(true);
    };

    renderer.commit(&format!("! {problem}"))?;

    // Wrapped rather than clipped: short as this row is, a narrow enough window
    // would still cut it, and half of it says nothing about what was lost.
    let rows: Vec<Row> = fold("asked for this session only", renderer.transcript_columns())
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
fn refused<T: Terminal>(
    renderer: &mut Renderer<T>,
    problem: &dyn std::fmt::Display,
) -> Result<(), Fatal> {
    let rows: Vec<Row> = fold(&format!("! {problem}"), renderer.transcript_columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Trouble, row))
        .collect();

    Ok(renderer.present(&rows)?)
}

/// What is being asked now, and the lines that would ask for something else.
fn listed<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Read out of a configuration file or off the command line either way, so
    // it goes out the way arrived text goes out.
    let runner = conversation.runner();
    match runner.model() {
        "" => renderer.commit(NO_MODEL_CHOSEN)?,
        name => renderer.commit(&in_force(
            conversation.serving(),
            name,
            runner.effort(),
            terms.style().glyphs(),
        ))?,
    }

    let providers = terms.providers.snapshot();
    let stored = terms.logins.read();
    let lines = lines(
        narrowing::every(&providers),
        &in_use(terms, &providers, &stored),
        terms.consent.routes(),
        terms.style().glyphs(),
    );

    // A line at a time, so each passes through the window on its way up: the
    // list is taller than a window can be, and laid down at once only the
    // rows that fit the window are ever drawn. Folded rather than cut where
    // the window is narrower, since what a line ends with, `trains`, is the
    // part that must be read, and folded again at each width the window takes.
    for line in lines {
        relaid(renderer, Laid::Hung, move |columns| {
            fold(&line, columns)
                .into_iter()
                .map(|part| Row::new().then(Slot::Quiet, part))
                .collect()
        })?;
    }
    Ok(())
}

/// The line that asks for each model of `all` the credential in use serves,
/// for a window with no shelf: the same models the shelf lists, and `trains`
/// after one that is itself warned, as its row would say.
fn lines(
    all: Vec<Selected>,
    using: &BTreeMap<&str, InUse>,
    routes: &crucible_app::content_use::Routes,
    glyphs: Glyphs,
) -> Vec<String> {
    narrowed(all, using)
        .iter()
        .map(|one| {
            let (provider, model) = (one.provider, one.model);
            let asks = format!("/model {}/{}", provider.name, model.name);
            let named = if model.shown == model.name {
                asks
            } else {
                about(&asks, model.shown, glyphs)
            };
            let route = crucible_app::content_use::model_route(provider.name, model.name);
            if routes.warned(&route).is_some() {
                about(&named, TRAINS, glyphs)
            } else {
                named
            }
        })
        .collect()
}

/// The credential each offered provider is sent with, read off `stored`
/// and the environment as a run would read it.
fn in_use<'a>(
    terms: &Terms,
    providers: &'a Providers,
    stored: &StoredCredentials,
) -> BTreeMap<&'a str, InUse> {
    let auth = crucible_app::startup::ProviderAuth {
        settings: &terms.settings,
        from: &*terms.environment,
        stored,
        subscriptions: &terms.subscriptions,
    };
    offered(providers)
        .filter_map(|provider| {
            crucible_app::providers::in_use(provider, auth).map(|one| (provider.name, one))
        })
        .collect()
}

/// The model in force as the shelf's title row says it, with the rung in
/// force: while a turn runs none may be taken here, but one is still being
/// asked on, and the row under the box names it too.
fn titled(current: Asked<'_>, glyphs: Glyphs) -> String {
    match current.model {
        "" => format!("now  {NOTHING_ASKED}"),
        name => format!(
            "now  {}",
            label(
                current.provider.unwrap_or_default(),
                name,
                current.effort,
                current.pace.served.then_some("fast"),
                glyphs
            )
        ),
    }
}

/// The model a switch took, as the row answering `/model` says it: the same
/// label the row under the box then draws.
fn answered(provider: &str, name: &str, effort: Option<Effort>, glyphs: Glyphs) -> String {
    label(provider, name, effort.map(Effort::as_str), None, glyphs)
}

/// The model in force, as the list printed where no shelf fits opens.
fn in_force(provider: Option<&str>, name: &str, effort: Option<Effort>, glyphs: Glyphs) -> String {
    label(
        provider.unwrap_or_default(),
        name,
        effort.map(Effort::as_str),
        None,
        glyphs,
    )
}

#[cfg(test)]
mod tests;
