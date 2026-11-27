//! An entity-relationship diagram's entities listed along the chains its
//! relationships make, so the engine draws each relationship between two
//! boxes it sets side by side.
//!
//! Invariants:
//!
//! - The engine sets entities left to right in the order they are listed: in
//!   one row while that fits its room, and past it in rows as many as the
//!   square root of their count, rounded up. It draws each relationship as a
//!   straight line along the row from one end to the other, so a line
//!   between two entities not side by side runs through every box between
//!   them and erases its name. So the entities are listed chain by chain,
//!   each chain from its end listed first, and a diagram whose relationships
//!   do not make chains is refused whole: an entity related to itself, two
//!   relationships between one pair, an entity in three relationships, or a
//!   ring.
//! - A line between two rows the engine runs down the right side, past
//!   every box, from the end of the upper row to the lower: it reaches the
//!   boxes whole only where each is the last in its row, and a box after
//!   either in its row has its name run through. It sets that line's label
//!   under the upper box, ending beside the side, so the label clears every
//!   box where it is no wider than the upper box's name with its padding and
//!   sides. So the engine is handed its room only when every row it would end
//!   inside a chain ends before the last entity, set alone in the last row,
//!   with a label that clears every box; otherwise it is handed none, sets
//!   every entity in one row, and a row wider than the room is refused rather
//!   than broken.

use std::collections::BTreeMap;

use crucible_tui::columns;
use mermaid_text::ErDiagram;

/// The columns the engine sets on each side of an entity's name, its padding
/// and side together.
const NAME_SIDE: usize = 3;

/// `diagram` with its entities listed along the chains its relationships
/// make, and the room to hand the engine for it in `room` columns; `None`
/// when its relationships do not make chains.
pub(super) fn chained(mut diagram: ErDiagram, room: usize) -> Option<(ErDiagram, usize)> {
    let count = diagram.entities.len();
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); count];
    // Each pair related, and its label's width.
    let mut pairs = BTreeMap::new();
    for relationship in &diagram.relationships {
        let from = diagram.entity_index(&relationship.from)?;
        let to = diagram.entity_index(&relationship.to)?;
        let label = relationship.label.as_deref().map_or(0, columns);
        if from == to || pairs.insert((from.min(to), from.max(to)), label).is_some() {
            return None;
        }
        neighbours.get_mut(from)?.push(to);
        neighbours.get_mut(to)?.push(from);
    }
    if neighbours.iter().any(|near| near.len() > 2) {
        return None;
    }
    let order = ordered(&neighbours)?;

    // Should the engine need more than one row, each is as wide as the
    // square root of the entities' count, rounded up. Each relationship joins
    // two entities next to each other in the order, and turns down the right
    // side where the first of them ends a row.
    let mut position = vec![0; count];
    for (at, &entity) in order.iter().enumerate() {
        if let Some(entity) = position.get_mut(entity) {
            *entity = at;
        }
    }
    let across = (count.isqrt() + usize::from(count.isqrt().pow(2) < count)).max(1);
    let name = |entity: usize| {
        diagram
            .entities
            .get(entity)
            .map_or(0, |entity| columns(&entity.name))
    };
    let unbroken = pairs.iter().all(|(&(one, other), &label)| {
        let (Some(&one_at), Some(&other_at)) = (position.get(one), position.get(other)) else {
            return false;
        };
        let (upper, upper_at) = if one_at < other_at {
            (one, one_at)
        } else {
            (other, other_at)
        };
        upper_at % across != across - 1
            || upper_at + 2 == count && label <= name(upper) + 2 * NAME_SIDE
    });

    let mut entities: Vec<_> = diagram.entities.into_iter().map(Some).collect();
    diagram.entities = order
        .iter()
        .filter_map(|&at| entities.get_mut(at).and_then(Option::take))
        .collect();
    Some((diagram, if unbroken { room } else { usize::MAX }))
}

/// Every entity, by its place in the diagram's list, chain by chain in the
/// order each chain's first entity is listed, each chain from whichever of
/// its ends is listed first; `None` when one is a ring.
fn ordered(neighbours: &[Vec<usize>]) -> Option<Vec<usize>> {
    let mut placed = vec![false; neighbours.len()];
    let mut order = Vec::with_capacity(neighbours.len());
    for start in 0..neighbours.len() {
        if placed.get(start).copied().unwrap_or(true) {
            continue;
        }
        let end = *walked(neighbours, start)?.last()?;
        let mut chain = walked(neighbours, end)?;
        if chain.last() < chain.first() {
            chain.reverse();
        }
        for &entity in &chain {
            if let Some(entity) = placed.get_mut(entity) {
                *entity = true;
            }
        }
        order.extend(chain);
    }
    Some(order)
}

/// The entities met walking from `from`, each step away from the one before,
/// to the end of its chain; `None` when the walk comes back round to `from`.
fn walked(neighbours: &[Vec<usize>], from: usize) -> Option<Vec<usize>> {
    let mut chain = vec![from];
    let mut before = None;
    let mut at = from;
    while let Some(&next) = neighbours
        .get(at)?
        .iter()
        .find(|&&near| Some(near) != before)
    {
        if next == from || chain.len() >= neighbours.len() {
            return None;
        }
        chain.push(next);
        before = Some(at);
        at = next;
    }
    Some(chain)
}
