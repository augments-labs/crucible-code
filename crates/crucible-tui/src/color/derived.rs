//! Colour a palette works out, rather than colour somebody chose.
//!
//! Every other value in [`crate::color`] is a sequence a person picked and a
//! test checked. One is not: the ground behind the reader's own prompt is a
//! step off *their* terminal's background, so nobody could have written it down
//! — it is arithmetic, done once at startup, against a colour the terminal only
//! reveals at runtime.
//!
//! **That is the whole of what earns a computed colour here.** A hue somebody
//! chose is better than a hue nobody chose: a person weighs identity, and this
//! weighs distance. So the mapping below is used where there was no choice to
//! be had, and never to second-guess one that was made.
//!
//! **Nothing here is on the render path.** These run while the palette is being
//! settled and their answers are held; a frame reads bytes that already exist.
//!
//! **The blend is integer arithmetic.** Not for speed — it runs a handful of
//! times — but because a float is turned back into a byte by a cast this
//! workspace has no safe spelling for, and a percentage is what the design
//! states anyway.

/// The colours the terminal owns, whose values this process cannot know.
///
/// The first sixteen are the reader's, defined by their terminal theme.
/// Measuring a distance to one would be measuring against a colour nobody here
/// can see, so nothing is ever mapped into them — [`nearest_indexed`] starts
/// above them.
const OWNED: u8 = 16;

/// Where the six-by-six-by-six cube ends and the grey ramp begins.
const RAMP: u8 = 232;

/// One colour composited over another, `percent` of the way.
///
/// A percentage rather than a fraction: the two the design states are 12 and 4,
/// both exact, and integers all the way through means the answer is rounded
/// rather than truncated. Truncating biases every channel toward the darker
/// end, which on a light terminal is the difference between a band and a smear.
pub(crate) fn blend(over: (u8, u8, u8), under: (u8, u8, u8), percent: u8) -> (u8, u8, u8) {
    let percent = u16::from(percent.min(100));

    let mix = |over: u8, under: u8| {
        let sum = u16::from(over) * percent + u16::from(under) * (100 - percent);
        // The +50 is the rounding: integer division truncates, and half a step
        // down on every channel is a visible shift on a large flat area.
        u8::try_from((sum + 50) / 100).unwrap_or(u8::MAX)
    };

    (
        mix(over.0, under.0),
        mix(over.1, under.1),
        mix(over.2, under.2),
    )
}

/// The nearest of the indexed colours this process can name.
///
/// The cube and the grey ramp, which are the two hundred and forty whose values
/// are fixed by the standard rather than by the reader's theme. The ramp earns
/// its place in the set: it is twenty-four steps where the cube's own grey
/// diagonal is six, and the ground behind a prompt is very nearly always a grey.
pub(crate) fn nearest_indexed(target: (u8, u8, u8)) -> u8 {
    nearest(target, OWNED..=u8::MAX, indexed)
}

/// The nearest of the sixteen, as the parameter that selects it.
///
/// **Approximate by nature.** These are the reader's colours and this only has
/// the values a terminal conventionally gives them, so the answer is "the one a
/// reader would name this by" rather than a measurement. It is the last rung of
/// the ladder for exactly that reason: a terminal with nothing better.
pub(crate) fn nearest_basic(target: (u8, u8, u8)) -> u8 {
    let at = nearest(target, 0..=15, basic);

    // 30 to 37 for the first eight, 90 to 97 for the bright ones.
    if at < 8 { 30 + at } else { 82 + at }
}

/// Whichever of `over` is the least distance from `target`.
fn nearest(
    target: (u8, u8, u8),
    over: std::ops::RangeInclusive<u8>,
    colour: impl Fn(u8) -> (u8, u8, u8),
) -> u8 {
    let mut best = *over.start();
    let mut closest = f64::MAX;

    for at in over {
        let apart = distance(target, colour(at));
        if apart < closest {
            closest = apart;
            best = at;
        }
    }

    best
}

/// How far apart two colours look, rather than how far apart their bytes are.
///
/// Euclidean distance in `CIE L*a*b*`, which is laid out so that a step of one
/// looks about the same size wherever it is taken. Distance in sRGB is not:
/// it treats a change in blue as it treats the same change in green, and the
/// eye does not — so the naive answer picks visibly wrong neighbours, and the
/// greens are where it shows first.
fn distance(one: (u8, u8, u8), other: (u8, u8, u8)) -> f64 {
    let (light, a, b) = lab(one);
    let (other_light, other_a, other_b) = lab(other);

    (other_light - light).mul_add(
        other_light - light,
        (other_a - a).mul_add(other_a - a, (other_b - b) * (other_b - b)),
    )
}

/// One colour in `CIE L*a*b*`, by way of linear RGB and `XYZ` under D65.
fn lab((red, green, blue): (u8, u8, u8)) -> (f64, f64, f64) {
    fn linear(value: u8) -> f64 {
        let value = f64::from(value) / 255.0;

        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    // The lightness a component contributes past the point where the curve
    // would head for zero faster than the eye does.
    fn curved(value: f64) -> f64 {
        if value > 0.008_856 {
            value.cbrt()
        } else {
            7.787f64.mul_add(value, 16.0 / 116.0)
        }
    }

    let (red, green, blue) = (linear(red), linear(green), linear(blue));

    // D65, the white point the sRGB standard is defined against.
    let x = curved(0.4124f64.mul_add(red, 0.3576f64.mul_add(green, 0.1805 * blue)) / 0.950_47);
    let y = curved(0.2126f64.mul_add(red, 0.7152f64.mul_add(green, 0.0722 * blue)));
    let z = curved(0.0193f64.mul_add(red, 0.1192f64.mul_add(green, 0.9505 * blue)) / 1.088_83);

    (116.0f64.mul_add(y, -16.0), 500.0 * (x - y), 200.0 * (y - z))
}

/// What an indexed parameter stands for, for every index this maps into.
fn indexed(at: u8) -> (u8, u8, u8) {
    if at < RAMP {
        let at = u32::from(at - OWNED);
        // Six rungs a channel, and the gap below the first one is wider than
        // the gaps above it. That is the standard's shape, not a choice here.
        let rung = |value: u32| match value {
            0 => 0,
            other => u8::try_from(55 + 40 * other).unwrap_or(u8::MAX),
        };

        return (rung(at / 36), rung((at % 36) / 6), rung(at % 6));
    }

    let grey = 8 + 10 * (at - RAMP);
    (grey, grey, grey)
}

/// What one of the sixteen conventionally is.
///
/// The values a terminal that was never configured gives them. See
/// [`nearest_basic`] for why an assumption is the right shape here.
fn basic(at: u8) -> (u8, u8, u8) {
    match at {
        0 => (0, 0, 0),
        1 => (205, 0, 0),
        2 => (0, 205, 0),
        3 => (205, 205, 0),
        4 => (0, 0, 238),
        5 => (205, 0, 205),
        6 => (0, 205, 205),
        7 => (229, 229, 229),
        8 => (127, 127, 127),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (92, 92, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        _ => (255, 255, 255),
    }
}

#[cfg(test)]
mod tests;
