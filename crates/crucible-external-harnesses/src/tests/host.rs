//! What reaches an event from the program's own text.

use crucible_execution::Text;

use crate::host::hidden;

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
