//! `/fast`: whether the model in force is asked to answer fast, where its
//! vendor serves a fast form, and what that costs.
//!
//! Two rows over the model in force, standard and fast, with the vendor's
//! price and speed beneath fast and whatever the vendor says of it first above
//! both. The
//! words are the provider's, for this model on this credential: a price read
//! off another model, or off another way of signing in, is a price somebody
//! pays without having been shown it.
//!
//! A model with no fast form, and one that is itself a fast model, are told so
//! on one line rather than offered a panel with one row that could be taken.
//! What is taken is written down with the model it was taken for, and goes
//! back to standard on its own where the price shown may no longer be the
//! price.

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::providers::Rows;
use crucible_app::speed::Hastened;
use crucible_client_api::Command;
use crucible_models::{Cost, Effort, FastForm, Speed};
use crucible_tui::{Glyphs, Offered, Panel, Renderer, Row, Slot, Terminal, clip, fold, label};

use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::converse::picking::{self, Taken};

use super::{Asked, Terms, about, say};

/// What escape leaves behind.
const LEFT: &str = "cancelled, the speed is unchanged";

/// What `Standard` says beneath it.
const STANDARD: &str = "The standard price and speed";

/// The two speeds, in the order the panel and the lines to type list them.
const SPEEDS: [Speed; 2] = [Speed::Standard, Speed::Fast];

/// Runs it: the speed named, one taken off the panel, or the speed in force
/// and the lines that ask for each.
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
    let runner = conversation.runner();
    let named = conversation.serving();
    let Some(provider) = named.filter(|_| !runner.model().is_empty()) else {
        return renderer.commit(terms.unasked(named)).map_err(Fatal::from);
    };
    let asked = Asked {
        provider: Some(provider),
        model: runner.model(),
        effort: runner.effort().map(Effort::as_str),
        pace: super::Pace::of(runner),
    };
    let glyphs = terms.style().glyphs();
    let cost = match offered(asked, glyphs) {
        Ok(cost) => cost,
        Err(line) => return say(renderer, &line),
    };

    let speed = match said {
        "" => None,
        "on" => Some(Speed::Fast),
        "off" => Some(Speed::Standard),
        // Anything at all can follow `/fast `, so it is not said back.
        _ => {
            renderer.commit("! /fast takes on or off")?;
            return listing(renderer, cost, glyphs);
        }
    };
    if let Some(speed) = speed {
        return taken(speed, renderer, conversation, terms);
    }

    if keys {
        match chosen(renderer, terms, asked, cost, &mut |_| Ok(()))? {
            Taken::Took(speed) => return taken(speed, renderer, conversation, terms),
            Taken::Left => return say(renderer, LEFT),
            Taken::Cramped => {}
        }
    }

    let row = Row::new().then(
        Slot::Plain,
        clip(asked.pace.speed.as_str(), renderer.columns()),
    );
    renderer.present(&[row])?;
    listing(renderer, cost, glyphs)
}

/// The panel stood over a running turn, and the speed taken off it, to be
/// asked for once the turn ends. `None` where nothing was taken, and the line
/// that says why has been said.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn picked_while<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    asked: Asked<'_>,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Option<Speed>, Fatal> {
    let glyphs = terms.style().glyphs();
    if asked.provider.is_none() || asked.model.is_empty() {
        renderer.commit(terms.unasked(asked.provider))?;
        return Ok(None);
    }
    let cost = match offered(asked, glyphs) {
        Ok(cost) => cost,
        Err(line) => {
            say(renderer, &line)?;
            return Ok(None);
        }
    };
    match chosen(renderer, terms, asked, cost, while_waiting)? {
        Taken::Took(speed) => Ok(Some(speed)),
        Taken::Left => {
            say(renderer, LEFT)?;
            Ok(None)
        }
        Taken::Cramped => {
            listing(renderer, cost, glyphs)?;
            Ok(None)
        }
    }
}

/// Asks for `speed` from the next turn on, and says what came of it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn taken<T: Terminal>(
    speed: Speed,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let asked = Command::SetSpeed(crucible_app::client::pace(speed));
    let hastened = match terms.perform(conversation, asked) {
        Performed::Speed(hastened) => hastened,
        other => return say(renderer, &astray(&other)),
    };
    let glyphs = terms.style().glyphs();
    let runner = conversation.runner();
    let named = label(
        runner.serving(),
        runner.model(),
        runner.effort().map(Effort::as_str),
        (speed == Speed::Fast).then_some("fast"),
        glyphs,
    );
    let said = match hastened {
        Hastened::Unasked => {
            return renderer
                .commit(terms.unasked(conversation.serving()))
                .map_err(Fatal::from);
        }
        Hastened::Unsupported | Hastened::Own => {
            let form = runner.provider().fast(runner.model());
            let (provider, model) = (runner.serving(), runner.model());
            return say(renderer, &unoffered(provider, model, form, glyphs));
        }
        Hastened::Taken { unwritten: None } => named,
        Hastened::Taken {
            unwritten: Some(problem),
        } => {
            renderer.commit(&format!("! {problem}"))?;
            format!("{named}, this session only")
        }
    };

    let rows: Vec<Row> = fold(&said, renderer.columns())
        .into_iter()
        .map(|row| Row::new().then(Slot::Quiet, row))
        .collect();
    Ok(renderer.present(&rows)?)
}

/// What fast costs for the model `asked` is over, or the line that says why
/// there is nothing to switch.
fn offered(asked: Asked<'_>, glyphs: Glyphs) -> Result<Cost, String> {
    match asked.pace.form {
        FastForm::Field(cost) => Ok(cost),
        form @ (FastForm::None | FastForm::Own(_)) => Err(unoffered(
            asked.provider.unwrap_or_default(),
            asked.model,
            form,
            glyphs,
        )),
    }
}

/// The line for a model that has no fast form to switch on or off.
fn unoffered(provider: &str, model: &str, form: FastForm, glyphs: Glyphs) -> String {
    let named = label(provider, model, None, None, glyphs);
    match form {
        FastForm::Own(_) => format!("{named} is a fast model of its own; /model lists the others"),
        FastForm::None | FastForm::Field(_) => format!("{named} has no fast form"),
    }
}

/// Stands the panel where the prompt box was, and says which speed came off
/// it.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    asked: Asked<'_>,
    cost: Cost,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Taken<Speed>, Fatal> {
    let glyphs = terms.style().glyphs();
    let provider = asked.provider.unwrap_or_default();
    let title = title(provider, asked.model, credential(provider, terms), glyphs);
    let footer = format!("enter to choose {} esc to cancel", glyphs.dot());
    let fast = worded(cost, glyphs);
    let shown = [
        Offered {
            name: "Standard",
            says: STANDARD,
        },
        Offered {
            name: "Fast",
            says: &fast,
        },
    ];
    let panel = Panel {
        source: None,
        title: &title,
        said: cost.caveat,
        shown: &shown,
        // On the speed in force, so Enter changes nothing by accident.
        chosen: usize::from(asked.pace.speed == Speed::Fast),
        footer: &footer,
    };

    Ok(picking::pick_while(renderer, terms.style(), panel, while_waiting)?.of(&SPEEDS))
}

/// What the panel is standing over: the provider, the model, and the
/// credential the price is for.
fn title(provider: &str, model: &str, credential: Option<String>, glyphs: Glyphs) -> String {
    let dot = glyphs.dot();
    let named = format!("Speed {dot} {provider} {dot} {model}");
    match credential {
        Some(credential) => format!("{named} {dot} {credential}"),
        None => named,
    }
}

/// The words for the credential `provider` is served by: the stored one, else
/// the one its variable holds.
fn credential(provider: &str, terms: &Terms) -> Option<String> {
    let rows = Rows::production();
    let stored = terms.logins.read();
    rows.held(provider, &stored)
        .or_else(|| rows.environment(provider))
        .map(|row| row.credential().replace('·', terms.style().glyphs().dot()))
}

/// What fast costs and how much faster the vendor says it is, where it says.
fn worded(cost: Cost, glyphs: Glyphs) -> String {
    match cost.speed {
        Some(speed) => format!("{} {} {speed}", cost.price, glyphs.dot()),
        None => cost.price.to_owned(),
    }
}

/// The two lines that ask for each speed, the price beside fast.
fn listing<T: Terminal>(
    renderer: &mut Renderer<T>,
    cost: Cost,
    glyphs: Glyphs,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let rows = [
        about("/fast on", &worded(cost, glyphs), glyphs),
        about("/fast off", STANDARD, glyphs),
    ]
    .map(|line| Row::new().then(Slot::Quiet, clip(&line, columns)));
    Ok(renderer.present(&rows)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_the_credential_the_price_is_for() {
        assert_eq!(
            title(
                "openai",
                "gpt-6-sol",
                Some("OpenAI key".to_owned()),
                Glyphs::Unicode
            ),
            "Speed · openai · gpt-6-sol · OpenAI key"
        );
        assert_eq!(
            title("openai", "gpt-6-sol", None, Glyphs::Ascii),
            "Speed - openai - gpt-6-sol"
        );
    }

    #[test]
    fn fast_says_its_price_and_the_speed_the_vendor_states_beside_it() {
        let stated = Cost {
            price: "2x the price",
            speed: Some("up to 2.5x faster"),
            caveat: None,
        };
        assert_eq!(
            worded(stated, Glyphs::Unicode),
            "2x the price · up to 2.5x faster"
        );
        assert_eq!(
            worded(
                Cost {
                    speed: None,
                    ..stated
                },
                Glyphs::Ascii
            ),
            "2x the price"
        );
    }

    #[test]
    fn a_model_with_nothing_to_switch_says_which_of_the_two_it_is() {
        let own = FastForm::Own(Cost {
            price: "6x the speed for 3x the quota",
            speed: None,
            caveat: None,
        });
        assert_eq!(
            unoffered(
                "deepseek",
                "deepseek-flash",
                FastForm::None,
                Glyphs::Unicode
            ),
            "deepseek · deepseek-flash has no fast form"
        );
        assert_eq!(
            unoffered(
                "moonshot",
                "kimi-for-coding-highspeed",
                own,
                Glyphs::Unicode
            ),
            "moonshot · kimi-for-coding-highspeed is a fast model of its own; /model lists the others"
        );
    }
}
