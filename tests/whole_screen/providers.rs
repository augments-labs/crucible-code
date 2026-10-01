//! The vendors this release adds, at the key box: a key that does not fit the
//! row it was typed on is refused there, with nothing stored and nothing sent,
//! and the box stands again for the one that does.

use crate::vendor::Vendor;
use crate::watched::Watched;

#[test]
fn a_key_of_another_kind_is_refused_in_the_box_and_the_box_stands_again() {
    // `MiMo` takes a pay-as-you-go key, which starts `sk-`; a key from its
    // Token Plan starts `tp-` and is answered with a 401 at this address, a
    // turn later and in words that say only that the key is wrong.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::keyless("mimo-key-marks", 80, 24, &vendor);

    window.types_until("/login mimo\r", "paste or type your API key");
    window.types_until(
        "tp-fabricated-token-plan-key\r",
        "not a key for this row; its keys start sk-",
    );
    assert!(
        window.picture().contains("paste or type your API key"),
        "{}",
        window.picture()
    );
    window.types_until("sk-fabricated-mimo-key\r", "login successful");

    let held = std::fs::read_to_string(window.home().join("auth.json")).unwrap_or_default();
    assert!(held.contains("mimo@xiaomimimo.com"), "{held}");
    assert!(
        held.contains("sk-fabricated-mimo-key"),
        "the key that fits is kept"
    );
    assert!(
        !held.contains("tp-fabricated"),
        "the refused key is never stored"
    );
    assert!(
        !window.picture().contains("tp-fabricated"),
        "{}",
        window.picture()
    );
}
