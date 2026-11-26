//! A diagram's picture, drawn to pixels and encoded for the terminal.
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
//! It is never scaled up, so a small diagram stays its own size.
//!
//! The decode slot is the one place in the process a picture is drawn. The
//! drawing thread waits [`WAIT`] for it and no longer; a picture it could not
//! have in time is drawn as text, so a frame is never held for another
//! thread's decode.
//!
//! Nothing is kept here and nothing is written to disk: the renderer keeps
//! what it shows, bounded, and asks again from that, so [`Raster::cached`]
//! holds nothing. Only a drawer's picture is encoded: an image file's form is
//! refused.
//!
//! Kitty and iTerm2 are sent the same PNG. Sixel is sent the pixels laid over
//! the palette's ground, since it has no transparency, so a new ground is a
//! new payload; a picture wider than Sixel is encoded at is drawn as text, as
//! too wide, rather than narrowed.

use crucible_raster::{
    MOST_SIXEL_WIDTH, Refused, SixelPalette, Slot, png, raster_width, rasterise, sixel,
};
use crucible_tui::KeptPicture;
use crucible_tui::chrome::Undrawn;
use crucible_tui::graphics::{Form, Payload, PictureKey, Pictures, Protocol};
use std::time::Duration;

/// The longest the drawing thread waits for the decode slot before it draws
/// the block as text.
const WAIT: Duration = Duration::from_millis(40);

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

/// `form` drawn in `key`'s palette at the width its room allows, and encoded
/// for `key`'s protocol; or why it is drawn as text.
fn encoded(form: &Form, key: &PictureKey) -> Result<Payload, Undrawn> {
    let Form::Svg(svg) = form else {
        return Err(Undrawn::NotHere);
    };
    let svg = svg.as_str().as_bytes();
    let room = u32::try_from(key.bucket.columns())
        .unwrap_or(u32::MAX)
        .saturating_mul(u32::from(key.cell.width));
    // Taken here, before the size is read, since reading it parses the whole
    // picture; and so not waited for again inside: the raster's own entry
    // points let the thread that holds the slot straight through.
    let _slot = Slot::take(WAIT).map_err(refusal)?;
    let width = raster_width(svg).map_err(refusal)?.min(room);
    let pixels = rasterise(svg, width, &key.palette.inks()).map_err(refusal)?;
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
mod tests {
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

    /// Every test here takes the one decode slot in the process, and one of
    /// them holds it on purpose: they take turns, so none is refused for
    /// another's sake.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

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
}
