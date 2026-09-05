//! Slash commands: the list a `/` opens above the box, and what running one
//! says.
//!
//! One list, read three ways — the menu that filters as a command is typed,
//! `/help`'s answer, and the match that decides what a finished line does. That
//! is what the registry [`builtins`] fills is for: a command that was listed
//! and did nothing, or did something and was never listed, is not a case
//! anybody has to remember to check, because all three walk the same snapshot.
//! [`EVERY`] is what fills it, in the order `/help` lists them, and each entry
//! is registered with the built-in provenance a wiring diagnostic reports. The
//! registry refuses a second command under a taken name and says which two
//! sources claimed it, so a contribution registered later cannot quietly take
//! `/help` from under the reader.
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

use crucible_core::{
    Collision, Compacting, Mode, Provenance, Registered, Registry, RegistryError, RegistrySnapshot,
    SourceKind,
};
use crucible_runner::Runner;
use crucible_tui::{Glyphs, Key, Listed, Menu, Pressed, Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::region::{self, Moved};
use super::{Held, Terms, mode, picking};
use crate::cli::Served;

mod cache;
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
    /// Inspect or clean provider prompt-cache state.
    Cache,
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
const EVERY: [Command; 12] = [
    Command::Help,
    Command::Model,
    Command::Effort,
    Command::Login,
    Command::Logout,
    Command::Mode,
    Command::Theme,
    Command::Resume,
    Command::Cache,
    Command::Compact,
    Command::Clear,
    Command::Exit,
];

/// One slash command as the registry holds it.
///
/// The built-in ones wrap a [`Command`]; the provenance says so, and is what a
/// collision diagnostic names. There is nothing else here yet on purpose: what
/// a command does is still decided arm by arm below, and a record that carried
/// a second way of running one would be a case those arms could not see.
#[derive(Debug)]
pub(crate) struct Slash {
    /// Which command this is.
    command: Command,
    /// Where it came from.
    provenance: Provenance,
}

impl Slash {
    /// A command compiled into this binary.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] where the name does not fit a source identity, which a
    /// constant name cannot fail to.
    fn builtin(command: Command) -> Result<Self, RegistryError> {
        let name = command.name();
        let provenance = Provenance::new(
            SourceKind::Builtin,
            format!("crucible:{name}"),
            format!("built-in {name} command"),
        )?;
        Ok(Self {
            command,
            provenance,
        })
    }
}

impl Registered for Slash {
    fn id(&self) -> &str {
        self.command.name()
    }

    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn retained_bytes(&self) -> usize {
        size_of::<Self>() + self.provenance.retained_bytes()
    }
}

/// The commands a line is read against: one generation of the registry.
pub(crate) type Commands = RegistrySnapshot<Slash>;

/// The command registry with every built-in command in it, in the order
/// `/help` lists them.
///
/// Refusing collisions rather than ranking them: a command is typed by name,
/// and two things answering to one name is exactly the ambiguity a reader
/// cannot see from the box.
///
/// # Errors
///
/// [`RegistryError`] where a built-in could not be registered — a name written
/// twice in [`EVERY`], or one too long for a source identity. Both are wiring
/// defects, and the sentence names the command.
pub(crate) fn builtins() -> Result<Registry<Slash>, RegistryError> {
    let registry = Registry::new(Collision::Refuse);
    let mut staged = registry.stage();
    for command in EVERY {
        staged.register(Slash::builtin(command)?)?;
    }
    registry.commit(staged)?;
    Ok(registry)
}

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

/// A command read mid-turn, owned so it crosses from the keyboard loop to the
/// turn's own.
///
/// The line it was read from is the box's, and the box is cleared as the
/// command is taken, so the command and its rest are owned here rather than
/// borrowed from a line that is gone.
#[derive(Debug)]
pub(super) enum Owned {
    /// A command, and whatever was typed after it.
    Known {
        /// Which one.
        command: Command,
        /// What followed it. Empty where nothing did.
        rest: String,
    },
    /// A word shaped like a command that names none. The word is not kept: an
    /// unknown one is refused whichever it is, and the panel says only that it
    /// names no command.
    Unknown,
}

impl Owned {
    /// Which command this is, or `Exit` for a word that names none — a command
    /// that is never live, so an unknown word is refused the way a safe-looking
    /// typo is.
    pub(super) fn command(&self) -> Command {
        match self {
            Self::Known { command, .. } => *command,
            Self::Unknown => Command::Exit,
        }
    }

    /// What it does while a turn is running: the command's own class, or a
    /// refusal for a word that names none.
    pub(super) fn class(&self) -> MidTurn {
        match self {
            Self::Known { command, .. } => command.mid_turn(),
            Self::Unknown => MidTurn::Refused("names no command"),
        }
    }
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
            Self::Cache => "/cache",
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
            Self::Cache => "inspect or clean prompt-cache state",
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

    /// What it does while a turn is running.
    ///
    /// The runner that every one of these would act on is on the worker thread
    /// for the length of a turn, so nothing here reaches it. The commands that
    /// move nothing but the screen open and apply live; `/model` opens and its
    /// pick is held for the turn the loop starts next; the rest are refused,
    /// each with the one-line reason that is its own. A command added here
    /// decides which of the three it is in the same place it names itself.
    const fn mid_turn(self) -> MidTurn {
        match self {
            Self::Help | Self::Theme => MidTurn::Live,
            // The mode is a ladder, not a picker: shift+tab steps it mid-turn,
            // and the panel between turns. So mid-turn the key is the way in,
            // and the command is told the same — stepped to and held for the
            // turn the loop starts next, the change a running turn's gate
            // cannot take.
            Self::Model | Self::Mode => MidTurn::Deferred,
            Self::Effort => {
                MidTurn::Refused("sets how hard it thinks, which the running turn has taken")
            }
            Self::Login => MidTurn::Refused("adds a key the request now in flight cannot use"),
            Self::Logout => {
                MidTurn::Refused("removes the key the request now in flight is signed with")
            }
            Self::Resume => MidTurn::Refused("leaves this session for an earlier one, mid-answer"),
            Self::Cache => MidTurn::Refused("inspects provider state held by the running request"),
            Self::Compact => {
                MidTurn::Refused("cuts the window the turn now running is answering in")
            }
            Self::Clear => MidTurn::Refused("starts a new session, leaving the one being answered"),
            Self::Exit => MidTurn::Refused("ends the session, turn and all"),
        }
    }
}

/// What a command does while a turn is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MidTurn {
    /// Opens and applies now: it moves nothing but the screen.
    Live,
    /// Opens now and takes effect on the turn started next.
    Deferred,
    /// Does nothing, with the reason it cannot said on a panel instead.
    Refused(&'static str),
}

/// What `line` asked for, or `None` where it asked for no command at all.
///
/// The whole of the parsing, done once, here. Everything downstream has either
/// a [`Command`] or a word already known to be a slash and letters, which is
/// what makes an unknown one safe to say back: it cannot be carrying an escape
/// sequence, because a word carrying one is not shaped like a command and never
/// reaches this far.
pub(super) fn wanted<'a>(commands: &Commands, line: &'a str) -> Option<Wanted<'a>> {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));

    if !shaped(word) {
        return None;
    }

    Some(match named(commands, word) {
        Some(command) => Wanted::Known {
            command,
            rest: rest.trim(),
        },
        None => Wanted::Unknown(word),
    })
}

/// Runs a command that moves nothing but the screen, with a turn behind it.
///
/// The picker and the list are the same ones the between-turns command opens;
/// `while_waiting` is what differs. It is the turn's drain, run once a pass so
/// the transcript goes on rendering while the panel stands, and it is the
/// reason this is reached from the mid-turn loop rather than from `run`.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn live<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    wanted: &Owned,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<(), Fatal> {
    let style = terms.style();
    let rest = match wanted {
        Owned::Known { rest, .. } => rest.as_str(),
        Owned::Unknown => "",
    };
    match wanted.command() {
        Command::Theme => theme::live(renderer, terms, rest, while_waiting),
        Command::Help => {
            let commands = terms.commands.snapshot();
            // No keys to read: the list is stood, and any key closes it.
            region::stand_while(
                renderer,
                |_| style,
                &mut Still,
                |_, columns, _| (listing(&commands, columns, style.glyphs()), None),
                |arrived, _| {
                    if matches!(arrived, Pressed::Resized) {
                        Moved::Redraw
                    } else {
                        Moved::Left
                    }
                },
                while_waiting,
            )?;
            Ok(())
        }
        // The classifier decides which commands reach here; a live one this
        // arm does not name is a build error at the match, not a silent skip.
        _ => Ok(()),
    }
}

/// The stateless marker a panel with nothing to hold is stood with.
struct Still;

/// Runs a command whose pick the turn started next applies.
///
/// `/model` is the one of these. The picker opens over the running turn and the
/// consequence is said and agreed to before the pick is held; the running turn
/// keeps the model it started with. What is taken is held rather than applied,
/// and the loop applies it when the runner is this side's again.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
/// What a deferred command leaves held for the next turn.
pub(super) enum Kept {
    /// A model picked and confirmed, to be applied when the runner is back.
    Model(Served, String),
}

pub(super) fn deferred<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    current: &str,
    wanted: &Owned,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Option<Kept>, Fatal> {
    match &wanted {
        // A loop rather than a sequence, because "go back" returns to the
        // picker, and a pick is only held once it has been confirmed.
        Owned::Known {
            command: Command::Model,
            ..
        } => loop {
            let picked = model::picked_while(renderer, terms, current, while_waiting)?;
            let picking::Taken::Took(selected) = picked else {
                // Left, or no room for a panel: nothing is held.
                return Ok(None);
            };

            if model::confirmed(renderer, terms, selected, while_waiting)? {
                let (provider, name) = selected.parts();
                return Ok(Some(Kept::Model(provider, name)));
            }
            // "go back": round to the picker.
        },
        // The mode is stepped by shift+tab, which is its own mid-turn way in;
        // `/mode` here has no picker to stand, so it is the step the key would
        // make, held for the next turn the same way. The loop holds it.
        // `/mode` is stepped by shift+tab, which is its own mid-turn way in;
        // it has no picker to stand here, so the step is the key's, held for
        // the next turn the same way — and the loop holds it. Every other
        // command the classifier does not route here holds nothing either.
        _ => Ok(None),
    }
}

/// Applies a model picked mid-turn, at the start of the turn it was held for.
///
/// Reached from the loop, not the keyboard: the runner is back from the worker,
/// and the pick made over the running turn is the one this turn is asked under.
pub(super) fn apply_model<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    provider: Served,
    name: &str,
) -> Result<(), Fatal> {
    model::apply(renderer, runner, terms, provider, name)
}

/// Stands why a command cannot run now over the box until escape closes it.
///
/// Where the box was, as every panel is: the rule, the command's name, the one
/// reason it cannot run while a turn is, and the key that closes it. The turn
/// goes on above — the panel stands where the working row, the box, the status
/// and the map were, and the transcript keeps its own rows. Nothing of the turn
/// changes: the command did nothing, and this is the whole of what happened.
pub(super) fn refused<T: Terminal>(
    renderer: &mut Renderer<T>,
    command: Command,
    why: &'static str,
    style: Style,
) -> Result<Option<&'static str>, Fatal> {
    region::stand(
        renderer,
        |_| style,
        &mut Still,
        |_, columns, _| {
            let glyphs = style.glyphs();
            let mut rows = vec![
                Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
                Row::new(),
                Row::new().then(Slot::Strong, command.name()),
            ];
            rows.extend(
                fold(why, columns)
                    .into_iter()
                    .map(|line| Row::new().then(Slot::Plain, line)),
            );
            rows.push(Row::new());
            rows.push(Row::new().then(Slot::Quiet, "esc to close"));
            (rows, None)
        },
        |arrived, _| {
            // Matching the key rather than the state: the panel holds nothing,
            // so only the press decides what the loop does with it.
            if matches!(
                arrived,
                Pressed::Escape | Pressed::Key(Key::Enter | Key::Interrupt | Key::Eof)
            ) {
                Moved::Left
            } else if arrived == Pressed::Resized {
                Moved::Redraw
            } else {
                Moved::Still
            }
        },
    )?;
    Ok(None)
}

/// A command read mid-turn, owned so it crosses from the keyboard loop to the
/// turn's own. `None` where the line is no command, the same as [`wanted`].
pub(super) fn owned(commands: &Commands, line: &str) -> Option<Owned> {
    wanted(commands, line).map(|wanted| match wanted {
        Wanted::Known { command, rest } => Owned::Known {
            command,
            rest: rest.to_owned(),
        },
        Wanted::Unknown(_) => Owned::Unknown,
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
pub(super) fn filtering(commands: &Commands, line: &str, glyphs: Glyphs) -> Vec<Listed<'static>> {
    if !shaped(line) {
        return Vec::new();
    }

    commands
        .entries()
        .iter()
        .map(|slash| slash.command)
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
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Ran, Fatal> {
    // Nothing is drawn on the way out. The loop is about to end and the shell's
    // own prompt is the next thing on the screen; a row saying goodbye is a row
    // between the two.
    if leaves(wanted) {
        return Ok(Ran::Leave);
    }

    // Directly under the line that asked, with nothing between: the answer is
    // hung off that line by the mark in front of it, and a blank row between
    // the two would leave the mark pointing at nothing. The blank goes after,
    // where the next block starts — the box below is already parted from it,
    // and the next thing said belongs under the pair rather than in it.
    let start = renderer.lines();
    let making = answer(wanted, renderer, runner, held, terms)?;
    renderer.subordinate(start, terms.style().glyphs())?;
    renderer.commit("")?;

    Ok(making.map_or(Ran::Again, Ran::Room))
}

/// What one command has to say, drawn.
fn answer<T: Terminal>(
    wanted: Wanted<'_>,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
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
        } => renderer.present(&listing(&terms.commands.snapshot(), columns, glyphs))?,

        // Nothing is drawn for it here. What it asks for is run above, where a
        // request is run, and everything a reader sees of one comes from there.
        Wanted::Known {
            command: Command::Compact,
            ..
        } => return Ok(Some(Compacting::Asked)),

        Wanted::Known {
            command: Command::Model,
            rest,
        } => model::run(rest, renderer, runner, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Effort,
            rest,
        } => effort::run(rest, renderer, runner, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Login,
            rest,
        } => login::run(rest, renderer, runner, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Logout,
            rest,
        } => logout::run(rest, renderer, runner, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Mode,
            rest,
        } => moded(rest, renderer, runner, style)?,

        Wanted::Known {
            command: Command::Theme,
            rest,
        } => theme::run(rest, renderer, terms, held.answers.keys)?,

        // The one other command that can end in a request: a session picked up
        // is put to the reader before it is carried, and one of the three
        // answers costs one.
        Wanted::Known {
            command: Command::Resume,
            rest,
        } => return resume::run(rest, renderer, runner, held, terms),

        Wanted::Known {
            command: Command::Cache,
            rest,
        } => cache::run(rest, renderer, runner)?,

        Wanted::Known {
            command: Command::Clear,
            ..
        } => clear::run(renderer, runner, held, terms)?,

        Wanted::Unknown(word) => {
            renderer.commit(&format!("! no such command: {word}"))?;
            renderer.commit("")?;
            renderer.present(&listing(&terms.commands.snapshot(), columns, glyphs))?;
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

/// The columns the mark an answer is hung under takes off every row of it:
/// the mark, one column in either glyph set, and the space after it.
const HUNG: usize = 2;

/// Says one thing back, quietly, wrapped to the window it is said in.
///
/// What `/login` and `/logout` answer with when there is one thing to say: a
/// credential was stored, removed, or left alone with the reason why. Wrapped
/// rather than cut, because the reason is at the end of the sentence and is
/// the part somebody asked for; the caller that hangs the answer under the
/// command indents whatever ran over. Wrapped short of the mark, too: the
/// rows are hung after they are laid, and a row folded to the whole width is
/// [`HUNG`] columns too wide once it is.
fn say<T: Terminal>(renderer: &mut Renderer<T>, said: &str) -> Result<(), Fatal> {
    let rows: Vec<Row> = fold(said, renderer.columns().saturating_sub(HUNG))
        .into_iter()
        .map(|part| Row::new().then(Slot::Quiet, part))
        .collect();

    Ok(renderer.present(&rows)?)
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
fn listing(commands: &Commands, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let shown: Vec<Listed<'static>> = commands
        .entries()
        .iter()
        .map(|one| one.command.listed(glyphs))
        .collect();

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

/// The command that word names, in this generation of the registry.
fn named(commands: &Commands, word: &str) -> Option<Command> {
    commands.find(word).map(|slash| slash.command)
}

/// Whether this word is shaped like a command name: a slash, then letters, and
/// nothing else.
///
/// It is what keeps a prompt that opens with a path a prompt. `/etc/hosts is
/// wrong` is a sentence about a file and `/Users/me/notes.md` is a file, and
/// neither is read as a command that happens not to exist. A line is only ever
/// taken for a command where it could not be anything else.
pub(super) fn shaped(word: &str) -> bool {
    match word.strip_prefix('/') {
        Some(rest) => rest.chars().all(|one| one.is_ascii_alphabetic()),
        None => false,
    }
}

#[cfg(test)]
mod tests;
