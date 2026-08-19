use super::*;

/// An environment holding exactly these variables.
fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let held: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();

    move |wanted| {
        held.iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.clone())
    }
}

#[test]
fn the_variable_answers_with_the_last_field_it_holds() {
    // `fg;bg`, and the three-field form some terminals write instead. What is
    // wanted is the last one either way.
    assert_eq!(
        seeded(&environment(&[("COLORFGBG", "15;0")])),
        Some(Ground::Dark)
    );
    assert_eq!(
        seeded(&environment(&[("COLORFGBG", "0;15")])),
        Some(Ground::Light)
    );
    assert_eq!(
        seeded(&environment(&[("COLORFGBG", "15;default;0")])),
        Some(Ground::Dark)
    );
}

#[test]
fn the_dark_half_of_the_sixteen_is_nought_to_six_and_eight() {
    // The rxvt convention, which is the only thing this variable has ever
    // meant: 7 is white and 9 upwards are the bright half.
    for dark in [0, 1, 2, 3, 4, 5, 6, 8] {
        assert_eq!(
            seeded(&environment(&[("COLORFGBG", &format!("7;{dark}"))])),
            Some(Ground::Dark),
            "background {dark}"
        );
    }

    for light in [7, 9, 10, 11, 12, 13, 14, 15] {
        assert_eq!(
            seeded(&environment(&[("COLORFGBG", &format!("0;{light}"))])),
            Some(Ground::Light),
            "background {light}"
        );
    }
}

#[test]
fn a_variable_that_says_nothing_usable_says_nothing_at_all() {
    // None rather than a guess. Nothing downstream treats it as a failure --
    // the prompt row is drawn correctly with no ground known.
    for said in ["", "15", "15;", "15;default", "15;99", "15;-1", "15;white"] {
        assert_eq!(
            seeded(&environment(&[("COLORFGBG", said)])),
            None,
            "{said:?}"
        );
    }

    assert_eq!(seeded(&environment(&[])), None);
}

#[test]
fn an_answer_is_read_at_every_width_a_component_can_be_written_in() {
    // One to four hex digits each, and each scaled against its own maximum
    // rather than against 255 -- `rgb:f/f/f` is white, not almost black.
    assert_eq!(rgb("rgb:0/0/0"), Some((0, 0, 0)));
    assert_eq!(rgb("rgb:f/f/f"), Some((255, 255, 255)));
    assert_eq!(rgb("rgb:ff/ff/ff"), Some((255, 255, 255)));
    assert_eq!(rgb("rgb:ffff/ffff/ffff"), Some((255, 255, 255)));
    assert_eq!(rgb("rgb:fff/fff/fff"), Some((255, 255, 255)));
    assert_eq!(rgb("rgb:ffff/0000/0000"), Some((255, 0, 0)));
}

#[test]
fn the_hash_spellings_are_read_too() {
    assert_eq!(rgb("#000000"), Some((0, 0, 0)));
    assert_eq!(rgb("#ffffff"), Some((255, 255, 255)));
    assert_eq!(rgb("#ffffffffffff"), Some((255, 255, 255)));
    assert_eq!(rgb("#ff0000"), Some((255, 0, 0)));
}

#[test]
fn the_alpha_a_terminal_appends_is_not_part_of_the_colour() {
    assert_eq!(rgb("rgba:ffff/0000/0000/ffff"), Some((255, 0, 0)));
}

#[test]
fn an_answer_in_no_spelling_at_all_is_no_answer() {
    for said in [
        "",
        "rgb:",
        "rgb:f/f",
        "rgb:g/g/g",
        "#",
        "#fffff",
        "#zzzzzz",
        "12,34,56",
    ] {
        assert_eq!(rgb(said), None, "{said:?}");
        assert_eq!(replied(said), None, "{said:?}");
    }
}

#[test]
fn an_answer_becomes_a_ground_by_how_much_light_comes_off_it() {
    assert_eq!(replied("rgb:0000/0000/0000"), Some(Ground::Dark));
    assert_eq!(replied("rgb:ffff/ffff/ffff"), Some(Ground::Light));
    // The grounds the design measured against, each on the side it belongs.
    assert_eq!(replied("#0d0d10"), Some(Ground::Dark));
    assert_eq!(replied("#1e1e1e"), Some(Ground::Dark));
    assert_eq!(replied("#282c34"), Some(Ground::Dark));
    assert_eq!(replied("#f7f7f4"), Some(Ground::Light));
    assert_eq!(replied("#fdf6e3"), Some(Ground::Light));
}
