//! Working out what a call did to a file, from the two versions of it.
//!
//! A tool that rewrote a file is the only thing that ever holds both, and it
//! holds them for the length of one call. So this is where the change is read
//! off, once, on the way out.
//!
//! What it looks for is the one place the two versions stop agreeing. Lines are
//! matched from the top until they differ and from the bottom until they differ,
//! and whatever is left between those two marks is what moved. That is the whole
//! of the algorithm, and it is enough because of what these tools are: a
//! replacement of exact text, or a file put down whole. Neither produces the
//! case a general line diff exists for — several changes far apart, with matched
//! lines between them that a prefix scan would report as changed.
//!
//! Where a call does make several distant changes at once, this says so as one
//! block reaching from the first to the last, with the lines between carried as
//! [`Change::Kept`] rather than claimed as changed. So the counts stay true and
//! the block gets long, which [`Diff`] then bounds and says it bounded. A wrong
//! count would be worth more code than this; a long block is not.
//!
//! The scan allocates nothing, walking both versions with iterators over text a
//! tool already holds. A line is copied only as it is handed to [`Diff`], which
//! frees the ones past its bound, so a whole-file rewrite costs a line at a time.

use crucible_core::{Change, Diff, Line};

/// How many unchanged lines are kept on each side of what moved.
///
/// Three, because a changed line is read against the ones around it and a reader
/// scrolled past the call needs to recognise where in the file this happened.
/// More than that and the block stops being an excerpt.
const CONTEXT: usize = 3;

/// What changed between two versions of one file.
///
/// Line numbers are the ones the reader would find these lines at: a removed
/// line carries its number in the file as it was, and everything else carries
/// its number in the file as it now is. That is why the two sides of a change
/// can print the same number and why the lines below it do not continue from
/// either — the file got longer or shorter, and the numbers say so.
pub(crate) fn between(before: &str, after: &str) -> Diff {
    let (was, now) = (before.lines().count(), after.lines().count());

    let head = before
        .lines()
        .zip(after.lines())
        .take_while(|(old, new)| old == new)
        .count();

    // Bounded by what the scan from the top did not already claim, so a file
    // whose whole content repeats cannot have one line counted as both.
    let room = was.min(now).saturating_sub(head);
    let tail = before
        .lines()
        .rev()
        .zip(after.lines().rev())
        .take_while(|(old, new)| old == new)
        .take(room)
        .count();

    let removed = was.saturating_sub(head).saturating_sub(tail);
    let added = now.saturating_sub(head).saturating_sub(tail);
    if removed == 0 && added == 0 {
        return Diff::new([]);
    }

    let above = head.saturating_sub(CONTEXT);
    let below = head.saturating_add(added);

    let leading = numbered(after, above, head.saturating_sub(above), Change::Kept);
    let gone = numbered(before, head, removed, Change::Removed);
    let came = numbered(after, head, added, Change::Added);
    let trailing = numbered(after, below, CONTEXT.min(tail), Change::Kept);

    Diff::new(leading.chain(gone).chain(came).chain(trailing))
}

/// `count` lines of `text` from `skip`, each marked `change` and carrying the
/// number it sits at in the file it was taken from, counted from one.
fn numbered(
    text: &str,
    skip: usize,
    count: usize,
    change: Change,
) -> impl Iterator<Item = Line> + '_ {
    text.lines()
        .skip(skip)
        .take(count)
        .enumerate()
        .map(move |(at, line)| Line::new(skip.saturating_add(at).saturating_add(1), change, line))
}

#[cfg(test)]
mod tests;
