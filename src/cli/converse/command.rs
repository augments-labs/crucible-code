//! Slash commands: the list a `/` opens above the box, and what running one
//! says.
//!
//! One list, read three ways — the menu that filters as a command is typed,
//! `/help`'s answer, and the match that decides what a finished line does. That
//! is what [`EVERY`] is for: a command that was listed and did nothing, or did
//! something and was never listed, is not a case anybody has to remember to
//! check, because all three walk the same array.
//!
//! An answer is committed rows, the same as everything else that has happened
//! here. Nothing is entered and there is nothing to dismiss: what a command
//! said stays in the scrollback above the box, in the order it was asked.
//!
//! Which of the two ways to draw one uses is decided by where the words came
//! from. Rows this module composed go through [`Renderer::present`], which
//! writes them in colour and does not wrap, because a component was given the
//! width and returned rows that fit it. A word that arrived on the line, or out
//! of a configuration file, goes through [`Renderer::commit`] instead — that is
//! the path that wraps and drops escape sequences, and it is the one every
//! other `!` line in this program already takes.

use crucible_core::Mode;
use crucible_runner::Runner;
use crucible_tui::{Glyphs, Listed, Menu, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::{Terms, mode};

mod login;
mod logout;
mod model;
mod resume;

/// What a line beginning `/` can ask for.
///
/// Closed, and matched arm by arm where it is run, so a command added here is a
/// compile error until it has been given something to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Command {
    /// What these are.
    Help,
    /// Which model answers.
    Model,
    /// A key for a provider, given to a box that does not echo it.
    Login,
    /// A key crucible wrote down, forgotten.
    Logout,
    /// The permission mode: the one in force, or the one named.
    Mode,
    /// The sessions recorded here, and picking one of them up.
    Resume,
    /// Forget the transcript, keeping the session.
    Clear,
    /// End the session.
    Exit,
}

/// Every command there is, in the order `/help` lists them.
///
/// The ones that only say something first and the one that ends the session
/// last. A list is read to find what you did not know to look for, and nobody
/// is looking up how to leave.
const EVERY: [Command; 8] = [
    Command::Help,
    Command::Model,
    Command::Login,
    Command::Logout,
    Command::Mode,
    Command::Resume,
    Command::Clear,
    Command::Exit,
];

/// What a line turned out to be asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wanted<'a> {
    /// A command, and whatever was typed after it.
    Known {
        /// Which one.
        command: Command,
        /// What followed it, trimmed. Empty where nothing did.
        rest: &'a str,
    },
    /// A word shaped like a command that names none.
    Unknown(&'a str),
}

/// What is to happen once a command has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ran {
    /// Back to the prompt.
    Again,
    /// The session is over.
    Leave,
}

impl Command {
    /// What is typed to run it.
    const fn name(self) -> &'static str {
        match self {
            Self::Help => "/help",
            Self::Model => "/model",
            Self::Login => "/login",
            Self::Logout => "/logout",
            Self::Mode => "/mode",
            Self::Resume => "/resume",
            Self::Clear => "/clear",
            Self::Exit => "/exit",
        }
    }

    /// What it does, in the few words a row has room for.
    const fn says(self, glyphs: Glyphs) -> &'static str {
        match self {
            Self::Help => "what these are",
            Self::Model => "which model answers, or set one",
            Self::Login => "give crucible a key for a provider",
            Self::Logout => "forget a key crucible wrote down",
            // The ring itself rather than a sentence about it. `/mode` is the
            // one command that takes a word after it, and the words it takes
            // are the useful half of what there is to say.
            Self::Mode => mode::ring(glyphs),
            Self::Resume => "pick up an earlier session here",
            Self::Clear => "forget what has been said",
            Self::Exit => "leave",
        }
    }

    /// How a list draws it.
    const fn listed(self, glyphs: Glyphs) -> Listed<'static> {
        Listed {
            name: self.name(),
            says: self.says(glyphs),
        }
    }
}

/// What `line` asked for, or `None` where it asked for no command at all.
///
/// The whole of the parsing, done once, here. Everything downstream has either
/// a [`Command`] or a word already known to be a slash and letters, which is
/// what makes an unknown one safe to say back: it cannot be carrying an escape
/// sequence, because a word carrying one is not shaped like a command and never
/// reaches this far.
pub(super) fn wanted(line: &str) -> Option<Wanted<'_>> {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));

    if !shaped(word) {
        return None;
    }

    Some(match named(word) {
        Some(command) => Wanted::Known {
            command,
            rest: rest.trim(),
        },
        None => Wanted::Unknown(word),
    })
}

/// What the menu shows while `line` is being typed.
///
/// Empty unless the line is one word shaped like a command name, which is what
/// closes the menu again the moment the line becomes something else — a path, a
/// sentence, a command with a word after it. A bare `/` is a prefix of every
/// name, so it opens the whole list.
///
/// Nothing is allocated in the ordinary case, where the line is a prompt.
pub(super) fn filtering(line: &str, glyphs: Glyphs) -> Vec<Listed<'static>> {
    if !shaped(line) {
        return Vec::new();
    }

    EVERY
        .into_iter()
        .filter(|command| command.name().starts_with(line))
        .map(|command| command.listed(glyphs))
        .collect()
}

/// Runs one, and leaves what it had to say in the record.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
///
/// `keys` is whether there is a keyboard, which `/login` and `/logout` need
/// before either opens something nobody down a pipe could answer.
pub(super) fn run<T: Terminal>(
    wanted: Wanted<'_>,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<Ran, Fatal> {
    // Nothing is drawn on the way out. The loop is about to end and the shell's
    // own prompt is the next thing on the screen; a row saying goodbye is a row
    // between the two.
    if leaves(wanted) {
        return Ok(Ran::Leave);
    }

    // A blank row on each side, the same as under the welcome: what separates
    // one block from the next is a row with nothing on it, and both of this
    // block's neighbours — the line that asked, the box below — are rows the
    // eye is already resting on.
    renderer.commit("")?;
    answer(wanted, renderer, runner, terms, keys)?;
    renderer.commit("")?;

    Ok(Ran::Again)
}

/// What one command has to say, drawn.
fn answer<T: Terminal>(
    wanted: Wanted<'_>,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let style = terms.style;
    let glyphs = style.glyphs();

    match wanted {
        // Answered by `run`, which returns before this is reached. Spelled out
        // rather than left to a wildcard, so a command added later stops the
        // build here instead of running and saying nothing.
        Wanted::Known {
            command: Command::Exit,
            ..
        } => {}

        Wanted::Known {
            command: Command::Help,
            ..
        } => renderer.present(&listing(columns, glyphs), style.palette())?,

        Wanted::Known {
            command: Command::Model,
            rest,
        } => model::run(rest, renderer, runner, terms)?,

        Wanted::Known {
            command: Command::Login,
            rest,
        } => login::run(rest, renderer, terms, keys)?,

        Wanted::Known {
            command: Command::Logout,
            rest,
        } => logout::run(rest, renderer, terms, keys)?,

        Wanted::Known {
            command: Command::Mode,
            rest,
        } => moded(rest, renderer, runner, style)?,

        Wanted::Known {
            command: Command::Resume,
            rest,
        } => resume::run(rest, renderer, runner, terms)?,

        Wanted::Known {
            command: Command::Clear,
            ..
        } => renderer.present(&forgotten(runner.forget(), columns), style.palette())?,

        Wanted::Unknown(word) => {
            renderer.commit(&format!("! no such command: {word}"))?;
            renderer.commit("")?;
            renderer.present(&listing(columns, glyphs), style.palette())?;
        }
    }

    Ok(())
}

/// `/mode`: the mode in force and the ring it is one of, or the mode named, put
/// where it was named.
///
/// Nothing is agreed to first, which is also true of the key that steps through
/// the ring. The two are one change reached two ways, and a mode that took
/// effect on the press from one of them and waited on the other would be
/// answering the same question differently depending on how it was asked.
fn moded<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    style: Style,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let ring = Row::new().then(Slot::Quiet, clip(mode::ring(style.glyphs()), columns));

    if said.is_empty() {
        let rows = [sentence(runner.mode(), columns), ring];
        renderer.present(&rows, style.palette())?;
        return Ok(());
    }

    let Some(asked) = mode::named(said) else {
        // The word came off the line, so it goes out the way arrived text goes
        // out. Unlike the word an unknown command is named by, this one was
        // never shape-checked: anything at all can follow `/mode `.
        renderer.commit(&format!("! {said} is not a mode"))?;
        renderer.present(&[ring], style.palette())?;
        return Ok(());
    };

    runner.switch(asked);
    renderer.present(&[sentence(asked, columns)], style.palette())?;
    Ok(())
}

/// What `/clear` says, having forgotten `held` messages.
///
/// The second row is the one worth drawing every time. What is above the box is
/// the terminal's scrollback and stays exactly where it is — this program never
/// took that screen and is not about to hand it back empty — so the difference
/// between a session that forgot and one that did not is invisible without a
/// line saying which happened.
fn forgotten(held: usize, columns: usize) -> Vec<Row> {
    if held == 0 {
        return vec![Row::new().then(Slot::Quiet, clip("nothing had been said", columns))];
    }

    let said = match held {
        1 => "forgotten: 1 message".to_owned(),
        _ => format!("forgotten: {held} messages"),
    };

    vec![
        Row::new().then(Slot::Plain, clip(&said, columns)),
        Row::new().then(
            Slot::Quiet,
            clip("what is on screen stays where it is", columns),
        ),
    ]
}

/// Says one line back, quietly, clipped to the window it is said in.
///
/// What `/login` and `/logout` answer with when there is one thing to say: a key
/// was written down, a key was forgotten, or neither happened and here is why.
fn say<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms, said: &str) -> Result<(), Fatal> {
    let row = Row::new().then(Slot::Quiet, clip(said, renderer.columns()));

    Ok(renderer.present(&[row], terms.style.palette())?)
}

/// The row that says which mode is in force, in the colour that mode is drawn
/// in.
fn sentence(mode: Mode, columns: usize) -> Row {
    Row::new().then(mode::tone(mode), clip(mode.sentence(), columns))
}

/// The whole list, which is what `/help` is for.
fn listing(columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let shown: Vec<Listed<'static>> = EVERY.into_iter().map(|one| one.listed(glyphs)).collect();

    Menu {
        shown: &shown,
        chosen: None,
    }
    .rows(columns, glyphs)
}

/// Whether this one ends the session.
const fn leaves(wanted: Wanted<'_>) -> bool {
    matches!(
        wanted,
        Wanted::Known {
            command: Command::Exit,
            ..
        }
    )
}

/// The command that word names.
fn named(word: &str) -> Option<Command> {
    EVERY.into_iter().find(|command| command.name() == word)
}

/// Whether this word is shaped like a command name: a slash, then letters, and
/// nothing else.
///
/// It is what keeps a prompt that opens with a path a prompt. `/etc/hosts is
/// wrong` is a sentence about a file and `/Users/me/notes.md` is a file, and
/// neither is read as a command that happens not to exist. A line is only ever
/// taken for a command where it could not be anything else.
fn shaped(word: &str) -> bool {
    match word.strip_prefix('/') {
        Some(rest) => rest.chars().all(|one| one.is_ascii_alphabetic()),
        None => false,
    }
}

#[cfg(test)]
mod tests;
