//! The layers, resolved into the settings a turn actually runs with.
//!
//! This file holds what every block shares — the layering, and the merge that
//! decides which layer wins a position. A block that carries meaning of its own
//! has a module beside it under `settings/`, so its keys, the types they become
//! and the tests for both sit in one place. `providers` and `env` stay here:
//! what they hold is read straight back out. The one value that is not a string
//! afterwards — how hard to think — becomes a type this crate does not own, so
//! there is no meaning here for a module to hold.

use std::borrow::Cow;
use std::fmt;

use crucible_models::{Effort, Speed};
use crucible_tools::Rules;
use crucible_types::{PromptCachePolicy, address};
use serde_json::{Map, Value};

use crate::document::Document;
use crate::env;
use crate::shape::{DOCUMENT, Shape};

mod compaction;
mod input;
pub(crate) mod layers;
pub(crate) mod mcp;
mod menu;
mod output;
mod permissions;
mod prompt;
pub(crate) mod prompt_cache;
pub(crate) mod sandbox;
mod updates;
mod variables;

pub use compaction::{Compaction, When};
pub use input::Sending;
pub use layers::{local, user};
pub use mcp::McpServer;
pub use menu::Forced;
pub use output::{Color, Glyphs, ScreenMode, ThemeChoice, ToolDetail, TranscriptColours};
pub use sandbox::SandboxSettings;
pub use updates::Updates;
pub use variables::ScrollSpeed;

pub(crate) use variables::{refused, spelled};

/// What every layer together says a setting is.
///
/// Built once at startup and read from there on, so nothing on the turn path
/// touches a file.
#[derive(Clone, Default)]
pub struct Settings {
    value: Value,

    /// Prompt-cache policy retains per-field authority provenance outside the
    /// generic JSON merge, whose list rule is concatenation rather than the
    /// intersection this security boundary requires.
    prompt_cache: PromptCachePolicy,

    /// Host confinement enablement, disabled unless configured and weakened only by
    /// a user-originated layer.
    sandbox: SandboxSettings,
    /// Every layer's rules together. Held apart from the value because a rule
    /// is read where it is written — see [`Document::parse`] — and what survives
    /// the layering is the rule rather than its text.
    rules: Rules,
    /// The keys of the menu rows a workspace layer states, which the user's
    /// own file cannot change. Bounded by the rows there are.
    pinned: Vec<&'static str>,
}

impl fmt::Debug for Settings {
    /// Written by hand so the `env` block is redacted, each server's
    /// arguments shown as a reader is shown them, what an extension was told
    /// is hidden, and a `baseUrl` shows its recipient alone. This type is
    /// what the wiring above holds for the whole session, so it is the one
    /// most likely to end up inside somebody's diagnostic — and it holds
    /// every variable the two private layers set, values and all.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("value", &env::Redacted(&self.value))
            .field("rules", &self.rules)
            .field("prompt_cache", &self.prompt_cache)
            .field("sandbox", &self.sandbox)
            .field("pinned", &self.pinned)
            .finish()
    }
}

impl Settings {
    /// Resolves the documents that were found, in any order.
    ///
    /// Order is taken from each document's origin rather than from the sequence
    /// they arrive in, so a caller that reads the files in a different order
    /// than it lists them still gets the same answer.
    ///
    /// Not public: [`Settings::read`] is the one way in, so there is no second
    /// path that could find a different set of files than the first one does.
    #[cfg(test)]
    pub(crate) fn resolve(documents: Vec<Document>) -> Self {
        Self::resolve_checked(documents).expect("sample documents form a compatible policy")
    }

    /// Resolves documents while retaining the configuration error for a
    /// cross-layer prompt-cache authority conflict.
    pub(crate) fn resolve_checked(
        mut documents: Vec<Document>,
    ) -> Result<Self, crate::ConfigError> {
        documents.sort_by_key(|document| document.origin().nearness());

        let prompt_cache = prompt_cache::resolve(&documents)?;
        let sandbox = sandbox::resolve(&documents)?;
        let pinned = menu::pinned(&documents);

        let mut value = Value::Object(Map::new());
        for document in &documents {
            merge(&mut value, document.value(), &DOCUMENT);
        }

        // Concatenated, like every list in the document and for the reason
        // `Rules::absorb` gives: a nearer layer may add to what is allowed and
        // may not take away what a farther one denied.
        let mut rules = Rules::new();
        for document in documents {
            rules.absorb(document.rules());
        }

        Ok(Self {
            value,
            prompt_cache,
            sandbox,
            rules,
            pinned,
        })
    }

    /// Which provider to ask, when the command line names none.
    ///
    /// The one setting that chooses a vendor. A key says a provider can be
    /// reached and nothing more, so this is what a machine holding two of them
    /// is settled by — and the name is read back as it was written, since which
    /// names are real is the binary's to know.
    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        self.value.get("provider")?.as_str()
    }

    /// The model to ask this provider for, when the command line names none.
    #[must_use]
    pub fn model(&self, provider: &str) -> Option<&str> {
        self.value
            .get("providers")?
            .get(provider)?
            .get("model")?
            .as_str()
    }

    /// Where this provider's requests go, when the vendor's own address is not
    /// where they should.
    ///
    /// Read back as the string it was written as. What it may be is decided
    /// where it is applied — the wiring parses it into an address a provider
    /// can be built at, and a value this returns is not yet one.
    #[must_use]
    pub fn base_url(&self, provider: &str) -> Option<&str> {
        self.value
            .get("providers")?
            .get(provider)?
            .get("baseUrl")?
            .as_str()
    }

    /// How hard to think, for every turn sent to this provider.
    ///
    /// A rung rather than the word it was written as, because the word is only
    /// ever one of five and the type that holds them is [`Effort`]'s.
    /// Nothing under `settings/` owns this one: there is no meaning here beyond
    /// the rung, and the shape is what refuses anything that is not one.
    ///
    /// `None` is "no layer said", not the middle rung — the vendor's own
    /// default is what a session nobody has an opinion about runs on.
    #[must_use]
    pub fn effort(&self, provider: &str) -> Option<Effort> {
        self.value
            .get("providers")?
            .get(provider)?
            .get("effort")?
            .as_str()?
            .parse()
            .ok()
    }

    /// The speed the user's own file asks this provider for.
    ///
    /// Fast only where the file says `true`; anything else, the key's absence
    /// included, is standard. It holds only for the `model` beside it, which
    /// `hastened` reads with it. Whether the model in force can be asked for
    /// it is not this file's to know, and is decided where the request is
    /// built.
    #[must_use]
    pub fn speed(&self, provider: &str) -> Speed {
        let fast = self
            .value
            .get("providers")
            .and_then(|all| all.get(provider))
            .and_then(|chosen| chosen.get("fast"))
            .and_then(Value::as_bool);
        if fast == Some(true) {
            Speed::Fast
        } else {
            Speed::Standard
        }
    }

    /// The name of the variable this provider's key is read from.
    ///
    /// The name only. A key has no path into a configuration file: this is the
    /// setting for somebody who keeps a work key and a personal key in two
    /// variables, and the value still comes from the environment.
    #[must_use]
    pub fn api_key_env(&self, provider: &str) -> Option<&str> {
        self.value
            .get("providers")?
            .get(provider)?
            .get("apiKeyEnv")?
            .as_str()
    }

    /// The variables the commands crucible runs are started with.
    ///
    /// Crucible's own environment is not touched: writing to it is `unsafe` in
    /// edition 2024 and this workspace denies that. So this block says what
    /// `cargo test` or `git` sees, and crucible reads its own `CRUCIBLE_CODE_`
    /// settings out of it as settings, below the environment it was started
    /// in, as [`Self::scroll_speed`] does. The one variable it reads before
    /// opening a file is refused here outright rather than left to look
    /// applied.
    ///
    /// Each value is the text a command is handed: a string as written, and a
    /// whole number as its digits, so a setting reaches a command the same
    /// whichever way the file spelled it.
    pub fn env(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> {
        self.value
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|vars| {
                vars.iter()
                    .filter_map(|(name, value)| Some((name.as_str(), variables::spelled(value)?)))
            })
    }

    /// Whether the person running crucible said this extension may run.
    ///
    /// False where nothing said, which is what an extension nobody has decided
    /// about is. `enabled` widens, so neither project file can answer this on
    /// behalf of whoever opened the checkout: an installed extension is off
    /// until the person running crucible turns it on in their own file.
    #[must_use]
    pub fn extension_enabled(&self, id: &str) -> bool {
        self.value
            .get("extensions")
            .and_then(|block| block.get(id))
            .and_then(|one| one.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The manifest digest this extension was agreed to at.
    ///
    /// `None` covers a decision that recorded none and an extension nobody has
    /// mentioned. They are one answer because they are one answer to whoever
    /// asked: nothing here says which program was agreed to, so nothing here
    /// lets one run.
    ///
    /// This widens for the reason `enabled` does. A committed file that could
    /// write it would be naming the bytes somebody else's checkout agreed to.
    #[must_use]
    pub fn extension_digest(&self, id: &str) -> Option<&str> {
        self.value
            .get("extensions")
            .and_then(|block| block.get(id))
            .and_then(|one| one.get("digest"))
            .and_then(Value::as_str)
    }

    /// The names written under this extension's `config` block.
    ///
    /// The names and not what they were set to. Crucible does not know what any
    /// of these mean, so it cannot tell which of them holds a token somebody
    /// pasted — and a listing that printed the values would be this program
    /// putting a credential on a screen to describe a key it cannot read.
    ///
    /// Empty covers a block nobody wrote, a block written empty, and an
    /// extension nobody has mentioned at all. They are one answer here because
    /// they are one answer to whoever asked: nothing was set.
    #[must_use]
    pub fn extension_settings(&self, id: &str) -> Vec<&str> {
        self.value
            .get("extensions")
            .and_then(|block| block.get(id))
            .and_then(|one| one.get("config"))
            .and_then(Value::as_object)
            .map(|written| written.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// The routes the user has said yes to sending on, under
    /// `contentUse.accepted`, as written.
    ///
    /// Only ever the user's own: the key widens, so a project file that
    /// writes it is refused before this is read. A name no route of this
    /// build answers to is handed back like any other and means nothing to
    /// whoever reads it.
    #[must_use]
    pub fn content_accepted(&self) -> Vec<&str> {
        self.value
            .get("contentUse")
            .and_then(|block| block.get("accepted"))
            .and_then(Value::as_array)
            .map(|routes| routes.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// Whether operating-system confinement is enabled, false unless a document
    /// opts in with `sandbox.enabled`.
    /// Workspace layers may require confinement but cannot disable it.
    /// Policy constructors remain conservative; hosts apply this choice.
    #[must_use]
    pub fn sandbox_enabled(&self) -> bool {
        self.sandbox.enabled()
    }

    /// Effective sandbox settings shared by command and extension hosts.
    #[must_use]
    pub const fn sandbox(&self) -> &SandboxSettings {
        &self.sandbox
    }
}

/// Lays a nearer layer over what the farther ones already said.
///
/// The shape decides how, which is what keeps the merge rule from becoming a
/// second description of the document. A scalar — `output.color` — is replaced
/// outright by the nearer layer. An object is merged key by key, so `providers`
/// and `env` take the nearest layer that mentioned each *name* rather than the
/// nearest layer that mentioned the block. A list is concatenated: a nearer
/// layer adds entries and removes none, which is the only rule that leaves a
/// `deny` written at home standing when a checked-out repository states rules
/// of its own. All three are in `docs/configuration/configuration.md`.
///
/// A [`Shape::TextSet`] is not a list here and is replaced like a scalar. The
/// sandbox's paths, domains and sockets are the text sets, and the sandbox
/// reads each layer's own document rather than this merge, because what a
/// project may add or narrow there is its decision and not this one's.
fn merge(base: &mut Value, near: &Value, shape: &'static Shape) {
    if shape.element().is_some()
        && let (Some(into), Some(from)) = (base.as_array_mut(), near.as_array())
    {
        into.extend(from.iter().cloned());
        return;
    }

    let (Some(into), Some(from)) = (base.as_object_mut(), near.as_object()) else {
        *base = near.clone();
        return;
    };

    for (key, value) in from {
        // A key with no shape is one JSON reserves — `$schema`, `$comment`.
        // Nothing merges into it, so the nearer layer's copy stands.
        match (into.get_mut(key), shape.field(key)) {
            (Some(held), Some(inner)) => merge(held, value, inner),
            _ => {
                into.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Replaces every value an extension was told in a printed document, keeping
/// the names, as [`Settings::extension_settings`] lists them.
///
/// The document-holding types print through [`env::Redacted`]. Crucible cannot
/// read these names, so it cannot tell which of them holds a key, and every
/// value goes: a nested block or list is replaced whole rather than descended
/// into, as a nested object under `env` is. A `config` that is not a block,
/// which no document that parsed holds, is replaced whole.
pub(crate) fn hide_extension_settings(shown_document: &mut Value) {
    let Some(extensions) = shown_document
        .get_mut("extensions")
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    for record in extensions.values_mut() {
        let Some(config) = record.get_mut("config") else {
            continue;
        };
        match config.as_object_mut() {
            Some(written) => {
                for value in written.values_mut() {
                    *value = Value::String(env::REDACTED.to_owned());
                }
            }
            None => *config = Value::String(env::REDACTED.to_owned()),
        }
    }
}

/// Replaces each provider's `baseUrl` in a printed document with the
/// recipient alone, as [`address::redacted`] shows it.
///
/// The wiring refuses some addresses and sends to others, but the document
/// holds what was written either way, and a line that says which host a turn
/// would have gone to is the one a reader of a diagnostic needs. The user and
/// password, the path and the query are not that, and are where a credential,
/// a tenant or a token is written. A `baseUrl` that is not text, which no
/// document that parsed holds, is replaced whole.
pub(crate) fn hide_base_url_targets(shown_document: &mut Value) {
    let Some(providers) = shown_document
        .get_mut("providers")
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    for record in providers.values_mut() {
        let Some(address) = record.get_mut("baseUrl") else {
            continue;
        };
        *address = Value::String(match address.as_str() {
            Some(written) => recipient(written),
            None => env::REDACTED.to_owned(),
        });
    }
}

/// `written` as [`address::redacted`] shows it, or replaced whole where that
/// would not be the recipient.
///
/// It reads the authority from the first `://` to the first `/`, `?` or `#`,
/// and a URL parser starts it there too and ends it at one of those three or
/// sooner, so anything it reads as a user or a path is hidden with them. Two
/// spellings break that. A scheme that is not a plain one holds whatever was
/// written before the first `://`, a user or a path among it. And a parser
/// ends the authority at a `\`, drops a tab or a line break wherever one is
/// written, and refuses a space or another control, so an authority holding
/// any of them is not one it reads. An address spelled either way is replaced
/// whole. A run of `/` after the scheme, which a parser skips, leaves an empty
/// authority here, and everything after it is hidden.
///
/// The provider refuses every one of these before it sends anything. This
/// check is where what the two print differs, because a printed setting is
/// written before any refusal is made.
fn recipient(written: &str) -> String {
    let readable = written.split_once("://").is_some_and(|(scheme, rest)| {
        let plain = !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        let unread = |c: char| c == '\\' || c.is_ascii_whitespace() || c.is_ascii_control();
        plain && !address::authority(rest).contains(unread)
    });
    if readable {
        address::redacted(written)
    } else {
        env::REDACTED.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use crate::document::Origin;

    use super::*;

    #[test]
    fn a_project_naming_one_provider_leaves_the_users_other_one_alone() {
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"model": "from-home"},
                              "openai": {"model": "also-from-home"}}}"#,
            Origin::User,
        );
        let project = Document::sample(
            r#"{"providers": {"anthropic": {"model": "from-project"}}}"#,
            Origin::Project,
        );

        // Handed over nearest-first, to prove the order they arrive in is not
        // what decides precedence.
        let settings = Settings::resolve(vec![project, user]);

        assert_eq!(settings.model("anthropic"), Some("from-project"));

        // The one the project said nothing about. A map is merged key by key,
        // so naming one provider is not a way to delete the others — a project
        // that pins its own model must not silently take away the model the
        // user set for a provider it never mentioned.
        assert_eq!(settings.model("openai"), Some("also-from-home"));
    }

    #[test]
    fn the_routes_said_yes_to_are_read_as_written_and_nothing_else_is() {
        let user = Document::sample(
            r#"{"contentUse": {"accepted": ["key:google", "key:nobody", "api.moonshot.ai"]}}"#,
            Origin::User,
        );
        let settings = Settings::resolve(vec![user]);
        assert_eq!(
            settings.content_accepted(),
            ["key:google", "key:nobody", "api.moonshot.ai"]
        );
        assert!(Settings::resolve(Vec::new()).content_accepted().is_empty());
    }

    #[test]
    fn the_provider_a_machine_asks_is_read_from_the_key_that_names_one() {
        let user = Document::sample(
            r#"{"provider": "openai",
                "providers": {"anthropic": {"model": "claude-sonnet-5"}}}"#,
            Origin::User,
        );

        let settings = Settings::resolve(vec![user]);

        // The top-level key, and only it. A model written under a provider is
        // what to ask *that* provider for, and reading it as a choice of
        // provider is the failure this key exists to end.
        assert_eq!(settings.provider(), Some("openai"));
    }

    #[test]
    fn a_machine_that_has_not_chosen_a_provider_says_so_rather_than_naming_one() {
        let user = Document::sample(
            r#"{"providers": {"openai": {"model": "gpt-5.6-terra"}}}"#,
            Origin::User,
        );

        let settings = Settings::resolve(vec![user]);

        assert_eq!(settings.provider(), None);
    }

    #[test]
    fn a_provider_can_be_told_which_variable_holds_its_key() {
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"apiKeyEnv": "WORK_ANTHROPIC_KEY"}}}"#,
            Origin::User,
        );

        let settings = Settings::resolve(vec![user]);

        // The name. Nothing in this crate ever reads the variable, and nothing
        // in a document ever carries the value.
        assert_eq!(
            settings.api_key_env("anthropic"),
            Some("WORK_ANTHROPIC_KEY")
        );
        assert_eq!(settings.api_key_env("openai"), None);
    }

    #[test]
    fn a_provider_can_be_told_how_hard_to_think_before_a_session_starts() {
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"effort": "max"}}}"#,
            Origin::User,
        );

        let settings = Settings::resolve(vec![user]);

        assert_eq!(settings.effort("anthropic"), Some(Effort::Max));

        // Not a default for the rest. A provider no layer mentioned is one the
        // vendor's own default applies to, and answering `high` here would send
        // a field nobody asked for to every model on every other list.
        assert_eq!(settings.effort("openai"), None);
    }

    #[test]
    fn a_provider_no_layer_mentioned_is_left_for_the_command_line_to_decide() {
        // None is "the files did not say", not a default. The default lives
        // where it already lives, and the wiring lays the command line over
        // this.
        assert_eq!(Settings::resolve(Vec::new()).model("anthropic"), None);
    }

    #[test]
    fn disabling_confinement_requires_user_authority() {
        let user = Document::sample(r#"{"sandbox":{"enabled":false}}"#, Origin::User);
        assert!(!Settings::resolve(vec![user]).sandbox_enabled());

        for origin in [Origin::Project, Origin::ProjectLocal] {
            let error = Document::parse(
                r#"{"sandbox":{"enabled":false}}"#,
                ".crucible/config.json",
                origin,
            )
            .unwrap_err();
            assert!(matches!(error, crate::ConfigError::Widening { .. }));
        }
    }

    #[test]
    fn a_project_may_strengthen_a_users_confinement_choice() {
        let user = Document::sample(r#"{"sandbox":{"enabled":false}}"#, Origin::User);
        let project = Document::sample(r#"{"sandbox":{"enabled":true}}"#, Origin::Project);

        assert!(Settings::resolve(vec![project, user]).sandbox_enabled());
    }

    #[test]
    fn env_takes_the_nearest_layer_that_named_each_variable() {
        // crucible's own names on the project side, because those are the only
        // ones a file under the working directory may set — a name whose
        // meaning this program fixes is not a way to hand a command somebody
        // else's program.
        let user = Document::sample(
            r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "12", "PAGER": "cat"}}"#,
            Origin::User,
        );
        let local = Document::sample(
            r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "30"}}"#,
            Origin::ProjectLocal,
        );

        let settings = Settings::resolve(vec![user, local]);
        let mut found: Vec<_> = settings.env().collect();
        found.sort_unstable();

        // Per name, not per block: overriding one variable in a checkout does
        // not turn off the rest of what the user set at home.
        assert_eq!(
            found,
            vec![
                ("CRUCIBLE_CODE_MOUSE_SCROLL_SPEED", Cow::Borrowed("30")),
                ("PAGER", Cow::Borrowed("cat"))
            ]
        );
    }

    #[test]
    fn scroll_speed_written_as_an_integer_is_exported_as_its_digits() {
        // A setting must reach a command the same whichever way the file spelled
        // it: `12` and `"12"` are one answer, so a command sees one variable.
        let exported = |text: &str| {
            Settings::resolve(vec![Document::sample(text, Origin::User)])
                .env()
                .map(|(name, value)| (name.to_owned(), value.to_string()))
                .collect::<Vec<_>>()
        };
        let digits = vec![(
            "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED".to_owned(),
            "12".to_owned(),
        )];

        assert_eq!(
            exported(r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": 12}}"#),
            digits
        );
        assert_eq!(
            exported(r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "12"}}"#),
            digits
        );
    }

    #[test]
    fn printing_the_settings_names_a_variable_and_shows_nothing_of_its_value() {
        // The user's own file may hold anything their commands need, so this
        // type holds secrets by design. What it must not do is print one: a
        // `{settings:?}` in a diagnostic somebody adds later is a leak nobody
        // reviewed, which is why the redaction lives in the type rather than in
        // the call sites.
        let user = Document::sample(r#"{"env": {"TOKEN": "hunter2"}}"#, Origin::User);
        let settings = Settings::resolve(vec![user]);

        let printed = format!("{settings:?}");
        assert!(printed.contains("TOKEN"), "got {printed}");
        assert!(!printed.contains("hunter2"), "got {printed}");

        // And the value is still there for the commands that need it.
        assert_eq!(
            settings.env().collect::<Vec<_>>(),
            vec![("TOKEN", Cow::Borrowed("hunter2"))]
        );
    }

    #[test]
    fn printing_the_settings_shows_everything_that_is_not_a_variable() {
        // Redacting is not a reason to print nothing. A diagnostic about which
        // model a turn asked for is exactly what this would be read for.
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"model": "from-home"}}}"#,
            Origin::User,
        );

        let printed = format!("{:?}", Settings::resolve(vec![user]));
        assert!(printed.contains("from-home"), "got {printed}");
    }

    #[test]
    fn printing_the_settings_names_what_an_extension_was_told_and_shows_none_of_it() {
        // An extension's block is opaque to crucible, so any value in it may be
        // the key the extension was told to use, whatever its kind. The names
        // stay, as a variable's do; every value goes, and what is nested under
        // one goes with it.
        let text = r#"{"extensions": {"acme.reviewer": {"enabled": true, "config": {
                         "token": "sk-not-a-real-one", "nested": {"inner": "deep-fake-value"},
                         "list": ["listed-fake-value"]}},
                       "acme.quiet": {"config": {"depth": 3}}}}"#;
        let document = Document::sample(text, Origin::User);
        let settings = Settings::resolve(vec![document.clone()]);

        for printed in [format!("{settings:?}"), format!("{document:?}")] {
            assert!(printed.contains("token"), "got {printed}");
            assert!(printed.contains("depth"), "got {printed}");
            for value in [
                "sk-not-a-real-one",
                "deep-fake-value",
                "listed-fake-value",
                "Number(3)",
            ] {
                assert!(!printed.contains(value), "{value} in {printed}");
            }
        }

        // And the extension is still told what was written.
        assert_eq!(
            settings.extension_settings("acme.reviewer"),
            vec!["list", "nested", "token"]
        );
    }

    #[test]
    fn printing_the_settings_shows_where_a_base_url_goes_and_not_who_it_goes_as() {
        // A user and a password in the address is refused where it is applied,
        // but it is held, and printed, before that. The host is what a reader
        // of the line needs; the userinfo is what nobody should read.
        let text = r#"{"providers": {"anthropic": {
                         "baseUrl": "https://user:pa55word@host.example/v1"}}}"#;
        let document = Document::sample(text, Origin::User);
        let settings = Settings::resolve(vec![document.clone()]);

        for printed in [format!("{settings:?}"), format!("{document:?}")] {
            assert!(
                printed.contains("https://host.example/[redacted]"),
                "got {printed}"
            );
            assert!(!printed.contains("pa55word"), "got {printed}");
            assert!(!printed.contains("user:"), "got {printed}");
            assert!(!printed.contains("user@"), "got {printed}");
        }

        // What is applied is what was written.
        assert_eq!(
            settings.base_url("anthropic"),
            Some("https://user:pa55word@host.example/v1")
        );
    }

    #[test]
    fn printing_the_settings_shows_where_a_base_url_goes_and_not_its_path_or_query() {
        // A gateway's path and query are where a tenant or a token is put, so
        // a printed setting names the recipient alone, as the provider's own
        // diagnostics do.
        let text = r#"{"providers": {"openai": {"baseUrl":
                         "https://host.example:8443/tenant-fake/v1?api-key=not-a-real-key"}}}"#;
        let document = Document::sample(text, Origin::User);
        let settings = Settings::resolve(vec![document.clone()]);

        for printed in [format!("{settings:?}"), format!("{document:?}")] {
            assert!(
                printed.contains("https://host.example:8443/[redacted]"),
                "got {printed}"
            );
            for hidden in ["tenant-fake", "api-key", "not-a-real-key"] {
                assert!(!printed.contains(hidden), "{hidden} in {printed}");
            }
        }

        // What is applied is what was written.
        assert_eq!(
            settings.base_url("openai"),
            Some("https://host.example:8443/tenant-fake/v1?api-key=not-a-real-key")
        );
    }

    #[test]
    fn a_base_url_spelled_so_its_authority_cannot_be_read_is_hidden_whole() {
        // Nothing may be shown as a recipient that a URL parser would read as
        // a user or a path, so an address with no plain `scheme://` in front,
        // or whose authority holds a `\`, a space or a control, is not shown
        // at all.
        for written in [
            "https:user:pa55word@host.example/v1",
            "user:pa55word@host.example",
            "https:user:pa55word@host.example/x://y",
            "https:tenant-fake/x://y",
            "localhost:8080/v1?api-key=not-a-real-key",
            "https://host.example\\tenant-fake",
            "https://\\/user:pa55word@host.example",
            "https://\t/user:pa55word@host.example/v1",
            "https://\n/user:pa55word@host.example/v1",
            "https://host.example /tenant-fake",
        ] {
            assert_eq!(recipient(written), env::REDACTED, "{written:?}");
        }
        assert_eq!(
            recipient("https://a:b@c:d@host.example:8443/v1?q=1"),
            "https://host.example:8443/[redacted]"
        );
        assert_eq!(
            recipient("https://host.example/v1/@path"),
            "https://host.example/[redacted]"
        );
    }

    #[test]
    fn a_base_url_whose_authority_follows_a_run_of_slashes_shows_no_user() {
        // A URL parser skips every `/` after `https://`, so in each of these
        // it reads `user:pa55word` as the user. Read from the first `://`, the
        // authority is empty, and the user is in what follows it, which is
        // hidden.
        for written in [
            "https:///user:pa55word@host.example/v1",
            "https:////user:pa55word@host.example",
        ] {
            assert_eq!(recipient(written), "https:///[redacted]", "{written}");
        }
    }

    #[test]
    fn an_extension_nobody_decided_about_is_off_rather_than_unknown() {
        // Nothing said is the state every installed extension starts in, and
        // it has to read as off. An answer of "unknown" here would be one more
        // caller deciding for itself what to do about somebody else's code.
        let user = Document::sample(
            r#"{"extensions": {"acme.reviewer": {"enabled": true},
                               "acme.quiet": {"enabled": false}}}"#,
            Origin::User,
        );
        let settings = Settings::resolve(vec![user]);

        assert!(settings.extension_enabled("acme.reviewer"));
        assert!(!settings.extension_enabled("acme.quiet"));
        assert!(!settings.extension_enabled("acme.never-mentioned"));
        assert!(!Settings::default().extension_enabled("acme.reviewer"));
    }

    #[test]
    fn the_digest_an_extension_was_agreed_to_at_is_read_back_or_is_absent() {
        // Absent and never-mentioned are one answer for the same reason `off`
        // is: a decision that names no bytes permits nothing, so there is
        // nothing for a caller to tell apart.
        let user = Document::sample(
            r#"{"extensions": {"acme.reviewer": {"enabled": true, "digest": "sha256:0f1e"},
                               "acme.plain": {"enabled": true}}}"#,
            Origin::User,
        );
        let settings = Settings::resolve(vec![user]);

        assert_eq!(
            settings.extension_digest("acme.reviewer"),
            Some("sha256:0f1e")
        );
        assert_eq!(settings.extension_digest("acme.plain"), None);
        assert_eq!(settings.extension_digest("acme.never-mentioned"), None);
        assert_eq!(Settings::default().extension_digest("acme.reviewer"), None);
    }

    #[test]
    fn the_names_written_for_an_extension_come_back_without_what_was_set() {
        // The names alone, because this is what the listing shows and a value
        // is the one part that could be a token somebody pasted. What crucible
        // knows about them is that they were written; what they mean is the
        // extension's.
        let user = Document::sample(
            r#"{"extensions": {"acme.reviewer": {"enabled": true, "config": {
                                 "style": "terse", "token": "sk-not-a-real-one",
                                 "depth": 3}},
                               "acme.quiet": {"enabled": true, "config": {}},
                               "acme.plain": {"enabled": true}}}"#,
            Origin::User,
        );
        let settings = Settings::resolve(vec![user]);

        // Sorted, because the document is read into a map that holds its keys
        // that way. A listing whose rows moved between two runs of a program
        // nobody changed would read as something having happened.
        assert_eq!(
            settings.extension_settings("acme.reviewer"),
            vec!["depth", "style", "token"]
        );

        // Three ways of having nothing written, all one answer: the caller is
        // showing a person what was set, and a block left empty and a block
        // never opened are the same thing to them.
        assert!(settings.extension_settings("acme.quiet").is_empty());
        assert!(settings.extension_settings("acme.plain").is_empty());
        assert!(
            settings
                .extension_settings("acme.never-mentioned")
                .is_empty()
        );
        assert!(
            Settings::default()
                .extension_settings("acme.reviewer")
                .is_empty()
        );
    }
}
