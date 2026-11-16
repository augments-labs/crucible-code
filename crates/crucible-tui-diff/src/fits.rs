//! The width rule held over every drawer in this crate, at every width and
//! room the components of `crucible-tui` are held to.
//!
//! A drawer named here is swept; one that is not, the repository gate finds:
//! every `impl Draws for` in this crate's sources must name its type in this
//! file. The fixtures are hostile on purpose: a path too long to fit and too
//! wide to break, a section name and lines that run past any width, and a
//! file long enough to fold.

use crucible_tui::testing::{fits, rooms};

use crate::{Change, Changes, File, Side};

/// A diff as hostile as its kind gets inside its caps.
const HOSTILE: &str = "diff --git a/a/very/long/path/that/does/not/fit/anywhere/in/a/narrow/window.rs b/a/very/long/path/that/does/not/fit/anywhere/in/a/narrow/window.rs
--- a/a/very/long/path/that/does/not/fit/anywhere/in/a/narrow/window.rs
+++ b/a/very/long/path/that/does/not/fit/anywhere/in/a/narrow/window.rs
@@ -1,3 +1,3 @@ impl Something for SomethingElseWithAVeryLongNameIndeed
 fn kept_line_that_runs_on_and_on_well_past_any_width_a_terminal_could_offer() {}
-let 漢字漢字漢字 = \"a string with wide characters 漢字漢字漢字漢字漢字漢字漢字\";
+let 漢字漢字漢字 = \"a string with wide characters 漢字漢字漢字漢字漢字漢字漢字漢字\";
 \tindented\twith\ttabs\tall\tthe\tway\tacross\tthe\tline
@@ -9000,1 +9000,1 @@
-x
+y
rename from old/name.txt
rename to new/name.txt
Binary files a/picture.png and b/picture.png differ
";

#[test]
fn a_change_read_from_a_fence_fits_every_room() {
    fits("a change read from a diff", &Changes, HOSTILE);
}

#[test]
fn a_tool_s_edit_fits_every_room() {
    let lines: Vec<(usize, Side, String)> = (1..=60)
        .map(|at| {
            let side = match at % 3 {
                0 => Side::Removed,
                1 => Side::Added,
                _ => Side::Kept,
            };
            (
                at * 7,
                side,
                format!("{at} {}", "a long line of a file ".repeat(at % 9)),
            )
        })
        .collect();
    let change = Change::new(vec![
        File::numbered(
            Some("a/path/to/a/file/somewhere/deep/inside/the/repository.rs"),
            lines
                .iter()
                .map(|(at, side, text)| (*at, *side, text.as_str())),
        )
        .dropping(9_999),
        File::numbered(None, [(1, Side::Added, "x")]),
    ]);
    for room in rooms() {
        for row in Changes.draw(&change, &room).rows {
            assert!(
                row.columns() <= room.columns,
                "a tool's edit drew {} columns into {room:?}: {:?}",
                row.columns(),
                row.text()
            );
        }
    }
}
