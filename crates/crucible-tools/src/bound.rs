//! How much a tool may say.
//!
//! One figure, in bytes, for every tool whose answer is a list. What a tool
//! returns goes into the next request whole, so the bound has to be the bytes:
//! a count of lines is not the same promise, and the difference is not small.
//! Two hundred matching lines of four hundred characters is eighty kilobytes
//! against the thirty `bash` holds itself to, and the caller that chose the two
//! hundred is the model.
//!
//! So the bytes bound the answer and the count a call sends is a second, smaller
//! limit on top of them — with a ceiling of its own, because a number a model
//! writes is an argument like any other and the rest of them are checked.

/// The most one tool may say, in bytes.
///
/// Where it came from is what a command's output was already held to, and one
/// answer to "how much may a tool say" is worth more than a figure fitted to
/// each tool: a turn spends its context on whichever tool it called, and a
/// budget that moves with that is not a budget.
pub(crate) const OUTPUT: usize = 30_000;

/// As much of `lines` as fits inside [`OUTPUT`], and how many were left out.
///
/// Whole lines, and in the order they arrived. A list cut mid-line hands the
/// model half a path to call the next tool with, and letting a later short line
/// take the room a long one could not would leave a gap in the middle of an
/// answer that reads as consecutive.
///
/// The rest are counted rather than quietly dropped, because a caller has to be
/// able to say in the output that there were more — a cut result reads to the
/// model as a complete one.
pub(crate) fn within(lines: impl IntoIterator<Item = String>) -> (String, usize) {
    let mut kept = String::new();
    let mut left = 0;

    for line in lines {
        if left > 0 || kept.len() + line.len() > OUTPUT {
            left += 1;
        } else {
            kept.push_str(&line);
        }
    }

    (kept, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of a known length, so a test can say how many of them fit.
    fn lines(count: usize, each: usize) -> Vec<String> {
        (0..count).map(|_| "x".repeat(each - 1) + "\n").collect()
    }

    #[test]
    fn everything_that_fits_comes_back_whole() {
        let (kept, left) = within(lines(10, 100));

        assert_eq!(kept.len(), 1_000);
        assert_eq!(left, 0);
    }

    #[test]
    fn what_does_not_fit_is_counted_rather_than_cut_in_half() {
        let (kept, left) = within(lines(1_000, 100));

        assert_eq!(kept.len(), OUTPUT);
        assert_eq!(left, 1_000 - OUTPUT / 100);
        assert!(kept.ends_with('\n'), "a line was cut through the middle");
    }

    #[test]
    fn a_line_that_did_not_fit_does_not_let_a_later_one_past() {
        // Otherwise the answer skips a line and reads as consecutive, which is
        // worse than the one it left out: nothing in the text says a gap is
        // there.
        let mut lines = lines(2, 20_000);
        lines.push("short\n".to_owned());

        let (kept, left) = within(lines);

        assert_eq!(kept.len(), 20_000);
        assert_eq!(left, 2);
    }
}
