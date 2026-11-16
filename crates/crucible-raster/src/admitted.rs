//! What a picture may say, judged on the XML before the tree parser reads it.
//!
//! The tree parser follows what a picture says without asking what it costs,
//! so five things are refused before it starts, each as not an image:
//!
//! - an element outside [`DRAWN_ELEMENTS`](super::DRAWN_ELEMENTS);
//! - a reference anywhere but where a picture is drawn with one: an `href`, in
//!   any namespace, is let in on a `<use>` that names an element of the
//!   picture by `#id`, and on a filter's image step that names anything but an
//!   element, which is drawn as nothing or refused as an SVG. A gradient that
//!   takes its stops from another, an image step that draws an element, and an
//!   `href` on any other element are refused;
//! - an id given to more than one element, or given in a namespace: the parser
//!   reads `xml:id` as an id, and where an id is given twice it names one
//!   element from a `<use>` and another from a `url(#id)`;
//! - a number of the picture's geometry past [`MOST_MAGNITUDE`]: a length, a
//!   coordinate, a radius, a number of path data, a view box, a factor of a
//!   transform, a width of a stroke and a font size, wherever the parser reads
//!   one, in an attribute or in a `style` attribute;
//! - a `width`, `height`, `x` or `y` in a percentage, `em` or `ex`, in an
//!   attribute or in a `style` attribute, on a `<svg>` inside the picture, on a
//!   `<symbol>` or on a `<use>`: each sets the view the next percentage is
//!   taken of from the view it sits in, so a chain of them multiplies a size
//!   past any judged here. The outermost `<svg>` keeps its relative sizes,
//!   which the parser takes of a view of its own.
//!
//! A length is judged in user units: an absolute unit by its size, a
//! percentage by the largest view any element of the picture sets, and a size
//! in `em` or `ex` by the largest font size the element can be given, which is
//! worked out through everything that holds or copies it, as the parser
//! compounds it. The outermost `<svg>` sets a view by its own size, relative
//! or not, and a `<use>` that draws a `<symbol>` by the size it gives it. A
//! value that does not parse is passed over, as the parser passes over it.
//!
//! What a picture lets through is priced by the outline the parser builds of
//! each shape, with its sizes taken the same way: the curves it cuts a circle,
//! an ellipse, a rounded rectangle and an arc of a path into, more the larger
//! the radius, and, for a shape stroked by what it or anything that holds or
//! copies it says, the outline of that stroke, an edge or a curve at a time.
//! A stroked shape the page can turn or skew, by a transform of its own or of
//! anything that holds, copies or marks it, is outlined a second time at its
//! size on the page: each curve of that second outline is priced at its size
//! times the most that the views, the transforms and, for a marker, the
//! stroke it is drawn on can enlarge it. A size past the largest measured
//! refuses the picture as too large.

use super::{
    ARC_OUTLINE_NS, CUBIC_MADE_NS, CURVE_OUTLINE_NS, Elements, Refused, STRAIGHT_OUTLINE_NS,
    STROKED_SHAPE_NS, is_drawn_element,
};
use resvg::usvg::roxmltree::{Document, Node};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use svgtypes::{
    Angle, FontShorthand, Length, LengthListParser, LengthUnit, PathParser, PathSegment,
    PointsParser, Transform, ViewBox,
};

/// The largest size a number of a picture's geometry may have, in user units:
/// 100 000.
///
/// The parser cuts an arc into curves by its radius, with no bound on how
/// many: a circle of radius `3e38` is cut into millions before it is thrown
/// away, and one arc of radius `1e154` asks for 7.5 GB. At 100 000 an arc is
/// eleven curves at the most. The pictures this crate draws use no number
/// larger than 3 117, so this is more than thirty times that.
const MOST_MAGNITUDE: f64 = 100_000.0;

/// The font size the parser starts from: 12.
const FIRST_FONT_SIZE: f64 = 12.0;

/// The namespaces the parser reads an attribute in: none, that of SVG, that of
/// `XLink` and that of XML. An attribute in any other is passed over.
const READ_NAMESPACES: &[&str] = &[
    super::SVG_NAMESPACE,
    "http://www.w3.org/1999/xlink",
    "http://www.w3.org/XML/1998/namespace",
];

/// Lengths that are given as a list, every one of which is judged.
const LENGTH_LISTS: &[&str] = &["x", "y", "dx", "dy", "stroke-dasharray"];

/// Lengths that are given one at a time.
const LENGTHS: &[&str] = &[
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "fr",
    "width",
    "height",
    "refX",
    "refY",
    "markerWidth",
    "markerHeight",
    "stroke-width",
    "stroke-dashoffset",
    "letter-spacing",
    "word-spacing",
    "baseline-shift",
    "textLength",
];

/// The lengths that take the value `inherit` from what holds the element.
const INHERITED: &[&str] = &[
    "stroke-dasharray",
    "stroke-width",
    "stroke-dashoffset",
    "letter-spacing",
    "word-spacing",
    "baseline-shift",
];

/// Whether `document` says only what a picture may say: see the module.
///
/// The time the parser takes to build the outline of each element, for each
/// place it is drawn, when it is.
///
/// # Errors
/// [`Refused::NotAnImage`] for anything the module refuses, and for an element
/// that comes to hold or copy itself.
pub(super) fn admitted(document: &Document<'_>) -> Result<Outlines, Refused> {
    let elements = Elements::of(document);
    let allowed = elements.all.iter().all(|element| {
        is_drawn_element(element)
            && references_allowed(element)
            && !id_in_a_namespace(element)
            && !sets_a_view_relatively(element)
    });
    if !allowed || given_twice(&elements) {
        return Err(Refused::NotAnImage);
    }
    sizes_within(&elements).ok_or(Refused::NotAnImage)
}

/// The time the tree parser takes to build the outline of each element, in
/// document order, each time it is drawn.
pub(super) struct Outlines(Vec<u64>);

impl Outlines {
    /// The time for the element at `at`: none for an element that is not a
    /// shape.
    pub(super) fn of(&self, at: usize) -> u64 {
        self.0.get(at).copied().unwrap_or(0)
    }
}

/// Whether every `href` of `element` is one a picture is drawn with.
fn references_allowed(element: &Node<'_, '_>) -> bool {
    element
        .attributes()
        .filter(|attribute| attribute.name() == "href")
        .all(|attribute| {
            let names_an_element = attribute.value().trim_start().starts_with('#');
            match element.tag_name().name() {
                "use" => names_an_element,
                "feImage" => !names_an_element,
                _ => false,
            }
        })
}

/// Whether `element` is given an id in a namespace.
fn id_in_a_namespace(element: &Node<'_, '_>) -> bool {
    element
        .attributes()
        .any(|attribute| attribute.name() == "id" && attribute.namespace().is_some())
}

/// Whether `element` sets the view inside it by a size relative to the view it
/// sits in: a `width`, `height`, `x` or `y` in a percentage, `em` or `ex` on a
/// `<svg>` that is not the outermost, a `<symbol>` or a `<use>`.
fn sets_a_view_relatively(element: &Node<'_, '_>) -> bool {
    let sets_a_view = match element.tag_name().name() {
        "svg" => element.parent_element().is_some(),
        "symbol" | "use" => true,
        _ => false,
    };
    sets_a_view
        && properties(element)
            .into_iter()
            .filter(|(name, _)| matches!(*name, "width" | "height" | "x" | "y"))
            .filter_map(|(_, value)| Length::from_str(value).ok())
            .any(|length| {
                matches!(
                    length.unit,
                    LengthUnit::Percent | LengthUnit::Em | LengthUnit::Ex
                )
            })
}

/// Whether any id is given to more than one element.
fn given_twice(elements: &Elements<'_, '_>) -> bool {
    let mut seen = HashSet::new();
    elements
        .all
        .iter()
        .filter_map(|element| element.attribute("id"))
        .any(|id| !seen.insert(id))
}

/// A size as far as it can be told before the tree is built: a part in user
/// units, and a part that is so many times a font size.
#[derive(Clone, Copy, Default)]
struct Size {
    fixed: f64,
    ems: f64,
}

impl Size {
    fn fixed(fixed: f64) -> Self {
        Self {
            fixed: fixed.abs(),
            ems: 0.0,
        }
    }

    fn ems(ems: f64) -> Self {
        Self {
            fixed: 0.0,
            ems: ems.abs(),
        }
    }

    /// The larger of each part.
    fn or(self, other: Self) -> Self {
        Self {
            fixed: self.fixed.max(other.fixed),
            ems: self.ems.max(other.ems),
        }
    }

    /// The size in user units under a font `font` large.
    fn at(self, font: f64) -> f64 {
        self.fixed.max(self.ems * font)
    }

    /// `length` as a size, a percentage of `view`.
    fn of(length: Length, view: f64) -> Self {
        let number = length.number;
        match length.unit {
            LengthUnit::None | LengthUnit::Px => Self::fixed(number),
            LengthUnit::In => Self::fixed(number * 96.0),
            LengthUnit::Cm => Self::fixed(number * 96.0 / 2.54),
            LengthUnit::Mm => Self::fixed(number * 96.0 / 25.4),
            LengthUnit::Pt => Self::fixed(number * 4.0 / 3.0),
            LengthUnit::Pc => Self::fixed(number * 16.0),
            LengthUnit::Em => Self::ems(number),
            LengthUnit::Ex => Self::ems(number / 2.0),
            LengthUnit::Percent => Self::fixed(number / 100.0 * view),
        }
    }

    /// A font size given as `value`, where an `em`, an `ex`, a percentage and
    /// a size named relative to another are of the font size of what holds
    /// the element. `None` for `inherit`.
    fn of_font(value: &str) -> Option<Self> {
        let value = value.trim();
        if value == "inherit" {
            return None;
        }
        let Ok(length) = Length::from_str(value) else {
            // A size by name is the font size of what holds it, times 1.2 for
            // each step larger than `medium`, as the parser takes it.
            let steps = match value {
                "large" | "larger" => 1,
                "x-large" => 2,
                "xx-large" => 3,
                _ => 0,
            };
            return Some(Self::ems(1.2_f64.powi(steps)));
        };
        Some(match length.unit {
            LengthUnit::Percent => Self::ems(length.number / 100.0),
            _ => Self::of(length, 0.0),
        })
    }
}

/// Whether `size` is within [`MOST_MAGNITUDE`]: one that is not a number is
/// not.
fn within(size: f64) -> bool {
    size <= MOST_MAGNITUDE
}

/// The attributes of `element` the parser reads, and the declarations of its
/// `style` attribute, a `font` shorthand among them as the font size it gives.
fn properties<'a>(element: &Node<'a, '_>) -> Vec<(&'a str, &'a str)> {
    let attributes = element
        .attributes()
        .filter(|attribute| {
            attribute
                .namespace()
                .is_none_or(|namespace| READ_NAMESPACES.contains(&namespace))
        })
        .map(|attribute| (attribute.name(), attribute.value()));
    let declared = element
        .attribute("style")
        .into_iter()
        .flat_map(simplecss::DeclarationTokenizer::from)
        .map(|declaration| (declaration.name, declaration.value));
    attributes
        .chain(declared)
        .map(|(name, value)| match name {
            "font" => (
                "font-size",
                FontShorthand::from_str(value).map_or("", |font| font.font_size),
            ),
            _ => (name, value),
        })
        .collect()
}

/// The outline of every element, if every number of every element's geometry
/// is within [`MOST_MAGNITUDE`]; see the module. `None` where one is not, or
/// where an element comes to hold or copy itself.
fn sizes_within(elements: &Elements<'_, '_>) -> Option<Outlines> {
    let order = elements.in_order()?;
    let count = elements.all.len();
    let properties: Vec<Vec<(&str, &str)>> = elements.all.iter().map(properties).collect();
    let view = properties
        .iter()
        .flatten()
        .filter_map(|&(name, value)| match name {
            "viewBox" => ViewBox::from_str(value)
                .ok()
                .map(|view| view.w.abs().max(view.h.abs())),
            _ => None,
        })
        .fold(100.0_f64, f64::max);
    // A `<use>` sets the view of the `<symbol>` it draws by its size.
    let mut view = elements
        .all
        .iter()
        .enumerate()
        .zip(&properties)
        .filter(|((at, element), _)| {
            element.tag_name().name() == "svg"
                || elements
                    .copied(*at)
                    .is_some_and(|copied| elements.name(copied) == "symbol")
        })
        .flat_map(|(_, properties)| properties)
        .filter(|(name, _)| matches!(*name, "width" | "height"))
        .filter_map(|(_, value)| Length::from_str(value).ok())
        .filter(|length| {
            !matches!(
                length.unit,
                LengthUnit::Percent | LengthUnit::Em | LengthUnit::Ex
            )
        })
        .map(|length| Size::of(length, 0.0).fixed)
        .fold(view, f64::max);

    // The lengths given as `inherit` somewhere, whose largest given value is
    // carried down to every element that holds or is copied by another.
    let inherited: Vec<&str> = INHERITED
        .iter()
        .copied()
        .filter(|name| {
            properties
                .iter()
                .flatten()
                .any(|&(given, value)| given == *name && value.trim() == "inherit")
        })
        .collect();
    let mut carried: HashMap<&str, Vec<Size>> = inherited
        .iter()
        .map(|&name| (name, vec![Size::default(); count]))
        .collect();
    // The largest font size each element can have, and the largest it is
    // given that `inherit` takes.
    let mut font = vec![FIRST_FONT_SIZE; count];
    let mut font_given = vec![Size::default(); count];
    let mut stroked = vec![false; count];
    // How many times larger on the page than in its own units each element
    // can be, at the most, and whether what places it can turn or skew it.
    let mut scale = vec![1.0_f64; count];
    let mut turned = vec![false; count];
    // For each element inside a marker, whether a marker holding it is sized
    // by the stroke of the shape it is drawn on.
    let mut marked: Vec<Option<bool>> = vec![None; count];
    let mut widest_stroke = 1.0_f64;

    let parents = elements.parents();
    for &at in &order {
        let from: &[usize] = parents.get(at).map_or(&[], Vec::as_slice);
        let largest = |of: &[f64]| {
            from.iter()
                .filter_map(|&parent| of.get(parent).copied())
                .fold(None, |most: Option<f64>, size| {
                    Some(most.map_or(size, |most| most.max(size)))
                })
        };
        let given_above = from
            .iter()
            .filter_map(|&parent| font_given.get(parent).copied())
            .fold(Size::default(), Size::or);
        let font_above = largest(&font).unwrap_or(FIRST_FONT_SIZE);
        let own = properties.get(at).map_or(&[][..], Vec::as_slice);
        let mut size = None;
        let mut given = given_above;
        for &(name, value) in own {
            if name != "font-size" || value.is_empty() {
                continue;
            }
            let spec = Size::of_font(value).unwrap_or(given_above);
            given = given.or(spec);
            size = Some(size.unwrap_or(0.0_f64).max(spec.at(font_above)));
        }
        let size = size.unwrap_or(font_above);
        if !within(size) {
            return None;
        }
        // The outermost `<svg>` comes first, and sets the view of everything
        // after it by its own size, a relative one taken of the view before.
        let outermost = elements.parent(at).is_none();
        let around = view;
        if outermost {
            view = own
                .iter()
                .filter(|(name, _)| matches!(*name, "width" | "height"))
                .filter_map(|(_, value)| Length::from_str(value).ok())
                .map(|length| Size::of(length, around).at(size))
                .fold(view, f64::max);
        }
        if let Some(entry) = font.get_mut(at) {
            *entry = size;
        }
        if let Some(entry) = font_given.get_mut(at) {
            *entry = given;
        }

        for (&name, carried) in &mut carried {
            let above = from
                .iter()
                .filter_map(|&parent| carried.get(parent).copied())
                .fold(Size::default(), Size::or);
            let mut here = above;
            for &(given, value) in own {
                if given != name {
                    continue;
                }
                if value.trim() == "inherit" {
                    if !within(above.at(size)) {
                        return None;
                    }
                } else {
                    here = here.or(lengths(value)
                        .map(|length| Size::of(length, view))
                        .fold(Size::default(), Size::or));
                }
            }
            if let Some(entry) = carried.get_mut(at) {
                *entry = here;
            }
        }

        let view_of = |name: &str| {
            if outermost && matches!(name, "width" | "height" | "x" | "y") {
                around
            } else {
                view
            }
        };
        if !own
            .iter()
            .all(|&(name, value)| property_within(name, value, size, view_of(name)))
        {
            return None;
        }

        // A shape is stroked by the last `stroke` it is given, a declaration
        // of its style over its attribute, and otherwise by what holds or
        // copies it.
        let is_stroked = strokes(own, from, &stroked);
        if let Some(entry) = stroked.get_mut(at) {
            *entry = is_stroked;
        }

        // What a marker holds is placed by the shape it is drawn on, not by
        // what holds the marker, and is worked out after every element.
        let name = elements.name(at);
        let (stretch, turns) = transformed(own);
        let fitted = fitted(name, own, outermost, view, |length| {
            Size::of(length, if outermost { around } else { view }).at(size)
        });
        let (above, turned_above, marked_above) = if name == "marker" {
            marker_placed(own)
        } else {
            (
                largest(&scale).unwrap_or(1.0),
                from.iter()
                    .any(|&parent| turned.get(parent).copied().unwrap_or(false)),
                from.iter()
                    .filter_map(|&parent| marked.get(parent).copied().flatten())
                    .reduce(|one, other| one || other),
            )
        };
        if let Some(entry) = scale.get_mut(at) {
            *entry = above * stretch * fitted;
        }
        if let Some(entry) = turned.get_mut(at) {
            *entry = turned_above || turns;
        }
        if let Some(entry) = marked.get_mut(at) {
            *entry = marked_above;
        }
        widest_stroke = own
            .iter()
            .filter(|(name, _)| *name == "stroke-width")
            .filter_map(|(_, value)| Length::from_str(value).ok())
            .map(|length| Size::of(length, view).at(size))
            .fold(widest_stroke, f64::max);
    }

    // A marker is placed by the shape it is drawn on, which may be anywhere
    // outside the markers: as large as the largest of them, turned if any of
    // them is, and in the units of the widest stroke where it says so.
    let outside = || (0..count).filter(|&at| marked.get(at).is_some_and(Option::is_none));
    let largest_outside = outside()
        .filter_map(|at| scale.get(at).copied())
        .fold(1.0_f64, f64::max);
    let turned_outside = outside().any(|at| turned.get(at).copied().unwrap_or(false));
    let outlines = (0..count)
        .map(|at| {
            let own = properties.get(at).map_or(&[][..], Vec::as_slice);
            let font = font.get(at).copied().unwrap_or(FIRST_FONT_SIZE);
            let mut placed = Placed {
                stroked: stroked.get(at).copied().unwrap_or(false),
                turned: turned.get(at).copied().unwrap_or(false),
                scale: scale.get(at).copied().unwrap_or(1.0),
            };
            if let Some(Some(in_strokes)) = marked.get(at) {
                placed.turned |= turned_outside;
                placed.scale *= largest_outside * if *in_strokes { widest_stroke } else { 1.0 };
            }
            outline(elements.name(at), own, placed, |length| {
                Size::of(length, view).at(font)
            })
        })
        .collect();
    Some(Outlines(outlines))
}

/// How far the transforms `own` gives stretch an element at the most, and
/// whether any of them turns or skews it.
fn transformed(own: &[(&str, &str)]) -> (f64, bool) {
    own.iter()
        .filter(|(name, _)| *name == "transform")
        .filter_map(|(_, value)| Transform::from_str(value).ok())
        .fold((1.0_f64, false), |(most, turns), matrix| {
            // The largest singular value of the matrix's linear part.
            let sum = matrix.a * matrix.a
                + matrix.b * matrix.b
                + matrix.c * matrix.c
                + matrix.d * matrix.d;
            let determinant = matrix.a * matrix.d - matrix.b * matrix.c;
            let spread = (sum * sum - 4.0 * determinant * determinant)
                .max(0.0)
                .sqrt();
            let stretch = f64::midpoint(sum, spread).sqrt();
            (
                most.max(stretch),
                turns || matrix.b != 0.0 || matrix.c != 0.0,
            )
        })
}

/// How many times the view box of the element `name` whose properties are
/// `own` enlarges what it holds, at the most, by the size it is fitted to:
/// that of the outermost `<svg>` and of a `<marker>` by the lengths `size`
/// takes into user units, and `view`, the largest view any element sets, for
/// a `<svg>` inside the picture and a `<symbol>`, which what draws them can
/// size. One for an element with no view box.
fn fitted(
    name: &str,
    own: &[(&str, &str)],
    outermost: bool,
    view: f64,
    size: impl Fn(Length) -> f64,
) -> f64 {
    let Some(view_box) = own
        .iter()
        .filter(|(name, _)| *name == "viewBox")
        .filter_map(|(_, value)| ViewBox::from_str(value).ok())
        .find(|view_box| view_box.w > 0.0 && view_box.h > 0.0)
    else {
        return 1.0;
    };
    let given = |of: &str| {
        own.iter()
            .filter(|(name, _)| *name == of)
            .filter_map(|(_, value)| Length::from_str(value).ok())
            .next_back()
    };
    let (width, height) = match name {
        // The outermost `<svg>` is as large as its view box unless it says.
        "svg" if outermost => (
            given("width").map_or(view_box.w, &size),
            given("height").map_or(view_box.h, &size),
        ),
        "svg" | "symbol" => (view, view),
        // A marker is three units square unless it says.
        "marker" => (
            given("markerWidth").map_or(3.0, &size),
            given("markerHeight").map_or(3.0, &size),
        ),
        _ => return 1.0,
    };
    (width / view_box.w).max(height / view_box.h)
}

/// Whether a shape whose properties are `own` is stroked: by the last
/// `stroke` it is given, and otherwise, or by `inherit`, by any of the
/// elements `from` that hold or copy it.
fn strokes(own: &[(&str, &str)], from: &[usize], stroked: &[bool]) -> bool {
    let stroke = own
        .iter()
        .rev()
        .find(|(name, _)| *name == "stroke")
        .map(|(_, value)| value.trim());
    match stroke {
        Some("none") => false,
        Some("inherit") | None => from
            .iter()
            .any(|&parent| stroked.get(parent).copied().unwrap_or(false)),
        Some(_) => true,
    }
}

/// How a marker whose properties are `own` places what it holds, before the
/// shape it is drawn on: at its own size, turned if its `orient` turns it,
/// and whether it is sized by the stroke of that shape.
fn marker_placed(own: &[(&str, &str)]) -> (f64, bool, Option<bool>) {
    let in_strokes = !own
        .iter()
        .any(|&(name, value)| name == "markerUnits" && value.trim() == "userSpaceOnUse");
    (1.0, orient_turns(own), Some(in_strokes))
}

/// Whether a marker whose properties are `own` is turned along the shape it
/// is drawn on, or by an angle it gives.
fn orient_turns(own: &[(&str, &str)]) -> bool {
    own.iter()
        .filter(|(name, _)| *name == "orient")
        .any(|(_, value)| match value.trim() {
            "auto" | "auto-start-reverse" => true,
            value => Angle::from_str(value).is_ok_and(|angle| angle.number != 0.0),
        })
}

/// How a shape is placed on the page: whether it is stroked, whether what
/// places it can turn or skew it, and how many times larger than in its own
/// units it can be, at the most.
#[derive(Clone, Copy)]
struct Placed {
    stroked: bool,
    turned: bool,
    scale: f64,
}

/// What the parser builds of a shape: the curves it cuts its arcs into, the
/// time it takes to outline its stroke, past [`STROKED_SHAPE_NS`], and the
/// time it takes to outline that stroke again at the shape's size on the page,
/// which it does where what places the shape turns or skews it.
#[derive(Clone, Copy, Default)]
struct Built {
    curves: u64,
    stroke: u64,
    again: u64,
}

impl Built {
    /// `edges` straight edges, outlined at the same cost at any size.
    fn straight(edges: u64) -> Self {
        let stroke = edges.saturating_mul(STRAIGHT_OUTLINE_NS);
        Self {
            curves: 0,
            stroke,
            again: stroke,
        }
    }

    /// The time it takes to build, placed as `placed` says.
    fn time(self, placed: Placed) -> u64 {
        let curves = self.curves.saturating_mul(CUBIC_MADE_NS);
        if !placed.stroked {
            return curves;
        }
        let once = curves
            .saturating_add(STROKED_SHAPE_NS)
            .saturating_add(self.stroke);
        if placed.turned {
            once.saturating_add(STROKED_SHAPE_NS)
                .saturating_add(self.again)
        } else {
            once
        }
    }
}

/// The time the parser takes to build the outline of the element `name` whose
/// properties are `own`, placed as `placed` says, with `size` the size of a
/// length of it in user units. Nothing for an element that is not a shape, or
/// a shape the parser skips: a circle, an ellipse or a rectangle with no size,
/// and fewer than two points.
fn outline(name: &str, own: &[(&str, &str)], placed: Placed, size: impl Fn(Length) -> f64) -> u64 {
    let largest = |of: &str| {
        own.iter()
            .filter(|(name, _)| *name == of)
            .filter_map(|(_, value)| Length::from_str(value).ok())
            .map(&size)
            .fold(None, |most: Option<f64>, size| {
                Some(most.map_or(size, |most| most.max(size)))
            })
    };
    // One radius given stands for both, as the parser takes it.
    let radii = || {
        let (rx, ry) = (largest("rx"), largest("ry"));
        (rx.or(ry).unwrap_or(0.0), ry.or(rx).unwrap_or(0.0))
    };
    let each = |of: &str, built: &dyn Fn(&str) -> Option<Built>| {
        own.iter()
            .filter(|(name, _)| *name == of)
            .filter_map(|(_, value)| built(value))
            .map(|built| built.time(placed))
            .max()
            .unwrap_or(0)
    };
    let built = match name {
        "circle" => {
            let radius = largest("r").unwrap_or(0.0);
            (radius > 0.0).then(|| rounded(radius, radius, placed.scale))
        }
        "ellipse" => {
            let (rx, ry) = radii();
            (rx > 0.0 && ry > 0.0).then(|| rounded(rx.max(ry), rx.max(ry), placed.scale))
        }
        "rect" => {
            let width = largest("width").unwrap_or(0.0);
            let height = largest("height").unwrap_or(0.0);
            let (rx, ry) = radii();
            let (corner_x, corner_y) = (rx.min(width / 2.0), ry.min(height / 2.0));
            (width > 0.0 && height > 0.0).then(|| {
                if corner_x > 0.0 {
                    rounded(corner_x.max(corner_y), width.max(height), placed.scale)
                } else {
                    Built::straight(4)
                }
            })
        }
        "line" => Some(Built::straight(1)),
        "polyline" | "polygon" => {
            let closed = u64::from(name == "polygon");
            return each("points", &|points| {
                let count = u64::try_from(PointsParser::from(points).count()).unwrap_or(u64::MAX);
                (count >= 2).then(|| Built::straight(count - 1 + closed))
            });
        }
        "path" => return each("d", &|data| Some(path_built(data, placed.scale))),
        _ => None,
    };
    built.map_or(0, |built| built.time(placed))
}

/// A circle, an ellipse or a rectangle with rounded corners: four arcs of
/// `radius`, and the outline of a stroke as long as `side`, and as long as
/// `scale` times that on the page.
fn rounded(radius: f64, side: f64, scale: f64) -> Built {
    Built {
        curves: count((arc_curves(radius) / 4.0).ceil()).saturating_mul(4),
        stroke: priced(ARC_OUTLINE_NS, side),
        again: priced(ARC_OUTLINE_NS, side * scale),
    }
}

/// What the parser builds of path data `data`: each arc cut into curves by its
/// radius, and its stroke outlined a segment at a time, a curve by how far its
/// points reach from where it starts, the control a smooth curve reflects
/// among them, and outlined again with each curve `scale` times as large.
fn path_built(data: &str, scale: f64) -> Built {
    let mut built = Built::default();
    let (mut at, mut start) = ((0.0_f64, 0.0_f64), (0.0_f64, 0.0_f64));
    // The second control of the last cubic, and the control of the last
    // quadratic, which the smooth curve after each reflects.
    let (mut cubic, mut quadratic) = (None, None);
    for segment in PathParser::from(data) {
        let Ok(segment) = segment else {
            break;
        };
        let from = if segment.is_abs() { (0.0, 0.0) } else { at };
        let point = |x: f64, y: f64| (from.0 + x, from.1 + y);
        let reflected = |control: Option<(f64, f64)>| {
            control.map_or(at, |(x, y)| (2.0 * at.0 - x, 2.0 * at.1 - y))
        };
        let curve = |points: &[(f64, f64)]| {
            let reach = points
                .iter()
                .map(|(x, y)| (x - at.0).abs().max((y - at.1).abs()))
                .fold(0.0_f64, f64::max);
            (
                priced(CURVE_OUTLINE_NS, reach),
                priced(CURVE_OUTLINE_NS, reach * scale),
            )
        };
        let straight = (STRAIGHT_OUTLINE_NS, STRAIGHT_OUTLINE_NS);
        let (end, (stroke, again)) = match segment {
            PathSegment::MoveTo { x, y, .. } => {
                start = point(x, y);
                (start, (0, 0))
            }
            PathSegment::LineTo { x, y, .. } => (point(x, y), straight),
            PathSegment::HorizontalLineTo { x, .. } => ((from.0 + x, at.1), straight),
            PathSegment::VerticalLineTo { y, .. } => ((at.0, from.1 + y), straight),
            PathSegment::ClosePath { .. } => (start, straight),
            PathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
                ..
            } => {
                let (second, end) = (point(x2, y2), point(x, y));
                (end, curve(&[point(x1, y1), second, end]))
            }
            PathSegment::SmoothCurveTo { x2, y2, x, y, .. } => {
                let (second, end) = (point(x2, y2), point(x, y));
                (end, curve(&[reflected(cubic), second, end]))
            }
            PathSegment::Quadratic { x1, y1, x, y, .. } => {
                let end = point(x, y);
                (end, curve(&[point(x1, y1), end]))
            }
            PathSegment::SmoothQuadratic { x, y, .. } => {
                let end = point(x, y);
                (end, curve(&[reflected(quadratic), end]))
            }
            PathSegment::EllipticalArc {
                rx,
                ry,
                x_axis_rotation,
                x,
                y,
                ..
            } => {
                let end = point(x, y);
                let radius = arc_radius(at, end, rx, ry, x_axis_rotation);
                if radius > 0.0 {
                    built.curves = built
                        .curves
                        .saturating_add(count(arc_curves(radius).ceil()));
                    (
                        end,
                        (
                            priced(ARC_OUTLINE_NS, radius),
                            priced(ARC_OUTLINE_NS, radius * scale),
                        ),
                    )
                } else {
                    (end, straight)
                }
            }
        };
        (cubic, quadratic) = match segment {
            PathSegment::CurveTo { x2, y2, .. } | PathSegment::SmoothCurveTo { x2, y2, .. } => {
                (Some(point(x2, y2)), None)
            }
            PathSegment::Quadratic { x1, y1, .. } => (None, Some(point(x1, y1))),
            PathSegment::SmoothQuadratic { .. } => (None, Some(reflected(quadratic))),
            _ => (None, None),
        };
        built.stroke = built.stroke.saturating_add(stroke);
        built.again = built.again.saturating_add(again);
        at = end;
    }
    built
}

/// How many curves the parser cuts a whole turn of an arc of `radius` into, as
/// a number not yet rounded up: kurbo's count at the parser's tolerance of a
/// tenth of a unit, never fewer than four.
fn arc_curves(radius: f64) -> f64 {
    (1.1163 * radius / 0.1).powf(1.0 / 6.0).max(3.999_999)
}

/// The price `table` gives `size`: that of the first size it is within, and
/// past the last one more than any picture may take.
fn priced(table: &[(f64, u64)], size: f64) -> u64 {
    table
        .iter()
        .find(|(most, _)| size <= *most)
        .map_or(u64::MAX, |&(_, time)| time)
}

/// `value` as a count: a float saturates, and nothing is negative.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturates, and is judged against a ceiling"
)]
fn count(value: f64) -> u64 {
    value as u64
}

/// The lengths of a list, up to the first that does not parse.
fn lengths(value: &str) -> impl Iterator<Item = Length> + '_ {
    LengthListParser::from(value).map_while(Result::ok)
}

/// Whether the property `name` given as `value`, on an element whose font size
/// is `font` and in a picture whose largest view is `view`, is within
/// [`MOST_MAGNITUDE`].
fn property_within(name: &str, value: &str, font: f64, view: f64) -> bool {
    let size = |length: Length| Size::of(length, view).at(font);
    match name {
        _ if LENGTH_LISTS.contains(&name) => lengths(value).all(|length| within(size(length))),
        _ if LENGTHS.contains(&name) => {
            Length::from_str(value).map_or(true, |length| within(size(length)))
        }
        "viewBox" => ViewBox::from_str(value).map_or(true, |view| {
            [view.x, view.y, view.w, view.h]
                .into_iter()
                .all(|number| within(number.abs()))
        }),
        "transform" | "gradientTransform" => Transform::from_str(value).map_or(true, |matrix| {
            [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
                .into_iter()
                .all(|number| within(number.abs()))
        }),
        "points" => PointsParser::from(value).all(|(x, y)| within(x.abs()) && within(y.abs())),
        "d" => path_within(value),
        _ => true,
    }
}

/// Whether every point path data `data` reaches, and the radius of every arc
/// it draws, are within [`MOST_MAGNITUDE`].
///
/// A point is taken where the data puts it, a relative one from the point
/// before. An arc's radius is the larger of its two, grown as the parser grows
/// them when they are too small to reach from one end to the other; an arc the
/// parser draws as a line has none.
fn path_within(data: &str) -> bool {
    let point = |x: f64, y: f64| within(x.abs()) && within(y.abs());
    let (mut at_x, mut at_y) = (0.0_f64, 0.0_f64);
    let (mut start_x, mut start_y) = (0.0_f64, 0.0_f64);
    for segment in PathParser::from(data) {
        let Ok(segment) = segment else {
            break;
        };
        let (from_x, from_y) = if segment.is_abs() {
            (0.0, 0.0)
        } else {
            (at_x, at_y)
        };
        let ends = match segment {
            PathSegment::MoveTo { x, y, .. } => {
                (start_x, start_y) = (from_x + x, from_y + y);
                (from_x + x, from_y + y)
            }
            PathSegment::LineTo { x, y, .. } | PathSegment::SmoothQuadratic { x, y, .. } => {
                (from_x + x, from_y + y)
            }
            PathSegment::HorizontalLineTo { x, .. } => (from_x + x, at_y),
            PathSegment::VerticalLineTo { y, .. } => (at_x, from_y + y),
            PathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
                ..
            } => {
                if !point(from_x + x1, from_y + y1) || !point(from_x + x2, from_y + y2) {
                    return false;
                }
                (from_x + x, from_y + y)
            }
            PathSegment::SmoothCurveTo { x2, y2, x, y, .. } => {
                if !point(from_x + x2, from_y + y2) {
                    return false;
                }
                (from_x + x, from_y + y)
            }
            PathSegment::Quadratic { x1, y1, x, y, .. } => {
                if !point(from_x + x1, from_y + y1) {
                    return false;
                }
                (from_x + x, from_y + y)
            }
            PathSegment::EllipticalArc {
                rx,
                ry,
                x_axis_rotation,
                x,
                y,
                ..
            } => {
                let to = (from_x + x, from_y + y);
                if !within(arc_radius((at_x, at_y), to, rx, ry, x_axis_rotation)) {
                    return false;
                }
                to
            }
            PathSegment::ClosePath { .. } => (start_x, start_y),
        };
        if !point(ends.0, ends.1) {
            return false;
        }
        (at_x, at_y) = ends;
    }
    true
}

/// The radius an arc from `from` to `to` is cut into curves by, as the parser
/// works it out: the larger of `rx` and `ry`, grown by the same factor as both
/// when they cannot reach from one end to the other. Nothing for an arc drawn
/// as a line.
fn arc_radius(from: (f64, f64), to: (f64, f64), rx: f64, ry: f64, rotation: f64) -> f64 {
    let (rx, ry) = (rx.abs(), ry.abs());
    if rx <= 1e-5 || ry <= 1e-5 || from == to {
        return 0.0;
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let half_x = (from.0 - to.0) / 2.0;
    let half_y = (from.1 - to.1) / 2.0;
    let along = cos * half_x + sin * half_y;
    let across = -sin * half_x + cos * half_y;
    let reach = (along * along) / (rx * rx) + (across * across) / (ry * ry);
    rx.max(ry) * reach.sqrt().max(1.0)
}
