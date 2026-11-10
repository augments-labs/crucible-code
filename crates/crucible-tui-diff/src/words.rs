//! Which words of a changed line changed, against the line it replaced.
//!
//! A line is cut into runs of spaces and runs of everything else, and the two
//! lines' runs are compared. A word replaced by one word is narrowed to the
//! part that differs, cut only where a word may be (never between two
//! letters), so `input.lines()` becoming `input.lines().enumerate()`
//! raises `.enumerate()` and not the call before it. Two lines with no word in
//! common, and no word that kept most of itself, are a rewrite rather than an edit, and nothing on either is raised:
//! a line raised end to end says less than one that is not.
//!
//! The work is bounded twice: a line of more than [`TOKENS`] runs is not
//! compared, and one change compares at most [`PAIRS`] pairs of lines. Both
//! are counts rather than clocks, so the same change is raised the same way
//! on every machine.

use std::ops::Range;

use similar::{Algorithm, DiffTag, capture_diff_slices};

/// The most runs a line is cut into and still compared.
pub const TOKENS: usize = 48;

/// The most pairs of lines one change compares.
pub const PAIRS: usize = 8;

/// The byte ranges raised on the old line and on the new one.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Raised {
    pub(crate) old: Vec<Range<usize>>,
    pub(crate) new: Vec<Range<usize>>,
}

/// What changed between `old` and `new`, or `None` where they are not
/// compared: too long, or nothing but spaces in common.
pub(crate) fn raised(old: &str, new: &str) -> Option<Raised> {
    let old_runs = runs(old)?;
    let new_runs = runs(new)?;
    let old_words: Vec<&str> = old_runs
        .iter()
        .filter_map(|run| old.get(run.clone()))
        .collect();
    let new_words: Vec<&str> = new_runs
        .iter()
        .filter_map(|run| new.get(run.clone()))
        .collect();

    let mut raised = Raised::default();
    let mut shared = false;

    for op in capture_diff_slices(Algorithm::Myers, &old_words, &new_words) {
        let (tag, olds, news) = op.as_tag_tuple();
        let old_span = span(&old_runs, &olds);
        let new_span = span(&new_runs, &news);
        match tag {
            DiffTag::Equal => {
                shared |= old_words
                    .get(olds)
                    .is_some_and(|words| words.iter().any(|word| !word.trim().is_empty()));
            }
            DiffTag::Delete => push(old, &mut raised.old, old_span),
            DiffTag::Insert => push(new, &mut raised.new, new_span),
            DiffTag::Replace if olds.len() == 1 && news.len() == 1 => {
                let (old_part, new_part) = narrowed(old, new, old_span.clone(), new_span);
                // A word that kept more than half of itself at its ends is
                // the same word edited, which is something in common.
                let kept = old_span.len() - old_part.len();
                shared |= kept * 2 > old_span.len();
                push(old, &mut raised.old, old_part);
                push(new, &mut raised.new, new_part);
            }
            DiffTag::Replace => {
                push(old, &mut raised.old, old_span);
                push(new, &mut raised.new, new_span);
            }
        }
    }

    if !shared {
        return None;
    }
    raised.old = merged(old, raised.old);
    raised.new = merged(new, raised.new);
    Some(raised)
}

/// `line` cut into runs of spaces and runs of the rest, or `None` past
/// [`TOKENS`] of them.
fn runs(line: &str) -> Option<Vec<Range<usize>>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut spacing = None;
    for (at, character) in line.char_indices() {
        let space = character.is_whitespace();
        let end = at + character.len_utf8();
        match runs.last_mut() {
            Some(run) if spacing == Some(space) => run.end = end,
            _ => {
                if runs.len() == TOKENS {
                    return None;
                }
                runs.push(at..end);
                spacing = Some(space);
            }
        }
    }
    Some(runs)
}

/// The bytes the runs at `which` cover.
fn span(runs: &[Range<usize>], which: &Range<usize>) -> Range<usize> {
    let start = runs.get(which.start).map_or(0, |run| run.start);
    let end = which
        .end
        .checked_sub(1)
        .and_then(|last| runs.get(last))
        .map_or(start, |run| run.end);
    start..end.max(start)
}

/// One word replaced by one: only the part between what the two share at
/// either end.
fn narrowed(
    old: &str,
    new: &str,
    old_span: Range<usize>,
    new_span: Range<usize>,
) -> (Range<usize>, Range<usize>) {
    let (Some(old_word), Some(new_word)) = (old.get(old_span.clone()), new.get(new_span.clone()))
    else {
        return (old_span, new_span);
    };
    let before = shared_start(old_word, new_word);
    let old_rest = old_word.get(before..).unwrap_or_default();
    let new_rest = new_word.get(before..).unwrap_or_default();
    let after = shared_end(old_rest, new_rest);
    (
        old_span.start + before..old_span.end - after,
        new_span.start + before..new_span.end - after,
    )
}

/// The bytes two words share at their start, ending where a word may: a
/// part of a word is not raised, so `columns` against `rows` is the whole word.
fn shared_start(old: &str, new: &str) -> usize {
    let mut shared = 0;
    let mut kept = 0;
    let mut olds = old.chars().peekable();
    let mut news = new.chars().peekable();
    while let (Some(a), Some(b)) = (olds.peek().copied(), news.peek().copied()) {
        if a != b {
            break;
        }
        olds.next();
        news.next();
        shared += a.len_utf8();
        if parts(Some(a), olds.peek().copied()) && parts(Some(b), news.peek().copied()) {
            kept = shared;
        }
    }
    kept
}

/// The bytes two words share at their end, starting where a word may.
fn shared_end(old: &str, new: &str) -> usize {
    let mut shared = 0;
    let mut kept = 0;
    let mut olds = old.chars().rev().peekable();
    let mut news = new.chars().rev().peekable();
    while let (Some(a), Some(b)) = (olds.peek().copied(), news.peek().copied()) {
        if a != b {
            break;
        }
        olds.next();
        news.next();
        shared += a.len_utf8();
        if parts(olds.peek().copied(), Some(a)) && parts(news.peek().copied(), Some(b)) {
            kept = shared;
        }
    }
    kept
}

/// Whether a word may be cut between `before` and `after`: at either end of
/// it, or where either side is not a letter, a digit or an underscore.
fn parts(before: Option<char>, after: Option<char>) -> bool {
    let wordy = |character: char| character.is_alphanumeric() || character == '_';
    match (before, after) {
        (Some(before), Some(after)) => !wordy(before) || !wordy(after),
        _ => true,
    }
}

/// Adds `range` of `line`, without the spaces at either end of it.
fn push(line: &str, raised: &mut Vec<Range<usize>>, range: Range<usize>) {
    let Some(text) = line.get(range.clone()) else {
        return;
    };
    let start = range.start + (text.len() - text.trim_start().len());
    let end = range.end - (text.len() - text.trim_end().len());
    if start < end {
        raised.push(start..end);
    }
}

/// `ranges` with any two that only spaces part made one.
fn merged(line: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && line
                .get(last.end..range.start)
                .is_some_and(|between| between.chars().all(char::is_whitespace))
        {
            last.end = range.end;
            continue;
        }
        merged.push(range);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(line: &str, ranges: &[Range<usize>]) -> Vec<String> {
        ranges
            .iter()
            .map(|range| line.get(range.clone()).unwrap_or("?").to_owned())
            .collect()
    }

    #[test]
    fn a_word_replaced_by_several_is_raised_whole_on_both_sides() {
        let (old, new) = ("for line in x {", "for (at, line) in x {");
        let raised = raised(old, new).expect("compared");
        assert_eq!(shown(old, &raised.old), ["line"]);
        assert_eq!(shown(new, &raised.new), ["(at, line)"]);
    }

    #[test]
    fn a_word_replaced_by_one_is_narrowed_to_what_differs() {
        let (old, new) = ("    tree", "    tree.finish()");
        let raised = raised(old, new).expect("compared");
        assert!(raised.old.is_empty());
        assert_eq!(shown(new, &raised.new), [".finish()"]);
    }

    #[test]
    fn lines_with_only_spaces_in_common_are_not_compared() {
        assert_eq!(raised("alpha beta", "gamma delta"), None);
    }

    #[test]
    fn a_line_of_too_many_words_is_not_compared() {
        let long = "a ".repeat(TOKENS);
        assert_eq!(raised(&long, "a"), None);
    }

    #[test]
    fn words_in_another_script_are_cut_on_character_boundaries() {
        let (old, new) = ("let é = ünï;", "let é = üna;");
        let raised = raised(old, new).expect("compared");
        assert_eq!(shown(old, &raised.old), ["ünï"]);
        assert_eq!(shown(new, &raised.new), ["üna"]);
    }

    #[test]
    fn part_of_a_word_is_not_raised() {
        let (old, new) = ("let a = columns / 2;", "let a = rows / 2;");
        let raised = raised(old, new).expect("compared");
        assert_eq!(shown(old, &raised.old), ["columns"]);
        assert_eq!(shown(new, &raised.new), ["rows"]);
    }

    #[test]
    fn a_word_added_is_raised_without_the_space_after_it() {
        let (old, new) = ("line of the file", "line of the changed file");
        let raised = raised(old, new).expect("compared");
        assert!(raised.old.is_empty());
        assert_eq!(shown(new, &raised.new), ["changed"]);
    }
}
