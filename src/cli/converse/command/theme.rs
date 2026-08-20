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
use crucible_tui::{clip, fold};

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

/// What the specimen shows.
///
/// One diff, and both axes are visible in it at once. The rows a change touched
/// carry a ground, which is what an **interface** theme decides — it is the one
/// place this program picks a ground and an ink as a pair. The rows it did not
/// touch carry no ground, so they are free to be read, and they are: those are
/// what a **syntax** theme decides. Move either mark and the picture changes,
/// in the half that mark is about.
///
/// That split is not a compromise reached for the picture's sake — it is the
/// only arrangement the rest of this crate allows. A span carries one slot, so
/// a row cannot be both "a line a change took out" and "a keyword"; the rows
/// that are one are not the other, which is how a diff already reads.
const SPECIMEN: [(&str, &str, &str); 5] = [
    (" 31", " ", "fn summarize(users: &[User]) -> String {"),
    (" 32", "-", "    let active = users.len();"),
    (
        " 32",
        "+",
        "    let active = users.iter().filter(alive).count();",
    ),
    (" 33", " ", "    format!(\"{active} active\") // the answer"),
    (" 34", " ", "}"),
];

/// The rows drawn beside the panel in whatever the marks are standing on.
fn specimen(standing: &Standing, columns: usize, _glyphs: Glyphs) -> Vec<Row> {
    let front = 7;

    // Below the gutter there is no room for a specimen at all, and a row built
    // anyway would be wider than the window: the fill cannot shrink one, and a
    // live row the terminal wraps itself leaves the count of what was drawn
    // short by however many rows it took.
    if columns <= front {
        return Vec::new();
    }

    let room = columns.saturating_sub(front);

    let mut rows: Vec<Row> = SPECIMEN
        .iter()
        .map(|(number, sign, line)| {
            let (ground, gutter) = match *sign {
                "-" => (Slot::Removed, Slot::RemovedNumber),
                "+" => (Slot::Added, Slot::AddedNumber),
                _ => (Slot::Plain, Slot::Quiet),
            };

            let mut row = Row::new().then(gutter, format!("{number} {sign} "));

            if ground == Slot::Plain {
                // Untouched by the change, so nothing has taken its ground and
                // the syntax theme is free to say what it is. Read through the
                // same scan the model's own answers go through: a specimen
                // drawn by different code is one that can be right while the
                // thing it stands for is wrong.
                for (slot, text) in read_as_code(line) {
                    let left = room.saturating_sub(row.columns().saturating_sub(front));
                    row.push(slot, clip(&text, left));
                }
            } else {
                row.push(ground, clip(line, room));
            }

            row.fill(ground, columns);
            row
        })
        .collect();

    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Quiet, clip(standing.syntax(), columns)));
    rows
}

/// One line of code, read the way a fenced block in the transcript is read.
fn read_as_code(line: &str) -> Vec<(Slot, String)> {
    let mut runs = Vec::new();
    let mut markdown = crucible_tui::Markdown::default();

    markdown.read(&format!("```rust\n{line}\n```\n"), &mut |slot, text| {
        if text != "\n" {
            runs.push((slot, text.to_owned()));
        }
    });
    runs
}

/// Runs the command.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    if !said.is_empty() {
        if let Some((chosen, name, _)) = EVERY.iter().find(|(_, name, _)| *name == said) {
            return taken(*chosen, name, renderer, terms);
        }
        // A word that is not an interface theme may still be a syntax one, and
        // a reader typing `/theme dracula` means the only thing it can mean.
        if crucible_tui::syntax::every_theme()
            .iter()
            .any(|named| named == said)
        {
            return reading(said, renderer, terms);
        }
        return mistyped(said, renderer, terms);
    }

    if keys {
        match chosen(renderer, terms)? {
            Picked::Took(Chose::Interface(at)) => {
                let (choice, name, _) = EVERY.get(at).copied().unwrap_or(EVERY[0]);
                return taken(choice, name, renderer, terms);
            }
            Picked::Took(Chose::Code(named)) => return reading(&named, renderer, terms),
            // Escape asked for the screen that was there before the panel, so a
            // listing under it would be the same question put a second time.
            Picked::Left => return say(renderer, terms, LEFT),
            // No room to stand one: the listing below is the answer.
            Picked::Cramped => {}
        }
    }

    listed(renderer, terms)
}

/// What came off the panel.
///
/// Three answers rather than two, because what is owed differs in each: one
/// that was left has already been answered, and one there was no room for was
/// never drawn and read no key — so the listing is still owed.
enum Picked {
    /// A row was taken.
    Took(Chose),
    /// Escape. The reader asked for the screen they had before it.
    Left,
    /// No room to stand one. The caller still owes the answer.
    Cramped,
}

/// Which of the two lists a taken row came off.
enum Chose {
    /// A row of the interface list, by index.
    Interface(usize),
    /// A syntax theme, by name.
    Code(String),
}

/// Which of the two lists the keys are acting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// The interface: borders, marks, modes, the diff.
    Interface,
    /// Fenced code.
    Code,
}

/// Where the mark stands on each axis, and which one the keys act on.
struct Standing {
    axis: Axis,
    interface: usize,
    code: usize,
    /// Every syntax theme there is, read once when the panel opens rather than
    /// per frame — the list does not change while somebody is looking at it.
    themes: Vec<String>,
}

impl Standing {
    /// How many rows the axis in view has.
    fn count(&self) -> usize {
        match self.axis {
            Axis::Interface => EVERY.len(),
            Axis::Code => self.themes.len(),
        }
    }

    /// Which row the mark is on, on the axis in view.
    fn at(&mut self) -> &mut usize {
        match self.axis {
            Axis::Interface => &mut self.interface,
            Axis::Code => &mut self.code,
        }
    }

    /// The syntax theme the mark is standing on.
    fn syntax(&self) -> &str {
        self.themes
            .get(self.code)
            .map_or(crucible_tui::syntax::THEME_UNLESS_SAID, String::as_str)
    }
}

/// The narrowest terminal that gets the specimen beside the list.
///
/// Under it the two columns would each be too thin to read: a name and its
/// sentence want about forty, and a line of code wants about the same. So it
/// stacks instead, which is the same picture one column at a time.
const SIDE_BY_SIDE: usize = 88;

/// What the list is given when the specimen stands beside it.
const LIST: usize = 40;

/// Blank columns between the two.
const BETWEEN: usize = 3;

/// Stands the panel where the prompt box was, and says what came off it.
fn chosen<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<Picked, Fatal> {
    let was = terms.style();
    let ground = was.ground();
    let themes = crucible_tui::syntax::every_theme();

    let mut standing = Standing {
        axis: Axis::Interface,
        interface: EVERY
            .iter()
            .position(|(choice, ..)| Some(*choice) == terms.chosen.get())
            .unwrap_or_default(),
        code: themes
            .iter()
            .position(|named| Some(named.as_str()) == terms.reading.borrow().as_deref())
            .unwrap_or_default(),
        themes,
    };

    let ended = region::stand(
        renderer,
        // The preview: both axes as the mark is standing on them, so moving on
        // either one visibly changes the column on the right.
        |standing: &Standing| previewing(was, ground, standing),
        &mut standing,
        |standing, columns, room| {
            let style = previewing(was, ground, standing);
            (laid(standing, style, columns, room), None)
        },
        walking,
    )?;

    // Nothing to put back: the preview never writes `terms.style`. What the
    // panel is drawn in comes from the closure above, which is handed a style
    // built per frame and thrown away with it — so leaving changes nothing, and
    // taking is what writes.

    Ok(match ended {
        Ended::Took => Picked::Took(match standing.axis {
            Axis::Interface => Chose::Interface(standing.interface),
            Axis::Code => Chose::Code(standing.syntax().to_owned()),
        }),
        Ended::Left => Picked::Left,
        // Never drawn and no key read, so nothing was cancelled: what is owed
        // is the listing, the way `/model` and `/effort` answer the same state.
        Ended::Cramped => Picked::Cramped,
    })
}

/// Up and down the list, left and right between the two, taken on enter.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn walking(arrived: Pressed, standing: &mut Standing) -> Moved {
    let count = standing.count();

    match arrived {
        Pressed::Up => {
            let next = standing.at().checked_sub(1);
            step(standing.at(), next)
        }
        Pressed::Down => {
            let next = Some(*standing.at() + 1).filter(|next| *next < count);
            step(standing.at(), next)
        }
        // The two axes are a ring of two, so either arrow reaches the other.
        Pressed::Key(Key::Left | Key::Right) => {
            standing.axis = match standing.axis {
                Axis::Interface => Axis::Code,
                Axis::Code => Axis::Interface,
            };
            Moved::Redraw
        }
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// The style both marks together preview.
fn previewing(was: Style, ground: Option<crucible_tui::Ground>, standing: &Standing) -> Style {
    let (choice, ..) = EVERY.get(standing.interface).copied().unwrap_or(EVERY[0]);
    let wearing = was.wearing(Style::theme(Some(choice), ground));

    match crucible_tui::syntax::colours(standing.syntax()) {
        Some(six) => wearing.reading(six),
        None => wearing,
    }
}

/// The whole picture: a rule the width of the terminal, the two axes named
/// under it, and then the list with the specimen beside it.
///
/// Beside rather than beneath, where there is room. The two are read together —
/// the eye goes from a name to what it does to the code — and a specimen under
/// the list is a specimen below the fold on a short window.
///
/// The rule is drawn here rather than left to the panel, because it belongs to
/// the whole picture rather than to the column on the left: one that stopped
/// where the list stops would draw a box around half of what is being chosen.
fn laid(standing: &Standing, style: Style, columns: usize, room: usize) -> Vec<Row> {
    let glyphs = style.glyphs();
    let mut rows = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
        axes(standing.axis, glyphs),
    ];

    let under = room.saturating_sub(rows.len());
    let shown: Vec<Offered<'_>> = match standing.axis {
        Axis::Interface => EVERY
            .iter()
            .map(|(_, name, says)| Offered { name, says })
            .collect(),
        Axis::Code => standing
            .themes
            .iter()
            .map(|named| Offered {
                name: named,
                says: "",
            })
            .collect(),
    };

    let panel = Panel {
        title: TITLE,
        said: Some(SAID),
        shown: &shown,
        chosen: match standing.axis {
            Axis::Interface => standing.interface,
            Axis::Code => standing.code,
        },
        // Drawn below, across the whole width, for the reason the rule is: it
        // names the keys for the picture rather than for the column on the left.
        footer: "",
    };

    if columns < SIDE_BY_SIDE {
        let beneath = specimen(standing, columns, glyphs);
        let list = panel.within(columns, under.saturating_sub(beneath.len()), glyphs);

        if list.is_empty() {
            return Vec::new();
        }
        rows.extend(ruleless(list));
        rows.extend(beneath);
        rows.push(Row::new());
        rows.push(Row::new().then(Slot::Quiet, clip(FOOTER, columns)));
        return rows;
    }

    let beside = columns.saturating_sub(LIST + BETWEEN);
    let list = ruleless(panel.within(LIST, under, glyphs));
    if list.is_empty() {
        return Vec::new();
    }

    let shown = specimen(standing, beside, glyphs);

    // Centred against the list rather than hung from its first row. The list is
    // long — there are a great many syntax themes — and a specimen pinned to the
    // top of it sits in the corner of a column of empty space, which reads as
    // something that has not finished loading rather than as the answer to what
    // the mark is standing on.
    let above = list.len().saturating_sub(shown.len()) / 2;

    for at in 0..list.len().max(above + shown.len()) {
        let mut row = list.get(at).cloned().unwrap_or_default();
        row.pad(LIST + BETWEEN);

        if let Some(beside) = at.checked_sub(above).and_then(|at| shown.get(at)) {
            row = row.join(beside.clone());
        }
        rows.push(row);
    }

    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Quiet, clip(FOOTER, columns)));
    rows
}

/// The row naming the two axes, with the one the keys act on marked.
///
/// Marked as well as coloured, for the reason every list here is: the thing a
/// key is about to act on is the last thing to leave to a hue.
fn axes(axis: Axis, glyphs: Glyphs) -> Row {
    let mut row = Row::new();

    for (which, name) in [(Axis::Interface, "interface"), (Axis::Code, "code")] {
        let here = which == axis;
        row.push(
            if here { Slot::Accent } else { Slot::Quiet },
            if here {
                format!("{} {name}   ", glyphs.caret())
            } else {
                format!("  {name}   ")
            },
        );
    }

    row.push(Slot::Quiet, "(← → )");
    row
}

/// The panel without the rule it drew for itself.
fn ruleless(mut rows: Vec<Row>) -> Vec<Row> {
    if !rows.is_empty() {
        rows.remove(0);
    }
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

/// Takes a syntax theme: on for this session, and written down for the next run.
fn reading<T: Terminal>(
    named: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let Some(six) = crucible_tui::syntax::colours(named) else {
        return mistyped(named, renderer, terms);
    };

    let now = terms.style().reading(six);
    terms.style.set(now);
    *terms.reading.borrow_mut() = Some(named.to_owned());
    renderer.wears(now.palette());
    renderer.commit(named)?;

    let Err(problem) = remember::syntax(&terms.choosing, named) else {
        return Ok(());
    };

    renderer.commit(&format!("! {problem}"))?;

    let rows: Vec<Row> = fold("read this way for this session only", renderer.columns())
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

    let mut rows = rows;
    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Quiet, "and for fenced code:"));

    let reading = terms.reading.borrow();
    for named in crucible_tui::syntax::every_theme() {
        let mark = if reading.as_deref() == Some(named.as_str()) {
            style.glyphs().done()
        } else {
            " "
        };
        rows.push(
            Row::new()
                .then(Slot::DoneMark, format!("{mark} "))
                .then(Slot::Plain, named),
        );
    }

    Ok(renderer.present(&rows, style.palette())?)
}

#[cfg(test)]
mod tests;
