//! Generated input: valid pictures that must come back with their size, and
//! damaged, forged and nested ones that must come back refused or drawn, never
//! as a panic, a hang or a picture past the ceilings.
//!
//! Every case is made from a seed and its index alone, so a failure names the
//! case that reproduces it. A run makes [`DEFAULT_CASES`] cases; the variable
//! named by [`CASES_VARIABLE`] makes more of them for a longer run.
//!
//! This is a test binary of its own so that its pictures take turns for the
//! decode slot only with each other.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a fixture that cannot be made, or a check that fails, is the test's failure; the clippy setting that allows both reaches only the functions marked as tests, not the helpers beside them"
)]

use crucible_raster::{
    MOST_DECODED, MOST_PNG, MOST_SIXEL_WIDTH, Pixels, Refused, SixelPalette, decode, png,
    rasterise, sixel,
};
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const SEED: u64 = 0x5eed_0fc0_ffee;
const DEFAULT_CASES: u64 = 1024;
const CASES_VARIABLE: &str = "CRUCIBLE_RASTER_CASES";

/// Long enough for the slowest case in a debug build on a loaded machine, and
/// short against a hang.
const DEADLINE: Duration = Duration::from_mins(1);

const INKS: &[(&str, [u8; 3])] = &[("ink", [20, 40, 60]), ("quiet", [120, 120, 120])];
const PALETTE: SixelPalette = SixelPalette {
    ground: [10, 10, 10],
    colours: 64,
};

/// The decode slot is the process's, and a wait for it is bounded: tests that
/// draw take turns rather than make each other wait out the bound.
fn alone() -> MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

fn cases() -> u64 {
    std::env::var(CASES_VARIABLE)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(DEFAULT_CASES)
}

// ---------------------------------------------------------------------------
// The runner: a panic and a hang are findings, not the test's own end.

#[derive(Debug, PartialEq, Eq)]
enum Ended<T> {
    Returned(T),
    Panicked,
    Hung,
}

/// `work` on a thread of its own, given `deadline` to answer. A thread that
/// outlives its deadline is left behind: nothing can stop it.
fn guarded<T: Send + 'static>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Ended<T> {
    let (answer, heard) = mpsc::channel();
    thread::spawn(move || {
        // A send that fails means the runner has already given up.
        let _ = answer.send(work());
    });
    match heard.recv_timeout(deadline) {
        Ok(value) => Ended::Returned(value),
        Err(RecvTimeoutError::Timeout) => Ended::Hung,
        Err(RecvTimeoutError::Disconnected) => Ended::Panicked,
    }
}

#[test]
fn a_planted_panic_is_found_by_the_generated_runner() {
    let ended = guarded(Duration::from_secs(5), || -> u8 {
        panic!("planted: a decoder that panics")
    });
    assert_eq!(ended, Ended::Panicked);
}

#[test]
fn a_planted_unbounded_loop_is_found_by_the_generated_runner() {
    let begun = Instant::now();
    let ended = guarded(Duration::from_millis(300), move || {
        // Unbounded as far as the runner can tell. It ends itself after a few
        // seconds, so a runner that fails to cut it off cannot hang the suite.
        while begun.elapsed() < Duration::from_secs(4) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    assert_eq!(ended, Ended::Hung);
}

// ---------------------------------------------------------------------------
// The generator.

/// xorshift64*: small, and the same on every platform.
struct Rng(u64);

impl Rng {
    /// The generator for case `index`: no case depends on another.
    fn for_case(index: u64) -> Self {
        let mut rng = Rng(SEED ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        // The first outputs of nearby seeds are alike; spend them.
        for _ in 0..4 {
            rng.next();
        }
        rng
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number from `low` to `high`, both included.
    fn between(&mut self, low: u32, high: u32) -> u32 {
        let span = u64::from(high.saturating_sub(low)) + 1;
        let drawn = self.next() >> 11;
        low.saturating_add(u32::try_from(drawn % span).unwrap_or(0))
    }

    fn below(&mut self, bound: usize) -> usize {
        let bound = u32::try_from(bound).unwrap_or(u32::MAX).max(1);
        usize::try_from(self.between(0, bound - 1)).unwrap_or(0)
    }

    fn byte(&mut self) -> u8 {
        u8::try_from(self.next() >> 56).unwrap_or(0)
    }

    fn bytes(&mut self, count: usize) -> Vec<u8> {
        (0..count).map(|_| self.byte()).collect()
    }

    fn chance(&mut self, one_in: u32) -> bool {
        self.between(1, one_in.max(1)) == 1
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> Option<&'a T> {
        from.get(self.below(from.len()))
    }
}

/// What a case is, and what it must come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// An encoder's own output: decoded to the size it was made with.
    ValidImage,
    /// An encoder's output with bytes changed, dropped or repeated.
    DamagedImage,
    /// A format's magic number followed by noise.
    NoiseAfterMagic,
    /// A header whose size has been rewritten past the ceiling.
    ForgedSize,
    /// Elements inside elements, as deep as the parser lets them go.
    DeepSvg,
    /// References from one element to another, chained or multiplied.
    LinkedSvg,
    /// Sizes and numbers at their extremes.
    ExtremeSvg,
    /// A great many elements.
    CrowdedSvg,
    /// Text in any script, at any size.
    TextSvg,
    /// A valid document with bytes changed, or none of it.
    DamagedSvg,
}

const FAMILIES: [Family; 10] = [
    Family::ValidImage,
    Family::DamagedImage,
    Family::NoiseAfterMagic,
    Family::ForgedSize,
    Family::DeepSvg,
    Family::LinkedSvg,
    Family::ExtremeSvg,
    Family::CrowdedSvg,
    Family::TextSvg,
    Family::DamagedSvg,
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    /// Bytes for `decode`, and the size they must come back with, if any.
    Image {
        bytes: Vec<u8>,
        size: Option<(u32, u32)>,
        refused: Option<Refused>,
    },
    /// Bytes for `rasterise`, and a width.
    Svg { bytes: Vec<u8>, width_px: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
    Gif,
    WebP,
}

const FORMATS: [Format; 4] = [Format::Png, Format::Jpeg, Format::Gif, Format::WebP];

impl Format {
    fn magic(self) -> &'static [u8] {
        match self {
            Format::Png => b"\x89PNG\r\n\x1a\n",
            Format::Jpeg => b"\xff\xd8\xff\xe0",
            Format::Gif => b"GIF89a",
            Format::WebP => b"RIFF\0\0\0\0WEBPVP8L",
        }
    }
}

/// A picture of noise, `width` by `height`, in `format`.
fn encoded(rng: &mut Rng, format: Format, width: u32, height: u32) -> Vec<u8> {
    let pixel_bytes = usize::try_from(width * height * 4).unwrap_or(0);
    let flat = rng.chance(2);
    let rgba = if flat {
        let colour = rng.bytes(4);
        colour.iter().copied().cycle().take(pixel_bytes).collect()
    } else {
        rng.bytes(pixel_bytes)
    };
    let image = DynamicImage::ImageRgba8(
        RgbaImage::from_raw(width, height, rgba).expect("a buffer of the size it was made for"),
    );
    let (image, kind) = match format {
        Format::Png => (image, ImageFormat::Png),
        // JPEG has no alpha.
        Format::Jpeg => (DynamicImage::ImageRgb8(image.to_rgb8()), ImageFormat::Jpeg),
        Format::Gif => (image, ImageFormat::Gif),
        Format::WebP => (image, ImageFormat::WebP),
    };
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, kind)
        .expect("an encoder to write to memory");
    out.into_inner()
}

fn valid_image(rng: &mut Rng) -> Input {
    let format = *rng.pick(&FORMATS).unwrap_or(&Format::Png);
    let (width, height) = (rng.between(1, 40), rng.between(1, 40));
    let bytes = encoded(rng, format, width, height);
    Input::Image {
        bytes,
        size: Some((width, height)),
        refused: None,
    }
}

fn damaged_image(rng: &mut Rng) -> Input {
    let format = *rng.pick(&FORMATS).unwrap_or(&Format::Png);
    let (width, height) = (rng.between(1, 40), rng.between(1, 40));
    let mut bytes = encoded(rng, format, width, height);
    for _ in 0..rng.between(1, 4) {
        let at = rng.below(bytes.len());
        match rng.between(0, 5) {
            0 | 1 => {
                if let Some(byte) = bytes.get_mut(at) {
                    *byte = rng.byte();
                }
            }
            2 => bytes.truncate(at),
            3 => drop(bytes.drain(at..bytes.len().min(at + 1 + rng.below(32)))),
            4 => {
                let piece: Vec<u8> = bytes
                    .iter()
                    .skip(at)
                    .take(1 + rng.below(64))
                    .copied()
                    .collect();
                bytes.splice(at..at, piece);
            }
            _ => {
                if let Some(byte) = bytes.get_mut(at) {
                    *byte ^= 1 << rng.between(0, 7);
                }
            }
        }
    }
    Input::Image {
        bytes,
        size: None,
        refused: None,
    }
}

fn noise_after_magic(rng: &mut Rng) -> Input {
    let format = *rng.pick(&FORMATS).unwrap_or(&Format::Png);
    let mut bytes = format.magic().to_vec();
    let count = rng.below(4096);
    bytes.extend(rng.bytes(count));
    Input::Image {
        bytes,
        size: None,
        refused: None,
    }
}

/// The checksum of a PNG chunk, which a rewritten header must carry to be believed.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A size whose pixels are past `MOST_DECODED` and whose sides a header can
/// hold: 4 bytes a pixel, so a million pixels is past the ceiling.
fn forged_sides(rng: &mut Rng, most_side: u32) -> (u32, u32) {
    let fewest = u32::try_from(MOST_DECODED / 4 / usize::try_from(most_side).unwrap_or(1) + 1)
        .unwrap_or(most_side);
    // The shorter side may be anything that still leaves the area past the
    // ceiling against the longer one.
    let height = rng.between(fewest.min(most_side), most_side);
    let width_needed = u32::try_from(MOST_DECODED / 4 / usize::try_from(height).unwrap_or(1) + 1)
        .unwrap_or(most_side);
    (rng.between(width_needed.min(most_side), most_side), height)
}

fn forged_size(rng: &mut Rng) -> Input {
    let format = *rng.pick(&FORMATS).unwrap_or(&Format::Png);
    let mut bytes = encoded(rng, format, 8, 8);
    let forged = match format {
        Format::Png => {
            // IHDR: length, "IHDR", width, height, ..., crc.
            let (width, height) = forged_sides(rng, i32::MAX.unsigned_abs());
            bytes.splice(16..20, width.to_be_bytes());
            bytes.splice(20..24, height.to_be_bytes());
            let crc = crc32(bytes.get(12..29).unwrap_or(&[]));
            bytes.splice(29..33, crc.to_be_bytes());
            true
        }
        Format::Gif => {
            // Logical screen descriptor: two 16-bit sides, little endian.
            let (width, height) = forged_sides(rng, u32::from(u16::MAX));
            let (width, height) = (
                u16::try_from(width).unwrap_or(u16::MAX),
                u16::try_from(height).unwrap_or(u16::MAX),
            );
            bytes.splice(6..8, width.to_le_bytes());
            bytes.splice(8..10, height.to_le_bytes());
            // The first image descriptor carries the size the frame is drawn at.
            if let Some(at) = bytes.iter().position(|byte| *byte == 0x2c) {
                bytes.splice(at + 5..at + 7, width.to_le_bytes());
                bytes.splice(at + 7..at + 9, height.to_le_bytes());
            }
            true
        }
        Format::Jpeg => {
            // A baseline start of frame: precision, height, width, big endian.
            let (width, height) = forged_sides(rng, u32::from(u16::MAX));
            let (width, height) = (
                u16::try_from(width).unwrap_or(u16::MAX),
                u16::try_from(height).unwrap_or(u16::MAX),
            );
            let found = bytes
                .windows(2)
                .position(|pair| pair == [0xff, 0xc0] || pair == [0xff, 0xc2]);
            found.is_some_and(|at| {
                bytes.splice(at + 5..at + 7, height.to_be_bytes());
                bytes.splice(at + 7..at + 9, width.to_be_bytes());
                true
            })
        }
        // A lossless header holds 14 bits a side: 16 384 by 16 384 at most.
        Format::WebP => {
            let (width, height) = forged_sides(rng, 1 << 14);
            let found = bytes.windows(5).position(|run| run.first() == Some(&0x2f));
            found.is_some_and(|at| {
                // The alpha hint and version bits above the sides stay as written.
                let kept = bytes
                    .get(at + 1..at + 5)
                    .and_then(|run| run.first_chunk::<4>())
                    .map_or(0, |run| u32::from_le_bytes(*run) & 0xf000_0000);
                let packed = kept | (width - 1) | ((height - 1) << 14);
                bytes.splice(at + 1..at + 5, packed.to_le_bytes());
                true
            })
        }
    };
    Input::Image {
        bytes,
        size: None,
        // A header that could not be rewritten is only a damaged picture.
        refused: forged.then_some(Refused::TooLarge),
    }
}

/// `text` added to the end of `out`.
fn put(out: &mut String, text: std::fmt::Arguments<'_>) {
    // Writing to a string does not fail.
    let _ = out.write_fmt(text);
}

fn svg_around(inner: &str) -> Vec<u8> {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"100\" height=\"60\" viewBox=\"0 0 100 60\">{inner}</svg>"
    )
    .into_bytes()
}

fn width_px(rng: &mut Rng) -> u32 {
    match rng.between(0, 9) {
        0 => 0,
        1 => 1,
        2 => u32::MAX,
        3 => rng.between(10_000, 1_000_000),
        _ => rng.between(1, 600),
    }
}

fn deep_svg(rng: &mut Rng) -> Input {
    const WRAPPERS: [&str; 4] = ["g", "svg", "a", "switch"];
    let depth = rng.between(1, 1100);
    let mut open = String::new();
    let mut close = Vec::new();
    for _ in 0..depth {
        let tag = rng.pick(&WRAPPERS).copied().unwrap_or("g");
        if tag == "svg" {
            open.push_str("<svg>");
        } else {
            put(&mut open, format_args!("<{tag}>"));
        }
        close.push(tag);
    }
    open.push_str("<rect width=\"50\" height=\"30\" fill=\"currentColor\"/>");
    for tag in close.iter().rev() {
        put(&mut open, format_args!("</{tag}>"));
    }
    Input::Svg {
        bytes: svg_around(&open),
        width_px: width_px(rng).min(800),
    }
}

fn linked_svg(rng: &mut Rng) -> Input {
    let mut inner = String::new();
    match rng.between(0, 5) {
        // A chain of groups, each drawing the one before.
        0 => {
            let links = rng.between(1, 300);
            inner.push_str("<defs><rect id=\"u0\" width=\"20\" height=\"20\"/>");
            for link in 1..=links {
                put(
                    &mut inner,
                    format_args!(
                        "<g id=\"u{link}\"><use href=\"#u{}\" x=\"1\"/></g>",
                        link - 1
                    ),
                );
            }
            put(&mut inner, format_args!("</defs><use href=\"#u{links}\"/>"));
        }
        // Each level draws the one before ten times: ten to the power of the
        // levels, if anything expanded them.
        1 => {
            let levels = rng.between(1, 12);
            inner.push_str("<defs><rect id=\"l0\" width=\"2\" height=\"2\"/>");
            for level in 1..=levels {
                put(&mut inner, format_args!("<g id=\"l{level}\">"));
                for _ in 0..10 {
                    put(&mut inner, format_args!("<use href=\"#l{}\"/>", level - 1));
                }
                inner.push_str("</g>");
            }
            put(
                &mut inner,
                format_args!("</defs><use href=\"#l{levels}\"/>"),
            );
        }
        // Masks, clips and patterns that name each other, in a line or a ring.
        2 => {
            let links = rng.between(1, 60);
            let closed = rng.chance(2);
            inner.push_str("<defs>");
            for link in 0..links {
                let next = if link + 1 < links {
                    Some(link + 1)
                } else if closed {
                    Some(0)
                } else {
                    None
                };
                let reference =
                    next.map_or(String::new(), |next| format!(" mask=\"url(#m{next})\""));
                put(
                    &mut inner,
                    format_args!(
                        "<mask id=\"m{link}\"><rect width=\"100\" height=\"60\" fill=\"#fff\"{reference}/></mask>"
                    ),
                );
            }
            inner.push_str("</defs><rect width=\"100\" height=\"60\" mask=\"url(#m0)\"/>");
        }
        3 => {
            let links = rng.between(1, 60);
            let closed = rng.chance(2);
            inner.push_str("<defs>");
            for link in 0..links {
                let next = if link + 1 < links {
                    link + 1
                } else if closed {
                    0
                } else {
                    link
                };
                put(
                    &mut inner,
                    format_args!(
                        "<clipPath id=\"c{link}\" clip-path=\"url(#c{next})\"><circle cx=\"50\" cy=\"30\" r=\"{}\"/></clipPath>\
                     <pattern id=\"p{link}\" width=\"10\" height=\"10\" patternUnits=\"userSpaceOnUse\">\
                     <rect width=\"5\" height=\"5\" fill=\"url(#p{next})\"/></pattern>",
                        10 + link
                    ),
                );
            }
            inner.push_str("</defs><rect width=\"100\" height=\"60\" clip-path=\"url(#c0)\" fill=\"url(#p0)\"/>");
        }
        // A filter chain that asks for more than the canvas. It is blurs,
        // offsets and floods only: a turbulence or a morphology over a region
        // this large is refused for the pixmaps it would hold, which the
        // hostile corpus pins with its `filter-work` files, and a costly
        // primitive over a smaller region is refused for its work, which the
        // corpus pins with its `work` files. So the chain here is one the
        // drawing is let to finish or is refused for its size.
        4 => {
            let steps = rng.between(1, 20);
            inner.push_str("<defs><filter id=\"f\" x=\"-1000%\" y=\"-1000%\" width=\"3000%\" height=\"3000%\">");
            for step in 0..steps {
                let primitive = match rng.between(0, 2) {
                    0 => format!(
                        "<feGaussianBlur stdDeviation=\"{}\"/>",
                        rng.between(0, 1_000_000)
                    ),
                    1 => format!(
                        "<feFlood flood-color=\"#{:06x}\"/>",
                        rng.between(0, 0xff_ffff)
                    ),
                    _ => format!(
                        "<feOffset dx=\"{}\" dy=\"-{}\"/>",
                        rng.between(0, 99_999),
                        step
                    ),
                };
                inner.push_str(&primitive);
            }
            inner
                .push_str("</filter></defs><rect width=\"100\" height=\"60\" filter=\"url(#f)\"/>");
        }
        // Markers and symbols that draw themselves.
        _ => {
            inner.push_str(
                "<defs><marker id=\"k\" markerWidth=\"10\" markerHeight=\"10\">\
                 <path d=\"M0 0 L10 5 L0 10\" marker-start=\"url(#k)\" marker-end=\"url(#k)\"/></marker>\
                 <symbol id=\"s\"><use href=\"#s\"/></symbol></defs>\
                 <path d=\"M10 10 L90 50\" stroke=\"#000\" marker-start=\"url(#k)\" marker-end=\"url(#k)\"/>\
                 <use href=\"#s\"/>",
            );
        }
    }
    Input::Svg {
        bytes: svg_around(&inner),
        width_px: width_px(rng).min(800),
    }
}

fn number(rng: &mut Rng) -> String {
    const EXTREMES: [&str; 14] = [
        "0",
        "-0",
        "1e-300",
        "1e300",
        "1e400",
        "-1e400",
        "NaN",
        "inf",
        "-5",
        "99999999999",
        "0.0000001",
        "100%",
        "5em",
        "1e",
    ];
    match rng.between(0, 3) {
        0 => rng.pick(&EXTREMES).copied().unwrap_or("0").to_owned(),
        1 => format!("{}", rng.between(0, u32::MAX)),
        _ => format!("{}.{}", rng.between(0, 5000), rng.between(0, 999)),
    }
}

fn extreme_svg(rng: &mut Rng) -> Input {
    let (width, height, view) = (number(rng), number(rng), number(rng));
    let dash = number(rng);
    let stroke = number(rng);
    let scale = number(rng);
    let bytes = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {view} {view}\"><g transform=\"scale({scale})\">\
         <path d=\"M0 0 L{view} {view}\" stroke=\"#000\" stroke-width=\"{stroke}\" \
         stroke-dasharray=\"{dash} {dash}\" fill=\"none\"/>\
         <circle cx=\"{view}\" cy=\"{view}\" r=\"{width}\"/></g></svg>"
    )
    .into_bytes();
    Input::Svg {
        bytes,
        width_px: width_px(rng),
    }
}

fn crowded_svg(rng: &mut Rng) -> Input {
    let count = rng.between(1, 3000);
    let mut inner = String::new();
    for _ in 0..count {
        let (x, y) = (rng.between(0, 100), rng.between(0, 60));
        match rng.between(0, 3) {
            0 => put(
                &mut inner,
                format_args!(
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{}\" height=\"{}\" fill=\"#{:06x}\" opacity=\"0.5\"/>",
                    rng.between(1, 30),
                    rng.between(1, 30),
                    rng.between(0, 0x00ff_ffff)
                ),
            ),
            1 => put(
                &mut inner,
                format_args!(
                    "<circle cx=\"{x}\" cy=\"{y}\" r=\"{}\" fill=\"currentColor\"/>",
                    rng.between(1, 30)
                ),
            ),
            2 => put(
                &mut inner,
                format_args!(
                    "<path d=\"M{x} {y} C{} {} {} {} {} {}\" stroke=\"ink\" fill=\"none\" \
                 stroke-width=\"{}\" transform=\"rotate({})\"/>",
                    rng.between(0, 100),
                    rng.between(0, 60),
                    rng.between(0, 100),
                    rng.between(0, 60),
                    rng.between(0, 100),
                    rng.between(0, 60),
                    rng.between(1, 9),
                    rng.between(0, 360)
                ),
            ),
            _ => put(
                &mut inner,
                format_args!(
                    "<line x1=\"{x}\" y1=\"{y}\" x2=\"{}\" y2=\"{}\" stroke=\"#444\" class=\"ink\"/>",
                    rng.between(0, 100),
                    rng.between(0, 60)
                ),
            ),
        }
    }
    Input::Svg {
        bytes: svg_around(&inner),
        width_px: rng.between(1, 600),
    }
}

fn text_svg(rng: &mut Rng) -> Input {
    // Scripts the one embedded face has no glyph for, marks that combine, and
    // right-to-left runs.
    const SCRIPTS: [&str; 8] = [
        "abc xyz",
        "\u{05d0}\u{05d1}\u{05d2}",
        "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}",
        "\u{4e2d}\u{6587}\u{65e5}\u{672c}",
        "e\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}",
        "\u{1f600}\u{1f4a9}",
        "\u{202e}abc\u{202c}",
        "\u{0000}\u{fffe}\u{ffff}",
    ];
    let mut inner = String::new();
    for _ in 0..rng.between(1, 8) {
        let run = rng.pick(&SCRIPTS).copied().unwrap_or("abc");
        let repeated = run.repeat(rng.below(400) + 1);
        let escaped = repeated
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('\u{0}', "");
        put(
            &mut inner,
            format_args!(
                "<text x=\"{}\" y=\"{}\" font-size=\"{}\" font-family=\"{}\" textLength=\"{}\" \
             letter-spacing=\"{}\" fill=\"currentColor\">{escaped}<tspan dy=\"{}\">{escaped}</tspan></text>",
                rng.between(0, 100),
                rng.between(0, 60),
                number(rng),
                rng.pick(&["DejaVu Sans", "serif", "Comic Sans", "sans-serif"])
                    .copied()
                    .unwrap_or(""),
                number(rng),
                number(rng),
                number(rng),
            ),
        );
    }
    Input::Svg {
        bytes: svg_around(&inner),
        width_px: rng.between(1, 600),
    }
}

fn damaged_svg(rng: &mut Rng) -> Input {
    let Input::Svg {
        mut bytes,
        width_px,
    } = crowded_svg(rng)
    else {
        return Input::Svg {
            bytes: Vec::new(),
            width_px: 1,
        };
    };
    bytes.truncate(bytes.len().min(1 + rng.below(2000)));
    for _ in 0..rng.between(0, 6) {
        let at = rng.below(bytes.len().max(1));
        match rng.between(0, 4) {
            0 => {
                if let Some(byte) = bytes.get_mut(at) {
                    *byte = rng.byte();
                }
            }
            1 => {
                bytes.splice(at..at, b"\xff\xfe\x80\xc0\xaf".iter().copied());
            }
            2 => {
                let doctype = b"<!DOCTYPE s [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\">\
                    <!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\"><!ENTITY d \"&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;\">]>";
                bytes.splice(0..0, doctype.iter().copied());
                bytes.splice(
                    at.min(bytes.len())..at.min(bytes.len()),
                    b"<text>&d;&d;&d;&d;&d;&d;</text>".iter().copied(),
                );
            }
            3 => {
                bytes.splice(at..at, b"<![CDATA[".iter().copied());
            }
            _ => bytes.truncate(at),
        }
    }
    Input::Svg { bytes, width_px }
}

fn make(family: Family, rng: &mut Rng) -> Input {
    match family {
        Family::ValidImage => valid_image(rng),
        Family::DamagedImage => damaged_image(rng),
        Family::NoiseAfterMagic => noise_after_magic(rng),
        Family::ForgedSize => forged_size(rng),
        Family::DeepSvg => deep_svg(rng),
        Family::LinkedSvg => linked_svg(rng),
        Family::ExtremeSvg => extreme_svg(rng),
        Family::CrowdedSvg => crowded_svg(rng),
        Family::TextSvg => text_svg(rng),
        Family::DamagedSvg => damaged_svg(rng),
    }
}

fn case(index: u64) -> (Family, Input) {
    let family = FAMILIES
        .get(usize::try_from(index % 10).unwrap_or(0))
        .copied()
        .unwrap_or(Family::ValidImage);
    (family, make(family, &mut Rng::for_case(index)))
}

// ---------------------------------------------------------------------------
// What every picture that comes back must be.

/// The checks any picture that was drawn or decoded passes.
fn check_drawn(pixels: &Pixels) -> Result<(), String> {
    let (width, height) = (pixels.width(), pixels.height());
    if width == 0 || height == 0 {
        return Err(format!("a picture {width} by {height}"));
    }
    let expected = usize::try_from(u64::from(width) * u64::from(height) * 4).ok();
    if expected != Some(pixels.rgba().len()) {
        return Err(format!(
            "{width} by {height} holds {} bytes",
            pixels.rgba().len()
        ));
    }
    if pixels.rgba().len() > MOST_DECODED {
        return Err(format!("{} bytes of pixels", pixels.rgba().len()));
    }
    match png(pixels) {
        Ok(bytes) if bytes.len() > MOST_PNG => {
            return Err(format!("a PNG of {} bytes", bytes.len()));
        }
        Ok(bytes) if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
            return Err("a PNG without its signature".to_owned());
        }
        Ok(bytes) => {
            // What the crate wrote, it reads back at the size it was.
            let back = decode(&bytes).map_err(|refused| format!("its PNG, decoded: {refused}"))?;
            if (back.width(), back.height()) != (width, height) {
                return Err("its PNG came back at another size".to_owned());
            }
        }
        Err(Refused::TooLarge) => {}
        Err(other) => return Err(format!("its PNG: {other}")),
    }
    match sixel(pixels, &PALETTE) {
        Ok(_) if width > MOST_SIXEL_WIDTH => Err("a Sixel payload past the width".to_owned()),
        Ok(bytes) if !(bytes.starts_with(b"\x1bP") && bytes.ends_with(b"\x1b\\")) => {
            Err("a Sixel payload without its introducer and terminator".to_owned())
        }
        Ok(_) => Ok(()),
        Err(Refused::TooLarge) if width > MOST_SIXEL_WIDTH => Ok(()),
        Err(other) => Err(format!("its Sixel: {other}")),
    }
}

/// What one case came to, or why it is a failure.
fn judge(input: Input) -> Result<(), String> {
    match input {
        Input::Image {
            bytes,
            size,
            refused,
        } => match decode(&bytes) {
            Ok(pixels) => {
                if let Some(wanted) = size
                    && (pixels.width(), pixels.height()) != wanted
                {
                    return Err(format!(
                        "decoded at {} by {}, made at {} by {}",
                        pixels.width(),
                        pixels.height(),
                        wanted.0,
                        wanted.1
                    ));
                }
                if let Some(wanted) = refused {
                    return Err(format!("drawn, where {wanted} was due"));
                }
                check_drawn(&pixels)
            }
            Err(Refused::OverTime) => Err("the decode slot was not free".to_owned()),
            Err(got) => {
                if size.is_some() {
                    Err(format!("a valid picture was refused: {got}"))
                } else if refused.is_some_and(|wanted| wanted != got) {
                    Err(format!("refused as {got}, where a forged size is TooLarge"))
                } else {
                    Ok(())
                }
            }
        },
        Input::Svg { bytes, width_px } => match rasterise(&bytes, width_px, INKS) {
            Ok(pixels) => check_drawn(&pixels),
            Err(Refused::OverTime) => Err("the decode slot was not free".to_owned()),
            Err(_) => Ok(()),
        },
    }
}

/// The cases `from` to `to`, each with a deadline, the first failure first.
fn run(from: u64, to: u64) -> Result<(), String> {
    for index in from..to {
        let (family, input) = case(index);
        let began = Instant::now();
        match guarded(DEADLINE, move || judge(input)) {
            Ended::Returned(Ok(())) => {}
            Ended::Returned(Err(why)) => {
                return Err(format!("case {index} ({family:?}, seed {SEED:#x}): {why}"));
            }
            Ended::Panicked => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): panicked"
                ));
            }
            Ended::Hung => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): no answer in {:?}",
                    began.elapsed()
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn the_same_case_is_made_from_the_same_seed_and_index() {
    for index in [0, 1, 7, 100, 4095] {
        assert_eq!(case(index), case(index), "case {index}");
    }
    assert_ne!(case(10).1, case(20).1);
}

#[test]
fn every_family_of_case_is_made_within_a_run() {
    let made: Vec<Family> = (0..DEFAULT_CASES).map(|index| case(index).0).collect();
    for family in FAMILIES {
        assert!(made.contains(&family), "no {family:?} in a run");
    }
}

#[test]
fn the_forged_headers_are_forged_to_something_the_decoder_reads_as_a_size() {
    // A forgery the decoder cannot read as a size proves nothing about the
    // ceiling: it would be refused as damage, and the check would never know.
    let _alone = alone();
    let mut refused_as_size = [0_u32; 4];
    for index in (0..400).map(|n| n * 10 + 3) {
        let (family, input) = case(index);
        assert_eq!(family, Family::ForgedSize);
        let Input::Image { bytes, refused, .. } = input else {
            panic!("a forged size is a picture");
        };
        if refused.is_some() {
            let format = FORMATS
                .iter()
                .position(|format| bytes.starts_with(format.magic().get(..4).unwrap_or(&[])))
                .unwrap_or(0);
            if decode(&bytes) == Err(Refused::TooLarge)
                && let Some(count) = refused_as_size.get_mut(format)
            {
                *count += 1;
            }
        }
    }
    assert!(
        refused_as_size.iter().all(|count| *count > 0),
        "forged and refused as too large, by format: {refused_as_size:?}"
    );
}

#[test]
fn generated_pictures_are_decoded_drawn_or_refused_within_the_ceilings() {
    let _alone = alone();
    let total = cases();
    if let Err(why) = run(0, total) {
        panic!("{why}");
    }
}
