//! `/context`: how the window of the next request is spent.
//!
//! The model and the size of its window, one bar across the whole window, and
//! a row for each part of the request with its tokens and its share. The bar
//! and each row's swatch wear the same tone, so the two read together; with
//! colour off the rows still read by label and number, and free is the one
//! part shaded rather than solid.
//!
//! Free is printed as the prompt line prints what is left — of the room before
//! compaction, in whole percent — so the two never disagree; every other row
//! is a share of the whole window, to a tenth. The bar is drawn by tokens
//! alone.
//!
//! A window the model never stated has nothing to take a share of: the panel
//! says so, and shows the tokens with no bar, no free row and no percent.
//!
//! It reads and changes nothing, so it stands over a running turn too, with
//! the figures that turn last reported, which include anything it has
//! recorded since its last request.

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_client_api::{self as api, Category};
use crucible_tui::{Bar, Fill, Glyphs, Key, Part, Pressed, Renderer, Row, Slot, Terminal};

use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::draw;

use super::region::{self, Ended, Moved};
use super::{HUNG, Still, Terms};

/// The narrowest window the rows keep the two-cell swatch and the wider
/// columns in; below it they close up behind a single cell.
const ROOMY: usize = 42;

/// Asks for the breakdown of the next request and stands it, or prints it
/// where no keys can close a panel or there is no room to stand one.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let context = match terms.perform(conversation, api::Command::Context) {
        Performed::Context { model, breakdown } => {
            crucible_app::client::context(&model, &breakdown)
        }
        other => return Ok(renderer.commit(&astray(&other))?),
    };
    if keys && stood(renderer, terms, &context, |_| Ok(()))? != Ended::Cramped {
        return Ok(());
    }
    // Hung under the line that asked, so laid out short of the mark.
    let columns = renderer.transcript_columns().saturating_sub(HUNG);
    Ok(renderer.present(&body(&context, columns, terms.style().glyphs()))?)
}

/// Stands the panel over a running turn, with the figures of the last
/// request it built, or prints them where there is no room to stand it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn live<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    context: &api::Context,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<(), Fatal> {
    if stood(renderer, terms, context, while_waiting)? == Ended::Cramped {
        let columns = renderer.transcript_columns();
        renderer.present(&body(context, columns, terms.style().glyphs()))?;
    }
    Ok(())
}

/// Stands the panel until it is closed, or says there was no room.
fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    context: &api::Context,
    while_waiting: impl FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Ended, Fatal> {
    let style = terms.style();
    region::stand_while(
        renderer,
        |_| style,
        &mut Still,
        |_, columns, _| (panel(context, columns, style.glyphs()), None),
        |pressed, _| closing(&pressed),
        while_waiting,
    )
}

/// What a key does to the panel: escape, the key its footer names, closes it,
/// as interrupt and end of input close every panel; a new size redraws it;
/// nothing else, enter among it, touches it.
fn closing(pressed: &Pressed) -> Moved {
    match pressed {
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// The panel's rows at `columns`: a rule, the body, and how to close it.
fn panel(context: &api::Context, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut rows = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
    ];
    rows.extend(body(context, columns, glyphs));
    rows.push(Row::new());
    rows.push(
        Row::new()
            .then(Slot::Quiet, "esc to close")
            .clipped(columns),
    );
    rows
}

/// The title, the bar where the window is known, and a row per category.
fn body(context: &api::Context, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let dot = glyphs.dot();
    let model = context
        .model
        .as_ref()
        .map(|model| format!("{} {dot} ", model.text().as_str()))
        .unwrap_or_default();
    // A window of nothing is no more a window to take a share of than none.
    let window = context.window.filter(|&window| window > 0);
    let size = window.map_or_else(
        || "window not known".to_owned(),
        |window| format!("{} window", draw::tokens(window)),
    );
    let title = format!("Context {dot} {model}{size}");
    let mut rows = vec![
        Row::new().then(Slot::Strong, title).clipped(columns),
        Row::new(),
    ];

    let Some(window) = window else {
        rows.extend(
            Category::EVERY
                .into_iter()
                .filter(|&category| category != Category::Free)
                .map(|category| unmeasured(context, category).clipped(columns)),
        );
        return rows;
    };

    let parts = Category::EVERY.map(|category| Part {
        slot: tone(category),
        fill: fill(category),
        size: context.tokens(category),
    });
    rows.push(Bar { parts: &parts }.row(columns, glyphs));
    rows.push(Row::new());
    rows.extend(Category::EVERY.into_iter().map(|category| {
        measured(context, category, window, columns >= ROOMY, glyphs).clipped(columns)
    }));
    rows
}

/// One category's row, with its swatch and its share of `window`.
fn measured(
    context: &api::Context,
    category: Category,
    window: u64,
    roomy: bool,
    glyphs: Glyphs,
) -> Row {
    let tokens = context.tokens(category);
    let share = match category {
        // What the prompt line says is left, to the percent it says it in.
        Category::Free => context
            .left
            .map(|left| format!("{}%", left.get()))
            .unwrap_or_default(),
        Category::System
        | Category::Instructions
        | Category::Tools
        | Category::Mcp
        | Category::Messages
        | Category::Reserve => share(tokens, window),
    };
    let (indent, cells, tokens_wide, share_wide) = if roomy {
        ("  ", 2, 7, 9)
    } else {
        ("", 1, 6, 7)
    };
    Row::new()
        .then(Slot::Plain, indent)
        .then(tone(category), fill(category).cell(glyphs).repeat(cells))
        .then(Slot::Plain, format!(" {:<21}", name(category)))
        .then(
            Slot::Quiet,
            format!("{:>tokens_wide$}{share:>share_wide$}", count(tokens)),
        )
}

/// One category's row where there is no window to take a share of.
fn unmeasured(context: &api::Context, category: Category) -> Row {
    Row::new()
        .then(Slot::Plain, format!("  {:<24}", name(category)))
        .then(
            Slot::Quiet,
            format!("{:>7}", count(context.tokens(category))),
        )
}

/// What a category is called on its row.
const fn name(category: Category) -> &'static str {
    match category {
        Category::System => "system prompt",
        Category::Instructions => "project instructions",
        Category::Tools => "tool schemas",
        Category::Mcp => "MCP tool schemas",
        Category::Messages => "messages",
        Category::Reserve => "reserve",
        Category::Free => "free",
    }
}

/// The tone a category's part of the bar, and its swatch, are drawn in.
const fn tone(category: Category) -> Slot {
    match category {
        Category::System => Slot::Plain,
        Category::Instructions => Slot::DoneMark,
        Category::Tools => Slot::DoingMark,
        Category::Mcp => Slot::Trouble,
        Category::Messages => Slot::Accent,
        Category::Reserve | Category::Free => Slot::Quiet,
    }
}

/// Spent, or free.
const fn fill(category: Category) -> Fill {
    match category {
        Category::Free => Fill::Shaded,
        Category::System
        | Category::Instructions
        | Category::Tools
        | Category::Mcp
        | Category::Messages
        | Category::Reserve => Fill::Solid,
    }
}

/// A count of tokens, to a tenth of the unit it is read in.
fn count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{}.{}k", tokens / 1_000, tokens % 1_000 / 100),
        _ => format!("{}.{}M", tokens / 1_000_000, tokens % 1_000_000 / 100_000),
    }
}

/// `tokens` as a share of `window`, to the nearest tenth of a percent; none
/// at all is said without the tenth, so it reads apart from a sliver.
fn share(tokens: u64, window: u64) -> String {
    if tokens == 0 {
        return "0%".to_owned();
    }
    let window = u128::from(window);
    let tenths = (u128::from(tokens) * 1_000 + window / 2) / window;
    format!("{}.{}%", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests;
