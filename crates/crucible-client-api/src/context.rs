//! How the window of the next request is spent, by what holds it.
//!
//! A [`Context`] is the runner's count of the request it would build now,
//! divided into a closed set of [`Category`] parts, read off the conversation
//! when a client asks for it. Its parts are numbers of tokens and nothing they
//! were counted from: no instruction, schema or message crosses, only how much
//! of the window each kind of them takes.
//!
//! What is left of the window is the same whole-number reading a
//! [`Snapshot`](crate::Snapshot) carries, so a client showing both never shows
//! two figures for one fact. A model that has not said how large its window is
//! leaves the window, the reading and the free part out rather than guessing
//! them.

use serde_json::Value;

use crate::error::{ErrorCode, Refusal};
use crate::snapshot::{Model, Percent};
use crate::wire::{Fields, Writing, text, written};

/// What a part of the window is held by.
///
/// Closed: content that reaches a request is one of these, and a new kind of
/// it is a new arm every reader decides about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Crucible's own instructions, or the ones that replaced them.
    System,
    /// What settings appended to the system field.
    Instructions,
    /// The schemas of the tools a request advertises.
    Tools,
    /// The schemas of tools a Model Context Protocol server serves.
    Mcp,
    /// The conversation, tool results included.
    Messages,
    /// What is kept back for the answer and for compaction.
    Reserve,
    /// What nothing holds yet.
    Free,
}

impl Category {
    /// Every part, in the order a window is filled.
    pub const EVERY: [Self; 7] = [
        Self::System,
        Self::Instructions,
        Self::Tools,
        Self::Mcp,
        Self::Messages,
        Self::Reserve,
        Self::Free,
    ];

    /// The field this part crosses as.
    const fn field(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Instructions => "instructions",
            Self::Tools => "tools",
            Self::Mcp => "mcp",
            Self::Messages => "messages",
            Self::Reserve => "reserve",
            Self::Free => "free",
        }
    }
}

/// How the window of the next request is spent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// The model the request is for, where one is chosen.
    pub model: Option<Model>,
    /// How many tokens the model's window holds, where it said.
    pub window: Option<u64>,
    /// What percentage of the window is left, as the prompt line reads it,
    /// where the window is known.
    pub left: Option<Percent>,
    /// Tokens of crucible's own instructions.
    pub system: u64,
    /// Tokens settings appended to the system field.
    pub instructions: u64,
    /// Tokens of advertised tool schemas.
    pub tools: u64,
    /// Tokens of tool schemas a Model Context Protocol server serves.
    pub mcp: u64,
    /// Tokens of the conversation.
    pub messages: u64,
    /// Tokens kept back for the answer and for compaction.
    pub reserve: u64,
    /// Tokens nothing holds; none where the window is not known.
    pub free: u64,
}

impl Context {
    /// The tokens `category` holds.
    #[must_use]
    pub const fn tokens(&self, category: Category) -> u64 {
        match category {
            Category::System => self.system,
            Category::Instructions => self.instructions,
            Category::Tools => self.tools,
            Category::Mcp => self.mcp,
            Category::Messages => self.messages,
            Category::Reserve => self.reserve,
            Category::Free => self.free,
        }
    }

    pub(crate) fn written(&self) -> Value {
        Category::EVERY
            .into_iter()
            .fold(Writing::new(), |object, category| {
                object.with(category.field(), self.tokens(category))
            })
            .maybe(
                "model",
                self.model.as_ref().map(|model| written(model.text())),
            )
            .maybe("window", self.window)
            .maybe("left", self.left.map(Percent::get))
            .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let context = Self {
            model: fields
                .maybe("model")
                .map(text)
                .transpose()?
                .map(|model| Model::of(model).ok_or_else(|| Refusal::new(ErrorCode::Malformed)))
                .transpose()?,
            window: fields.maybe_number("window")?,
            left: fields
                .maybe_number("left")?
                .map(|left| {
                    u8::try_from(left)
                        .ok()
                        .and_then(Percent::new)
                        .ok_or_else(|| Refusal::new(ErrorCode::Malformed))
                })
                .transpose()?,
            system: fields.number(Category::System.field())?,
            instructions: fields.number(Category::Instructions.field())?,
            tools: fields.number(Category::Tools.field())?,
            mcp: fields.number(Category::Mcp.field())?,
            messages: fields.number(Category::Messages.field())?,
            reserve: fields.number(Category::Reserve.field())?,
            free: fields.number(Category::Free.field())?,
        };
        fields.done()?;
        Ok(context)
    }
}
