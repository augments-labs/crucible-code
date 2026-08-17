use super::*;

/// The lowest contrast ratio a colour may have against a ground it has to work
/// on. Three to one is the bar for text that carries meaning by being coloured
/// rather than by being read at length, which is every slot here.
const LEGIBLE: f64 = 3.0;

/// The two grounds every colour has to clear, because the terminal's is not
/// asked for and could be either.
const GROUNDS: [(u8, u8, u8); 2] = [(0, 0, 0), (255, 255, 255)];

/// The slots that take the ground, in the order a row of them is built.
const DIFF: [Slot; 4] = [
    Slot::Removed,
    Slot::RemovedNumber,
    Slot::Added,
    Slot::AddedNumber,
];

/// Every slot there is.
///
/// The `match` below is what keeps this list honest: a slot added to the enum
/// stops it compiling until it has been given a place here, which is to say
/// until its colour has been checked against both grounds.
fn all() -> [Slot; 10] {
    /// Where a slot sits in the list.
    fn place(slot: Slot) -> usize {
        match slot {
            Slot::Plain => 0,
            Slot::Accent => 1,
            Slot::Strong => 2,
            Slot::Quiet => 3,
            Slot::AllowEdits => 4,
            Slot::FullAccess => 5,
            Slot::Removed => 6,
            Slot::RemovedNumber => 7,
            Slot::Added => 8,
            Slot::AddedNumber => 9,
        }
    }

    let slots = [
        Slot::Plain,
        Slot::Accent,
        Slot::Strong,
        Slot::Quiet,
        Slot::AllowEdits,
        Slot::FullAccess,
        Slot::Removed,
        Slot::RemovedNumber,
        Slot::Added,
        Slot::AddedNumber,
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

/// What a sequence does to one half of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sets {
    /// Nothing: that half is left to the terminal's own theme.
    Nothing,
    /// A colour, at a rung that names one rather than spelling it out.
    Named,
    /// A colour, with its three channels written out.
    Exact((u8, u8, u8)),
}

/// The ink and the ground a sequence sets, in that order.
///
/// Walked in order rather than searched for, because a channel is a number like
/// any other: `30` is a blue channel inside `48;2;74;26;30` and the plain
/// foreground anywhere else, and a scan that did not consume what `48` brought
/// with it would read the first as the second.
fn sets(sequence: &str) -> (Sets, Sets) {
    let (mut ink, mut ground) = (Sets::Nothing, Sets::Nothing);
    let Some(body) = sequence
        .strip_prefix("\x1b[")
        .and_then(|rest| rest.strip_suffix('m'))
    else {
        return (ink, ground);
    };

    let mut params = body.split(';');
    while let Some(param) = params.next() {
        match param {
            "38" => ink = extended(&mut params),
            "48" => ground = extended(&mut params),
            _ if named(param, 30) => ink = Sets::Named,
            _ if named(param, 40) => ground = Sets::Named,
            _ => {}
        }
    }

    (ink, ground)
}

/// Whether `param` is one of the eight codes at `base`, or one of the eight
/// bright ones sixty above it.
fn named(param: &str, base: u16) -> bool {
    param.parse::<u16>().is_ok_and(|code| {
        (base..base + 8).contains(&code) || (base + 60..base + 68).contains(&code)
    })
}

/// What a `38` or a `48` brings with it: an index, or three channels.
fn extended(params: &mut core::str::Split<'_, char>) -> Sets {
    match params.next() {
        Some("2") => channels(params).map_or(Sets::Named, Sets::Exact),
        Some("5") => {
            params.next();
            Sets::Named
        }
        _ => Sets::Named,
    }
}

/// The next three parameters, if all three are channels.
fn channels(params: &mut core::str::Split<'_, char>) -> Option<(u8, u8, u8)> {
    Some((
        params.next()?.parse().ok()?,
        params.next()?.parse().ok()?,
        params.next()?.parse().ok()?,
    ))
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
    // and "looks fine here" is the thing this replaces. A slot that took the
    // ground is not in this: it is checked against the one it took, below.
    for slot in all() {
        let (Sets::Exact(hue), Sets::Nothing) = sets(at(Depth::Exact).open(slot)) else {
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
fn a_slot_that_takes_the_ground_is_legible_on_the_one_it_takes() {
    // The one pair this palette gets to choose both halves of, so it is the
    // pair that is checked. Nothing else here can be: everywhere else the other
    // half is the reader's, which is why everywhere else clears both.
    for slot in DIFF {
        let (Sets::Exact(hue), Sets::Exact(ground)) = sets(at(Depth::Exact).open(slot)) else {
            panic!("{slot:?} spells out its ink and its ground, or it cannot be checked");
        };

        let ratio = contrast(hue, ground);
        assert!(
            ratio >= LEGIBLE,
            "{slot:?} is {ratio:.2}:1 on the ground it carries, under {LEGIBLE}:1"
        );
    }
}

#[test]
fn the_two_slots_without_a_hue_are_the_two_that_meant_not_to_have_one() {
    // So the check above is known to have skipped only what it should. Plain is
    // the reader's own foreground and Quiet is their theme's answer to "subdued
    // on this ground", which is the one judgement worth deferring to.
    let hueless: Vec<Slot> = all()
        .into_iter()
        .filter(|slot| !matches!(sets(at(Depth::Exact).open(*slot)).0, Sets::Exact(_)))
        .collect();

    assert_eq!(hueless, [Slot::Plain, Slot::Quiet]);
}

#[test]
fn a_slot_takes_the_ground_only_where_it_writes_the_ink_for_it() {
    // The inline design in one assertion: the ground behind a row belongs to
    // the terminal, and a background attribute is how a process takes it. A
    // diff takes it, and may, because it writes the ink for that ground in the
    // same sequence. Half of the pair is the failure, at any rung -- a ground
    // over the reader's own foreground is a contrast nobody chose.
    for slot in all() {
        for depth in [Depth::Exact, Depth::Indexed, Depth::Basic, Depth::Off] {
            let written = at(depth).open(slot);
            let (ink, ground) = sets(written);

            assert!(
                ground == Sets::Nothing || ink != Sets::Nothing,
                "{slot:?} at {depth:?} took the ground and left the ink: {written:?}"
            );
        }
    }
}

#[test]
fn nothing_but_a_diff_takes_the_ground_at_any_rung() {
    // And so the exception stays four slots wide. A rung is where it would go
    // unnoticed -- the sixteen-colour ladder is the one nobody is looking at,
    // and it is the one where a ground is a single digit away from a hue.
    for slot in all() {
        for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
            let written = at(depth).open(slot);

            assert_eq!(
                sets(written).1 != Sets::Nothing,
                DIFF.contains(&slot),
                "{slot:?} at {depth:?}: {written:?}"
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
