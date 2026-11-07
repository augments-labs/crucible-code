//! Pictures and image files as bounded pixels, and as PNG and Sixel payloads.

use image::{DynamicImage, ImageDecoder, ImageError, ImageReader, Limits};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Error as UsvgError, ImageHrefResolver, Options, Tree, fontdb};
use std::cell::Cell;
use std::io::Cursor;
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Why bytes did not become pixels, or pixels did not become a payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// Something went past a ceiling: the input, the pixels it would decode
    /// to, or the payload they would encode to.
    #[error("past a size ceiling")]
    TooLarge,
    /// The bytes are not a picture or an image file this crate reads.
    #[error("not a picture or image file this crate reads")]
    NotAnImage,
    /// The decode slot was not free in time.
    #[error("the decode slot was not free in time")]
    OverTime,
}

/// The most bytes of picture or image file any entry point reads: 1 MiB.
pub const MOST_INPUT: usize = 1 << 20;

/// The most bytes of decoded pixels an entry point returns, and the most the
/// decoders may allocate while making them: 2 MiB, which is 524 288 RGBA
/// pixels.
pub const MOST_DECODED: usize = 2 << 20;

/// The most bytes of PNG [`png`] returns: 1 MiB.
pub const MOST_PNG: usize = 1 << 20;

/// The widest picture, in pixels, [`sixel`] encodes: 600.
pub const MOST_SIXEL_WIDTH: u32 = 600;

/// A picture as straight (not premultiplied) RGBA, 8 bits a channel, whose
/// bytes are within [`MOST_DECODED`].
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Pixels {
    /// The pixels as the entry points make them, or refused past the ceiling.
    fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Pixels, Refused> {
        if width == 0
            || height == 0
            || rgba.len() != rgba_len(width, height).ok_or(Refused::TooLarge)?
        {
            return Err(Refused::NotAnImage);
        }
        if rgba.len() > MOST_DECODED {
            return Err(Refused::TooLarge);
        }
        Ok(Pixels {
            width,
            height,
            rgba,
        })
    }

    /// Columns.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Rows.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Row-major `[r, g, b, a]` bytes, four a pixel.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}x{})", self.width, self.height)
    }
}

/// The bytes of a `width` by `height` RGBA picture, or `None` where they do
/// not fit a `usize`.
fn rgba_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    usize::try_from(pixels.checked_mul(4)?).ok()
}

/// Decode a PNG, JPEG, GIF or WebP file; the first frame of an animation.
///
/// Refused past [`MOST_INPUT`] bytes in, and past [`MOST_DECODED`] bytes out —
/// judged from the header before anything is allocated for the pixels.
///
/// # Errors
/// [`Refused::TooLarge`] past a ceiling, [`Refused::NotAnImage`] for bytes no
/// decoder here reads, [`Refused::OverTime`] when the decode slot stays busy.
pub fn decode(bytes: &[u8]) -> Result<Pixels, Refused> {
    if bytes.len() > MOST_INPUT {
        return Err(Refused::TooLarge);
    }
    let _slot = Slot::take(WAIT)?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Refused::NotAnImage)?;
    reader.limits(decoding_limits());
    let decoder = reader.into_decoder().map_err(|error| refusal(&error))?;
    // The header is read; nothing for the pixels is allocated until the sizes
    // it claims have been held against the ceiling.
    let (width, height) = decoder.dimensions();
    if rgba_len(width, height).is_none_or(|rgba| rgba > MOST_DECODED)
        || usize::try_from(decoder.total_bytes()).is_ok_and(|held| held > MOST_DECODED)
    {
        return Err(Refused::TooLarge);
    }
    let picture = DynamicImage::from_decoder(decoder).map_err(|error| refusal(&error))?;
    Pixels::from_rgba(width, height, picture.into_rgba8().into_raw())
}

/// Draw an SVG at `width_px` wide, as straight RGBA with nothing behind it.
///
/// `inks` are the colours of the classes the SVG names: each `(class, rgb)`
/// colours, through `currentColor`, whatever carries that class, so one SVG is
/// drawn in the theme of whichever composition asks. A class is made of ASCII
/// letters, digits, `-` and `_`; any other name is refused rather than let into
/// the style sheet.
///
/// Nothing outside the bytes is read: an `<image>` that is not a `data:` URL, an
/// `@import` and a font URL are not followed, and the only font is the one this
/// crate carries. The height follows the SVG's own proportions, rounded up.
///
/// # Errors
/// [`Refused::TooLarge`] for an SVG past [`MOST_INPUT`] or a drawing past
/// [`MOST_DECODED`], [`Refused::NotAnImage`] for bytes that are not an SVG, a
/// zero width or an ink that cannot be a class, [`Refused::OverTime`] when the
/// decode slot stays busy.
pub fn rasterise(svg: &[u8], width_px: u32, inks: &[(&str, [u8; 3])]) -> Result<Pixels, Refused> {
    if svg.len() > MOST_INPUT {
        return Err(Refused::TooLarge);
    }
    if width_px == 0 {
        return Err(Refused::NotAnImage);
    }
    let options = Options {
        style_sheet: Some(style_sheet(inks)?),
        font_family: FONT_FAMILY.to_owned(),
        fontdb: fonts(),
        image_href_resolver: ImageHrefResolver {
            resolve_data: ImageHrefResolver::default_data_resolver(),
            // The default reads the file an `href` names. Nothing here is
            // allowed to name one.
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    let _slot = Slot::take(WAIT)?;
    let tree = Tree::from_data(svg, &options).map_err(|error| match error {
        UsvgError::ElementsLimitReached => Refused::TooLarge,
        _ => Refused::NotAnImage,
    })?;
    let size = tree.size();
    // The ceiling is judged on the size the drawing would have, before a pixmap
    // is asked for. A float is cast to a `u32` by saturating, so a height too
    // tall for one is still too tall for the ceiling.
    let height = (f64::from(size.height()) * f64::from(width_px) / f64::from(size.width()))
        .ceil()
        .max(1.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "saturates, and the ceiling is checked next"
    )]
    let height = height as u32;
    if rgba_len(width_px, height).is_none_or(|rgba| rgba > MOST_DECODED) {
        return Err(Refused::TooLarge);
    }
    let mut canvas = Pixmap::new(width_px, height).ok_or(Refused::TooLarge)?;
    #[allow(clippy::cast_precision_loss, reason = "a size under the ceiling")]
    let scale = Transform::from_scale(
        width_px as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, scale, &mut canvas.as_mut());
    let straight = canvas
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    Pixels::from_rgba(width_px, height, straight)
}

/// The `color` of each class in `inks`, as the style sheet that gives it.
fn style_sheet(inks: &[(&str, [u8; 3])]) -> Result<String, Refused> {
    inks.iter()
        .map(|(name, [red, green, blue])| {
            let is_class = !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
            is_class
                .then(|| format!(".{name} {{ color: #{red:02x}{green:02x}{blue:02x} }}\n"))
                .ok_or(Refused::NotAnImage)
        })
        .collect()
}

/// `DejaVu Sans` 2.37, the one font any text is drawn in. Its licence is beside
/// it in `fonts/`.
const DEJAVU_SANS: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");

/// The family name [`DEJAVU_SANS`] carries, which every generic family and the
/// default are pointed at.
const FONT_FAMILY: &str = "DejaVu Sans";

/// The font database: [`DEJAVU_SANS`] and nothing from the system, loaded on
/// the first drawing.
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = fontdb::Database::new();
            database.load_font_source(fontdb::Source::Binary(Arc::new(DEJAVU_SANS)));
            database.set_serif_family(FONT_FAMILY);
            database.set_sans_serif_family(FONT_FAMILY);
            database.set_cursive_family(FONT_FAMILY);
            database.set_fantasy_family(FONT_FAMILY);
            database.set_monospace_family(FONT_FAMILY);
            Arc::new(database)
        })
        .clone()
}

/// How long an entry point waits for the decode slot. The work behind it is
/// bounded by the caps, so this is only the time a queue of pictures may take.
const WAIT: Duration = Duration::from_secs(10);

/// The decoders' own ceilings: what they may allocate, and the sides they may
/// be told of, so one that checks its limits stops before it allocates.
fn decoding_limits() -> Limits {
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(MOST_DECODED as u64);
    limits.max_image_width = Some(u32::try_from(MOST_DECODED / 4).unwrap_or(u32::MAX));
    limits.max_image_height = limits.max_image_width;
    limits
}

/// What a decoder's failure means here: a ceiling it kept, or bytes it could
/// not read.
fn refusal(error: &ImageError) -> Refused {
    match error {
        ImageError::Limits(_) => Refused::TooLarge,
        _ => Refused::NotAnImage,
    }
}

/// The decode slot: the one place in the process where a picture is decoded or
/// drawn, so the transient memory of a decode is paid once however many
/// pictures are waiting.
///
/// It is held by a thread, not by a value that moves: the guard is `!Send`,
/// and it frees the slot from `Drop`, including when its holder unwinds. A
/// thread that already holds it is let through again without waiting on
/// itself, and the guard it is given then frees nothing.
#[derive(Debug)]
pub struct Slot {
    /// Whether dropping this guard frees the slot, or only gives back a
    /// re-entry on the thread that holds it.
    frees: bool,
    _held_by_one_thread: PhantomData<*const ()>,
}

/// Whether some thread holds the slot, and the wake-up for those waiting.
static HELD: Mutex<bool> = Mutex::new(false);
static FREED: Condvar = Condvar::new();

thread_local! {
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

fn held() -> MutexGuard<'static, bool> {
    // The flag is a bool set and cleared under the lock, so a panic elsewhere
    // cannot leave it half-written.
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Slot {
    /// Take the slot, waiting up to `within` for whoever holds it.
    ///
    /// The wait is for the slot only. What a holder does with it is bounded by
    /// the caps on its input and its pixels, not by a deadline.
    ///
    /// # Errors
    /// [`Refused::OverTime`] when the slot is not free within `within`.
    pub fn take(within: Duration) -> Result<Slot, Refused> {
        if HOLDING.get() {
            return Ok(Slot {
                frees: false,
                _held_by_one_thread: PhantomData,
            });
        }
        let deadline = Instant::now().checked_add(within);
        let mut flag = held();
        while *flag {
            let left = match deadline {
                Some(at) => at.saturating_duration_since(Instant::now()),
                None => Duration::MAX,
            };
            if left.is_zero() {
                return Err(Refused::OverTime);
            }
            flag = FREED
                .wait_timeout(flag, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *flag = true;
        HOLDING.set(true);
        Ok(Slot {
            frees: true,
            _held_by_one_thread: PhantomData,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.frees {
            HOLDING.set(false);
            *held() = false;
            FREED.notify_one();
        }
    }
}

#[cfg(test)]
mod tests;
