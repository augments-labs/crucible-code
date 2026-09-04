//! The layers, resolved into the settings a turn actually runs with.
//!
//! This file holds what every block shares — the layering, and the merge that
//! decides which layer wins a position. A block that carries meaning of its own
//! has a module beside it under `settings/`, so its keys, the types they become
//! and the tests for both sit in one place. `providers` and `env` stay here:
//! what they hold is read straight back out. The one value that is not a string
//! afterwards — how hard to think — becomes a type this crate does not own, so
//! there is no meaning here for a module to hold.

use std::fmt;

use crucible_core::{Effort, PromptCachePolicy, Rules};
use serde_json::{Map, Value};

use crate::document::Document;
use crate::env;
use crate::shape::{DOCUMENT, Shape};

mod compaction;
mod input;
mod layers;
mod mcp;
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
pub use output::{Color, Glyphs, ThemeChoice, ToolDetail};
pub use updates::Updates;
pub use variables::ScrollSpeed;

pub(crate) use variables::refused;

/// What every layer together says a setting is.
///
/// Built once at startup and read from there on, so nothing on the turn path
/// touches a file.
#[derive(Clone)]
pub struct Settings {
    value: Value,

    /// Prompt-cache policy retains per-field authority provenance outside the
    /// generic JSON merge, whose list rule is concatenation rather than the
    /// intersection this security boundary requires.
    prompt_cache: PromptCachePolicy,

    /// Host confinement mode, disabled unless configured and weakened only by
    /// a user-originated layer.
    sandbox: crucible_core::SandboxMode,

    /// Every layer's rules together. Held apart from the value because a rule
    /// is read where it is written — see [`Document::parse`] — and what survives
    /// the layering is the rule rather than its text.
    rules: Rules,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            value: Value::default(),
            prompt_cache: PromptCachePolicy::default(),
            sandbox: sandbox::resolve(&[]),
            rules: Rules::default(),
        }
    }
}

impl fmt::Debug for Settings {
    /// Written by hand so the `env` block is redacted. This type is what the
    /// wiring above holds for the whole session, so it is the one most likely
    /// to end up inside somebody's diagnostic — and it holds every variable
    /// the two private layers set, values and all.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("value", &env::Redacted(&self.value))
            .field("rules", &self.rules)
            .field("prompt_cache", &self.prompt_cache)
            .field("sandbox", &self.sandbox)
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
        let sandbox = sandbox::resolve(&documents);

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
    /// ever one of five and the type that holds them is [`crucible_core`]'s.
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
    /// Crucible's own environment is not touched and cannot be: writing to it
    /// is `unsafe` in edition 2024 and this workspace forbids that. So this
    /// block says what `cargo test` or `git` sees, not what crucible sees —
    /// crucible's own settings have keys of their own, and the one variable it
    /// reads before opening a file is refused here outright rather than left to
    /// look applied.
    pub fn env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.value
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|vars| {
                vars.iter()
                    .filter_map(|(name, value)| Some((name.as_str(), value.as_str()?)))
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

    /// Effective operating-system confinement mode, off unless a document
    /// opts in with `sandbox.enabled` or explicitly selects `sandbox.mode`.
    /// Workspace layers may require confinement but cannot disable it.
    /// Core policy constructors remain conservative; hosts apply this choice.
    #[must_use]
    pub const fn sandbox_mode(&self) -> crucible_core::SandboxMode {
        self.sandbox
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
    fn explicit_confinement_modes_remain_subject_to_user_authority() {
        let user = Document::sample(r#"{"sandbox":{"mode":"off"}}"#, Origin::User);
        assert_eq!(
            Settings::resolve(vec![user]).sandbox_mode(),
            crucible_core::SandboxMode::Off
        );

        for origin in [Origin::Project, Origin::ProjectLocal] {
            let error = Document::parse(
                r#"{"sandbox":{"mode":"degraded"}}"#,
                ".crucible/config.json",
                origin,
            )
            .unwrap_err();
            assert!(matches!(error, crate::ConfigError::Widening { .. }));
        }
    }

    #[test]
    fn a_project_may_strengthen_a_users_confinement_choice() {
        let user = Document::sample(r#"{"sandbox":{"mode":"off"}}"#, Origin::User);
        let project = Document::sample(r#"{"sandbox":{"mode":"required"}}"#, Origin::Project);

        assert_eq!(
            Settings::resolve(vec![project, user]).sandbox_mode(),
            crucible_core::SandboxMode::Required
        );
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
            vec![("CRUCIBLE_CODE_MOUSE_SCROLL_SPEED", "30"), ("PAGER", "cat")]
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
            vec![("TOKEN", "hunter2")]
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
