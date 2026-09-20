//! What a front end may ask the application to do.
//!
//! One arm for each thing the application already does for a person at a
//! terminal, and none besides: a command that does not ship has no arm, so
//! there is no spelling a decoder could take for it.
//!
//! Nothing here reaches the host's files. A prompt is its words; what is
//! attached to it is chosen on the host, by the front end standing there, and
//! handed to the application beside the request rather than named inside it. A
//! login names a provider and never the credential: the secret is stored on
//! the host, and the command says only that it now can be read.

use std::fmt;
use std::str::FromStr;

use crucible_types::SessionId;
use serde_json::Value;

use crate::bounds::{Name, PROMPT_BYTES};
use crate::error::{ErrorCode, Refusal};
use crate::pending::Decision;
use crate::wire::{Fields, Writing};

/// One thing asked of the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Take a turn over these words.
    Prompt(Prompt),
    /// Make room in the conversation by replacing it with a recap.
    Compact,
    /// Stop the turn that is running.
    Cancel,
    /// Answer the action a turn is stopped on.
    ///
    /// The form a decision travels in. A decision settles through the front
    /// end the stopped turn is asking, while that turn is stopped, and what it
    /// comes to is the turn's own outcome. One that arrives as a command of
    /// its own finds no turn stopped and nothing pending, and is refused as
    /// [`StaleDecision`](crate::ErrorCode::StaleDecision).
    Decide(Decision),
    /// Start a new session, leaving this one where a resume can find it.
    Clear,
    /// Pick a recorded session of this workspace back up.
    Resume(SessionId),
    /// Ask this model of this provider from the next turn on.
    SelectModel {
        /// The provider, by the name its registry gives it.
        provider: Name,
        /// The model, by the name its vendor gives it.
        model: Name,
        /// How hard to think, where the choice is made with the model.
        effort: Option<Rung>,
    },
    /// Think this hard from the next turn on.
    SetEffort(Rung),
    /// Decide tool calls under this mode from the next one on.
    SetMode(Mode),
    /// Step to the next permission mode.
    CycleMode,
    /// Adopt the credential just stored on the host for this provider.
    Login {
        /// The provider, by the name its registry gives it.
        provider: Name,
    },
    /// Forget the credential stored on the host for this provider.
    Logout {
        /// The provider, by the name its registry gives it.
        provider: Name,
    },
    /// List the persistent prompt-cache resources this conversation holds.
    InspectCache,
    /// Delete them.
    CleanCache,
    /// Require, or stop requiring, the sandbox for new processes.
    Sandbox {
        /// Whether it is to be required.
        enabled: bool,
    },
    /// Remember how this machine's sessions are drawn.
    Theme(Theme),
    /// Name the commands that ship.
    Help,
    /// Leave the conversation.
    Exit,
}

impl Command {
    /// The word each arm crosses as, in the order the arms are declared.
    pub const KINDS: [&'static str; 18] = [
        "prompt",
        "compact",
        "cancel",
        "decide",
        "clear",
        "resume",
        "select_model",
        "set_effort",
        "set_mode",
        "cycle_mode",
        "login",
        "logout",
        "inspect_cache",
        "clean_cache",
        "sandbox",
        "theme",
        "help",
        "exit",
    ];

    /// The word this command crosses as.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Prompt(_) => "prompt",
            Self::Compact => "compact",
            Self::Cancel => "cancel",
            Self::Decide(_) => "decide",
            Self::Clear => "clear",
            Self::Resume(_) => "resume",
            Self::SelectModel { .. } => "select_model",
            Self::SetEffort(_) => "set_effort",
            Self::SetMode(_) => "set_mode",
            Self::CycleMode => "cycle_mode",
            Self::Login { .. } => "login",
            Self::Logout { .. } => "logout",
            Self::InspectCache => "inspect_cache",
            Self::CleanCache => "clean_cache",
            Self::Sandbox { .. } => "sandbox",
            Self::Theme(_) => "theme",
            Self::Help => "help",
            Self::Exit => "exit",
        }
    }

    pub(crate) fn written(&self) -> Value {
        let object = Writing::kind(self.kind());
        match self {
            Self::Prompt(prompt) => object.with("text", prompt.as_str()),
            Self::Decide(decision) => object.with("decision", decision.written()),
            Self::Resume(session) => object.with("session", session.as_str()),
            Self::SelectModel {
                provider,
                model,
                effort,
            } => object
                .with("provider", provider.as_str())
                .with("model", model.as_str())
                .maybe("effort", effort.map(Rung::as_str)),
            Self::SetEffort(rung) => object.with("effort", rung.as_str()),
            Self::SetMode(mode) => object.with("mode", mode.as_str()),
            Self::Login { provider } | Self::Logout { provider } => {
                object.with("provider", provider.as_str())
            }
            Self::Sandbox { enabled } => object.with("enabled", *enabled),
            Self::Theme(theme) => object.with("part", theme.part()).with("name", theme.name()),
            Self::Compact
            | Self::Cancel
            | Self::Clear
            | Self::CycleMode
            | Self::InspectCache
            | Self::CleanCache
            | Self::Help
            | Self::Exit => object,
        }
        .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let kind = fields.kind()?;
        let command = match kind.as_str() {
            "prompt" => Self::Prompt(Prompt::new(&fields.string("text")?)?),
            "compact" => Self::Compact,
            "cancel" => Self::Cancel,
            "decide" => Self::Decide(Decision::read(fields.take("decision")?)?),
            "clear" => Self::Clear,
            "resume" => Self::Resume(session(&fields.string("session")?)?),
            "select_model" => Self::SelectModel {
                provider: fields.name("provider")?,
                model: fields.name("model")?,
                effort: fields.maybe("effort").as_ref().map(word).transpose()?,
            },
            "set_effort" => Self::SetEffort(word(&fields.take("effort")?)?),
            "set_mode" => Self::SetMode(word(&fields.take("mode")?)?),
            "cycle_mode" => Self::CycleMode,
            "login" => Self::Login {
                provider: fields.name("provider")?,
            },
            "logout" => Self::Logout {
                provider: fields.name("provider")?,
            },
            "inspect_cache" => Self::InspectCache,
            "clean_cache" => Self::CleanCache,
            "sandbox" => Self::Sandbox {
                enabled: fields.flag("enabled")?,
            },
            "theme" => {
                let part = fields.string("part")?;
                Self::Theme(Theme::read(&part, &fields.string("name")?)?)
            }
            "help" => Self::Help,
            "exit" => Self::Exit,
            _ => return Err(ErrorCode::UnknownCommand.into()),
        };
        fields.done()?;
        Ok(command)
    }
}

/// The session `word` names.
fn session(word: &str) -> Result<SessionId, Refusal> {
    if word.len() > crate::bounds::NAME_BYTES {
        return Err(ErrorCode::TooLarge.into());
    }
    SessionId::from_str(word).map_err(|_| ErrorCode::InvalidArgument.into())
}

/// One of a closed set of words, read from a JSON string.
fn word<T: FromStr<Err = Refusal>>(value: &Value) -> Result<T, Refusal> {
    value
        .as_str()
        .ok_or_else(|| Refusal::new(ErrorCode::Malformed))?
        .parse()
}

/// The words of a prompt: never empty, never over [`PROMPT_BYTES`].
///
/// Redacted from `Debug`, because they are a person's.
#[derive(Clone, PartialEq, Eq)]
pub struct Prompt(Box<str>);

impl Prompt {
    /// A prompt saying `words`.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] over [`PROMPT_BYTES`], checked before the words
    /// are copied; [`ErrorCode::InvalidArgument`] where there are none.
    pub fn new(words: &str) -> Result<Self, Refusal> {
        if words.len() > PROMPT_BYTES {
            return Err(ErrorCode::TooLarge.into());
        }
        if words.is_empty() {
            return Err(ErrorCode::InvalidArgument.into());
        }
        Ok(Self(words.into()))
    }

    /// The words.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Prompt([redacted; {} bytes])", self.0.len())
    }
}

/// How hard a model is asked to think, as this protocol spells it.
///
/// The protocol's own ladder rather than the engine's: the application maps
/// one onto the other, and says so where a model does not serve the rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rung {
    /// The least.
    Low,
    /// Some.
    Medium,
    /// A good deal.
    High,
    /// More than that.
    Xhigh,
    /// The most.
    Max,
}

impl Rung {
    /// Every rung, weakest first.
    pub const EVERY: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    /// The word it crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl FromStr for Rung {
    type Err = Refusal;

    fn from_str(word: &str) -> Result<Self, Refusal> {
        Self::EVERY
            .into_iter()
            .find(|rung| rung.as_str() == word)
            .ok_or_else(|| ErrorCode::InvalidArgument.into())
    }
}

/// The mode tool calls are decided under, as this protocol spells it.
///
/// A mode is a standing instruction the person at the host gives, and setting
/// one is a command like any other; it is not an answer to a pending action
/// and mints nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Every call that changes something is asked about.
    Ask,
    /// Edits inside the workspace run; the rest is asked about.
    AllowEdits,
    /// Nothing is asked about.
    FullAccess,
}

impl Mode {
    /// Every mode, in the order they are stepped through.
    pub const EVERY: [Self; 3] = [Self::Ask, Self::AllowEdits, Self::FullAccess];

    /// The word it crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AllowEdits => "allow_edits",
            Self::FullAccess => "full_access",
        }
    }
}

impl FromStr for Mode {
    type Err = Refusal;

    fn from_str(word: &str) -> Result<Self, Refusal> {
        Self::EVERY
            .into_iter()
            .find(|mode| mode.as_str() == word)
            .ok_or_else(|| ErrorCode::InvalidArgument.into())
    }
}

/// The colours a session is drawn in, as the configuration spells them.
///
/// Closed, because the configuration it is written into is: a word outside
/// this set would be a file the next run refuses to start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Palette {
    /// Follow the terminal's own background.
    Auto,
    /// For a dark ground.
    Dark,
    /// For a light one.
    Light,
    /// For a dark ground, told apart without red against green.
    ColourblindDark,
    /// For a light ground, told apart the same way.
    ColourblindLight,
    /// Only the sixteen colours the terminal already has.
    Ansi,
}

impl Palette {
    /// Every palette, in the order they are offered.
    pub const EVERY: [Self; 6] = [
        Self::Auto,
        Self::Dark,
        Self::Light,
        Self::ColourblindDark,
        Self::ColourblindLight,
        Self::Ansi,
    ];

    /// The word it crosses as, and is written down as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::ColourblindDark => "colourblind-dark",
            Self::ColourblindLight => "colourblind-light",
            Self::Ansi => "ansi",
        }
    }
}

impl FromStr for Palette {
    type Err = Refusal;

    fn from_str(word: &str) -> Result<Self, Refusal> {
        Self::EVERY
            .into_iter()
            .find(|palette| palette.as_str() == word)
            .ok_or_else(|| ErrorCode::InvalidArgument.into())
    }
}

/// Which remembered drawing choice a theme command writes down.
///
/// Nothing is drawn by it: how a session looks is the front end's, and what
/// reaches the application is only the choice to write down for the next run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Theme {
    /// The colours the session is drawn in.
    Drawing(Palette),
    /// The colours source code is read in, by the name its theme goes by.
    Syntax(Name),
}

impl Theme {
    /// The theme named.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Drawing(palette) => palette.as_str(),
            Self::Syntax(name) => name.as_str(),
        }
    }

    const fn part(&self) -> &'static str {
        match self {
            Self::Drawing(_) => "drawing",
            Self::Syntax(_) => "syntax",
        }
    }

    fn read(part: &str, name: &str) -> Result<Self, Refusal> {
        match part {
            "drawing" => Ok(Self::Drawing(name.parse()?)),
            "syntax" => Ok(Self::Syntax(Name::new(name)?)),
            _ => Err(ErrorCode::InvalidArgument.into()),
        }
    }
}
