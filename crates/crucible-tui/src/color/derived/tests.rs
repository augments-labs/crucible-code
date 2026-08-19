use super::*;

/// The colour an indexed parameter stands for, worked out the other way round,
/// so a mapping can be checked against the thing it was mapping into.
fn indexed(at: u8) -> (u8, u8, u8) {
    match at {
        16..=231 => {
            let at = u32::from(at) - 16;
            let step = |value: u32| match value {
                0 => 0,
                other => u8::try_from(55 + 40 * other).expect("a cube rung to be a byte"),
            };
            (step(at / 36), step((at % 36) / 6), step(at % 6))
        }
        232..=255 => {
            let grey = 8 + 10 * (at - 232);
            (grey, grey, grey)
        }
        // The first sixteen belong to the reader's terminal, so nothing here
        // knows what they are and nothing maps into them. Black stands in, and
        // the test below is what proves nothing ever asks.
        _ => (0, 0, 0),
    }
}

#[test]
fn nothing_is_blended_at_the_two_ends() {
    // Zero is all of what is underneath and one is all of what is over it. A
    // blend that moved at either end would tint a ground nobody asked to tint.
    let over = (255, 255, 255);
    let under = (13, 13, 16);

    assert_eq!(blend(over, under, 0), under);
    assert_eq!(blend(over, under, 100), over);
}

#[test]
fn a_blend_moves_the_same_way_the_alpha_does() {
    // Monotonic per channel: more alpha is never less of the colour on top.
    let over = (255, 255, 255);
    let under = (13, 13, 16);
    let mut last = blend(over, under, 0);

    for step in 1..=100u8 {
        let next = blend(over, under, step);

        assert!(
            next.0 >= last.0 && next.1 >= last.1 && next.2 >= last.2,
            "{last:?} -> {next:?}"
        );
        last = next;
    }

    assert_eq!(last, over);
}

#[test]
fn a_blend_stays_between_the_two_colours_it_is_between() {
    let over = (255, 255, 255);
    let under = (13, 13, 16);

    for step in 0..=100u8 {
        let (red, green, blue) = blend(over, under, step);

        assert!((under.0..=over.0).contains(&red), "red {red}");
        assert!((under.1..=over.1).contains(&green), "green {green}");
        assert!((under.2..=over.2).contains(&blue), "blue {blue}");
    }
}

#[test]
fn every_indexed_colour_is_its_own_nearest() {
    // The property that says the mapping is a mapping rather than an
    // approximation with a bias: a colour the terminal can already show is the
    // one it is given back.
    for at in 16..=255u8 {
        assert_eq!(nearest_indexed(indexed(at)), at, "index {at}");
    }
}

#[test]
fn nothing_is_mapped_into_the_sixteen_the_terminal_owns() {
    // Those are the reader's, defined by their theme, and this process has no
    // idea what they are. Mapping into them would be measuring a distance to a
    // colour nobody here can see.
    for red in [0u8, 64, 128, 192, 255] {
        for green in [0u8, 64, 128, 192, 255] {
            for blue in [0u8, 64, 128, 192, 255] {
                assert!(
                    nearest_indexed((red, green, blue)) >= 16,
                    "{red},{green},{blue}"
                );
            }
        }
    }
}

#[test]
fn a_colour_is_mapped_nearer_its_own_hue_than_another() {
    // The reason this is perceptual rather than a straight distance in sRGB:
    // the naive one picks visibly wrong neighbours, and the greens are where it
    // shows first.
    assert_eq!(nearest_indexed((255, 0, 0)), 196);
    assert_eq!(nearest_indexed((0, 255, 0)), 46);
    assert_eq!(nearest_indexed((0, 0, 255)), 21);
    assert_eq!(nearest_indexed((255, 255, 255)), 231);
    assert_eq!(nearest_indexed((0, 0, 0)), 16);
}

#[test]
fn a_grey_is_mapped_onto_the_ramp_rather_than_into_the_cube() {
    // The ramp is twenty-four steps and the cube's grey diagonal is six, so a
    // near-grey has a much nearer neighbour on the ramp -- which is the whole
    // reason the ramp is in the candidate set.
    for grey in [18u8, 48, 88, 128, 168, 208, 238] {
        let at = nearest_indexed((grey, grey, grey));

        assert!((232..=255).contains(&at), "grey {grey} went to index {at}");
    }
}

#[test]
fn the_sixteen_are_answered_by_their_own_parameter() {
    // Approximate by nature, and the assertion is only that each primary lands
    // on the parameter a reader would name it by.
    assert_eq!(nearest_basic((0, 0, 0)), 30);
    assert_eq!(nearest_basic((255, 0, 0)), 91);
    assert_eq!(nearest_basic((0, 255, 0)), 92);
    assert_eq!(nearest_basic((255, 255, 255)), 97);
}
