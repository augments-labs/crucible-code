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

    /// A whole number that cannot be negative — a count of tokens, of turns.
    ///
    /// A number rather than a string that looks like one. This document is
    /// served to editors from a registry, so what the schema says a value is
    /// is what somebody's editor will insist on: `"25000"` where a count
    /// belongs would be a wrong type nobody could see was wrong.
    ///
    /// Nothing here has an upper bound. A ceiling written into the schema would
    /// be this crate deciding how large somebody's model is, which is exactly
    /// the fact it does not have.
    Count,

    /// A whole number *written as a string*, between two bounds.
    ///
    /// A string because the one place this appears is `env`, and the
    /// environment holds strings — see [`VALUE`]. The bounds are what the
    /// schema publishes; refusing a value outside them is
    /// `settings::variables`, one layer down, because a refusal there names the
    /// variable without quoting what was set beside it and the block it is in
    /// is the block a token would be in. Two lists, tested against each other,
    /// the same way a [`Choice`](Shape::Choice) and its reader are.
    Whole(&'static Whole),

    /// True or false, and nothing that looks like either.
    ///
    /// A JSON boolean rather than `"true"`, for the reason [`Count`](Shape::Count)
    /// is a number: this document is served to editors from a registry, so what
    /// the schema says a value is is what somebody's editor will insist on, and
    /// a string where a flag belongs would be a wrong type nobody could see was
    /// wrong.
    Flag,

    /// An object whose keys are exactly these, all of them optional.
    Fields(&'static [Field]),

    /// An object whose keys the user chooses — a provider name, a variable
    /// name — each value having the same shape.
    ///
    /// `declared` is the handful of those names crucible chose after all. The
    /// `env` block is the environment and a variable in it is somebody's own,
    /// except for the ones under crucible's namespace, whose meanings this
    /// program fixes — and a name whose meaning is fixed here is a name an
    /// editor can complete, describe and fill in. It is not a second kind of
    /// key: an undeclared one is still accepted and still takes `others`,
    /// which is what keeps this an object the user keys.
    Named {
        /// Names crucible chose, which the schema describes and the walk uses
        /// in place of `others`.
        declared: &'static [Field],
        /// Every other name, whoever chose it.
        others: &'static Shape,
    },

    /// An array, every element having the same shape.
    ///
    /// Order carries no meaning in a list that does not repeat, and neither
    /// does a second copy of an element, which is what lets such a list be
    /// concatenated across layers instead of replaced. See
    /// `docs/configuration/configuration.md`.
    List {
        /// What each element is.
        of: &'static Shape,
        /// Whether a repeated element means something.
        ///
        /// False for every list whose kind decides the outcome — a rule named
        /// twice wins once, a directory named twice is reached once — and the
        /// schema marks a repeat so that a paste that went in twice is seen.
        /// True for a list that is a sequence: the arguments handed to a
        /// program are positional, and `-e` twice is two of them.
        repeats: bool,
    },

    /// An object whose names belong to somebody else.
    ///
    /// Not a laxer [`Named`](Shape::Named): that one still says what every
    /// value under it is, and this cannot, because the program that will read
    /// these names is not this one. Crucible carries the block from the file to
    /// whoever it is addressed to and reads nothing on the way.
    ///
    /// So there is no unknown key here and no wrong value — only a value that
    /// is not an object at all, which is the one thing a block being a block
    /// still asserts. Whatever is inside is the extension's to refuse, in its
    /// own words, about its own keys.
    Opaque,
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

    /// What this key means where no layer set it, written as it would be
    /// written in a document.
    ///
    /// An editor fills it in from here, which is the whole reason it is a
    /// string rather than a value: what goes in the file is what a reader
    /// meets. `None` where crucible has no answer to state — a window worked
    /// out from the model, an effort the vendor decides — because a default
    /// invented for the schema is a sentence about behaviour that nothing runs.
    pub(crate) usual: Option<&'static str>,

    /// Whether a record carrying this shape is incomplete without this key.
    ///
    /// Declared here, beside the key, so that the walk that refuses a record
    /// missing it and the schema that marks it required are reading one
    /// answer. A list of mandatory paths kept somewhere else would be a second
    /// declaration of the same keys, and an editor that stopped agreeing with
    /// the parser is the failure nobody sees until a file is refused.
    ///
    /// Only meaningful under [`Shape::Fields`], where the keys are crucible's
    /// own. A block the user keys has no key that must be there — which name
    /// would it be?
    pub(crate) needed: bool,

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
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "effort",
        about: "How hard to think before answering, when --effort does not say. Left off, the vendor's own default for whichever model is being asked",
        shape: Shape::Choice(EFFORT),
        // A `Choice` lists its own answers.
        examples: &[],
        usual: None,
        needed: false,
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
        usual: None,
        needed: false,
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
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "defaultContextWindow",
        about: "The context-window size in tokens for any model of this provider not named above",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "contextWindow",
        about: "The context-window size in tokens, keyed by model name; an explicit value may opt into a larger native window",
        shape: Shape::Named {
            declared: &[],
            others: &WINDOW,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
]);

/// What a variable in the `env` block may be: a value, applied verbatim.
///
/// A string even for a setting that reads as a number, because this block is
/// the environment and the environment holds strings. `"12"` is what the
/// variable would have to be to arrive any other way.
const VALUE: Shape = Shape::Text;

/// The bounds a whole number written as a string is allowed to fall between.
///
/// Both are written out by the schema, one alternative per number, which is
/// what makes the published bounds exactly the accepted ones without an
/// algorithm standing between them. That costs a line of pattern per number, so
/// this is for a range small enough to write out; a wider one wants a different
/// answer here rather than a longer version of this one.
pub(crate) struct Whole {
    /// The smallest accepted.
    pub(crate) least: u16,
    /// The largest.
    pub(crate) most: u16,
}

/// How far one notch of the wheel may be asked to move the transcript.
///
/// One rather than none at the bottom. A wheel set to move nothing is a setting
/// that looks applied and does nothing, and a reader who wants the wheel to
/// leave the transcript alone is asking for a thing crucible no longer has to
/// give, because the screen it scrolls is its own.
///
/// A screenful on most terminals at the top. Past that the wheel stops being a
/// scroll and becomes a jump: two notches and the rows that were on screen are
/// gone with nothing between them to read, which is a worse way to lose your
/// place than scrolling too slowly ever is.
pub(crate) const SCROLL_SPEED: Whole = Whole { least: 1, most: 30 };

/// The name of that setting, as it is written in the block.
///
/// Spelled out rather than built from [`crate::env::NAMESPACE`], because a name
/// assembled at run time is a name nobody can grep for. A test keeps it inside
/// the namespace.
pub(crate) const MOUSE_SCROLL_SPEED: &str = "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED";

/// The variables in the `env` block whose meaning crucible fixes.
///
/// Everything else there is somebody's own and takes [`VALUE`]. These are
/// settings that happen to be spelled as environment variables, so that one of
/// them can be written for a project, for a user, or in front of a single run —
/// and being declared is what lets an editor say what each one does and what it
/// is when nobody says.
const ENV: &[Field] = &[Field {
    name: MOUSE_SCROLL_SPEED,
    about: "How many rows of the transcript one notch of the wheel moves",
    shape: Shape::Whole(&SCROLL_SPEED),
    // The bounds are the answers, and the schema writes them out.
    examples: &[],
    usual: Some("6"),
    // Crucible's own namespace is what either project file may set: these are
    // settings rather than secrets, and none of them widens anything.
    needed: false,
    widens: false,
}];

/// Every answer `providers.<name>.effort` accepts, weakest first.
///
/// The rungs [`crucible_core::Effort`] holds, spelled the way it spells them —
/// this is the declaration an editor completes from, and the type is what turns
/// one back into a value. A test walks this list through that parse, so a rung
/// added to one and not the other is a build that fails rather than a key the
/// schema accepts and the program drops.
pub(crate) const EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Every answer `systemPrompt.tone` accepts, in the order a picker offers them.
///
/// The tones [`crucible_core::Tone`] holds, spelled the way it spells them.
/// The second `Choice` in this file whose meaning belongs to another crate, and
/// tested the same way the first is: both directions, so a word here the
/// program no longer parses is caught, and so is a tone the program grew that
/// no document can reach.
pub(crate) const TONE: &[&str] = &["concise", "explanatory", "learning"];

/// Every answer `output.color` accepts, in the order the schema lists them.
///
/// Named here rather than written inline because `settings` has to turn one of
/// these back into a value, and a set spelled out in two places is a set that
/// can drift. This is the declaration; the reader is tested against it.
pub(crate) const COLOR: &[&str] = &["auto", "always", "never"];

/// Every answer `output.toolDetail` accepts.
pub(crate) const TOOL_DETAIL: &[&str] = &["compact", "full"];

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

/// What the model is asked under, where the reader wants something else.
///
/// Two hooks that look alike and are not. `append` adds to what crucible says;
/// `custom` says it instead — and what it says instead of includes the line
/// about asking before building the wrong thing. So `custom` widens and
/// `append` does not, which is the same split the `permissions` block above
/// already makes: the key that can take a guard away is read only from the file
/// whoever is sitting here owns, and the key that can only add to one is
/// readable from a checkout.
///
/// Neither reaches the workspace root, the tool list or the model's own name.
/// Those are what the session found out, not what crucible has an opinion
/// about, and a reader replacing an opinion has not said the facts are wrong.
const PROMPT: &[Field] = &[
    Field {
        name: "tone",
        about: "How much of the reasoning comes back with the answer: concise for the conclusion, explanatory for why it is that one, learning for what to know before touching the code again",
        shape: Shape::Choice(TONE),
        // A `Choice` lists its own answers.
        examples: &[],
        usual: Some("concise"),
        needed: false,
        widens: false,
    },
    Field {
        name: "custom",
        about: "Instructions to ask every turn under in place of crucible's own, replacing them entirely. Read only from the configuration file in your home directory",
        shape: Shape::Text,
        examples: &["You are a reviewer. Read and explain; never edit a file."],
        // It replaces the paragraph that says to ask before building the wrong
        // thing, which is a guard a repository may not take away from whoever
        // opened it.
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "append",
        about: "Instructions to add to crucible's own, said after them and every turn",
        shape: Shape::Text,
        examples: &["This repository is deployed on Fridays; never push to main."],
        usual: None,
        needed: false,
        widens: false,
    },
];

/// What the terminal is drawn with, and how much of a line it gets.
const OUTPUT: &[Field] = &[
    Field {
        name: "color",
        about: "Whether to write colour: auto follows the terminal and NO_COLOR, always and never override it",
        shape: Shape::Choice(COLOR),
        // A `Choice` lists its own answers, so an example would be one of them
        // written twice.
        examples: &[],
        usual: Some("auto"),
        needed: false,
        widens: false,
    },
    Field {
        name: "theme",
        about: "Which colours crucible draws with: auto follows the terminal's own background, ansi spends only the sixteen it already has",
        shape: Shape::Choice(THEME),
        examples: &[],
        usual: Some("auto"),
        needed: false,
        widens: false,
    },
    Field {
        name: "syntaxTheme",
        about: "Which theme fenced code is drawn in — a name from /theme, such as Monokai Extended, GitHub, Dracula or Nord",
        shape: Shape::Text,
        examples: &["Monokai Extended", "GitHub"],
        usual: Some("Monokai Extended"),
        needed: false,
        widens: false,
    },
    Field {
        name: "glyphs",
        about: "Which characters crucible draws with: unicode for box drawing, ascii for a font that lacks it",
        shape: Shape::Choice(GLYPHS),
        examples: &[],
        usual: Some("unicode"),
        needed: false,
        widens: false,
    },
    Field {
        name: "toolDetail",
        about: "How much of a tool call and its result one line shows",
        shape: Shape::Choice(TOOL_DETAIL),
        examples: &[],
        usual: Some("compact"),
        needed: false,
        widens: false,
    },
];

/// Every answer `input.send` accepts.
///
/// Two, because they are the two that every terminal can tell apart. Return
/// arrives everywhere; Alt and Return together arrive everywhere as an escape
/// and a Return, which is a spelling as old as the terminals that still use it.
/// Control and Return is not on the list and cannot be: nothing distinguishes
/// it from Return itself unless the terminal has agreed to a newer keyboard
/// protocol, so offering it would let a reader choose a key their terminal
/// will never report and leave them unable to send anything at all.
pub(crate) const SEND: &[&str] = &["enter", "altEnter"];

/// What the keyboard does, for the one press whose answer is not the same
/// everywhere.
///
/// A block rather than a key at the root because the question it settles —
/// which press ends a prompt and which one opens a line under it — is the
/// first of a kind, not the only one.
const INPUT: &[Field] = &[Field {
    name: "send",
    about: "Which press sends a prompt: enter sends and Shift+Enter, Alt+Enter or Ctrl+J opens a line; altEnter swaps the two, for a terminal that keeps Enter for itself",
    shape: Shape::Choice(SEND),
    examples: &[],
    usual: Some("enter"),
    needed: false,
    widens: false,
}];

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
    usual: Some("auto"),
    needed: false,
    widens: false,
}];

/// Every answer `permissions.mode` accepts.
///
/// Spelled the way [`crucible_core::Mode`] spells it, so what the prompt line
/// shows is what you would type here.
pub(crate) const MODE: &[&str] = &["ask", "allowEdits", "fullAccess"];

/// Every answer `sandbox.mode` accepts, strongest first.
pub(crate) const SANDBOX_MODE: &[&str] = &["required", "degraded", "off"];

/// Operating-system confinement policy.
const SANDBOX: &[Field] = &[Field {
    name: "mode",
    about: "Whether commands require verified kernel confinement, may use an explicit compatibility fallback, or run unconfined; only user configuration may weaken required",
    shape: Shape::Choice(SANDBOX_MODE),
    examples: &[],
    usual: Some("required"),
    // Semantic parsing permits a project to state `required` while refusing
    // only the weakening values, which this key-wide flag cannot express.
    needed: false,
    widens: false,
}];

/// Every answer `compaction.when` accepts.
pub(crate) const COMPACTION_WHEN: &[&str] = &["full", "never"];

/// Every answer `promptCaching.mode` accepts.
pub(crate) const PROMPT_CACHE_MODE: &[&str] = &["observeOnly", "prefer", "require", "prohibit"];

/// Every provider-neutral prompt-cache mechanism a policy may allow.
pub(crate) const PROMPT_CACHE_MECHANISM: &[&str] = &[
    "providerManagedUsageOnly",
    "automaticPrefix",
    "explicitBreakpoints",
    "persistentContent",
];

/// Every answer `promptCaching.isolationScope` accepts, narrowest first.
pub(crate) const PROMPT_CACHE_ISOLATION: &[&str] = &["run", "session", "workspace", "user"];

/// Provider-neutral retention classes.
pub(crate) const PROMPT_CACHE_RETENTION: &[&str] = &["providerDefault", "ephemeral", "extended"];

/// Authority over separately managed remote cache resources.
pub(crate) const PROMPT_CACHE_PERSISTENT: &[&str] = &["forbid", "reuse", "create", "require"];

/// One allowed prompt-cache mechanism.
const CACHE_MECHANISM: Shape = Shape::Choice(PROMPT_CACHE_MECHANISM);

/// A bounded requested retention class and ceiling.
const CACHE_RETENTION: &[Field] = &[
    Field {
        name: "class",
        about: "Provider-neutral retention class; extended retention must be chosen in the user configuration",
        shape: Shape::Choice(PROMPT_CACHE_RETENTION),
        examples: &[],
        usual: Some("providerDefault"),
        needed: false,
        widens: false,
    },
    Field {
        name: "maxSeconds",
        about: "Hard maximum provider retention in seconds; required for ephemeral and extended retention",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
];

/// Whether persistent cached-content resources may be used or created.
const CACHE_RESOURCES: &[Field] = &[Field {
    name: "mode",
    about: "Whether remote persistent cache resources are forbidden, reusable, creatable, or required; creation authority must come from user configuration",
    shape: Shape::Choice(PROMPT_CACHE_PERSISTENT),
    examples: &[],
    usual: Some("forbid"),
    needed: false,
    widens: false,
}];

/// Provider-neutral prompt-cache policy.
const PROMPT_CACHE: &[Field] = &[
    Field {
        name: "mode",
        about: "Whether Crucible observes provider caching, prefers the verified native mechanism, requires it, or requires a documented opt-out",
        shape: Shape::Choice(PROMPT_CACHE_MODE),
        examples: &[],
        usual: Some("prefer"),
        needed: false,
        widens: false,
    },
    Field {
        name: "allowedMechanisms",
        about: "Provider-neutral cache mechanisms still permitted after capability resolution; layers intersect this list",
        shape: Shape::List {
            of: &CACHE_MECHANISM,
            repeats: false,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "isolationScope",
        about: "Broadest identity scope allowed to share a cache prefix",
        shape: Shape::Choice(PROMPT_CACHE_ISOLATION),
        examples: &[],
        usual: Some("session"),
        needed: false,
        widens: false,
    },
    Field {
        name: "requestedRetention",
        about: "Optional provider-neutral retention request under a hard duration ceiling",
        shape: Shape::Fields(CACHE_RETENTION),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "persistentResources",
        about: "Separate authority for remotely persisted cached-content resources",
        shape: Shape::Fields(CACHE_RESOURCES),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "namespace",
        about: "Bounded opaque user-owned label included in cache scope identity; never a provider cache key",
        shape: Shape::Text,
        examples: &["personal"],
        usual: None,
        // A project-chosen external identity could collide with another scope.
        needed: false,
        widens: true,
    },
];

/// What a session does when the model's window fills.
///
/// A turn that reaches the end of a window has always had one of two endings:
/// it fails, or it makes room and carries on. These keys are how somebody
/// chooses which, and how much room is left for the exchange that follows.
const COMPACTION: &[Field] = &[
    Field {
        name: "when",
        about: "Whether a full window is answered by compacting the session, or by letting the turn fail",
        shape: Shape::Choice(COMPACTION_WHEN),
        examples: &[],
        usual: Some("full"),
        needed: false,
        widens: false,
    },
    Field {
        name: "reserve",
        about: "Tokens to keep free for the next answer and the tools it calls, instead of the room crucible works out from the model",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "keep",
        about: "How many tokens of recent turns are kept word for word after the rest becomes a recap",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "recap",
        about: "Maximum tokens a structured compaction recap may produce; ordinary recaps stop earlier",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "askOnResume",
        about: "How large a session has to be, in tokens, before picking it up asks whether to carry it whole. Zero never asks",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "spendCeiling",
        about: "The most tokens one turn may produce before crucible stops it, where a runaway turn is worth bounding",
        shape: Shape::Count,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
];

/// How much one model accepts, under the name it is asked for.
///
/// Keyed by model rather than stated once for the provider, because a session
/// changes which model it asks without changing which vendor it writes to — and
/// a figure that stayed behind would describe the model somebody just left.
const WINDOW: Shape = Shape::Count;

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
        usual: Some("ask"),
        needed: false,
        widens: true,
    },
    Field {
        name: "allow",
        about: "Rules for calls that run without being put to you. Read only from the configuration file in your home directory",
        shape: Shape::List {
            of: &RULE,
            repeats: false,
        },
        // A whole command rather than a program and a wildcard. `bash(git *)`
        // would read as the obvious thing to write and would cover `git push`,
        // and an example is where somebody learns which one to write.
        examples: &["read(src/**)", "bash(cargo test)"],
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "ask",
        about: "Rules for calls that are always put to you, whatever the mode says",
        shape: Shape::List {
            of: &RULE,
            repeats: false,
        },
        examples: &["edit(Cargo.lock)", "bash(git push)"],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "deny",
        about: "Rules for calls that are refused in every mode, beating any allow written beside them",
        shape: Shape::List {
            of: &RULE,
            repeats: false,
        },
        examples: &["read(.env)", "edit(.git/**)"],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "extraDirectories",
        about: "Absolute paths to directories outside the working directory that tools may reach. Read only from the configuration file in your home directory",
        shape: Shape::List {
            of: &DIRECTORY,
            repeats: false,
        },
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
        usual: None,
        needed: false,
        widens: true,
    },
];

/// What one extension's block may say.
///
/// Keyed by the identifier the extension's own manifest states, which is why
/// the block above declares no names: crucible does not know what is installed
/// until it has swept the directory, and a list written here would be a second
/// answer to that going stale on whatever is installed next.
const EXTENSION: Shape = Shape::Fields(&[
    Field {
        name: "enabled",
        about: "Whether crucible may run this extension. Not enough on its own: `digest` says which program was agreed to. Read only from the configuration file in your home directory",
        shape: Shape::Flag,
        // No example: `true` and `false` are the whole of what may be written here,
        // and the schema already says so.
        examples: &[],
        usual: Some("false"),
        // Turning on somebody else's code is the plainest widening in the document.
        // Either project filename can be committed, so a repository that could
        // write this would be granting authority on behalf of whoever cloned it,
        // to a program that has not been read, before anything has been typed.
        needed: false,
        widens: true,
    },
    Field {
        name: "digest",
        about: "The manifest digest this extension was agreed to at, as --extensions prints it. Read only from the configuration file in your home directory",
        shape: Shape::Text,
        // No example. Every digest is over one particular file, so a specimen
        // here would be a value nobody's installation has and every editor
        // offers to complete the field with.
        examples: &[],
        // Nothing. A digest crucible chose would be crucible agreeing to an
        // extension on the reader's behalf, which is the one thing this field
        // exists to stop.
        usual: None,
        // For the reason `enabled` does, and it is the same widening: a
        // committed file that could write this would be answering which
        // program was agreed to, on behalf of whoever cloned the checkout.
        needed: false,
        widens: true,
    },
    Field {
        name: "config",
        about: "Settings for the extension itself, in whatever names its own documentation gives. Read only from the configuration file in your home directory",
        shape: Shape::Opaque,
        // None either. Every name that could stand here belongs to an extension
        // this build has never heard of, so an example would be crucible making
        // one up and every editor completing the file with it.
        examples: &[],
        // Nothing, rather than an empty block. An extension with no settings
        // written for it is not the same as one told to use none, and which of
        // those it is is the extension's own to decide.
        usual: None,
        // For the reason `enabled` does, once removed. Crucible cannot read
        // these names, so it cannot tell a harmless one from a directory to
        // send the checkout to — and a key whose danger it has no way to weigh
        // is one a committed file may not write on behalf of whoever cloned it.
        needed: false,
        widens: true,
    },
]);

/// What one MCP server's record may say.
///
/// Keyed by the identifier the reader chooses, which is the name every tool the
/// server contributes is qualified by — `mcp:docs/search` is the `search` tool
/// of the server written down as `docs`. That is why the identifier is checked
/// rather than merely retained: a name carrying the two characters that
/// qualification is spelled with would produce a tool name nobody could read
/// back to a server.
///
/// Every key here widens, and it is the same widening throughout: this block
/// says which program crucible starts, what it is told, and what it is started
/// with. A committed file able to write any of it would be choosing somebody
/// else's server, arguments and environment on behalf of whoever cloned the
/// checkout, before anything has been typed. So the whole record is read from
/// the configuration file in the home directory and a file under the working
/// directory may not carry it at all.
///
/// Nothing here starts anything. A record is an inert statement that a server
/// exists and how it would be launched; what launches one is an exact
/// selection, made per agent or per run.
const MCP_SERVER: Shape = Shape::Fields(&[
    Field {
        name: "command",
        about: "The program to run for this server. An absolute path, or a bare program name for PATH to answer. Read only from the configuration file in your home directory",
        shape: Shape::Text,
        // A bare name first, because it is the one spelling every platform
        // reads back and it is what `written` in the shape tests puts in every
        // other example's record. Then one absolute spelling per platform: what
        // counts as absolute is a drive or a share on Windows and a leading
        // slash everywhere else, and this schema is one file served to both.
        examples: &[
            "npx",
            "/usr/local/bin/docs-mcp",
            r"C:\Program Files\docs-mcp\docs-mcp.exe",
        ],
        // Nothing. A command crucible chose would be crucible choosing whose
        // program runs, which is the one thing this key exists to state.
        usual: None,
        // Every other key here means something without this one; this is what a
        // record is for. A record missing it is refused where it was written,
        // rather than becoming a server that quietly does not exist.
        needed: true,
        widens: true,
    },
    Field {
        name: "args",
        about: "What to pass the program, one argument per element, applied verbatim. Read only from the configuration file in your home directory",
        shape: Shape::List {
            of: &VALUE,
            // A command line is a sequence, so `-e` twice is two arguments and
            // not a paste that went in twice.
            repeats: true,
        },
        examples: &["-y", "@example/docs-mcp"],
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "directory",
        about: "An absolute path to start the program in. Left off, the directory crucible was started in. Read only from the configuration file in your home directory",
        shape: Shape::Text,
        // No example. Every absolute path is somebody's own machine, and one
        // written here is what an editor offers to complete the field with.
        examples: &[],
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "env",
        about: "Environment variables for this server, applied verbatim — values, so nothing secret belongs here. Read only from the configuration file in your home directory",
        shape: Shape::Named {
            declared: &[],
            others: &VALUE,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: true,
    },
    // The *names*. A secret never appears in a configuration file: crucible
    // reads the variable named here out of its own environment and passes the
    // value to the server, so nothing that arrives this way has a path into a
    // document, a session file or a log line. `env` above is for values, and
    // this is for everything a value would be the wrong place for.
    Field {
        name: "envFrom",
        about: "Environment variables for this server taken from crucible's own, keyed by the name the server reads and holding the name crucible reads — names, never secrets. Read only from the configuration file in your home directory",
        shape: Shape::Named {
            declared: &[],
            others: &VALUE,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "handshakeSeconds",
        about: "How long to wait for the server to agree a protocol version before giving up on it. Read only from the configuration file in your home directory",
        shape: Shape::Count,
        examples: &[],
        usual: Some("10"),
        needed: false,
        widens: true,
    },
    Field {
        name: "requestSeconds",
        about: "How long to wait for one request to this server before giving up on it. Read only from the configuration file in your home directory",
        shape: Shape::Count,
        examples: &[],
        usual: Some("60"),
        needed: false,
        widens: true,
    },
    Field {
        name: "shutdownSeconds",
        about: "How long the server is given to stop on its own before it is killed. Read only from the configuration file in your home directory",
        shape: Shape::Count,
        examples: &[],
        usual: Some("5"),
        needed: false,
        widens: true,
    },
    Field {
        name: "restarts",
        about: "How many times this server may be started again after it ends. Read only from the configuration file in your home directory",
        shape: Shape::Count,
        examples: &[],
        usual: Some("0"),
        needed: false,
        widens: true,
    },
    Field {
        name: "required",
        about: "Whether a run that selected this server fails when it cannot be prepared, rather than carrying on without its tools. Read only from the configuration file in your home directory",
        shape: Shape::Flag,
        // `true` and `false` are the whole of what may be written here.
        examples: &[],
        usual: Some("false"),
        needed: false,
        widens: true,
    },
]);

/// What the `mcp` block may say.
///
/// One key, rather than servers keyed directly under `mcp`. The block is where
/// anything else about MCP would go, and a document that keyed servers at the
/// top of it could never gain a second key without a server called by that
/// name meaning two things at once.
const MCP: Shape = Shape::Fields(&[Field {
    name: "servers",
    about: "MCP servers that may be selected, keyed by the identifier their tools are qualified by. Nothing is started by being written here",
    shape: Shape::Named {
        declared: &[],
        others: &MCP_SERVER,
    },
    examples: &[],
    // Nothing, rather than an empty block. Crucible installs no server, and a
    // default written here would be one it installed.
    usual: None,
    // Refused under the working directory as a whole, so a project file is
    // told at the block rather than at whichever key it wrote first.
    needed: false,
    widens: true,
}]);

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
        usual: None,
        needed: false,
        widens: true,
    },
    Field {
        name: "providers",
        about: "Per-provider defaults, keyed by provider name",
        shape: Shape::Named {
            declared: &[],
            others: &PROVIDER,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "env",
        about: "Environment variables for the commands crucible runs. A file under the working directory may set only crucible's own CRUCIBLE_CODE_ names",
        shape: Shape::Named {
            declared: ENV,
            others: &VALUE,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "extensions",
        about: "Per-extension settings, keyed by the identifier the extension's manifest states",
        shape: Shape::Named {
            declared: &[],
            others: &EXTENSION,
        },
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "mcp",
        about: "MCP servers crucible may be asked to start",
        shape: MCP,
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "input",
        about: "What the keyboard does",
        shape: Shape::Fields(INPUT),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "output",
        about: "What the terminal shows",
        shape: Shape::Fields(OUTPUT),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "systemPrompt",
        about: "What the model is asked under: how much it explains, and anything you would rather it were told instead",
        shape: Shape::Fields(PROMPT),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "permissions",
        about: "What runs without being put to you, what is refused outright, and where tools may reach",
        shape: Shape::Fields(PERMISSIONS),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "sandbox",
        about: "Operating-system confinement for commands and descendant processes",
        shape: Shape::Fields(SANDBOX),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "compaction",
        about: "What happens when the model's window fills up",
        shape: Shape::Fields(COMPACTION),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "promptCaching",
        about: "Provider-side reuse of an identical prompt prefix, enabled through each provider's verified native mechanism by default",
        shape: Shape::Fields(PROMPT_CACHE),
        examples: &[],
        usual: None,
        needed: false,
        widens: false,
    },
    Field {
        name: "updates",
        about: "Whether crucible finds out that a newer release exists",
        shape: Shape::Fields(UPDATES),
        examples: &[],
        usual: None,
        needed: false,
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

            // Empty even where a `Named` declares some, because under one no
            // key is unknown: the sentence this feeds is the one a misspelling
            // gets back, and there is no misspelling of a name the user chose.
            // Under an `Opaque` there is no misspelling either, and crucible
            // could not name the alternatives if there were.
            Self::Text
            | Self::Choice(_)
            | Self::Count
            | Self::Flag
            | Self::Whole(_)
            | Self::Named { .. }
            | Self::List { .. }
            | Self::Opaque => Vec::new(),
        }
    }

    /// The whole declaration of `name` under this one, if crucible declared it.
    ///
    /// Under a [`Shape::Named`] most keys are the user's — a provider name, a
    /// variable name — and there is nothing declared about one to return. The
    /// handful crucible did choose there answer the same way a field does.
    pub(crate) fn declared(&self, name: &str) -> Option<&'static Field> {
        match self {
            Self::Fields(fields)
            | Self::Named {
                declared: fields, ..
            } => fields.iter().find(|field| field.name == name),
            Self::Text
            | Self::Choice(_)
            | Self::Count
            | Self::Flag
            | Self::Whole(_)
            | Self::List { .. }
            | Self::Opaque => None,
        }
    }

    /// The keys a block of this shape cannot mean anything without.
    ///
    /// Empty for every shape whose keys the user chose: which name would have
    /// to be there? A [`Named`](Self::Named) block's declared keys are names
    /// crucible described, not names it requires.
    pub(crate) fn needed(&self) -> impl Iterator<Item = &'static Field> {
        let fields: &'static [Field] = match self {
            Self::Fields(fields) => fields,
            Self::Text
            | Self::Choice(_)
            | Self::Count
            | Self::Flag
            | Self::Whole(_)
            | Self::List { .. }
            | Self::Named { .. }
            | Self::Opaque => &[],
        };
        fields.iter().filter(|field| field.needed)
    }

    /// The shape of `name` under this one, if it is a key at all.
    pub(crate) fn field(&self, name: &str) -> Option<&'static Shape> {
        match self {
            Self::Fields(_) => self.declared(name).map(|field| &field.shape),
            Self::Named { others, .. } => {
                Some(self.declared(name).map_or(*others, |field| &field.shape))
            }
            // `None` under an `Opaque` for a different reason than under a
            // scalar: there are keys, and none of them is crucible's to
            // describe. The merge reads that as the nearer layer's copy
            // standing, which is the only answer available when nothing here
            // knows whether two of these names mean the same thing.
            Self::Text
            | Self::Choice(_)
            | Self::Count
            | Self::Flag
            | Self::Whole(_)
            | Self::List { .. }
            | Self::Opaque => None,
        }
    }

    /// The shape of one element, when this shape holds elements.
    ///
    /// What the merge asks to find out whether a nearer layer replaces this
    /// position or is appended to it, so that the rule stays a property of the
    /// declaration rather than a special case per block.
    pub(crate) fn element(&self) -> Option<&'static Shape> {
        match self {
            Self::List { of, .. } => Some(of),
            Self::Text
            | Self::Choice(_)
            | Self::Count
            | Self::Flag
            | Self::Whole(_)
            | Self::Fields(_)
            | Self::Named { .. }
            | Self::Opaque => None,
        }
    }

    /// What this shape is called in a message about the wrong kind of value.
    pub(crate) fn wanted(&self) -> &'static str {
        match self {
            Self::Text => "a string",
            Self::Choice(_) => "one of a fixed set of strings",
            Self::Count => "a whole number that is not negative",
            Self::Flag => "true or false",
            Self::Whole(_) => "a whole number written as a string",
            Self::Fields(_) | Self::Named { .. } => "an object",
            Self::List { .. } => "a list",
            // Said at more length than the plain object above, because a
            // reader who wrote a string here is somebody following an
            // extension's own documentation, and the useful thing to tell them
            // is that its settings go inside a block rather than beside the
            // key.
            Self::Opaque => "an object of the extension's own settings",
        }
    }
}

/// What the schema says a key falls back to, by the path a document writes it
/// at.
///
/// For the tests beside each settings module. What each of them is for: the
/// declaration here and the value the module returns when nothing was written
/// are two answers to one question, and a schema that goes on offering a
/// default the program stopped having is worse than one offering none — an
/// editor writes it into the file.
///
/// # Panics
///
/// If the path names no key, or names one with no default declared. Both are
/// the test's own mistake rather than a document's.
#[cfg(test)]
pub(crate) fn usual(path: &[&str]) -> &'static str {
    let mut shape = &DOCUMENT;
    let (last, above) = path.split_last().expect("a path names at least one key");

    for name in above {
        shape = shape.field(name).expect("every key above the last exists");
    }

    shape
        .declared(last)
        .expect("the last key exists")
        .usual
        .expect("the key states what it falls back to")
}
