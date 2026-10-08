//! The one line a failure is said in on standard error, and how much of it.
//!
//! A fatal error, a panic and an `auth` command's refusal each say why in one
//! `crucible: ` line, and each is cut here. What the line says is its owner's
//! sentence, which quotes what a checkout or the environment chose, so it is
//! held to a ceiling and marked where it was cut, and what a terminal would
//! act on in it is written as its escape.

use crucible_client_api::Text;
use crucible_types::shown::escaped;

/// The one line a failure is said in: `crucible: `, `problem` as [`cut`]
/// keeps it with every character [`escaped`] escapes written as its escape, a
/// line break among them, and a line feed to end it.
///
/// The sentence often quotes what a checkout chose, a key in its
/// configuration or a directory's name, and no failure's own sentence runs
/// over lines: a break in one came from a value it quotes, and
/// kept, it would start a line on standard error that no failure said, which
/// a person or a script reading it would take for crucible's own. Escaped, it
/// is still the value, and the person who sees it can tell which key or
/// directory it was.
pub(super) fn failing(problem: &str) -> String {
    format!("crucible: {}\n", escaped(&cut(problem)))
}

/// `problem` held to the client contract's ceiling for an error's words,
/// [`TEXT_BYTES`](crucible_client_api::bounds::TEXT_BYTES), cut on a character
/// boundary and ending [`CUT`] where it ran over.
///
/// A value a failure quotes is bounded only by the file or the variable it
/// came from, and a configuration value can be a megabyte long; a front end
/// with no terminal is given the same failure cut to this ceiling, so the
/// terminal says no more of it than that one can. Cut before it is escaped,
/// as the configuration check cuts its report, so the ceiling counts the
/// sentence's own bytes.
pub(super) fn cut(problem: &str) -> String {
    let kept = Text::cut(problem);
    if kept.truncated() {
        format!("{}{CUT}", kept.as_str())
    } else {
        kept.as_str().to_owned()
    }
}

/// What a failure cut to its ceiling ends with, the mark the configuration
/// check and the other command-line listings put after a string they cut.
const CUT: &str = "… (cut)";
