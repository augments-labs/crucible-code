//! What the terminal says its background is, and which way that makes it go.
//!
//! Two things will answer, and neither is asked for here: this module reads.
//! `COLORFGBG` is a variable some terminals set at launch, and a reply to the
//! query is a string a terminal wrote back. Both arrive as text, both are
//! parsed here, and the I/O that fetches the second lives elsewhere — which is
//! what makes every answer below testable with no terminal attached.
//!
//! **Nothing here guesses.** An answer in a spelling this does not know is
//! `None` rather than a default, because the caller has a correct thing to do
//! with `None`: draw the prompt row with no ground at all. A default would
//! replace a known-unknown with a wrong-known, and paint a band against a
//! ground nobody has established.

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

/// The ground an answer to the query says it is.
#[must_use]
pub fn replied(data: &str) -> Option<Ground> {
    let colour = rgb(data)?;

    Some(if is_light(colour) {
        Ground::Light
    } else {
        Ground::Dark
    })
}

/// Which way one colour goes.
///
/// The same question [`replied`] answers, asked about a colour rather than
/// about a string. One decision in one place: a second threshold elsewhere
/// would be two answers about one terminal, free to disagree about which ink
/// belongs on it.
pub(crate) fn is_light(colour: (u8, u8, u8)) -> bool {
    luminance(colour) > MIDPOINT
}

/// The exact channels an answer carries.
///
/// Separate from [`replied`] because the band is blended against the colour
/// itself rather than against which side of the midpoint it fell.
///
/// Two spellings, which are the two an `XParseColor` answer comes in.
/// `rgb:R/G/B` carries one to four hex digits per component, each scaled
/// against its own maximum rather than against 255 — `rgb:f/f/f` is white, and
/// reading it as `0f0f0f` would make it very nearly black. Some terminals
/// append an alpha; it says nothing about the ground and is ignored.
#[must_use]
pub fn rgb(data: &str) -> Option<(u8, u8, u8)> {
    if let Some(rest) = data
        .strip_prefix("rgb:")
        .or_else(|| data.strip_prefix("rgba:"))
    {
        let mut parts = rest.split('/');
        let (red, green, blue) = (parts.next()?, parts.next()?, parts.next()?);

        return Some((component(red)?, component(green)?, component(blue)?));
    }

    let hex = data.strip_prefix('#')?;
    if hex.is_empty() || !hex.len().is_multiple_of(3) {
        return None;
    }

    let each = hex.len() / 3;
    Some((
        component(hex.get(..each)?)?,
        component(hex.get(each..each * 2)?)?,
        component(hex.get(each * 2..)?)?,
    ))
}

/// One component, however many digits it was written in, as a byte.
fn component(hex: &str) -> Option<u8> {
    if hex.is_empty() || hex.len() > 4 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    let read = u32::from_str_radix(hex, 16).ok()?;
    // Its own maximum, which is what the width of the field decides.
    let most = 16u32.pow(u32::try_from(hex.len()).ok()?) - 1;

    u8::try_from(read * 255 / most).ok()
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
