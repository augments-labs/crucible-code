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
//! [`MOST_DECODED`]. The second is the work of the filter primitives, each
//! counted as the pixels of its region times what it does with each pixel,
//! summed over the tree. It is refused past [`MOST_FILTER_WORK`].

use crate::{MOST_DECODED, Refused};
use resvg::tiny_skia::{IntRect, Transform};
use resvg::usvg::filter::{Filter, Kind};
use resvg::usvg::{ClipPath, Group, Image, ImageKind, Mask, Node, Paint, Path, Tree};

/// The most filter work a drawing may ask for: 140 000 pixel-steps.
///
/// A step is one pass over one pixel by the cheapest primitive; a primitive
/// that does more with each pixel (a turbulence per octave, a morphology per
/// window cell, a convolution per matrix cell) counts that many. The number is
/// the largest total at which the slowest primitive measured still drew within
/// 20 ms in a release build: over 140 000 pixels a drop shadow took 17 ms, a
/// Gaussian blur 14 ms and a one-octave turbulence 19 ms.
const MOST_FILTER_WORK: u64 = 140_000;

/// What a drawing holds and does, as far as the walk can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cost {
    /// The most bytes of pixmaps held at once.
    pub(crate) bytes: u64,
    /// The filter work of every primitive, in pixel-steps.
    pub(crate) work: u64,
}

/// What drawing `tree` onto a canvas `width` by `height` pixels, under `scale`,
/// would cost.
///
/// # Errors
/// [`Refused::TooLarge`] as soon as the bytes held pass [`MOST_DECODED`] or the
/// work passes [`MOST_FILTER_WORK`].
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

    /// Filter work added and judged.
    fn work(&mut self, steps: u64) -> Result<(), Refused> {
        self.cost.work = self.cost.work.saturating_add(steps);
        if self.cost.work > MOST_FILTER_WORK {
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
                Node::Image(image) => self.image(image, transform, context, held)?,
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
            self.clip(clip, layer_bytes, held)?;
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

    /// A clip is a pixmap of the layer's size, and a mask made from it. A
    /// clipped child of a clip is drawn on one of its own, and clipped in turn.
    fn clip(&mut self, clip: &ClipPath, layer: u64, held: u64) -> Result<(), Refused> {
        let held = held.saturating_add(layer);
        self.hold(held.saturating_add(layer / 4))?;
        self.clipped(clip.root(), layer, held)?;
        if let Some(inner) = clip.clip_path() {
            self.clip(inner, layer, held)?;
        }
        Ok(())
    }

    fn clipped(&mut self, parent: &Group, layer: u64, held: u64) -> Result<(), Refused> {
        for node in parent.children() {
            match node {
                Node::Group(group) => match group.clip_path() {
                    Some(clip) => {
                        let held = held.saturating_add(layer);
                        self.hold(held)?;
                        self.clipped(group, layer, held)?;
                        self.clip(clip, layer, held)?;
                    }
                    None => self.clipped(group, layer, held)?,
                },
                Node::Text(text) => self.clipped(text.flattened(), layer, held)?,
                Node::Path(_) | Node::Image(_) => {}
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

    /// A picture nested in a picture is drawn onto a copy of the pixmap it is
    /// drawn on. An image of any other kind is not drawn.
    fn image(
        &mut self,
        image: &Image,
        transform: Transform,
        context: &Context,
        held: u64,
    ) -> Result<(), Refused> {
        let ImageKind::SVG(nested) = image.kind() else {
            return Ok(());
        };
        if !image.is_visible() {
            return Ok(());
        }
        let held = held.saturating_add(bytes(context.pixmap.0, context.pixmap.1));
        self.hold(held)?;
        let inner = Context {
            most: most_layer(context.pixmap.0, context.pixmap.1)?,
            pixmap: context.pixmap,
        };
        self.nodes(nested.root(), transform, &inner, held)
    }
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
        _ => 1,
    }
}
