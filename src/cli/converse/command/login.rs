//! `/login`: how crucible is paid for, taken off a panel, and a key given to a
//! box that does not echo it.
//!
//! Two panels, because they are two questions. The first is how you already pay
//! a vendor — a subscription plan, or a console account billed by API usage —
//! and only the console account reaches the second, which asks whose. A plan is
//! drawn without being connected to yet, and says so when it is chosen: a panel
//! listing only what works would let somebody holding a plan conclude their plan
//! is unsupported, when what is true is that it is unbuilt.
//!
//! The vendor is named on the line or chosen from the second panel, and the key
//! is neither. A key typed after a command is a key in the shell's history file,
//! in the process listing while the command runs, and in the scrollback
//! afterwards — three places it was never meant to be, none of which this
//! program could clear. So the two halves are asked for separately: who the key
//! is for, out loud, and the key itself into a box.
//!
//! What that writes is then read straight back, and the session is handed the
//! provider it buys. A key is given by somebody who wants to type at the screen
//! in front of them, and the file it lands in is what makes the run after this
//! one ask the same thing rather than what makes this one ask at all.
//!
//! A run with no key for anything is left alone to draw its prompt. The warning
//! under the welcome names this command and `/model` both, which is the whole of
//! what somebody meeting crucible for the first time has to read; a panel
//! standing in front of that prompt would be this program answering a question
//! nobody asked it, on the one screen where the reader is still finding out
//! where they are.
//!
//! Naming somebody this build has never heard of, a panel that was left, and a
//! window with no room to stand one in all come out the same way: which names
//! crucible knows and which variable each of them signs a request from, written
//! into the scrollback where it can be scrolled. Every one of those halves comes
//! off [`PROVIDERS`], so a vendor this build serves and cannot be logged in to
//! is not a state that exists — which is a claim about console keys, since a
//! plan cannot be logged in to at all yet.

use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip};

use crate::cli::converse::picking::{self, Picked, Taken};
use crate::cli::converse::secret;
use crate::cli::{Fatal, PROVIDERS, Served, remember};

use super::{Terms, say};

/// One way of giving crucible something to sign its requests with.
///
/// Two are subscription plans and one is a console key billed by usage. What a
/// reader is choosing between here is how they already pay a vendor, which is
/// not the same question as which vendor — somebody paying for a plan and
/// somebody holding that same vendor's console key are two people, and only one
/// of them has a key to type.
#[derive(Clone, Copy)]
struct Way {
    /// What it is called on the panel.
    shown: &'static str,

    /// What it buys. The part being chosen between, so it names the plans in
    /// the vendor's own words rather than describing them.
    says: &'static str,

    /// Whether choosing it reaches a key today.
    ///
    /// A plan crucible cannot use yet is still drawn. A panel listing only what
    /// works would let somebody holding one conclude their plan is unsupported,
    /// when what is true is that it is unbuilt — and the two are answered by
    /// different people on different days.
    reaches: bool,
}

/// What the panel offers, in the order it lists them.
///
/// Plans first and the console account last. A reader holding a plan is the one
/// who most needs telling that crucible cannot sign with it yet, and a row under
/// the one that works is a row they would have walked past.
const WAYS: [Way; 3] = [
    Way {
        shown: "OpenAI",
        says: "ChatGPT Plus, Pro, Business and Enterprise plans — not connected yet",
        reaches: false,
    },
    Way {
        shown: "MoonshotAI",
        says: "Kimi Code — not connected yet",
        reaches: false,
    },
    Way {
        shown: "Console account",
        says: "API usage billing",
        reaches: true,
    },
];

/// The sentence under the title of the panel that asks how.
const HOW: &str = "Choose how crucible signs its requests.";

/// What a plan that is drawn but not connected answers with.
///
/// It says where the reader is rather than only that they cannot go on: the row
/// they chose is still on the panel a moment later, and a refusal that did not
/// name the way that does work would send them back to walk the same list.
const UNBUILT: &str = "plans are not connected yet; a console key is the way in today";

/// The sentence under the title of the second panel, which only a console
/// account reaches: whose console, and what happens to the key.
///
/// It names the console outright. Standing where it does, under a row the
/// reader chose a moment ago, a sentence about "a provider" and "a key" would
/// read as the whole of `/login` rather than as the half of it below the plans.
const SAID: &str = concat!(
    "Choose the vendor whose console the key comes from. It is typed into a ",
    "box that does not echo it, and signs requests from the next turn on."
);

/// The one key worth naming on either panel: the arrows and Enter are what a
/// list with a mark on it is already saying.
const CANCEL: &str = "esc to cancel";

/// What escape leaves behind, in place of the rows it used to write.
const LEFT: &str = "cancelled, nothing signed in";

/// Runs it: a key taken for the one named, one chosen off the panel, or where
/// each of them reads a key from.
///
/// `keys` is whether there is a keyboard to take one from. Down a pipe there is
/// not, and a panel or a box waiting for something nobody can type is a session
/// that stopped — so what a piped run gets is the rows naming the variables
/// instead.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let named = PROVIDERS.into_iter().find(|one| one.name == said);

    if keys {
        if let Some(named) = named {
            return given(named, renderer, runner, terms);
        }

        // Nobody named and a keyboard to walk a list with: the panels, which
        // ask how first and which vendor second. A name typed on the line
        // answered both at once, which is why it never sees either.
        if said.is_empty() && walked(renderer, runner, terms)? {
            return Ok(());
        }
    }

    // The word came off the line and was never shape-checked — anything at all
    // can follow `/login ` — so it goes out the way arrived text goes out, and
    // the names that would have worked go under it.
    if named.is_none() && !said.is_empty() {
        renderer.commit(&format!("! no provider called {said}"))?;
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = PROVIDERS
        .into_iter()
        .filter(|one| named.is_none_or(|only| only.name == one.name))
        .map(|one| {
            let said = format!("/login {} — a key from {}", one.name, one.key);

            Row::new().then(Slot::Quiet, clip(&said, columns))
        })
        .collect();

    Ok(renderer.present(&rows, terms.style.palette())?)
}

/// Asks how crucible should sign its requests, and takes the answer to an end.
///
/// Two panels rather than one, because the two questions have different answers
/// for the same person: how you pay a vendor, and then — where that is a console
/// key — which vendor's. A plan does not reach the second, since there is
/// nothing to type for one yet.
///
/// `false` is a window with no room to stand a panel in, and only that: the
/// caller draws the rows instead, which is the one answer a short window can be
/// given. Escape comes back `true`, because leaving a panel is an answer — the
/// screen from before it is what was asked for, and a list of every provider
/// underneath would be the same question put a second time.
fn walked<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<bool, Fatal> {
    let way = match asked(renderer, terms)?.of(&WAYS) {
        Taken::Took(way) => way,
        Taken::Left => {
            say(renderer, terms, LEFT)?;
            return Ok(true);
        }
        Taken::Cramped => return Ok(false),
    };

    if !way.reaches {
        say(renderer, terms, &format!("{} {UNBUILT}", way.shown))?;

        return Ok(true);
    }

    let named = match chosen(renderer, terms)?.of(&PROVIDERS) {
        Taken::Took(named) => named,
        Taken::Left => {
            say(renderer, terms, LEFT)?;
            return Ok(true);
        }
        Taken::Cramped => return Ok(false),
    };

    given(named, renderer, runner, terms)?;

    Ok(true)
}

/// Stands the panel that asks how, and says how it ended.
fn asked<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<Picked, Fatal> {
    let shown: Vec<Offered<'_>> = WAYS
        .iter()
        .map(|way| Offered {
            name: way.shown,
            says: way.says,
        })
        .collect();

    let panel = Panel {
        title: "Log in",
        said: Some(HOW),
        shown: &shown,
        chosen: 0,
        footer: CANCEL,
    };

    picking::pick(renderer, terms.style, panel)
}

/// Stands the panel that asks whose console, and says how it ended.
fn chosen<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<Picked, Fatal> {
    // Where else the same key can come from — and the one thing that differs
    // between rows that would otherwise read identically.
    let says: Vec<String> = PROVIDERS
        .iter()
        .map(|one| format!("typed here, or set in {}", one.key))
        .collect();

    let shown: Vec<Offered<'_>> = PROVIDERS
        .iter()
        .zip(&says)
        .map(|(one, says)| Offered {
            name: one.shown,
            says,
        })
        .collect();

    let panel = Panel {
        title: "Log in",
        said: Some(SAID),
        shown: &shown,
        chosen: 0,
        footer: CANCEL,
    };

    picking::pick(renderer, terms.style, panel)
}

/// Asks for a key, writes it down, and sets this session up with it.
fn given<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let Some(key) = secret::ask(renderer, terms.style, &format!("{} api key", named.name))? else {
        return say(renderer, terms, "nothing was written");
    };

    match terms.logins.keep(named.name, &key) {
        Ok(()) => taken(named, renderer, runner, terms),

        // The key is still in hand and the box is gone, so there is nothing to
        // retry from — which is why this says what stopped rather than only
        // that something did.
        Err(problem) => say(renderer, terms, &format!("! {problem}")),
    }
}

/// Hands this session the provider the key that was just written down buys.
///
/// The key is on disk, so this run is now the run the next launch would be, and
/// reading it back through the same resolution is what makes that true here
/// instead of only at the next start. What it costs is a second read of a file
/// written a line ago; what it buys is that somebody who has just logged in can
/// type at the session in front of them.
///
/// A model and a rung come with it, and only where nothing has answered yet: a
/// flag or a command that named one is somebody's own answer, and a file that
/// said nothing about this run while there was no key does not get to overrule
/// it now that there is. Where nothing names a model either, the line says so —
/// `/model` is the other half of a first minute, and a session that stopped at
/// "logged in" would leave the reader to find that out from the next refusal.
fn taken<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let set = match (terms.serving)(named, &terms.logins.read()) {
        Ok(set) => set,

        // Written and unusable, which is exactly what the next run would meet.
        // Said now rather than left for it: a session that looked logged in and
        // refused every turn is the state this whole command exists to end.
        Err(problem) => return say(renderer, terms, &format!("! {problem}")),
    };

    runner.serve(set.provider);
    terms.provider.set(Some(named.name));

    // Written down as well as switched to, so the next run here opens on the
    // vendor just logged in to instead of asking again — a key says a provider
    // can be reached and never which to ask, and this command is somebody
    // saying which. A failure loses the half that outlives the process and not
    // the session in front of the reader, which is the bargain `/model` is on.
    if let Err(problem) = remember::asking(&terms.choosing, named.name) {
        say(renderer, terms, &format!("! {problem}"))?;
    }

    if runner.model().is_empty()
        && let Some(model) = set.model
    {
        runner.ask(&model);
    }

    if runner.effort().is_none()
        && let Some(effort) = set.effort
    {
        runner.think(effort);
    }

    let said = if runner.model().is_empty() {
        format!("logged in to {}; choose a model with /model", named.name)
    } else {
        format!("logged in to {}, asking {}", named.name, runner.model())
    };

    say(renderer, terms, &said)
}
