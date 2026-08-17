//! Colour, asked for by the job it does rather than by the hue it is.
//!
//! Nothing anywhere names a colour. It names a [`Slot`] — the accent, the
//! quiet, the mode in force — and a [`Palette`] settles once, at startup, what
//! a slot is worth on this terminal. That is what makes a theme a later release
//! rather than a later rewrite: a theme replaces what a slot resolves to and
//! never who asks for it.
//!
//! **The ground behind a row belongs to the reader.** An inline renderer draws
//! into the terminal's own buffer, so the ground behind every row is the one
//! the reader chose, and it stays that way by emitting no background attribute
//! at all — not by detecting theirs. That is also why one palette serves every
//! terminal: asking would mean a blocking round-trip on a startup path budgeted
//! at 20 ms, for one hue, so instead every colour here clears 3:1 against a
//! black ground *and* a white one, and the test at the bottom is where that is
//! checked.
//!
//! Quiet is the exception that proves it: bright black at every rung, never a
//! dimmed accent. "Legible but subdued on whatever this is" is a judgement only
//! the terminal's own theme has made, so it is the one colour worth deferring
//! to rather than computing.
//!
//! A diff is the one thing that takes the ground, and it takes it the only way
//! that is safe: a slot painting a ground paints its ink in the same sequence,
//! so the pair is a pair this palette chose whole and the reader's theme is
//! never the other half of a contrast nobody checked. Which is what the bottom
//! of the file checks about them instead — 3:1 against the ground they carry,
//! rather than against black and white, because a row that has taken the ground
//! has none of the reader's left behind it to clear.

/// What a colour is asked for by.
///
/// Closed on purpose. A slot is a decision about what the interface means, and
/// a new one should have to be given a value at every rung before it compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The reader's own foreground. Most of what is drawn is this.
    Plain,
    /// Borders, the prompt mark, the rules that separate one thing from
    /// another.
    Accent,
    /// The accent, emphasised: the product's name, and a command's name.
    Strong,
    /// Present but secondary — a hint, a timestamp, a label.
    Quiet,
    /// The mode that lets a file be written without asking.
    AllowEdits,
    /// The mode that asks about nothing at all.
    FullAccess,
    /// A line a change took out, on a ground of its own.
    Removed,
    /// That line's number, and the sign beside it, on the same ground.
    ///
    /// The sign travels with the number because the two are one column group:
    /// a reader finds the line, then reads which way it went, and the gutter is
    /// where both of those live.
    RemovedNumber,
    /// A line a change put in, on a ground of its own.
    Added,
    /// That line's number and sign, on the same ground.
    AddedNumber,
}

/// One slot's answer at each rung of the ladder.
///
/// Whole sequences rather than components, because the alternative is
/// formatting one per span per frame, and the render path may not allocate.
#[derive(Debug, Clone, Copy)]
struct Ink {
    /// Twenty-four bit, for a terminal that says it can take it.
    exact: &'static str,
    /// The nearest of the two hundred and fifty-six indexed colours.
    indexed: &'static str,
    /// One of the sixteen every terminal has always had.
    basic: &'static str,
}

/// Back to whatever the terminal was doing before the row started.
const RESET: &str = "\x1b[0m";

/// The one colour the terminal's own theme chooses, at every rung.
const QUIET: Ink = Ink {
    exact: "\x1b[90m",
    indexed: "\x1b[90m",
    basic: "\x1b[90m",
};

/// Nothing at all, at every rung.
const NONE: Ink = Ink {
    exact: "",
    indexed: "",
    basic: "",
};

impl Slot {
    /// What this slot is worth, at each rung.
    const fn ink(self) -> Ink {
        match self {
            Self::Plain => NONE,
            Self::Accent => Ink {
                exact: "\x1b[38;2;18;137;127m",
                indexed: "\x1b[38;5;30m",
                basic: "\x1b[36m",
            },
            // The accent again, emphasised rather than lightened: a terminal
            // brightens a bold colour on its own, and doing it here as well
            // pushes the result off the light ground it also has to work on.
            Self::Strong => Ink {
                exact: "\x1b[1;38;2;18;137;127m",
                indexed: "\x1b[1;38;5;30m",
                basic: "\x1b[1;36m",
            },
            Self::Quiet => QUIET,
            Self::AllowEdits => Ink {
                exact: "\x1b[38;2;53;145;90m",
                indexed: "\x1b[38;5;65m",
                basic: "\x1b[32m",
            },
            Self::FullAccess => Ink {
                exact: "\x1b[38;2;176;132;22m",
                indexed: "\x1b[38;5;136m",
                basic: "\x1b[33m",
            },
            // Ground and ink in one sequence, so neither can be written without
            // the other. The four below are the whole of the exception in this
            // file, and each pair clears 3:1 on itself.
            Self::Removed => Ink {
                exact: "\x1b[48;2;74;26;30;38;2;255;199;202m",
                indexed: "\x1b[48;5;52;38;5;224m",
                basic: "\x1b[41;97m",
            },
            // A shade nearer the ground's own hue, so the gutter reads as the
            // edge of the block rather than as more of the line. At sixteen
            // colours there is no such shade to be had, so the emphasis is
            // weight instead — the meaning is in the ground either way.
            Self::RemovedNumber => Ink {
                exact: "\x1b[48;2;74;26;30;38;2;255;138;145m",
                indexed: "\x1b[48;5;52;38;5;210m",
                basic: "\x1b[1;41;97m",
            },
            Self::Added => Ink {
                exact: "\x1b[48;2;19;61;36;38;2;198;245;214m",
                indexed: "\x1b[48;5;22;38;5;194m",
                basic: "\x1b[42;97m",
            },
            Self::AddedNumber => Ink {
                exact: "\x1b[48;2;19;61;36;38;2;126;226;160m",
                indexed: "\x1b[48;5;22;38;5;114m",
                basic: "\x1b[1;42;97m",
            },
        }
    }
}

/// How much colour this terminal will take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// Twenty-four bit, said so by `COLORTERM`.
    Exact,
    /// Two hundred and fifty-six, said so by `TERM`.
    Indexed,
    /// Sixteen, which is what a terminal saying nothing in particular has.
    Basic,
    /// None: a pipe, `NO_COLOR`, `--color never`, or `TERM=dumb`.
    Off,
}

/// The variable a terminal announces twenty-four bit colour in.
const COLORTERM: &str = "COLORTERM";

/// The variable naming the terminal's type.
const TERM: &str = "TERM";

/// What a slot is worth on the terminal this run is attached to.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    depth: Depth,
}

impl Palette {
    /// Settles the ladder once, from the terminal and the environment.
    ///
    /// `color` is whether to write any at all, which the configuration, the
    /// environment and `is_terminal` have already agreed on between them —
    /// this decides only how much. `from` reads the environment as a parameter
    /// because writing to the real one is `unsafe` in edition 2024 and this
    /// workspace forbids it.
    #[must_use]
    pub fn resolve(color: bool, from: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            depth: if color { Self::depth(from) } else { Depth::Off },
        }
    }

    /// A palette that writes no escape bytes at all.
    #[must_use]
    pub fn plain() -> Self {
        Self { depth: Depth::Off }
    }

    /// The sequence that starts `slot`, or nothing when there is no colour.
    #[must_use]
    pub fn open(self, slot: Slot) -> &'static str {
        let ink = slot.ink();

        match self.depth {
            Depth::Exact => ink.exact,
            Depth::Indexed => ink.indexed,
            Depth::Basic => ink.basic,
            Depth::Off => "",
        }
    }

    /// The sequence that ends any slot, or nothing when there is no colour.
    ///
    /// A row that ends without this leaves its attribute set on every row after
    /// it, including the shell prompt this process eventually returns to.
    #[must_use]
    pub fn close(self) -> &'static str {
        match self.depth {
            Depth::Off => "",
            _ => RESET,
        }
    }

    /// Whether anything at all is written.
    #[must_use]
    pub fn writes_color(self) -> bool {
        self.depth != Depth::Off
    }

    /// How far up the ladder this terminal goes.
    fn depth(from: &dyn Fn(&str) -> Option<String>) -> Depth {
        // The only two values anyone sets it to, and the only reason it exists.
        if from(COLORTERM).is_some_and(|set| set == "truecolor" || set == "24bit") {
            return Depth::Exact;
        }

        match from(TERM) {
            // A terminal that says it is dumb is telling the truth about it.
            Some(term) if term == "dumb" => Depth::Off,
            Some(term) if term.contains("256color") => Depth::Indexed,
            Some(_) => Depth::Basic,
            // Unset. Something is reading this that is not a terminal type, and
            // sixteen colours is still a guess about a stranger.
            None => Depth::Off,
        }
    }
}

#[cfg(test)]
mod tests;
