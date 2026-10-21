//! What reaches an event from the program's own text.

use crucible_execution::Text;
use crucible_types::{Change, Diff, Line};

use crate::host::{hidden, hidden_diff};

/// A value cut where its text was clipped still names part of the secret, so
/// the part left at the cut is withheld too.
#[test]
fn a_secret_cut_where_its_text_was_clipped_shows_none_of_itself() {
    let secret: Box<str> = "sentinel-c0ffee-5ec7e7".into();
    let said = format!("token {secret} and more");
    let clipped = Text::clipped(&said, "token sentinel-c0".len());

    let shown = hidden(&clipped, &[secret]);

    assert!(
        !shown.as_str().contains("sentinel-c0"),
        "{}",
        shown.as_str()
    );
    assert!(shown.as_str().starts_with("token "), "{}", shown.as_str());
    assert!(shown.incomplete());
}

/// A whole value is withheld wherever it appears.
#[test]
fn a_whole_secret_is_withheld() {
    let secret: Box<str> = "sentinel-c0ffee-5ec7e7".into();

    let shown = hidden(&Text::new(&format!("a {secret} b")), &[secret]);

    assert_eq!(shown.as_str(), "a [withheld] b");
}

/// A change the agent reports is its text too: a value is withheld from each
/// of its lines, and from a line cut at the most a line carries, the part
/// left at the cut as well, while the counts still say what moved.
#[test]
fn a_secret_in_a_reported_change_shows_none_of_itself() {
    let secret: Box<str> = "sentinel-c0ffee-5ec7e7".into();
    let long = format!("{}{secret}", "x".repeat(Line::TEXT - "sentinel-c0".len()));
    let diff = Diff::new([
        Line::new(1, Change::Removed, format!("key = {secret}")),
        Line::new(1, Change::Added, long),
    ]);

    let shown = hidden_diff(&diff, &[secret]).unwrap();

    let lines: Vec<&str> = shown.lines().iter().map(Line::text).collect();
    assert!(
        lines.iter().all(|line| !line.contains("sentinel")),
        "{lines:?}"
    );
    assert_eq!(lines.first(), Some(&"key = [withheld]"));
    assert_eq!((shown.added(), shown.removed()), (1, 1));
}
