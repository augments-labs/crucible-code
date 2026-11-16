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
//! holds nothing. Only Kitty is encoded for, and only a drawer's picture: an
//! image file's form is refused.

use crucible_raster::{Refused, Slot, png, raster_width, rasterise};
use crucible_tui::KeptPicture;
use crucible_tui::chrome::Undrawn;
use crucible_tui::graphics::{Form, Payload, PictureKey, Pictures, Protocol};
use std::time::Duration;

/// The longest the drawing thread waits for the decode slot before it draws
/// the block as text.
const WAIT: Duration = Duration::from_millis(40);

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
    match key.protocol {
        Protocol::Kitty => {}
        Protocol::Iterm2 | Protocol::Sixel => return Err(Undrawn::NotHere),
    }
    let svg = svg.as_str().as_bytes();
    let room = u32::try_from(key.bucket.columns())
        .unwrap_or(u32::MAX)
        .saturating_mul(u32::from(key.cell.width));
    let width = raster_width(svg).map_err(refusal)?.min(room);
    // Taken here, and so not waited for again inside: the raster's own entry
    // points let the thread that holds the slot straight through.
    let _slot = Slot::take(WAIT).map_err(refusal)?;
    let pixels = rasterise(svg, width, &key.palette.inks()).map_err(refusal)?;
    let bytes = png(&pixels).map_err(refusal)?;
    Ok(Payload::new(
        Protocol::Kitty,
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
    use crucible_tui::{Palette, PictureSize, Recording, Theme};
    use std::sync::{Mutex, PoisonError, mpsc};
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
        let truecolour = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
        PictureKey {
            kind: Kind::Mermaid,
            bucket: Bucket::of(columns),
            palette: Palette::resolve(true, theme, ground, &truecolour).identity(),
            protocol: Protocol::Kitty,
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

        let form = halves(40, 20);
        let key = key(Theme::Dark, None, 80);
        let asked = Instant::now();
        let made = Raster.make("graph", &key, &form);
        let waited = asked.elapsed();

        free.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(made.err(), Some(Undrawn::NotHere));
        assert!(waited >= WAIT, "it gave up after {waited:?}");
        // The wait, and the time a woken thread takes to be run again on a
        // busy machine; nowhere near the ten seconds a raster entry point
        // waits for the slot on its own.
        assert!(
            waited < WAIT + Duration::from_millis(200),
            "it waited {waited:?}"
        );

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
}
