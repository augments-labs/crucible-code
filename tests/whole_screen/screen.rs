//! A terminal that understands exactly what crucible promises to write.
//!
//! Not a general emulator, and deliberately not one. The renderer's claim is
//! that it moves the cursor with a small set of sequences and reaches nothing
//! above the region it drew; a screen that quietly did something sensible with
//! one outside that set would be agreeing with the claim it was brought here to
//! check. So anything outside the promised set is recorded by name and fails
//! the case that drew it, which makes this a second assertion — that the
//! renderer emits nothing it did not promise — carried by the same run as the
//! pictures.
//!
//! Three of crucible's own guarantees are checked as the bytes arrive rather
//! than at the end, so the frame that broke one is the frame that reports it:
//! no row is ever wider than the terminal, no cell outside the window is ever
//! addressed, and a frame that asked the screen to be held asks for it to be
//! shown again. All three are cheap enough to hold continuously, and none is
//! visible to a component test, which sees rows and never a screen.
//!
//! The second of those is what replaced a rewind that could reach above the top
//! of the screen. Every position crucible writes at is now named outright, so
//! the way that guarantee fails is an address off the window rather than a
//! count of rows that was one too many — and `scrolled` staying at zero for the
//! whole of a session is the other half of it, since a process that owns its
//! screen has no reason to push a row off the top of one.
//!
//! A native launch owns no screen. It draws a live region at the foot of the
//! terminal's own buffer, moving relatively and letting finished rows scroll
//! into the scrollback, so the screen opened for one applies the three
//! sequences that mode is made of — erase below, cursor up, cursor to column —
//! the way a terminal does, keeps the rows pushed off the top as a scrollback
//! the case can read, and puts everything it holds at the new width when the
//! window changes. It also clears the screen and that scrollback when asked,
//! which is how a native session gives a resized window everything again. How
//! it does that is the terminal's [`Profile`]: by default it rewraps, which is
//! what the renderer's own count of how far back its region is assumes of the
//! terminal, and a case can open one that keeps its rows as they were cut
//! instead. Each is still refused on a fullscreen launch, where a frame that
//! moved relatively is one the renderer never composed. Entering the alternate
//! screen is remembered in both, because a native case proves it ran in native
//! mode by that and not by its rows.
//!
//! A new size is told to the screen at the point in crucible's output where
//! the window took it, and takes effect one frame later at most. Crucible reads
//! the window's size before composing each frame, so what can still arrive
//! laid out for the old size is bounded: the rest of the frame open at that
//! point, whose first half was read already, or else one whole frame composed
//! before the size changed and written after it. That much is drawn at the old
//! size. The frame after it was composed with the new size known, and is held
//! to the new size whatever it was drawn for: it is tried on the window at the
//! new size, and drawn there if it breaks no promise, which is how the window
//! takes the size; one that breaks a promise is drawn there all the same, and
//! refused where it is written. Two things this does not tell apart. A frame
//! let through at the old size may have been the one drawn for the new one,
//! when it fits the old window too, which costs the check one frame and
//! nothing else, since the next is held to the new size. And a frame drawn for
//! the old size whose every row fits the new window is taken as drawn for it,
//! because nothing in the stream says otherwise; the window is then rewrapped
//! under it the way a terminal would have. A window never drawn for is
//! reported by the case when the screen goes quiet.
//!
//! Holding changes no cell here. What a real terminal does with it is show one
//! picture instead of two, which a screen assembled from every byte that
//! arrived cannot do — so the picture is the same either way, and what this
//! checks is that the two halves of it are paired. What it does change is when
//! a case may read: [`Screen::shows`] answers only between frames, because a
//! read that ends inside one would otherwise see what no terminal ever showed.
//!
//! Every character is kept with what it was drawn in: the whole state the
//! colour sequences before it left in force, which is not the last sequence
//! alone. [`Screen::picture`] leaves that out, so a text snapshot reads the
//! same in any theme; [`Screen::picture_in_colour`] writes each span's state
//! under its row, which is how a case sees a slot that changed colour. A
//! colour parameter outside the set crucible promises is refused like any
//! other sequence, since a span drawn in it would be captured without it.
//!
//! Columns are counted in characters by default rather than from a width
//! table. Most of what these cases put on screen (ASCII, box drawing, the
//! block glyphs of the wordmark, the arrows) is one column wide, so the two
//! counts agree, and counting characters keeps the checker independent of the
//! crate whose arithmetic is under test. A case that draws wide glyphs or
//! combining marks says which terminal it stands for instead: one that counts
//! as crucible does, or one that draws a whole emoji sequence as a single wide
//! glyph, which is where the two disagree. A wide glyph takes two cells, and a
//! mark that takes no column is not kept, so a picture shows the letter it
//! sits on.
//!
//! A third terminal draws a symbol that has a picture form as the picture, two
//! columns wide, where crucible counts it one. On it a row crucible fitted to
//! the window can be wider than the window, and that is the terminal's doing
//! rather than a broken promise, so it is not refused: what crosses the edge
//! goes on at the start of the next row, as a terminal with autowrap on does,
//! or with autowrap off is drawn in the last columns over what was there.
//! Autowrap is on when a screen opens, and turning it off and on are among the
//! sequences crucible promises, so a case can ask whether a session left it as
//! it found it.

/// The byte that opens a sequence.
const ESCAPE: u8 = 0x1b;

/// What ends a command string.
const BELL: u8 = 0x07;

/// What else ends one: the string terminator, `ESC \\`.
///
/// Both spellings, because a real terminal takes both and crucible writes both
/// — the tab title ends with a bell and a hyperlink ends with this, which is
/// the spelling the hyperlink's own definition is written in.
const TERMINATOR: &str = "\x1b\\";

/// The first half of a row ending, and a whole instruction on its own.
const RETURN: u8 = b'\r';

/// What opens a frame: the request to hold the screen until the frame ends.
const BEGIN_SYNC: &str = "\x1b[?2026h";

/// What ends one: the request to show what was held.
const END_SYNC: &str = "\x1b[?2026l";

/// The bytes that can end a control sequence and say what it was.
const ENDS: std::ops::RangeInclusive<char> = '\u{40}'..='\u{7e}';

/// Which of crucible's two screens a launch asked for, and so which sequences
/// the renderer has promised to confine itself to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The alternate screen, every row of it addressed by name.
    Fullscreen,
    /// The terminal's own buffer, drawn relatively at its foot.
    Native,
}

/// The terminal a native screen stands for: what it does with its rows when
/// the window changes width, and how many columns it gives each character.
///
/// The default is the terminal every case was written against, and the one
/// the native renderer assumes. The others are terminals that disagree with
/// it, for a case that asks what a reader of one of those is left with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Profile {
    pub(crate) reflow: Reflow,
    pub(crate) widths: Widths,
}

/// What a terminal does with the rows it holds when the window changes width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Reflow {
    /// Joins each line that was folded and folds it again at the new width,
    /// as most terminals now do.
    #[default]
    Rewraps,
    /// Cuts every row at the new width and joins nothing when the window
    /// widens again, so what was cut stays lost.
    Keeps,
}

/// How many columns a terminal gives each character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Widths {
    /// One column each, whatever the character.
    #[default]
    Characters,
    /// What the Unicode width table says, counted the way crucible counts it.
    Unicode,
    /// The same, except that a terminal drawing an emoji sequence as one glyph
    /// gives nothing to an emoji joined on after a zero-width joiner, or to a
    /// skin tone after a wide glyph. Crucible counts each part, so on this
    /// terminal a row with one of those is narrower than crucible thinks.
    Clustered,
    /// The same, except that a symbol with an emoji form is drawn as the
    /// picture, two columns wide, even where no selector asks for it, as a
    /// terminal whose font has the picture may draw it. Crucible counts such a
    /// symbol as one column, so on this terminal a row with one is wider than
    /// crucible thinks, and can run past the edge of a window crucible fitted
    /// it to.
    Pictured,
}

/// A screen, and everything crucible did to it.
#[derive(Debug, Clone)]
pub(crate) struct Screen {
    /// Which screen this is.
    mode: Mode,
    /// Which terminal it stands for.
    profile: Profile,
    /// How wide the terminal is.
    columns: usize,
    /// How tall it is.
    rows: usize,
    /// What is on it, one row per line of the window, each as wide as what was
    /// written on it rather than as wide as the window.
    grid: Vec<Vec<Cell>>,
    /// Whether each row of the window is the fold of a wider line, running on
    /// into the row under it — which only a resize of a native screen makes,
    /// and which the next resize needs so as to join the pieces again.
    ran_on: Vec<bool>,
    /// Which row the cursor is on, counted from the top of the window.
    row: usize,
    /// How many columns across it is.
    column: usize,
    /// How many rows have been pushed off the top of a fullscreen window, which
    /// is the one thing that happened that the picture below cannot show.
    ///
    /// It is expected to stay at nought: a process that owns its screen writes
    /// at the position it means and has no reason to make the window move under
    /// what it drew.
    scrolled: usize,
    /// The rows pushed off the top of a native window, oldest first: what a
    /// reader would find on scrolling back.
    scrollback: Vec<Vec<Cell>>,
    /// Whether each row of the scrollback ran on into the one under it.
    ran_on_back: Vec<bool>,
    /// What the next character written is drawn in.
    pen: Pen,
    /// Whether the session entered the alternate screen.
    alternate: bool,
    /// Whether a character written past the last column goes on at the start
    /// of the next row, which is what a terminal does until it is told not to.
    /// Told not to, it draws the character in the last columns instead.
    wraps: bool,
    /// What crucible did that it does not promise to do, in the order it was
    /// first done, each said once.
    refused: Vec<String>,
    /// The command strings that arrived, in the order they did.
    ///
    /// Kept rather than dropped because some of them are the whole point of a
    /// frame and none of them lands in a column: a hyperlink is an address
    /// wrapped around words that are drawn either way, so a picture is exactly
    /// the same whether one was written or not.
    commanded: Vec<String>,
    /// Whether a frame has asked the screen to be held and not yet asked for it
    /// to be shown.
    holding: bool,
    /// Bytes that arrived without the rest of what they belong to.
    ///
    /// A read ends wherever the kernel filled the buffer, which is not where
    /// anything was written. Held rather than drawn: half a sequence drawn as
    /// text is rubbish on the screen and columns the row was never charged for,
    /// and half a character decodes to a replacement one column wider than the
    /// character it stands for. Both invent failures that nothing did.
    pending: Vec<u8>,
    /// The size the window took that crucible has not drawn for yet.
    awaiting: Option<Awaiting>,
    /// The frame being read whole, while a size is awaited, to see whether it
    /// is the first drawn for it.
    collecting: Option<String>,
}

/// A size the window took, and what may still arrive laid out for the old one.
#[derive(Debug, Clone, Copy)]
struct Awaiting {
    columns: usize,
    rows: usize,
    /// Whether one whole frame laid out for the old size may still arrive:
    /// true when no frame was open at the point the window took the size,
    /// since the one composed before it would be whole; false when one was,
    /// since the rest of that frame is the one, and the next was composed with
    /// the new size known.
    spare: bool,
}

/// One column of a row: the character drawn there, and what it was drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    character: char,
    pen: Pen,
}

impl Cell {
    /// A column nothing was drawn in, which a row is padded with to reach the
    /// cursor.
    const BLANK: Self = Self {
        character: ' ',
        pen: Pen::DEFAULT,
    };

    /// The column a terminal leaves empty at the end of a row when the wide
    /// glyph after it would straddle the fold.
    const SKIPPED: Self = Self {
        character: SKIPPED,
        pen: Pen::DEFAULT,
    };

    /// What a picture shows in this column: nothing for the second half of a
    /// wide glyph, which the glyph already covers.
    fn shown(self) -> Option<char> {
        match self.character {
            TAIL => None,
            SKIPPED => Some(' '),
            character => Some(character),
        }
    }
}

/// What stands in the second column of a wide glyph, in the glyph's pen.
///
/// A noncharacter, which no text is meant to hold, so it is never mistaken
/// for a character crucible wrote. Keeping it in a cell of its own is what
/// keeps a row's cells and its columns the same count.
const TAIL: char = '\u{fdd0}';

/// What stands in the column a fold left empty, so that joining the fold
/// again takes the gap away with it.
const SKIPPED: char = '\u{fdd1}';

/// The zero-width joiner, which joins the emoji after it to the one before.
const JOINER: char = '\u{200d}';

impl Widths {
    /// The columns this terminal gives `character`, written after `before`.
    fn of(self, character: char, before: Option<char>) -> usize {
        match self {
            Self::Characters => 1,
            Self::Unicode => measured(character, before),
            Self::Clustered => match (before, character) {
                (Some(JOINER), _) => 0,
                (Some(base), '\u{1f3fb}'..='\u{1f3ff}') if measured(base, None) == 2 => 0,
                _ => measured(character, before),
            },
            Self::Pictured => match measured(character, before) {
                1 if ('\u{2600}'..='\u{27bf}').contains(&character) => 2,
                counted => counted,
            },
        }
    }

    /// Whether a row crucible fitted to the window can be wider than it on
    /// this terminal, so that what runs past the edge is the terminal's doing
    /// rather than crucible writing past the window.
    fn widens_rows(self) -> bool {
        matches!(self, Self::Pictured)
    }
}

/// The columns crucible counts `character` as adding to a row after
/// `before`, which is how a selector that widens the character before it is
/// counted at all.
fn measured(character: char, before: Option<char>) -> usize {
    let before = before.map(String::from).unwrap_or_default();

    crucible_tui::columns(&format!("{before}{character}"))
        .saturating_sub(crucible_tui::columns(&before))
}

/// What a character is drawn in: the whole state the colour sequences before
/// it left in force, which is not the same as the last of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Pen {
    /// Which [`Attribute`]s are on, one bit each.
    attributes: u8,
    foreground: Option<Colour>,
    background: Option<Colour>,
}

/// An attribute a colour sequence turns on or off, in the order a capture
/// spells them.
#[derive(Debug, Clone, Copy)]
enum Attribute {
    Bold,
    Dim,
    Italic,
    Underline,
    Reverse,
    Struck,
}

impl Attribute {
    const ALL: [Self; 6] = [
        Self::Bold,
        Self::Dim,
        Self::Italic,
        Self::Underline,
        Self::Reverse,
        Self::Struck,
    ];

    /// This attribute's bit in [`Pen::attributes`].
    const fn bit(self) -> u8 {
        1 << self as u8
    }

    /// The word a capture writes for it.
    const fn name(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::Dim => "dim",
            Self::Italic => "italic",
            Self::Underline => "underline",
            Self::Reverse => "reverse",
            Self::Struck => "strikethrough",
        }
    }
}

impl Pen {
    /// The terminal's own colours, with no attribute set.
    const DEFAULT: Self = Self {
        attributes: 0,
        foreground: None,
        background: None,
    };

    /// This pen with the colour sequence `params` applied, or `None` when the
    /// sequence holds a parameter outside the set crucible promises.
    ///
    /// The set is what a span is said to be drawn in: the six attributes the
    /// renderer sets, strikethrough last, each one's way back off, the sixteen
    /// colours, the 256 and the exact ones, and the way back to the terminal's
    /// own. A parameter outside it is refused rather than passed over, since a
    /// span drawn in it would be captured in a state that leaves it out.
    fn applied(self, params: &str) -> Option<Self> {
        let mut pen = self;
        let mut codes = params.split(';');

        while let Some(code) = codes.next() {
            match code {
                "" | "0" => pen = Self::DEFAULT,
                "1" => pen.turn(&[Attribute::Bold], true),
                "2" => pen.turn(&[Attribute::Dim], true),
                "3" => pen.turn(&[Attribute::Italic], true),
                "4" => pen.turn(&[Attribute::Underline], true),
                "7" => pen.turn(&[Attribute::Reverse], true),
                "9" => pen.turn(&[Attribute::Struck], true),
                "22" => pen.turn(&[Attribute::Bold, Attribute::Dim], false),
                "23" => pen.turn(&[Attribute::Italic], false),
                "24" => pen.turn(&[Attribute::Underline], false),
                "27" => pen.turn(&[Attribute::Reverse], false),
                "29" => pen.turn(&[Attribute::Struck], false),
                "39" => pen.foreground = None,
                "49" => pen.background = None,
                "38" => pen.foreground = Some(Colour::chosen(&mut codes)?),
                "48" => pen.background = Some(Colour::chosen(&mut codes)?),
                _ => match code.parse::<u16>().ok()? {
                    named @ (30..=37 | 90..=97) => pen.foreground = Some(Colour::Named(named)),
                    named @ (40..=47 | 100..=107) => pen.background = Some(Colour::Named(named)),
                    _ => return None,
                },
            }
        }

        Some(pen)
    }

    /// Turns each of `attributes` on, or off.
    fn turn(&mut self, attributes: &[Attribute], on: bool) {
        for attribute in attributes {
            if on {
                self.attributes |= attribute.bit();
            } else {
                self.attributes &= !attribute.bit();
            }
        }
    }

    /// What this pen draws in, in the fixed order a capture spells it: the
    /// attributes, then the foreground and the background as the parameters
    /// that chose them. Empty for the terminal's own colours.
    fn described(self) -> String {
        let colours = [
            self.foreground
                .map(|colour| format!("fg={}", colour.params(38))),
            self.background
                .map(|colour| format!("bg={}", colour.params(48))),
        ];

        Attribute::ALL
            .into_iter()
            .filter(|attribute| self.attributes & attribute.bit() != 0)
            .map(|attribute| attribute.name().to_owned())
            .chain(colours.into_iter().flatten())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Colour {
    /// The colour `38` or `48` goes on to name: `5` and an index, or `2` and
    /// red, green and blue.
    fn chosen<'a>(codes: &mut impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut next = || codes.next().and_then(|code| code.parse::<u8>().ok());

        match next()? {
            5 => Some(Self::Indexed(next()?)),
            2 => Some(Self::Exact(next()?, next()?, next()?)),
            _ => None,
        }
    }

    /// The parameters that choose this colour, where `lead` is `38` for the
    /// foreground and `48` for the background.
    fn params(self, lead: u16) -> String {
        match self {
            Self::Named(named) => named.to_string(),
            Self::Indexed(index) => format!("{lead};5;{index}"),
            Self::Exact(red, green, blue) => format!("{lead};2;{red};{green};{blue}"),
        }
    }
}

/// A colour, in the form the sequence that chose it named it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    /// One of the sixteen a terminal has, by the parameter that chose it:
    /// `30` to `37` and `90` to `97` for the foreground, ten more for the
    /// background.
    Named(u16),
    /// One of the 256-colour palette.
    Indexed(u8),
    /// An exact colour, red, green and blue.
    Exact(u8, u8, u8),
}

/// The version this build draws on its opening screen.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The character that stands in for each of its characters.
///
/// One per character, so the row keeps its width and the frame around it keeps
/// its corners: the picture is a grid already drawn, and a stand-in of another
/// length would move every bar to its right rather than only the version. The
/// mask therefore follows the version's own length, which leaves one case that
/// still moves a snapshot — a release whose version is spelled in more
/// characters than the last. That one is worth reading, because it moves what
/// the opening screen is laid out around.
const MASK: char = '#';

impl Screen {
    /// An empty screen of that size, for a fullscreen launch.
    pub(crate) fn new(columns: usize, rows: usize) -> Self {
        Self::fullscreen_on(columns, rows, Profile::default())
    }

    /// The same, standing for the terminal `profile` describes.
    pub(crate) fn fullscreen_on(columns: usize, rows: usize, profile: Profile) -> Self {
        Self {
            profile,
            ..Self::opened(Mode::Fullscreen, columns, rows)
        }
    }

    /// An empty screen of that size, for a native launch.
    pub(crate) fn native(columns: usize, rows: usize) -> Self {
        Self::native_on(columns, rows, Profile::default())
    }

    /// The same, standing for the terminal `profile` describes.
    pub(crate) fn native_on(columns: usize, rows: usize, profile: Profile) -> Self {
        Self {
            profile,
            ..Self::opened(Mode::Native, columns, rows)
        }
    }

    fn opened(mode: Mode, columns: usize, rows: usize) -> Self {
        Self {
            mode,
            profile: Profile::default(),
            columns,
            rows,
            grid: vec![Vec::new(); rows],
            ran_on: vec![false; rows],
            row: 0,
            column: 0,
            scrolled: 0,
            scrollback: Vec::new(),
            ran_on_back: Vec::new(),
            pen: Pen::default(),
            alternate: false,
            wraps: true,
            refused: Vec::new(),
            commanded: Vec::new(),
            holding: false,
            pending: Vec::new(),
            awaiting: None,
            collecting: None,
        }
    }

    /// The rows pushed off the top of the window, oldest first, drawn the way
    /// [`picture`](Self::picture) draws its rows so the two read on from each
    /// other: everything a reader could scroll back to, then the window.
    ///
    /// Empty on a fullscreen screen, where a row pushed off the top is counted
    /// in the picture's header instead, since nothing there should be making
    /// one.
    pub(crate) fn scrollback(&self) -> String {
        self.framed(&self.scrollback)
    }

    /// Whether the session entered the alternate screen.
    ///
    /// A fullscreen launch does this first of all and a native one never does,
    /// so it is the one byte that says which screen a session was drawn on —
    /// rows look the same in both until something scrolls.
    pub(crate) fn entered_alternate(&self) -> bool {
        self.alternate
    }

    /// Takes bytes as they came off the terminal.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        self.pending = data.split_off(readable(&data));

        match std::str::from_utf8(&data) {
            Ok(text) => self.take(text),
            // Not a truncation — `readable` has already held one of those back
            // — so these are bytes that are not text at all.
            Err(_) => self.refuse("wrote bytes that are not text".to_owned()),
        }
    }

    /// Changes the size of the window, at this point in what crucible wrote:
    /// what is fed next and was laid out for the old size is drawn at it, up
    /// to the bound [`Self::take`] states, and the window takes the size at
    /// the frame after that.
    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        self.awaiting = Some(Awaiting {
            columns,
            rows,
            spare: !self.is_holding(),
        });
    }

    /// The size the window took that crucible has not drawn for yet.
    pub(crate) fn awaiting(&self) -> Option<(usize, usize)> {
        self.awaiting
            .map(|awaiting| (awaiting.columns, awaiting.rows))
    }

    /// Takes text that arrived whole.
    ///
    /// Drawn as it comes, except while a new size waits for crucible to draw
    /// for it. A frame is laid out for the size crucible read before composing
    /// it, so the frame open when the window changed finishes at the old size,
    /// and each frame after it is read whole first: see [`Self::arrived`] for
    /// which one more may be drawn at the old size and why the one after it is
    /// held to the new one. Bytes outside any frame are drawn at the size the
    /// screen has.
    fn take(&mut self, text: &str) {
        let mut rest = text;

        while !rest.is_empty() {
            if self.awaiting.is_none() {
                self.draw(rest);
                return;
            }

            if let Some(frame) = self.collecting.as_mut() {
                let Some(at) = rest.find(END_SYNC) else {
                    frame.push_str(rest);
                    return;
                };
                let end = at + END_SYNC.len();
                frame.push_str(rest.get(..end).unwrap_or_default());
                rest = rest.get(end..).unwrap_or_default();
                let frame = self.collecting.take().unwrap_or_default();
                self.arrived(&frame);
            } else {
                let Some(at) = rest.find(BEGIN_SYNC) else {
                    self.draw(rest);
                    return;
                };
                self.draw(rest.get(..at).unwrap_or_default());
                self.collecting = Some(String::new());
                rest = rest.get(at..).unwrap_or_default();
            }
        }
    }

    /// Draws a whole frame that arrived while a new size was awaited.
    ///
    /// Tried on a copy of the screen at the new size first: a frame that
    /// breaks no promise there is taken as drawn for it, and the copy becomes
    /// the screen. One that does is drawn at the old size if one such frame
    /// may still arrive — composed before the window changed and written
    /// after it, which crucible's reading the size before each frame allows
    /// once, and only when no frame was open at the change — and otherwise
    /// drawn on the window at the new size, where the rows it wrote past the
    /// width are refused.
    fn arrived(&mut self, frame: &str) {
        let Some(awaiting) = self.awaiting else {
            return self.draw(frame);
        };

        let mut trial = self.clone();
        trial.refused.clear();
        trial.fit(awaiting.columns, awaiting.rows);
        trial.draw(frame);

        if trial.refused.is_empty() {
            trial.refused = std::mem::take(&mut self.refused);
            trial.awaiting = None;
            *self = trial;
        } else if awaiting.spare {
            self.awaiting = Some(Awaiting {
                spare: false,
                ..awaiting
            });
            self.draw(frame);
        } else {
            self.awaiting = None;
            self.fit(awaiting.columns, awaiting.rows);
            self.draw(frame);
        }
    }

    /// Puts the window at a size, under what is already drawn.
    ///
    /// On a fullscreen screen, rows are kept and clipped rather than reflowed,
    /// which is what xterm does and what leaves the picture rectangular; a row
    /// drawn legally at the old width is not charged for the new one, because
    /// the check that matters happens where the row is written. A window that
    /// lost rows loses them off the top, so what was at the foot of it stays at
    /// the foot.
    ///
    /// None of that is scrolling. A window that got shorter has fewer rows to
    /// show, which is the reader's doing and says nothing about what crucible
    /// wrote — and the count exists to catch a frame reaching past the bottom
    /// of a screen this process owns.
    ///
    /// A native screen reflows instead, as its [`Profile`] says: see
    /// [`Self::reflow`].
    fn fit(&mut self, columns: usize, rows: usize) {
        match self.mode {
            Mode::Fullscreen => self.clip(columns, rows),
            Mode::Native => self.reflow(columns, rows),
        }
    }

    /// Clips every row to the new width and the window to the new height.
    fn clip(&mut self, columns: usize, rows: usize) {
        for row in &mut self.grid {
            cut(row, columns);
        }

        while self.grid.len() > rows {
            self.grid.remove(0);
            self.row = self.row.saturating_sub(1);
        }
        self.grid.resize(rows, Vec::new());
        self.ran_on = vec![false; rows];

        self.columns = columns;
        self.rows = rows;
        self.column = self.column.min(columns);
        self.row = self.row.min(rows.saturating_sub(1));
    }

    /// Puts everything the terminal holds at the new width, the way a reader
    /// dragging the corner of the window would see it.
    ///
    /// A terminal that rewraps, which is what the native renderer assumes when
    /// it works out how far back the top of its region is, joins rows that ran
    /// on into each other into one line again and folds each line at the new
    /// width, never through a wide glyph. One that keeps its rows cuts each at
    /// the new width instead and joins none, not even two that one line ran
    /// on across, so it has nothing to join when it widens again.
    /// Either way the cursor keeps its place in the line it was on, nothing
    /// empty below it is kept, and the window is the foot of what is left, the
    /// rest above it being scrollback.
    fn reflow(&mut self, columns: usize, rows: usize) {
        let was = self.columns;
        let cursor = self.scrollback.len() + self.row;
        let back = self.scrollback.drain(..).zip(self.ran_on_back.drain(..));
        let held: Vec<(Vec<Cell>, bool)> = back
            .chain(self.grid.drain(..).zip(self.ran_on.drain(..)))
            .collect();

        // Rows back into lines, with where in its line the cursor was.
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        let mut line = Vec::new();
        let mut caret = (0, 0);
        for (index, (mut row, ran_on)) in held.into_iter().enumerate() {
            let (column, ran_on) = match self.profile.reflow {
                Reflow::Rewraps => (self.column, ran_on),
                Reflow::Keeps => {
                    cut(&mut row, columns);
                    (self.column.min(columns), false)
                }
            };
            if index == cursor {
                caret = (lines.len(), line.len() + column);
            }
            if ran_on {
                row.resize(was, Cell::BLANK);
                row.retain(|cell| cell.character != SKIPPED);
                line.extend(row);
            } else {
                line.extend(row);
                lines.push(std::mem::take(&mut line));
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        while lines.len() > caret.0 + 1 && lines.last().is_some_and(Vec::is_empty) {
            lines.pop();
        }

        // And lines into rows at the new width.
        let width = columns.max(1);
        let mut folded: Vec<(Vec<Cell>, bool)> = Vec::new();
        let mut at = (0, 0);
        for (index, line) in lines.into_iter().enumerate() {
            let first = folded.len();
            let offset = if index == caret.0 { caret.1 } else { 0 };
            let (pieces, landed) = fold(&line, width, offset);
            let count = pieces.len();
            for (piece, row) in pieces.into_iter().enumerate() {
                folded.push((row, piece + 1 < count));
            }
            if index == caret.0 {
                at = (first + landed.0, landed.1);
                while folded.len() <= at.0 {
                    folded.push((Vec::new(), false));
                }
            }
        }

        let top = folded.len().saturating_sub(rows);
        let window = folded.split_off(top);
        for (row, ran_on) in folded {
            self.scrollback.push(row);
            self.ran_on_back.push(ran_on);
        }
        for (row, ran_on) in window {
            self.grid.push(row);
            self.ran_on.push(ran_on);
        }
        self.grid.resize(rows, Vec::new());
        self.ran_on.resize(rows, false);
        self.row = at.0.saturating_sub(top);
        self.column = at.1;
        self.columns = columns;
        self.rows = rows;
    }

    /// Whether autowrap is on, as the last sequence that set it left it.
    pub(crate) fn wraps(&self) -> bool {
        self.wraps
    }

    /// What crucible did that it does not promise to do.
    pub(crate) fn refusals(&self) -> &[String] {
        &self.refused
    }

    /// The command strings this screen was sent, payloads only.
    pub(crate) fn commands(&self) -> &[String] {
        &self.commanded
    }

    /// Whether a frame is still being held.
    ///
    /// True on a quiet screen means a frame asked the terminal to wait for the
    /// rest of it and never said the rest had arrived — which on a real one is
    /// a picture that stops changing until the terminal's own timeout gives up
    /// on the frame. A frame being read whole is one of those until its end
    /// arrives.
    pub(crate) fn is_holding(&self) -> bool {
        self.holding || self.collecting.is_some()
    }

    /// Whether `wanted` is on a frame that has finished being written.
    ///
    /// A read of the terminal can end anywhere, inside a frame as easily as
    /// between two, and a real terminal goes on showing the frame before until
    /// the held one is closed. Text from a frame still being held is what this
    /// picture has and that terminal does not show yet.
    pub(crate) fn shows(&self, wanted: &str) -> bool {
        self.shows_where(|picture| picture.contains(wanted))
    }

    /// The same, for a frame a piece of text cannot name: whether one that has
    /// finished being written is a picture `drawn` holds of.
    pub(crate) fn shows_where(&self, drawn: impl Fn(&str) -> bool) -> bool {
        !self.holding && drawn(&self.picture())
    }

    /// The screen, as a picture with the size and the cursor above it.
    ///
    /// Every row is padded to the full width and closed with a bar, so a
    /// trailing space is a character in the diff rather than something an
    /// editor is free to strip. The cursor is on the header line because where
    /// it parks is half of what the arithmetic under test decides, and a
    /// picture that only showed the text would assert the other half.
    ///
    /// This build's own version is masked out of it. The opening screen draws
    /// one, and a snapshot holding it is a snapshot every release has to
    /// re-accept — which is how a picture stops being read and becomes a file
    /// that gets a yes. [`MASK`] is what stands there instead, one per character.
    pub(crate) fn picture(&self) -> String {
        let header = self.header();
        let rows = self.framed(&self.grid);
        if rows.is_empty() {
            header
        } else {
            format!("{header}\n{rows}")
        }
    }

    /// The line a picture opens with: the size, the cursor, and how many rows
    /// left the top of the window.
    fn header(&self) -> String {
        match self.mode {
            Mode::Fullscreen => format!(
                "{}x{} cursor {},{} scrolled {}",
                self.columns, self.rows, self.row, self.column, self.scrolled
            ),
            Mode::Native => format!(
                "{}x{} cursor {},{} scrollback {}",
                self.columns,
                self.rows,
                self.row,
                self.column,
                self.scrollback.len()
            ),
        }
    }

    /// The screen as [`picture`](Self::picture) draws it, with what each row
    /// was drawn in under it.
    ///
    /// Under each row that holds a span not in the terminal's own colours is
    /// one line per such span, in column order: `  <first>..<last> <state>`,
    /// columns counted from one, and the state as [`Pen::described`] spells
    /// it. A slot that moves to another colour changes this and leaves the
    /// picture as it was.
    pub(crate) fn picture_in_colour(&self) -> String {
        let mut lines = vec![self.header()];

        for row in &self.grid {
            lines.push(self.framed_row(row));

            let mut first = 0;
            for span in row.chunk_by(|one, other| one.pen == other.pen) {
                let last = first + span.len();
                if let Some(cell) = span.first()
                    && cell.pen != Pen::DEFAULT
                {
                    lines.push(format!("  {}..{last} {}", first + 1, cell.pen.described()));
                }
                first = last;
            }
        }

        lines.join("\n")
    }

    /// `rows` as the lines of a picture: each padded to the full width, closed
    /// with a bar, and with this build's version masked out.
    fn framed(&self, rows: &[Vec<Cell>]) -> String {
        rows.iter()
            .map(|row| self.framed_row(row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One row as a line of a picture.
    fn framed_row(&self, row: &[Cell]) -> String {
        let mut line: String = row.iter().filter_map(|cell| cell.shown()).collect();
        for _ in row.len()..self.columns {
            line.push(' ');
        }

        let masked = MASK.to_string().repeat(VERSION.len());

        format!("|{line}|").replace(VERSION, &masked)
    }

    /// Says once that crucible did something it does not promise to do.
    fn refuse(&mut self, what: String) {
        if !self.refused.contains(&what) {
            self.refused.push(what);
        }
    }

    /// Draws text that arrived whole, sequences and all.
    fn draw(&mut self, text: &str) {
        let mut rest = text;

        while !rest.is_empty() {
            match rest.find(char::from(ESCAPE)) {
                Some(0) => rest = self.sequence(rest),
                Some(at) => {
                    self.plain(rest.get(..at).unwrap_or_default());
                    rest = rest.get(at..).unwrap_or_default();
                }
                None => {
                    self.plain(rest);
                    rest = "";
                }
            }
        }
    }

    /// Draws text with no sequence in it, ending rows where it says to.
    fn plain(&mut self, text: &str) {
        let mut rest = text;

        while let Some(at) = rest.find(['\r', '\n']) {
            self.put(rest.get(..at).unwrap_or_default());
            let tail = rest.get(at..).unwrap_or_default();

            let taken = if tail.starts_with("\r\n") {
                self.column = 0;
                self.down();
                2
            } else if tail.starts_with('\r') {
                self.column = 0;
                1
            } else {
                // A terminal in raw mode does not return the carriage on a bare
                // newline, so a row ended with one stair-steps across the
                // screen. The renderer writes `\r\n` on a terminal for exactly
                // that reason, and this is where it would be caught not doing.
                self.refuse("ended a row with a bare newline".to_owned());
                self.down();
                1
            };

            rest = tail.get(taken..).unwrap_or_default();
        }

        self.put(rest);
    }

    /// Writes characters where the cursor is, padding the row to reach it.
    ///
    /// Each takes the columns the profile gives it: a wide glyph is followed
    /// by a [`TAIL`], and a character given none is not kept, since what it
    /// changes is how the one before it is drawn and a picture holds letters.
    ///
    /// On a terminal that can draw a row wider than crucible counted it, a
    /// character that would cross the edge is where the terminal acts: it
    /// goes on at the start of the next row, which then runs on from this
    /// one, or, with that turned off, is drawn in the last columns over what
    /// was there. Anywhere else, crossing the edge is crucible writing past
    /// the window, and is refused below.
    fn put(&mut self, text: &str) {
        let mut column = self.column;
        let widths = self.profile.widths;

        let mut before = self.grid.get_mut(self.row).and_then(|row| {
            while row.len() < column {
                row.push(Cell::BLANK);
            }
            row.get(..column)
                .and_then(|left| left.iter().rev().find(|cell| cell.character != TAIL))
                .map(|cell| cell.character)
        });
        for character in text.chars() {
            let wide = widths.of(character, before);
            if widths.widens_rows() && wide > 0 && column + wide > self.columns {
                if self.wraps {
                    if let Some(ran_on) = self.ran_on.get_mut(self.row) {
                        *ran_on = true;
                    }
                    self.down();
                    column = 0;
                } else {
                    column = self.columns.saturating_sub(wide);
                }
            }
            let Some(row) = self.grid.get_mut(self.row) else {
                break;
            };
            let drawn = Cell {
                character,
                pen: self.pen,
            };
            let tail = Cell {
                character: TAIL,
                ..drawn
            };
            for cell in [drawn, tail].into_iter().take(wide) {
                place(row, column, cell);
                column += 1;
            }
            before = Some(character);
        }

        if column > self.columns {
            self.refuse(format!(
                "wrote row {} out to column {column} on a screen {} columns wide",
                self.row, self.columns
            ));
        }
        self.column = column;
    }

    /// Steps the cursor down a row, scrolling the window when there is none.
    ///
    /// The row pushed off the top is kept on a native screen, where scrolling
    /// is how a finished row reaches the reader's scrollback, and counted on a
    /// fullscreen one, where nothing should be making the window move.
    fn down(&mut self) {
        self.row += 1;

        if self.row >= self.rows {
            let top = self.grid.remove(0);
            let ran_on = self.ran_on.remove(0);
            self.grid.push(Vec::new());
            self.ran_on.push(false);
            self.row = self.rows.saturating_sub(1);

            match self.mode {
                Mode::Fullscreen => self.scrolled += 1,
                Mode::Native => {
                    self.scrollback.push(top);
                    self.ran_on_back.push(ran_on);
                }
            }
        }
    }

    /// Erases from the cursor to the end of the row it is on.
    fn erase_row(&mut self) {
        if let Some(row) = self.grid.get_mut(self.row) {
            cut(row, self.column);
        }
        if let Some(ran_on) = self.ran_on.get_mut(self.row) {
            *ran_on = false;
        }
    }

    /// Erases from the cursor to the end of the screen: the rest of this row
    /// and every row under it.
    fn erase_below(&mut self) {
        self.erase_row();
        for row in self.grid.iter_mut().skip(self.row + 1) {
            row.clear();
        }
        for ran_on in self.ran_on.iter_mut().skip(self.row + 1) {
            *ran_on = false;
        }
    }

    /// Erases every row of the window, leaving the cursor where it is.
    fn erase_screen(&mut self) {
        for row in &mut self.grid {
            row.clear();
        }
        for ran_on in &mut self.ran_on {
            *ran_on = false;
        }
    }

    /// Forgets every row pushed off the top of the window.
    fn erase_scrollback(&mut self) {
        self.scrollback.clear();
        self.ran_on_back.clear();
    }

    /// Moves the cursor up `params` rows, one when none is given, and stops at
    /// the top of the window as a terminal does.
    fn up(&mut self, params: &str) {
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        self.row = self.row.saturating_sub(count);
    }

    /// Puts the cursor in column `params`, counted from one, and no further
    /// right than the last column the window has.
    fn across(&mut self, params: &str) {
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        self.column = (count - 1).min(self.columns.saturating_sub(1));
    }

    /// Reads one escape sequence and returns what follows it.
    fn sequence<'a>(&mut self, rest: &'a str) -> &'a str {
        let body = rest.get(1..).unwrap_or_default();

        match body.chars().next() {
            Some('[') => self.control(body.get(1..).unwrap_or_default()),
            Some(']') => self.command(body.get(1..).unwrap_or_default()),
            Some(other) => {
                self.refuse(format!("wrote ESC {other}"));
                body.get(other.len_utf8()..).unwrap_or_default()
            }
            None => "",
        }
    }

    /// Reads `ESC [ … ` up to the byte that says what it was.
    fn control<'a>(&mut self, after: &'a str) -> &'a str {
        let Some(at) = after.find(|character| ENDS.contains(&character)) else {
            self.refuse("wrote a control sequence with no end to it".to_owned());
            return "";
        };

        let ends = after.get(at..).and_then(|rest| rest.chars().next());
        let rest = ends.map_or("", |ends| {
            after.get(at + ends.len_utf8()..).unwrap_or_default()
        });

        if let Some(ends) = ends {
            self.act(after.get(..at).unwrap_or_default(), ends);
        }
        rest
    }

    /// Acts on one control sequence, or refuses it by name.
    ///
    /// The whole set the renderer promises: park at a named cell, erase the
    /// rest of a row, colour, the two that hold a frame until all of it has
    /// arrived, the modes crucible borrows from the terminal — the screen it
    /// draws on among them — and the one question it asks. A native launch
    /// adds the three its frames are made of, erase to the end of the screen,
    /// cursor up and cursor to a column, and the two that clear the screen and
    /// the scrollback before what was kept is written again.
    fn act(&mut self, params: &str, ends: char) {
        if params == "?1049" && ends == 'h' {
            self.alternate = true;
        }

        if self.mode == Mode::Native {
            match (params, ends) {
                ("" | "0", 'J') => return self.erase_below(),
                ("2", 'J') => return self.erase_screen(),
                ("3", 'J') => return self.erase_scrollback(),
                (_, 'A') => return self.up(params),
                (_, 'G') => return self.across(params),
                _ => {}
            }
        }

        match (params, ends) {
            // Colour moves nothing, but it is what the characters after it
            // are drawn in, so the pen takes it.
            (_, 'm') => match self.pen.applied(params) {
                Some(pen) => self.pen = pen,
                None => self.refuse(format!("wrote ESC[{params}{ends}")),
            },
            // The modes crucible borrows from the terminal, and the
            // device-attributes question it asks once at startup. Neither
            // moves the cursor or fills a column: the modes are state handed
            // back by a guard, and the last is a question.
            //
            // The modes are the mouse switched on at the prompt — buttons,
            // motion while one is held and motion while none is, and
            // coordinates that survive a wide window — bracketed
            // paste so a pasted newline is not a submission, and the level of
            // the newer key encoding that says which modifier was held — that
            // last one pushed with `>1u` and popped with `<u`, which is a stack
            // on the terminal rather than a pair of switches.
            //
            // Device attributes is not asked for its own sake. It is what says
            // the answer to the question before it has already arrived, because
            // a terminal replies in the order it was asked — without it, a
            // terminal implementing neither would be waited on for the whole
            // patience rather than answered at once.
            ("?1000" | "?1002" | "?1003" | "?1006" | "?2004" | "?25" | "?1049", 'h' | 'l')
            | (">1" | "<", 'u')
            | ("", 'c') => {}
            // Autowrap, which a native session turns off so that a row the
            // terminal draws wider than crucible counted it loses its last
            // cells rather than running on into a row crucible never counted.
            ("?7", 'h') => self.wraps = true,
            ("?7", 'l') => self.wraps = false,
            (_, 'H') => self.park(params),
            ("" | "0", 'K') => self.erase_row(),
            ("?2026", 'h') => self.hold(),
            ("?2026", 'l') => self.show(),
            _ => self.refuse(format!("wrote ESC[{params}{ends}")),
        }
    }

    /// Holds the screen for a frame that is being written.
    fn hold(&mut self) {
        if self.holding {
            self.refuse("held a screen that was already being held".to_owned());
        }
        self.holding = true;
    }

    /// Shows what was held.
    fn show(&mut self) {
        if !self.holding {
            self.refuse("showed a screen that was never held".to_owned());
        }
        self.holding = false;
    }

    /// Puts the cursor at a named cell, counted the way the terminal counts
    /// them: row and then column, both from one, and either left out meaning
    /// the first.
    ///
    /// This is the whole of how the renderer moves, which is what makes the
    /// check on it worth carrying. A cell off the window is a frame drawn
    /// somewhere the reader cannot see, and on a real terminal it is clamped
    /// rather than refused — so nothing about the picture would say it had
    /// happened.
    fn park(&mut self, params: &str) {
        let mut at = params.split(';');
        let row = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
        let column = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
        let (row, column): (usize, usize) = (usize::max(row, 1) - 1, usize::max(column, 1) - 1);

        if row >= self.rows {
            self.refuse(format!(
                "parked the cursor on row {row} of a screen {} rows tall",
                self.rows
            ));
        }

        // One past the last column is where a cursor rests at the end of a full
        // row; anything beyond that is a column this window does not have.
        if column > self.columns {
            self.refuse(format!(
                "parked the cursor at column {column} on a screen {} columns wide",
                self.columns
            ));
        }

        self.row = row.min(self.rows.saturating_sub(1));
        self.column = column.min(self.columns);
    }

    /// Reads `ESC ] … BEL`: the tab title, and the one question at startup.
    fn command<'a>(&mut self, after: &'a str) -> &'a str {
        // Whichever ending comes first. A payload cannot contain either, so
        // the first one found is the one that ends this string rather than a
        // later one belonging to something else.
        let bell = after.find(char::from(BELL)).map(|at| (at, 1));
        let terminated = after.find(TERMINATOR).map(|at| (at, TERMINATOR.len()));
        let Some((at, ends)) = bell.into_iter().chain(terminated).min() else {
            self.refuse("wrote a command string with no end to it".to_owned());
            return "";
        };

        let said = after.get(..at).unwrap_or_default();
        self.commanded.push(said.to_owned());

        // `0;` is the tab title, which crucible holds for as long as it runs.
        // `11;?` asks the terminal what colour its own background is, which is
        // what the row a prompt is left on takes its ground from — asked once,
        // before the first frame, and never again. A terminal that does not
        // implement it ignores it, as it ignores any command string it does not
        // know; this window is not a terminal and draws the payload instead,
        // which is why it has to be named here rather than left to be noticed.
        // `8;;` is a hyperlink: the address the run after it points at, and
        // the same with nothing after it to close one. A terminal that knows
        // them makes the words between clickable, and one that does not draws
        // the words and drops these — which is what this does, since a link
        // changes nothing about which column anything lands in.
        if !said.starts_with("0;") && !said.starts_with("8;") && said != "11;?" {
            self.refuse(format!("wrote the command string {said:?}"));
        }

        after.get(at + ends..).unwrap_or_default()
    }
}

/// Puts `cell` in `column` of `row`, blanking what a wide glyph it lands on
/// half of leaves behind, as a terminal does.
fn place(row: &mut Vec<Cell>, column: usize, cell: Cell) {
    if cell.character != TAIL
        && row.get(column).is_some_and(|old| old.character == TAIL)
        && let Some(glyph) = column.checked_sub(1).and_then(|left| row.get_mut(left))
    {
        *glyph = Cell::BLANK;
    }
    if let Some(tail) = row
        .get_mut(column + 1)
        .filter(|next| next.character == TAIL)
    {
        *tail = Cell::BLANK;
    }

    match row.get_mut(column) {
        Some(old) => *old = cell,
        None => row.push(cell),
    }
}

/// Cuts `row` at `columns`, blanking a wide glyph the cut goes through rather
/// than keeping half of it.
fn cut(row: &mut Vec<Cell>, columns: usize) {
    if row.get(columns).is_some_and(|cell| cell.character == TAIL)
        && let Some(glyph) = columns.checked_sub(1).and_then(|last| row.get_mut(last))
    {
        *glyph = Cell::BLANK;
    }
    row.truncate(columns);
}

/// `line` folded into rows `width` columns wide, and the row and column the
/// cell `offset` into it lands on.
///
/// A wide glyph that would straddle a fold goes down whole, leaving a
/// [`SKIPPED`] column where it would have started. An offset past the end of
/// the line lands where the cells after it would have.
fn fold(line: &[Cell], width: usize, offset: usize) -> (Vec<Vec<Cell>>, (usize, usize)) {
    let mut rows = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    let mut landed = None;

    for (index, cell) in line.iter().enumerate() {
        let wide = line
            .get(index + 1)
            .is_some_and(|next| next.character == TAIL);
        let needs = if wide { 2 } else { 1 };
        if cell.character != TAIL && !row.is_empty() && row.len() + needs > width {
            row.resize(width, Cell::SKIPPED);
            rows.push(std::mem::take(&mut row));
        }
        if index == offset {
            landed = Some((rows.len(), row.len()));
        }
        row.push(*cell);
    }

    let end = if row.len() >= width {
        (rows.len() + 1, 0)
    } else {
        (rows.len(), row.len())
    };
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    let landed = landed.unwrap_or_else(|| {
        let column = end.1 + offset.saturating_sub(line.len());
        (end.0 + column / width, column % width)
    });

    (rows, landed)
}

/// How much of `data` can be read now.
///
/// Everything, unless it ends in the middle of something. Three things can be
/// cut in half by a read, and every one of them makes this screen report a
/// failure nothing committed — so each is held back until the rest arrives.
fn readable(data: &[u8]) -> usize {
    let mut end = data.len();

    // A sequence with no end yet. Drawn as text it is rubbish on the screen and
    // columns the row was never charged for.
    if let Some(at) = data.iter().rposition(|byte| *byte == ESCAPE)
        && !finished(data.get(at..).unwrap_or_default())
    {
        end = at;
    }

    end = match std::str::from_utf8(data.get(..end).unwrap_or_default()) {
        // A character with bytes still on their way. Decoded now it becomes a
        // replacement, which is one column wider than what it stands for.
        Err(problem) if problem.error_len().is_none() => problem.valid_up_to(),
        // Whole, or bytes that are not a character at all — which is not this
        // function's to say, and is kept so that `feed` says it.
        _ => end,
    };

    // A carriage return that may be the first half of a row ending. Read on its
    // own it leaves the newline behind it looking like one written alone, which
    // is a thing this screen refuses — and the refusal would land on whichever
    // frame the kernel happened to cut there.
    if end > 0 && data.get(end - 1) == Some(&RETURN) {
        end -= 1;
    }

    end
}

/// Whether `tail`, which begins with an escape, is a whole sequence.
///
/// `tail` starts at the last escape in what has arrived, so nothing inside it
/// opens a second one — which is what makes looking for a single end enough.
fn finished(tail: &[u8]) -> bool {
    match tail.get(1) {
        None => false,
        Some(b'[') => tail
            .iter()
            .skip(2)
            .any(|byte| ENDS.contains(&char::from(*byte))),
        Some(b']') => {
            tail.contains(&BELL) || tail.windows(2).any(|pair| pair == TERMINATOR.as_bytes())
        }
        // Every other sequence is two bytes, and both are here.
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{Profile, Reflow, Screen, Widths};

    /// One of everything the renderer writes, in the shapes it writes it.
    ///
    /// The box characters are three bytes each and one column each, which is
    /// what crucible draws a frame with — so a read cut inside one is a cut
    /// this screen has to survive rather than a case invented for the test.
    const WRITTEN: &str = concat!(
        "\x1b]0;▽ crucible\x07",
        "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h",
        "\x1b[?2026h\x1b[?25l",
        "\x1b[1;1H\x1b[m\x1b[Kcrucible v0.0.9",
        "\x1b[2;1H\x1b[m\x1b[K\x1b[36m│ › ───\x1b[0m",
        "\x1b[2;1H\x1b[?25h\x1b[?2026l",
        "\x1b[?2026h\x1b[?25l",
        "\x1b[6;1H\x1b[m\x1b[K╭──╮",
        "\x1b[7;1H\x1b[m\x1b[K│  │",
        "\x1b[8;1H\x1b[m\x1b[K╰──╯",
        "\x1b[7;3H\x1b[?25h\x1b[?2026l",
    );

    /// The same bytes one at a time, which is every cut point at once.
    fn byte_at_a_time(written: &str) -> Screen {
        let mut screen = Screen::new(24, 8);

        for byte in written.as_bytes() {
            screen.feed(std::slice::from_ref(byte));
        }
        screen
    }

    /// A picture holds no version, so cutting a release costs no snapshot.
    ///
    /// The opening screen draws this build's own version, and one case here
    /// leaves it on screen. Without the mask, every release re-accepts that
    /// snapshot — and a snapshot re-accepted on a schedule is one nobody
    /// reads, which is the whole of what it was for.
    #[test]
    fn a_picture_holds_no_version_of_the_build_that_drew_it() {
        let mut screen = Screen::new(24, 8);
        screen.feed(concat!("crucible v", env!("CARGO_PKG_VERSION")).as_bytes());

        let picture = screen.picture();

        assert!(
            !picture.contains(env!("CARGO_PKG_VERSION")),
            "the picture names the version it was drawn by:\n{picture}"
        );
        assert!(picture.contains("crucible v#"), "{picture}");
    }

    #[test]
    fn a_picture_in_colour_says_what_each_span_was_drawn_in() {
        // Each span is drawn in everything still in force when it was written,
        // not in the last sequence alone: `22;23` turns two attributes off and
        // leaves the background the sequence before it chose.
        let mut screen = Screen::new(12, 2);
        screen.feed(
            concat!(
                "\x1b[1;38;5;30mab\x1b[0mc \x1b[3m\x1b[48;2;1;2;3md\x1b[22;23mef\x1b[m",
                "\r\n\x1b[1;43;30mok\x1b[m",
            )
            .as_bytes(),
        );

        assert_eq!(
            screen.picture_in_colour(),
            [
                "12x2 cursor 1,2 scrolled 0",
                "|abc def     |",
                "  1..2 bold fg=38;5;30",
                "  5..5 italic bg=48;2;1;2;3",
                "  6..7 bg=48;2;1;2;3",
                "|ok          |",
                "  1..2 bold fg=30 bg=43",
            ]
            .join("\n")
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_colour_the_screen_cannot_name_is_refused() {
        // Blinking is nothing crucible promises, and a span drawn in it would
        // otherwise be captured in a state that leaves it out.
        let mut screen = Screen::new(12, 2);
        screen.feed(b"\x1b[5mx\x1b[38;5mx");

        assert_eq!(screen.refusals(), ["wrote ESC[5m", "wrote ESC[38;5m"]);
    }

    #[test]
    fn a_stream_cut_anywhere_by_the_read_draws_the_same_screen() {
        // A read ends where the kernel filled the buffer, and under load it
        // ends in more places. Every failure this screen could report that
        // nothing committed comes from a cut: half a sequence, half a
        // character, or a `\r` read without the `\n` written with it.
        let mut whole = Screen::new(24, 8);
        whole.feed(WRITTEN.as_bytes());
        let split = byte_at_a_time(WRITTEN);

        assert_eq!(split.picture(), whole.picture());
        assert!(whole.refusals().is_empty(), "{:?}", whole.refusals());
        assert!(split.refusals().is_empty(), "{:?}", split.refusals());
    }

    #[test]
    fn a_row_wider_than_the_terminal_is_reported() {
        // The first of the two invariants, watched failing. One nobody has
        // seen say no is a check the cases are passing for free.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"a row that is far too long");

        assert!(
            screen
                .refusals()
                .iter()
                .any(|said| said.contains("column 26")),
            "{:?}",
            screen.refusals()
        );
    }

    #[test]
    fn parking_the_cursor_off_the_window_is_reported() {
        // The second. A real terminal clamps an address it does not have,
        // which is what makes this worth watching for: the row is drawn
        // somewhere the reader can see and nothing about the picture says the
        // frame meant it to be anywhere else.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[9;1H");

        assert!(
            screen
                .refusals()
                .iter()
                .any(|said| said.contains("of a screen 4 rows tall")),
            "{:?}",
            screen.refusals()
        );
    }

    #[test]
    fn a_sequence_the_renderer_does_not_promise_is_reported_by_name() {
        // Erasing the whole screen at once is not how a frame gets there: a
        // row is erased by the frame that is about to write it, so a screen
        // cleared out from under one is a sequence nothing here composed.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[2J");

        assert_eq!(screen.refusals(), ["wrote ESC[2J"]);
    }

    #[test]
    fn a_frame_still_held_when_the_screen_goes_quiet_is_visible_from_outside() {
        // The third invariant. A real terminal holds the picture it has until
        // the closing sequence arrives, so a frame that opened one and never
        // closed it is a screen that has stopped changing — which is invisible
        // to a picture assembled from every byte, and is the point of asking.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[?2026h\x1b[1;1H\x1b[Kone");

        assert!(screen.is_holding());
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());

        screen.feed(b"\x1b[?2026l");
        assert!(!screen.is_holding());
    }

    #[test]
    fn text_in_a_frame_still_being_written_is_not_yet_shown() {
        // A read can end inside a frame: the box's top edge has arrived and its
        // bottom edge has not. A real terminal shows the frame before until the
        // closing sequence, so a step waiting for the top edge must not take
        // the screen until the rest of that frame is on it too.
        let mut screen = Screen::new(12, 4);
        screen.feed(b"\x1b[?2026h\x1b[1;1H\x1b[K+- 1 queued");

        assert!(!screen.shows("1 queued"), "{}", screen.picture());

        screen.feed(b"\x1b[2;1H\x1b[K+----------\x1b[?2026l");
        assert!(screen.shows("1 queued"), "{}", screen.picture());
    }

    #[test]
    fn showing_a_screen_that_was_never_held_is_reported() {
        // The pairing is what the invariant is made of, so the half nothing
        // opened is refused as loudly as the half nothing closed.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[?2026l");

        assert_eq!(screen.refusals(), ["showed a screen that was never held"]);
    }

    #[test]
    fn a_row_ended_with_a_bare_newline_is_reported() {
        // Raw mode does not return the carriage on one, so rows stair-step
        // across the screen — and the picture assembled from them is not the
        // one the reader saw.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"one\ntwo");

        assert_eq!(screen.refusals(), ["ended a row with a bare newline"]);
    }

    /// The rows of `picture`, edges and header off.
    fn rows(picture: &str) -> Vec<String> {
        picture
            .lines()
            .skip(1)
            .map(|line| line.trim_matches('|').to_owned())
            .collect()
    }

    /// A whole frame laid out for eight columns, as the native renderer
    /// writes one: back to the region's top, erase below, two rows, park.
    const EIGHT_WIDE: &[u8] = b"\x1b[?2026h\r\x1b[Jeight ch\r\nlast row\x1b[1A\x1b[1G\x1b[?2026l";

    #[test]
    fn one_frame_drawn_for_the_old_width_is_let_through_after_a_resize_and_a_second_is_refused() {
        // Crucible reads the window's size before composing each frame, so
        // after the size changed between two frames, one more frame can have
        // been laid out for the old width: composed before the change and
        // written after it. That frame is drawn for the old width. The one
        // after it was composed after crucible had the size, and is held to
        // the new width whether or not crucible drew for it.
        let mut screen = Screen::native(8, 4);
        screen.feed(EIGHT_WIDE);
        screen.resize(6, 4);
        screen.feed(EIGHT_WIDE);

        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
        assert!(screen.picture().starts_with("8x4 "), "{}", screen.picture());
        assert_eq!(screen.awaiting(), Some((6, 4)));

        screen.feed(EIGHT_WIDE);

        assert_eq!(
            screen.refusals(),
            [
                "wrote row 0 out to column 8 on a screen 6 columns wide",
                "wrote row 1 out to column 8 on a screen 6 columns wide"
            ],
            "{}",
            screen.picture()
        );
    }

    #[test]
    fn the_tail_of_the_frame_open_at_a_resize_is_let_through_and_the_next_frame_is_not() {
        // A frame reaches the terminal in more than one write, so the window
        // can change with half of one read. The rest of it was laid out for
        // the old width and finishes at it; the frame after it was composed
        // once crucible had the size, and is held to the new width.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"\x1b[?2026h\r\x1b[Jeight ch\r\n");
        screen.resize(6, 4);
        screen.feed(b"last row\x1b[1A\x1b[1G\x1b[?2026l");

        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
        assert!(screen.picture().starts_with("8x4 "), "{}", screen.picture());
        assert_eq!(screen.awaiting(), Some((6, 4)));

        screen.feed(EIGHT_WIDE);

        assert_eq!(
            screen.refusals(),
            [
                "wrote row 0 out to column 8 on a screen 6 columns wide",
                "wrote row 1 out to column 8 on a screen 6 columns wide"
            ],
            "{}",
            screen.picture()
        );
    }

    #[test]
    fn the_first_frame_that_fits_the_new_window_takes_it_and_holds_what_follows_to_it() {
        // The frame crucible draws for the new size is drawn on the window at
        // that size — rewrapped under it, as a terminal would have — and a
        // frame at the old width after it is refused where it is written.
        let mut screen = Screen::native(8, 4);
        screen.feed(EIGHT_WIDE);
        screen.resize(6, 4);
        screen.feed(b"\x1b[?2026h\r\x1b[Jsix ch\r\nsix ch\x1b[1A\x1b[1G\x1b[?2026l");

        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
        assert_eq!(screen.awaiting(), None);
        assert_eq!(
            rows(&screen.picture()),
            ["six ch", "six ch", "      ", "      "],
            "{}",
            screen.picture()
        );

        screen.feed(b"\x1b[?2026h\r\x1b[Jeight ch\x1b[1G\x1b[?2026l");

        assert_eq!(
            screen.refusals(),
            ["wrote row 0 out to column 8 on a screen 6 columns wide"]
        );
    }

    #[test]
    fn a_frame_being_read_whole_counts_as_held() {
        // The frame is held back from the picture until its end says which
        // width it was drawn for, so a quiet screen with half of one is a frame
        // still held, the same as one whose closing sequence never came.
        let mut screen = Screen::native(8, 4);
        screen.resize(6, 4);
        screen.feed(b"\x1b[?2026h\r\x1b[Jsix ch");

        assert!(screen.is_holding());

        screen.feed(b"\x1b[1G\x1b[?2026l");

        assert!(!screen.is_holding());
        assert_eq!(screen.awaiting(), None);
    }

    #[test]
    fn erase_below_on_a_native_screen_clears_from_the_cursor_to_the_foot() {
        // The sequence every native frame opens with: back to the top of the
        // region, then everything from there to the foot of the screen goes,
        // the rest of the row the cursor is on included. What stands above the
        // cursor is the reader's and is not touched.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"one\r\ntwo\r\nthree\r\nfour");
        screen.feed(b"\r\x1b[2At\x1b[J");

        assert_eq!(
            rows(&screen.picture()),
            ["one     ", "t       ", "        ", "        "]
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn cursor_up_on_a_native_screen_climbs_and_stops_at_the_top() {
        // A terminal clamps a climb past its top row, and the native renderer
        // leans on that: how far back the region's top is after a resize is
        // worked out rather than read, so a count that lands high is one the
        // terminal is trusted to stop.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"one\r\ntwo");
        screen.feed(b"\x1b[5AX");

        assert_eq!(
            rows(&screen.picture()),
            ["oneX    ", "two     ", "        ", "        "]
        );
        assert!(
            screen.picture().starts_with("8x4 cursor 0,4"),
            "{}",
            screen.picture()
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn cursor_to_column_on_a_native_screen_lands_there() {
        // Counted from one, the way the terminal counts, and clamped to the
        // last column the window has.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"one two\x1b[3Gx\x1b[99G!");

        assert_eq!(
            rows(&screen.picture()).first().map(String::as_str),
            Some("onx two!")
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn the_three_native_sequences_are_still_refused_on_a_fullscreen_screen() {
        // The full screen names every row it writes, so a frame that moved
        // relatively or erased wholesale is one it never composed — and a
        // screen that applied either would be agreeing with the claim it is
        // here to check.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[J\x1b[1A\x1b[1G");

        assert_eq!(
            screen.refusals(),
            ["wrote ESC[J", "wrote ESC[1A", "wrote ESC[1G"]
        );
    }

    #[test]
    fn clearing_a_native_screen_and_its_scrollback_leaves_neither() {
        // What a native session writes before giving a resized window
        // everything again: once both are cleared, the rows that follow are
        // the only copy a reader can find.
        let mut screen = Screen::native(8, 2);
        screen.feed(b"one\r\ntwo\r\nthree\x1b[H\x1b[2J\x1b[3J\x1b[Hfour");

        assert_eq!(screen.scrollback(), "");
        assert_eq!(rows(&screen.picture()), ["four    ", "        "]);
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_row_scrolled_off_a_native_screen_is_in_its_scrollback() {
        // In the terminal's own buffer a row pushed off the top is kept, and
        // what a reader could scroll back to is half of what a native case
        // asserts on: a row written once is a row found once in the scrollback
        // and the window together.
        let mut screen = Screen::native(8, 2);
        screen.feed(b"one\r\ntwo\r\nthree");

        assert_eq!(screen.scrollback(), "|one     |");
        assert_eq!(rows(&screen.picture()), ["two     ", "three   "]);
        assert!(
            screen.picture().starts_with("8x2 cursor 1,5 scrollback 1"),
            "{}",
            screen.picture()
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_native_screen_narrowed_rewraps_what_it_holds_and_widened_joins_it_again() {
        // A terminal that rewraps folds each line at the new width and joins
        // the pieces again when the window widens, which is what the native
        // renderer counts on when it works out where the region's top went.
        // `fit` is the fold itself, which a frame drawn for the size applies.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"abcdefgh\r\nij\r\n");

        screen.fit(4, 4);
        assert_eq!(rows(&screen.picture()), ["abcd", "efgh", "ij  ", "    "]);
        assert!(
            screen.picture().starts_with("4x4 cursor 3,0 scrollback 0"),
            "{}",
            screen.picture()
        );

        screen.fit(8, 4);
        assert_eq!(
            rows(&screen.picture()),
            ["abcdefgh", "ij      ", "        ", "        "]
        );
        assert!(
            screen.picture().starts_with("8x4 cursor 2,0 scrollback 0"),
            "{}",
            screen.picture()
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_native_screen_narrowed_past_its_height_scrolls_the_top_rows_back() {
        // Rows that no longer fit go into the scrollback, and the cursor keeps
        // its place in the line it was on.
        let mut screen = Screen::native(8, 3);
        screen.feed(b"abcdefgh\r\nij\r\nk");

        screen.fit(4, 3);

        assert_eq!(screen.scrollback(), "|abcd|");
        assert_eq!(rows(&screen.picture()), ["efgh", "ij  ", "k   "]);
        assert!(
            screen.picture().starts_with("4x3 cursor 2,1 scrollback 1"),
            "{}",
            screen.picture()
        );
    }

    /// A screen standing for a terminal that counts columns by `widths`.
    fn counting(widths: Widths, columns: usize) -> Screen {
        Screen::native_on(
            columns,
            2,
            Profile {
                widths,
                ..Profile::default()
            },
        )
    }

    #[test]
    fn a_unicode_terminal_counts_wide_glyphs_and_marks_as_crucible_does() {
        // Two columns for each wide glyph, two for a sun asked to draw as an
        // emoji, none for the joiner or the accent. A mark is not kept in the
        // picture, which shows the letter it sits on.
        for said in [
            "漢字",
            "☀\u{fe0f}",
            "e\u{301}",
            "👨\u{200d}👩\u{200d}👧",
            "👍🏽",
        ] {
            let mut screen = counting(Widths::Unicode, 12);
            screen.feed(said.as_bytes());

            let columns = crucible_tui::columns(said);
            assert!(
                screen
                    .picture()
                    .starts_with(&format!("12x2 cursor 0,{columns} ")),
                "{said:?} is {columns} columns to crucible:\n{}",
                screen.picture()
            );
        }

        let mut screen = counting(Widths::Unicode, 12);
        screen.feed("漢字e\u{301}x".as_bytes());
        assert_eq!(
            rows(&screen.picture()).first().map(String::as_str),
            Some("漢字ex      ")
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_clustered_terminal_counts_an_emoji_sequence_as_one_wide_glyph() {
        // The family is six columns to crucible, a man, a woman and a girl
        // with a joiner between each, and two on a terminal that draws it as
        // one glyph. The thumb and its skin tone are four and two.
        for (said, clustered) in [("👨\u{200d}👩\u{200d}👧", 2), ("👍🏽", 2)] {
            let mut screen = counting(Widths::Clustered, 12);
            screen.feed(said.as_bytes());

            assert_ne!(crucible_tui::columns(said), clustered, "{said:?}");
            assert!(
                screen
                    .picture()
                    .starts_with(&format!("12x2 cursor 0,{clustered} ")),
                "{said:?}:\n{}",
                screen.picture()
            );
        }
    }

    #[test]
    fn a_wide_glyph_is_folded_whole_and_joined_again() {
        // At five columns the second glyph would straddle the fold, so the
        // terminal leaves the last column of the first row empty and moves it
        // down whole. Widened again, the gap goes with the fold.
        let mut screen = counting(Widths::Unicode, 6);
        screen.feed("ab漢字".as_bytes());

        screen.fit(5, 2);
        assert_eq!(rows(&screen.picture()), ["ab漢 ", "字   "]);

        screen.fit(6, 2);
        assert_eq!(rows(&screen.picture()), ["ab漢字", "      "]);
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_terminal_that_keeps_its_rows_cuts_them_and_never_joins_them_again() {
        // What a terminal that does not rewrap shows a reader who narrows the
        // window and widens it again: each row as far as the narrow window
        // reached, and nothing past it.
        let mut screen = Screen::native_on(
            8,
            4,
            Profile {
                reflow: Reflow::Keeps,
                ..Profile::default()
            },
        );
        screen.feed(b"abcdefgh\r\nij\r\n");

        screen.fit(4, 4);
        assert_eq!(rows(&screen.picture()), ["abcd", "ij  ", "    ", "    "]);
        assert!(
            screen.picture().starts_with("4x4 cursor 2,0 scrollback 0"),
            "{}",
            screen.picture()
        );

        screen.fit(8, 4);
        assert_eq!(
            rows(&screen.picture()),
            ["abcd    ", "ij      ", "        ", "        "]
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_row_drawn_wider_than_crucible_counted_runs_on_until_autowrap_is_off() {
        // Crucible counts each of the three symbols one column, so the row is
        // five columns to it. Drawn as pictures they are two each, and the
        // last would cross the edge: it goes on at the start of the next row,
        // or, with autowrap off, is drawn over the one before it.
        let said = "ab\u{2600}\u{2601}\u{2602}";
        assert_eq!(crucible_tui::columns(said), 5);

        let mut screen = counting(Widths::Pictured, 6);
        screen.feed(said.as_bytes());
        assert_eq!(
            rows(&screen.picture()),
            ["ab\u{2600}\u{2601}", "\u{2602}    "]
        );
        assert!(screen.wraps());

        let mut screen = counting(Widths::Pictured, 6);
        screen.feed(b"\x1b[?7l");
        screen.feed(said.as_bytes());
        assert_eq!(rows(&screen.picture()), ["ab\u{2600}\u{2602}", "      "]);
        assert!(!screen.wraps());

        screen.feed(b"\x1b[?7h");
        assert!(screen.wraps());
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_terminal_that_keeps_its_rows_leaves_a_row_it_ran_on_as_the_rows_it_drew() {
        // The same row run on across two, on a terminal that does not rewrap:
        // widened, it joins neither, as it joins no other row.
        let mut screen = Screen::native_on(
            6,
            2,
            Profile {
                reflow: Reflow::Keeps,
                widths: Widths::Pictured,
            },
        );
        screen.feed("ab\u{2600}\u{2601}\u{2602}".as_bytes());

        screen.fit(8, 2);
        assert_eq!(
            rows(&screen.picture()),
            ["ab\u{2600}\u{2601}  ", "\u{2602}      "]
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn entering_the_alternate_screen_is_remembered() {
        // The one thing that says a session took the full screen rather than
        // the reader's buffer, kept so a native case can ask.
        let mut screen = Screen::native(8, 4);
        assert!(!screen.entered_alternate());

        screen.feed(b"\x1b[?1049h");

        assert!(screen.entered_alternate());
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }
}
