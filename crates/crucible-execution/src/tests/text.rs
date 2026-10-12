//! Reported text is bounded and says when it was clipped.

use crate::{Currency, TEXT_BYTES, Text};

/// Text past the ceiling is clipped on a character boundary and marked
/// incomplete; text within it is kept whole.
#[test]
fn text_past_its_ceiling_is_clipped_and_says_so() {
    let whole = Text::new("short");
    assert_eq!(whole.as_str(), "short");
    assert!(!whole.incomplete());

    let long = "é".repeat(TEXT_BYTES);
    let clipped = Text::new(&long);
    assert!(clipped.incomplete());
    assert!(clipped.as_str().len() <= TEXT_BYTES);
    assert!(clipped.as_str().chars().all(|letter| letter == 'é'));

    let odd = Text::clipped("aé", 2);
    assert_eq!(odd.as_str(), "a", "a character is never split");
    assert!(odd.incomplete());
}

/// A currency code is three upper-case letters and nothing else.
#[test]
fn a_currency_is_three_upper_case_letters() {
    assert_eq!(
        Currency::new("USD").map(|code| code.as_str().to_owned()),
        Some("USD".into())
    );
    for refused in ["usd", "US", "USDT", "U$D", ""] {
        assert_eq!(Currency::new(refused), None, "{refused}");
    }
}
