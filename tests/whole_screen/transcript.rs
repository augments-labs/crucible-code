//! The corpus of model answers, as the whole-screen suite shows it.
//!
//! One answer per construct or group of constructs this program draws, each
//! streamed through the real binary by a case in `main.rs` whose name carries
//! the construct, the width and the screen mode. The answers themselves are the
//! crate's own corpus, included rather than copied: a copied answer would let a
//! capture here and the picture beside its component describe two different
//! answers, and nothing on either side would notice.
//!
//! What the cases share lives here too — the prompt one types and the closing
//! line one waits for — so the cases themselves read as the window they open
//! and the picture they keep.

mod corpus {
    include!("../../crates/crucible-tui/src/markdown/corpus.rs");
}

use std::fmt::Write as _;

use base64::Engine as _;

pub(crate) use corpus::END;

/// What every corpus case types to ask for its answer.
///
/// One prompt for all of them, because what a case is about is the answer, and
/// a prompt that changed between two cases would put a different row in each
/// record above the answer.
pub(crate) const ASK: &str = "show me the corpus\r";

/// The answer kept under `name`, which a case names rather than repeats.
///
/// A case that asked for a name the corpus does not hold is a case nobody would
/// see fail on its subject, so the miss is what it fails on.
pub(crate) fn said(name: &str) -> &'static str {
    corpus::answer(name).unwrap_or_else(|| panic!("no corpus answer named {name}"))
}

/// The file the corpus's tool edit rewrites.
pub(crate) const REWRITTEN: &str = "the-long-note.md";

/// How many lines the edit replaces, on each side of it.
///
/// More than the `Diff::LINES` lines a tool keeps, so the call reports what it
/// could not draw rather than drawing a whole file's worth of change.
pub(crate) const REWRITTEN_LINES: usize = 100;

/// One version of [`REWRITTEN`], every line saying which version it is.
pub(crate) fn rewrite(tense: &str) -> String {
    (1..=REWRITTEN_LINES).fold(String::new(), |mut file, at| {
        let _ = writeln!(file, "the line that {tense} here, number {at}");
        file
    })
}

/// The call the scripted vendor makes: every line of the old file replaced.
pub(crate) fn rewrite_input() -> String {
    serde_json::json!({
        "path": REWRITTEN,
        "find": rewrite("was"),
        "replace": rewrite("is now"),
    })
    .to_string()
}

/// The first block of `name`'s answer that opens with `opening`, as the answer
/// wrote it: from that line down to the blank line, or the end, that closes it.
///
/// The text a selection over the drawn block has to put on the clipboard, read
/// out of the corpus rather than written beside it, for the reason the answers
/// themselves are included rather than copied.
pub(crate) fn block_of(name: &str, opening: &str) -> String {
    let answer = said(name);
    let from = answer
        .find(opening)
        .unwrap_or_else(|| panic!("no block opening {opening:?} in the {name} answer"));
    let rest = answer.get(from..).unwrap_or_default();
    let to = rest.find("\n\n").map_or(rest.len(), |at| at + 1);
    rest.get(..to).unwrap_or_default().to_owned()
}

/// What crucible asked the terminal to put on the clipboard, in order.
///
/// Read out of the command strings the window was sent: `52;c;` and the base64
/// of the text, which is the whole of what a terminal is given.
pub(crate) fn copied(commands: &[String]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|said| said.strip_prefix("52;c;"))
        .map(|payload| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(payload)
                .expect("a clipboard request carries base64");
            String::from_utf8(bytes).expect("a clipboard request carries text")
        })
        .collect()
}
