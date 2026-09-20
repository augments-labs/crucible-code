//! The action a turn is stopped on, and a client's answer to it.
//!
//! A [`Pending`] is put by the application and names itself with a
//! [`PendingId`] the application minted. A [`Decision`] is a client's word
//! about one: it carries the identity it is answering and what was said, and
//! that is all it is. Whether it settles anything is the application's to
//! decide, against the action that is pending at that moment — a decision
//! naming any other identity, or answering a different kind of question, is
//! refused and the action stays pending.
//!
//! Nothing here is a permission. A [`Ruling`] is a yes or a no as a client
//! spells it; the proof a tool runs under is minted by the permission engine,
//! from its own verdict type, inside the call that asked, and no value of this
//! crate converts into either.

use serde_json::Value;

use crate::bounds::{Said, Text};
use crate::error::{ErrorCode, Refusal};
use crate::wire::{Fields, Writing};

/// The identity of one pending action, minted by the application.
///
/// Never reused while the host runs, so the identity of an action that was
/// settled, abandoned or belonged to an earlier turn names nothing afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PendingId(u64);

impl PendingId {
    /// The identity numbered `number`.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.0
    }
}

/// What kind of thing a tool call would do, as the question about it is put.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Read inside the workspace.
    Reads,
    /// Read outside it.
    ReadsOutside,
    /// Change a file.
    MutatesFile,
    /// Start a process.
    SpawnsProcess,
    /// Reach a host over the network.
    ReachesNetwork,
}

impl Effect {
    /// Every effect.
    pub const EVERY: [Self; 5] = [
        Self::Reads,
        Self::ReadsOutside,
        Self::MutatesFile,
        Self::SpawnsProcess,
        Self::ReachesNetwork,
    ];

    /// The word it crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reads => "reads",
            Self::ReadsOutside => "reads_outside",
            Self::MutatesFile => "mutates_file",
            Self::SpawnsProcess => "spawns_process",
            Self::ReachesNetwork => "reaches_network",
        }
    }

    fn named(word: &str) -> Result<Self, Refusal> {
        Self::EVERY
            .into_iter()
            .find(|effect| effect.as_str() == word)
            .ok_or_else(|| ErrorCode::InvalidArgument.into())
    }
}

/// One answer a question offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What choosing it is called, and what is sent back to choose it.
    pub name: Text,
    /// What it means, where the question said.
    pub says: Text,
}

/// One question a model put to the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The few words it is headed by.
    pub heading: Text,
    /// The question.
    pub asks: Text,
    /// Whether more than one answer may be chosen.
    pub several: bool,
    /// The answers offered.
    pub choices: Vec<Choice>,
}

/// What a turn is stopped on until somebody answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Whether one tool call may run.
    Permission {
        /// Which pending action this is.
        id: PendingId,
        /// The tool that would run.
        tool: Text,
        /// The kind of thing it would do.
        effect: Effect,
        /// What exactly it would do that to, as the host would show it.
        subject: Text,
    },
    /// Questions a model asked of the person.
    Questions {
        /// Which pending action this is.
        id: PendingId,
        /// The questions, in the order they are to be answered.
        questions: Vec<Asked>,
    },
}

impl Pending {
    /// Which pending action this is.
    #[must_use]
    pub const fn id(&self) -> PendingId {
        match self {
            Self::Permission { id, .. } | Self::Questions { id, .. } => *id,
        }
    }

    /// The value this travels as.
    #[must_use]
    pub fn written(&self) -> Value {
        match self {
            Self::Permission {
                id,
                tool,
                effect,
                subject,
            } => Writing::kind("permission")
                .with("id", id.number())
                .text("tool", tool)
                .with("effect", effect.as_str())
                .text("subject", subject),
            Self::Questions { id, questions } => {
                Writing::kind("questions").with("id", id.number()).with(
                    "questions",
                    questions.iter().map(Asked::written).collect::<Vec<_>>(),
                )
            }
        }
        .finish()
    }

    /// The pending action `value` is.
    ///
    /// # Errors
    ///
    /// [`Refusal`] for anything but a whole, bounded pending action.
    pub fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let kind = fields.kind()?;
        let id = PendingId::new(fields.number("id")?);
        let pending = match kind.as_str() {
            "permission" => Self::Permission {
                id,
                tool: fields.text("tool")?,
                effect: Effect::named(&fields.string("effect")?)?,
                subject: fields.text("subject")?,
            },
            "questions" => Self::Questions {
                id,
                questions: fields
                    .list("questions")?
                    .into_iter()
                    .map(Asked::read)
                    .collect::<Result<_, _>>()?,
            },
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(pending)
    }
}

impl Asked {
    fn written(&self) -> Value {
        let choices: Vec<Value> = self
            .choices
            .iter()
            .map(|choice| {
                Writing::new()
                    .text("name", &choice.name)
                    .text("says", &choice.says)
                    .finish()
            })
            .collect();

        Writing::new()
            .text("heading", &self.heading)
            .text("asks", &self.asks)
            .with("several", self.several)
            .with("choices", choices)
            .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let asked = Self {
            heading: fields.text("heading")?,
            asks: fields.text("asks")?,
            several: fields.flag("several")?,
            choices: fields
                .list("choices")?
                .into_iter()
                .map(|value| {
                    let mut fields = Fields::of(value)?;
                    let choice = Choice {
                        name: fields.text("name")?,
                        says: fields.text("says")?,
                    };
                    fields.done()?;
                    Ok(choice)
                })
                .collect::<Result<_, Refusal>>()?,
        };
        fields.done()?;
        Ok(asked)
    }
}

/// A yes or a no, as a client says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ruling {
    /// The call may run.
    Allow,
    /// It may not.
    Deny,
}

/// How long a client means its ruling to last.
///
/// Two durations, not three: nothing a client says lasts past the host
/// process, because a standing rule is written by the person at the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lasting {
    /// This call only.
    Once,
    /// Every call like it until the host exits.
    Session,
}

/// The answer to one question: what was chosen, and anything added.
///
/// Both travel from the client to whatever asked, so both are [`Said`]: whole,
/// or refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// The answers chosen, by name, or words of the person's own.
    pub chosen: Vec<Said>,
    /// A note beside them.
    pub note: Said,
}

/// A client's word about one pending action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// About a permission question.
    Ruled {
        /// The pending action being answered.
        id: PendingId,
        /// Yes or no.
        ruling: Ruling,
        /// For how long.
        lasting: Lasting,
    },
    /// About a model's questions: one answer per question, in order.
    Answered {
        /// The pending action being answered.
        id: PendingId,
        /// The answers.
        answers: Vec<Picked>,
    },
    /// About a model's questions: nobody is going to answer them.
    Declined {
        /// The pending action being answered.
        id: PendingId,
    },
}

impl Decision {
    /// The pending action this says it is answering.
    #[must_use]
    pub const fn id(&self) -> PendingId {
        match self {
            Self::Ruled { id, .. } | Self::Answered { id, .. } | Self::Declined { id } => *id,
        }
    }

    pub(crate) fn written(&self) -> Value {
        match self {
            Self::Ruled {
                id,
                ruling,
                lasting,
            } => Writing::kind("ruled")
                .with("id", id.number())
                .with(
                    "ruling",
                    match ruling {
                        Ruling::Allow => "allow",
                        Ruling::Deny => "deny",
                    },
                )
                .with(
                    "lasting",
                    match lasting {
                        Lasting::Once => "once",
                        Lasting::Session => "session",
                    },
                ),
            Self::Answered { id, answers } => {
                Writing::kind("answered").with("id", id.number()).with(
                    "answers",
                    answers
                        .iter()
                        .map(|picked| {
                            Writing::new()
                                .with(
                                    "chosen",
                                    picked.chosen.iter().map(Said::as_str).collect::<Vec<_>>(),
                                )
                                .with("note", picked.note.as_str())
                                .finish()
                        })
                        .collect::<Vec<_>>(),
                )
            }
            Self::Declined { id } => Writing::kind("declined").with("id", id.number()),
        }
        .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let kind = fields.kind()?;
        let id = PendingId::new(fields.number("id")?);
        let decision = match kind.as_str() {
            "ruled" => Self::Ruled {
                id,
                ruling: match fields.string("ruling")?.as_str() {
                    "allow" => Ruling::Allow,
                    "deny" => Ruling::Deny,
                    _ => return Err(ErrorCode::InvalidArgument.into()),
                },
                lasting: match fields.string("lasting")?.as_str() {
                    "once" => Lasting::Once,
                    "session" => Lasting::Session,
                    _ => return Err(ErrorCode::InvalidArgument.into()),
                },
            },
            "answered" => Self::Answered {
                id,
                answers: fields
                    .list("answers")?
                    .into_iter()
                    .map(picked)
                    .collect::<Result<_, _>>()?,
            },
            "declined" => Self::Declined { id },
            _ => return Err(ErrorCode::InvalidArgument.into()),
        };
        fields.done()?;
        Ok(decision)
    }
}

fn picked(value: Value) -> Result<Picked, Refusal> {
    let mut fields = Fields::of(value)?;
    let picked = Picked {
        chosen: fields
            .list("chosen")?
            .into_iter()
            .map(|value| crate::wire::said(&value))
            .collect::<Result<_, _>>()?,
        note: fields.said("note")?,
    };
    fields.done()?;
    Ok(picked)
}
