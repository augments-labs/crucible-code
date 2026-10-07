//! What the two writers make of a character a terminal would act on.

use serde_json::ser::Formatter as _;

use super::{Escaping, escaped};

/// What [`Escaping`] writes of one string fragment.
fn written(fragment: &str) -> String {
    let mut bytes = Vec::new();
    Escaping
        .write_string_fragment(&mut bytes, fragment)
        .expect("a write into memory");
    String::from_utf8(bytes).expect("UTF-8")
}

#[test]
fn a_document_escapes_del_the_c1_controls_and_the_format_characters() {
    assert_eq!(written("a\u{7f}b\u{9b}c"), r"a\u007fb\u009bc");
    assert_eq!(written("x\u{202e}y\u{feff}"), r"x\u202ey\ufeff");
    assert_eq!(written("plain — text ✓"), "plain — text ✓");
}

#[test]
fn a_format_character_past_the_basic_plane_is_written_as_its_two_halves() {
    let said = written("tag\u{e0041}");
    assert_eq!(said, r"tag\udb40\udc41");
    let read: String = serde_json::from_str(&format!("\"{said}\"")).expect("a JSON string");
    assert_eq!(read, "tag\u{e0041}");
}

#[test]
fn escaped_text_shows_a_line_break_and_an_override_instead_of_acting_on_them() {
    assert_eq!(
        escaped("a\u{1b}[2J\n\u{9b}\u{202e}b\tc"),
        r"a\u{1b}[2J\n\u{9b}\u{202e}b\tc"
    );
    assert_eq!(escaped("plain — text ✓"), "plain — text ✓");
}
