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
//!   them and erases its name, and a line between two rows is broken. So the
//!   entities are listed chain by chain, each chain from its end listed
//!   first, and a diagram whose relationships do not make chains is refused
//!   whole: an entity related to itself, two relationships between one pair,
//!   an entity in three relationships, or a ring.
//! - The engine is handed its room only when no row it would end falls
//!   inside a chain; otherwise it is handed none, sets every entity in one
//!   row, and a row wider than the room is refused rather than broken.

use std::collections::BTreeSet;

use mermaid_text::ErDiagram;

/// `diagram` with its entities listed along the chains its relationships
/// make, and the room to hand the engine for it in `room` columns; `None`
/// when its relationships do not make chains.
pub(super) fn chained(mut diagram: ErDiagram, room: usize) -> Option<(ErDiagram, usize)> {
    let count = diagram.entities.len();
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut pairs = BTreeSet::new();
    for relationship in &diagram.relationships {
        let from = diagram.entity_index(&relationship.from)?;
        let to = diagram.entity_index(&relationship.to)?;
        if from == to || !pairs.insert((from.min(to), from.max(to))) {
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
    // two entities next to each other in the order, and is broken where the
    // first of them ends a row.
    let mut position = vec![0; count];
    for (at, &entity) in order.iter().enumerate() {
        if let Some(entity) = position.get_mut(entity) {
            *entity = at;
        }
    }
    let across = (count.isqrt() + usize::from(count.isqrt().pow(2) < count)).max(1);
    let unbroken = pairs.iter().all(|&(one, other)| {
        position
            .get(one)
            .zip(position.get(other))
            .is_some_and(|(one, other)| one.min(other) % across != across - 1)
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
