//! How the terminal in hand reads the line of a file it is asked to open.
//!
//! Every terminal that opens links hands a `file://` address to an editor, and
//! they part on one thing: where the line goes. JetBrains reads it from `:12`
//! after the path and takes a fragment as part of the file's name; the rest
//! read the fragment, or drop it and open the file. JetBrains says it is the
//! one in hand, in `TERMINAL_EMULATOR`, so that is the one name read, and
//! anything else is a terminal that reads a fragment.

use std::path::Path;

use crucible_tui::{Files, Line};

/// What JetBrains sets `TERMINAL_EMULATOR` to in the terminals it runs.
const JETBRAINS: &str = "JetBrains-JediTerm";

/// Paths in the answer read against `root`, with the line written the way the
/// terminal `emulator` names reads one.
pub(crate) fn files(root: &Path, emulator: Option<&str>) -> Files {
    let line = if emulator == Some(JETBRAINS) {
        Line::Colon
    } else {
        Line::Fragment
    };
    Files::new(root, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetbrains_is_handed_the_line_after_a_colon_and_the_rest_a_fragment() {
        let root = Path::new("/repo");

        assert_eq!(
            files(root, Some(JETBRAINS)).address("a.rs:3").as_deref(),
            Some("file:///repo/a.rs:3")
        );
        for emulator in [None, Some("xterm"), Some("")] {
            assert_eq!(
                files(root, emulator).address("a.rs:3").as_deref(),
                Some("file:///repo/a.rs#3"),
                "{emulator:?}"
            );
        }
    }
}
