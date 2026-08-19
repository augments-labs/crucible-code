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
//! the reader chose. Almost nothing here emits a background attribute at all,
//! and the two things that do are covered below.
//!
//! **A theme is a table of hues, and it is tuned to one ground.** There used to
//! be one palette for every terminal, and every colour in it cleared 3:1
//! against black *and* white so that it could be. A theme is the admission that
//! this bought less than it cost: a colour that has to work on both grounds is
//! a colour at its best on neither. So each table now clears **4.5:1 against
//! the ground it is for**, which is strictly more contrast where it is actually
//! used, and the test at the bottom is where that is checked — per theme,
//! against that theme's own ground.
//!
//! Which ground the reader is on is a question this can now answer, because
//! [`crate::ground`] answers it without blocking: a variable read at startup,
//! and a reply that arrives later or never. Nothing waits for it, and `auto` is
//! what turns the answer into a table.
//!
//! Quiet is the exception that proves it: bright black at every rung, never a
//! dimmed accent. "Legible but subdued on whatever this is" is a judgement only
//! the terminal's own theme has made, so it is the one colour worth deferring
//! to rather than computing.
//!
//! Two slots carry an attribute and no hue at all, which is the same deferral
//! read the other way: weight and a line through the text are the reader's own
//! foreground, moved, and a foreground that is already legible on their ground
//! is still legible bolder. A terminal that draws neither loses the emphasis
//! and keeps the words, which is why nothing is ever said by weight alone.
//!
//! A diff is one of two things that take the ground, and it takes it the only
//! way that is safe: a slot painting a ground paints its ink in the same
//! sequence, so the pair is a pair this palette chose whole and the reader's
//! theme is never the other half of a contrast nobody checked. Which is what
//! the bottom of the file checks about them instead — against the ground they
//! carry, rather than against black and white, because a row that has taken the
//! ground has none of the reader's left behind it to clear.
//!
//! The row the reader's own prompt is on is the other, and it is the exception
//! to the exception: its ground is not chosen here at all. It is *their* ground,
//! blended a fixed step — lighter on a dark terminal, darker on a light one —
//! so it is never a colour this file picked and can never fight a terminal
//! theme nobody here has seen. That is also why the words on it stay the
//! reader's own foreground: a step that small leaves a foreground they already
//! chose for that ground exactly as legible as it was. Only the mark takes an
//! ink, and it takes the accent, on the same ground, in one sequence.
//!
//! It follows that this one value cannot be a constant. It is worked out once,
//! when the palette is settled, from a colour the terminal only reveals at
//! runtime — so it is held in the palette rather than in the table, inline and
//! fixed-width, because a palette is `Copy` and is passed by value into every
//! row that gets painted.

mod derived;

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
    /// The one task a plan is under way on.
    ///
    /// Weight rather than a hue, because the panel already spends its one warm
    /// colour on the mark beside this. A row that took a second hue would be
    /// two things asking to be looked at on a picture whose whole job is to
    /// answer that in one.
    Doing,
    /// That task's mark, and the only warm colour on the screen.
    DoingMark,
    /// A task a plan has finished with.
    ///
    /// Struck through and subdued together: behind you, still legible,
    /// deliberately easy to slide over.
    Done,
    /// That task's mark.
    DoneMark,
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
    /// The row the reader's own prompt is left on, once it has been asked.
    ///
    /// A ground and no ink. The ground is the reader's, blended a fixed step,
    /// and the words on it are the reader's own foreground for the reason the
    /// module doc gives. Nothing at all where no ground is known — which is a
    /// state the prompt row is drawn correctly in rather than a failure.
    Prompt,
    /// The mark before it, on that same ground.
    ///
    /// The one ink the band carries, and the accent, so the mark reads as the
    /// same mark it is everywhere else.
    PromptMark,
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

/// Which table of hues is in force.
///
/// A theme changes what a slot is worth and never who asks for it, which is why
/// nothing outside this file names one. `Auto` is not here: it is a question
/// about the terminal, answered before a palette exists, and by the time one
/// does it has already become one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// For a terminal with a dark ground.
    Dark,
    /// For a terminal with a light one.
    Light,
    /// Dark, with the diff off the red-green axis: what a change put in goes
    /// blue and what it took out goes amber.
    ColourblindDark,
    /// The same swap, for a light ground.
    ColourblindLight,
    /// The sixteen the terminal has always had, and nothing else — so the
    /// reader's own terminal theme decides every hue on the screen.
    Ansi,
}

/// The hues one theme spends.
///
/// Only the ones a theme actually changes. The reader's own foreground, the
/// terminal's own quiet, weight and a line through the text are the same in
/// every table by design, and repeating them four times would be four places
/// for one of them to drift.
#[derive(Debug, Clone, Copy)]
struct Tones {
    /// Borders, marks, rules.
    accent: Ink,
    /// The accent, emphasised.
    strong: Ink,
    /// The narrowest permission mode, and a task a plan has finished with.
    green: Ink,
    /// The widest permission mode, and the task under way.
    amber: Ink,
    /// A line a change took out.
    removed: Ink,
    /// Its number and sign.
    removed_number: Ink,
    /// A line a change put in.
    added: Ink,
    /// Its number and sign.
    added_number: Ink,
}

/// For a dark ground. Every value here is the one this project shipped before
/// there were themes, unchanged: they were picked by hand and checked, and
/// recomputing them would replace a decision with arithmetic.
const DARK: Tones = Tones {
    accent: Ink {
        exact: "\x1b[38;2;18;137;127m",
        indexed: "\x1b[38;5;30m",
        basic: "\x1b[36m",
    },
    // The accent again, emphasised rather than lightened: a terminal brightens
    // a bold colour on its own, and doing it here as well pushes the result off
    // the light ground it also has to work on.
    strong: Ink {
        exact: "\x1b[1;38;2;18;137;127m",
        indexed: "\x1b[1;38;5;30m",
        basic: "\x1b[1;36m",
    },
    green: Ink {
        exact: "\x1b[38;2;53;145;90m",
        indexed: "\x1b[38;5;65m",
        basic: "\x1b[32m",
    },
    amber: Ink {
        exact: "\x1b[38;2;176;132;22m",
        indexed: "\x1b[38;5;136m",
        basic: "\x1b[33m",
    },
    removed: Ink {
        exact: "\x1b[48;2;74;26;30;38;2;255;199;202m",
        indexed: "\x1b[48;5;52;38;5;224m",
        basic: "\x1b[41;97m",
    },
    // A shade nearer the ground's own hue, so the gutter reads as the edge of
    // the block rather than as more of the line. At sixteen colours there is no
    // such shade to be had, so the emphasis is weight instead — the meaning is
    // in the ground either way.
    removed_number: Ink {
        exact: "\x1b[48;2;74;26;30;38;2;255;138;145m",
        indexed: "\x1b[48;5;52;38;5;210m",
        basic: "\x1b[1;41;97m",
    },
    added: Ink {
        exact: "\x1b[48;2;19;61;36;38;2;198;245;214m",
        indexed: "\x1b[48;5;22;38;5;194m",
        basic: "\x1b[42;97m",
    },
    added_number: Ink {
        exact: "\x1b[48;2;19;61;36;38;2;126;226;160m",
        indexed: "\x1b[48;5;22;38;5;114m",
        basic: "\x1b[1;42;97m",
    },
};

/// For a light ground.
///
/// The same roles, re-tuned rather than re-imagined: the accent darkens far
/// enough to clear white, and both diff pairs invert — a pale ground carrying
/// dark ink, where the dark table has a dark ground carrying pale ink.
///
/// At the indexed rung the gutter is weight on the ink rather than a second
/// shade. The cube has no darker green than the one the added line already
/// uses, so there is nothing nearer the ground to move to, and this is the
/// answer the sixteen-colour rung has always given for the same reason.
const LIGHT: Tones = Tones {
    accent: Ink {
        exact: "\x1b[38;2;13;107;98m",
        indexed: "\x1b[38;5;23m",
        basic: "\x1b[36m",
    },
    strong: Ink {
        exact: "\x1b[1;38;2;13;107;98m",
        indexed: "\x1b[1;38;5;23m",
        basic: "\x1b[1;36m",
    },
    green: Ink {
        exact: "\x1b[38;2;44;122;57m",
        indexed: "\x1b[38;5;22m",
        basic: "\x1b[32m",
    },
    amber: Ink {
        exact: "\x1b[38;2;138;100;16m",
        indexed: "\x1b[38;5;94m",
        basic: "\x1b[33m",
    },
    removed: Ink {
        exact: "\x1b[48;2;255;227;229;38;2;92;17;22m",
        indexed: "\x1b[48;5;224;38;5;52m",
        basic: "\x1b[41;97m",
    },
    removed_number: Ink {
        exact: "\x1b[48;2;255;227;229;38;2;143;27;35m",
        indexed: "\x1b[1;48;5;224;38;5;52m",
        basic: "\x1b[1;41;97m",
    },
    added: Ink {
        exact: "\x1b[48;2;223;245;229;38;2;13;61;32m",
        indexed: "\x1b[48;5;194;38;5;22m",
        basic: "\x1b[42;97m",
    },
    added_number: Ink {
        exact: "\x1b[48;2;223;245;229;38;2;24;107;51m",
        indexed: "\x1b[1;48;5;194;38;5;22m",
        basic: "\x1b[1;42;97m",
    },
};

/// Dark, with the diff moved off the red-green axis.
///
/// What a change put in goes blue and what it took out goes amber, so the two
/// are told apart by hue for a deuteranope and a protanope as well as by the
/// sign in the gutter. The permission modes move with them: the narrow one is
/// the blue and the wide one the amber, because those two are the pair a reader
/// has to tell apart at a glance.
///
/// The indexed rung is picked by hand rather than by nearest-colour. The
/// arithmetic answer for both of these grounds is the same grey off the ramp —
/// they are dark and low in chroma, and the ramp is perceptually nearest to
/// each — which would take the one distinction this table exists to make and
/// delete it.
const COLOURBLIND_DARK: Tones = Tones {
    accent: Ink {
        exact: "\x1b[38;2;63;167;196m",
        indexed: "\x1b[38;5;38m",
        basic: "\x1b[36m",
    },
    strong: Ink {
        exact: "\x1b[1;38;2;63;167;196m",
        indexed: "\x1b[1;38;5;38m",
        basic: "\x1b[1;36m",
    },
    green: Ink {
        exact: "\x1b[38;2;74;158;255m",
        indexed: "\x1b[38;5;75m",
        basic: "\x1b[34m",
    },
    amber: Ink {
        exact: "\x1b[38;2;232;163;61m",
        indexed: "\x1b[38;5;215m",
        basic: "\x1b[33m",
    },
    removed: Ink {
        exact: "\x1b[48;2;74;42;16;38;2;255;224;194m",
        indexed: "\x1b[48;5;58;38;5;223m",
        basic: "\x1b[43;30m",
    },
    removed_number: Ink {
        exact: "\x1b[48;2;74;42;16;38;2;255;180;107m",
        indexed: "\x1b[1;48;5;58;38;5;223m",
        basic: "\x1b[1;43;30m",
    },
    added: Ink {
        exact: "\x1b[48;2;16;49;74;38;2;207;230;255m",
        indexed: "\x1b[48;5;18;38;5;153m",
        basic: "\x1b[44;97m",
    },
    added_number: Ink {
        exact: "\x1b[48;2;16;49;74;38;2;124;192;255m",
        indexed: "\x1b[48;5;18;38;5;75m",
        basic: "\x1b[1;44;97m",
    },
};

/// The same swap, for a light ground.
const COLOURBLIND_LIGHT: Tones = Tones {
    accent: Ink {
        exact: "\x1b[38;2;15;95;120m",
        indexed: "\x1b[38;5;24m",
        basic: "\x1b[36m",
    },
    strong: Ink {
        exact: "\x1b[1;38;2;15;95;120m",
        indexed: "\x1b[1;38;5;24m",
        basic: "\x1b[1;36m",
    },
    green: Ink {
        exact: "\x1b[38;2;28;95;168m",
        indexed: "\x1b[38;5;25m",
        basic: "\x1b[34m",
    },
    amber: Ink {
        exact: "\x1b[38;2;138;90;16m",
        indexed: "\x1b[38;5;94m",
        basic: "\x1b[33m",
    },
    removed: Ink {
        exact: "\x1b[48;2;255;234;218;38;2;92;42;8m",
        indexed: "\x1b[48;5;223;38;5;58m",
        basic: "\x1b[43;30m",
    },
    removed_number: Ink {
        exact: "\x1b[48;2;255;234;218;38;2;143;74;18m",
        indexed: "\x1b[1;48;5;223;38;5;58m",
        basic: "\x1b[1;43;30m",
    },
    added: Ink {
        exact: "\x1b[48;2;220;234;248;38;2;13;44;71m",
        indexed: "\x1b[48;5;189;38;5;18m",
        basic: "\x1b[44;97m",
    },
    added_number: Ink {
        exact: "\x1b[48;2;220;234;248;38;2;23;82;136m",
        indexed: "\x1b[1;48;5;189;38;5;18m",
        basic: "\x1b[1;44;97m",
    },
};

/// The sixteen and nothing else.
///
/// Not a fifth set of hues — the same answer at every rung, which is what makes
/// it mean "whatever these are on your terminal". A reader who has tuned their
/// own sixteen has already answered every question this file otherwise asks,
/// and this is how they say so.
const ANSI: Tones = Tones {
    accent: Ink {
        exact: "\x1b[36m",
        indexed: "\x1b[36m",
        basic: "\x1b[36m",
    },
    strong: Ink {
        exact: "\x1b[1;36m",
        indexed: "\x1b[1;36m",
        basic: "\x1b[1;36m",
    },
    green: Ink {
        exact: "\x1b[32m",
        indexed: "\x1b[32m",
        basic: "\x1b[32m",
    },
    amber: Ink {
        exact: "\x1b[33m",
        indexed: "\x1b[33m",
        basic: "\x1b[33m",
    },
    removed: Ink {
        exact: "\x1b[41;97m",
        indexed: "\x1b[41;97m",
        basic: "\x1b[41;97m",
    },
    removed_number: Ink {
        exact: "\x1b[1;41;97m",
        indexed: "\x1b[1;41;97m",
        basic: "\x1b[1;41;97m",
    },
    added: Ink {
        exact: "\x1b[42;97m",
        indexed: "\x1b[42;97m",
        basic: "\x1b[42;97m",
    },
    added_number: Ink {
        exact: "\x1b[1;42;97m",
        indexed: "\x1b[1;42;97m",
        basic: "\x1b[1;42;97m",
    },
};

impl Theme {
    /// The hues this theme spends.
    const fn tones(self) -> Tones {
        match self {
            Self::Dark => DARK,
            Self::Light => LIGHT,
            Self::ColourblindDark => COLOURBLIND_DARK,
            Self::ColourblindLight => COLOURBLIND_LIGHT,
            Self::Ansi => ANSI,
        }
    }
}

impl Slot {
    /// What this slot is worth in `theme`, at each rung.
    ///
    /// `None` for the two slots whose value is not in any table: the prompt
    /// band is worked out from the reader's own ground, so the palette holds it
    /// rather than the theme.
    const fn ink(self, theme: Theme) -> Option<Ink> {
        let tones = theme.tones();

        Some(match self {
            Self::Plain => NONE,
            Self::Accent => tones.accent,
            Self::Strong => tones.strong,
            Self::Quiet => QUIET,
            Self::AllowEdits | Self::DoneMark => tones.green,
            Self::FullAccess | Self::DoingMark => tones.amber,
            // Weight and nothing else, at every rung: the reader's own
            // foreground, emphasised. There is no ladder to climb because there
            // is no colour to spend -- an attribute is the same byte on a
            // terminal with sixteen colours and on one with sixteen million.
            Self::Doing => Ink {
                exact: "\x1b[1m",
                indexed: "\x1b[1m",
                basic: "\x1b[1m",
            },
            // Struck through and quiet together, and quiet is the terminal's own
            // answer for the reason it always is. The line through the text is
            // what carries the meaning where a terminal draws it and nothing
            // where one does not -- so the grey is beside it rather than behind
            // it, and a task that is finished still reads as subdued either way.
            Self::Done => Ink {
                exact: "\x1b[9;90m",
                indexed: "\x1b[9;90m",
                basic: "\x1b[9;90m",
            },
            Self::Removed => tones.removed,
            Self::RemovedNumber => tones.removed_number,
            Self::Added => tones.added,
            Self::AddedNumber => tones.added_number,
            Self::Prompt | Self::PromptMark => return None,
        })
    }
}

/// What a slot turned out to be worth, ready to be written.
///
/// Two kinds, because two things produce one. Almost every value is a sequence
/// somebody chose, checked in and borrowed as it stands; the prompt band is one
/// this palette worked out from the reader's own ground, so it has no static to
/// borrow and carries its bytes with it.
///
/// `Copy` and owned rather than a borrow, because what is worn outlives the
/// palette it came from: the streamed tail holds the slot in force across
/// deltas, and a reference into a palette that was passed by value would not
/// live that long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worn {
    /// A sequence written down in a theme's table.
    Chosen(&'static str),
    /// One this palette worked out.
    Computed(Sequence),
}

impl Worn {
    /// The bytes, whichever kind it turned out to be.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Chosen(said) => said,
            Self::Computed(sequence) => sequence.as_str(),
        }
    }

    /// Whether it writes anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }
}

impl std::fmt::Display for Worn {
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        into.write_str(self.as_str())
    }
}

/// One computed sequence: a ground, and an ink to go with it where there is one.
///
/// The pair travels together for the reason every other ground-painting slot's
/// does — a ground written without its ink leaves the reader's own foreground
/// as the other half of a contrast nobody checked.
fn painted(ground: (u8, u8, u8), ink: Option<Ink>, depth: Depth) -> Option<Sequence> {
    use std::fmt::Write as _;

    let mut sequence = Sequence::empty();
    let (red, green, blue) = ground;

    // Opened but not closed: the ink's parameters are spliced in before the
    // `m`, so the pair leaves as one sequence rather than two. Two would be two
    // chances to write one and not the other, which is the thing the rule about
    // taking the ground exists to stop.
    match depth {
        Depth::Exact => write!(sequence, "\x1b[48;2;{red};{green};{blue}"),
        Depth::Indexed => write!(sequence, "\x1b[48;5;{}", derived::nearest_indexed(ground)),
        // The background parameter is the foreground one, ten higher.
        Depth::Basic => write!(sequence, "\x1b[{}", derived::nearest_basic(ground) + 10),
        Depth::Off => return None,
    }
    .ok()?;

    if let Some(ink) = ink {
        let worn = match depth {
            Depth::Exact => ink.exact,
            Depth::Indexed => ink.indexed,
            Depth::Basic => ink.basic,
            Depth::Off => return None,
        };

        // The table's value is a whole sequence, so what is wanted out of it is
        // the parameters between the brackets and the `m`.
        let parameters = worn
            .strip_prefix("\x1b[")
            .and_then(|rest| rest.strip_suffix('m'))?;

        write!(sequence, ";{parameters}").ok()?;
    }

    sequence.write_str("m").ok()?;
    Some(sequence)
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
    /// Which table of hues is in force.
    theme: Theme,
    /// The reader's own ground, as the terminal reported it. Held so a theme
    /// changed mid-session can work its two sequences out again rather than
    /// guess at what they were blended from.
    ground: Option<(u8, u8, u8)>,
    /// That ground, blended a step. `None` where none is known, and then the
    /// prompt row simply does not take one.
    band: Option<Sequence>,
    /// The same ground, carrying the accent, for the mark on that row.
    band_mark: Option<Sequence>,
}

/// How far a band is moved off the ground it was blended from, in hundredths.
///
/// Not the same step in both directions, deliberately. A light ground needs
/// less movement to read as a band than a dark one does, and matching them
/// would make one of the two either invisible or loud.
const LIGHTEN: u8 = 12;
const DARKEN: u8 = 4;

/// The most bytes a computed sequence can come to.
///
/// A ground and an ink at twenty-four bits, with the bold that a mark may
/// carry: `\x1b[1;48;2;255;255;255;38;2;255;255;255m` is forty. The rest is
/// room rather than a prediction, and [`Sequence`] refuses to overrun it
/// instead of growing.
const SEQUENCE: usize = 48;

/// A sequence this palette worked out, held where a `&'static str` cannot go.
///
/// Fixed-width and inline, because [`Palette`] is `Copy` and is passed by value
/// into every row that gets painted. A `String` here would put an allocation
/// behind a value that is copied on the render path, which is the one thing
/// this file may not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sequence {
    bytes: [u8; SEQUENCE],
    len: usize,
}

impl Sequence {
    /// An empty one, ready to be written into.
    const fn empty() -> Self {
        Self {
            bytes: [0; SEQUENCE],
            len: 0,
        }
    }

    /// What was written, as the terminal is sent it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only ASCII is ever written here, so this cannot fail -- and an empty
        // answer where it somehow did is a row without colour rather than a
        // panic on the render path.
        self.bytes
            .get(..self.len)
            .and_then(|written| str::from_utf8(written).ok())
            .unwrap_or_default()
    }
}

impl std::fmt::Write for Sequence {
    fn write_str(&mut self, said: &str) -> std::fmt::Result {
        let end = self.len.checked_add(said.len()).ok_or(std::fmt::Error)?;
        let room = self.bytes.get_mut(self.len..end).ok_or(std::fmt::Error)?;

        room.copy_from_slice(said.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl Palette {
    /// Settles the ladder once, from the terminal and the environment.
    ///
    /// `color` is whether to write any at all, which the configuration, the
    /// environment and `is_terminal` have already agreed on between them —
    /// this decides only how much. `from` reads the environment as a parameter
    /// because writing to the real one is `unsafe` in edition 2024 and this
    /// workspace forbids it.
    /// `theme` is which table of hues to spend, already resolved — `auto` is a
    /// question about the terminal and is answered before this is reached.
    /// `ground` is what the terminal said its background is, and `None` is a
    /// terminal that has not said: then the one slot blended off it takes no
    /// ground at all, which is a state the prompt row is drawn correctly in.
    #[must_use]
    pub fn resolve(
        color: bool,
        theme: Theme,
        ground: Option<(u8, u8, u8)>,
        from: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let depth = if color { Self::depth(from) } else { Depth::Off };
        // Worked out here, once, and held: the only alternative is formatting
        // it per span per frame, and the render path may not.
        let (band, band_mark) = Self::banding(depth, theme, ground);

        Self {
            depth,
            theme,
            ground,
            band,
            band_mark,
        }
    }

    /// The two sequences blended off the reader's ground, at the rung the table
    /// in force allows.
    ///
    /// Its own function because they are settled in two places — once from the
    /// environment, and again every time the picker moves its mark — and the
    /// two disagreeing is a band that outlives the theme it was blended for.
    /// It hands back the pair rather than a whole palette so that what else a
    /// palette carries stays the caller's to keep: a field added later cannot
    /// be silently reset here.
    fn banding(
        depth: Depth,
        theme: Theme,
        ground: Option<(u8, u8, u8)>,
    ) -> (Option<Sequence>, Option<Sequence>) {
        // `ansi` means the sixteen and nothing else. The band is derived rather
        // than chosen, but it still has to be spelled at some rung, and a
        // reader picks that answer precisely because their terminal — or
        // whatever is recording it — cannot take twenty-four bits. A ground
        // that ignored them would be the one thing on the row that did.
        let rung = match theme {
            Theme::Ansi if depth != Depth::Off => Depth::Basic,
            _ => depth,
        };
        let band = ground.map(Self::band);

        (
            band.and_then(|band| painted(band, None, rung)),
            band.and_then(|band| painted(band, Some(theme.tones().accent), rung)),
        )
    }

    /// The reader's own ground, moved one step.
    ///
    /// Lighter where it is dark and darker where it is light, by the two
    /// amounts above. Never a colour chosen here, which is the whole point:
    /// a band derived from their ground cannot fight a terminal theme nobody
    /// here has seen.
    fn band(ground: (u8, u8, u8)) -> (u8, u8, u8) {
        let (over, step) = if crate::ground::is_light(ground) {
            ((0, 0, 0), DARKEN)
        } else {
            ((255, 255, 255), LIGHTEN)
        };

        derived::blend(over, ground, step)
    }

    /// Whether the row a prompt is left on takes a ground here.
    #[must_use]
    pub fn bands(self) -> bool {
        self.band.is_some()
    }

    /// Which table this palette spends.
    #[must_use]
    pub fn theme(self) -> Theme {
        self.theme
    }

    /// The same palette, spending a different table.
    ///
    /// Everything the terminal decided is carried across — how far up the
    /// ladder it goes, and the band blended off its own ground — because none
    /// of that is the theme's to change. Only the mark's ink is worked out
    /// again, since it is the one computed value a table has a say in.
    #[must_use]
    pub fn wearing(self, theme: Theme) -> Self {
        // The blend off the ground is fixed, but the rung it is spelled at and
        // the ink the mark takes are both the table's, so both are settled
        // again — by the same function the environment settles them with, so
        // moving the picker's mark cannot reach a state resolving never could.
        // `depth` is the terminal's own answer throughout and is never narrowed
        // in place: a table that spends less does not make the next one spend
        // less too.
        let (band, band_mark) = Self::banding(self.depth, theme, self.ground);

        Self {
            theme,
            band,
            band_mark,
            ..self
        }
    }

    /// A palette that writes no escape bytes at all.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            depth: Depth::Off,
            theme: Theme::Dark,
            ground: None,
            band: None,
            band_mark: None,
        }
    }

    /// The sequence that starts `slot`, or nothing when there is no colour.
    #[must_use]
    pub fn open(self, slot: Slot) -> Worn {
        if self.depth == Depth::Off {
            return Worn::Chosen("");
        }

        let Some(ink) = slot.ink(self.theme) else {
            // The two the table has no answer for. Held on the palette because
            // they were worked out from the reader's ground rather than chosen.
            let band = match slot {
                Slot::PromptMark => self.band_mark,
                _ => self.band,
            };

            return band.map_or(Worn::Chosen(""), Worn::Computed);
        };

        Worn::Chosen(match self.depth {
            Depth::Exact => ink.exact,
            Depth::Indexed => ink.indexed,
            Depth::Basic => ink.basic,
            Depth::Off => "",
        })
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
