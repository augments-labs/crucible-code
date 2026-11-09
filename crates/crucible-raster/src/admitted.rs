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
//! compounds it. A value that does not parse is passed over, as the parser
//! passes over it.

use super::{Elements, Refused, is_drawn_element};
use resvg::usvg::roxmltree::{Document, Node};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use svgtypes::{
    FontShorthand, Length, LengthListParser, LengthUnit, PathParser, PathSegment, PointsParser,
    Transform, ViewBox,
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
/// # Errors
/// [`Refused::NotAnImage`] for anything the module refuses, and for an element
/// that comes to hold or copy itself.
pub(super) fn admitted(document: &Document<'_>) -> Result<(), Refused> {
    let elements = Elements::of(document);
    let allowed = elements.all.iter().all(|element| {
        is_drawn_element(element)
            && references_allowed(element)
            && !id_in_a_namespace(element)
            && !sets_a_view_relatively(element)
    });
    if !allowed || given_twice(&elements) || !sizes_within(&elements) {
        return Err(Refused::NotAnImage);
    }
    Ok(())
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

/// Whether every number of every element's geometry is within
/// [`MOST_MAGNITUDE`]; see the module. An element that comes to hold or copy
/// itself is not.
fn sizes_within(elements: &Elements<'_, '_>) -> bool {
    let Some(order) = elements.in_order() else {
        return false;
    };
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
    let view = elements
        .all
        .iter()
        .zip(&properties)
        .filter(|(element, _)| element.tag_name().name() == "svg")
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
            return false;
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
                        return false;
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

        if !own
            .iter()
            .all(|&(name, value)| property_within(name, value, size, view))
        {
            return false;
        }
    }
    true
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
