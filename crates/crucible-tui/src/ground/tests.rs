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
