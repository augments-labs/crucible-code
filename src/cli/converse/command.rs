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

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::providers::Served;
use crucible_client_api as api;
use crucible_models::{FastForm, Speed};
use crucible_registry::{
    Collision, Provenance, Registered, Registry, RegistryError, RegistrySnapshot, SourceKind,
};
use crucible_tools::Mode;
use crucible_tui::{Glyphs, Key, Listed, Menu, Pressed, Renderer, Row, Slot, Terminal, clip, fold};
use crucible_types::Compacting;

use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::style::Style;

use super::region::{self, Moved};
use super::{Held, Terms, mode, picking, warning};

mod cache;
mod clear;
mod context;
mod effort;
mod fast;
mod login;
mod logout;
mod model;
pub(crate) mod notes;
mod resume;
mod sandbox;
mod theme;

/// What a line beginning `/` can ask for.
///
/// Closed, and matched arm by arm where it is run, so a command added here is a
/// compile error until it has been given something to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Command {
    /// What these are.
    Help,
    /// Every release crucible has had, or one of them in full.
    ReleaseNotes,
    /// How the window of the next request is spent, part by part.
    Context,
    /// Which model answers.
    Model,
    /// How hard it is asked to think.
    Effort,
    /// How fast it is asked to answer, where its vendor serves a fast form.
    Fast,
    /// A key for a provider, given to a box that does not echo it.
    Login,
    /// An account or API key Crucible stored, removed.
    Logout,
    /// The permission mode: the one in force, or the one named.
    Mode,
    /// Inspect and configure operating-system confinement.
    Sandbox,
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
const EVERY: [Command; 16] = [
    Command::Help,
    Command::ReleaseNotes,
    Command::Context,
    Command::Model,
    Command::Effort,
    Command::Fast,
    Command::Login,
    Command::Logout,
    Command::Mode,
    Command::Sandbox,
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
    enablement: Option<std::sync::Arc<crucible_sandbox::SandboxEnablement>>,
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
            enablement: None,
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
pub(crate) fn builtins(
    enablement: &std::sync::Arc<crucible_sandbox::SandboxEnablement>,
) -> Result<Registry<Slash>, RegistryError> {
    let registry = Registry::new(Collision::Refuse);
    let mut staged = registry.stage();
    for command in EVERY {
        let mut slash = Slash::builtin(command)?;
        if command == Command::Sandbox {
            slash.enablement = Some(std::sync::Arc::clone(enablement));
        }
        staged.register(slash)?;
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
    /// A word shaped like a command that names none, and the two rows it is
    /// refused with, which the panel says in the words the transcript would.
    Unknown([String; 2]),
}

impl Owned {
    /// Which command this is, or `Exit` for a word that names none. `Exit` is a
    /// stand-in no caller runs: the class of a word that names none is a
    /// refusal, and [`refused`] says the word itself back rather than a name.
    pub(super) fn command(&self) -> Command {
        match self {
            Self::Known { command, .. } => *command,
            Self::Unknown(_) => Command::Exit,
        }
    }

    /// What it does while a turn is running: the command's own class, or a
    /// refusal for a word that names none.
    pub(super) fn class(&self) -> MidTurn {
        match self {
            Self::Known { command, .. } => command.mid_turn(),
            Self::Unknown(_) => MidTurn::Refused("names no command"),
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
            Self::ReleaseNotes => "/release-notes",
            Self::Context => "/context",
            Self::Model => "/model",
            Self::Effort => "/effort",
            Self::Fast => "/fast",
            Self::Login => "/login",
            Self::Logout => "/logout",
            Self::Mode => "/mode",
            Self::Sandbox => "/sandbox",
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
            Self::ReleaseNotes => "what changed in each release",
            Self::Context => "what fills the model's window",
            Self::Model => "pick which model answers",
            Self::Effort => "pick how hard it thinks",
            Self::Fast => "pick how fast it answers",
            // How you are signed in, rather than what crucible signs with. A
            // key is one of the ways in and the row is read by somebody who
            // does not know yet which of them is theirs.
            Self::Login => "sign in to a provider account",
            Self::Logout => "remove a stored account or API key",
            // The ring itself rather than a sentence about it. `/mode` is the
            // one command that takes a word after it, and the words it takes
            // are the useful half of what there is to say.
            Self::Mode => mode::ring(glyphs),
            Self::Sandbox => "inspect or configure sandbox confinement",
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
            Self::Help | Self::Theme | Self::Context => MidTurn::Live,
            Self::Sandbox => {
                MidTurn::Refused("changes the policy for new commands; open it between turns")
            }
            // The mode is a ladder, not a picker: shift+tab steps it mid-turn,
            // and the panel between turns. So mid-turn the key is the way in,
            // and the command is told the same — stepped to and held for the
            // turn the loop starts next, the change a running turn's gate
            // cannot take.
            //
            // The speed belongs to the next request, as the model does: the
            // `/fast` panel opens now and the speed taken is asked for once
            // the turn ends.
            Self::Model | Self::Mode | Self::Fast => MidTurn::Deferred,
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
            // Refused rather than printed under the tail: a thousand rows
            // would part the answer being written, and they will be there to
            // print once it is done.
            Self::ReleaseNotes => {
                MidTurn::Refused("prints a thousand rows into the answer being written")
            }
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
/// a [`Command`] or a word already known to be a slash, letters and hyphens,
/// which is what makes an unknown one safe to say back: it cannot be carrying
/// an escape sequence, because a word carrying one is not shaped like a command
/// and never reaches this far.
///
/// An unknown word is only ever the whole line. `/tmp is full` is a sentence
/// that happens to open with a directory, and refusing it would be refusing to
/// send somebody's question; a word typed alone is the one line that could
/// only have been meant as a command.
pub(super) fn wanted<'a>(commands: &Commands, line: &'a str) -> Option<Wanted<'a>> {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));

    if !shaped(word) {
        return None;
    }

    match named(commands, word) {
        Some(command) => Some(Wanted::Known {
            command,
            rest: rest.trim(),
        }),
        None if rest.is_empty() => Some(Wanted::Unknown(word)),
        None => None,
    }
}

/// Runs a command that moves nothing but the screen, with a turn behind it.
///
/// The picker, the list and the panel are the same ones the between-turns
/// command opens; `while_waiting` is what differs. It is the turn's drain, run
/// once a pass so the transcript goes on rendering while the panel stands, and
/// it is the reason this is reached from the mid-turn loop rather than from
/// `run`. `counted` is the window as the running turn last divided it, which
/// `/context` shows because the runner is away on the turn.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn live<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    wanted: &Owned,
    counted: &api::Context,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<(), Fatal> {
    let style = terms.style();
    let rest = match wanted {
        Owned::Known { rest, .. } => rest.as_str(),
        Owned::Unknown(_) => "",
    };
    match wanted.command() {
        Command::Theme => theme::live(renderer, terms, rest, while_waiting),
        Command::Context => context::live(renderer, terms, counted, while_waiting),
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

/// Runs a command whose pick is held for the turn started next.
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
    /// A speed taken, to be asked for when the runner is back.
    Speed(Speed),
}

/// Who is answering and for which model, by name: what a panel stood while the
/// runner is away is told instead of reading it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asked<'a> {
    /// The provider's name in the registry, where one is answering.
    pub(super) provider: Option<&'static str>,
    /// The model in force, empty where none is.
    pub(super) model: &'a str,
    /// The rung it is asked on, where one is in force.
    pub(super) effort: Option<&'a str>,
    /// How fast the model is asked to answer, and was last served.
    pub(super) pace: Pace,
}

/// How the model in force is asked to answer fast, the speed it is asked at,
/// and whether the last answer was served fast: read off the runner while it
/// is this side's, for what is drawn while it is away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Pace {
    /// The model's fast form, as its provider answers.
    pub(super) form: FastForm,
    /// The speed it is asked at.
    pub(super) speed: Speed,
    /// Whether the last answer was served fast.
    pub(super) served: bool,
}

impl Pace {
    /// The pace of the model `runner` asks.
    pub(super) fn of(runner: &crucible_runner::Runner) -> Self {
        Self {
            form: runner.provider().fast(runner.model()),
            speed: runner.speed(),
            served: runner.served().fast(),
        }
    }
}

pub(super) fn deferred<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    current: Asked<'_>,
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
                match warning::choosing(
                    renderer,
                    terms,
                    (provider.name, &name),
                    true,
                    while_waiting,
                )? {
                    warning::Chosen::Take => return Ok(Some(Kept::Model(provider, name))),
                    warning::Chosen::Back => {}
                    warning::Chosen::Stop(said) => {
                        renderer.commit(&said)?;
                        return Ok(None);
                    }
                }
            }
            // "go back": round to the picker.
        },
        Owned::Known {
            command: Command::Fast,
            ..
        } => Ok(fast::picked_while(renderer, terms, current, while_waiting)?.map(Kept::Speed)),
        // `/mode` has no picker to stand here: mid-turn it makes the step
        // shift+tab would, and the loop holds it for the next turn. Every other
        // command the classifier does not route here holds nothing either.
        _ => Ok(None),
    }
}

/// Applies a model picked mid-turn, as the turn it was picked over ends.
///
/// Reached from the loop, not the keyboard: the runner is back from the worker,
/// and the pick made over the running turn is the one the next turn is asked
/// under.
pub(super) fn apply_model<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    provider: Served,
    name: &str,
) -> Result<(), Fatal> {
    model::apply(renderer, conversation, terms, provider, name)
}

/// Asks for a speed taken mid-turn, as the turn it was taken over ends.
pub(super) fn apply_speed<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    speed: Speed,
) -> Result<(), Fatal> {
    fast::taken(speed, renderer, conversation, terms)
}

/// Stands why a command cannot run now over the box until escape closes it.
///
/// Where the box was, as every panel is: the rule, the command's name, the one
/// reason it cannot run while a turn is, and the key that closes it. A word
/// that names no command has neither, and is said back with the names nearest
/// to it, as it would be between turns. The turn
/// goes on above — the panel stands where the working row, the box and the
/// status were, and the transcript keeps its own rows. Nothing of the turn
/// changes: the command did nothing, and this is the whole of what happened.
pub(super) fn refused<T: Terminal>(
    renderer: &mut Renderer<T>,
    wanted: &Owned,
    why: &'static str,
    style: Style,
) -> Result<Option<&'static str>, Fatal> {
    region::stand(
        renderer,
        |_| style,
        &mut Still,
        |_, columns, _| (refusing(wanted, why, columns, style.glyphs()), None),
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

/// The rows of the panel [`refused`] stands: the rule, what was asked for,
/// why it cannot run now, and the key that closes it.
///
/// A word that names no command has no name to head the panel and no reason of
/// its own, so it is said back the way the transcript says it between turns:
/// the word, and the names it was nearest to.
fn refusing(wanted: &Owned, why: &'static str, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut rows = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
    ];
    let said = match wanted {
        Owned::Known { command, .. } => {
            rows.push(Row::new().then(Slot::Strong, command.name()));
            vec![why]
        }
        Owned::Unknown(refused) => refused.iter().map(String::as_str).collect(),
    };
    // Folded after the indent a line opens with, so the names under a refused
    // word stay under its words rather than back at the edge.
    rows.extend(said.into_iter().flat_map(|line| {
        let words = line.trim_start();
        let gap = line.len() - words.len();
        // A window no wider than the indent keeps none of it, so the words
        // still have a column to be drawn in.
        let indent = " ".repeat(if gap < columns { gap } else { 0 });
        fold(words, columns - indent.len())
            .into_iter()
            .map(move |part| Row::new().then(Slot::Plain, format!("{indent}{part}")))
    }));
    rows.push(Row::new());
    rows.push(Row::new().then(Slot::Quiet, "esc to close"));
    rows
}

/// A command read mid-turn, owned so it crosses from the keyboard loop to the
/// turn's own. `None` where the line is no command, the same as [`wanted`].
pub(super) fn owned(commands: &Commands, line: &str) -> Option<Owned> {
    wanted(commands, line).map(|wanted| match wanted {
        Wanted::Known { command, rest } => Owned::Known {
            command,
            rest: rest.to_owned(),
        },
        Wanted::Unknown(word) => Owned::Unknown(refusal(commands, word)),
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
        .filter(|slash| slash.command.name().starts_with(line))
        .map(|slash| {
            let mut listed = slash.command.listed(glyphs);
            if let Some(enablement) = &slash.enablement {
                listed.says = if enablement.enabled() {
                    "sandbox enabled (enter to configure)"
                } else {
                    "sandbox disabled (enter to configure)"
                };
            }
            listed
        })
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
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Ran, Fatal> {
    // Nothing is drawn on the way out. The loop is about to end and the shell's
    // own prompt is the next thing on the screen; a row saying goodbye is a row
    // between the two.
    if leaves(wanted) {
        return Ok(Ran::Leave);
    }

    // The one answer not hung off the line that asked: a timeline has a rail
    // of its own down the left, and a thousand rows indented under a mark
    // would be a second one beside it. One release and the refusals are set
    // apart the same way, as the list's look draws them.
    if let Wanted::Known {
        command: Command::ReleaseNotes,
        rest,
    } = wanted
    {
        notes::run(rest, renderer, terms.style().glyphs())?;
        renderer.commit("")?;
        return Ok(Ran::Again);
    }

    // Directly under the line that asked, with nothing between: the answer is
    // hung off that line by the mark in front of it, and a blank row between
    // the two would leave the mark pointing at nothing. The blank goes after,
    // where the next block starts — the box below is already parted from it,
    // and the next thing said belongs under the pair rather than in it.
    let start = renderer.lines();
    let making = answer(wanted, renderer, conversation, held, terms)?;
    renderer.subordinate(start, terms.style().glyphs())?;
    renderer.commit("")?;

    Ok(making.map_or(Ran::Again, Ran::Room))
}

/// What one command has to say, drawn.
fn answer<T: Terminal>(
    wanted: Wanted<'_>,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let columns = renderer.transcript_columns();
    let style = terms.style();
    let glyphs = style.glyphs();

    match wanted {
        // Answered by `run`, which returns before this is reached: `/exit`
        // ends the conversation, and `/release-notes` is printed without being
        // hung under the line that asked. Spelled out rather than left to a
        // wildcard, so a command added later stops the build here instead of
        // running and saying nothing.
        Wanted::Known {
            command: Command::Exit | Command::ReleaseNotes,
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
        } => model::run(rest, renderer, conversation, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Effort,
            rest,
        } => effort::run(rest, renderer, conversation, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Fast,
            rest,
        } => fast::run(rest, renderer, conversation, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Login,
            rest,
        } => login::run(rest, renderer, conversation, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Logout,
            rest,
        } => logout::run(rest, renderer, conversation, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Mode,
            rest,
        } => moded(rest, renderer, conversation, terms)?,

        Wanted::Known {
            command: Command::Theme,
            rest,
        } => theme::run(rest, renderer, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Sandbox,
            rest,
        } => sandbox::run(rest, renderer, (conversation, terms), held.answers.keys)?,

        // The one other command that can end in a request: a session picked up
        // is put to the reader before it is carried, and one of the three
        // answers costs one.
        Wanted::Known {
            command: Command::Resume,
            rest,
        } => return resume::run(rest, renderer, conversation, held, terms),

        Wanted::Known {
            command: Command::Cache,
            rest,
        } => cache::run(rest, renderer, conversation, terms)?,

        Wanted::Known {
            command: Command::Context,
            ..
        } => context::run(renderer, conversation, terms, held.answers.keys)?,

        Wanted::Known {
            command: Command::Clear,
            ..
        } => clear::run(renderer, conversation, held, terms)?,

        Wanted::Unknown(word) => {
            for row in refusal(&terms.commands.snapshot(), word) {
                renderer.commit(&row)?;
            }
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
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let style = terms.style();
    let columns = renderer.transcript_columns();
    let ring = Row::new().then(Slot::Quiet, clip(mode::ring(style.glyphs()), columns));

    if said.is_empty() {
        let rows = [sentence(conversation.runner().mode(), columns), ring];
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

    let asking = api::Command::SetMode(crucible_app::client::mode(asked));
    match terms.perform(conversation, asking) {
        Performed::Mode(taken) => renderer.present(&[sentence(taken, columns)])?,
        other => renderer.commit(&astray(&other))?,
    }
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
    let rows: Vec<Row> = fold(said, renderer.transcript_columns().saturating_sub(HUNG))
        .into_iter()
        .map(|part| Row::new().then(Slot::Quiet, part))
        .collect();

    Ok(renderer.present(&rows)?)
}

/// A thing and what is said about it, parted by the mark that says they are two.
///
/// The listing a run with no keyboard is given in place of a panel is read that
/// way — a command down the left, what taking it would reach after the mark. So
/// is the row beneath a sign-in that has not finished, which is a state and the
/// key that leaves it, and so is the answer to a key that could not be kept,
/// which is what went wrong and then the way back in.
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

/// How many names a slip is answered with, at most. More than this is the
/// list again, and the list is what `/help` is for.
const NEAREST: usize = 3;

/// The registered names nearest to a word that names none, nearest first.
pub(super) fn nearest(commands: &Commands, word: &str) -> Vec<&'static str> {
    nearest_among(
        commands.entries().iter().map(|slash| slash.command.name()),
        word,
    )
}

/// The names of `names` nearest to `word`, nearest first, at most [`NEAREST`].
///
/// Near is a few edits away: a letter put in, taken out or changed, or two
/// neighbours swapped, which between them are the slips a hand makes. A longer
/// name tolerates more of them, one more for every five of its letters past
/// the first four, so `/hlep` finds `/help` while `/zzz` finds nothing. Of
/// names equally near, the one nearest the word in length comes first, since a
/// slip that kept the length changed a letter rather than dropping one; then
/// the order the names were given in.
fn nearest_among(names: impl Iterator<Item = &'static str>, word: &str) -> Vec<&'static str> {
    let mut near: Vec<(usize, usize, usize, &'static str)> = names
        .enumerate()
        .filter_map(|(listed, name)| {
            let edits = apart(word, name);
            let letters = name.len().saturating_sub(1);
            let tolerated = 1 + letters.saturating_sub(4) / 5;
            (edits > 0 && edits <= tolerated)
                .then(|| (edits, name.len().abs_diff(word.len()), listed, name))
        })
        .collect();
    near.sort_unstable();
    near.into_iter()
        .take(NEAREST)
        .map(|(.., name)| name)
        .collect()
}

/// How many single edits turn `word` into `name`: a character put in, taken
/// out or changed, or two neighbours swapped.
fn apart(word: &str, name: &str) -> usize {
    let word: Vec<char> = word.chars().collect();
    let name: Vec<char> = name.chars().collect();
    // Out of the table reads as too far to matter, so a slip at an edge is
    // never the cheapest way through.
    let at = |row: &[usize], column: usize| row.get(column).copied().unwrap_or(usize::MAX / 2);

    let mut earlier = vec![0; name.len() + 1];
    let mut last: Vec<usize> = (0..=name.len()).collect();
    for (typed, one) in word.iter().enumerate() {
        let mut row = Vec::with_capacity(name.len() + 1);
        row.push(typed + 1);
        for (listed, other) in name.iter().enumerate() {
            let mut best = (at(&last, listed) + usize::from(one != other))
                .min(at(&last, listed + 1) + 1)
                .min(at(&row, listed) + 1);
            let swapped = typed > 0
                && listed > 0
                && word.get(typed - 1) == Some(other)
                && name.get(listed - 1) == Some(one);
            if swapped {
                best = best.min(at(&earlier, listed - 1) + 1);
            }
            row.push(best);
        }
        earlier = std::mem::replace(&mut last, row);
    }
    at(&last, name.len())
}

/// The two rows a word that names no command is refused with: the word, and
/// the names it was nearest to, or where the whole list is when it was near
/// none of them.
///
/// The word is said back as it came, because only a word shaped like a
/// command gets this far and such a word has nothing in it to write to the
/// terminal but a slash, letters and hyphens.
pub(super) fn refusal(commands: &Commands, word: &str) -> [String; 2] {
    let near = nearest(commands, word);
    let then = if near.is_empty() {
        format!("  {} lists every command", Command::Help.name())
    } else {
        format!("  nearest: {}", near.join(", "))
    };
    [format!("! no such command: {word}"), then]
}

/// Every name a line may open with, in this generation of the registry, for
/// the box to know its first word by.
pub(super) fn names(commands: &Commands) -> Vec<&'static str> {
    commands
        .entries()
        .iter()
        .map(|slash| slash.command.name())
        .collect()
}

/// The command that word names, in this generation of the registry.
fn named(commands: &Commands, word: &str) -> Option<Command> {
    commands.find(word).map(|slash| slash.command)
}

/// Whether this word is shaped like a command name: a slash, then an ASCII
/// letter, then ASCII letters and hyphens, and nothing else. A bare slash is the key that
/// opens the list, and passes too.
///
/// It is what keeps a prompt that opens with a path a prompt. `/etc/hosts is
/// wrong` is a sentence about a file and `/Users/me/notes.md` is a file, and
/// neither is read as a command that happens not to exist. A line is only ever
/// taken for a command where it could not be anything else.
pub(super) fn shaped(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('/') else {
        return false;
    };
    let mut letters = rest.chars();
    match letters.next() {
        None => true,
        Some(first) => {
            first.is_ascii_alphabetic()
                && letters.all(|one| one.is_ascii_alphabetic() || one == '-')
        }
    }
}

#[cfg(test)]
mod tests;
