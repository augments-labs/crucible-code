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
//! regenerates, `cargo test` compares it against the checked-in copy, and the
//! parser accepts the key without being told about it separately.

pub(crate) mod schema;

#[cfg(test)]
mod tests;

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

    /// An array, every element having the same shape.
    ///
    /// Order carries no meaning anywhere in this document and neither does a
    /// repeat, which is what lets a list be concatenated across layers instead
    /// of replaced. See `docs/configuration/configuration.md`.
    List(&'static Shape),
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
    /// Values somebody could write here, shown by an editor beside the
    /// sentence above.
    ///
    /// Each is one *string*, so for a [`Shape::List`] they are elements rather
    /// than whole lists — a list's elements being exactly where the shape of a
    /// value is not evident from its name. Empty where it is: a `Choice` names
    /// its own answers, and an example naming a real model id would rot.
    ///
    /// These ship to every editor that resolves the schema, so they are
    /// teaching material rather than filler. Whatever stands here is what gets
    /// pasted.
    pub(crate) examples: &'static [&'static str],

    /// Whether this key can only ever loosen what crucible does unasked.
    ///
    /// Declared here, beside the key, rather than as a list of paths somewhere
    /// that checks documents — a list like that is a second declaration of the
    /// same keys, and the one that goes stale is the one nothing reads. The
    /// walk refuses a `true` in either workspace layer, and the
    /// reasoning for that sits where the refusal is.
    ///
    /// It reaches no schema. One schema is served to all three layers, and a
    /// key refused in two of them is still a key in the user file — an editor
    /// that struck it out would still be wrong there.
    pub(crate) widens: bool,
}

/// What a provider may be told.
const PROVIDER: Shape = Shape::Fields(&[
    Field {
        name: "model",
        about: "The model to ask when --model does not name one",
        shape: Shape::Text,
        // No example. It would have to name a real model, and a model id in a
        // file served to every editor outlives the model.
        examples: &[],
        widens: false,
    },
    Field {
        name: "effort",
        about: "How hard to think before answering, when --effort does not say. Left off, the vendor's own default for whichever model is being asked",
        shape: Shape::Choice(EFFORT),
        // A `Choice` lists its own answers.
        examples: &[],
        widens: false,
    },
    // The *name*. A key never appears in a configuration file: this workspace
    // resolves one from the environment by the name given here, and the value
    // has no path into a document, a session file or a log line. Choosing that
    // name still chooses which inherited value is sent away as a credential,
    // so a file from the workspace may not make the choice.
    Field {
        name: "apiKeyEnv",
        about: "Name of the environment variable holding this provider's API key — the name, never the key",
        shape: Shape::Text,
        examples: &[],
        widens: true,
    },
    // The one key here that decides *who* the request goes to, which is who
    // receives the API key with it. `widens` is what keeps that out of the
    // layers a clone can bring: a repository able to set this would be a repository
    // that reads the key of everyone who opens it.
    Field {
        name: "baseUrl",
        about: "Address to send this provider's requests to instead of the vendor's, for a gateway or a proxy",
        shape: Shape::Text,
        examples: &["https://gateway.example/v1/messages"],
        widens: true,
    },
]);

/// What a variable in the `env` block may be: a value, applied verbatim.
///
/// A string even for a setting that reads as a number, because this block is
/// the environment and the environment holds strings. `"12"` is what the
/// variable would have to be to arrive any other way.
const VALUE: Shape = Shape::Text;

/// Every answer `providers.<name>.effort` accepts, weakest first.
///
/// The rungs [`crucible_core::Effort`] holds, spelled the way it spells them —
/// this is the declaration an editor completes from, and the type is what turns
/// one back into a value. A test walks this list through that parse, so a rung
/// added to one and not the other is a build that fails rather than a key the
/// schema accepts and the program drops.
pub(crate) const EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Every answer `output.color` accepts, in the order the schema lists them.
///
/// Named here rather than written inline because `settings` has to turn one of
/// these back into a value, and a set spelled out in two places is a set that
/// can drift. This is the declaration; the reader is tested against it.
pub(crate) const COLOR: &[&str] = &["auto", "always", "never"];

/// Every answer `output.toolDetail` accepts.
pub(crate) const TOOL_DETAIL: &[&str] = &["compact", "full"];

/// Every answer `output.mouse` accepts.
///
/// Two, and they are the two ends of one trade rather than a preference. See
/// the field below.
pub(crate) const MOUSE: &[&str] = &["off", "click"];

/// Every answer `output.theme` accepts.
///
/// `auto` is not the absence of an answer: it is the answer "decide from the
/// terminal", which a nearer layer may state to undo a theme a further one
/// named. The four after it are tables, and `ansi` is the instruction to spend
/// nothing but the sixteen the terminal already has.
pub const THEME: &[&str] = &[
    "auto",
    "dark",
    "light",
    "colourblind-dark",
    "colourblind-light",
    "ansi",
];

/// Every answer `output.glyphs` accepts.
///
/// Asked rather than detected. A terminal that draws a box-drawing character as
/// a hollow square has a font missing it, and a font is not something a program
/// can interrogate over a pipe — so this is the answer, not a fallback for one.
pub(crate) const GLYPHS: &[&str] = &["unicode", "ascii"];

/// What the terminal is drawn with, and how much of a line it gets.
const OUTPUT: &[Field] = &[
    Field {
        name: "color",
        about: "Whether to write colour: auto follows the terminal and NO_COLOR, always and never override it",
        shape: Shape::Choice(COLOR),
        // A `Choice` lists its own answers, so an example would be one of them
        // written twice.
        examples: &[],
        widens: false,
    },
    Field {
        name: "theme",
        about: "Which colours crucible draws with: auto follows the terminal's own background, ansi spends only the sixteen it already has",
        shape: Shape::Choice(THEME),
        examples: &[],
        widens: false,
    },
    Field {
        name: "glyphs",
        about: "Which characters crucible draws with: unicode for box drawing, ascii for a font that lacks it",
        shape: Shape::Choice(GLYPHS),
        examples: &[],
        widens: false,
    },
    Field {
        name: "mouse",
        about: "off leaves the mouse to the terminal, so the wheel scrolls it; click lets you place the cursor in the prompt and open a result that was cut short, and the wheel stops scrolling",
        shape: Shape::Choice(MOUSE),
        examples: &[],
        widens: false,
    },
    Field {
        name: "toolDetail",
        about: "How much of a tool call and its result one line shows",
        shape: Shape::Choice(TOOL_DETAIL),
        examples: &[],
        widens: false,
    },
];

/// Every answer `updates.check` accepts.
pub(crate) const UPDATE_CHECK: &[&str] = &["auto", "never"];

/// Whether crucible finds out that it is out of date.
///
/// A key rather than a fact about the build, because asking means reaching a
/// server crucible was not asked to reach. `never` is the answer for a machine
/// where that is not wanted, and it is the whole of what turns it off.
const UPDATES: &[Field] = &[Field {
    name: "check",
    about: "Whether crucible asks GitHub which release is newest, and says so when this one is behind",
    shape: Shape::Choice(UPDATE_CHECK),
    examples: &[],
    widens: false,
}];

/// Every answer `permissions.mode` accepts.
///
/// Spelled the way [`crucible_core::Mode`] spells it, so what the prompt line
/// shows is what you would type here.
pub(crate) const MODE: &[&str] = &["ask", "allowEdits", "fullAccess"];

/// One rule: a tool, and what it may act on.
const RULE: Shape = Shape::Text;

/// One directory the workspace also reaches.
const DIRECTORY: Shape = Shape::Text;

/// What runs unasked, what is refused, and what happens to everything else.
///
/// The three kinds are separate keys rather than one list of `kind: pattern`
/// entries, because the kind decides which rule wins and reading a `deny` list
/// on its own is the property that buys.
///
/// Three of these five widen and two tighten, and that is the whole of what the
/// either workspace layer is allowed to say: `ask` and `deny` only ever
/// put more in front of the user, so a repository that wants its own `.git`
/// left alone can still say so in a file everyone gets.
const PERMISSIONS: &[Field] = &[
    Field {
        name: "mode",
        about: "What happens to a call no rule mentions: ask about every change and command, allow changes to files, or allow everything. Read only from the configuration file in your home directory",
        shape: Shape::Choice(MODE),
        examples: &[],
        widens: true,
    },
    Field {
        name: "allow",
        about: "Rules for calls that run without being put to you. Read only from the configuration file in your home directory",
        shape: Shape::List(&RULE),
        // A whole command rather than a program and a wildcard. `bash(git *)`
        // would read as the obvious thing to write and would cover `git push`,
        // and an example is where somebody learns which one to write.
        examples: &["read(src/**)", "bash(cargo test)"],
        widens: true,
    },
    Field {
        name: "ask",
        about: "Rules for calls that are always put to you, whatever the mode says",
        shape: Shape::List(&RULE),
        examples: &["edit(Cargo.lock)", "bash(git push)"],
        widens: false,
    },
    Field {
        name: "deny",
        about: "Rules for calls that are refused in every mode, beating any allow written beside them",
        shape: Shape::List(&RULE),
        examples: &["read(.env)", "edit(.git/**)"],
        widens: false,
    },
    Field {
        name: "extraDirectories",
        about: "Absolute paths to directories outside the working directory that tools may reach. Read only from the configuration file in your home directory",
        shape: Shape::List(&DIRECTORY),
        // One spelling per platform. What counts as absolute is a drive or a
        // share on Windows and a leading slash everywhere else, and this schema
        // is one file served to both — so a single Unix example would be handed
        // to a Windows editor as a completion crucible then refuses.
        examples: &[
            "/home/you/src/shared-library",
            r"C:\Users\you\src\shared-library",
        ],
        // A directory outside the working directory is reach the workspace
        // would otherwise refuse, so naming one is widening even though no
        // rule is written.
        widens: true,
    },
];

/// The document itself.
pub(crate) const DOCUMENT: Shape = Shape::Fields(&[
    // The only key that says *which* provider. Everything under `providers` is
    // a subordinate clause — when asking this one, use that model, that
    // variable, that address — and none of them may be read as a choice of
    // vendor. This is the main clause, and it is deliberately not a `Choice`:
    // which providers exist is the binary's to say, and a set written down
    // here would be a second declaration of it that goes stale on the release
    // that adds the fourth.
    //
    // It widens for the reason `baseUrl` does, and more plainly: it decides
    // who a turn is sent to, and therefore who receives the API key and the
    // prompt. A repository may not make that choice on behalf of whoever
    // opened it, under either project filename.
    Field {
        name: "provider",
        about: "Which provider to ask, by the name --model qualifies a model with. Read only from the configuration file in your home directory",
        shape: Shape::Text,
        // No example. It would have to name one of the providers this build
        // serves, and that list belongs to the binary rather than to this
        // crate — the schema is generated from here and served to every editor.
        examples: &[],
        widens: true,
    },
    Field {
        name: "providers",
        about: "Per-provider defaults, keyed by provider name",
        shape: Shape::Named(&PROVIDER),
        examples: &[],
        widens: false,
    },
    Field {
        name: "env",
        about: "Environment variables for the commands crucible runs. A file under the working directory may set only crucible's own CRUCIBLE_CODE_ names",
        shape: Shape::Named(&VALUE),
        examples: &[],
        widens: false,
    },
    Field {
        name: "output",
        about: "What the terminal shows",
        shape: Shape::Fields(OUTPUT),
        examples: &[],
        widens: false,
    },
    Field {
        name: "permissions",
        about: "What runs without being put to you, what is refused outright, and where tools may reach",
        shape: Shape::Fields(PERMISSIONS),
        examples: &[],
        widens: false,
    },
    Field {
        name: "updates",
        about: "Whether crucible finds out that a newer release exists",
        shape: Shape::Fields(UPDATES),
        examples: &[],
        widens: false,
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
            Self::Text | Self::Choice(_) | Self::Named(_) | Self::List(_) => Vec::new(),
        }
    }

    /// The whole declaration of `name` under this one, if crucible declared it.
    ///
    /// Only a shape whose keys crucible chose has one. Under a
    /// [`Shape::Named`] the key is the user's — a provider name, a variable
    /// name — and there is nothing declared about it to return.
    pub(crate) fn declared(&self, name: &str) -> Option<&'static Field> {
        match self {
            Self::Fields(fields) => fields.iter().find(|field| field.name == name),
            Self::Text | Self::Choice(_) | Self::Named(_) | Self::List(_) => None,
        }
    }

    /// The shape of `name` under this one, if it is a key at all.
    pub(crate) fn field(&self, name: &str) -> Option<&'static Shape> {
        match self {
            Self::Fields(_) => self.declared(name).map(|field| &field.shape),
            Self::Named(inner) => Some(inner),
            Self::Text | Self::Choice(_) | Self::List(_) => None,
        }
    }

    /// The shape of one element, when this shape holds elements.
    ///
    /// What the merge asks to find out whether a nearer layer replaces this
    /// position or is appended to it, so that the rule stays a property of the
    /// declaration rather than a special case per block.
    pub(crate) fn element(&self) -> Option<&'static Shape> {
        match self {
            Self::List(inner) => Some(inner),
            Self::Text | Self::Choice(_) | Self::Fields(_) | Self::Named(_) => None,
        }
    }

    /// What this shape is called in a message about the wrong kind of value.
    pub(crate) fn wanted(&self) -> &'static str {
        match self {
            Self::Text => "a string",
            Self::Choice(_) => "one of a fixed set of strings",
            Self::Fields(_) | Self::Named(_) => "an object",
            Self::List(_) => "a list",
        }
    }
}
