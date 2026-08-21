//! What the terminal says its background is, and which way that makes it go.
//!
//! One thing answers here and it is a variable: `COLORFGBG`, which some
//! terminals set at launch and which says which way the ground goes without
//! saying what colour it is. It arrives as text and is read with no terminal
//! attached, which is what makes it testable.
//!
//! The other answer — the exact channels, asked for over the wire — is read by
//! the crate that asks for it. `XParseColor` has more spellings than are worth
//! writing twice, and a second parser here would be a second thing to be wrong
//! about somebody else's format. See [`crate::asked`].
//!
//! **Nothing here guesses.** A variable in a spelling this does not know is
//! `None` rather than a default: what the caller does about an unanswered
//! question is the caller's decision, and one taken here would be taken again
//! there, out of sight of whatever else it knows by then. What the palette does
//! do with `None` is in [`crate::color`], beside the table it does it from.

/// Which way the terminal's own background goes.
///
/// The question is not what colour it is — it is which ink belongs on it, and
/// that is the only thing anything downstream asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// Dark ink belongs on it.
    Light,
    /// Light ink belongs on it.
    Dark,
}

/// The variable some terminals set to say what their two ends are.
const COLORFGBG: &str = "COLORFGBG";

/// Where a colour stops being one to put light ink on and starts being one to
/// put dark ink on.
///
/// Relative luminance at `L* = 50`, which is the lightness a reader perceives
/// as halfway and therefore the point where black ink and white ink are equally
/// legible. Not the arithmetic middle of the scale: luminance is linear in
/// light and perception is not, so `0.5` would call a great many terminals dark
/// that everybody looking at them would call light.
const MIDPOINT: f64 = 0.1842;

/// The ground a variable in the environment already answered, before anything
/// was asked.
///
/// Free, and synchronous: it is a variable, so the first frame can be drawn on
/// this without waiting for anything. `from` reads the environment as a
/// parameter because writing to the real one is `unsafe` in edition 2024 and
/// this workspace forbids it.
#[must_use]
pub fn seeded(from: &dyn Fn(&str) -> Option<String>) -> Option<Ground> {
    // `fg;bg`, and the `fg;other;bg` some terminals write instead. The
    // background is the last field either way, which is the whole of the
    // difference between the two forms.
    let said = from(COLORFGBG)?;
    // Past the foreground first: a value with no `;` in it at all is a variable
    // holding one field, and reading that one as the background would answer
    // confidently off the wrong end of a malformed setting.
    let (_, rest) = said.split_once(';')?;
    let background: u8 = rest.rsplit(';').next()?.parse().ok()?;

    // The rxvt convention, which is the only thing this variable has ever
    // meant: the dark half of the sixteen is 0 to 6 and 8, and 7 with 9 upwards
    // are the light one.
    match background {
        0..=6 | 8 => Some(Ground::Dark),
        7 | 9..=15 => Some(Ground::Light),
        _ => None,
    }
}

/// Which way one colour goes.
///
/// One decision in one place: a second threshold elsewhere would be two answers
/// about one terminal, free to disagree about which ink belongs on it.
#[must_use]
pub fn is_light(colour: (u8, u8, u8)) -> bool {
    luminance(colour) > MIDPOINT
}

/// Relative luminance, as the contrast formula defines it.
///
/// The same formula the contrast floors in [`crate::color`] are checked with,
/// deliberately: a colour this calls light is one whose theme was verified
/// against a light ground, and two formulas would let those two answers
/// disagree about the same terminal.
pub(crate) fn luminance((red, green, blue): (u8, u8, u8)) -> f64 {
    fn channel(value: u8) -> f64 {
        let value = f64::from(value) / 255.0;

        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
}

#[cfg(test)]
mod tests;
