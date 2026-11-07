//! Pictures and image files as bounded pixels, and as PNG and Sixel payloads.

use icy_sixel::{EncodeOptions, SixelImage};
use image::codecs::png::PngEncoder;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError, ImageReader, Limits,
};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::roxmltree::{self, Document, Node, ParsingOptions};
use resvg::usvg::{Error as UsvgError, ImageHrefResolver, Options, Tree, fontdb};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Cursor, Write};
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
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
/// A mask, clip path, pattern, filter, marker or copy that comes to name itself
/// through three elements or more is refused before the parser sees it: the
/// parser only looks for a loop two elements long, and follows a longer one
/// until its stack is gone, whatever size that stack is.
///
/// # Errors
/// [`Refused::TooLarge`] for an SVG past [`MOST_INPUT`] or a drawing past
/// [`MOST_DECODED`], [`Refused::NotAnImage`] for bytes that are not an SVG, a
/// zero width, a document whose references lead back to themselves or an ink
/// that cannot be a class, [`Refused::OverTime`] when the
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
            resolve_data: without_loops(ImageHrefResolver::default_data_resolver()),
            // The default reads the file an `href` names. Nothing here is
            // allowed to name one.
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    let _slot = Slot::take(WAIT)?;
    // The parser and the renderer recurse as deep as the document nests, and
    // the document may nest as deep as the XML parser allows. That is more
    // than the stack of whichever thread asked, so the drawing has its own.
    thread::scope(|scope| {
        thread::Builder::new()
            .stack_size(DRAWING_STACK)
            .spawn_scoped(scope, || draw(svg, width_px, &options))
            .map_err(|_| Refused::TooLarge)?
            .join()
            .unwrap_or_else(|unwound| std::panic::resume_unwind(unwound))
    })
}

/// The stack the drawing thread is given: room for the deepest nesting the XML
/// parser lets through, in a build with no optimisation, and twice over. Only
/// the pages a drawing touches are ever backed by memory.
const DRAWING_STACK: usize = 32 << 20;

/// [`rasterise`] once the slot is held and the drawing has the stack it needs.
fn draw(svg: &[u8], width_px: u32, options: &Options<'_>) -> Result<Pixels, Refused> {
    let document = parse(svg).ok_or(Refused::NotAnImage)?;
    if leads_back(&document) {
        return Err(Refused::NotAnImage);
    }
    let tree = Tree::from_xmltree(&document, options).map_err(|error| match error {
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

/// `svg` as the tree parser would read it: text, and XML with a doctype
/// allowed. A gzip stream is not text here, and the parser's own gzip reader is
/// not built.
fn parse(svg: &[u8]) -> Option<Document<'_>> {
    let text = std::str::from_utf8(svg).ok()?;
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, options).ok()
}

/// A data resolver that does not hand a nested SVG whose references lead back
/// to themselves to the tree parser. Such an image is left out, as one that
/// does not parse is.
fn without_loops(
    resolve: resvg::usvg::ImageHrefDataResolverFn<'static>,
) -> resvg::usvg::ImageHrefDataResolverFn<'static> {
    Box::new(move |mime, data, options| {
        let is_nested_svg = matches!(mime, "image/svg+xml" | "text/plain");
        if is_nested_svg && parse(&data).is_some_and(|document| leads_back(&document)) {
            return None;
        }
        resolve(mime, data, options)
    })
}

/// Whether some element of `document` comes to refer to itself: through an
/// attribute or a style sheet that names another element, or through the
/// elements it holds.
///
/// Every `url(#id)` in an attribute, every `href` that is a `#id` (an `a` link
/// is not drawn from), and every `url(#id)` in a `<style>` is a reference. A
/// style sheet cannot say which element a rule reaches, so one that names
/// anything but a gradient is taken to lead back: a gradient draws nothing
/// that is drawn from another. Where an id is given twice the parser uses the
/// last, and so does this.
fn leads_back(document: &Document<'_>) -> bool {
    let elements: Vec<Node<'_, '_>> = document.descendants().filter(Node::is_element).collect();
    let position: HashMap<roxmltree::NodeId, usize> = elements
        .iter()
        .enumerate()
        .map(|(at, element)| (element.id(), at))
        .collect();
    let named: HashMap<&str, usize> = elements
        .iter()
        .enumerate()
        .filter_map(|(at, element)| Some((element.attribute("id")?, at)))
        .collect();

    let is_gradient = |at: &usize| {
        elements.get(*at).is_some_and(|element| {
            matches!(
                element.tag_name().name(),
                "linearGradient" | "radialGradient"
            )
        })
    };
    let styled_by_a_sheet = elements
        .iter()
        .filter(|element| element.tag_name().name() == "style")
        .flat_map(|sheet| sheet.descendants().filter_map(|text| text.text()))
        .flat_map(url_targets)
        .filter_map(|target| named.get(target))
        .any(|at| !is_gradient(at));
    if styled_by_a_sheet {
        return true;
    }

    let edges: Vec<Vec<usize>> = elements
        .iter()
        .map(|element| {
            let held = element
                .children()
                .filter(Node::is_element)
                .filter_map(|child| position.get(&child.id()).copied());
            let named_by_attributes = element.attributes().flat_map(|attribute| {
                let link = (attribute.name() == "href" && element.tag_name().name() != "a")
                    .then(|| attribute.value().trim().strip_prefix('#'))
                    .flatten();
                url_targets(attribute.value()).into_iter().chain(link)
            });
            held.chain(named_by_attributes.filter_map(|target| named.get(target).copied()))
                .collect()
        })
        .collect();

    // Depth first, without recursing: a document can be a thousand deep. A
    // reference back to an element on the path being walked is a loop.
    let mut state = vec![Walk::New; elements.len()];
    for first in 0..elements.len() {
        if state.get(first) != Some(&Walk::New) {
            continue;
        }
        let mut path = vec![(first, 0_usize)];
        if let Some(entry) = state.get_mut(first) {
            *entry = Walk::OnThePath;
        }
        while let Some((element, next)) = path.last().copied() {
            let Some(&to) = edges.get(element).and_then(|edges| edges.get(next)) else {
                if let Some(entry) = state.get_mut(element) {
                    *entry = Walk::Done;
                }
                path.pop();
                continue;
            };
            if let Some((_, taken)) = path.last_mut() {
                *taken += 1;
            }
            match state.get(to).copied() {
                Some(Walk::OnThePath) => return true,
                Some(Walk::New) => {
                    if let Some(entry) = state.get_mut(to) {
                        *entry = Walk::OnThePath;
                    }
                    path.push((to, 0));
                }
                _ => {}
            }
        }
    }
    false
}

/// How far the walk of [`leads_back`] has come with an element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walk {
    New,
    OnThePath,
    Done,
}

/// The ids that `url(#id)` names in `value`, in a style attribute, a presentation
/// attribute or a style sheet. `url` is read in any case, and the id up to the
/// closing bracket, a quote or a space.
fn url_targets(value: &str) -> Vec<&str> {
    // Lower-casing ASCII keeps every byte where it was.
    value
        .to_ascii_lowercase()
        .match_indices("url(")
        .filter_map(|(at, found)| {
            let rest = value.get(at + found.len()..)?;
            let rest =
                rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '\'' || c == '"');
            let rest = rest.strip_prefix('#')?;
            let end = rest
                .find(|c: char| c == ')' || c == '\'' || c == '"' || c.is_ascii_whitespace())
                .unwrap_or(rest.len());
            rest.get(..end)
        })
        .collect()
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

/// Encode pixels as a PNG file, which a terminal that draws PNG shows as it is.
///
/// # Errors
/// [`Refused::TooLarge`] when the file would be past [`MOST_PNG`] bytes; the
/// bytes already made are dropped, and none past the ceiling are kept.
pub fn png(pixels: &Pixels) -> Result<Vec<u8>, Refused> {
    png_within(pixels, MOST_PNG)
}

/// [`png`] with the ceiling given, so a test can sit on the byte.
fn png_within(pixels: &Pixels, most: usize) -> Result<Vec<u8>, Refused> {
    let mut out = Capped {
        kept: Vec::new(),
        most,
        refused: false,
    };
    PngEncoder::new(&mut out)
        .write_image(
            pixels.rgba(),
            pixels.width(),
            pixels.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| Refused::TooLarge)?;
    // The encoder writes its last chunk from a destructor that drops the
    // error, so a refusal there leaves a short file and an `Ok`. The sink
    // remembers it has refused.
    if out.refused {
        return Err(Refused::TooLarge);
    }
    Ok(out.kept)
}

/// A sink that keeps what it is given up to a ceiling and fails the write that
/// would pass it, keeping none of that write.
struct Capped {
    kept: Vec<u8>,
    most: usize,
    refused: bool,
}

impl Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.kept.len().saturating_add(bytes.len()) > self.most {
            self.refused = true;
            return Err(io::Error::other("past the ceiling"));
        }
        self.kept.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// How a Sixel payload is made, which the terminal cannot say for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SixelPalette {
    /// The colour transparent and half-transparent pixels are laid over: Sixel
    /// has no alpha worth the name, so what shows through is decided here, and
    /// it should be the terminal's own background.
    pub ground: [u8; 3],
    /// The most colours the payload defines, from 2 to 256; a number outside
    /// that is taken as the nearest end of it.
    pub colours: u16,
}

/// Encode pixels as a Sixel payload, `ESC P … ESC \`, for a terminal that
/// draws Sixel.
///
/// Refused wider than [`MOST_SIXEL_WIDTH`]: quantising grows steeply with the
/// area, and a picture that wide already fills the screens this is for.
///
/// # Errors
/// [`Refused::TooLarge`] for a picture wider than the ceiling,
/// [`Refused::NotAnImage`] when the encoder cannot make a payload of it.
pub fn sixel(pixels: &Pixels, palette: &SixelPalette) -> Result<Vec<u8>, Refused> {
    if pixels.width() > MOST_SIXEL_WIDTH {
        return Err(Refused::TooLarge);
    }
    let [red, green, blue] = palette.ground;
    let laid_over: Vec<u8> = pixels
        .rgba()
        .chunks_exact(4)
        .flat_map(|pixel| {
            let [r, g, b, alpha] = *pixel.first_chunk::<4>().unwrap_or(&[0; 4]);
            let over = |top: u8, under: u8| {
                let (top, under, alpha) = (u16::from(top), u16::from(under), u16::from(alpha));
                u8::try_from((top * alpha + under * (255 - alpha) + 127) / 255).unwrap_or(u8::MAX)
            };
            [over(r, red), over(g, green), over(b, blue), 255]
        })
        .collect();
    let width = usize::try_from(pixels.width()).map_err(|_| Refused::TooLarge)?;
    let height = usize::try_from(pixels.height()).map_err(|_| Refused::TooLarge)?;
    let options = EncodeOptions {
        max_colors: palette.colours.clamp(2, 256),
        ..EncodeOptions::default()
    };
    SixelImage::try_from_rgba(laid_over, width, height)
        .and_then(|image| image.encode_with(&options))
        .map(String::into_bytes)
        .map_err(|_| Refused::NotAnImage)
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
