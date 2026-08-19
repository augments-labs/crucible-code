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

/// What the terminal says its background is, in exact channels.
///
/// `None` for every way this can decline, and they are all ordinary: output is
/// redirected, the terminal does not implement the question, it answered
/// something unrecognised, or it did not answer inside `patience`. The caller
/// has a correct thing to do with `None` — the row a prompt is left on simply
/// takes no ground — so nothing here is an error worth propagating.
#[must_use]
pub fn asked(patience: Duration) -> Option<(u8, u8, u8)> {
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
