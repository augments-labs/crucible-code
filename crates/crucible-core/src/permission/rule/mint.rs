//! Writing a rule, which is `parse` in the other direction.
//!
//! Both spellings of the grammar live beside each other on purpose. A rule
//! minted here is read back by `parse` with no step in between, so a subject
//! that would need quoting to survive the trip is one this module has to spell
//! rather than one the reader has to guess at.

use std::fmt;

use crate::tool::ToolCall;

use super::super::{Command, Sensitivity};

/// The text of one rule, as it would be written in a configuration file.
///
/// Minted from a question, and the same value the prompt shows and the file
/// receives — so what somebody agreed to and what lands on disk cannot be two
/// strings that merely started out alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minted(Box<str>);

impl Minted {
    /// The text, for writing it down.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Minted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The narrowest rule covering exactly what a question showed, when one can be
/// written at all.
///
/// Narrowest means literal: the file that was named, the command that was
/// shown. Never the directory above one or the program behind the other. A
/// broad rule is something a person writes on purpose, having thought about
/// what it lets through; nothing here mints one on their behalf out of a single
/// yes.
///
/// `None` where no honest rule exists — a path that did not resolve, a command
/// nobody could read, or a line that is more than one command. The last is the
/// interesting one: a rule per constituent would let each of them run alone
/// from now on, which is wider than the line that was actually agreed to.
#[must_use]
pub fn narrowest(call: &ToolCall, sensitivity: &Sensitivity) -> Option<Minted> {
    let subject = match sensitivity {
        Sensitivity::ReadOnly { target } | Sensitivity::MutatesFile { target } => {
            // The spelling somebody would recognise, falling back to the only
            // name a file outside the working directory has.
            target.below_root().or_else(|| target.absolute())?
        }

        Sensitivity::SpawnsProcess {
            command: Command::Understood { parts },
        } => match &**parts {
            [one] => one,
            _ => return None,
        },

        Sensitivity::SpawnsProcess {
            command: Command::Opaque(_),
        } => return None,
    };

    Some(Minted(
        format!("{}({})", call.name, literally(subject)).into(),
    ))
}

/// A subject spelled so that it matches itself and nothing else.
///
/// The characters a glob reads are ones a filename and a command line are both
/// allowed to contain: `rm *.tmp` is a command somebody runs, and writing it
/// into a rule unescaped would mint `rm <anything>.tmp` — precisely the
/// generalisation this module exists not to make.
///
/// Whitespace at either end is the same failure by a quieter route. A file may
/// be named `" secret.txt"`, and `parse` trims what it finds inside the
/// brackets — so a leading or trailing space written plainly comes back as a
/// rule about `secret.txt`, which is a different file and one nobody was asked
/// about. Escaped, there is nothing left for the trim to take.
fn literally(subject: &str) -> String {
    let mut spelled = String::with_capacity(subject.len());

    // Where the last character starts, which is all "at an end" needs to know.
    let last = subject.char_indices().next_back().map_or(0, |(at, _)| at);

    for (at, character) in subject.char_indices() {
        let end = at == 0 || at == last;

        // A one-character class is the escape that needs no escape character,
        // so it means the same thing wherever the glob is compiled.
        if matches!(character, '*' | '?' | '[' | ']' | '{' | '}' | '\\')
            || (end && character.is_whitespace())
        {
            spelled.push('[');
            spelled.push(character);
            spelled.push(']');
        } else {
            spelled.push(character);
        }
    }

    spelled
}

#[cfg(test)]
mod tests;
