//! The shape of a configuration document, declared once.
//!
//! Both readers of a document read this: the parser walks a value against it to
//! decide what is a key and what is a mistake, and the schema is generated from
//! it. That is the whole reason it exists as data rather than as two pieces of
//! code. A hand-written schema beside a hand-written parser is two declarations
//! of the same keys, and the gate that would catch them disagreeing can only
//! sample the documents someone thought to write down; generating one from the
//! other makes the disagreement unrepresentable instead.
//!
//! Adding a key is therefore an edit to this file and nowhere else. The schema
//! regenerates, `scripts/check.sh` compares it against the checked-in copy, and
//! the parser accepts the key without being told about it separately.

/// What a value at one position in the document may be.
pub(crate) enum Shape {
    /// Any string.
    Text,

    /// A string from a fixed set.
    Choice(&'static [&'static str]),

    /// An object whose keys are exactly these, all of them optional.
    Fields(&'static [Field]),

    /// An object whose keys the user chooses — a provider name, a variable
    /// name — each value having the same shape.
    Named(&'static Shape),
}

/// One key, and everything the parser and the schema each need about it.
///
/// `about` is written for the person reading a completion popup in their
/// editor, because that is where it ends up.
pub(crate) struct Field {
    /// The key as it appears in the document.
    pub(crate) name: &'static str,
    /// One sentence, ending without a full stop, in crucible's own words.
    pub(crate) about: &'static str,
    /// What its value may be.
    pub(crate) shape: Shape,
}

/// What a provider may be told.
const PROVIDER: Shape = Shape::Fields(&[
    Field {
        name: "model",
        about: "The model to ask when --model does not name one",
        shape: Shape::Text,
    },
    // The *name*. A key never appears in a configuration file: this workspace
    // resolves one from the environment by the name given here, and the value
    // has no path into a document, a session file or a log line.
    Field {
        name: "apiKeyEnv",
        about: "Name of the environment variable holding this provider's API key — the name, never the key",
        shape: Shape::Text,
    },
]);

/// What a variable in the `env` block may be: a value, applied verbatim.
///
/// A string even for a setting that reads as a number, because this block is
/// the environment and the environment holds strings. `"12"` is what the
/// variable would have to be to arrive any other way.
const VALUE: Shape = Shape::Text;

/// Every answer `output.color` accepts, in the order the schema lists them.
///
/// Named here rather than written inline because `settings` has to turn one of
/// these back into a value, and a set spelled out in two places is a set that
/// can drift. This is the declaration; the reader is tested against it.
pub(crate) const COLOR: &[&str] = &["auto", "always", "never"];

/// Every answer `output.toolDetail` accepts.
pub(crate) const TOOL_DETAIL: &[&str] = &["compact", "full"];

/// How much of a tool call one line shows, and whether anything is dimmed.
const OUTPUT: &[Field] = &[
    Field {
        name: "color",
        about: "Whether to dim the prompt: auto follows the terminal and NO_COLOR, always and never override it",
        shape: Shape::Choice(COLOR),
    },
    Field {
        name: "toolDetail",
        about: "How much of a tool call and its result one line shows",
        shape: Shape::Choice(TOOL_DETAIL),
    },
];

/// The document itself.
pub(crate) const DOCUMENT: Shape = Shape::Fields(&[
    Field {
        name: "providers",
        about: "Per-provider defaults, keyed by provider name",
        shape: Shape::Named(&PROVIDER),
    },
    Field {
        name: "env",
        about: "Environment variables to set, crucible's own CRUCIBLE_CODE_ settings among them. A checked-in file may set only those",
        shape: Shape::Named(&VALUE),
    },
    Field {
        name: "output",
        about: "What the terminal shows",
        shape: Shape::Fields(OUTPUT),
    },
]);

impl Shape {
    /// The keys this shape accepts, for the sentence an unknown one gets back.
    ///
    /// Empty when the user chooses the keys, which is what tells the caller to
    /// say something other than "did you mean".
    pub(crate) fn keys(&self) -> Vec<&'static str> {
        match self {
            Self::Fields(fields) => fields.iter().map(|field| field.name).collect(),
            Self::Text | Self::Choice(_) | Self::Named(_) => Vec::new(),
        }
    }

    /// The shape of `name` under this one, if it is a key at all.
    pub(crate) fn field(&self, name: &str) -> Option<&'static Shape> {
        match self {
            Self::Fields(fields) => fields
                .iter()
                .find(|field| field.name == name)
                .map(|field| &field.shape),
            Self::Named(inner) => Some(inner),
            Self::Text | Self::Choice(_) => None,
        }
    }

    /// What this shape is called in a message about the wrong kind of value.
    pub(crate) fn wanted(&self) -> &'static str {
        match self {
            Self::Text => "a string",
            Self::Choice(_) => "one of a fixed set of strings",
            Self::Fields(_) | Self::Named(_) => "an object",
        }
    }
}
