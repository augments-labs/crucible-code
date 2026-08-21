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
//! said stays in the transcript above the box, in the order it was asked.
//!
//! Which of the two ways to draw one uses is decided by where the words came
//! from. Rows this module composed go through [`Renderer::present`], which
//! writes them in colour and does not wrap, because a component was given the
//! width and returned rows that fit it. A word that arrived on the line, or out
//! of a configuration file, goes through [`Renderer::commit`] instead — that is
//! the path that wraps and drops escape sequences, and it is the one every
//! other `!` line in this program already takes.

use crucible_core::{Compacting, Mode};
use crucible_runner::Runner;
use crucible_tui::{Glyphs, Listed, Menu, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::{Terms, mode};

mod clear;
mod effort;
mod login;
mod logout;
mod model;
mod resume;
mod theme;

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
    /// How hard it is asked to think.
    Effort,
    /// A key for a provider, given to a box that does not echo it.
    Login,
    /// An account or API key Crucible stored, removed.
    Logout,
    /// The permission mode: the one in force, or the one named.
    Mode,
    /// Which table of colours the terminal is drawn with.
    Theme,
    /// The sessions recorded here, and picking one of them up.
    Resume,
    /// Make room in the model's window now, rather than when it fills.
    Compact,
    /// A new session with nothing said in it, this one left on `/resume`.
    Clear,
    /// End the session.
    Exit,
}

/// Every command there is, in the order `/help` lists them.
///
/// The ones that only say something first and the one that ends the session
/// last. A list is read to find what you did not know to look for, and nobody
/// is looking up how to leave.
const EVERY: [Command; 11] = [
    Command::Help,
    Command::Model,
    Command::Effort,
    Command::Login,
    Command::Logout,
    Command::Mode,
    Command::Theme,
    Command::Resume,
    Command::Compact,
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
    /// The command asked for room to be made, for the reason it names.
    ///
    /// Handed back rather than done here, because making room is a request and
    /// the loop above is where a request is run: on a worker, with the box live
    /// under it and the key that stops it doing something. A command that ran
    /// one on this thread would be a screen that draws nothing and a keyboard
    /// that answers nothing for as long as the model takes.
    Room(Compacting),
}

impl Command {
    /// What is typed to run it.
    const fn name(self) -> &'static str {
        match self {
            Self::Help => "/help",
            Self::Model => "/model",
            Self::Effort => "/effort",
            Self::Login => "/login",
            Self::Logout => "/logout",
            Self::Mode => "/mode",
            Self::Theme => "/theme",
            Self::Resume => "/resume",
            Self::Compact => "/compact",
            Self::Clear => "/clear",
            Self::Exit => "/exit",
        }
    }

    /// What it does, in the few words a row has room for.
    const fn says(self, glyphs: Glyphs) -> &'static str {
        match self {
            Self::Help => "what these are",
            Self::Model => "pick which model answers",
            Self::Effort => "pick how hard it thinks",
            // How you are signed in, rather than what crucible signs with. A
            // key is one of the ways in and the row is read by somebody who
            // does not know yet which of them is theirs.
            Self::Login => "sign in to a provider account",
            Self::Logout => "remove a stored account or API key",
            // The ring itself rather than a sentence about it. `/mode` is the
            // one command that takes a word after it, and the words it takes
            // are the useful half of what there is to say.
            Self::Mode => mode::ring(glyphs),
            Self::Theme => "pick the colours crucible draws with",
            Self::Resume => "pick up an earlier session here",
            // What it does to the session rather than what it is for: somebody
            // reading this row is deciding whether to spend a request on it,
            // and what they lose is the part they cannot get back.
            Self::Compact => "replace what is behind you with notes on it",
            // What it is for rather than what it does to the session: the
            // row is read by somebody who wants the context empty, and
            // "leaving this one" is what they need warning of.
            Self::Clear => "start a new session, leaving this one",
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
/// `keys` is whether there is a keyboard, which `/model`, `/login` and
/// `/logout` need before any of them opens something nobody down a pipe could
/// answer.
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
    let making = answer(wanted, renderer, runner, terms, keys)?;
    renderer.commit("")?;

    Ok(making.map_or(Ran::Again, Ran::Room))
}

/// What one command has to say, drawn.
fn answer<T: Terminal>(
    wanted: Wanted<'_>,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<Option<Compacting>, Fatal> {
    let columns = renderer.columns();
    let style = terms.style();
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
        } => renderer.present(&listing(columns, glyphs))?,

        // Nothing is drawn for it here. What it asks for is run above, where a
        // request is run, and everything a reader sees of one comes from there.
        Wanted::Known {
            command: Command::Compact,
            ..
        } => return Ok(Some(Compacting::Asked)),

        Wanted::Known {
            command: Command::Model,
            rest,
        } => model::run(rest, renderer, runner, terms, keys)?,

        Wanted::Known {
            command: Command::Effort,
            rest,
        } => effort::run(rest, renderer, runner, terms, keys)?,

        Wanted::Known {
            command: Command::Login,
            rest,
        } => login::run(rest, renderer, runner, terms, keys)?,

        Wanted::Known {
            command: Command::Logout,
            rest,
        } => logout::run(rest, renderer, runner, terms, keys)?,

        Wanted::Known {
            command: Command::Mode,
            rest,
        } => moded(rest, renderer, runner, style)?,

        Wanted::Known {
            command: Command::Theme,
            rest,
        } => theme::run(rest, renderer, terms, keys)?,

        // The one other command that can end in a request: a session picked up
        // is put to the reader before it is carried, and one of the three
        // answers costs one.
        Wanted::Known {
            command: Command::Resume,
            rest,
        } => return resume::run(rest, renderer, runner, terms, keys),

        Wanted::Known {
            command: Command::Clear,
            ..
        } => clear::run(renderer, runner, terms)?,

        Wanted::Unknown(word) => {
            renderer.commit(&format!("! no such command: {word}"))?;
            renderer.commit("")?;
            renderer.present(&listing(columns, glyphs))?;
        }
    }

    Ok(None)
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
        renderer.present(&rows)?;
        return Ok(());
    }

    let Some(asked) = mode::named(said) else {
        // The word came off the line, so it goes out the way arrived text goes
        // out. Unlike the word an unknown command is named by, this one was
        // never shape-checked: anything at all can follow `/mode `.
        renderer.commit(&format!("! {said} is not a mode"))?;
        renderer.present(&[ring])?;
        return Ok(());
    };

    runner.switch(asked);
    renderer.present(&[sentence(asked, columns)])?;
    Ok(())
}

/// Says one line back, quietly, clipped to the window it is said in.
///
/// What `/login` and `/logout` answer with when there is one thing to say: a
/// credential was stored, removed, or left alone with the reason why.
fn say<T: Terminal>(renderer: &mut Renderer<T>, said: &str) -> Result<(), Fatal> {
    let row = Row::new().then(Slot::Quiet, clip(said, renderer.columns()));

    Ok(renderer.present(&[row])?)
}

/// A thing and what is said about it, parted by the mark that says they are two.
///
/// The listing a run with no keyboard is given in place of a panel is read that
/// way — a command down the left, what taking it would reach after the mark. So
/// is a row of the sign-in's panel, which names the plan an account is billed
/// under and then what taking the row does, and so is the row beneath a sign-in
/// that has not finished, which is a state and the key that leaves it.
///
/// They share this because a mark that differed between them would say they
/// were different kinds of thing, on surfaces somebody meets one after another
/// inside a single sign-in.
fn about(thing: &str, said: &str, glyphs: Glyphs) -> String {
    format!("{thing} {} {said}", glyphs.dash())
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
