//! `/theme`: which table of colours the terminal is drawn with, chosen by
//! seeing rather than by reading a name.
//!
//! **The specimen is the whole point.** A theme is a list of hues and nobody
//! can picture one from a word, so the panel stands over a few rows drawn in
//! whatever the mark is on: a diff, because the diff is where a theme spends
//! the colours that carry meaning rather than decoration, and the row a prompt
//! is left on, because that one takes a ground. Moving the mark redraws them.
//!
//! **Leaving puts back what was in force.** The preview is not a preview if it
//! only goes one way: the palette really does change as the mark moves, so
//! escape has to change it back, and it does — the style is put back before the
//! prompt returns.
//!
//! **What is taken outlives the session.** A theme is the same answer every
//! time this machine is used, so asking once a session is asking for ever. It
//! goes to the user's own file, spliced, and a file that cannot be written
//! leaves the theme in force for this session with one row saying so — the
//! contract `/model` already keeps for the same reason.
//!
//! **Rows already in scrollback keep the colours they were drawn in.** This
//! process draws inline and can never go back over what it has committed. A
//! theme changes what is drawn from here on, and the transcript above the box
//! is the record of what it looked like at the time.

use crucible_config::ThemeChoice;
use crucible_tui::{Glyphs, Offered, Panel, Renderer, Row, Slot, Terminal};

use crate::cli::Fatal;
use crate::cli::converse::region::{self, Ended, Moved, step};
use crate::cli::remember;
use crate::cli::style::Style;
use crucible_tui::{Key, Pressed};

use super::{Terms, say};

/// What escape leaves behind, in place of the listing it used to write.
const LEFT: &str = "cancelled, no theme taken";

/// The one key worth naming under the panel.
const FOOTER: &str = "enter takes it · esc puts back the one in force";

/// The few words at the top saying what is being chosen.
const TITLE: &str = "Theme";

/// The sentence under it.
const SAID: &str = "the mark moves and the rows below are drawn in whatever it is standing on";

/// Every theme there is, in the order the panel lists them.
///
/// `auto` first because it is the answer most people want and the only one that
/// keeps being right when they change their terminal. The two that leave the
/// red-green axis are named for what they do rather than for who they are for:
/// a reader knows their own eyes, and a row that guesses at them is a row that
/// gets it wrong out loud.
const EVERY: [(ThemeChoice, &str, &str); 6] = [
    (
        ThemeChoice::Auto,
        "auto",
        "follow the terminal's own background",
    ),
    (ThemeChoice::Dark, "dark", "for a dark background"),
    (ThemeChoice::Light, "light", "for a light one"),
    (
        ThemeChoice::ColourblindDark,
        "colourblind-dark",
        "dark, with the diff off the red-green axis",
    ),
    (
        ThemeChoice::ColourblindLight,
        "colourblind-light",
        "light, with the same swap",
    ),
    (
        ThemeChoice::Ansi,
        "ansi",
        "only the sixteen colours your terminal already has",
    ),
];

/// What the specimen shows: a line a change took out, and the one that replaced
/// it.
///
/// A diff rather than anything prettier, because the diff is the one place a
/// theme paints a ground and picks a pair to go on it — which is the part that
/// cannot be checked by reading a name.
/// What the specimen spends before the code starts: the line number, a space,
/// the sign, and the space after it.
///
/// Named rather than counted at each use, because the clip and the fill have to
/// agree about it — one column out either way is a row that says less than it
/// could, or one the terminal wraps itself.
const GUTTER: usize = 7;

const SPECIMEN: [(&str, &str, &str); 3] = [
    (
        " 209",
        " ",
        "fn depth(from: &dyn Fn(&str) -> Option<String>)",
    ),
    (" 210", "-", "    if from(COLORTERM).is_some_and(truecolor)"),
    (" 210", "+", "    let Some(depth) = Self::depth(from)"),
];

/// What came off the panel.
///
/// Three answers rather than two, because what is owed differs in each: one
/// that was left has already been answered, and one there was no room for was
/// never drawn and read no key — so the listing is still owed.
enum Picked {
    /// The row at this index was taken.
    Took(usize),
    /// Escape. The reader asked for the screen they had before it.
    Left,
    /// No room to stand one. The caller still owes the answer.
    Cramped,
}

/// Runs the command.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    if !said.is_empty() {
        return match EVERY.iter().find(|(_, name, _)| *name == said) {
            Some((chosen, name, _)) => taken(*chosen, name, renderer, terms),
            None => mistyped(said, renderer, terms),
        };
    }

    if keys {
        match chosen(renderer, terms)? {
            Picked::Took(at) => {
                let (choice, name, _) = EVERY.get(at).copied().unwrap_or(EVERY[0]);
                return taken(choice, name, renderer, terms);
            }
            // Escape asked for the screen that was there before the panel, so a
            // listing under it would be the same question put a second time.
            Picked::Left => return say(renderer, terms, LEFT),
            // No room to stand one: the listing below is the answer.
            Picked::Cramped => {}
        }
    }

    listed(renderer, terms)
}

/// Stands the panel where the prompt box was, and says which row came off it.
///
/// `None` where it was left, and where there was no room to stand one — the
/// listing below is what answers the second case.
fn chosen<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<Picked, Fatal> {
    let was = terms.style();
    let ground = was.ground();
    let mut at = EVERY
        .iter()
        .position(|(choice, ..)| Some(*choice) == terms.chosen.get())
        .unwrap_or_default();

    let shown: Vec<Offered<'_>> = EVERY
        .iter()
        .map(|(_, name, says)| Offered { name, says })
        .collect();

    let ended = region::stand(
        renderer,
        // The preview: what the mark is standing on, not what is in force.
        |marked: &usize| previewing(was, ground, *marked),
        &mut at,
        |marked, columns, room| {
            let style = previewing(was, ground, *marked);
            let panel = Panel {
                title: TITLE,
                said: Some(SAID),
                shown: &shown,
                chosen: *marked,
                footer: FOOTER,
            };

            let specimen = specimen(columns, style.glyphs(), style.palette().bands());
            let above = room.saturating_sub(specimen.len());
            let mut rows = panel.within(columns, above, style.glyphs());

            if rows.is_empty() {
                return (Vec::new(), None);
            }
            rows.extend(specimen);
            (rows, None)
        },
        |arrived, at| walking(arrived, at, EVERY.len()),
    )?;

    // Nothing to put back: the preview never writes `terms.style`. What the
    // panel is drawn in comes from the closure above, which is handed a style
    // built per frame and thrown away with it — so leaving changes nothing, and
    // taking is what writes.

    Ok(match ended {
        Ended::Took => Picked::Took(at),
        Ended::Left => Picked::Left,
        // Never drawn and no key read, so nothing was cancelled: what is owed
        // is the listing, the way `/model` and `/effort` answer the same state.
        Ended::Cramped => Picked::Cramped,
    })
}

/// Up and down the list, taken on enter and put back on escape.
///
/// The list is walked rather than stepped along, so it is the vertical arrows
/// that move it — a component whose arrows disagree with its picture is one
/// nobody trusts twice.
fn walking(arrived: Pressed, at: &mut usize, count: usize) -> Moved {
    match arrived {
        Pressed::Up => step(at, at.checked_sub(1)),
        Pressed::Down => step(at, Some(*at + 1).filter(|next| *next < count)),
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// The style a mark standing on `at` previews.
fn previewing(was: Style, ground: Option<crucible_tui::Ground>, at: usize) -> Style {
    let (choice, ..) = EVERY.get(at).copied().unwrap_or(EVERY[0]);

    was.wearing(Style::theme(Some(choice), ground))
}

/// The rows drawn under the panel in whatever the mark is on.
fn specimen(columns: usize, glyphs: Glyphs, banded: bool) -> Vec<Row> {
    // Below the gutter there is no room for a specimen at all, and a row built
    // anyway would be wider than the window: the fill cannot shrink one, and a
    // live row the terminal wraps leaves the count of what was drawn short.
    if columns <= GUTTER {
        return Vec::new();
    }

    let mut rows = vec![Row::new()];

    for (number, sign, line) in SPECIMEN {
        let (ground, gutter) = match sign {
            "-" => (Slot::Removed, Slot::RemovedNumber),
            "+" => (Slot::Added, Slot::AddedNumber),
            _ => (Slot::Plain, Slot::Quiet),
        };

        let mut row = Row::new().then(gutter, format!("{number} {sign} "));
        row.push(
            ground,
            crucible_tui::clip(line, columns.saturating_sub(GUTTER)),
        );
        row.fill(ground, columns);
        rows.push(row);
    }

    rows.push(Row::new());
    rows.extend(crucible_tui::Prompt::committed(
        "and the row your own prompt is left on",
        columns,
        glyphs,
        banded,
    ));
    rows
}

/// Takes it: on for this session, and written down for the next run.
fn taken<T: Terminal>(
    choice: ThemeChoice,
    name: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let was = terms.style();
    let now = was.wearing(Style::theme(Some(choice), was.ground()));

    terms.style.set(now);
    terms.chosen.set(Some(choice));
    renderer.wears(now.palette());
    renderer.commit(name)?;

    let Err(problem) = remember::drawing(&terms.choosing, name) else {
        return Ok(());
    };

    renderer.commit(&format!("! {problem}"))?;

    // Wrapped rather than clipped, for the reason `/model` wraps the same
    // sentence: short as the row is, a narrow enough window would still cut it,
    // and half of it says nothing about what was lost.
    let rows: Vec<Row> =
        crucible_tui::fold("drawn this way for this session only", renderer.columns())
            .into_iter()
            .map(|row| Row::new().then(Slot::Quiet, row))
            .collect();

    Ok(renderer.present(&rows, now.palette())?)
}

/// What a word that names no theme is answered with.
fn mistyped<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    renderer.commit(&format!("! no such theme: {said}"))?;
    listed(renderer, terms)
}

/// The themes, written into the scrollback where no panel could be stood.
fn listed<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<(), Fatal> {
    let style = terms.style();
    let in_force = terms.chosen.get();
    let widest = EVERY
        .iter()
        .map(|(_, name, _)| name.len())
        .max()
        .unwrap_or(0);

    let rows: Vec<Row> = EVERY
        .iter()
        .map(|(choice, name, says)| {
            let mark = if Some(*choice) == in_force {
                style.glyphs().done()
            } else {
                " "
            };
            let mut row = Row::new().then(Slot::DoneMark, format!("{mark} "));
            row.push(Slot::Plain, format!("{name:widest$}"));
            row.push(Slot::Quiet, format!("  {says}"));
            row
        })
        .collect();

    Ok(renderer.present(&rows, style.palette())?)
}

#[cfg(test)]
mod tests;
