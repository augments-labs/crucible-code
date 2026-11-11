//! Pictures and image files as bounded pixels, and as PNG and Sixel payloads.

mod admitted;
mod budget;

use admitted::Outlines;
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
use std::sync::atomic::{AtomicBool, Ordering};
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

/// The most bytes of decoded pixels an entry point returns, and the most a
/// decode or a drawing may hold at once while making them: 2 MiB, which is
/// 524 288 RGBA pixels.
///
/// For an image file that is what the decoders allocate. For a picture it is
/// the sum of every pixmap the renderer would hold at the same time: the
/// canvas, the layer of each group drawn apart from its parent, the result of
/// each filter primitive, and each clip, mask and pattern. A picture whose sum
/// is past it is refused before it is drawn.
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
/// Only the elements a picture is drawn with are let in: the root, groups,
/// definitions, symbols and uses; paths, rectangles, circles, ellipses, lines,
/// polylines and polygons; text and its spans; markers; linear and radial
/// gradients and their stops; clip paths; filters, their steps and what those
/// steps hold; and a title, a description and metadata. A picture holding any
/// other element, or an element in a namespace other than SVG's, is refused
/// before the parser sees it: a `<style>` sheet, a `<mask>`, a `<pattern>`, an
/// `<image>`, a `<foreignObject>`, a `<switch>`, a link, a `<tref>` and a
/// `<textPath>` among them.
///
/// A reference is let in only where a picture is drawn with one: an `href` on
/// a `<use>` that names an element of the picture by `#id`, and on a filter's
/// image step that names anything but an element. A gradient that takes its
/// stops from another, an image step that draws an element and an `href` on
/// any other element refuse the picture before the parser sees it, as do an
/// id given to two elements or given in a namespace, and a number of the
/// picture's geometry (a length, a coordinate, a radius, a number of path
/// data, a view box, a factor of a transform, a width of a stroke or a font
/// size) past 100 000 user units: the parser cuts an arc into curves by its
/// radius, with no bound on how many. A `width`, `height`, `x` or `y` in a
/// percentage, `em` or `ex` on a `<svg>` inside the picture, a `<symbol>` or a
/// `<use>` refuses it too: each takes the view inside it from the view it sits
/// in, so a chain of them multiplies a size without bound. The outermost
/// `<svg>` keeps its relative sizes, and a percentage inside the picture is
/// taken of the largest view they, a view box or a `<use>` of a `<symbol>`
/// set.
///
/// Nothing outside the bytes is read, and the only font is the one this crate
/// carries. A filter's image step that names a `data:` URL holding an SVG
/// refuses the picture, since that SVG is not judged as this one is; one
/// holding a raster image, or naming a file or a URL, is drawn as nothing. The
/// height follows the SVG's own proportions, rounded up.
///
/// A clip path, filter, marker or copy that comes to name itself through three
/// elements or more is refused before the parser sees it: the parser only
/// looks for a loop two elements long, and follows a longer one until its
/// stack is gone, whatever size that stack is. So is a marker drawn inside a
/// marker, and a clip or a filter inside a clip: the parser makes the inner
/// one again for every place the outer one is drawn.
///
/// The copies the parser would make are counted before it makes any of them:
/// what each `<use>` copies, and the copies within those; a marker's content
/// at every vertex it is drawn on; a clip or a filter sized to each shape it is
/// on; the stops of a gradient in the units of a shape's bounding box, for
/// every shape it paints; the letters of a text copied where it is not drawn;
/// and every rule of the inks matched against every copy. The outline of every
/// shape drawn, once or in a copy, is counted with them: the curves a circle,
/// an ellipse, a rounded rectangle or an arc is cut into, more the larger it
/// is, and for a stroked shape the outline of its stroke by its size. Copies
/// that would take it more than 16 ms to make are refused, however few bytes
/// ask for them.
///
/// What the drawing would hold and compute is counted before it is drawn: a
/// drawing that would hold more than [`MOST_DECODED`] bytes of pixmaps at once,
/// or ask for more work than the renderer does in 20 ms, is refused, and so is
/// a stroke whose outline reaches more than a million pixels from the origin,
/// which the renderer outlines at a quarter pixel however long it is.
///
/// # Errors
/// [`Refused::TooLarge`] for an SVG past [`MOST_INPUT`], copies and outlines
/// past what the parser may make, a marker inside a marker, a clip or a filter inside a clip,
/// a drawing past [`MOST_DECODED`] or past the work it may ask for, or a stroke
/// reaching too far; [`Refused::NotAnImage`] for bytes that are not an SVG, a
/// zero width, an element outside those drawn, a reference where none is let
/// in, an id given twice or in a namespace, a number past 100 000, a relative
/// size on a view inside the picture, an SVG inside a filter's image step, a
/// drawing the renderer panics on, a document whose references lead back to
/// themselves or an ink that cannot be a class; [`Refused::OverTime`] when the
/// decode slot stays busy.
pub fn rasterise(svg: &[u8], width_px: u32, inks: &[(&str, [u8; 3])]) -> Result<Pixels, Refused> {
    if svg.len() > MOST_INPUT {
        return Err(Refused::TooLarge);
    }
    if width_px == 0 {
        return Err(Refused::NotAnImage);
    }
    // Set when a filter's image step holds an SVG, which the parser would
    // build with none of the checks this one is held to.
    let embedded = AtomicBool::new(false);
    let options = Options {
        style_sheet: Some(style_sheet(inks)?),
        font_family: FONT_FAMILY.to_owned(),
        fontdb: fonts(),
        image_href_resolver: ImageHrefResolver {
            // No image is drawn: the renderer is built without raster images,
            // and an SVG refuses the picture once the tree is built.
            resolve_data: Box::new(|mime, data, _| {
                if matches!(mime, "image/svg+xml" | "text/plain") && parse(&data).is_some() {
                    embedded.store(true, Ordering::Relaxed);
                }
                None
            }),
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
            .name("picture".to_owned())
            .stack_size(DRAWING_STACK)
            .spawn_scoped(scope, || {
                draw(svg, width_px, inks.len(), &options, &embedded)
            })
            .map_err(|_| Refused::TooLarge)?
            .join()
            // A renderer that panics on a picture has not drawn it, and the
            // panic ends with the drawing thread. The process's panic hook
            // has still reported it.
            .unwrap_or(Err(Refused::NotAnImage))
    })
}

/// The stack the drawing thread is given: room for the deepest nesting the XML
/// parser lets through, in a build with no optimisation, and twice over. Only
/// the pages a drawing touches are ever backed by memory.
const DRAWING_STACK: usize = 32 << 20;

/// How many times [`draw`] has gone on to build a tree, and to draw one: what
/// the tests read to tell a picture refused before that work from one refused
/// after it, which no clock can tell on every machine.
#[cfg(test)]
static BUILT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static DRAWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// [`rasterise`] once the slot is held and the drawing has the stack it needs,
/// with `rules` rules in the inks' style sheet and `embedded` set by the image
/// resolver of `options` when it is handed an SVG.
fn draw(
    svg: &[u8],
    width_px: u32,
    rules: usize,
    options: &Options<'_>,
    embedded: &AtomicBool,
) -> Result<Pixels, Refused> {
    let document = parse(svg).ok_or(Refused::NotAnImage)?;
    // What a picture says is judged first: the elements, references, ids and
    // sizes of numbers the parser would follow without counting.
    let outlines = admitted::admitted(&document)?;
    if leads_back(&document) {
        return Err(Refused::NotAnImage);
    }
    // The tree parser makes every copy a `<use>`, a marker, a clip or a filter
    // asks for, and the copies within them, before anything else could be
    // judged.
    if copying(&document, &outlines, rules)? > MOST_COPYING_NS {
        return Err(Refused::TooLarge);
    }
    #[cfg(test)]
    BUILT.fetch_add(1, Ordering::Relaxed);
    let tree = Tree::from_xmltree(&document, options).map_err(|error| match error {
        UsvgError::ElementsLimitReached => Refused::TooLarge,
        _ => Refused::NotAnImage,
    })?;
    if embedded.load(Ordering::Relaxed) {
        return Err(Refused::NotAnImage);
    }
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
    #[allow(clippy::cast_precision_loss, reason = "a size under the ceiling")]
    let scale = Transform::from_scale(
        width_px as f32 / size.width(),
        height as f32 / size.height(),
    );
    // What the renderer would hold and compute is judged before it holds or
    // computes any of it: the canvas is not the only pixmap it makes.
    budget::cost(&tree, scale, width_px, height)?;
    #[cfg(test)]
    DRAWN.fetch_add(1, Ordering::Relaxed);
    let mut canvas = Pixmap::new(width_px, height).ok_or(Refused::TooLarge)?;
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

/// Whether some element of `document` comes to refer to itself: through an
/// attribute that names another element, or through the elements it holds.
///
/// Every `url(#id)` in an attribute and every `href` that is a `#id` is a
/// reference. It is asked of a picture [`admitted`](admitted::admitted) has
/// let through, so a style sheet has been refused, an `href` that names an
/// element is on a `<use>`, and no id is given to two elements: which of two
/// elements an id names, the first or the last, does not arise.
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

    let edges: Vec<Vec<usize>> = elements
        .iter()
        .map(|element| {
            let held = element
                .children()
                .filter(Node::is_element)
                .filter_map(|child| position.get(&child.id()).copied());
            let named_by_attributes = element.attributes().flat_map(|attribute| {
                let link = (attribute.name() == "href")
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

/// The most time the copies [`copying`] counts may take the tree parser to
/// make: 16 ms, so that with the rest of the tree a picture is built within
/// the 20 ms the dearest drawing takes.
const MOST_COPYING_NS: u64 = 16_000_000;

/// The time the tree parser takes to make a copy of one element, without what
/// it holds, whether or not the copy is drawn: 200 ns.
///
/// A line of 200 groups and two levels used of five levels of ten were copied
/// in the definitions, where nothing of them is drawn, at up to 180 ns a copy,
/// the dearest measured.
const COPY_MADE_NS: u64 = 200;

/// The time the tree parser takes besides, for a copy that is drawn: 650 ns.
///
/// Five levels of ten uses of a rectangle off the page, 222 224 copies drawn of
/// 469 130 made, took 281.6 ms to build, which is 580 ns a drawn copy past
/// [`COPY_MADE_NS`] and [`COPIED_ATTRIBUTE_BYTE_NS`], the dearest measured.
const COPY_DRAWN_NS: u64 = 650;

/// The time the tree parser takes for one byte of the attributes of a copy
/// that is drawn: 20 ns. Path data, among them, is read again for every copy.
///
/// Copies of a stroked path of 116 KB took up to 16.3 ns a byte of its
/// attributes, the dearest measured.
const COPIED_ATTRIBUTE_BYTE_NS: u64 = 20;

/// The time the tree parser takes for one byte of the text of a copy that is
/// drawn: 1.5 µs, since a copied text is shaped again.
///
/// A thousand letters were copied at up to 1.43 µs a byte, the dearest
/// measured.
const COPIED_LETTER_NS: u64 = 1_500;

/// The time the tree parser takes to match one rule of the inks' style sheet
/// against one copy, drawn or not: 7 ns.
///
/// A thousand rules were matched against ten thousand groups at 6.4 ns a
/// match, the dearest measured.
const MATCH_NS: u64 = 7;

/// The time the tree parser takes for one byte of the text of a copy that is
/// not drawn: 2 ns, since a text is gathered before it is known to be drawn.
///
/// Twenty thousand copies of a thousand letters in the definitions were made
/// at 1.41 ns a letter past [`COPY_MADE_NS`], the dearest measured.
const UNDRAWN_LETTER_NS: u64 = 2;

/// The time the tree parser takes to copy one stop of a gradient in the units
/// of a shape's bounding box, for each shape that gradient paints: 4 ns.
const STOP_CLONED_NS: u64 = 4;

/// The time the tree parser takes to make one curve of an arc, filled,
/// stroked or neither: 110 ns. A circle, an ellipse and a rectangle's rounded
/// corners are four arcs each, and the parser cuts an arc into more curves the
/// larger its radius, twelve for a circle of radius 100 000.
///
/// Twenty thousand circles and twenty thousand arcs of a path were built at
/// up to 101 ns a curve past those of radius 1, the dearest measured.
const CUBIC_MADE_NS: u64 = 110;

/// The time the tree parser takes besides for a shape it strokes: 2.4 µs.
///
/// Lines, rectangles and polygons stroked with each join and each cap were
/// outlined at up to 2.38 µs a shape past the edges each has, with round caps
/// on a line, the dearest measured, at any size and any width of stroke.
const STROKED_SHAPE_NS: u64 = 2_400;

/// The time the tree parser takes to outline one straight edge of a stroke:
/// 1.4 µs.
///
/// A polyline of ten thousand edges with round joins, stroked from 10 to
/// 100 000 wide, was outlined at up to 1.38 µs an edge, the dearest measured.
const STRAIGHT_OUTLINE_NS: u64 = 1_400;

/// The time the tree parser takes to outline the stroke of an arc, by the
/// largest radius each price holds to: a circle, an ellipse, a rectangle with
/// rounded corners by its larger side, and an arc of a path, each measured at
/// every width of stroke from a millionth to 100 000 and the dearest kept.
///
/// The stroker halves a curve until its outline is within a quarter of a
/// unit, so the work grows with the curve's length; where the halving stops
/// turns on the width, by more than the length alone would say, so a price is
/// the dearest width at that size and not the width given.
const ARC_OUTLINE_NS: &[(f64, u64)] = &[
    (1.0, 25_600),
    (10.0, 28_200),
    (100.0, 31_000),
    (300.0, 49_500),
    (1_000.0, 181_100),
    (3_000.0, 252_100),
    (10_000.0, 597_000),
    (30_000.0, 1_079_200),
    (100_000.0, 2_594_400),
    (200_000.0, 3_169_900),
];

/// The time the tree parser takes to outline the stroke of a curve of a path
/// that is not an arc, by the farthest its points reach from where it
/// starts, each price measured as [`ARC_OUTLINE_NS`] is.
const CURVE_OUTLINE_NS: &[(f64, u64)] = &[
    (1.0, 11_500),
    (10.0, 11_700),
    (100.0, 11_700),
    (300.0, 18_100),
    (1_000.0, 28_500),
    (3_000.0, 45_200),
    (10_000.0, 97_500),
    (30_000.0, 154_400),
    (100_000.0, 713_100),
    (200_000.0, 1_125_800),
];

/// The namespace of SVG. The parser reads an element in it or in none, and
/// passes over one in any other with all it holds.
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// The elements a picture is drawn with: what holds and names the rest,
/// shapes, text, markers, gradients, clip paths, and filters with their steps
/// and what those steps hold. Any other element refuses the picture, a style
/// sheet, a mask, a pattern, an image, a `foreignObject`, a `switch`, a link,
/// a `tref` and a `textPath` among them: each is drawn by a part of the parser
/// or the renderer that nothing here judges.
const DRAWN_ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "title",
    "desc",
    "metadata",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "marker",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "filter",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDropShadow",
    "feFlood",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMorphology",
    "feOffset",
    "feSpecularLighting",
    "feTile",
    "feTurbulence",
    "feMergeNode",
    "feFuncR",
    "feFuncG",
    "feFuncB",
    "feFuncA",
    "feDistantLight",
    "fePointLight",
    "feSpotLight",
];

/// The kinds of marker a shape is given: at its start, at every vertex
/// between, at its end, and, where a `marker` shorthand or a style attribute
/// gives it, at any of those.
const MARKER_KINDS: usize = 4;

/// The elements of a document in document order, each after its parent, and
/// the elements each one names.
struct Elements<'a, 'input> {
    all: Vec<Node<'a, 'input>>,
    position: HashMap<roxmltree::NodeId, usize>,
    /// Each id and the first element given it, which a `<use>` names.
    first: HashMap<&'a str, usize>,
    /// Each id and the last element given it, which a `url(#id)` names.
    last: HashMap<&'a str, usize>,
}

impl<'a, 'input> Elements<'a, 'input> {
    fn of(document: &'a Document<'input>) -> Self {
        let all: Vec<Node<'a, 'input>> = document.descendants().filter(Node::is_element).collect();
        let position = all
            .iter()
            .enumerate()
            .map(|(at, element)| (element.id(), at))
            .collect();
        let mut first = HashMap::new();
        let mut last = HashMap::new();
        for (at, element) in all.iter().enumerate() {
            if let Some(id) = element.attribute("id") {
                first.entry(id).or_insert(at);
                last.insert(id, at);
            }
        }
        Self {
            all,
            position,
            first,
            last,
        }
    }

    fn name(&self, at: usize) -> &'a str {
        self.all
            .get(at)
            .map_or("", |element| element.tag_name().name())
    }

    fn parent(&self, at: usize) -> Option<usize> {
        let parent = self.all.get(at)?.parent_element()?;
        self.position.get(&parent.id()).copied()
    }

    fn children(&self, at: usize) -> impl Iterator<Item = usize> + '_ {
        self.all
            .get(at)
            .into_iter()
            .flat_map(Node::children)
            .filter(Node::is_element)
            .filter_map(|child| self.position.get(&child.id()).copied())
    }

    /// The element the `<use>` at `at` copies.
    fn copied(&self, at: usize) -> Option<usize> {
        let element = self.all.get(at)?;
        if element.tag_name().name() != "use" {
            return None;
        }
        used(element).and_then(|id| self.first.get(id).copied())
    }

    /// What each element takes what it inherits from: what holds it, and
    /// every `<use>` that copies it, whose copy holds it.
    fn parents(&self) -> Vec<Vec<usize>> {
        let mut parents: Vec<Vec<usize>> = (0..self.all.len())
            .map(|at| self.parent(at).into_iter().collect())
            .collect();
        for at in 0..self.all.len() {
            if let Some(copied) = self.copied(at)
                && let Some(entry) = parents.get_mut(copied)
            {
                entry.push(at);
            }
        }
        parents
    }

    /// Every element after everything it inherits from, as [`Self::parents`]
    /// gives it, or `None` where an element comes to hold or copy itself.
    fn in_order(&self) -> Option<Vec<usize>> {
        let parents = self.parents();
        let mut waiting: Vec<usize> = parents.iter().map(Vec::len).collect();
        let mut heirs: Vec<Vec<usize>> = vec![Vec::new(); self.all.len()];
        for (at, from) in parents.iter().enumerate() {
            for &parent in from {
                if let Some(entry) = heirs.get_mut(parent) {
                    entry.push(at);
                }
            }
        }
        let mut ready: Vec<usize> = (0..self.all.len())
            .filter(|&at| waiting.get(at) == Some(&0))
            .collect();
        let mut order = Vec::with_capacity(self.all.len());
        while let Some(at) = ready.pop() {
            order.push(at);
            for &heir in heirs.get(at).map_or(&[][..], Vec::as_slice) {
                if let Some(entry) = waiting.get_mut(heir) {
                    *entry = entry.saturating_sub(1);
                    if *entry == 0 {
                        ready.push(heir);
                    }
                }
            }
        }
        (order.len() == self.all.len()).then_some(order)
    }

    /// The elements the attribute `name` of the element at `at` names by
    /// `url(#id)`, or every attribute where `name` is `None`.
    fn linked(&self, at: usize, name: Option<&str>) -> Vec<usize> {
        self.all
            .get(at)
            .into_iter()
            .flat_map(Node::attributes)
            .filter(|attribute| name.is_none_or(|name| attribute.name() == name))
            .flat_map(|attribute| url_targets(attribute.value()))
            .filter_map(|id| self.last.get(id).copied())
            .collect()
    }

    /// Every element the element at `at` holds or names: by holding it, by
    /// copying it, by `url(#id)` and by an `href` that is a `#id`, taken as
    /// either element given that id.
    fn reached_from(&self, at: usize) -> Vec<usize> {
        let hrefs = self
            .all
            .get(at)
            .into_iter()
            .flat_map(Node::attributes)
            .filter(|attribute| attribute.name() == "href")
            .filter_map(|attribute| attribute.value().trim().strip_prefix('#'))
            .flat_map(|id| [self.first.get(id).copied(), self.last.get(id).copied()])
            .flatten();
        self.children(at)
            .chain(self.copied(at))
            .chain(self.linked(at, None))
            .chain(hrefs)
            .collect()
    }

    /// The markers the element at `at` gives itself and what it holds, by
    /// kind: `marker-start`, `marker-mid`, `marker-end`, and the `marker`
    /// shorthand or a style attribute that names a marker property.
    fn markers_given(&self, at: usize) -> [Vec<usize>; MARKER_KINDS] {
        let mut given: [Vec<usize>; MARKER_KINDS] = Default::default();
        let attributes = self.all.get(at).into_iter().flat_map(Node::attributes);
        for attribute in attributes {
            let kind = match attribute.name() {
                "marker-start" => 0,
                "marker-mid" => 1,
                "marker-end" => 2,
                "marker" => 3,
                "style" if attribute.value().to_ascii_lowercase().contains("marker") => 3,
                _ => continue,
            };
            let markers = url_targets(attribute.value())
                .into_iter()
                .filter_map(|id| self.last.get(id).copied())
                .filter(|&to| self.name(to) == "marker");
            if let Some(list) = given.get_mut(kind) {
                list.extend(markers);
            }
        }
        given
    }
}

/// Whether `element` is one of [`DRAWN_ELEMENTS`], in SVG's namespace or in
/// none.
fn is_drawn_element(element: &Node<'_, '_>) -> bool {
    let name = element.tag_name();
    name.namespace()
        .is_none_or(|namespace| namespace == SVG_NAMESPACE)
        && DRAWN_ELEMENTS.contains(&name.name())
}

/// The time the tree parser would take making the copies that `document` asks
/// for, a copy within a copy counted as often as it is made, by
/// [`COPY_MADE_NS`], [`COPY_DRAWN_NS`], [`COPIED_ATTRIBUTE_BYTE_NS`],
/// [`COPIED_LETTER_NS`] and [`UNDRAWN_LETTER_NS`], by [`MATCH_NS`] for each
/// of `rules` rules matched against each copy, by [`STOP_CLONED_NS`] for
/// each stop copied to paint a shape, and by `outlines` for each shape drawn,
/// once or in a copy.
///
/// Copies are what a `<use>` makes of the element it names; what a marker
/// holds, made again at every place it is drawn on a shape; a clip path or a
/// filter sized to the shape it is on, made again for every shape that names
/// it; and the stops of a gradient in the units of a shape's box, made again
/// for every shape drawn with it, the shapes drawn once among them. A use
/// names its element as the parser reads the name: an `href` with no
/// namespace before an `xlink:href`, spaces before the `#` and the id up to
/// the first space. A use in the definitions, or one that names what
/// nothing draws, is copied all the same, and so counted. A copy is taken to
/// be drawn unless it is in the definitions and outside every clip, marker
/// and filter there, which are drawn wherever they are named from. A shape is
/// taken to be given each marker that it or what holds it gives, every marker
/// any `<use>` is given, and a marker between its ends at every vertex it may
/// have.
///
/// # Errors
/// [`Refused::TooLarge`] for a marker drawn inside a marker, a clip or a
/// filter inside a clip, and a copy that would come to hold itself.
fn copying(document: &Document<'_>, outlines: &Outlines, rules: usize) -> Result<u64, Refused> {
    let elements = Elements::of(document);
    let count = elements.all.len();
    let given: Vec<[Vec<usize>; MARKER_KINDS]> =
        (0..count).map(|at| elements.markers_given(at)).collect();
    // The element whose markers of each kind each element is drawn with: its
    // own where it gives some, else those of what holds it.
    let mut source: Vec<[Option<usize>; MARKER_KINDS]> = vec![[None; MARKER_KINDS]; count];
    for at in 0..count {
        let inherited = elements
            .parent(at)
            .and_then(|parent| source.get(parent).copied())
            .unwrap_or([None; MARKER_KINDS]);
        let own = given.get(at);
        if let Some(entry) = source.get_mut(at) {
            *entry = std::array::from_fn(|kind| {
                let has_own = own
                    .and_then(|own| own.get(kind))
                    .is_some_and(|markers| !markers.is_empty());
                if has_own {
                    Some(at)
                } else {
                    inherited.get(kind).copied().flatten()
                }
            });
        }
    }
    let markers_of = |at: usize, kind: usize| -> &[usize] {
        source
            .get(at)
            .and_then(|sources| sources.get(kind).copied().flatten())
            .and_then(|from| given.get(from))
            .and_then(|given| given.get(kind))
            .map_or(&[], Vec::as_slice)
    };
    if nested(&elements, |at| {
        (0..MARKER_KINDS).any(|kind| !markers_of(at, kind).is_empty())
    }) {
        return Err(Refused::TooLarge);
    }

    // A shape a use copies is drawn with the markers the use is given, which
    // are not those of what holds the shape. One node of each kind stands for
    // every marker any use is given, after the elements.
    let mut by_uses: [Vec<usize>; MARKER_KINDS] = Default::default();
    for at in (0..count).filter(|&at| elements.name(at) == "use") {
        for (kind, markers) in by_uses.iter_mut().enumerate() {
            markers.extend_from_slice(markers_of(at, kind));
        }
    }
    for markers in &mut by_uses {
        markers.sort_unstable();
        markers.dedup();
    }
    let given_by_uses = |kind: usize| count + kind;

    // State `2 * node + drawn` is a node, drawn or not. Each edge is a state
    // made from this one and the times it is made.
    let states = count.saturating_add(MARKER_KINDS).saturating_mul(2);
    let mut edges: Vec<Vec<(usize, u64)>> = vec![Vec::new(); states];
    let state = |node: usize, drawn: bool| 2 * node + usize::from(drawn);
    for at in 0..count {
        let Some(element) = elements.all.get(at) else {
            continue;
        };
        let reaching = vertices(element);
        for drawn in [false, true] {
            let held = holds_drawn(element, drawn);
            let mut made: Vec<(usize, u64)> = elements
                .children(at)
                .map(|child| (state(child, held), 1))
                .chain(elements.copied(at).map(|copied| (state(copied, drawn), 1)))
                .collect();
            if drawn {
                if let Some(vertices) = reaching {
                    for kind in 0..MARKER_KINDS {
                        let places = match kind {
                            1 => vertices,
                            3 => vertices.saturating_add(2),
                            _ => 1,
                        };
                        made.extend(
                            markers_of(at, kind)
                                .iter()
                                .map(|&marker| (state(marker, true), places)),
                        );
                        if by_uses.get(kind).is_some_and(|markers| !markers.is_empty()) {
                            made.push((state(given_by_uses(kind), true), places));
                        }
                    }
                }
                made.extend(
                    elements
                        .linked(at, None)
                        .into_iter()
                        .filter(|&to| elements.all.get(to).is_some_and(sized_to_shape))
                        .map(|to| (state(to, true), 1)),
                );
            }
            if let Some(entry) = edges.get_mut(state(at, drawn)) {
                *entry = made;
            }
        }
    }
    for (kind, markers) in by_uses.iter().enumerate() {
        let made: Vec<(usize, u64)> = markers
            .iter()
            .map(|&marker| (state(marker, true), 1))
            .collect();
        for drawn in [false, true] {
            if let Some(entry) = edges.get_mut(state(given_by_uses(kind), drawn)) {
                entry.clone_from(&made);
            }
        }
    }
    let stops = cloned_stops(&elements);
    let alone = |state: usize| {
        let (at, drawn) = (state / 2, state % 2 == 1);
        let cloned = if drawn {
            let stops = stops
                .get(at)
                .map_or(0, |&stops| stops.saturating_mul(STOP_CLONED_NS));
            stops.saturating_add(outlines.of(at))
        } else {
            0
        };
        elements.all.get(at).map_or(0, |element| {
            made_alone(element, drawn, rules).saturating_add(cloned)
        })
    };

    // Each element once, where the document puts it, and not the stops it
    // is painted with or the outline it is built with, which are made for it
    // all the same. Every element comes after its parent.
    let mut drawn_once = vec![true; count];
    for at in 0..count {
        if let Some(parent) = elements.parent(at) {
            let drawn = drawn_once.get(parent).copied().unwrap_or(true);
            let held = elements
                .all
                .get(parent)
                .is_none_or(|parent| holds_drawn(parent, drawn));
            if let Some(entry) = drawn_once.get_mut(at) {
                *entry = held;
            }
        }
    }
    let once = drawn_once
        .iter()
        .enumerate()
        .map(|(at, &drawn)| {
            elements
                .all
                .get(at)
                .map_or(0, |element| made_alone(element, drawn, rules))
        })
        .fold(0, u64::saturating_add);

    // What building each state takes, with all it makes. Depth first, without
    // recursing, each state once: a copy of a copy is counted by adding what
    // its state was found to take, as many times as it is made.
    let mut built: Vec<Option<u64>> = vec![None; states];
    let mut on_the_path = vec![false; states];
    let next = |from: usize, taken: usize| {
        edges
            .get(from)
            .and_then(|edges| edges.get(taken))
            .map(|&(to, _)| to)
    };
    let first = state(0, true);
    let mut path = vec![(first, 0_usize)];
    if let Some(entry) = on_the_path.get_mut(first) {
        *entry = true;
    }
    while let Some((from, taken)) = path.last().copied() {
        let Some(to) = next(from, taken) else {
            let held = edges
                .get(from)
                .into_iter()
                .flatten()
                .map(|&(to, times)| {
                    built
                        .get(to)
                        .copied()
                        .flatten()
                        .unwrap_or(0)
                        .saturating_mul(times)
                })
                .fold(0, u64::saturating_add);
            if let Some(entry) = built.get_mut(from) {
                *entry = Some(alone(from).saturating_add(held));
            }
            if let Some(entry) = on_the_path.get_mut(from) {
                *entry = false;
            }
            path.pop();
            continue;
        };
        if let Some((_, taken)) = path.last_mut() {
            *taken += 1;
        }
        if on_the_path.get(to).copied().unwrap_or(false) {
            return Err(Refused::TooLarge);
        }
        if built.get(to).copied().flatten().is_none() {
            if let Some(entry) = on_the_path.get_mut(to) {
                *entry = true;
            }
            path.push((to, 0));
        }
    }
    // The document itself is built once; all past it is copies.
    Ok(built
        .get(first)
        .copied()
        .flatten()
        .unwrap_or(0)
        .saturating_sub(once))
}

/// The stops the tree parser copies to paint each element: for a shape or
/// text, those of each gradient in the units of its box that its fill and its
/// stroke name, and none for anything else. A fill or stroke an element does
/// not give, or gives as `inherit`, is that of what holds or copies it, the
/// one with the most stops where there are several.
fn cloned_stops(elements: &Elements<'_, '_>) -> Vec<u64> {
    const PAINTS: [&str; 2] = ["fill", "stroke"];
    let count = elements.all.len();
    // The stops each element holds when it is a gradient in the units of a
    // shape's box, counted once however many shapes name it.
    let held: Vec<u64> = elements
        .all
        .iter()
        .map(|element| {
            let in_box = matches!(
                element.tag_name().name(),
                "linearGradient" | "radialGradient"
            ) && element
                .attribute("gradientUnits")
                .is_none_or(|units| units.trim() != "userSpaceOnUse");
            if !in_box {
                return 0;
            }
            let stops = element
                .children()
                .filter(|child| child.is_element() && child.tag_name().name() == "stop")
                .count();
            u64::try_from(stops).unwrap_or(u64::MAX)
        })
        .collect();
    // The stops of the gradient `value` names, where it is in the units of a
    // shape's box; `None` for `inherit`.
    let stops_named = |value: &str| -> Option<u64> {
        if value.trim() == "inherit" {
            return None;
        }
        Some(
            url_targets(value)
                .first()
                .and_then(|id| elements.last.get(id).copied())
                .and_then(|at| held.get(at).copied())
                .unwrap_or(0),
        )
    };
    let given = |at: usize, paint: &str| -> Option<u64> {
        let element = elements.all.get(at)?;
        let declared = element
            .attribute("style")
            .into_iter()
            .flat_map(simplecss::DeclarationTokenizer::from)
            .filter(|declaration| declaration.name == paint)
            .map(|declaration| declaration.value)
            .last();
        stops_named(declared.or_else(|| element.attribute(paint))?)
    };
    let parents = elements.parents();
    let mut painted = vec![[0_u64; PAINTS.len()]; count];
    for at in elements.in_order().unwrap_or_default() {
        let above = |paint: usize| {
            parents
                .get(at)
                .into_iter()
                .flatten()
                .filter_map(|&parent| painted.get(parent)?.get(paint).copied())
                .max()
                .unwrap_or(0)
        };
        let own: [u64; PAINTS.len()] = std::array::from_fn(|paint| {
            PAINTS
                .get(paint)
                .and_then(|name| given(at, name))
                .unwrap_or_else(|| above(paint))
        });
        if let Some(entry) = painted.get_mut(at) {
            *entry = own;
        }
    }
    painted
        .iter()
        .enumerate()
        .map(|(at, stops)| {
            let shape = matches!(
                elements.name(at),
                "path"
                    | "rect"
                    | "circle"
                    | "ellipse"
                    | "line"
                    | "polyline"
                    | "polygon"
                    | "text"
                    | "tspan"
            );
            if shape {
                stops
                    .iter()
                    .fold(0_u64, |sum, &stops| sum.saturating_add(stops))
            } else {
                0
            }
        })
        .collect()
}

/// Whether a marker is drawn inside a marker, or a clip or a filter inside a
/// clip, where `marked` says whether an element is given a marker. The parser
/// makes the inner one again for every place the outer one is drawn, and
/// again within each of those, with no count that bounds it.
///
/// Inside is everything a marker or a clip path holds, and everything that
/// names: what it copies, and what it names by `url(#id)` or `href`. A clip
/// path's own `clip-path` is not inside it.
fn nested(elements: &Elements<'_, '_>, marked: impl Fn(usize) -> bool) -> bool {
    let inside = |outer: &str| {
        let mut seen = vec![false; elements.all.len()];
        let mut waiting: Vec<usize> = (0..elements.all.len())
            .filter(|&at| elements.name(at) == outer)
            .flat_map(|at| elements.children(at))
            .collect();
        while let Some(at) = waiting.pop() {
            if let Some(entry) = seen.get_mut(at).filter(|seen| !**seen) {
                *entry = true;
                waiting.extend(elements.reached_from(at));
            }
        }
        seen
    };
    let marker_in_marker = inside("marker").iter().enumerate().any(|(at, &inside)| {
        inside && (vertices_of(elements, at) || elements.name(at) == "use") && marked(at)
    });
    let clip_in_clip = inside("clipPath").iter().enumerate().any(|(at, &inside)| {
        inside
            && elements
                .linked(at, None)
                .into_iter()
                .any(|to| matches!(elements.name(to), "clipPath" | "filter"))
    });
    marker_in_marker || clip_in_clip
}

/// Whether the element at `at` is a shape a marker is drawn on.
fn vertices_of(elements: &Elements<'_, '_>, at: usize) -> bool {
    elements.all.get(at).and_then(vertices).is_some()
}

/// The most places a marker between the ends of `element` is drawn at, where
/// it is a shape a marker is drawn on: one for each vertex it may have.
///
/// A path has a vertex at most for each number and each command in its data,
/// and one more; a path with an arc, which is cut into curves by its size, is
/// taken to have one for each byte of its data. A polyline has one at most
/// for each number in its points, and a polygon two more, to close it. A line
/// has two, and a rectangle, a circle and an ellipse at most 32, as curves.
fn vertices(element: &Node<'_, '_>) -> Option<u64> {
    let numbers = |value: &str| {
        let mut runs = 0_usize;
        let mut in_one = false;
        for byte in value.bytes() {
            let digit = byte.is_ascii_digit();
            if digit && !in_one {
                runs += 1;
            }
            in_one = digit;
        }
        runs
    };
    let attribute = |name: &str| element.attribute(name).unwrap_or("");
    let places = match element.tag_name().name() {
        "path" => {
            let data = attribute("d");
            if data.contains(['A', 'a']) {
                data.len()
            } else {
                let commands = data.bytes().filter(u8::is_ascii_alphabetic).count();
                numbers(data).saturating_add(commands).saturating_add(1)
            }
        }
        "polyline" => numbers(attribute("points")).saturating_add(1),
        "polygon" => numbers(attribute("points")).saturating_add(2),
        "line" => 2,
        "rect" | "circle" | "ellipse" => 32,
        _ => return None,
    };
    Some(u64::try_from(places).unwrap_or(u64::MAX))
}

/// Whether a clip path or filter is made again for each shape that names it:
/// a clip path sized to the shape's box, and a filter whose region or steps
/// are, or that takes them from another filter.
fn sized_to_shape(element: &Node<'_, '_>) -> bool {
    let is = |name: &str, value: &str| {
        element
            .attribute(name)
            .is_some_and(|given| given.trim() == value)
    };
    match element.tag_name().name() {
        "clipPath" => is("clipPathUnits", "objectBoundingBox"),
        "filter" => {
            let inherits = element
                .attributes()
                .any(|attribute| attribute.name() == "href");
            inherits
                || !is("filterUnits", "userSpaceOnUse")
                || !is("primitiveUnits", "userSpaceOnUse")
        }
        _ => false,
    }
}

/// Whether what `element` holds is drawn, where `element` is drawn or not: not
/// in the definitions, and again in a clip, marker or filter.
fn holds_drawn(element: &Node<'_, '_>, drawn: bool) -> bool {
    match element.tag_name().name() {
        "defs" => false,
        "clipPath" | "marker" | "filter" => true,
        _ => drawn,
    }
}

/// What making `element` takes, without what it holds and with `rules` rules
/// matched against it: the copy, the text it holds, and where it is drawn its
/// attributes and that text shaped.
fn made_alone(element: &Node<'_, '_>, drawn: bool, rules: usize) -> u64 {
    let bytes = |count: usize| u64::try_from(count).unwrap_or(u64::MAX);
    let letters: usize = if matches!(element.tag_name().name(), "text" | "tspan") {
        element
            .children()
            .filter_map(|child| child.text().filter(|_| child.is_text()))
            .map(str::len)
            .sum()
    } else {
        0
    };
    let made = COPY_MADE_NS
        .saturating_add(bytes(letters).saturating_mul(UNDRAWN_LETTER_NS))
        .saturating_add(bytes(rules).saturating_mul(MATCH_NS));
    if !drawn {
        return made;
    }
    let attributes: usize = element
        .attributes()
        .map(|attribute| attribute.name().len() + attribute.value().len())
        .sum();
    made.saturating_add(COPY_DRAWN_NS)
        .saturating_add(bytes(attributes).saturating_mul(COPIED_ATTRIBUTE_BYTE_NS))
        .saturating_add(bytes(letters).saturating_mul(COPIED_LETTER_NS))
}

/// The id a `<use>` names, as the tree parser reads it.
fn used<'a>(element: &Node<'a, '_>) -> Option<&'a str> {
    const XLINK: &str = "http://www.w3.org/1999/xlink";
    let href = |namespace: Option<&str>| {
        element
            .attributes()
            .find(|attribute| attribute.name() == "href" && attribute.namespace() == namespace)
    };
    let value = href(None).or_else(|| href(Some(XLINK)))?.value();
    let id = value
        .trim_start_matches([' ', '\t', '\n', '\r'])
        .strip_prefix('#')?;
    id.split(' ').next().filter(|id| !id.is_empty())
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
/// bounded by the caps on its input and pixels and by the memory and
/// work a drawing is judged to need before it is drawn, so this is only the
/// time a queue of pictures may take.
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
    /// the caps on its input and its pixels and by the memory and work
    /// a drawing is judged to need, not by a deadline.
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
