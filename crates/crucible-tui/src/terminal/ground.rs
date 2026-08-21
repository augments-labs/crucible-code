//! Asking the terminal what colour its own background is.
//!
//! The reading of an answer is [`crate::ground`], which is pure and has no
//! terminal in it. This is the half that has one: it writes the question,
//! waits a bounded time, and hands back the channels or nothing.
//!
//! **Nothing here is on the render path, and it runs once.** The answer is what
//! a palette is settled from, and a palette is settled at startup — so this is
//! paid for once, before the first frame, and never again.
//!
//! **A terminal that does not answer costs the wait and nothing else.** Which
//! is why the wait is short and named by the caller rather than left to a
//! default: this process is on a startup budget, and an answer that arrives
//! after the budget is worth less than the budget is.
//!
//! **Nothing is left behind.** The question is a write and not a mode, and the
//! terminal state it needs to read the reply is taken and handed back by the
//! crate underneath — which is the whole reason that crate is here rather than
//! a hand-rolled read: the platform branching, the multiplexer passthrough and
//! the terminals that answer nothing at all are the parts that are easy to get
//! wrong and impossible to notice.

use std::io::IsTerminal;
use std::time::Duration;

/// The variables that say the terminal is not on this machine, or is behind
/// something that has to relay the question.
///
/// Any of them and the question is not asked at all. The reason is not the
/// latency itself — it is what a late answer does. The crate underneath returns
/// on the timeout without draining what it was waiting for, so a reply that
/// arrives afterwards is still in the terminal's input queue when the prompt
/// starts reading keys: the reader's first line opens with
/// `]11;rgb:1c1c/1c1c/1c1c` already typed into it.
///
/// A patience long enough to cover a link like that would be most of a startup
/// budget spent on one hue. So the trade is made the other way round: where an
/// answer would be slow, it is not asked for, and the palette blends its band
/// off the ground its table was drawn for instead.
const RELAYED: [&str; 4] = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "TMUX"];

/// What the terminal says its background is, in exact channels.
///
/// `None` for every way this can decline, and they are all ordinary — output is
/// redirected, the terminal does not implement the question, it answered
/// something unrecognised, or it did not answer inside `patience`. Not
/// answering is the common case rather than the odd one: the question is not
/// widely implemented. The caller has a correct thing to do with `None`, so
/// nothing here is an error worth propagating.
#[must_use]
pub fn asked(patience: Duration, from: &dyn Fn(&str) -> Option<String>) -> Option<(u8, u8, u8)> {
    if relayed(from) {
        return None;
    }

    // Both ends, for the reason `Raw::enter` checks both: a question needs a
    // terminal to answer it, and an answer is only worth having if the thing
    // that would use it is drawing on one. Asked of a pipe, the query bytes
    // would end up in whatever kept the output.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }

    // Built rather than spelled out: the options are marked non-exhaustive, so
    // a later release adding one leaves this compiling with whatever it decided
    // the default is, which is the right way round for a question this is
    // deliberately not the expert on.
    let mut options = terminal_colorsaurus::QueryOptions::default();
    options.timeout = patience;

    let asked = terminal_colorsaurus::background_color(options);

    asked.ok().map(|colour| colour.scale_to_8bit())
}

/// Whether anything says this terminal is somewhere else, or behind something.
fn relayed(from: &dyn Fn(&str) -> Option<String>) -> bool {
    if RELAYED
        .iter()
        .any(|named| from(named).is_some_and(|set| !set.is_empty()))
    {
        return true;
    }

    // `screen` multiplexes too, and announces itself in the terminal type
    // rather than in a variable of its own.
    from("TERM").is_some_and(|term| term.starts_with("screen"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment holding exactly these variables.
    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
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

    #[test]
    fn a_terminal_somewhere_else_is_not_asked() {
        // What this buys is not speed. A reply that arrives after the patience
        // has run out is left in the input queue by the crate underneath, and
        // the prompt reads it as though somebody had typed it.
        for named in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "TMUX"] {
            assert!(relayed(&environment(&[(named, "something")])), "{named}");
        }

        assert!(relayed(&environment(&[("TERM", "screen.xterm-256color")])));
        assert!(relayed(&environment(&[("TERM", "screen")])));
    }

    #[test]
    fn a_variable_set_to_nothing_is_not_set() {
        // What a shell leaves behind when somebody unsets one the wrong way.
        assert!(!relayed(&environment(&[("SSH_TTY", "")])));
    }

    #[test]
    fn an_ordinary_local_terminal_is_asked() {
        assert!(!relayed(&environment(&[("TERM", "xterm-256color")])));
        assert!(!relayed(&environment(&[])));
    }

    #[test]
    fn nothing_is_asked_of_a_pipe_however_the_environment_looks() {
        // Under the suite neither end is a terminal, so this is the branch that
        // runs — and it is the one that must never write the query into
        // whatever kept the output.
        assert_eq!(asked(Duration::from_millis(1), &environment(&[])), None);
    }
}
