//! A diagram's picture, drawn to pixels and encoded for the terminal, and an
//! image a turn named, encoded for it from the PNG the application read.
//!
//! The renderer asks for a picture on the drawing thread when a block closes,
//! and for the same picture again in a new palette or at a new width from what
//! it kept, so nothing here runs a drawer: a drawer's SVG arrives with its
//! inks named by slot, and the key's palette says what each slot is worn as,
//! which under the ansi theme is xterm's value for the terminal's own colour
//! and the ground the terminal reported.
//!
//! A picture is rasterised at the narrowest of its own width, the widest the
//! raster's ceiling on decoded pixels holds, and the room it is placed in: the
//! narrow edge of the window's width bucket, in cells of the window's pixels.
//! It is never scaled up, so a small diagram stays its own size. A Sixel
//! picture is also drawn no taller than the rows its key gives it, since the
//! terminal shows its pixels as they are; a PNG is scaled into its cells by
//! the terminal and is drawn once for any of them.
//!
//! The decode slot is the one place in the process a picture is drawn. The
//! drawing thread waits [`WAIT`] for it and no longer; a picture it could not
//! have in time is drawn as text, so a frame is never held for another
//! thread's decode.
//!
//! Nothing is kept here: the renderer keeps what it shows, bounded, and asks
//! again from that, so [`Raster::cached`] holds nothing. A session's own
//! pictures are [`Caching`], which hands each diagram it draws to the one
//! thread that writes the picture cache, without waiting, and asks that cache
//! first only while a session put back is drawn: on a full screen for the
//! newest diagrams the renderer keeps once the walk is over, and in native
//! mode for each diagram as the walk sends it into the scrollback. Live, a
//! frame never reads the disk. An image is never handed to it, since it is
//! read again from its file.
//!
//! A display formula's picture is kept and read under the toolchain the
//! session's check found, and is neither while the check has not passed: a
//! picture another TeX made is never shown as this one's, and a session put
//! back runs no TeX. In native mode a formula put back is read as a diagram
//! is, so one sent into the scrollback before the check passed stays its
//! text. On a full screen it is read by [`kept_formulas`], on the math task's
//! thread, for the formulas the renderer tells out once the walk is over.
//!
//! An image arrives as the PNG the application read it into, already within
//! its ceilings and made by this process, and is decoded again here in the
//! slot only to learn its size and, for Sixel, its pixels. Kitty and iTerm2
//! are sent that PNG, said to be no wider than its room so the terminal
//! scales it into the cells it is placed in. Sixel cannot be scaled, so an
//! image wider than its room or than Sixel is encoded at, or taller than the
//! rows its key gives it, is refused as too wide rather than drawn larger
//! than the window or cut.
//!
//! Kitty and iTerm2 are sent the same PNG. Sixel is sent the pixels laid over
//! the palette's ground, since it has no transparency, so a new ground is a
//! new payload; a picture wider than Sixel is encoded at is drawn as text, as
//! too wide, rather than narrowed.

use super::math_engine::{Outcome, Read};
use super::picture_cache::{Cache, Entry, Queue};
use crucible_client_api::ImageRefused;
use crucible_raster::{
    MOST_SIXEL_WIDTH, Refused, SixelPalette, Slot, decode, png, raster_width, rasterise, sixel,
};
use crucible_tui::KeptPicture;
use crucible_tui::chrome::Undrawn;
use crucible_tui::graphics::{Form, Formula, Payload, PictureKey, Pictures, Protocol};
use crucible_tui::markdown::Kind;
use std::sync::Arc;
use std::time::Duration;

/// The longest the drawing thread waits for the decode slot before it draws
/// the block as text.
pub(super) const WAIT: Duration = Duration::from_millis(40);

/// The most colours a Sixel payload defines: the registers a terminal that
/// draws Sixel is expected to have.
const SIXEL_COLOURS: u16 = 256;

/// The session's pictures, drawn by `crucible_raster`.
#[derive(Debug, Default)]
pub(crate) struct Raster;

impl Pictures for Raster {
    fn cached(&mut self, _: &str, _: &PictureKey) -> Option<(Form, Payload)> {
        None
    }

    fn make(&mut self, _: &str, key: &PictureKey, form: &Form) -> Result<Payload, Undrawn> {
        encoded(form, key)
    }

    fn reencode(&mut self, kept: &KeptPicture, key: &PictureKey) -> Result<Payload, Undrawn> {
        encoded(&kept.form, key)
    }
}

/// The session's pictures drawn as [`Raster`] draws them, each new one handed
/// to the picture cache's writer; and, while a session put back is drawn,
/// looked for in that cache first.
#[derive(Debug)]
pub(crate) struct Caching {
    cache: Cache,
    queue: Queue,
    resuming: bool,
    /// What the session's check found, which names the toolchain a
    /// formula's picture is kept and read under.
    math: Outcome,
}

impl Caching {
    /// Pictures read from `cache` on resume and written through `queue`, a
    /// formula's under the toolchain `math` names.
    pub(crate) const fn new(cache: Cache, queue: Queue, math: Outcome) -> Self {
        Self {
            cache,
            queue,
            resuming: false,
            math,
        }
    }
}

impl Pictures for Caching {
    fn cached(&mut self, source: &str, key: &PictureKey) -> Option<(Form, Payload)> {
        // Asked live, it would read the disk inside a frame; live, the
        // drawer runs instead, and only a session put back reads it.
        if !self.resuming {
            return None;
        }
        if key.kind == Kind::DisplayMath {
            // Read only under the toolchain this session's check found, so a
            // picture another TeX made is never shown as this one's; before
            // the check has passed, the formula stays its text.
            let toolchain = self.math.toolchain()?;
            return self.cache.get_made_by(source, key, &toolchain);
        }
        self.cache.get(source, key)
    }

    fn make(&mut self, source: &str, key: &PictureKey, form: &Form) -> Result<Payload, Undrawn> {
        let payload = encoded(form, key)?;
        // A full queue drops the write: the picture is drawn either way. A
        // formula's is kept under the toolchain that made it, and not at all
        // where none is named.
        let _ = if key.kind == Kind::DisplayMath {
            self.math
                .toolchain()
                .and_then(|toolchain| Entry::made_by(source, key, &toolchain, form, &payload))
                .is_some_and(|entry| self.queue.sent(entry))
        } else {
            self.queue.put(source, key, form, &payload)
        };
        Ok(payload)
    }

    fn reencode(&mut self, kept: &KeptPicture, key: &PictureKey) -> Result<Payload, Undrawn> {
        encoded(&kept.form, key)
    }

    fn resuming(&mut self, resuming: bool) {
        self.resuming = resuming;
    }
}

/// What reads, from `cache`, the picture an earlier session kept of a formula
/// put back on a full screen, under the toolchain this session's check found:
/// asked on the math task's thread, never the drawing thread's.
pub(crate) fn kept_formulas(cache: Cache) -> Read<Formula> {
    Arc::new(move |formula: &Formula, toolchain: &str| {
        let key = formula.kept()?;
        match cache.get_made_by(formula.source(), key, toolchain)? {
            (Form::Svg(svg), _) => Some(svg),
            // A formula is kept as the picture TeX made of it, never a PNG.
            (Form::Png(_), _) => None,
        }
    })
}

/// `form` drawn in `key`'s palette at the width its room allows, and encoded
/// for `key`'s protocol; or why it is drawn as text.
fn encoded(form: &Form, key: &PictureKey) -> Result<Payload, Undrawn> {
    let svg = match form {
        Form::Svg(svg) => svg.as_str().as_bytes(),
        Form::Png(png) => return pictured(png, key),
    };
    let room = room(key);
    // Taken here, before the size is read, since reading it parses the whole
    // picture; and so not waited for again inside: the raster's own entry
    // points let the thread that holds the slot straight through.
    let _slot = Slot::take(WAIT).map_err(refusal)?;
    let width = raster_width(svg).map_err(refusal)?.min(room);
    let mut pixels = rasterise(svg, width, &key.palette.inks()).map_err(refusal)?;
    // A Sixel bitmap is shown pixel for pixel, so one taller than the rows
    // it is shown in is drawn again as much narrower as brings it into them.
    let tall = u32::from(key.rows.max(1)).saturating_mul(u32::from(key.cell.height));
    if key.protocol == Protocol::Sixel && pixels.height() > tall {
        let narrower = u64::from(width) * u64::from(tall) / u64::from(pixels.height());
        let narrower = u32::try_from(narrower).unwrap_or(width).max(1);
        pixels = rasterise(svg, narrower, &key.palette.inks()).map_err(refusal)?;
    }
    let bytes = match key.protocol {
        Protocol::Kitty | Protocol::Iterm2 => png(&pixels).map_err(refusal)?,
        // Wider than Sixel is encoded at, it is text: drawn narrower it would
        // not be the picture at its own size.
        Protocol::Sixel if pixels.width() > MOST_SIXEL_WIDTH => return Err(Undrawn::TooWide),
        Protocol::Sixel => sixel(
            &pixels,
            &SixelPalette {
                ground: key.palette.ground(),
                colours: SIXEL_COLOURS,
            },
        )
        .map_err(refusal)?,
    };
    Ok(Payload::new(
        key.protocol,
        bytes,
        (pixels.width(), pixels.height()),
        key.cell,
    ))
}

/// How many pixels wide a picture under `key` has room for: the narrow edge
/// of the window's width bucket, in cells of the window's pixels.
fn room(key: &PictureKey) -> u32 {
    u32::try_from(key.bucket.columns())
        .unwrap_or(u32::MAX)
        .saturating_mul(u32::from(key.cell.width))
}

/// The image `png` encoded for `key`'s protocol at no more than its room.
fn pictured(png: &[u8], key: &PictureKey) -> Result<Payload, Undrawn> {
    let room = room(key);
    let _slot = Slot::take(WAIT).map_err(refusal)?;
    let pixels = decode(png).map_err(refusal)?;
    let (width, height) = (pixels.width(), pixels.height());
    match key.protocol {
        Protocol::Kitty | Protocol::Iterm2 => {
            let shown = if width > room {
                let fitted = u64::from(height) * u64::from(room) / u64::from(width);
                (room, u32::try_from(fitted).unwrap_or(height).max(1))
            } else {
                (width, height)
            };
            Ok(Payload::new(key.protocol, png.to_vec(), shown, key.cell))
        }
        Protocol::Sixel => {
            let tall = u32::from(key.rows.max(1)).saturating_mul(u32::from(key.cell.height));
            if width > room.min(MOST_SIXEL_WIDTH) || height > tall {
                return Err(Undrawn::TooWide);
            }
            let bytes = sixel(
                &pixels,
                &SixelPalette {
                    ground: key.palette.ground(),
                    colours: SIXEL_COLOURS,
                },
            )
            .map_err(refusal)?;
            Ok(Payload::new(key.protocol, bytes, (width, height), key.cell))
        }
    }
}

/// The words an image's edge gives for why the application did not show it.
///
/// Written out case by case so a new reason is a decision here.
pub(crate) const fn refused(refused: ImageRefused) -> Undrawn {
    match refused {
        ImageRefused::NotShown => Undrawn::ImageNotShown,
        ImageRefused::TooLarge => Undrawn::ImageTooLarge,
        ImageRefused::NotAnImage => Undrawn::NotAnImage,
        ImageRefused::NotInTime => Undrawn::NotReadInTime,
        ImageRefused::Changed => Undrawn::ChangedSinceRead,
    }
}

/// A formula's picture as `dvisvgm` made it, worn in the transcript's own
/// colour: or `None`, where it is not text with a root to wear one on.
///
/// `dvisvgm` names no colour, so every glyph would be drawn black on any
/// ground. The root is given the class plain text is inked by and a fill of
/// `currentColor`, which every glyph inherits, so the raster draws it in the
/// palette's ink for that slot, and in the new one when the theme changes.
/// Everything else in the picture is left as it came, for the raster to take
/// or refuse.
pub(super) fn inked(made: &crucible_app::math::Svg) -> Option<crucible_tui::drawing::Svg> {
    let text = std::str::from_utf8(made.bytes()).ok()?;
    let root = text.match_indices("<svg").find_map(|(at, _)| {
        let after = at + "<svg".len();
        text.get(after..)?
            .starts_with(|next: char| next.is_ascii_whitespace() || next == '>' || next == '/')
            .then_some(after)
    })?;
    let (head, tail) = text.split_at_checked(root)?;
    let class = crucible_tui::Slot::Plain.class();
    crucible_tui::drawing::Svg::new(format!(
        "{head} class=\"{class}\" fill=\"currentColor\"{tail}"
    ))
    .ok()
}

/// Why a picture the raster refused is drawn as text.
///
/// Written out case by case so a new refusal is a decision here.
const fn refusal(refused: Refused) -> Undrawn {
    match refused {
        Refused::TooLarge => Undrawn::PastCap,
        Refused::NotAnImage | Refused::OverTime => Undrawn::NotHere,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crucible_tui::drawing::{Bucket, Mode, Svg};
    use crucible_tui::graphics::{Capability, Cell, Protocol};
    use crucible_tui::markdown::Kind;
    use crucible_tui::{
        MOST_KEPT_IMAGES, Palette, PictureSize, Recording, Renderer, ScreenMode, Size, Terminal,
        TerminalError, Theme,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError, mpsc};
    use std::time::Instant;

    /// A picture as `dvisvgm` writes one: its header, and a root naming no
    /// colour, so everything in it is drawn in the default black.
    const MADE: &str = "<?xml version='1.0' encoding='UTF-8'?>\n<!-- This file was generated by dvisvgm 3.6 -->\n<svg version='1.1' xmlns='http://www.w3.org/2000/svg' width='8pt' height='8pt' viewBox='0 0 8 8'>\n<path d='M0 0H8V8H0Z'/>\n</svg>";

    #[test]
    fn a_formula_picture_is_worn_in_the_ink_of_plain_text() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let made = crucible_app::math::Svg::new(MADE.as_bytes());
        let svg = inked(&made).expect("a picture with a root");
        let inks = [(crucible_tui::Slot::Plain.class(), [200, 10, 20])];
        let pixels = rasterise(svg.as_str().as_bytes(), 8, &inks).unwrap();
        let rgba = pixels.rgba();
        assert_eq!(rgba.get(..4), Some(&[200, 10, 20, 255][..]));
    }

    #[test]
    fn a_formula_picture_that_is_not_text_or_has_no_root_is_none() {
        assert!(inked(&crucible_app::math::Svg::new(&b"<svg \xff/>"[..])).is_none());
        assert!(inked(&crucible_app::math::Svg::new(&b"<g/>"[..])).is_none());
        assert!(inked(&crucible_app::math::Svg::new(&b"<svgx/>"[..])).is_none());
    }

    /// Every test here takes the one decode slot in the process, and one of
    /// them holds it on purpose: they take turns, so none is refused for
    /// another's sake.
    pub(crate) static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// The cell every test's window draws in.
    const CELL: Cell = Cell {
        width: 8,
        height: 16,
    };

    /// A diagram's key, `columns` wide, in `theme` against the reported
    /// `ground`, on a truecolour Kitty terminal.
    fn key(theme: Theme, ground: Option<(u8, u8, u8)>, columns: usize) -> PictureKey {
        key_for(Protocol::Kitty, theme, ground, columns)
    }

    /// The same key, on a truecolour terminal that confirmed `protocol`.
    fn key_for(
        protocol: Protocol,
        theme: Theme,
        ground: Option<(u8, u8, u8)>,
        columns: usize,
    ) -> PictureKey {
        let truecolour = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
        PictureKey {
            kind: Kind::Mermaid,
            bucket: Bucket::of(columns),
            palette: Palette::resolve(true, theme, ground, &truecolour).identity(),
            protocol,
            cell: CELL,
            rows: crucible_tui::MOST_PLACED,
        }
    }

    /// A picture `width` by `height`, its left half inked as the accent and
    /// its right half in the reader's own foreground.
    fn halves(width: u32, height: u32) -> Form {
        let half = width / 2;
        Form::Svg(
            Svg::new(format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">\
                 <rect class=\"Accent\" width=\"{half}\" height=\"{height}\" fill=\"currentColor\"/>\
                 <rect class=\"Plain\" x=\"{half}\" width=\"{half}\" height=\"{height}\" fill=\"currentColor\"/>\
                 </svg>"
            ))
            .unwrap(),
        )
    }

    /// The colour of the pixel at `x`, `y` of the PNG `payload` carries.
    fn colour_at(payload: &Payload, x: u32, y: u32) -> [u8; 3] {
        let pixels = image::load_from_memory(payload.bytes()).unwrap().to_rgb8();
        pixels.get_pixel(x, y).0
    }

    #[test]
    fn an_ansi_picture_is_inked_in_xterms_sixteen_and_against_the_reported_ground() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let form = halves(40, 20);

        // Under a light ground the reader reported, the accent is xterm's
        // cyan and the reader's own foreground stands out from that ground.
        let light = key(Theme::Ansi, Some((255, 255, 255)), 80);
        let payload = Raster.make("graph", &light, &form).unwrap();
        assert_eq!(payload.pixels(), (40, 20));
        assert_eq!(colour_at(&payload, 5, 10), [0, 205, 205]);
        assert_eq!(colour_at(&payload, 35, 10), [0, 0, 0]);

        // Worn again against a dark ground, from what it was drawn from.
        let kept = KeptPicture {
            pixels: payload.pixels(),
            form,
            payload,
            size: PictureSize::Fit,
        };
        let dark = key(Theme::Ansi, Some((30, 30, 30)), 80);
        let again = Raster.reencode(&kept, &dark).unwrap();
        assert_eq!(colour_at(&again, 5, 10), [0, 205, 205]);
        assert_eq!(colour_at(&again, 35, 10), [255, 255, 255]);
    }

    #[test]
    fn a_picture_is_drawn_at_its_own_width_or_its_rooms_whichever_is_less() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);

        // Narrower than its room: its own width, never scaled up.
        let narrow = Raster
            .make("graph", &key(Theme::Dark, None, 80), &halves(40, 20))
            .unwrap();
        assert_eq!(narrow.pixels(), (40, 20));

        // Wider than its room: the room's, which is the bucket's narrow edge
        // in cells of the window's pixels, eight cells of eight here.
        let wide = Raster
            .make("graph", &key(Theme::Dark, None, 10), &halves(400, 200))
            .unwrap();
        assert_eq!(wide.pixels(), (64, 32));
        assert_eq!(wide.cells(), (8, 2));
    }

    /// A picture `width` by `height` with nothing on it, so every pixel is
    /// the ground it is laid over.
    fn bare(width: u32, height: u32) -> Form {
        Form::Svg(
            Svg::new(format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">\
                 <rect class=\"Accent\" width=\"2\" height=\"2\" fill=\"currentColor\"/>\
                 </svg>"
            ))
            .unwrap(),
        )
    }

    #[test]
    fn an_iterm2_picture_is_the_png_its_protocol_names() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let key = key_for(Protocol::Iterm2, Theme::Dark, None, 80);
        let payload = Raster.make("graph", &key, &halves(40, 20)).unwrap();

        assert_eq!(payload.protocol(), Protocol::Iterm2);
        assert!(payload.bytes().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(payload.pixels(), (40, 20));
        // The same image Kitty is sent: only the sequence around it differs.
        let kitty = Raster
            .make(
                "graph",
                &key_for(Protocol::Kitty, Theme::Dark, None, 80),
                &halves(40, 20),
            )
            .unwrap();
        assert_eq!(payload.bytes(), kitty.bytes());
    }

    #[test]
    fn a_sixel_picture_is_drawn_no_taller_than_the_rows_its_key_gives_it_and_a_png_is_not() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        // Five rows of sixteen pixels: a picture twice as wide as it is
        // tall is drawn eighty pixels tall, and as much narrower as keeps
        // its shape, since a Sixel bitmap is drawn cell for cell.
        let sixel = PictureKey {
            rows: 5,
            ..key_for(Protocol::Sixel, Theme::Dark, None, 80)
        };
        let short = Raster.make("graph", &sixel, &halves(400, 200)).unwrap();
        assert_eq!(short.pixels(), (160, 80));
        assert_eq!(short.cells(), (20, 5));

        // A PNG is scaled by the terminal into whatever cells it is shown
        // in, so it is drawn at its own size whatever the rows.
        let png = PictureKey {
            rows: 5,
            ..key(Theme::Dark, None, 80)
        };
        let whole = Raster.make("graph", &png, &halves(400, 200)).unwrap();
        assert_eq!(whole.pixels(), (400, 200));
    }

    #[test]
    fn a_sixel_picture_is_one_device_control_string_and_carries_no_escape_of_its_own() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let key = key_for(Protocol::Sixel, Theme::Dark, None, 80);
        let payload = Raster.make("graph", &key, &halves(40, 20)).unwrap();

        assert_eq!(payload.protocol(), Protocol::Sixel);
        assert_eq!(payload.pixels(), (40, 20));
        let bytes = payload.bytes();
        let inside = bytes
            .strip_prefix(b"\x1bP")
            .and_then(|rest| rest.strip_suffix(b"\x1b\\"))
            .unwrap_or_else(|| panic!("not one device control string: {bytes:?}"));
        // Nothing between the two may end the string early or open another:
        // no escape, no eight-bit string terminator, no bell, no control at
        // all, only the printable characters Sixel is spelt in.
        assert!(
            inside.iter().all(|byte| (0x20..0x7f).contains(byte)),
            "a byte outside Sixel's alphabet: {inside:?}"
        );
        // It opens as Sixel, its numeric parameters and then `q`, as the
        // renderer requires before it writes one.
        let parameters = inside
            .iter()
            .take_while(|byte| byte.is_ascii_digit() || **byte == b';')
            .count();
        assert_eq!(
            inside.get(parameters),
            Some(&b'q'),
            "not opened as Sixel: {:?}",
            inside.get(..16)
        );

        // What it was drawn from and what it is are never the same bytes, so
        // the record counts both.
        let kept = KeptPicture {
            pixels: payload.pixels(),
            form: halves(40, 20),
            payload,
            size: PictureSize::Fit,
        };
        assert_eq!(kept.bytes(), kept.form.len() + kept.payload.bytes().len());
    }

    #[test]
    fn a_sixel_picture_is_laid_over_the_ground_of_the_theme_it_is_worn_in() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let form = bare(40, 20);
        let dark = Raster
            .make(
                "graph",
                &key_for(Protocol::Sixel, Theme::Dark, None, 80),
                &form,
            )
            .unwrap();
        let light = Raster
            .make(
                "graph",
                &key_for(Protocol::Sixel, Theme::Light, None, 80),
                &form,
            )
            .unwrap();

        // Sixel names each colour in hundredths: the empty picture is black
        // under the dark theme and white under the light one.
        let says = |payload: &Payload, colour: &str| {
            String::from_utf8_lossy(payload.bytes()).contains(colour)
        };
        assert!(
            says(&dark, ";2;0;0;0"),
            "{:?}",
            String::from_utf8_lossy(dark.bytes())
        );
        assert!(!says(&dark, ";2;100;100;100"));
        assert!(
            says(&light, ";2;100;100;100"),
            "{:?}",
            String::from_utf8_lossy(light.bytes())
        );
        assert_ne!(dark.bytes(), light.bytes());

        // A ground the terminal reported is the one it is laid over.
        let reported = Raster
            .make(
                "graph",
                &key_for(Protocol::Sixel, Theme::Dark, Some((255, 255, 255)), 80),
                &form,
            )
            .unwrap();
        assert!(says(&reported, ";2;100;100;100"));
    }

    #[test]
    fn a_sixel_picture_wider_than_six_hundred_pixels_is_too_wide_and_is_text() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        // Room for both: a hundred and twenty cells of eight pixels.
        let widest = crucible_raster::MOST_SIXEL_WIDTH;
        for theme in [Theme::Dark, Theme::Light] {
            let key = key_for(Protocol::Sixel, theme, None, 120);
            assert_eq!(
                Raster.make("graph", &key, &halves(widest + 1, 20)).err(),
                Some(Undrawn::TooWide),
                "{theme:?}"
            );
            let kept = KeptPicture {
                pixels: (widest + 1, 20),
                form: halves(widest + 1, 20),
                payload: Payload::new(Protocol::Sixel, Vec::new(), (widest + 1, 20), CELL),
                size: PictureSize::Fit,
            };
            assert_eq!(Raster.reencode(&kept, &key).err(), Some(Undrawn::TooWide));
            let at_most = Raster.make("graph", &key, &halves(widest, 20)).unwrap();
            assert_eq!(at_most.pixels(), (widest, 20));
        }
    }

    #[test]
    fn a_held_slot_delays_a_picture_by_no_more_than_the_wait_and_then_it_is_text() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let (taken, held) = mpsc::channel();
        let (free, freed) = mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let slot = crucible_raster::Slot::take(Duration::from_secs(10));
            taken.send(slot.is_ok()).unwrap();
            let _ = freed.recv();
            drop(slot);
        });
        assert!(held.recv().unwrap(), "the holder never took the slot");

        // Even a picture whose size cannot be read waits its turn: its size
        // is read by the parse that draws it, inside the slot.
        let form = halves(40, 20);
        let sizeless = Form::Svg(
            Svg::new(
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"4\"/></svg>"
                    .to_owned(),
            )
            .unwrap(),
        );
        let key = key(Theme::Dark, None, 80);
        let mut waits = Vec::new();
        for asked_for in [&form, &sizeless] {
            let asked = Instant::now();
            let made = Raster.make("graph", &key, asked_for);
            waits.push((made.err(), asked.elapsed()));
        }

        free.send(()).unwrap();
        holder.join().unwrap();
        for (refused, waited) in waits {
            assert_eq!(refused, Some(Undrawn::NotHere));
            assert!(waited >= WAIT, "it gave up after {waited:?}");
            // The wait, and the time a woken thread takes to be run again on
            // a busy machine; nowhere near the ten seconds a raster entry
            // point waits for the slot on its own.
            assert!(
                waited < WAIT + Duration::from_millis(200),
                "it waited {waited:?}"
            );
        }

        // The slot free again, the same picture is drawn.
        assert!(Raster.make("graph", &key, &form).is_ok());
    }

    #[test]
    fn a_sessions_screen_sends_a_diagram_to_a_terminal_that_confirmed_kitty() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let truecolour = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
        let mut renderer = crate::cli::furnished(
            Recording::new(80, 40),
            &crucible_config::Settings::default(),
            Box::new(crate::cli::pictures::Raster),
        );
        renderer.wears(Palette::resolve(true, Theme::Dark, None, &truecolour));
        renderer.images(Capability::Graphics {
            protocol: Protocol::Kitty,
            cell: CELL,
        });
        renderer.diagrams(Mode::Image);

        renderer
            .stream("```mermaid\ngraph TD\n  A --> B\n```\n")
            .unwrap();
        renderer.settle().unwrap();

        let written = renderer.terminal().written();
        assert!(
            written.contains("\x1b_Ga=t,f=100,i=") && written.contains("\x1b_Ga=p,U=1,i="),
            "no picture was sent: {written:?}"
        );
    }

    /// A session on a truecolour Kitty terminal 80 by 40 on `screen`, whose
    /// display formulas are pictures drawn by `pictures`.
    fn math_session(screen: ScreenMode, pictures: Box<dyn Pictures>) -> Renderer<Recording> {
        let truecolour = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
        let mut renderer = Renderer::drawing(Recording::new(80, 40), screen);
        renderer.drawers(crate::cli::draw::drawers());
        renderer.pictures(pictures);
        renderer.wears(Palette::resolve(true, Theme::Dark, None, &truecolour));
        renderer.images(Capability::Graphics {
            protocol: Protocol::Kitty,
            cell: CELL,
        });
        renderer.math(Mode::Image);
        renderer
    }

    /// Whether `renderer` has sent its terminal a Kitty picture.
    fn sent_a_picture(renderer: &Renderer<Recording>) -> bool {
        renderer.terminal().written().contains("\x1b_Ga=t,f=100,i=")
    }

    #[test]
    fn a_formula_a_session_put_back_shows_is_read_from_what_was_kept_under_the_toolchain_found() {
        use crate::cli::math_engine::Said;
        use crate::cli::picture_cache::{Cache, writing};
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let sample = crate::cli::sample::Sample::new("pictures-kept-formula");
        let cache = Cache::open(&sample.root());
        let found = || Outcome::of(Said::Available("stand-in".into()));
        let formula = "$$\nx^2\n$$\n";

        // Shown live, its picture is kept under the toolchain that made it.
        let (queue, writer) = writing(cache.clone());
        let caching = Caching::new(cache.clone(), queue, found());
        let mut live = math_session(ScreenMode::Fullscreen, Box::new(caching));
        live.stream(formula).unwrap();
        live.settle().unwrap();
        let told = live.formulas();
        let [made] = told.as_slice() else {
            panic!("told {told:?}");
        };
        assert!(made.kept().is_none(), "a live formula is made");
        // Sized in bare numbers, as the raster reads a size.
        let svg = inked(&crucible_app::math::Svg::new(
            &b"<svg xmlns='http://www.w3.org/2000/svg' width='16' height='8'><path d='M0 0H16V8H0Z'/></svg>"[..],
        ))
        .unwrap();
        live.formula_made(made, svg).unwrap();
        assert!(sent_a_picture(&live), "not shown live");
        writer.drain();

        // Put back on a full screen, it is told out to be read, and what is
        // read for it under the toolchain found is shown; under another,
        // nothing is.
        let (queue, _writer) = writing(cache.clone());
        let caching = Caching::new(cache.clone(), queue, found());
        let mut put_back = math_session(ScreenMode::Fullscreen, Box::new(caching));
        put_back.resuming(true);
        put_back.stream(formula).unwrap();
        put_back.resuming(false);
        put_back.settle().unwrap();
        assert!(!sent_a_picture(&put_back), "read on the drawing thread");
        let told = put_back.formulas();
        let [kept] = told.as_slice() else {
            panic!("told {told:?}");
        };
        assert!(kept.kept().is_some(), "put back to be read: {kept:?}");
        let read = kept_formulas(cache.clone());
        assert!(read(kept, "TeX Live 2024").is_none(), "read under another");
        put_back
            .formula_made(kept, read(kept, "stand-in").expect("what was kept"))
            .unwrap();
        assert!(sent_a_picture(&put_back), "not shown");

        // In native mode, put back once the check has passed, it is read as
        // it goes into the scrollback; before, it stays its text.
        for (said, shown) in [
            (Said::Available("stand-in".into()), true),
            (Said::Asking, false),
        ] {
            let (queue, _writer) = writing(cache.clone());
            let caching = Caching::new(cache.clone(), queue, Outcome::of(said.clone()));
            let mut native = math_session(ScreenMode::Native, Box::new(caching));
            native.resuming(true);
            native.stream(formula).unwrap();
            native.settle().unwrap();
            native.resuming(false);
            assert_eq!(sent_a_picture(&native), shown, "{said:?}");
            assert!(native.formulas().is_empty(), "{said:?} was told out");
        }
    }

    /// The longest a width change may take, built optimised as the binary
    /// ships, to lay a session at the caps out again and draw it.
    const RELAID_WITHIN: Duration = Duration::from_millis(500);

    /// How many diagrams the timed session draws: four times as many as the
    /// record keeps pictures of.
    const DRAWN: usize = 4 * MOST_KEPT_IMAGES;

    /// How many of them are refused a picture only once it is drawn, for its
    /// size.
    const TOO_WIDE: usize = 2;

    /// A window whose size changes while the renderer holds it, as a
    /// terminal's does.
    struct Resizing(std::cell::Cell<Size>);

    impl Terminal for Resizing {
        fn size(&self) -> Result<Size, TerminalError> {
            Ok(self.0.get())
        }

        fn write(&mut self, _: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn flush(&mut self) -> Result<(), TerminalError> {
            Ok(())
        }

        fn is_terminal(&self) -> bool {
            true
        }
    }

    /// What a [`Counted`] raster has been asked for.
    #[derive(Debug, Default)]
    struct Asked {
        /// Pictures drawn from a drawer's SVG.
        made: AtomicUsize,
        /// Kept pictures encoded again.
        reencoded: AtomicUsize,
    }

    /// [`Raster`], counting what it is asked for.
    struct Counted(Arc<Asked>);

    impl Pictures for Counted {
        fn cached(&mut self, source: &str, key: &PictureKey) -> Option<(Form, Payload)> {
            Raster.cached(source, key)
        }

        fn make(
            &mut self,
            source: &str,
            key: &PictureKey,
            form: &Form,
        ) -> Result<Payload, Undrawn> {
            self.0.made.fetch_add(1, Ordering::Relaxed);
            Raster.make(source, key, form)
        }

        fn reencode(&mut self, kept: &KeptPicture, key: &PictureKey) -> Result<Payload, Undrawn> {
            self.0.reencoded.fetch_add(1, Ordering::Relaxed);
            Raster.reencode(kept, key)
        }
    }

    /// The `number`th gantt chart, in its fence: one section named for
    /// `number`, so no two are the same picture, and `tasks` tasks beside it,
    /// each two days long and starting on its own day within `span` days,
    /// every name as wide as a gantt draws one whole and spelt in the letters
    /// of `word`.
    fn gantt(number: usize, span: usize, tasks: usize, word: &str) -> String {
        use std::fmt::Write as _;

        let caps = crucible_tui_mermaid::Caps::GANTT;
        let columns = caps.label.unwrap_or(caps.bytes);
        let label = |index: usize| {
            let head = index.to_string();
            let tail: String = word
                .chars()
                .cycle()
                .take(columns.saturating_sub(head.len()))
                .collect();
            format!("{head}{}", tail.trim_end())
        };
        let mut fence = format!(
            "```mermaid\ngantt\ndateFormat YYYY-MM-DD\nsection {}\n",
            label(number)
        );
        for task in 1..=tasks {
            let day = task * 2 % span;
            let (month, date) = if day < 31 {
                (1, day + 1)
            } else {
                (2, day - 30)
            };
            let _ = writeln!(fence, "{} :2026-{month:02}-{date:02}, 2d", label(task));
        }
        fence.push_str("```\n\n");
        fence
    }

    #[test]
    fn a_width_change_at_the_caps_encodes_again_the_kept_pictures_and_no_more() {
        // The record's pictures at their caps: as many as it keeps, each a
        // gantt as large as sixteen can be together, and one more drawn as
        // text again past them. Every other diagram is a gantt at a gantt's
        // caps, laid out again as text: a few refused their picture for its
        // size once it was drawn, and the rest by their drawer, for letters
        // the picture's font has none for.
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let truecolour = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
        let at_caps = crucible_tui_mermaid::Caps::GANTT.nodes - 1;
        let pictured = MOST_KEPT_IMAGES + 1;
        let fence = |number: usize| {
            if number < TOO_WIDE {
                gantt(number, 38, at_caps, "word ")
            } else if number < DRAWN - pictured {
                gantt(number, 38, at_caps, "слово ")
            } else {
                gantt(number, 24, 26, "word ")
            }
        };
        for screen in [ScreenMode::Fullscreen, ScreenMode::Native] {
            let window = Resizing(std::cell::Cell::new(Size {
                columns: 80,
                rows: 40,
            }));
            let mut renderer = Renderer::drawing(window, screen);
            let asked = Arc::new(Asked::default());
            renderer.drawers(crate::cli::draw::drawers());
            renderer.pictures(Box::new(Counted(Arc::clone(&asked))));
            renderer.wears(Palette::resolve(true, Theme::Dark, None, &truecolour));
            renderer.images(Capability::Graphics {
                protocol: Protocol::Kitty,
                cell: CELL,
            });
            renderer.diagrams(Mode::Image);
            for number in 0..DRAWN {
                renderer.stream(&fence(number)).unwrap();
            }
            renderer.settle().unwrap();
            assert_eq!(
                asked.made.load(Ordering::Relaxed),
                TOO_WIDE + pictured,
                "{screen:?}"
            );
            assert_eq!(asked.reencoded.load(Ordering::Relaxed), 0, "{screen:?}");

            renderer.terminal().0.set(Size {
                columns: 120,
                rows: 40,
            });
            let started = Instant::now();
            renderer.resized().unwrap();
            let took = started.elapsed();

            assert_eq!(
                asked.made.load(Ordering::Relaxed),
                TOO_WIDE + pictured,
                "{screen:?}: a width change drew a picture the record does not keep"
            );
            assert_eq!(
                asked.reencoded.load(Ordering::Relaxed),
                MOST_KEPT_IMAGES,
                "{screen:?}: a width change did not encode every kept picture again"
            );
            // Unoptimised, drawing a picture takes several times as long as
            // in the binary that ships, so only an optimised build is held to
            // the bound.
            if !cfg!(debug_assertions) {
                assert!(
                    took < RELAID_WITHIN,
                    "{screen:?}: a width change took {took:?}"
                );
            }

            // A new palette encodes the kept pictures again, and draws none
            // a picture was refused for its size.
            renderer.wears(Palette::resolve(true, Theme::Light, None, &truecolour));
            renderer.settle().unwrap();

            assert_eq!(
                asked.made.load(Ordering::Relaxed),
                TOO_WIDE + pictured,
                "{screen:?}: a new palette drew a picture refused for its size"
            );
            assert_eq!(
                asked.reencoded.load(Ordering::Relaxed),
                2 * MOST_KEPT_IMAGES,
                "{screen:?}: a new palette did not encode every kept picture again"
            );
        }
    }

    /// A PNG `width` by `height`, as the application hands one over.
    fn png_of(width: u32, height: u32) -> Vec<u8> {
        let Form::Svg(svg) = halves(width, height) else {
            return Vec::new();
        };
        let pixels = rasterise(svg.as_str().as_bytes(), width, &[]).unwrap();
        png(&pixels).unwrap()
    }

    #[test]
    fn an_image_is_sent_as_the_png_the_application_read_said_to_be_no_wider_than_its_room() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        for protocol in [Protocol::Kitty, Protocol::Iterm2] {
            let key = PictureKey {
                kind: Kind::Image,
                ..key_for(protocol, Theme::Dark, None, 80)
            };
            let small = png_of(40, 20);
            let payload = Raster
                .make("a.png", &key, &Form::Png(small.clone()))
                .unwrap();
            assert_eq!(payload.bytes(), small.as_slice(), "{protocol:?}");
            assert_eq!(payload.pixels(), (40, 20), "{protocol:?}");

            let room = room(&key);
            let wide = png_of(room * 2, 100);
            let payload = Raster
                .make("a.png", &key, &Form::Png(wide.clone()))
                .unwrap();
            assert_eq!(payload.bytes(), wide.as_slice(), "{protocol:?}");
            assert_eq!(payload.pixels(), (room, 50), "{protocol:?}");
            assert!(
                usize::from(payload.cells().0) <= key.bucket.columns(),
                "{protocol:?}"
            );
        }
    }

    #[test]
    fn a_sixel_image_it_would_have_to_scale_is_too_wide_and_one_it_need_not_is_drawn() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let key = PictureKey {
            kind: Kind::Image,
            rows: 5,
            ..key_for(Protocol::Sixel, Theme::Dark, None, 120)
        };
        let widest = MOST_SIXEL_WIDTH.min(room(&key));
        for (width, height) in [(widest + 1, 20), (40, 5 * 16 + 1)] {
            assert_eq!(
                Raster
                    .make("a.png", &key, &Form::Png(png_of(width, height)))
                    .err(),
                Some(Undrawn::TooWide),
                "{width} by {height}"
            );
        }
        let drawn = Raster
            .make("a.png", &key, &Form::Png(png_of(widest, 5 * 16)))
            .unwrap();
        assert!(drawn.bytes().starts_with(b"\x1bP"), "not a Sixel string");
        assert_eq!(drawn.pixels(), (widest, 80));
    }

    #[test]
    fn bytes_that_are_not_an_image_are_never_sent() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        for protocol in [Protocol::Kitty, Protocol::Iterm2, Protocol::Sixel] {
            let key = key_for(protocol, Theme::Dark, None, 80);
            assert_eq!(
                Raster
                    .make("a.png", &key, &Form::Png(b"\x1b]52;c;aGk=\x07".to_vec()))
                    .err(),
                Some(Undrawn::NotHere),
                "{protocol:?}"
            );
        }
    }

    #[test]
    fn every_reason_an_image_is_not_shown_has_words_of_its_own_and_none_says_shown_as_written() {
        let words: Vec<&str> = ImageRefused::EVERY
            .into_iter()
            .map(|reason| {
                let undrawn = refused(reason);
                assert!(!undrawn.written(), "{reason:?}");
                undrawn.words()
            })
            .collect();
        assert_eq!(
            words,
            [
                "not shown",
                "too large",
                "not an image",
                "not read in time",
                "changed since it was read"
            ]
        );
    }
}
