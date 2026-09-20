//! What the model is told about commands that ended while nobody waited.
//!
//! The stable operator instructions a turn is asked under are the
//! application's — `crucible_app::startup::under` — and no session fact enters
//! them. What is written here is the one note the loop adds about the session
//! itself: which background commands have ended since the model last heard.

use std::fmt::Write as _;

use crucible_builtins::Ended;

use crate::cli::draw::spelled;

/// What has ended, in the words the model is told it in.
///
/// `None` where nothing has, which is almost every turn: a note about nothing is
/// a sentence the model has to read past to find the ones that mean something.
///
/// One wording for three ways of arriving. Under a turn being started, into a
/// turn already running, or — where nobody is at the keyboard and nothing is
/// running — as the turn itself; see `converse::typing`. A model that read one
/// sentence about a build that fell over and a different one about the same
/// build depending on who happened to get there first would be reading about
/// two builds. So it says "have ended" rather than naming a boundary: which
/// turn it lands in is not a fact about the command.
///
/// It opens by saying who is speaking because two of those three ways record it
/// as a message from the developer — that is the only channel a turn already
/// running has — and a model that took this for something a person typed would
/// answer the person about it.
pub(crate) fn said(ended: &[Ended]) -> Option<String> {
    if ended.is_empty() {
        return None;
    }

    let mut said = String::from(
        "crucible, not the developer: commands you left running have ended. They are \
         gone and nothing is waiting on them; what each of them printed is below, so \
         there is nothing left to run to find out how one went:",
    );

    for one in ended {
        let how = match (&one.unpublished, one.code) {
            (Some(_), _) => "ended".to_owned(),
            (None, Some(0)) => "finished".to_owned(),
            (None, Some(code)) => format!("failed with exit status {code}"),
            (None, None) => "was killed".to_owned(),
        };
        // In the same sentence, because a model told only that a command ended
        // would go on as though the files it wrote were there.
        let unpublished = one.unpublished.as_ref().map_or_else(String::new, |why| {
            // One line of it: the note is a list, and a reason carrying its own
            // line break would read as the next command's line.
            let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(", but nothing it wrote was published: {why}")
        });

        let _ = write!(
            said,
            "\n- #{} {} — {how} after printing {} lines{unpublished}.",
            one.number,
            spelled(one.tool, &one.called),
            one.lines
        );

        // Indented under the line it belongs to, because several commands can
        // end into one note and unindented output would read as the note's own
        // words — or as the next command's.
        if !one.printed.trim().is_empty() {
            let _ = write!(said, " What it printed:\n\n{}", indented(&one.printed));
        }
    }

    Some(said)
}

/// Every line of `printed` moved in by four spaces.
fn indented(printed: &str) -> String {
    printed
        .trim_end()
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
