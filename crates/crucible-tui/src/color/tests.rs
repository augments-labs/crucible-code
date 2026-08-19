use super::*;

/// The lowest contrast ratio a colour may have against the ground its theme is
/// for.
///
/// Four and a half rather than the three this held while one palette had to
/// serve every terminal. A table tuned to one ground can afford the higher bar,
/// and a colour that has to work on both grounds is a colour at its best on
/// neither -- which is the trade a theme exists to stop making.
const LEGIBLE: f64 = 4.5;

/// The ground each theme is for, which is what its colours are checked against.
///
/// Black and white rather than a real terminal's: they are the far ends, so a
/// colour that clears one of them clears every ground on that side of the
/// midpoint.
fn ground_of(theme: Theme) -> (u8, u8, u8) {
    match theme {
        Theme::Light | Theme::ColourblindLight => (255, 255, 255),
        // Ansi spells no hue out at any rung, so nothing of it reaches a
        // contrast check; the ground named here is never used.
        Theme::Dark | Theme::ColourblindDark | Theme::Ansi => (0, 0, 0),
    }
}

/// Every theme there is, so no check can quietly cover only one.
const THEMES: [Theme; 5] = [
    Theme::Dark,
    Theme::Light,
    Theme::ColourblindDark,
    Theme::ColourblindLight,
    Theme::Ansi,
];

/// Terminal grounds a reader plausibly has, for the checks about the band.
///
/// The seven the design measured: the two ends, three dark terminals in common
/// use, and two light ones.
const TERMINALS: [(u8, u8, u8); 7] = [
    (0, 0, 0),
    (13, 13, 16),
    (30, 30, 30),
    (40, 44, 52),
    (247, 247, 244),
    (255, 255, 255),
    (253, 246, 227),
];

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
fn all() -> [Slot; 16] {
    /// Where a slot sits in the list.
    fn place(slot: Slot) -> usize {
        match slot {
            Slot::Plain => 0,
            Slot::Accent => 1,
            Slot::Strong => 2,
            Slot::Quiet => 3,
            Slot::AllowEdits => 4,
            Slot::FullAccess => 5,
            Slot::Doing => 6,
            Slot::DoingMark => 7,
            Slot::Done => 8,
            Slot::DoneMark => 9,
            Slot::Removed => 10,
            Slot::RemovedNumber => 11,
            Slot::Added => 12,
            Slot::AddedNumber => 13,
            Slot::Prompt => 14,
            Slot::PromptMark => 15,
        }
    }

    let slots = [
        Slot::Plain,
        Slot::Accent,
        Slot::Strong,
        Slot::Quiet,
        Slot::AllowEdits,
        Slot::FullAccess,
        Slot::Doing,
        Slot::DoingMark,
        Slot::Done,
        Slot::DoneMark,
        Slot::Removed,
        Slot::RemovedNumber,
        Slot::Added,
        Slot::AddedNumber,
        Slot::Prompt,
        Slot::PromptMark,
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
    wearing(depth, Theme::Dark, None)
}

/// The same, in a named theme and over a named terminal ground.
fn wearing(depth: Depth, theme: Theme, ground: Option<(u8, u8, u8)>) -> Palette {
    let band = ground.map(Palette::band);

    Palette {
        depth,
        theme,
        band: band.and_then(|band| painted(band, None, depth)),
        band_mark: band.and_then(|band| painted(band, Some(theme.tones().accent), depth)),
    }
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
fn every_colour_is_legible_on_the_ground_its_theme_is_for() {
    // The claim every theme rests on. A table is tuned to one ground, so that
    // ground is what it is measured against -- and it clears a higher bar than
    // the old one-palette-for-everything did, which is the whole trade.
    for theme in THEMES {
        for slot in all() {
            let (Sets::Exact(hue), Sets::Nothing) =
                sets(wearing(Depth::Exact, theme, None).open(slot).as_str())
            else {
                continue;
            };

            let ratio = contrast(hue, ground_of(theme));
            assert!(
                ratio >= LEGIBLE,
                "{theme:?}: {slot:?} is {ratio:.2}:1 on the ground it is for, under {LEGIBLE}:1"
            );
        }
    }
}

#[test]
fn a_slot_that_takes_the_ground_is_legible_on_the_one_it_takes() {
    // The pairs this palette gets to choose both halves of, so they are the
    // pairs that are checked -- at both rungs that spell a colour out, because
    // an indexed table is picked separately and can drift from the exact one.
    for theme in THEMES {
        for slot in DIFF {
            let written = wearing(Depth::Exact, theme, None).open(slot);
            let (Sets::Exact(hue), Sets::Exact(ground)) = sets(written.as_str()) else {
                // Ansi names its colours rather than spelling them, and what
                // they are worth is the reader's terminal theme's answer.
                assert_eq!(theme, Theme::Ansi, "{theme:?}: {slot:?} spells nothing out");
                continue;
            };

            let ratio = contrast(hue, ground);
            assert!(
                ratio >= LEGIBLE,
                "{theme:?}: {slot:?} is {ratio:.2}:1 on the ground it carries, under {LEGIBLE}:1"
            );
        }
    }
}

#[test]
fn the_two_diff_grounds_are_told_apart_by_hue_and_not_only_by_the_sign() {
    // What the colourblind tables exist for, and the property the arithmetic
    // would have deleted: nearest-colour puts both of their grounds on the same
    // grey, because each really is nearest that grey. The signs in the gutter
    // still carry the meaning, and this is the belt as well as the braces.
    for theme in THEMES.into_iter().filter(|theme| *theme != Theme::Ansi) {
        let palette = wearing(Depth::Exact, theme, None);
        let ground = |slot| match sets(palette.open(slot).as_str()) {
            (_, Sets::Exact(ground)) => ground,
            other => panic!("{theme:?}: {slot:?} carries no ground: {other:?}"),
        };

        let (removed, added) = (ground(Slot::Removed), ground(Slot::Added));
        assert_ne!(removed, added, "{theme:?}: one ground under both signs");
    }
}

#[test]
fn the_slots_without_a_hue_are_the_ones_that_meant_not_to_have_one() {
    // So the check above is known to have skipped only what it should. Plain is
    // the reader's own foreground and Quiet is their theme's answer to "subdued
    // on this ground", which is the one judgement worth deferring to. The other
    // two are that same foreground with an attribute on it -- weight, and a
    // line through it -- so what they are legible against is whatever Plain
    // was. The last two are the band, whose ground is the reader's own and
    // whose ink is deliberately theirs as well.
    let hueless: Vec<Slot> = all()
        .into_iter()
        .filter(|slot| {
            !matches!(
                sets(
                    wearing(Depth::Exact, Theme::Dark, None)
                        .open(*slot)
                        .as_str()
                )
                .0,
                Sets::Exact(_)
            )
        })
        .collect();

    assert_eq!(
        hueless,
        [
            Slot::Plain,
            Slot::Quiet,
            Slot::Doing,
            Slot::Done,
            Slot::Prompt,
            Slot::PromptMark
        ]
    );
}

#[test]
fn a_diff_takes_the_ground_only_where_it_writes_the_ink_for_it() {
    // The inline design in one assertion: the ground behind a row belongs to
    // the terminal, and a background attribute is how a process takes it. A
    // diff takes it, and may, because it writes the ink for that ground in the
    // same sequence. Half of the pair is the failure, at any rung -- a ground
    // over the reader's own foreground is a contrast nobody chose.
    //
    // The band is not in this and is checked below instead. Its ground is not
    // one this file chose, so there is no pair here to get half right.
    for theme in THEMES {
        for slot in all()
            .into_iter()
            .filter(|slot| !matches!(slot, Slot::Prompt | Slot::PromptMark))
        {
            for depth in [Depth::Exact, Depth::Indexed, Depth::Basic, Depth::Off] {
                let written = wearing(depth, theme, None).open(slot);
                let (ink, ground) = sets(written.as_str());

                assert!(
                    ground == Sets::Nothing || ink != Sets::Nothing,
                    "{theme:?}: {slot:?} at {depth:?} took the ground and left the ink: {written:?}"
                );
            }
        }
    }
}

#[test]
fn nothing_but_a_diff_and_the_band_takes_the_ground_at_any_rung() {
    // And so the exception stays exactly as wide as it is argued for. A rung is
    // where it would go unnoticed -- the sixteen-colour ladder is the one
    // nobody is looking at, and it is the one where a ground is a single digit
    // away from a hue.
    for theme in THEMES {
        for slot in all() {
            for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
                let written = wearing(depth, theme, Some((13, 13, 16))).open(slot);
                let takes = sets(written.as_str()).1 != Sets::Nothing;
                let may = DIFF.contains(&slot) || matches!(slot, Slot::Prompt | Slot::PromptMark);

                assert_eq!(takes, may, "{theme:?}: {slot:?} at {depth:?}: {written:?}");
            }
        }
    }
}

#[test]
fn the_band_is_a_step_off_the_readers_own_ground_and_never_a_stride() {
    // Why the band may take the ground without writing an ink for it: it is not
    // a colour this file chose. It is the reader's own, moved far enough to be
    // seen and no further -- so a foreground that was legible on their ground
    // is still legible on this, and there is no second half of a contrast for
    // anyone to have skipped.
    //
    // The ceiling is what makes that argument true rather than merely intended.
    for terminal in TERMINALS {
        let band = Palette::band(terminal);
        let apart = contrast(band, terminal);

        assert!(band != terminal, "{terminal:?}: the band is invisible");
        assert!(
            apart <= 1.6,
            "{terminal:?}: the band is {apart:.2}:1 off it, which is a stride"
        );
    }
}

#[test]
fn the_band_is_nothing_at_all_where_no_ground_is_known() {
    // The state a terminal that answered neither question leaves this in, and
    // it is a correct state rather than a failure: the prompt row is drawn with
    // its mark and its blank row and no ground, which is what it looked like
    // before any of this existed.
    for theme in THEMES {
        for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
            let palette = wearing(depth, theme, None);

            for slot in [Slot::Prompt, Slot::PromptMark] {
                assert_eq!(
                    palette.open(slot).as_str(),
                    "",
                    "{theme:?} at {depth:?}: {slot:?}"
                );
            }
        }
    }
}

#[test]
fn the_mark_on_the_band_carries_its_ground_and_its_accent_in_one_sequence() {
    // The one ink the band does carry. It goes in the same sequence as the
    // ground for the reason every other ground-painting slot's does: two
    // sequences are two chances to write one and not the other.
    for theme in THEMES {
        let palette = wearing(Depth::Exact, theme, Some((13, 13, 16)));
        let (ink, ground) = sets(palette.open(Slot::PromptMark).as_str());

        assert_ne!(ground, Sets::Nothing, "{theme:?}: the mark took no ground");
        assert_ne!(ink, Sets::Nothing, "{theme:?}: the mark carries no accent");
    }
}

#[test]
fn quiet_is_the_terminals_own_answer_at_every_rung() {
    for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
        assert_eq!(
            at(depth).open(Slot::Quiet).as_str(),
            "\x1b[90m",
            "{depth:?}"
        );
    }
}

#[test]
fn a_run_that_writes_no_colour_writes_no_bytes() {
    let palette = Palette::plain();

    for slot in all() {
        assert_eq!(palette.open(slot).as_str(), "", "{slot:?}");
    }
    assert_eq!(palette.close(), "");
    assert!(!palette.writes_color());
}

#[test]
fn colour_turned_off_outside_this_module_stops_at_the_top_of_it() {
    // Whether there is colour was settled by the configuration, the environment
    // and `is_terminal` together. A terminal shouting COLORTERM does not reopen
    // it.
    let palette = Palette::resolve(
        false,
        Theme::Dark,
        None,
        &environment(&[(COLORTERM, "truecolor")]),
    );

    assert_eq!(palette.open(Slot::Accent).as_str(), "");
}

#[test]
fn a_terminal_that_announces_twenty_four_bit_gets_it() {
    for announced in ["truecolor", "24bit"] {
        let palette = Palette::resolve(
            true,
            Theme::Dark,
            None,
            &environment(&[(COLORTERM, announced)]),
        );

        assert_eq!(
            palette.open(Slot::Accent).as_str(),
            "\x1b[38;2;18;137;127m",
            "{announced}"
        );
    }
}

#[test]
fn a_term_naming_two_hundred_and_fifty_six_gets_the_nearest_of_them() {
    let palette = Palette::resolve(
        true,
        Theme::Dark,
        None,
        &environment(&[(TERM, "xterm-256color")]),
    );

    assert_eq!(palette.open(Slot::Accent).as_str(), "\x1b[38;5;30m");
}

#[test]
fn a_terminal_that_says_nothing_in_particular_gets_the_sixteen_it_has() {
    let palette = Palette::resolve(true, Theme::Dark, None, &environment(&[(TERM, "xterm")]));

    assert_eq!(palette.open(Slot::Accent).as_str(), "\x1b[36m");
}

#[test]
fn a_terminal_that_says_it_is_dumb_is_believed() {
    let palette = Palette::resolve(true, Theme::Dark, None, &environment(&[(TERM, "dumb")]));

    assert_eq!(palette.open(Slot::Accent).as_str(), "");
}

#[test]
fn an_unset_term_is_not_guessed_at() {
    // Nothing said it was a terminal type at all, and sixteen colours written
    // to something that turns out not to want them is sixteen colours of
    // rubbish in somebody's log.
    let palette = Palette::resolve(true, Theme::Dark, None, &environment(&[]));

    assert_eq!(palette.open(Slot::Accent).as_str(), "");
}

#[test]
fn colourterm_outranks_term() {
    // A 256-colour TERM is what a terminal emulator inherits; COLORTERM is what
    // it sets for itself.
    let palette = Palette::resolve(
        true,
        Theme::Dark,
        None,
        &environment(&[(COLORTERM, "truecolor"), (TERM, "xterm-256color")]),
    );

    assert_eq!(palette.open(Slot::Accent).as_str(), "\x1b[38;2;18;137;127m");
}

#[test]
fn every_slot_that_has_a_colour_ends_it() {
    // An attribute left open outlives the process: the shell prompt underneath
    // inherits it, and the reader has to type `reset`.
    for depth in [Depth::Exact, Depth::Indexed, Depth::Basic] {
        assert_eq!(at(depth).close(), RESET, "{depth:?}");
    }
}
