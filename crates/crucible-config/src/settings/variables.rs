//! The settings crucible reads for itself out of the `env` block.
//!
//! That block is the environment: what it holds is handed to the commands
//! crucible starts, and crucible's own settings live in it under the
//! [`NAMESPACE`](crate::env::NAMESPACE) prefix, which is what makes "this is a
//! setting, not somebody's token" checkable rather than a matter of trust.
//!
//! So each one has two places it can be written — the block, which is how a
//! project or a home directory sets it for every run, and the environment
//! crucible was started in, which is what somebody typed in front of this run.
//! The nearer of the two wins.
//!
//! A value here means something other than the string it was written as, so
//! the reading of each answer sits beside the type it produces, and the two
//! entry points below are the only ways in: [`refused`] for a document, while
//! the file it came from is still open, and a reader on [`Settings`] for the
//! answer that survives every layer.

use crate::error::{Accepted, ConfigError};

use super::Settings;

/// Whether the terminal is emptied before crucible draws its first row.
///
/// Spelled out rather than built from the prefix, because a name assembled at
/// run time is a name nobody can grep for. The test below is what keeps it in
/// the namespace.
pub(crate) const CLEAR_SCREEN: &str = "CRUCIBLE_CODE_CLEAR_SCREEN";

/// Every way of writing yes.
///
/// Two spellings because there are two places to write one: a shell variable is
/// usually set to `1` and a JSON file usually reads `true`, and refusing either
/// would be refusing the way somebody normally writes it where they wrote it.
const YES: [&str; 2] = ["1", "true"];

/// Every way of writing no.
const NO: [&str; 2] = ["0", "false"];

/// What a setting of this kind takes, for the message when it was given
/// something else.
fn accepted() -> Accepted {
    Accepted::new(YES.iter().chain(&NO).copied().collect())
}

/// Whether a name is one of crucible's own settings set to something it does
/// not take.
///
/// `Some` is a refusal, and it carries what the setting takes instead. A name
/// outside the namespace is not crucible's business — the block is the
/// environment, and a variable there is a string on its way to a command.
///
/// Asked per document rather than of the resolved settings, because this is the
/// last moment the file and the position are still known.
pub(crate) fn refused(name: &str, written: &str) -> Option<Accepted> {
    (name == CLEAR_SCREEN && ClearScreen::read(written).is_none()).then(accepted)
}

/// Whether the terminal is emptied before crucible draws its first row.
///
/// The default is not to. Crucible renders inline, so what is already on screen
/// is somebody's own work and the scrollback above it is theirs to keep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClearScreen(bool);

impl ClearScreen {
    /// Whether a clear was asked for.
    #[must_use]
    pub fn wanted(self) -> bool {
        self.0
    }

    /// Reads one of [`YES`] or [`NO`], however it was capitalised.
    ///
    /// `None` for anything else. There is no third answer to fall back to, and
    /// falling back to `no` would be a setting that looks applied and does
    /// nothing — which is the failure the refusals in this module exist to
    /// prevent.
    fn read(written: &str) -> Option<Self> {
        /// Whether a value is one of these spellings, however it was
        /// capitalised.
        fn among(answers: [&str; 2], written: &str) -> bool {
            answers
                .iter()
                .any(|answer| answer.eq_ignore_ascii_case(written))
        }

        if among(YES, written) {
            return Some(Self(true));
        }
        if among(NO, written) {
            return Some(Self(false));
        }
        None
    }
}

impl Settings {
    /// Whether to empty the terminal before crucible draws its first row.
    ///
    /// `from` is the environment crucible was started in, and it wins: it is
    /// what somebody typed in front of this run, against a block they wrote
    /// once for every run.
    ///
    /// # Errors
    ///
    /// [`ConfigError::AnswerInShell`] if the variable is set in the environment
    /// to something this cannot read. A value written in a *file* was refused
    /// while that file was still open, so the shell is the only place left that
    /// can still be wrong by the time this is asked.
    pub fn clear_screen(
        &self,
        from: &impl Fn(&str) -> Option<String>,
    ) -> Result<ClearScreen, ConfigError> {
        if let Some(set) = from(CLEAR_SCREEN) {
            return ClearScreen::read(&set).ok_or_else(|| ConfigError::AnswerInShell {
                name: CLEAR_SCREEN.into(),
                accepted: accepted(),
            });
        }

        Ok(self
            .env()
            .find(|(name, _)| *name == CLEAR_SCREEN)
            .and_then(|(_, written)| ClearScreen::read(written))
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, Origin};
    use crate::env;

    use super::*;

    /// An environment holding exactly these variables.
    fn shell(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let held: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        move |wanted| {
            held.iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.clone())
        }
    }

    /// Nothing written anywhere.
    fn nothing() -> impl Fn(&str) -> Option<String> {
        shell(&[])
    }

    #[test]
    fn every_setting_crucible_reads_for_itself_is_in_its_own_namespace() {
        // The prefix is what either workspace file is allowed to set, so a
        // name outside it would be a setting no project could use — and one
        // `check.rs` refuses in both project layers.
        assert!(env::ours(CLEAR_SCREEN));
    }

    #[test]
    fn every_answer_the_setting_accepts_reads_back_as_a_value() {
        // Two lists that have to agree: what `accepted()` promises a reader and
        // what `read` will actually take. Without this, adding a spelling to
        // one leaves the message advertising an answer the program refuses.
        for answer in YES {
            assert_eq!(
                ClearScreen::read(answer),
                Some(ClearScreen(true)),
                "{answer}"
            );
        }
        for answer in NO {
            assert_eq!(
                ClearScreen::read(answer),
                Some(ClearScreen(false)),
                "{answer}"
            );
        }
        for answer in YES.iter().chain(&NO) {
            assert!(refused(CLEAR_SCREEN, answer).is_none(), "{answer}");
        }
    }

    #[test]
    fn an_answer_is_read_however_it_was_capitalised() {
        // `TRUE` is what somebody writes in a shell often enough that refusing
        // it would only ever be pedantry.
        assert_eq!(ClearScreen::read("TRUE"), Some(ClearScreen(true)));
        assert_eq!(ClearScreen::read("False"), Some(ClearScreen(false)));
    }

    #[test]
    fn a_setting_nothing_mentioned_leaves_the_screen_alone() {
        let settings = Settings::resolve(Vec::new());

        assert_eq!(
            settings.clear_screen(&nothing()).expect("nothing was set"),
            ClearScreen::default()
        );
        assert!(!ClearScreen::default().wanted());
    }

    #[test]
    fn a_block_says_it_for_every_run_in_this_project() {
        let project = Document::sample(
            r#"{"env": {"CRUCIBLE_CODE_CLEAR_SCREEN": "true"}}"#,
            Origin::Project,
        );
        let settings = Settings::resolve(vec![project]);

        assert!(
            settings
                .clear_screen(&nothing())
                .expect("nothing was set in the shell")
                .wanted()
        );
    }

    #[test]
    fn the_shell_outranks_the_block() {
        let project = Document::sample(
            r#"{"env": {"CRUCIBLE_CODE_CLEAR_SCREEN": "true"}}"#,
            Origin::Project,
        );
        let settings = Settings::resolve(vec![project]);

        assert!(
            !settings
                .clear_screen(&shell(&[(CLEAR_SCREEN, "0")]))
                .expect("0 is an answer")
                .wanted()
        );
    }

    #[test]
    fn a_value_the_setting_does_not_take_is_refused_rather_than_ignored() {
        let settings = Settings::resolve(Vec::new());

        let problem = settings
            .clear_screen(&shell(&[(CLEAR_SCREEN, "sometimes")]))
            .expect_err("sometimes is not an answer");

        let said = problem.to_string();
        assert!(said.contains(CLEAR_SCREEN), "{said}");
        assert!(said.contains("true"), "{said}");
    }

    #[test]
    fn a_refusal_names_the_setting_and_never_the_value_beside_it() {
        // The block is the environment, so the next variable to go wrong could
        // hold a token. A message that quotes what was set is a message that
        // puts one into a log the moment that happens.
        let settings = Settings::resolve(Vec::new());

        let problem = settings
            .clear_screen(&shell(&[(CLEAR_SCREEN, "hunter2")]))
            .expect_err("hunter2 is not an answer");

        assert!(!problem.to_string().contains("hunter2"), "{problem}");
        assert!(!format!("{problem:?}").contains("hunter2"), "{problem:?}");
    }

    #[test]
    fn a_variable_that_is_not_crucibles_own_is_not_crucibles_business() {
        // The block is still the environment. `EDITOR=sometimes` is a string on
        // its way to a command, and this module has nothing to say about it.
        assert!(refused("EDITOR", "sometimes").is_none());
        assert!(refused(CLEAR_SCREEN, "sometimes").is_some());
    }
}
