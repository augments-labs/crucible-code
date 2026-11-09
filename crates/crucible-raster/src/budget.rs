//! What a drawing would hold and compute, judged before the renderer is asked
//! to draw it.
//!
//! The renderer allocates a pixmap for every group that is drawn apart from
//! its parent (opacity, filter, clip, mask, blend), for every result of a
//! filter, for every clip and mask, and for every pattern, and a filter's
//! pixmap can be as wide and tall as five canvases each way. None of that is
//! bounded by the size of the picture asked for, nor by the size of the file.
//! A filter primitive can also loop per pixel as many times as the file says.
//!
//! So the tree is walked the way the renderer walks it, and two sums are kept.
//! The first is the bytes of pixmaps held at once, taken at each allocation
//! with everything its ancestors still hold: where it is not known what the
//! renderer frees early, the pixmap is counted as held. It is refused past
//! [`MOST_DECODED`]. The second is the work: for each filter primitive the
//! pixels of its region times what it does with each pixel, and for each shape
//! the largest of four prices, all summed over the tree. They are what any
//! shape costs however little it covers; the pixels it reaches on the pixmap it
//! is drawn on, its fill and its stroke each on their own, a dashed stroke by
//! the dashes it is cut into instead of by the pixels it reaches, and every
//! stop of a gradient at every pixel the gradient paints added to them; every
//! segment of its outline by the rows of that pixmap it crosses, with every
//! stop of its gradients set up once for it; and every edge of its fill, and
//! of its stroke's outline, by the pixels along each of those rows it can
//! walk past. A shape in a clip is priced as the fill it is drawn as, on the
//! clip's own pixmap. The work is refused past [`MOST_WORK`], and a stroke
//! whose outline reaches past [`MOST_STROKED_REACH`] pixels from the origin is
//! refused however little work it is counted as: the renderer cannot outline
//! it as finely as it tries to. An image is never drawn: the parser is given
//! none.

use crate::{MOST_DECODED, Refused};
use resvg::tiny_skia::{IntRect, PathSegment, Point, Rect, StrokeDash, Transform};
use resvg::usvg::filter::{Filter, Kind};
use resvg::usvg::{ClipPath, Group, Mask, Node, Paint, Path, Stroke, Tree};

/// The time a step stands for, in nanoseconds: 140.
///
/// [`MOST_WORK`] steps were about 20 ms for the slowest primitive measured.
const STEP_NS: u64 = 140;

/// The most work a drawing may ask for: 140 000 pixel-steps.
///
/// A step is one pass over one pixel by the cheapest primitive; a primitive
/// that does more with each pixel (a turbulence per octave, a morphology per
/// window cell, a convolution per matrix cell, a merge per input) counts that
/// many, and a shape counts [`PIXELS_PER_STEP`] of the pixels it reaches as
/// one. The number is the largest total at which the slowest primitive
/// measured still drew within 20 ms in a release build: over 140 000 pixels a
/// drop shadow took 17 ms, a Gaussian blur 14 ms and a one-octave turbulence
/// 19 ms. A merge input is a copy of the region and took 2 ms at that total.
const MOST_WORK: u64 = 140_000;

/// How many pixels of a shape count as one step: 16.
///
/// A step is [`STEP_NS`], the time the slowest primitive took per step at the
/// ceiling. A shape is painted at between a fifth of a nanosecond a pixel, for
/// a plain colour, and seven, for a radial gradient of two stops, which is 20
/// pixels to a step. Sixteen counts the shapes a little above what they cost,
/// so a plain page-sized background does not use up a drawing's work. A
/// gradient of many stops is slower than any of these, and its stops are
/// counted on top of its pixels: [`PIXEL_STOPS_PER_STEP`].
const PIXELS_PER_STEP: u64 = 16;

/// The steps any shape costs however few pixels it reaches: 20.
///
/// The renderer builds and draws a shape that reaches no pixel, or only one:
/// a mebibyte of rectangles a pixel square, or of paths a unit long, took 2.4
/// microseconds a shape at the most, from the file read to the picture drawn,
/// which is 17 steps. Twenty is 2.8 microseconds.
const STEPS_PER_SHAPE: u64 = 20;

/// The time each segment of a filled outline costs, in nanoseconds: 300.
///
/// A line along one row took 137 ns and a cubic curve 285 ns, the dearest
/// measured; a move and the line that closes a contour are segments too.
const SEGMENT_NS: u64 = 300;

/// The time each segment of a stroked outline costs, in nanoseconds: 750.
///
/// The renderer outlines a stroke before it fills it: a line along one row,
/// stroked two pixels wide or more, took 739 ns.
const STROKED_SEGMENT_NS: u64 = 750;

/// The time each row of the pixmap a segment crosses costs, in nanoseconds:
/// 100.
///
/// The renderer walks every edge down every row it spans. A thousand edges
/// each crossing 5 000 rows took 97 ns an edge and a row, the dearest
/// measured; ten thousand crossing 100 rows, 31.
const ROW_NS: u64 = 100;

/// How many edge-rows of a fill, each times the pixels along its row it can
/// walk past, count as one step: 128.
///
/// The renderer adds every span of a row to a run-length row of coverage,
/// walking the runs from where the last span that covered a whole pixel ended,
/// and starts that walk over at each of the four rows it samples a pixel row
/// by. So when the edges of a fill are closer together than a pixel, every
/// span walks the row again, as far as it is wide or as the runs four times
/// the edges make. Thousands of edges across one row 500 to 9 300 pixels wide
/// took up to 0.65 ns an edge-row and a pixel so counted, the dearest
/// measured, which is 215 to a step.
const RUNS_PER_STEP: u64 = 128;

/// How many pixels and stops of a gradient count as one step: 512.
///
/// The renderer works out every stop at every pixel a gradient paints, on top
/// of painting the pixel: a linear gradient of 100 stops over 250 000 pixels
/// took 0.27 ns a pixel and a stop, the dearest measured, which is 527 to a
/// step. Linear gradients of 16 to 48 stops over two million pixels took 10
/// to 15 ns a pixel all told, which the pixels and the stops counted together
/// put at 13 to 22.
const PIXEL_STOPS_PER_STEP: u64 = 512;

/// The time the renderer takes to set up one stop of a gradient for a shape it
/// paints, in nanoseconds: 32.
///
/// The renderer makes the stops of a gradient again for each shape it fills
/// or strokes with it, before it knows whether any pixel is reached: a
/// thousand to twenty thousand stops on 200 to 2 000 rectangles a pixel square
/// took 29.7 to 31.7 ns a stop and a shape. A fill whose outline is no wider
/// or taller than a line is not made.
const STOP_SET_NS: u64 = 32;

/// How far from the origin of the pixmap the outline of a stroke may reach, in
/// pixels: a million.
///
/// The renderer outlines a stroke in the shape's own units, as finely as a
/// quarter of a pixel once transformed, and past a few million pixels a float
/// can no longer tell the points apart, so the outline is cut as finely as it
/// goes. Every stroke measured within a million pixels drew in half a
/// millisecond at most; past five million, from 400 to 700 ms.
const MOST_STROKED_REACH: f32 = 1e6;

/// The steps a dash costs however thin it is: 8.
///
/// Building and drawing a dash across a stroke no wider than a pixel took 2
/// steps at the most, whatever its length; 8 is four times that.
const STEPS_PER_DASH: u64 = 8;

/// The steps each pixel across a dash costs beyond the first, when that is
/// more than the pixels the dash covers are worth: 32.
///
/// The renderer works every row across a dash whatever its length: a dash
/// shorter than a pixel across a stroke 100 wide took 190 microseconds, which
/// is 14 steps for each of the 100. That is the dearest measured, and 32 is
/// more than twice it.
const STEPS_PER_DASH_ROW: u64 = 32;

/// What a drawing holds and does, as far as the walk can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cost {
    /// The most bytes of pixmaps held at once.
    pub(crate) bytes: u64,
    /// The work of every filter primitive and shape, in pixel-steps.
    pub(crate) work: u64,
}

/// What drawing `tree` onto a canvas `width` by `height` pixels, under `scale`,
/// would cost.
///
/// # Errors
/// [`Refused::TooLarge`] as soon as the bytes held pass [`MOST_DECODED`] or the
/// work passes [`MOST_WORK`].
pub(crate) fn cost(
    tree: &Tree,
    scale: Transform,
    width: u32,
    height: u32,
) -> Result<Cost, Refused> {
    let mut walk = Walk {
        cost: Cost { bytes: 0, work: 0 },
    };
    let canvas = bytes(width, height);
    walk.hold(canvas)?;
    let context = Context {
        most: most_layer(width, height)?,
        pixmap: (width, height),
    };
    walk.nodes(tree.root(), scale, &context, canvas)?;
    Ok(walk.cost)
}

/// The bytes of an RGBA pixmap, saturating.
fn bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
}

/// The largest layer the renderer gives a group drawn on a pixmap this size:
/// two pixmaps beyond each edge.
fn most_layer(width: u32, height: u32) -> Result<IntRect, Refused> {
    let reach = |side: u32| i32::try_from(side).unwrap_or(i32::MAX).saturating_mul(-2);
    IntRect::from_xywh(
        reach(width),
        reach(height),
        width.saturating_mul(5),
        height.saturating_mul(5),
    )
    .or_else(|| IntRect::from_ltrb(i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2))
    .ok_or(Refused::TooLarge)
}

/// `value` cast the way the renderer casts it: a float saturates.
#[allow(
    clippy::cast_possible_truncation,
    reason = "saturates, as the renderer's own cast does, and is judged against a ceiling"
)]
fn signed(value: f32) -> i32 {
    value as i32
}

/// `value` cast the way the renderer casts it: a float saturates.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturates, as the renderer's own cast does, and is judged against a ceiling"
)]
fn unsigned(value: f32) -> u32 {
    value as u32
}

/// `rect` brought within `bounds`, as the renderer does.
fn within(rect: IntRect, bounds: IntRect) -> Option<IntRect> {
    IntRect::from_ltrb(
        rect.left().max(bounds.left()),
        rect.top().max(bounds.top()),
        rect.right().min(bounds.right()),
        rect.bottom().min(bounds.bottom()),
    )
}

/// What the renderer is given besides the transform: the layers it may make,
/// and the size of the pixmap it is drawing on, which a nested picture copies.
struct Context {
    most: IntRect,
    pixmap: (u32, u32),
}

struct Walk {
    cost: Cost,
}

impl Walk {
    /// Bytes held at once, noted and judged.
    fn hold(&mut self, held: u64) -> Result<(), Refused> {
        self.cost.bytes = self.cost.bytes.max(held);
        if held > MOST_DECODED as u64 {
            return Err(Refused::TooLarge);
        }
        Ok(())
    }

    /// Work added and judged.
    fn work(&mut self, steps: u64) -> Result<(), Refused> {
        self.cost.work = self.cost.work.saturating_add(steps);
        if self.cost.work > MOST_WORK {
            return Err(Refused::TooLarge);
        }
        Ok(())
    }

    fn nodes(
        &mut self,
        parent: &Group,
        transform: Transform,
        context: &Context,
        held: u64,
    ) -> Result<(), Refused> {
        for node in parent.children() {
            match node {
                Node::Group(group) => self.group(group, transform, context, held)?,
                Node::Path(path) => self.path(path, transform, context, held)?,
                // An image is never built: the element is refused before the
                // tree is, and the resolver hands the parser nothing to draw.
                Node::Image(_) => {}
                Node::Text(text) => self.group(text.flattened(), transform, context, held)?,
            }
        }
        Ok(())
    }

    /// A group the renderer draws apart from its parent is a layer, and what
    /// the layer is given to do is done while it and everything above it is
    /// held.
    fn group(
        &mut self,
        group: &Group,
        transform: Transform,
        context: &Context,
        held: u64,
    ) -> Result<(), Refused> {
        let transform = transform.pre_concat(group.transform());
        if !group.should_isolate() {
            return self.nodes(group, transform, context, held);
        }
        // A filter is drawn upright and scaled afterwards, so the layer is
        // made in the space with the rotation taken out.
        let rest = if group.filters().is_empty() {
            Transform::identity()
        } else {
            let sx = transform.sx.hypot(transform.ky);
            let sy = transform.kx.hypot(transform.sy);
            Transform::from_row(
                transform.sx / sx,
                transform.ky / sx,
                transform.kx / sy,
                transform.sy / sy,
                0.0,
                0.0,
            )
        };
        let Some(inverse) = rest.invert() else {
            return Ok(());
        };
        let Some(most) = context
            .most
            .to_rect()
            .transform(inverse)
            .and_then(|rect| rect.round_out())
        else {
            return Ok(());
        };
        let transform = inverse.pre_concat(transform);
        let Some(bbox) = group.layer_bounding_box().transform(transform) else {
            return Ok(());
        };
        let grown = if group.filters().is_empty() {
            // Two pixels each side, so that an edge is not clipped.
            IntRect::from_xywh(
                signed(bbox.x().floor()).saturating_sub(2),
                signed(bbox.y().floor()).saturating_sub(2),
                unsigned(bbox.width().ceil()).saturating_add(4),
                unsigned(bbox.height().ceil()).saturating_add(4),
            )
        } else {
            IntRect::from_xywh(
                signed(bbox.x().floor()),
                signed(bbox.y().floor()),
                unsigned(bbox.width().ceil().max(1.0)),
                unsigned(bbox.height().ceil().max(1.0)),
            )
        };
        let Some(layer) = grown.and_then(|grown| within(grown, most)) else {
            return Ok(());
        };
        let size = (layer.width(), layer.height());
        let layer_bytes = bytes(size.0, size.1);
        let held = held.saturating_add(layer_bytes);
        self.hold(held)?;

        #[allow(
            clippy::cast_precision_loss,
            reason = "a position the layer was already judged at, and only an estimate is wanted"
        )]
        let inside = Transform::from_translate(-(layer.x() as f32), -(layer.y() as f32))
            .pre_concat(transform);
        let inner = Context { most, pixmap: size };
        self.nodes(group, inside, &inner, held)?;
        for filter in group.filters() {
            self.filter(filter, inside, &inner, held)?;
        }
        if let Some(clip) = group.clip_path() {
            self.clip(clip, inside, size, held)?;
        }
        if let Some(mask) = group.mask() {
            self.mask(mask, inside, &inner, layer_bytes, held)?;
        }
        Ok(())
    }

    /// A filter works on a copy of its region, keeps the result of every
    /// primitive, and has a few more pixmaps of that size in hand while it
    /// makes the next. Every result is as large as the region.
    fn filter(
        &mut self,
        filter: &Filter,
        transform: Transform,
        context: &Context,
        held: u64,
    ) -> Result<(), Refused> {
        let Some(source) = IntRect::from_xywh(0, 0, context.pixmap.0, context.pixmap.1) else {
            return Ok(());
        };
        let Some(region) = filter.rect().transform(transform).and_then(|rect| {
            IntRect::from_xywh(
                signed(rect.x().floor()),
                signed(rect.y().floor()),
                unsigned(rect.width().ceil()).max(1),
                unsigned(rect.height().ceil()).max(1),
            )
        }) else {
            return Ok(());
        };
        let Some(region) = within(region, source) else {
            return Ok(());
        };
        let pixels = u64::from(region.width()).saturating_mul(u64::from(region.height()));
        let primitives = u64::try_from(filter.primitives().len()).unwrap_or(u64::MAX);
        let held = held.saturating_add(
            bytes(region.width(), region.height()).saturating_mul(primitives.saturating_add(4)),
        );
        self.hold(held)?;
        for primitive in filter.primitives() {
            self.work(pixels.saturating_mul(steps(primitive.kind(), transform, region)))?;
            if let Kind::Image(image) = primitive.kind() {
                let nested = Context {
                    most: most_layer(region.width(), region.height())?,
                    pixmap: (region.width(), region.height()),
                };
                self.nodes(image.root(), transform, &nested, held)?;
            }
        }
        Ok(())
    }

    /// A clip is a pixmap of the layer's size, and a mask made from it. Its
    /// content is drawn on it under the clip's own transform, and a clip of
    /// the clip under the transform the clip was given. A clipped child of a
    /// clip is drawn on one of its own, and clipped in turn.
    fn clip(
        &mut self,
        clip: &ClipPath,
        transform: Transform,
        pixmap: (u32, u32),
        held: u64,
    ) -> Result<(), Refused> {
        let layer = bytes(pixmap.0, pixmap.1);
        let held = held.saturating_add(layer);
        self.hold(held.saturating_add(layer / 4))?;
        self.clipped(
            clip.root(),
            transform.pre_concat(clip.transform()),
            pixmap,
            held,
        )?;
        if let Some(inner) = clip.clip_path() {
            self.clip(inner, transform, pixmap, held)?;
        }
        Ok(())
    }

    /// The content of a clip: every path is filled, whatever paint the file
    /// gave it, and an image is not drawn.
    fn clipped(
        &mut self,
        parent: &Group,
        transform: Transform,
        pixmap: (u32, u32),
        held: u64,
    ) -> Result<(), Refused> {
        for node in parent.children() {
            match node {
                Node::Group(group) => {
                    let transform = transform.pre_concat(group.transform());
                    match group.clip_path() {
                        Some(clip) => {
                            let held = held.saturating_add(bytes(pixmap.0, pixmap.1));
                            self.hold(held)?;
                            self.clipped(group, transform, pixmap, held)?;
                            self.clip(clip, transform, pixmap, held)?;
                        }
                        None => self.clipped(group, transform, pixmap, held)?,
                    }
                }
                Node::Text(text) => {
                    self.clipped(text.flattened(), transform, pixmap, held)?;
                }
                Node::Path(path) => {
                    if path.is_visible() {
                        self.work(clipping(path, transform, pixmap))?;
                    }
                }
                Node::Image(_) => {}
            }
        }
        Ok(())
    }

    /// A mask is a pixmap of the layer's size, a coverage of the same area
    /// beside it, and what its own content and its own mask hold.
    #[allow(
        clippy::too_many_arguments,
        reason = "the walk's transform and context, the layer and what is already held: one call's state"
    )]
    fn mask(
        &mut self,
        mask: &Mask,
        transform: Transform,
        context: &Context,
        layer: u64,
        held: u64,
    ) -> Result<(), Refused> {
        if mask.root().children().is_empty() {
            return Ok(());
        }
        let held = held.saturating_add(layer);
        self.hold(held.saturating_add(layer / 4))?;
        self.nodes(
            mask.root(),
            transform,
            context,
            held.saturating_add(layer / 4),
        )?;
        if let Some(inner) = mask.mask() {
            self.mask(inner, transform, context, layer, held)?;
        }
        Ok(())
    }

    /// A pattern is drawn to a pixmap of its own before it paints.
    fn path(
        &mut self,
        path: &Path,
        transform: Transform,
        context: &Context,
        held: u64,
    ) -> Result<(), Refused> {
        if !path.is_visible() {
            return Ok(());
        }
        self.work(shape(path, transform, context.pixmap)?)?;
        let paints = path
            .fill()
            .map(resvg::usvg::Fill::paint)
            .into_iter()
            .chain(path.stroke().map(resvg::usvg::Stroke::paint));
        for paint in paints {
            let Paint::Pattern(pattern) = paint else {
                continue;
            };
            let (sx, sy) = transform.pre_concat(pattern.transform()).get_scale();
            let size = (
                unsigned((pattern.rect().width() * sx).round()),
                unsigned((pattern.rect().height() * sy).round()),
            );
            if size.0 == 0 || size.1 == 0 {
                continue;
            }
            let held = held.saturating_add(bytes(size.0, size.1));
            self.hold(held)?;
            let inner = Context {
                most: context.most,
                pixmap: size,
            };
            self.nodes(pattern.root(), Transform::from_scale(sx, sy), &inner, held)?;
        }
        Ok(())
    }
}

/// The steps drawing `path` costs on a pixmap `pixmap` wide and tall: the
/// largest of what any shape costs, the pixels it reaches with the stops of
/// its gradients at the pixels they paint, its segments by the rows they
/// cross, and the pixels its edges walk along those rows.
///
/// Painting is a pass over every pixel it reaches, the fill and the stroke
/// each on their own, and a dashed stroke is counted by its dashes instead.
/// The renderer builds the edges of the fill, and of the outline of the
/// stroke, from every segment of the path, and walks each down every row it
/// spans; a stroke spans half its width more each side. The edges of a fill,
/// and the two edges of the stroke's outline beside each segment, also walk
/// along each row they cross, as [`RUNS_PER_STEP`] says. A fill that reaches
/// no pixel builds no edge, and a stroke that reaches none is still outlined.
/// The stops of a gradient are set up for the fill and for the stroke, at
/// [`STOP_SET_NS`] each, whether or not a pixel is reached.
///
/// # Errors
/// [`Refused::TooLarge`] when the stroke is dashed and the length of the path
/// is not a number, and when the outline of the stroke reaches past
/// [`MOST_STROKED_REACH`] from the origin.
fn shape(path: &Path, transform: Transform, pixmap: (u32, u32)) -> Result<u64, Refused> {
    let canvas = IntRect::from_xywh(0, 0, pixmap.0, pixmap.1);
    let mut pixels = 0_u64;
    let mut nanos = 0_u64;
    let mut stops = 0_u64;
    let mut runs = 0_u64;
    if let Some(fill) = path.fill() {
        let area = reaches(path.bounding_box(), transform, canvas);
        let reach = area.map_or(0, pixels_of);
        pixels = pixels.saturating_add(reach.div_ceil(PIXELS_PER_STEP));
        stops = stops.saturating_add(reach.saturating_mul(gradient_stops(fill.paint())));
        if filled(path) {
            nanos = nanos.saturating_add(STOP_SET_NS.saturating_mul(gradient_stops(fill.paint())));
        }
        if let Some(area) = area.filter(|_| reach > 0 && filled(path)) {
            let walked = FILL.of(path.data(), transform, pixmap.1);
            nanos = nanos.saturating_add(walked.nanos);
            runs = walked.runs(area.width());
        }
    }
    if let Some(stroke) = path.stroke() {
        let outline = path.stroke_bounding_box().transform(transform);
        let near = outline.is_some_and(|outline| {
            [
                outline.left(),
                outline.top(),
                outline.right(),
                outline.bottom(),
            ]
            .into_iter()
            .all(|side| side.abs() <= MOST_STROKED_REACH)
        });
        if !near {
            return Err(Refused::TooLarge);
        }
        let area = reaches(path.stroke_bounding_box(), transform, canvas);
        let reach = area.map_or(0, pixels_of);
        pixels = pixels.saturating_add(match dashing(stroke) {
            Some(dash) => dashes(path, stroke, dash, transform, canvas)?,
            None => reach.div_ceil(PIXELS_PER_STEP),
        });
        stops = stops.saturating_add(reach.saturating_mul(gradient_stops(stroke.paint())));
        nanos = nanos.saturating_add(STOP_SET_NS.saturating_mul(gradient_stops(stroke.paint())));
        let (sx, sy) = transform.get_scale();
        let half = f64::from(stroke.width().get()) * f64::from(sx.max(sy)) / 2.0;
        let outline = Edges {
            each: STROKED_SEGMENT_NS,
            half,
            closes: false,
        };
        // Off the pixmap, the outline is made and no row is walked.
        let rows = if reach > 0 { pixmap.1 } else { 0 };
        let walked = outline.of(path.data(), transform, rows);
        nanos = nanos.saturating_add(walked.nanos);
        if let Some(area) = area.filter(|_| reach > 0) {
            // The outline has an edge on each side of every segment.
            let sides = Walked {
                nanos: walked.nanos,
                edges: walked.edges.saturating_mul(2),
                edge_rows: walked.edge_rows.saturating_mul(2),
            };
            runs = runs.saturating_add(sides.runs(area.width()));
        }
    }
    Ok(largest(pixels, nanos, stops, runs))
}

/// The edges of a fill: built from every segment, a contour left open closed
/// by one more line.
const FILL: Edges = Edges {
    each: SEGMENT_NS,
    half: 0.0,
    closes: true,
};

/// The steps a path in a clip costs on the clip's pixmap: the renderer fills
/// it black, strokes nothing, and skips a fill no wider or taller than a line,
/// but the shape was still built.
fn clipping(path: &Path, transform: Transform, pixmap: (u32, u32)) -> u64 {
    let canvas = IntRect::from_xywh(0, 0, pixmap.0, pixmap.1);
    let area = reaches(path.bounding_box(), transform, canvas);
    let reach = area.map_or(0, pixels_of);
    let Some(area) = area.filter(|_| path.fill().is_some() && filled(path) && reach > 0) else {
        return STEPS_PER_SHAPE;
    };
    let walked = FILL.of(path.data(), transform, pixmap.1);
    largest(
        reach.div_ceil(PIXELS_PER_STEP),
        walked.nanos,
        0,
        walked.runs(area.width()),
    )
}

/// Whether the renderer fills `path`: it skips one whose outline is no wider
/// or no taller than a line.
fn filled(path: &Path) -> bool {
    let bounds = path.data().bounds();
    bounds.width() > 0.0 && bounds.height() > 0.0
}

/// The steps of a shape: the largest of [`STEPS_PER_SHAPE`], its `pixels` in
/// steps with its pixels and `stops` added, the `nanos` its segments take,
/// and the `runs` its edges walk. A gradient's stops are worked at a pixel on
/// top of painting it, so the two are summed.
fn largest(pixels: u64, nanos: u64, stops: u64, runs: u64) -> u64 {
    STEPS_PER_SHAPE
        .max(pixels.saturating_add(stops.div_ceil(PIXEL_STOPS_PER_STEP)))
        .max(nanos.div_ceil(STEP_NS))
        .max(runs.div_ceil(RUNS_PER_STEP))
}

/// The part of a pixmap a shape with this bounding box reaches, as the
/// renderer clips it: none where it is off the pixmap.
fn reaches(bounds: Rect, transform: Transform, pixmap: Option<IntRect>) -> Option<IntRect> {
    bounds
        .transform(transform)
        .and_then(|rect| rect.round_out())
        .and_then(|rect| within(rect, pixmap?))
}

/// The pixels of `rect`.
fn pixels_of(rect: IntRect) -> u64 {
    u64::from(rect.width()).saturating_mul(u64::from(rect.height()))
}

/// What the edges of a path cost the renderer to build and walk.
struct Edges {
    /// The time each segment takes, in nanoseconds.
    each: u64,
    /// How far beyond the path the edges reach on each side, in pixels.
    half: f64,
    /// Whether a contour left open is closed by one more line.
    closes: bool,
}

/// What the edges of a path were found to cost.
struct Walked {
    /// The time building and walking them takes, in nanoseconds.
    nanos: u64,
    /// The edges, a curve counted as many times as it can cross one row: a
    /// quadratic twice, a cubic three times.
    edges: u64,
    /// Every row each edge crosses, summed.
    edge_rows: u64,
}

impl Walked {
    /// What the edges walk along their rows, on a span `width` pixels wide:
    /// for every edge-row, the runs it can pass, no more than the width and no
    /// more than four times the edges make. See [`RUNS_PER_STEP`].
    fn runs(&self, width: u32) -> u64 {
        let passed = u64::from(width).min(self.edges.saturating_mul(4));
        self.edge_rows.saturating_mul(passed)
    }
}

impl Edges {
    /// The time building the edges of `data` and walking them down a pixmap
    /// `height` rows tall takes: [`Edges::each`] for every segment, a move
    /// and a closing line among them, and [`ROW_NS`] for every row it
    /// crosses, as far as the pixmap reaches and at least one.
    fn of(&self, data: &resvg::tiny_skia::Path, transform: Transform, height: u32) -> Walked {
        let bottom = f64::from(height);
        let rows = |points: &[Point]| {
            let (mut top, mut low) = (f64::INFINITY, f64::NEG_INFINITY);
            for point in points {
                let mut point = *point;
                transform.map_point(&mut point);
                top = top.min(f64::from(point.y));
                low = low.max(f64::from(point.y));
            }
            let top = (top - self.half).max(0.0).floor();
            let low = (low + self.half).min(bottom).ceil();
            whole(low - top).max(1)
        };
        let mut total = Walked {
            nanos: 0,
            edges: 0,
            edge_rows: 0,
        };
        // A move is worked as a segment and makes no edge.
        let mut add = |points: &[Point]| {
            let crossed = rows(points);
            let walked = crossed.saturating_mul(ROW_NS);
            total.nanos = total.nanos.saturating_add(self.each.saturating_add(walked));
            let pieces = match points.len() {
                0 | 1 => 0,
                2 => 1,
                3 => 2,
                _ => 3,
            };
            total.edges = total.edges.saturating_add(pieces);
            total.edge_rows = total
                .edge_rows
                .saturating_add(crossed.saturating_mul(pieces));
        };
        let mut start = Point::zero();
        let mut at = Point::zero();
        let mut open = false;
        for segment in data.segments() {
            match segment {
                PathSegment::MoveTo(to) => {
                    if open && self.closes {
                        add(&[at, start]);
                    }
                    add(&[to]);
                    (start, at, open) = (to, to, true);
                }
                PathSegment::LineTo(to) => {
                    add(&[at, to]);
                    at = to;
                }
                PathSegment::QuadTo(control, to) => {
                    add(&[at, control, to]);
                    at = to;
                }
                PathSegment::CubicTo(first, second, to) => {
                    add(&[at, first, second, to]);
                    at = to;
                }
                PathSegment::Close => {
                    add(&[at, start]);
                    (at, open) = (start, false);
                }
            }
        }
        if open && self.closes {
            add(&[at, start]);
        }
        total
    }
}

/// The stops a paint works out at every pixel: none for a colour or a
/// pattern, whose own content is walked on its own.
fn gradient_stops(paint: &Paint) -> u64 {
    let count = match paint {
        Paint::LinearGradient(gradient) => gradient.stops().len(),
        Paint::RadialGradient(gradient) => gradient.stops().len(),
        Paint::Color(_) | Paint::Pattern(_) => 0,
    };
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// The dash array of `stroke` if the renderer will cut it into dashes: the
/// renderer draws a stroke solid when its array is not one it can use.
fn dashing(stroke: &Stroke) -> Option<&[f32]> {
    let array = stroke.dasharray()?;
    StrokeDash::new(array.to_vec(), stroke.dashoffset()).map(|_| array)
}

/// The steps a stroke cut into dashes by `array` costs on a pixmap.
///
/// The renderer cuts the path itself, before any transform, into as many
/// dashes as its length holds periods of the array, and builds and draws every
/// one. So the most there can be is the length over the period, rounded up,
/// for each entry of the array. Each costs what it can cover on the pixmap,
/// at most as long as the longest entry and as wide as the stroke, or what
/// working its width costs, whichever is more, and never less than
/// [`STEPS_PER_DASH`].
/// A count that no number can hold is refused.
///
/// # Errors
/// [`Refused::TooLarge`] when the length of the path is not a number.
fn dashes(
    path: &Path,
    stroke: &Stroke,
    array: &[f32],
    transform: Transform,
    pixmap: Option<IntRect>,
) -> Result<u64, Refused> {
    let Some(pixmap) = pixmap else {
        return Ok(0);
    };
    let period: f64 = array.iter().map(|entry| f64::from(*entry)).sum();
    let length = length(path.data());
    if !length.is_finite() || !period.is_finite() || period <= 0.0 {
        return Err(Refused::TooLarge);
    }
    let entries = u64::try_from(array.len()).unwrap_or(u64::MAX);
    let count = whole((length / period).ceil()).saturating_mul(entries);
    let longest = array
        .iter()
        .fold(0.0_f64, |most, one| most.max(f64::from(*one)));
    let width = f64::from(stroke.width().get());
    let (sx, sy) = transform.get_scale();
    let (sx, sy) = (f64::from(sx), f64::from(sy));
    let reach = f64::from(pixmap.width()) * f64::from(pixmap.height());
    let area = ((longest + width) * width * sx * sy).min(reach);
    // Pixels across the stroke beyond the first, as far as the canvas is wide.
    let across = (width * sx.max(sy) - 1.0)
        .min(f64::from(pixmap.width()) + f64::from(pixmap.height()))
        .max(0.0);
    let each = whole(area.ceil()).div_ceil(PIXELS_PER_STEP).max(
        STEPS_PER_DASH.saturating_add(whole(across.ceil()).saturating_mul(STEPS_PER_DASH_ROW)),
    );
    Ok(count.saturating_mul(each))
}

/// `value` as a count: a float saturates, and nothing is negative.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturates, and is judged against a ceiling"
)]
fn whole(value: f64) -> u64 {
    value as u64
}

/// The length of `path` in its own units: every segment of every contour, a
/// curve by the polygon of its control points, which is never shorter than the
/// curve.
fn length(path: &resvg::tiny_skia::Path) -> f64 {
    let between = |from: Point, to: Point| {
        (f64::from(to.x) - f64::from(from.x)).hypot(f64::from(to.y) - f64::from(from.y))
    };
    let mut total = 0.0;
    let mut start = Point::zero();
    let mut at = Point::zero();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(to) => {
                start = to;
                at = to;
            }
            PathSegment::LineTo(to) => {
                total += between(at, to);
                at = to;
            }
            PathSegment::QuadTo(control, to) => {
                total += between(at, control) + between(control, to);
                at = to;
            }
            PathSegment::CubicTo(first, second, to) => {
                total += between(at, first) + between(first, second) + between(second, to);
                at = to;
            }
            PathSegment::Close => {
                total += between(at, start);
                at = start;
            }
        }
    }
    total
}

/// What a primitive does with each pixel of its region, in pixel-steps.
fn steps(kind: &Kind, transform: Transform, region: IntRect) -> u64 {
    match kind {
        Kind::Turbulence(turbulence) => u64::from(turbulence.num_octaves()).max(1),
        Kind::Morphology(morphology) => {
            let (sx, sy) = transform.get_scale();
            let reach = |radius: f32, scale: f32, side: u32| {
                u64::from(unsigned((radius * scale).round()).min(side.saturating_sub(1)))
            };
            let across = reach(morphology.radius_x().get(), sx, region.width());
            let down = reach(morphology.radius_y().get(), sy, region.height());
            (2 * across + 1).saturating_mul(2 * down + 1)
        }
        Kind::ConvolveMatrix(convolve) => {
            let matrix = convolve.matrix();
            u64::from(matrix.columns())
                .saturating_mul(u64::from(matrix.rows()))
                .max(1)
        }
        // The whole region is drawn once for every input.
        Kind::Merge(merge) => u64::try_from(merge.inputs().len())
            .unwrap_or(u64::MAX)
            .max(1),
        _ => 1,
    }
}
