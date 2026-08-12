use super::*;

/// The lowest contrast ratio a colour may have against a ground it has to work
/// on. Three to one is the bar for text that carries meaning by being coloured
/// rather than by being read at length, which is every slot here.
const LEGIBLE: f64 = 3.0;

/// The two grounds every colour has to clear, because the terminal's is not
/// asked for and could be either.
const GROUNDS: [(u8, u8, u8); 2] = [(0, 0, 0), (255, 255, 255)];

/// Every slot there is.
///
/// The `match` below is what keeps this list honest: a slot added to the enum
/// stops it compiling until it has been given a place here, which is to say
/// until its colour has been checked against both grounds.
fn all() -> [Slot; 6] {
    /// Where a slot sits in the list.
    fn place(slot: Slot) -> usize {
        match slot {
            Slot::Plain => 0,
            Slot::Accent => 1,
            Slot::Strong => 2,
            Slot::Quiet => 3,
            Slot::AllowEdits => 4,
            Slot::FullAccess => 5,
        }
    }

    let slots = [
        Slot::Plain,
        Slot::Accent,
        Slot::Strong,
        Slot::Quiet,
        Slot::AllowEdits,
        Slot::FullAccess,
    ];

    for (index, slot) in slots.into_iter().enumerate() {
        assert_eq!(place(slot), index, "{slot:?} is listed twice or not at all");
    }

    slots
}

/// A palette that has already settled on a rung, without an environment to say
/// so. Every rung has to be checked, and only two of them are reachable through
/// a `TERM` a reader could plausibly have.
fn at(depth: Depth) -> Palette {
    Palette { depth }
}

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

/// The three channels of a `38;2;r;g;b` sequence, if it carries one.
fn exact_hue(sequence: &str) -> Option<(u8, u8, u8)> {
    let channels = sequence.strip_suffix('m')?.split_once("38;2;")?.1;
    let mut channels = channels.split(';').map(str::parse::<u8>);

    match (channels.next(), channels.next(), channels.next()) {
        (Some(Ok(red)), Some(Ok(green)), Some(Ok(blue))) => Some((red, green, blue)),
        _ => None,
    }
}

/// Relative luminance, as the contrast formula defines it.
fn luminance((red, green, blue): (u8, u8, u8)) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };

    0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
}

/// How far apart two colours are, as a contrast ratio.
fn contrast(one: (u8, u8, u8), other: (u8, u8, u8)) -> f64 {
    let (one, other) = (luminance(one), luminance(other));
    let (high, low) = if one > other {
        (one, other)
    } else {
        (other, one)
    };

    (high + 0.05) / (low + 0.05)
}

#[test]
fn every_colour_is_legible_on_a_dark_ground_and_on_a_light_one() {
    // The claim the whole no-detection design rests on. The ground belongs to
    // the reader and is never asked for, so one palette has to work on both --
    // and "looks fine here" is the thing this replaces.
    for slot in all() {
        let Some(hue) = exact_hue(at(Depth::Exact).open(slot)) else {
            continue;
        };

        for ground in GROUNDS {
            let ratio = contrast(hue, ground);
            assert!(
                ratio >= LEGIBLE,
                "{slot:?} is {ratio:.2}:1 against {ground:?}, under {LEGIBLE}:1"
            );
        }
    }
}

#[test]
fn the_two_slots_without_a_hue_are_the_two_that_meant_not_to_have_one() {
    // So the check above is known to have skipped only what it should. Plain is
    // the reader's own foreground and Quiet is their theme's answer to "subdued
    // on this ground", which is the one judgement worth deferring to.
    let hueless: Vec<Slot> = all()
        .into_iter()
        .filter(|slot| exact_hue(at(Depth::Exact).open(*slot)).is_none())
        .collect();

    assert_eq!(hueless, [Slot::Plain, Slot::Quiet]);
}

#[test]
fn no_slot_at_any_rung_ever_writes_a_background() {
    // The inline design in one assertion: the ground behind a row belongs to
    // the terminal, and a background attribute is how a process takes it.
    for slot in all() {
        for depth in [Depth::Exact, Depth::Indexed, Depth::Basic, Depth::Off] {
            let written = at(depth).open(slot);

            assert!(
                !written.contains("48;") && !written.contains("\x1b[4"),
                "{slot:?} at {depth:?} wrote a background: {written:?}"
            );
        }
    }
}

#[test]
fn quiet_is_the_terminals_own_answer_at_every_rung() {
    for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
        assert_eq!(at(depth).open(Slot::Quiet), "\x1b[90m", "{depth:?}");
    }
}

#[test]
fn a_run_that_writes_no_colour_writes_no_bytes() {
    let palette = Palette::plain();

    for slot in all() {
        assert_eq!(palette.open(slot), "", "{slot:?}");
    }
    assert_eq!(palette.close(), "");
    assert!(!palette.writes_color());
}

#[test]
fn colour_turned_off_outside_this_module_stops_at_the_top_of_it() {
    // Whether there is colour was settled by the configuration, the environment
    // and `is_terminal` together. A terminal shouting COLORTERM does not reopen
    // it.
    let palette = Palette::resolve(false, &environment(&[(COLORTERM, "truecolor")]));

    assert_eq!(palette.open(Slot::Accent), "");
}

#[test]
fn a_terminal_that_announces_twenty_four_bit_gets_it() {
    for announced in ["truecolor", "24bit"] {
        let palette = Palette::resolve(true, &environment(&[(COLORTERM, announced)]));

        assert_eq!(
            palette.open(Slot::Accent),
            "\x1b[38;2;18;137;127m",
            "{announced}"
        );
    }
}

#[test]
fn a_term_naming_two_hundred_and_fifty_six_gets_the_nearest_of_them() {
    let palette = Palette::resolve(true, &environment(&[(TERM, "xterm-256color")]));

    assert_eq!(palette.open(Slot::Accent), "\x1b[38;5;30m");
}

#[test]
fn a_terminal_that_says_nothing_in_particular_gets_the_sixteen_it_has() {
    let palette = Palette::resolve(true, &environment(&[(TERM, "xterm")]));

    assert_eq!(palette.open(Slot::Accent), "\x1b[36m");
}

#[test]
fn a_terminal_that_says_it_is_dumb_is_believed() {
    let palette = Palette::resolve(true, &environment(&[(TERM, "dumb")]));

    assert_eq!(palette.open(Slot::Accent), "");
}

#[test]
fn an_unset_term_is_not_guessed_at() {
    // Nothing said it was a terminal type at all, and sixteen colours written
    // to something that turns out not to want them is sixteen colours of
    // rubbish in somebody's log.
    let palette = Palette::resolve(true, &environment(&[]));

    assert_eq!(palette.open(Slot::Accent), "");
}

#[test]
fn colourterm_outranks_term() {
    // A 256-colour TERM is what a terminal emulator inherits; COLORTERM is what
    // it sets for itself.
    let palette = Palette::resolve(
        true,
        &environment(&[(COLORTERM, "truecolor"), (TERM, "xterm-256color")]),
    );

    assert_eq!(palette.open(Slot::Accent), "\x1b[38;2;18;137;127m");
}

#[test]
fn every_slot_that_has_a_colour_ends_it() {
    // An attribute left open outlives the process: the shell prompt underneath
    // inherits it, and the reader has to type `reset`.
    for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
        assert_eq!(at(depth).close(), RESET, "{depth:?}");
    }
}
