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

/// How many rows of the transcript one notch of the wheel moves.
///
/// Spelled out rather than built from the prefix, because a name assembled at
/// run time is a name nobody can grep for. The test below is what keeps it in
/// the namespace.
pub(crate) const MOUSE_SCROLL_SPEED: &str = "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED";

/// The fewest rows a notch may move.
///
/// One rather than none. A wheel set to move nothing is a setting that looks
/// applied and does nothing, which is the failure every refusal in this module
/// exists to prevent — and a reader who wants the wheel to leave the transcript
/// alone is asking for a thing crucible no longer has to give, because the
/// screen it scrolls is its own.
const LEAST: u16 = 1;

/// The most rows a notch may move.
///
/// A screenful on most terminals. Past that the wheel stops being a scroll and
/// becomes a jump: two notches and the rows that were on screen are gone with
/// nothing between them to read, which is a worse way to lose your place than
/// scrolling too slowly ever is.
const MOST: u16 = 30;

/// What a notch moves where nothing said otherwise.
///
/// Three lines of prose per notch, which is fast enough to cross a long answer
/// in a few flicks and slow enough that the rows going past can still be read.
const USUAL: u16 = 6;

/// What a setting of this kind takes, for the message when it was given
/// something else.
///
/// A sentence rather than a list, because the answers are a range and a range
/// written out is thirty words nobody reads to the end of.
fn accepted() -> Accepted {
    Accepted::new(vec!["a whole number of rows from 1 to 30"])
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
    (name == MOUSE_SCROLL_SPEED && ScrollSpeed::read(written).is_none()).then(accepted)
}

/// How many rows of the transcript one notch of the wheel moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollSpeed(u16);

impl Default for ScrollSpeed {
    fn default() -> Self {
        Self(USUAL)
    }
}

impl ScrollSpeed {
    /// How many rows to move, as the renderer counts them.
    #[must_use]
    pub fn rows(self) -> i32 {
        i32::from(self.0)
    }

    /// Reads a whole number of rows inside the bounds above.
    ///
    /// `None` for anything else, including a number outside them. Clamping
    /// would be a setting that looks applied and does something else, and the
    /// two are equally worth refusing: somebody who wrote `600` meant something
    /// by it, and being told the range is how they find out what crucible
    /// meant.
    fn read(written: &str) -> Option<Self> {
        written
            .parse::<u16>()
            .ok()
            .filter(|rows| (LEAST..=MOST).contains(rows))
            .map(Self)
    }
}

impl Settings {
    /// How many rows of the transcript one notch of the wheel moves.
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
    pub fn scroll_speed(
        &self,
        from: &impl Fn(&str) -> Option<String>,
    ) -> Result<ScrollSpeed, ConfigError> {
        if let Some(set) = from(MOUSE_SCROLL_SPEED) {
            return ScrollSpeed::read(&set).ok_or_else(|| ConfigError::AnswerInShell {
                name: MOUSE_SCROLL_SPEED.into(),
                accepted: accepted(),
            });
        }

        Ok(self
            .env()
            .find(|(name, _)| *name == MOUSE_SCROLL_SPEED)
            .and_then(|(_, written)| ScrollSpeed::read(written))
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
        assert!(env::ours(MOUSE_SCROLL_SPEED));
    }

    #[test]
    fn every_answer_the_setting_accepts_reads_back_as_a_value() {
        // Two lists that have to agree: what `accepted()` promises a reader and
        // what `read` will actually take. Without this, moving a bound leaves
        // the message advertising an answer the program refuses.
        for rows in LEAST..=MOST {
            let written = rows.to_string();
            assert_eq!(
                ScrollSpeed::read(&written),
                Some(ScrollSpeed(rows)),
                "{rows}"
            );
            assert!(refused(MOUSE_SCROLL_SPEED, &written).is_none(), "{rows}");
        }
    }

    #[test]
    fn a_number_outside_the_bounds_is_refused_rather_than_pulled_into_them() {
        // Clamping would be a setting that looks applied and does something
        // else. Somebody who wrote 600 meant something by it, and the range is
        // what tells them what crucible meant.
        for written in ["0", "31", "600", "65536"] {
            assert_eq!(ScrollSpeed::read(written), None, "{written}");
            assert!(refused(MOUSE_SCROLL_SPEED, written).is_some(), "{written}");
        }
    }

    #[test]
    fn a_count_is_a_count_and_not_a_thing_that_looks_like_one() {
        for written in ["", " 6", "6 ", "six", "6.0", "-6", "0x6"] {
            assert_eq!(ScrollSpeed::read(written), None, "{written:?}");
        }

        // A leading plus is a spelling of the same number, and refusing it
        // would be pedantry aimed at somebody who wrote what they meant.
        assert_eq!(ScrollSpeed::read("+6"), Some(ScrollSpeed(6)));
    }

    #[test]
    fn a_setting_nothing_mentioned_moves_the_usual_amount() {
        let settings = Settings::resolve(Vec::new());

        assert_eq!(
            settings.scroll_speed(&nothing()).expect("nothing was set"),
            ScrollSpeed::default()
        );
        assert_eq!(ScrollSpeed::default().rows(), i32::from(USUAL));
    }

    #[test]
    fn a_block_says_it_for_every_run_in_this_project() {
        let project = Document::sample(
            r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "3"}}"#,
            Origin::Project,
        );
        let settings = Settings::resolve(vec![project]);

        assert_eq!(
            settings
                .scroll_speed(&nothing())
                .expect("nothing was set in the shell")
                .rows(),
            3
        );
    }

    #[test]
    fn the_shell_outranks_the_block() {
        let project = Document::sample(
            r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "3"}}"#,
            Origin::Project,
        );
        let settings = Settings::resolve(vec![project]);

        assert_eq!(
            settings
                .scroll_speed(&shell(&[(MOUSE_SCROLL_SPEED, "12")]))
                .expect("12 is an answer")
                .rows(),
            12
        );
    }

    #[test]
    fn a_value_the_setting_does_not_take_is_refused_rather_than_ignored() {
        let settings = Settings::resolve(Vec::new());

        let problem = settings
            .scroll_speed(&shell(&[(MOUSE_SCROLL_SPEED, "quickly")]))
            .expect_err("quickly is not an answer");

        let said = problem.to_string();
        assert!(said.contains(MOUSE_SCROLL_SPEED), "{said}");
        assert!(said.contains("1 to 30"), "{said}");
    }

    #[test]
    fn a_refusal_names_the_setting_and_never_the_value_beside_it() {
        // The block is the environment, so the next variable to go wrong could
        // hold a token. A message that quotes what was set is a message that
        // puts one into a log the moment that happens.
        let settings = Settings::resolve(Vec::new());

        let problem = settings
            .scroll_speed(&shell(&[(MOUSE_SCROLL_SPEED, "hunter2")]))
            .expect_err("hunter2 is not an answer");

        assert!(!problem.to_string().contains("hunter2"), "{problem}");
        assert!(!format!("{problem:?}").contains("hunter2"), "{problem:?}");
    }

    #[test]
    fn a_variable_that_is_not_crucibles_own_is_not_crucibles_business() {
        // The block is still the environment. `EDITOR=sometimes` is a string on
        // its way to a command, and this module has nothing to say about it.
        assert!(refused("EDITOR", "sometimes").is_none());
        assert!(refused(MOUSE_SCROLL_SPEED, "sometimes").is_some());
    }
}
