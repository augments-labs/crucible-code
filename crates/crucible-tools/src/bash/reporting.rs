//! Which command lines only report what they found.
//!
//! The transcript folds a run of calls that only looked around into one line
//! saying how many there were. A command is the hard case: `ls` looks and `rm`
//! does not, and the difference is in the text the model sent rather than in
//! the tool. So the question is asked of the line, here, once.
//!
//! It is deliberately narrow. Everything this cannot read is *not* reporting,
//! because the cost of the two mistakes is not the same: a reporting line named
//! in the transcript costs a reader one row they did not need, and a writing
//! line folded into a count is a change to their workspace they never saw go
//! past. Where the two answers are equally defensible, this one picks the row.
//!
//! It is not a permission decision and cannot become one. Nothing here is
//! consulted before a command runs; the sandbox and the permission engine
//! settle what may run, and this settles only how the row is drawn afterwards.

use super::{command, wrapper};

/// Programs that report whatever flags they are given.
///
/// A program earns a place here by having no way to write from its own
/// arguments — the shell's redirections are already refused by the scanner, so
/// the only remaining question is whether the program itself takes a
/// destination. That is why `sed`, `find`, `sort` and `awk` are absent: `-i`,
/// `-delete`, `-o` and a `print >` inside the script all write, and a table
/// that has to know about them is a table that will one day be wrong about
/// them. The one print-only `sed` form supported below is checked separately.
const REPORTS: &[&str] = &[
    "basename", "cat", "df", "dirname", "du", "echo", "egrep", "false", "fgrep", "file", "grep",
    "head", "hostname", "id", "jq", "ls", "nl", "printf", "pwd", "readlink", "realpath", "rg",
    "stat", "tail", "true", "type", "uname", "uniq", "wc", "which", "whoami",
];

/// Invocations that report, spelled as the words that open them.
///
/// A program is read here rather than in [`REPORTS`] when the program itself
/// may do either and only what follows says which: `gh pr view` reads a pull
/// request and `gh pr create` opens one, so the answer is per subcommand and
/// never per binary. Longer prefixes are as welcome as short ones — what is
/// matched is the opening words of the command, so `gh pr` alone would be a
/// claim about `gh pr create` too, and is not made.
///
/// Sorted, so that a reader looking for a subcommand finds it or finds it
/// missing.
const REPORTING: &[&str] = &[
    "cargo metadata",
    "cargo tree",
    "gh issue list",
    "gh issue view",
    "gh pr checks",
    "gh pr diff",
    "gh pr list",
    "gh pr status",
    "gh pr view",
    "gh release list",
    "gh release view",
    "gh repo view",
    "gh run list",
    "gh run view",
    "gh workflow list",
    "gh workflow view",
    "git blame",
    "git cat-file",
    "git describe",
    "git diff",
    "git log",
    "git ls-files",
    "git ls-remote",
    "git rev-parse",
    "git shortlog",
    "git show",
    "git status",
];

/// Whether this command line only reports what it found.
pub(super) fn only(line: &str) -> bool {
    command::parts(line, |program| {
        matches!(program.rsplit('/').next(), Some("cd" | "sed")) || !wrapper::wraps(program)
    })
    .is_some_and(|parts| !parts.is_empty() && parts.iter().all(|part| reports(part)))
}

/// Whether one simple command reports.
fn reports(part: &str) -> bool {
    let program = part.split(' ').next().unwrap_or_default();

    // `/usr/bin/ls` and `ls` are the same program, and the scanner has already
    // refused everything that could make the spelling a lie.
    let program = program.rsplit('/').next().unwrap_or(program);

    program == "cd"
        || (program == "sed" && prints(part))
        || REPORTS.contains(&program)
        || REPORTING.iter().any(|reporting| {
            let rest = match reporting.split_once(' ') {
                Some((named, rest)) if named == program => rest,
                Some(_) | None => return false,
            };

            // The words after the program, matched whole: `gh pr view` must not
            // be found inside `gh pr viewers`, and must be found in `gh pr
            // view 487`.
            let after = part.split_once(' ').map(|(_, after)| after);
            after.is_some_and(|after| {
                after == rest
                    || after
                        .strip_prefix(rest)
                        .is_some_and(|next| next.starts_with(' '))
            })
        })
}

/// Only `sed -n 'N[,M]p' paths...`: a line range printed without any other
/// script or option. This is display classification, never permission to run.
fn prints(part: &str) -> bool {
    let mut words = part.split_whitespace();
    words.next();
    if words.next() != Some("-n") {
        return false;
    }
    let Some(script) = words.next() else {
        return false;
    };
    let script = script
        .strip_prefix('\'')
        .and_then(|text| text.strip_suffix('\''))
        .or_else(|| {
            script
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
        })
        .unwrap_or(script);
    let Some(range) = script.strip_suffix('p') else {
        return false;
    };
    let mut ends = range.split(',');
    let number = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !ends.next().is_some_and(number) || !ends.next().is_none_or(number) || ends.next().is_some()
    {
        return false;
    }
    // Options after the script can still edit a file or load another script.
    // Quoted or escaped option words are declined as well.
    words.all(|word| !word.trim_start_matches(['\'', '"', '\\']).starts_with('-'))
}

#[cfg(test)]
mod tests;
