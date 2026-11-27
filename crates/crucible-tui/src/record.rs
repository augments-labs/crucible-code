//! The record: what crucible has drawn, and what of it is on screen.
//!
//! The alternate screen has no scrollback, so this process is the one keeping
//! it. What that costs is bounded on purpose — [`MOST`] retained units, oldest
//! dropped —
//! because the budget in `CONTRIBUTING.md` is a ceiling on the whole process
//! and a store that grew with the session would spend it on rows nobody is
//! looking at. What falls off the top is not lost: the session log holds every
//! message, and the line a session ends on says where that is.
//!
//! In native mode the terminal's own scrollback shows it instead, and a line
//! written there is marked as sent rather than dropped: the live region draws
//! only what has not gone out yet, and what has is kept under the same ceiling,
//! so that a window that changes size can be given the whole of it again at
//! the width it has now. The terminal cannot do that on its own: what it keeps
//! was cut at the width it was written at, and no two terminals put it at a
//! new one the same way.
//!
//! A line is held as a [`Row`] — spans carrying slots — rather than as the
//! bytes a terminal would receive, so a narrower window re-wraps rather than
//! reflows. Only the lines the viewport covers are folded and painted, and that
//! is what keeps a frame proportional to the window rather than to the session.
//!
//! Four kinds of line, because four kinds of thing arrive here. Prose
//! *flows*: the model's answer and a tool's output were written as text and a
//! wrap is the only thing deciding where a row ends, so the width they are
//! folded at is whatever the window is now. A line that opens with a quote's
//! bar, an item's mark or the two columns a `<details>` block's body is set in
//! by is folded under it: the bar is stood again and the words hang where the
//! first row's began. A component whose source is still held is
//! *responsive*: a prompt, a diff, or a command's answer drawn from figures it
//! still holds is laid out again at the new width, and one drawn in a glyph
//! set, such as a tool's change, in a new glyph set too. A block of an
//! answer is *drawn*: a table, a rule or the rule under a heading arrives as
//! the source it was written as, which is kept beside what it was laid out as,
//! and laid out again for a new width or a new glyph set, and for a new way
//! of drawing its kind where a setting chooses one ([`Modes`]); a new theme
//! only paints the same rows in other colours. Both forms count against the
//! ceiling, and a drawn block that falls off the top takes its source with it.
//! The source is lent, not copied: [`Record::source_of`] answers for a row and
//! [`Record::sources`] walks the blocks, so what copies a block and what lists
//! it read the record itself.
//! Everything else is *set*: a box laid out by something that is gone is
//! clipped rather than pretending it can be rebuilt.
//!
//! An image a turn named is a drawn block too ([`Record::image`]), put down
//! after the turn: a top edge naming it by its path, and under it the picture
//! the application read, or on the edge the reason there is none. Its source
//! is the path, which is what the view of what was cut lists it by. It has no
//! drawer, so the picture is made once, from the PNG handed in, and kept as
//! what it was drawn from like any other: a new width, palette or size
//! encodes that again, and one refused or drawn as text to make room for a
//! newer picture stays its edge alone.
//!
//! A drawn block may be shown as a picture instead of its rows: a diagram
//! drawn as an image, on a terminal that confirmed a picture protocol, where
//! colour is written. It is then laid out as rows of cells the picture is
//! placed in, by Kitty, or held blank for an iTerm2 or Sixel image written
//! over them, and keeps what it was drawn from beside what the terminal is
//! sent, so a new width or a new palette encodes it again without asking its
//! drawer. A Sixel picture is quantised, which costs far more than a PNG, so
//! a new width alone encodes one again only once the band shows it
//! ([`Record::freshens`]) or a native session gives its scrollback back
//! ([`Record::freshens_all`]); until then it keeps the cells it had. Those two cost far more than a row, so pictures have caps of their
//! own: [`MOST_KEPT_IMAGES`] of them and [`MOST_KEPT_IMAGE_BYTES`] together.
//! Past either, the oldest is drawn as its text again for as long as it is
//! held, and the reader stays on the line they were reading, as they do for
//! any block that changes height.
//! A block refused its picture is not asked for one again at a new width, nor
//! in a new palette where it was refused for its size, so a resize or a new
//! theme encodes the kept pictures and no more. Each picture made is told
//! to the terminal by a number of its own, counted on from one drawn at
//! random, since the terminal's numbers are shared with every other program
//! writing to it: see [`MOST_PICTURE_ID`].
//!
//! A picture is shown at a [`PictureSize`]: at Fit it is never taller than
//! half the window, and a click on it or Enter on its source goes on to
//! Large and Full and back ([`Record::cycles`]), in a full screen only, for
//! as long as the session holds it. Kitty and iTerm2 scale the one image
//! they were sent into the cells a size gives it, so a new size or a window
//! of another height lays it out again and sends nothing new; a Sixel bitmap
//! is drawn cell for cell, so it is encoded again for them once the band
//! shows it, as for a new width, and a size it is too large to keep at, or
//! kept at only by drawing another picture as text, is passed over rather
//! than drawn as text. A native session's picture keeps
//! the rows it was first shown in, whatever the window's height becomes,
//! since the cells in the terminal's scrollback were laid for them and
//! nothing it sent is placed again ([`Record::windowed`]).
//!
//! The opening is responsive for the same reason. It is drawn from facts read
//! once at launch and kept for the whole session, so what laid it is still here
//! and a resize replaces those lines with the same card drawn for the window
//! there is now, in the glyph set in force now: a reader who changed it since
//! launch is not handed the card back in the characters they turned away from.

use std::fmt;

use std::collections::VecDeque;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher as _;
use std::ops::Range;

use crate::chrome::{self, Undrawn};
use crate::color::{PaletteIdentity, Slot, Theme};
use crate::drawing::{Bucket, Drawers, Mode, Room};
use crate::glyphs::Glyphs;
use crate::markdown::{self, Block, Kind, Links, Offers};
use crate::row::Row;
use crate::scroll_rail::Place;
use crate::terminal::graphics::{Capability, Form, Payload, PictureKey, Pictures, Protocol};

/// The most retained units the record keeps.
///
/// An ordinary line costs one. Responsive source is charged in window-sized
/// byte units as well, because a prompt or diff is not a size merely because it
/// draws as one block. At roughly the width of a window this is a few megabytes
/// and tens of screens of scrolling, deeper than a terminal's own default and
/// far inside the peak budget.
///
/// Public as [`RECORDED`](crate::RECORDED), for a caller printing more rows at
/// once than it can be sure fit: what it prints past this is dropped from the
/// top as the rest arrives.
pub(crate) const MOST: usize = 20_000;

/// Bytes of retained responsive source charged as one ordinary record line.
///
/// A prompt or diff keeps source that fixed rows used to discard after layout.
/// Charging it in window-sized units keeps that new hold under the record's
/// existing ceiling without making every ordinary line count its allocation.
const RETAINED_ROW_BYTES: usize = 80;

/// The most prompt landmarks the record keeps for the scroll rail to mark.
///
/// More than the rows of any ordinary window, and fixed so a prompt per line
/// cannot become a second record beside the record. Past it the oldest prompts
/// are let go, so the oldest stretch of a very long session goes unmarked.
const MOST_LANDMARKS: usize = 256;

/// The most addresses drawn shorter than written that the record keeps whole.
///
/// The reader's own bound, so one answer's list and the record's are the same
/// list: past it the oldest are let go, and an address that went is still in
/// the session log as the answer wrote it.
const MOST_SHORTENED: usize = markdown::SHORTENED_CAP;

/// The most blocks the record keeps shown as pictures at once.
///
/// A picture is kept twice over, as what it was drawn from and as what the
/// terminal is sent, so it is held to a count of its own beside the record's
/// lines; the oldest past it is drawn as text again.
pub const MOST_KEPT_IMAGES: usize = 16;

/// The most bytes every kept picture may come to together, what each was
/// drawn from and what the terminal is sent counted once each.
///
/// The oldest picture past it is drawn as text again, and one that alone is
/// more than this is never kept: see [`Undrawn::TooLargeToKeep`].
pub(crate) const MOST_KEPT_IMAGE_BYTES: usize = 1536 * 1024;

/// The most cells a picture may cover across or down: the placeholder cells
/// that stand for one name their row and column by one of 64 marks, and a
/// picture past them is drawn as text.
pub const MOST_PLACED: u16 = 64;

/// The largest number a picture is told to the terminal by, since it is
/// written as a 24-bit colour, and how many numbers one record gives out.
///
/// The terminal's numbers are one namespace for every program writing to it,
/// another session in the same window or another pane under the same tmux,
/// and a picture sent under a number replaces the one the terminal held by
/// it. So a record starts at a number drawn at random, goes on up to this one
/// and round from 1, never 0, and gives out no more than this many: a picture
/// it sent is never named by another of its own, and two sessions sharing a
/// terminal are unlikely to name pictures by the same numbers.
const MOST_PICTURE_ID: u32 = 0x00FF_FFFF;

/// How large a block's picture is shown: never wider than the transcript,
/// never larger than its own size, and always its own shape.
///
/// A full screen cycles one picture through them when the reader asks
/// ([`PictureSize::next`]), and the picture keeps the one it is at for as
/// long as the record keeps it; a native session shows every picture at
/// [`PictureSize::Fit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PictureSize {
    /// At most half the window tall, so a diagram leaves room for the
    /// answer around it.
    #[default]
    Fit,
    /// At most the window tall, to be looked at on its own.
    Large,
    /// Its own size, at most [`MOST_PLACED`] rows.
    Full,
}

impl PictureSize {
    /// The size a reader asking again goes on to, round from the largest to
    /// the first.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Fit => Self::Large,
            Self::Large => Self::Full,
            Self::Full => Self::Fit,
        }
    }

    /// The most rows a picture at this size is shown in, in a window
    /// `window` rows tall whose transcript band was `band` rows when the
    /// reader asked, by `protocol`; [`MOST_PLACED`] where the window's
    /// height is not known.
    ///
    /// An iTerm2 or Sixel image is written only while the band shows every
    /// row of it, so it is never taller than the band was; Kitty's cells
    /// show whatever part of it the band does.
    fn rows(self, protocol: Protocol, window: usize, band: usize) -> u16 {
        let most = usize::from(MOST_PLACED);
        if window == 0 {
            return MOST_PLACED;
        }
        let band = if band == 0 { window } else { band };
        let written_over = protocol != Protocol::Kitty;
        let rows = match self {
            Self::Fit => window / 2,
            Self::Large if written_over => band.min(window),
            Self::Large => window,
            Self::Full if written_over => band,
            Self::Full => most,
        };
        u16::try_from(rows.clamp(1, most)).unwrap_or(MOST_PLACED)
    }
}

/// A block's picture as the record keeps it: its size in pixels, what it was
/// drawn from, what the terminal is sent, and how large it is shown.
#[derive(Debug)]
pub struct KeptPicture {
    /// Its size in pixels, across and down.
    pub pixels: (u32, u32),
    /// What it was drawn from, worn again for a new palette or width.
    pub form: Form,
    /// What the terminal is sent.
    pub payload: Payload,
    /// How large it is shown.
    pub size: PictureSize,
}

impl KeptPicture {
    /// How many bytes it holds: what it was drawn from and what the terminal
    /// is sent, once each, and once in all where the two are the same image.
    #[must_use]
    pub fn bytes(&self) -> usize {
        match &self.form {
            Form::Png(png) if png.as_slice() == self.payload.bytes() => png.len(),
            form => form.len().saturating_add(self.payload.bytes().len()),
        }
    }
}

/// A drawn block's picture, the key it was made under and the number the
/// terminal is told it by.
#[derive(Debug)]
struct Pictured {
    id: u32,
    key: PictureKey,
    kept: KeptPicture,
    /// How many rows the transcript band was when the reader last chose its
    /// size; none until they do.
    band: usize,
    /// How many rows the window was when it was first shown, where it keeps
    /// the rows it was shown in then, even none before the window said; no
    /// count where it follows the window.
    window: Option<usize>,
}

impl Pictured {
    /// The most rows it is shown in, in a window `window` rows tall, or in
    /// the one it was first shown in where it keeps that.
    fn rows(&self, window: usize) -> u16 {
        let window = self.window.unwrap_or(window);
        self.kept.size.rows(self.key.protocol, window, self.band)
    }
}

/// Whether a picture is shown in the cells it was before a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shown {
    #[default]
    AsBefore,
    /// In other cells, which the terminal holding it has to be told.
    Reshaped,
}

/// What a block of an answer is shown as a picture by, and in.
struct Picturing {
    /// What makes the payload; none until the composition gives one.
    pictures: Option<Box<dyn Pictures>>,
    /// What the terminal said it shows, and in which cells.
    images: Capability,
    /// The palette a picture is worn in; none until the drawing says.
    palette: Option<PaletteIdentity>,
    /// The number the next picture is told to the terminal by.
    next: u32,
    /// How many numbers are left to give out.
    left: u32,
    /// Whether a picture kept was let go of since the drawing last asked.
    let_go: bool,
    /// Whether a picture kept was shown in other cells since the drawing
    /// last asked, which is what has every picture held looked up again to
    /// be placed there.
    shown: Shown,
    /// Whether a Sixel picture kept for another width is encoded for this
    /// one now, as it is where it is in view, rather than left until it is.
    now: bool,
    /// How many rows the window is; none until the drawing says.
    window: usize,
    /// Whether a picture keeps the rows it was first shown in rather than
    /// following the window's height: a native session's, whose scrollback
    /// holds the cells it was placed in.
    keeps: bool,
    /// How many rows the transcript band was at the last frame.
    band: usize,
}

impl fmt::Debug for Picturing {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Picturing")
            .field("pictures", &self.pictures.is_some())
            .field("images", &self.images)
            .field("palette", &self.palette)
            .field("next", &self.next)
            .field("left", &self.left)
            .field("let_go", &self.let_go)
            .field("shown", &self.shown)
            .field("now", &self.now)
            .field("window", &self.window)
            .field("keeps", &self.keeps)
            .field("band", &self.band)
            .finish()
    }
}

impl Default for Picturing {
    fn default() -> Self {
        Self {
            pictures: None,
            images: Capability::Text,
            palette: None,
            next: Self::first_number(),
            left: MOST_PICTURE_ID,
            let_go: false,
            shown: Shown::AsBefore,
            now: false,
            window: 0,
            keeps: false,
            band: 0,
        }
    }
}

impl Picturing {
    /// Remembers a picture kept shown in other cells, where `shown` says one
    /// was, until the drawing asks.
    fn saw(&mut self, shown: Shown) {
        if shown == Shown::Reshaped {
            self.shown = shown;
        }
    }

    /// A number at random from 1 to [`MOST_PICTURE_ID`], the first a record
    /// gives out.
    fn first_number() -> u32 {
        let drawn = RandomState::new().hash_one(()) % u64::from(MOST_PICTURE_ID);
        u32::try_from(drawn).map_or(1, |drawn| drawn + 1)
    }

    /// Gives out the next number, going round from [`MOST_PICTURE_ID`] to 1.
    fn number(&mut self) -> u32 {
        let id = self.next;
        self.next = if id >= MOST_PICTURE_ID { 1 } else { id + 1 };
        self.left = self.left.saturating_sub(1);
        id
    }
}

/// One line of the record, and whether a narrower window may re-fold it.
enum Line {
    /// Text that was written as text. Folded at whatever the window is now.
    Flowed(Row),
    /// One row of a component rebuilt from retained source at each width.
    Responsive {
        /// The display rows at the record's current width.
        rows: Vec<Row>,
        /// A prefix applied to each row after every responsive rebuild.
        prefix: Option<Subordinate>,
        /// How many ordinary record lines this retained source costs.
        weight: usize,
        /// What lays the whole block out at a width, in a glyph set.
        lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>,
        /// The glyph set the rows were laid out in, for a block drawn in one;
        /// none for a block whose rows no glyph set changes.
        glyphs: Option<Glyphs>,
    },
    /// A block of an answer, kept with its source.
    Drawn(Drawn),
    /// Rows a component laid out against a width. Clipped, never re-folded.
    Set(Row),
}

/// A block of an answer, the source it was written as and the rows it was
/// laid out as.
///
/// Laid out for one width, glyph set and theme at a time, and laid out again
/// when the first two change or the mode its kind is drawn in does; the theme is part of what it was laid for only
/// so that what is on screen can be told from what is wanted, since rows carry
/// slots and a theme is the colours a slot is painted in.
pub(crate) struct Drawn {
    block: Block,
    /// The display rows at [`Self::laid`].
    rows: Vec<Row>,
    /// A prefix applied to each row after every layout.
    prefix: Option<Subordinate>,
    /// How many ordinary record lines the source and the rows cost together.
    weight: usize,
    /// The width, glyph set and theme the rows were laid out for.
    laid: (usize, Glyphs, Theme),
    /// The modes the rows were drawn in.
    modes: Modes,
    /// Edits made to the rows since, applied again at every layout.
    edits: Vec<fn(&mut [Row])>,
    /// The picture it is shown as, where it is shown as one.
    ///
    /// Boxed, as what follows is, so a block that is not one costs a word.
    picture: Option<Box<Pictured>>,
    /// Why it is drawn as text under the key it was last asked for a picture
    /// under, so the same key is not asked again.
    refused: Option<Box<(PictureKey, Undrawn)>>,
    /// Whether a later picture took its place among those kept, after which
    /// it is drawn as text for as long as it is held.
    fallen: bool,
    /// What an image a turn named is shown under, where the block is one.
    image: Option<Box<Imaged>>,
}

/// What an image block's edge says besides its path.
#[derive(Debug)]
struct Imaged {
    /// The tool whose call read it, where one did.
    by: Option<Box<str>>,
    /// Why it has no picture, where it has none for a reason worth saying.
    refusal: Option<Undrawn>,
}

/// An image a turn named: the path it was named by, and the tool whose call
/// read it, where one did.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Named<'a> {
    /// The path, as it was named.
    pub(crate) path: &'a str,
    /// The tool, where one read it.
    pub(crate) by: Option<&'a str>,
}

/// The word an image block is listed under in the view of what was cut, with
/// its path under it.
const IMAGE: &str = "image";

/// The words an image's edge gives for a picture refused for `why`: what
/// would say a diagram's source is shown says, of an image, why it is not.
const fn imaged(why: Undrawn) -> Undrawn {
    match why {
        Undrawn::TooWide | Undrawn::TooLargeToKeep | Undrawn::ImageTooLarge => {
            Undrawn::ImageTooLarge
        }
        Undrawn::PastCap
        | Undrawn::NotHere
        | Undrawn::NotText
        | Undrawn::Unsupported(_)
        | Undrawn::ImageNotShown => Undrawn::ImageNotShown,
        Undrawn::NotAnImage => Undrawn::NotAnImage,
        Undrawn::NotReadInTime => Undrawn::NotReadInTime,
        Undrawn::ChangedSinceRead => Undrawn::ChangedSinceRead,
        Undrawn::PastTurnImages => Undrawn::PastTurnImages,
        Undrawn::NotRedrawnOnResume => Undrawn::NotRedrawnOnResume,
    }
}

/// `kept`, where it is placed in no more than `columns` and the most cells a
/// picture is, and is no larger than the record keeps one.
fn held(kept: KeptPicture, columns: usize) -> Result<KeptPicture, Undrawn> {
    let (across, down) = kept.payload.cells();
    if across > MOST_PLACED || down > MOST_PLACED || usize::from(across) > columns {
        return Err(Undrawn::TooWide);
    }
    if kept.bytes() > MOST_KEPT_IMAGE_BYTES {
        return Err(Undrawn::TooLargeToKeep);
    }
    Ok(kept)
}

/// What a block of an answer is laid out by: the links its addresses fold by,
/// the drawers its closed fences are drawn by, the mode each kind a setting
/// chooses for is drawn in, and what becomes of a key their rows offer.
#[derive(Clone, Copy)]
struct By<'a> {
    links: &'a Links,
    drawers: &'a Drawers,
    modes: Modes,
    offers: Offers,
    /// How many rows the window is, which bounds a picture's height.
    window: usize,
}

/// The mode each kind of block a setting chooses the drawing of is drawn in.
///
/// One field a kind, read through [`Modes::of`], whose match says which kinds
/// a setting chooses for, so a kind a later setting draws is one field and one
/// arm here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Modes {
    /// Display math: between `$$`, `\[` or a named environment, or a math
    /// fence.
    pub(crate) math: Mode,
    /// A diagram: a fence named `mermaid`.
    pub(crate) diagrams: Mode,
}

impl Default for Modes {
    /// The mode a setting left alone gives.
    fn default() -> Self {
        Self {
            math: Mode::Image,
            diagrams: Mode::Image,
        }
    }
}

impl Modes {
    /// The mode a block of `kind` is drawn in; none for a kind no setting
    /// chooses the drawing of.
    const fn of(self, kind: Kind) -> Option<Mode> {
        match kind {
            Kind::DisplayMath => Some(self.math),
            Kind::Mermaid => Some(self.diagrams),
            Kind::Table | Kind::Rule | Kind::Heading(_) | Kind::Diff | Kind::Image => None,
        }
    }

    /// Whether a block of `kind` is handed to its drawer at all: never in
    /// [`Mode::Written`], which shows it as it was written. [`Mode::Image`]
    /// is drawn as text wherever no picture is shown in its place.
    pub(crate) const fn draws(self, kind: Kind) -> bool {
        match self.of(kind) {
            Some(Mode::Written) => false,
            Some(Mode::Text | Mode::Image) | None => true,
        }
    }
}

impl Drawn {
    /// `block` laid out for `room` in `theme`, by what `by` holds.
    fn new(block: Block, room: Room, theme: Theme, by: By<'_>) -> Self {
        let Room { columns, glyphs } = room;
        let mut drawn = Self {
            block,
            rows: Vec::new(),
            prefix: None,
            weight: 0,
            laid: (columns, glyphs, theme),
            modes: by.modes,
            edits: Vec::new(),
            picture: None,
            refused: None,
            fallen: false,
            image: None,
        };
        drawn.lay(columns, glyphs, by);
        drawn
    }

    /// The top edge of an image block: its path, the tool that read it, and
    /// why it has no picture, each part made safe to stand in a row.
    fn image_edge(&self, image: &Imaged, columns: usize, glyphs: Glyphs) -> Row {
        let dot = glyphs.dot();
        let mut name = format!("{IMAGE} {dot} ");
        name.extend(self.block.source.chars().filter(|at| !at.is_control()));
        if let Some(by) = &image.by {
            name.push(' ');
            name.push_str(dot);
            name.push_str(" read by ");
            name.extend(by.chars().filter(|at| !at.is_control()));
        }
        let refusal = image.refusal.or_else(|| {
            if self.picture.is_some() {
                self.pictured(columns)
                    .is_none()
                    .then_some(Undrawn::ImageTooLarge)
            } else {
                self.refused.as_ref().map(|refused| imaged(refused.1))
            }
        });
        let top = chrome::Frame {
            label: chrome::Label::whole(&name, glyphs),
            refusal,
        }
        .top();
        Row::new()
            .then_structural(Slot::Quiet, &top)
            .clipped(columns)
    }

    /// The rows its picture stands in, where it has one that fits in
    /// `columns`.
    fn pictured(&self, columns: usize) -> Option<Vec<Row>> {
        let picture = self.picture.as_ref()?;
        let (across, down) = picture.kept.payload.cells();
        let row = match picture.kept.payload.protocol() {
            Protocol::Kitty => Row::picture,
            Protocol::Iterm2 | Protocol::Sixel => Row::reserved,
        };
        (usize::from(across) <= columns)
            .then(|| (0..down).map(|at| row(picture.id, at, across)).collect())
    }

    /// Lays the block out again for `columns` in `glyphs`, by what `by`
    /// holds, with every edit and the prefix it was given since, and answers
    /// whether its picture is shown in other cells than it was.
    fn lay(&mut self, columns: usize, glyphs: Glyphs, by: By<'_>) -> Shown {
        let lay = match by.offers {
            Offers::Stand => markdown::lay,
            Offers::Withdrawn => markdown::lay_unoffered,
        };
        // A kind shown as written is handed to no drawer, which is how a
        // block no drawer takes is laid out.
        let none = Drawers::default();
        let drawers = if by.modes.draws(self.block.kind) {
            by.drawers
        } else {
            &none
        };
        let mut shown = Shown::AsBefore;
        if let Some(picture) = self.picture.as_mut() {
            let rows = picture.rows(by.window);
            let was = picture.kept.payload.cells();
            picture.kept.payload.shown_in(rows);
            if picture.kept.payload.cells() != was {
                shown = Shown::Reshaped;
            }
        }
        let imaged = self.image.as_deref().map(|image| {
            let mut rows = vec![self.image_edge(image, columns, glyphs)];
            rows.extend(self.pictured(columns).unwrap_or_default());
            rows
        });
        let mut rows = imaged
            .or_else(|| self.pictured(columns))
            .unwrap_or_else(|| {
                let laid = lay(&self.block, columns, glyphs, by.links, drawers);
                responsive_rows(
                    laid.iter()
                        .flat_map(|row| {
                            markdown::framed(row, columns).unwrap_or_else(|| row.fold(columns))
                        })
                        .collect(),
                )
            });
        for edit in &self.edits {
            edit(&mut rows);
        }
        if let Some(prefix) = &self.prefix {
            prefix.prefix(&mut rows);
        }
        let bytes = self.block.source.len() + rows.iter().map(Row::bytes).sum::<usize>();
        self.weight = rows.len().max(bytes.div_ceil(RETAINED_ROW_BYTES)).max(1);
        self.rows = rows;
        self.laid.0 = columns;
        self.laid.1 = glyphs;
        self.modes = by.modes;
        shown
    }

    /// Whether its first row is the label of a block drawn as text with a
    /// source to read: the top edge of its frame, past any mark it hangs
    /// under, and not the border of a table, which is no edge.
    fn labelled(&self) -> bool {
        if self.picture.is_some() || self.block.kind.named().is_none() {
            return false;
        }
        let Some(first) = self.rows.first() else {
            return false;
        };
        let text = first.text();
        let hung = self
            .prefix
            .as_ref()
            .filter(|prefix| !prefix.keeps_first)
            .map(|prefix| {
                let mut first = prefix.marks_first;
                prefix.row(&mut first).text()
            })
            .unwrap_or_default();
        let edge = text.strip_prefix(hung.as_str()).unwrap_or(&text).trim_end();
        chrome::is_edge(edge)
            && [Glyphs::Unicode, Glyphs::Ascii]
                .into_iter()
                .all(|glyphs| edge != chrome::bottom(glyphs))
    }

    /// The block's source as line `line`, where its kind has one to read:
    /// an image's is the path it was named by.
    fn source(&self, line: usize) -> Option<Source<'_>> {
        let word = if self.image.is_some() {
            IMAGE
        } else {
            self.block.kind.named()?
        };
        Some(Source {
            line,
            word,
            text: &self.block.source,
        })
    }
}

/// A subordinate block's structural mark and text-column width.
#[derive(Debug, Clone)]
struct Subordinate {
    mark: Box<str>,
    opening: usize,
    /// Whether this prefix contributes the first row's mark.
    marks_first: bool,
    /// Whether a responsive rebuild already contributes that first-row mark.
    keeps_first: bool,
}

impl Subordinate {
    /// The mark on the first row, or its text-column indent after that.
    fn row(&self, first: &mut bool) -> Row {
        let mut row = Row::new();
        if *first && self.marks_first {
            row.push_structural(Slot::Quiet, &*self.mark);
            *first = false;
        } else {
            row.push_structural(Slot::Quiet, " ".repeat(self.opening));
        }
        row.push(Slot::Plain, " ");
        row
    }

    /// Hangs `rows`, a block laid out afresh, under this prefix again.
    fn prefix(&self, rows: &mut [Row]) {
        let mut first = self.marks_first;
        for (at, row) in rows.iter_mut().enumerate() {
            if !(self.keeps_first && at == 0) {
                row.prepend(self.row(&mut first));
            }
        }
    }
}

/// A mark hung on what is written from a line on, before it is written.
///
/// Native mode writes a line out once, and a reply can leave the live region
/// before the command writing it ends. What it is to be hung under is known
/// before the reply starts, so each of its lines is marked as it goes out and
/// [`Record::subordinate`] marks the rest.
#[derive(Debug)]
struct Hanging {
    /// The line the reply starts at, as [`Record::lines`] numbers it.
    from: usize,
    /// The first line not marked yet.
    next: usize,
    mark: Box<str>,
    /// Whether the mark itself is still to be given, rather than the indent.
    first: bool,
}

impl Line {
    /// Its share of the record's retained-memory ceiling.
    fn weight(&self) -> usize {
        match self {
            Self::Responsive { weight, .. } | Self::Drawn(Drawn { weight, .. }) => *weight,
            Self::Flowed(_) | Self::Set(_) => 1,
        }
    }
}

impl fmt::Debug for Line {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flowed(row) => out.debug_tuple("Flowed").field(row).finish(),
            Self::Responsive { rows, weight, .. } => out
                .debug_struct("Responsive")
                .field("rows", &rows.len())
                .field("weight", weight)
                .finish_non_exhaustive(),
            Self::Drawn(drawn) => out
                .debug_struct("Drawn")
                .field("block", &drawn.block)
                .field("rows", &drawn.rows.len())
                .field("weight", &drawn.weight)
                .finish_non_exhaustive(),
            Self::Set(row) => out.debug_tuple("Set").field(row).finish(),
        }
    }
}

/// The opening, and what can draw it again.
///
/// Where its lines are rather than a promise that they are first: nothing else
/// is laid this way today, and a span that says where it is costs one word and
/// cannot be wrong about it.
struct Opening {
    /// The first of its lines, counted as [`Record::gone`] counts.
    from: usize,
    /// How many lines it laid, at the width they were laid at.
    lines: usize,
    /// What laid them, kept for as long as they are held, and handed the
    /// width and the glyph set each time it lays them.
    lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>,
}

impl fmt::Debug for Opening {
    /// By hand because a closure has no `Debug`, and the span is the part of
    /// this a reader debugging a scroll position wants anyway.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Opening")
            .field("from", &self.from)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

/// Everything crucible has drawn into the transcript band, and where in it the
/// reader is looking.
#[derive(Debug)]
pub(crate) struct Record {
    /// Logical lines, oldest first. A responsive one may draw several rows.
    /// The last flowed line is the one streamed text is appended to.
    lines: VecDeque<Line>,
    /// How many display rows each line folds to at [`Self::columns`].
    ///
    /// One `u16` a line, kept beside the lines rather than worked out per
    /// frame: scrolling asks how tall the record is on every wheel event, and
    /// folding twenty thousand lines to answer it is the shape of slowness this
    /// crate exists to refuse. Emptied whole on a resize, which is the only
    /// thing that can make every answer wrong at once.
    tall: VecDeque<u16>,
    /// The cumulative display-row end of each line at [`Self::columns`].
    ///
    /// Absolute within the current epoch, which a resize or emptying the
    /// record starts afresh: dropping a line advances [`Self::before`] rather
    /// than subtracting from every end, so a seek from the scroll rail can
    /// binary-search this list without work proportional to the record on
    /// either append or spill. An end kept past its epoch places the next
    /// session's lines at the old one's rows, and the rail with them.
    ends: VecDeque<usize>,
    /// Display rows before the first retained line in the current width epoch.
    before: usize,
    /// The width every height above was worked out at.
    columns: usize,
    /// Retained-memory units across the logical lines currently held.
    weight: usize,
    /// How many display rows the record comes to, in total.
    ///
    /// The sum of [`Self::tall`], kept rather than added up: see above.
    rows: usize,
    /// How many lines have been dropped off the top.
    ///
    /// Added to an index into [`Self::lines`] it gives a number that means the
    /// same thing for as long as the session does, which is what lets [`Spot`]
    /// hold still while the record fills and spills underneath it.
    gone: usize,
    /// Where the top of the transcript band is in the record.
    top: Spot,
    /// Whether the band follows the foot of the record as it grows.
    ///
    /// The state a session starts in and returns to, because a reader who has
    /// not scrolled is watching the answer arrive. Scrolling up clears it;
    /// scrolling back to the foot sets it again.
    following: bool,
    /// The opening and what laid it, while its lines are still held.
    ///
    /// `None` once the record has spilled far enough to eat into them: the
    /// lines that are left were drawn at a width that has gone, and replacing
    /// part of a card with a whole one would draw it twice.
    opening: Option<Opening>,
    /// Prompt boundaries still retained, oldest first, in stable line numbers.
    ///
    /// Fixed independently of the record: the scroll rail needs enough places
    /// to mark across a long transcript, not one allocation for every prompt
    /// in it.
    landmarks: VecDeque<usize>,
    /// The whole of every address an answer was drawn with shorter than it was
    /// written, newest last, each with the line it was drawn on, at most
    /// [`MOST_SHORTENED`] of them and none twice.
    ///
    /// Beside the lines rather than in them, because what lists them reads
    /// them whole and the rows hold them cut; let go of with the lines they
    /// were drawn on.
    shortened: VecDeque<(usize, Box<str>)>,
    /// The prompt a press on its rail mark last landed on, as one of
    /// [`Self::landmarks`].
    ///
    /// Held here rather than by whoever pressed, because a resize that lays
    /// the opening out again renumbers every landmark under it, and this has
    /// to move with them or it names another prompt. A prompt that has left
    /// the record names nothing, and a prompt sent after it ends the landing:
    /// the prompt being answered is the one read under.
    landed: Option<usize>,
    /// Whether the last line is still being written to.
    ///
    /// A line is open from the first delta that lands in it until the newline
    /// that ends it, and only an open line can change. Held rather than worked
    /// out from what the last line looks like, because a line somebody wrote
    /// nothing on is a blank line and a line nobody has written on yet is not:
    /// the two are the same row and different facts.
    open: bool,
    /// Whether what was let go of last ended in a blank line, for
    /// [`Self::parted`] to answer once nothing is held.
    ///
    /// In native mode the record empties every time the session waits for a
    /// key, but the transcript has not: it is in the terminal, ending in
    /// whatever went out last. A record that answered from what it holds alone
    /// would take every block after that for the first of the session, and
    /// part none of them from the one above.
    parted_before: bool,
    /// The line, by its number counted from the first of the session, that
    /// parts what is above it from what follows although it is not blank.
    ///
    /// A divider: a block that follows it asks for a blank row and is given
    /// none, because the divider is already the space between them.
    parting: Option<usize>,
    /// The mark a reply still being written is hung under, from
    /// [`Self::hangs`] until [`Self::subordinate`] or [`Self::unhangs`].
    hanging: Option<Hanging>,
    /// One past the last line written out to the terminal's own scrollback, in
    /// native mode, counted from the first of the session.
    ///
    /// Lines before it are kept for [`Self::kept`] to give again, and are not
    /// the live region's to draw. Never moves in the full screen, where
    /// nothing is sent and every line is the band's.
    sent: usize,
    /// The first line of what the record now holds, as it began: the first of
    /// the session, or the first after the boundary the last emptying left.
    ///
    /// Only the ceiling drops a line otherwise, so where [`Self::gone`] has
    /// moved past this, what [`Self::kept`] gives back is not all there was.
    began: usize,
    /// What the text of an answer links to: kept here rather than by the
    /// reader alone because the reader is made again for every message, and
    /// a drawn block is read again at every width and glyph set it is laid
    /// out for.
    links: Links,
    /// What draws a closed fence of an answer in place of the fence as it was
    /// written: kept here for the reason the links are, since a drawn block
    /// asks its drawer again at every width and glyph set it is laid out for.
    drawers: Drawers,
    /// The mode each kind a setting chooses for is drawn in, for the reason
    /// the drawers are kept.
    modes: Modes,
    /// What becomes of a key a drawer offers in the rows kept here: it
    /// stands, unless what draws this record said no key is read there.
    offers: Offers,
    /// What a block of an answer is shown as a picture by, and in.
    picturing: Picturing,
}

/// Where in the record a display row is: a line, and how far into it.
///
/// A line rather than a row number, so that a resize — which changes how many
/// rows every line folds to, and therefore what any row number meant — leaves
/// the reader looking at the line they were looking at.
/// Ordered head-first, which the derive gets right only because `line` is
/// declared before `into`: one spot is above another when its line is earlier,
/// and within a line when it is fewer rows in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Spot {
    /// The line, counted from the first of the session rather than the first
    /// still held: see [`Record::gone`].
    line: usize,
    /// How many of that line's display rows are above the band.
    into: u16,
}

impl Record {
    /// An empty record, to be drawn at `columns`.
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            tall: VecDeque::new(),
            ends: VecDeque::new(),
            before: 0,
            columns,
            weight: 0,
            rows: 0,
            gone: 0,
            top: Spot { line: 0, into: 0 },
            following: true,
            opening: None,
            landmarks: VecDeque::new(),
            shortened: VecDeque::new(),
            landed: None,
            parted_before: true,
            parting: None,
            open: false,
            hanging: None,
            sent: 0,
            began: 0,
            links: Links::default(),
            drawers: Drawers::default(),
            modes: Modes::default(),
            offers: Offers::Stand,
            picturing: Picturing::default(),
        }
    }

    /// What the text of an answer links to, here and in every reader of one.
    pub(crate) fn links(&self) -> &Links {
        &self.links
    }

    /// What the text of an answer links to, to be changed.
    ///
    /// Said at startup, before the first answer. A block already drawn takes
    /// the new links the next time it is laid out.
    pub(crate) fn links_mut(&mut self) -> &mut Links {
        &mut self.links
    }

    /// What draws a closed fence of an answer.
    pub(crate) fn drawers(&self) -> &Drawers {
        &self.drawers
    }

    /// Hands this record the drawers a closed fence of an answer is drawn by,
    /// and what becomes of a key their rows offer: `offers` is
    /// [`Offers::Withdrawn`] where nothing reads a key from whoever is shown
    /// these rows.
    ///
    /// Said at startup, before the first answer. A block already drawn asks
    /// them the next time it is laid out.
    pub(crate) fn draws_with(&mut self, drawers: Drawers, offers: Offers) {
        self.drawers = drawers;
        self.offers = offers;
    }

    /// What draws a closed `block` of an answer as it would be laid out here:
    /// the drawers, or none where its kind is shown as written.
    pub(crate) fn drawers_for(&self, block: &Block) -> Option<&Drawers> {
        self.modes.draws(block.kind).then_some(&self.drawers)
    }

    /// Draws each kind a setting chooses for in the mode `modes` gives it.
    ///
    /// Said at startup and whenever the setting changes; a block already
    /// drawn is laid out in it again by the next [`Self::relay`].
    pub(crate) fn draws_in(&mut self, modes: Modes) {
        self.modes = modes;
    }

    /// Whether a block of `kind` would be handed to its drawer here, rather
    /// than shown as written.
    pub(crate) const fn draws(&self, kind: Kind) -> bool {
        self.modes.draws(kind)
    }

    /// Append streamed text, which flows.
    ///
    /// A `\n` ends the line it is in rather than being drawn, and text with no
    /// `\n` in it continues whatever line is open — which is what makes a delta
    /// arriving mid-word land in the same line as the word it finishes.
    /// `link` is where the run goes when it is clicked, and `None` is the run
    /// that goes nowhere — which is nearly all of them. Carried alongside
    /// rather than written into the text, so a run that wraps stays one address
    /// on both of its rows and a row that is measured is measured in the
    /// columns a reader can see.
    pub(crate) fn write(&mut self, slot: Slot, text: &str, link: Option<&str>) {
        let mut rest = text;
        while let Some(at) = rest.find('\n') {
            self.extend(slot, &rest[..at], link);
            self.close();
            rest = &rest[at + 1..];
        }
        if !rest.is_empty() {
            self.extend(slot, rest, link);
        }
    }

    /// Append the quiet address the reader put after a link's words, which
    /// flows with them and is left out of what a drag copies.
    pub(crate) fn aside(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let (true, Some(Line::Flowed(row))) = (self.open, self.lines.back_mut()) {
            row.push_structural(Slot::Quiet, text);
            self.remeasure();
            return;
        }

        let mut row = Row::new();
        row.push_structural(Slot::Quiet, text);
        self.put(Line::Flowed(row));
        self.open = true;
    }

    /// Keeps the whole of `address`, which the line being written drew
    /// shorter, for [`Self::sources`] to list.
    ///
    /// Kept once: drawn again, it moves to the newest, which is where a
    /// reader looking for it last saw it.
    pub(crate) fn shortened(&mut self, address: &str) {
        let line = self.lines().saturating_sub(1);
        self.shortened.retain(|(_, kept)| **kept != *address);
        self.shortened.push_back((line, address.into()));
        while self.shortened.len() > MOST_SHORTENED {
            self.shortened.pop_front();
        }
        self.forget_shortened();
    }

    /// Append rows a component laid out, which are set.
    pub(crate) fn lay(&mut self, rows: impl IntoIterator<Item = Row>) {
        for row in rows {
            self.put(Line::Set(row));
        }
    }

    /// Lay down one block whose width-independent source remains available.
    ///
    /// One logical record line however many display rows the component needs.
    /// Its stable line number therefore survives a resize that changes its
    /// height, and only a viewport covering it clones the rows already laid out.
    pub(crate) fn responsive(&mut self, retained: usize, lay: Box<dyn Fn(usize) -> Vec<Row>>) {
        self.end();
        let rows = responsive_rows(lay(self.columns));
        self.hold(
            rows,
            retained,
            Box::new(move |columns, _| lay(columns)),
            None,
        );
    }

    /// Lay down one block drawn in `glyphs` whose source remains available.
    ///
    /// As [`Self::responsive`], and laid out again in the glyph set a reader
    /// switches to as well as at a new width: what is drawn in one set is a
    /// component drawn again in the other, not a row left in characters the
    /// reader turned away from.
    pub(crate) fn responsive_in(
        &mut self,
        retained: usize,
        glyphs: Glyphs,
        lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>,
    ) {
        self.end();
        let rows = responsive_rows(lay(self.columns, glyphs));
        self.hold(rows, retained, lay, Some(glyphs));
    }

    /// Lay down one block whose source is about the size of what it lays.
    ///
    /// As [`Self::responsive`], charged for the text of the rows laid for the
    /// width there is now, and laid out once: the rows that are measured are
    /// the rows that are kept.
    pub(crate) fn responsive_as_laid(&mut self, lay: Box<dyn Fn(usize) -> Vec<Row>>) {
        self.end();
        let rows = responsive_rows(lay(self.columns));
        let retained = rows.iter().map(Row::bytes).sum();
        self.hold(
            rows,
            retained,
            Box::new(move |columns, _| lay(columns)),
            None,
        );
    }

    /// Keeps a responsive block as `rows`, with `retained` bytes of source,
    /// laid out in `glyphs` where it is drawn in a glyph set.
    fn hold(
        &mut self,
        rows: Vec<Row>,
        retained: usize,
        lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>,
        glyphs: Option<Glyphs>,
    ) {
        if !rows.is_empty() {
            let weight = rows.len().max(retained.div_ceil(RETAINED_ROW_BYTES)).max(1);
            self.put(Line::Responsive {
                rows,
                prefix: None,
                weight,
                lay,
                glyphs,
            });
        }
    }

    /// Lay down a block of an answer, laid out for the width there is now in
    /// `glyphs` and `theme`, keeping the source it was written as.
    ///
    /// One logical line however many rows it draws, as a responsive block is.
    /// A block that lays out as nothing is not kept.
    pub(crate) fn drawn(&mut self, block: Block, glyphs: Glyphs, theme: Theme) {
        self.end();
        let room = Room {
            columns: self.columns,
            glyphs,
        };
        let by = By {
            links: &self.links,
            drawers: &self.drawers,
            modes: self.modes,
            offers: self.offers,
            window: self.picturing.window,
        };
        let mut drawn = Drawn::new(block, room, theme, by);
        if self.settle(&mut drawn, self.columns, glyphs) {
            let by = By {
                links: &self.links,
                drawers: &self.drawers,
                modes: self.modes,
                offers: self.offers,
                window: self.picturing.window,
            };
            self.picturing.saw(drawn.lay(self.columns, glyphs, by));
            self.remeasure_all();
        }
        if !drawn.rows.is_empty() {
            self.put(Line::Drawn(drawn));
        }
    }

    /// Puts down an image a turn named by `path`, read by the tool `by` where
    /// one read it, as a top edge naming it and under it the picture made of
    /// `png`, or as the edge alone with the reason `png` is not there.
    ///
    /// The picture is made only where a block would be shown as one: the
    /// terminal confirmed a protocol and colour is written. It is held to the
    /// caps every picture is, and the PNG is kept as what it was drawn from
    /// and nowhere else.
    pub(crate) fn image(
        &mut self,
        Named { path, by }: Named<'_>,
        png: Result<Vec<u8>, Undrawn>,
        glyphs: Glyphs,
        theme: Theme,
    ) {
        self.end();
        let columns = self.columns;
        let mut drawn = Drawn {
            block: Block {
                kind: Kind::Image,
                source: path.to_owned(),
                closed: true,
            },
            rows: Vec::new(),
            prefix: None,
            weight: 0,
            laid: (columns, glyphs, theme),
            modes: self.modes,
            edits: Vec::new(),
            picture: None,
            refused: None,
            fallen: false,
            image: Some(Box::new(Imaged {
                by: by.map(Into::into),
                refusal: None,
            })),
        };
        let made = png.and_then(|png| {
            let key = self
                .picture_key(Kind::Image, columns)
                .ok_or(Undrawn::NotHere)?;
            self.made_image(path, &key, png, columns)
                .map(|kept| (key, kept))
        });
        match made {
            Ok((key, kept)) => {
                self.make_room(kept.bytes(), columns, glyphs);
                let window = self.picturing.keeps.then_some(self.picturing.window);
                drawn.picture = Some(Box::new(Pictured {
                    id: self.picturing.number(),
                    key,
                    kept,
                    band: 0,
                    window,
                }));
            }
            Err(why) => {
                if let Some(image) = drawn.image.as_mut() {
                    image.refusal = Some(imaged(why));
                }
            }
        }
        let by = By {
            links: &self.links,
            drawers: &self.drawers,
            modes: self.modes,
            offers: self.offers,
            window: self.picturing.window,
        };
        self.picturing.saw(drawn.lay(columns, glyphs, by));
        self.put(Line::Drawn(drawn));
    }

    /// The picture of an image made from `png` under `key`, held to what a
    /// block's picture is held to.
    fn made_image(
        &mut self,
        path: &str,
        key: &PictureKey,
        png: Vec<u8>,
        columns: usize,
    ) -> Result<KeptPicture, Undrawn> {
        if self.picturing.left == 0 {
            return Err(Undrawn::PastCap);
        }
        let pictures = self.picturing.pictures.as_mut().ok_or(Undrawn::NotHere)?;
        let form = Form::Png(png);
        let payload = pictures.make(path, key, &form)?;
        let kept = KeptPicture {
            pixels: payload.pixels(),
            form,
            payload,
            size: PictureSize::Fit,
        };
        held(kept, columns)
    }

    /// Shows a block of an answer as a picture, made by `pictures`.
    ///
    /// Said once, by what composes the drawing. A block is shown as one only
    /// where its kind is drawn as an image, the terminal confirmed a protocol
    /// pictures are sent in ([`Self::pictured_as`]) and colour is written.
    pub(crate) fn pictures(&mut self, pictures: Box<dyn Pictures>) {
        self.picturing.pictures = Some(pictures);
    }

    /// Whether an image a turn named would be shown as a picture now.
    pub(crate) fn shows_pictures(&self) -> bool {
        self.picturing.pictures.is_some() && self.picture_key(Kind::Image, self.columns).is_some()
    }

    /// Numbers the next picture `first`, and those after it on from there.
    #[cfg(test)]
    pub(crate) fn numbers_from(&mut self, first: u32) {
        self.picturing.next = first;
    }

    /// Leaves `left` numbers to give out.
    #[cfg(test)]
    pub(crate) fn numbers_left(&mut self, left: u32) {
        self.picturing.left = left;
    }

    /// Shows pictures in what `images` says the terminal shows, worn in
    /// `palette`: said at startup and whenever either changes, and a block
    /// already drawn is shown in them by the next [`Self::relay`].
    pub(crate) fn pictured_as(&mut self, images: Capability, palette: PaletteIdentity) {
        self.picturing.images = images;
        self.picturing.palette = Some(palette);
    }

    /// The key a block of `kind` is shown as a picture under at `columns`,
    /// where it is shown as one at all.
    fn picture_key(&self, kind: Kind, columns: usize) -> Option<PictureKey> {
        let wanted = match kind {
            Kind::Mermaid => self.modes.of(kind) == Some(Mode::Image),
            Kind::Image => true,
            Kind::DisplayMath | Kind::Table | Kind::Rule | Kind::Heading(_) | Kind::Diff => false,
        };
        if !wanted {
            return None;
        }
        let Capability::Graphics { protocol, cell } = self.picturing.images else {
            return None;
        };
        let palette = self.picturing.palette.filter(PaletteIdentity::draws)?;
        Some(PictureKey {
            kind,
            bucket: Bucket::of(columns),
            palette,
            protocol,
            cell,
            rows: MOST_PLACED,
        })
    }

    /// Gives `drawn`, which is not among the record's lines while it is
    /// asked, the picture the record now shows it as, or takes away one it
    /// no longer is; answers whether that changed what it is laid out as.
    ///
    /// A picture it keeps under another key is encoded again from what it
    /// was drawn from; a block with none is asked of the cache first and of
    /// its drawer only where the cache has nothing. Where keeping the picture
    /// would pass [`MOST_KEPT_IMAGES`] or [`MOST_KEPT_IMAGE_BYTES`], the
    /// oldest kept is drawn as text instead, laid out again at `columns` in
    /// `glyphs`, until it fits.
    ///
    /// A block refused its picture is asked again in a new protocol or cell,
    /// and not at a new width: a width change encodes again only the
    /// pictures kept, so it costs what they do, however many diagrams the
    /// session has drawn as text. Nor is one refused for its size asked again
    /// in a new palette, which changes no size; one not drawn here is, since
    /// that is also what a picture refused for want of time is told. A Sixel
    /// picture kept under a key that differs from this one by its width alone
    /// is kept as it is, unless it is wanted now.
    fn settle(&mut self, drawn: &mut Drawn, columns: usize, glyphs: Glyphs) -> bool {
        // An image's picture is made once, from what the application read;
        // with none, there is nothing to make one from again.
        if drawn.image.is_some() && drawn.picture.is_none() {
            return false;
        }
        let key = if drawn.fallen || !drawn.block.closed {
            None
        } else {
            self.picture_key(drawn.block.kind, columns)
                .map(|key| PictureKey {
                    rows: self.encoded_rows(drawn, key.protocol),
                    ..key
                })
        };
        let Some(key) = key else {
            let had = drawn.picture.take().is_some();
            self.picturing.let_go |= had;
            return had;
        };
        let put_off = !self.picturing.now
            && drawn.picture.as_ref().is_some_and(|picture| {
                picture.key.protocol == Protocol::Sixel
                    && PictureKey {
                        bucket: picture.key.bucket,
                        rows: picture.key.rows,
                        ..key
                    } == picture.key
            });
        if put_off {
            return false;
        }
        if drawn
            .picture
            .as_ref()
            .is_some_and(|picture| picture.key == key)
            || drawn.refused.as_ref().is_some_and(|refused| {
                let (was, why) = (refused.0, refused.1);
                let palette = match why {
                    Undrawn::NotHere => was.palette,
                    Undrawn::PastCap
                    | Undrawn::NotText
                    | Undrawn::TooWide
                    | Undrawn::Unsupported(_)
                    | Undrawn::TooLargeToKeep
                    | Undrawn::ImageNotShown
                    | Undrawn::ImageTooLarge
                    | Undrawn::NotAnImage
                    | Undrawn::NotReadInTime
                    | Undrawn::ChangedSinceRead
                    | Undrawn::PastTurnImages
                    | Undrawn::NotRedrawnOnResume => key.palette,
                };
                PictureKey {
                    bucket: key.bucket,
                    palette,
                    rows: key.rows,
                    ..was
                } == key
            })
        {
            return false;
        }
        let mut key = key;
        let mut made = self.make(drawn, &key, glyphs, columns);
        // A Sixel picture the reader made larger, too large to keep at that
        // size or kept there only by drawing another as text: the size after
        // it, and back to the first, rather than text. At the first it is
        // what a picture nobody resized would be.
        for _ in 0..2 {
            let window = self.picturing.window;
            let passed = match &made {
                Err(Undrawn::TooLargeToKeep) => true,
                Ok(kept) => !self.room_for(kept.bytes()),
                Err(_) => false,
            };
            if !passed {
                break;
            }
            let Some(picture) = drawn.picture.as_mut().filter(|picture| {
                picture.key.protocol == Protocol::Sixel && picture.kept.size != PictureSize::Fit
            }) else {
                break;
            };
            picture.kept.size = picture.kept.size.next();
            key.rows = picture.rows(window);
            if key == picture.key {
                return false;
            }
            made = self.make(drawn, &key, glyphs, columns);
        }
        let band = drawn.picture.as_ref().map_or(0, |picture| picture.band);
        let had = drawn.picture.take().is_some();
        self.picturing.let_go |= had;
        let kept = match made {
            Ok(kept) => kept,
            Err(why) => {
                drawn.refused = Some(Box::new((key, why)));
                return had;
            }
        };
        self.make_room(kept.bytes(), columns, glyphs);
        drawn.refused = None;
        let window = self.picturing.keeps.then_some(self.picturing.window);
        drawn.picture = Some(Box::new(Pictured {
            id: self.picturing.number(),
            key,
            kept,
            band,
            window,
        }));
        true
    }

    /// The most rows `drawn`'s picture is encoded to cover by `protocol`: a
    /// Sixel bitmap is drawn cell for cell, so it is encoded for the rows it
    /// is shown in, and Kitty and iTerm2 scale one image into any cells, so
    /// theirs is encoded once for the most there are.
    fn encoded_rows(&self, drawn: &Drawn, protocol: Protocol) -> u16 {
        match protocol {
            Protocol::Kitty | Protocol::Iterm2 => MOST_PLACED,
            Protocol::Sixel => drawn.picture.as_ref().map_or_else(
                || PictureSize::Fit.rows(protocol, self.picturing.window, 0),
                |picture| picture.rows(self.picturing.window),
            ),
        }
    }

    /// `drawn`'s picture under `key`, or why it is drawn as text.
    fn make(
        &mut self,
        drawn: &Drawn,
        key: &PictureKey,
        glyphs: Glyphs,
        columns: usize,
    ) -> Result<KeptPicture, Undrawn> {
        if self.picturing.left == 0 {
            return Err(Undrawn::PastCap);
        }
        let Some(pictures) = self.picturing.pictures.as_mut() else {
            return Err(Undrawn::NotHere);
        };
        // An image is encoded again from the PNG it keeps, and has no drawer.
        if drawn.image.is_some() {
            let picture = drawn.picture.as_ref().ok_or(Undrawn::NotHere)?;
            let payload = pictures.reencode(&picture.kept, key)?;
            let kept = KeptPicture {
                pixels: payload.pixels(),
                form: picture.kept.form.clone(),
                payload,
                size: picture.kept.size,
            };
            return held(kept, columns);
        }
        let (drawer, source) =
            markdown::taken(&drawn.block, &self.drawers).ok_or(Undrawn::NotHere)?;
        let kept = if let Some(picture) = &drawn.picture {
            let payload = pictures.reencode(&picture.kept, key)?;
            KeptPicture {
                pixels: payload.pixels(),
                form: picture.kept.form.clone(),
                payload,
                size: picture.kept.size,
            }
        } else if let Some((form, payload)) = pictures.cached(source, key) {
            KeptPicture {
                pixels: payload.pixels(),
                form,
                payload,
                size: PictureSize::Fit,
            }
        } else {
            let form = Form::Svg(drawer.picture(source, &Room { columns, glyphs })?);
            let payload = pictures.make(source, key, &form)?;
            KeptPicture {
                pixels: payload.pixels(),
                form,
                payload,
                size: PictureSize::Fit,
            }
        };
        held(kept, columns)
    }

    /// Whether one more picture of `bytes` is kept beside every picture kept
    /// now, under both caps, with none drawn as text for it.
    fn room_for(&self, bytes: usize) -> bool {
        let (count, held) = self.kept_pictures();
        count < MOST_KEPT_IMAGES && held.saturating_add(bytes) <= MOST_KEPT_IMAGE_BYTES
    }

    /// Draws the oldest kept pictures as text, each laid out again at
    /// `columns` in `glyphs`, until one more of `bytes` fits under both caps.
    fn make_room(&mut self, bytes: usize, columns: usize, glyphs: Glyphs) {
        loop {
            if self.room_for(bytes) {
                return;
            }
            let by = By {
                links: &self.links,
                drawers: &self.drawers,
                modes: self.modes,
                offers: self.offers,
                window: self.picturing.window,
            };
            let oldest = self.lines.iter_mut().find_map(|line| match line {
                Line::Drawn(drawn) if drawn.picture.is_some() => Some(drawn),
                _ => None,
            });
            let Some(oldest) = oldest else {
                return;
            };
            oldest.picture = None;
            oldest.fallen = true;
            self.picturing.let_go = true;
            let was = oldest.weight;
            self.picturing.saw(oldest.lay(columns, glyphs, by));
            self.weight = self
                .weight
                .saturating_sub(was)
                .saturating_add(oldest.weight);
            self.remeasure_all();
        }
    }

    /// Encodes again for the width there is now each Sixel picture a band
    /// `rows` tall shows that was kept for another, and answers whether any
    /// was.
    ///
    /// Asked before the band is drawn. One encoded again may change height
    /// and so what the band shows, so the band is looked at again after each;
    /// each is encoded at most once, so this ends within the pictures kept.
    pub(crate) fn freshens(&mut self, rows: usize) -> bool {
        self.picturing.band = rows;
        let mut any = false;
        for _ in 0..=MOST_KEPT_IMAGES {
            let from = if self.following {
                self.foot(rows)
            } else {
                self.top
            };
            let mut at = from.line.saturating_sub(self.gone);
            let mut into = usize::from(from.into);
            let mut left = rows;
            let mut stale = None;
            while left > 0 {
                let Some(&tall) = self.tall.get(at) else {
                    break;
                };
                if self.stale(at) {
                    stale = Some(at);
                    break;
                }
                left = left.saturating_sub(usize::from(tall).saturating_sub(into));
                into = 0;
                at += 1;
            }
            let Some(at) = stale else {
                break;
            };
            self.freshen(at);
            any = true;
        }
        if any {
            self.spill();
        }
        any
    }

    /// Encodes again for the width there is now every Sixel picture kept for
    /// another, and answers whether any was: for a native session about to
    /// give back every line it keeps.
    pub(crate) fn freshens_all(&mut self) -> bool {
        let mut any = false;
        for at in 0..self.lines.len() {
            if self.stale(at) {
                self.freshen(at);
                any = true;
            }
        }
        if any {
            self.spill();
        }
        any
    }

    /// Whether line `at` of those held is a Sixel picture kept for a width
    /// other than the one there is now, or for other rows than its size
    /// shows it in now.
    fn stale(&self, at: usize) -> bool {
        let bucket = Bucket::of(self.columns);
        matches!(
            self.lines.get(at),
            Some(Line::Drawn(Drawn { picture: Some(picture), .. }))
                if picture.key.protocol == Protocol::Sixel
                    && (picture.key.bucket != bucket
                        || picture.key.rows != picture.rows(self.picturing.window))
        )
    }

    /// Settles line `at`'s picture as one wanted now, and lays it out again
    /// where that changed it.
    fn freshen(&mut self, at: usize) {
        let Some(Line::Drawn(mut drawn)) = self
            .lines
            .get_mut(at)
            .map(|line| std::mem::replace(line, Line::Set(Row::new())))
        else {
            return;
        };
        let (columns, glyphs) = (self.columns, drawn.laid.1);
        self.picturing.now = true;
        let settled = self.settle(&mut drawn, columns, glyphs);
        self.picturing.now = false;
        if settled {
            let by = By {
                links: &self.links,
                drawers: &self.drawers,
                modes: self.modes,
                offers: self.offers,
                window: self.picturing.window,
            };
            let was = drawn.weight;
            self.picturing.saw(drawn.lay(columns, glyphs, by));
            self.weight = self.weight.saturating_sub(was).saturating_add(drawn.weight);
        }
        if let Some(line) = self.lines.get_mut(at) {
            *line = Line::Drawn(drawn);
        }
        self.remeasure_at(at);
    }

    /// Works out line `at`'s height again, after its rows were laid again,
    /// keeping the band on the row it was at within the line where it can.
    fn remeasure_at(&mut self, at: usize) {
        let (Some(line), Some(&was)) = (self.lines.get(at), self.tall.get(at)) else {
            return;
        };
        let now = Self::measure(line, self.columns);
        if let Some(tall) = self.tall.get_mut(at) {
            *tall = now;
        }
        let (was, now16) = (usize::from(was), now);
        let now = usize::from(now);
        self.rows = self.rows.saturating_sub(was).saturating_add(now);
        for end in self.ends.iter_mut().skip(at) {
            *end = end.saturating_sub(was).saturating_add(now);
        }
        if self.top.line == self.gone + at {
            self.top.into = self.top.into.min(now16.saturating_sub(1));
        }
    }

    /// Bounds the pictures by a window `rows` tall: said before each
    /// [`Self::relay`], which shows each in the rows its size gives it there.
    ///
    /// Where `keeps`, as in a native session, a picture shown already keeps
    /// the rows it was first shown in and only a picture made from now on is
    /// bounded by this window: its cells in the terminal's scrollback were
    /// laid for those rows, and placing its number in others would show
    /// those cells broken.
    pub(crate) fn windowed(&mut self, rows: usize, keeps: bool) {
        self.picturing.window = rows;
        self.picturing.keeps = keeps;
    }

    /// Shows the picture of line `line`, counted from the first of the
    /// session, at the size after the one it is at, and answers whether it
    /// has one to show.
    ///
    /// A Kitty or iTerm2 picture is the same image in other cells, laid out
    /// again now; a Sixel one is encoded again for them by the frame it is
    /// next in view in, as one kept for another width is.
    pub(crate) fn cycles(&mut self, line: usize) -> bool {
        let Some(at) = line.checked_sub(self.gone) else {
            return false;
        };
        let by = By {
            links: &self.links,
            drawers: &self.drawers,
            modes: self.modes,
            offers: self.offers,
            window: self.picturing.window,
        };
        let Some(Line::Drawn(drawn)) = self.lines.get_mut(at) else {
            return false;
        };
        let Some(picture) = drawn.picture.as_mut() else {
            return false;
        };
        picture.kept.size = picture.kept.size.next();
        picture.band = self.picturing.band;
        let was = drawn.weight;
        let (columns, glyphs) = (drawn.laid.0, drawn.laid.1);
        self.picturing.saw(drawn.lay(columns, glyphs, by));
        self.weight = self.weight.saturating_sub(was).saturating_add(drawn.weight);
        self.remeasure_at(at);
        true
    }

    /// Whether line `line`, counted from the first of the session, is shown
    /// as a picture.
    pub(crate) fn pictured(&self, line: usize) -> bool {
        line.checked_sub(self.gone)
            .and_then(|at| self.lines.get(at))
            .is_some_and(|line| {
                matches!(
                    line,
                    Line::Drawn(Drawn {
                        picture: Some(_),
                        ..
                    })
                )
            })
    }

    /// Whether a picture the record kept has been let go since this was last
    /// asked: answered once.
    pub(crate) fn let_pictures_go(&mut self) -> bool {
        std::mem::take(&mut self.picturing.let_go)
    }

    /// Whether a picture the record keeps has been shown in other cells
    /// since this was last asked, as one whose size the reader or the
    /// window's height changed: answered once.
    pub(crate) fn pictures_reshaped(&mut self) -> bool {
        std::mem::take(&mut self.picturing.shown) == Shown::Reshaped
    }

    /// What the terminal is sent for picture `id`, while the record keeps it.
    pub(crate) fn payload(&self, id: u32) -> Option<&Payload> {
        self.kept_by_number()
            .find(|picture| picture.id == id)
            .map(|picture| &picture.kept.payload)
    }

    /// The number of every picture the record keeps, oldest first.
    pub(crate) fn picture_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.kept_by_number().map(|picture| picture.id)
    }

    /// Every picture the record keeps, oldest first.
    fn kept_by_number(&self) -> impl Iterator<Item = &Pictured> {
        self.lines.iter().filter_map(|line| match line {
            Line::Drawn(Drawn {
                picture: Some(picture),
                ..
            }) => Some(&**picture),
            _ => None,
        })
    }

    /// How many pictures the record keeps, and how many bytes they hold.
    fn kept_pictures(&self) -> (usize, usize) {
        self.lines
            .iter()
            .filter_map(|line| match line {
                Line::Drawn(Drawn {
                    picture: Some(picture),
                    ..
                }) => Some(picture.kept.bytes()),
                _ => None,
            })
            .fold((0, 0), |(count, bytes), more| {
                (count + 1, bytes.saturating_add(more))
            })
    }

    /// Lays the record out for `columns`, `glyphs` and `theme`.
    ///
    /// A block of an answer, and a responsive block drawn in a glyph set, is
    /// laid out again where the width or the glyph set it was laid for has
    /// changed, and a block of an answer where the mode its kind is drawn in
    /// has; the theme changes only the colours its rows are painted in,
    /// which is the drawing's business. A new width is everything
    /// [`Self::resized`] does as well.
    pub(crate) fn relay(&mut self, columns: usize, glyphs: Glyphs, theme: Theme) {
        if columns == self.columns {
            let drawn = self.relay_drawn(columns, glyphs);
            let responsive = self.relay_responsive(columns, glyphs, false);
            if drawn || responsive {
                self.remeasure_all();
                self.spill();
            }
        } else {
            self.resized(columns, glyphs);
        }
        for line in &mut self.lines {
            if let Line::Drawn(drawn) = line {
                drawn.laid.2 = theme;
            }
        }
    }

    /// Lays out again each drawn block not laid for `columns` in `glyphs`, or
    /// in the mode its kind is drawn in now, and answers whether any was.
    fn relay_drawn(&mut self, columns: usize, glyphs: Glyphs) -> bool {
        let mut any = false;
        for at in 0..self.lines.len() {
            let Some(line) = self.lines.get_mut(at) else {
                continue;
            };
            // Out of the lines while its picture is settled, since keeping
            // one may draw an older picture as text.
            let Line::Drawn(drawn) = line else {
                continue;
            };
            let kind = drawn.block.kind;
            let window = self.picturing.window;
            // A window of another height shows a picture in other rows.
            let reshown = drawn.picture.as_ref().is_some_and(|picture| {
                let payload = &picture.kept.payload;
                payload.cells() != payload.shown_at(picture.rows(window))
            });
            let unlaid = (drawn.laid.0, drawn.laid.1) != (columns, glyphs)
                || drawn.modes.of(kind) != self.modes.of(kind)
                || reshown;
            let wants = drawn.picture.is_some()
                || (!drawn.fallen && self.picture_key(kind, columns).is_some());
            if !unlaid && !wants {
                continue;
            }
            let Some(Line::Drawn(mut drawn)) = self
                .lines
                .get_mut(at)
                .map(|line| std::mem::replace(line, Line::Set(Row::new())))
            else {
                continue;
            };
            let settled = self.settle(&mut drawn, columns, glyphs);
            if unlaid || settled {
                let by = By {
                    links: &self.links,
                    drawers: &self.drawers,
                    modes: self.modes,
                    offers: self.offers,
                    window: self.picturing.window,
                };
                let was = drawn.weight;
                self.picturing.saw(drawn.lay(columns, glyphs, by));
                self.weight = self.weight.saturating_sub(was).saturating_add(drawn.weight);
                any = true;
            }
            if let Some(line) = self.lines.get_mut(at) {
                *line = Line::Drawn(drawn);
            }
        }
        any
    }

    /// Lay down the opening in `glyphs`, keeping what laid it.
    ///
    /// The one block a resize draws again — see the prose at the top of this
    /// file for why this one and nothing else.
    pub(crate) fn opens(&mut self, glyphs: Glyphs, lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>) {
        self.end();
        let from = self.gone + self.lines.len();
        let laid = lay(self.columns, glyphs);
        let lines = laid.len();
        self.lay(laid);
        self.opening = Some(Opening { from, lines, lay });
    }

    /// Draw the opening again for `columns` in `glyphs`, in the lines it
    /// already holds.
    ///
    /// Before the heights are worked out again rather than after: this changes
    /// which lines there are, and [`Self::resized`] is what measures them.
    fn relay_opening(&mut self, columns: usize, glyphs: Glyphs) {
        let Some(opening) = self.opening.take() else {
            return;
        };

        // Part of it has fallen off the top, so what is left is no longer a
        // card — it is the bottom of one. Dropped rather than redrawn, which
        // leaves those lines standing as any other set rows do.
        if opening.from < self.gone {
            return;
        }

        // Some or all of it has gone out to the terminal's scrollback. Laying
        // it again here would change how many lines there are under every
        // number the session has handed out since, so it is kept as it was
        // laid, and [`Self::kept`] lays it afresh once all of it has gone,
        // which a replay sees to through [`Self::lets_opening_go`].
        if opening.from < self.sent {
            self.opening = Some(opening);
            return;
        }

        let at = opening.from - self.gone;
        let laid = (opening.lay)(columns, glyphs);
        let lines = laid.len();

        for _ in 0..opening.lines {
            self.lines.remove(at);
            self.weight = self.weight.saturating_sub(1);
        }
        for (step, row) in laid.into_iter().enumerate() {
            self.lines.insert(at + step, Line::Set(row));
            self.weight = self.weight.saturating_add(1);
        }

        // The reader's place is a line number, and there are now a different
        // number of lines above it. One inside the card has nowhere of its own
        // to go back to, so it goes to the top of the card.
        let past = opening.from + opening.lines;
        if self.top.line >= past {
            self.top.line = (self.top.line + lines).saturating_sub(opening.lines);
        } else if self.top.line > opening.from {
            self.top.line = opening.from;
        }
        for landmark in self.landmarks.iter_mut().chain(self.landed.as_mut()) {
            if *landmark >= past {
                *landmark = (*landmark + lines).saturating_sub(opening.lines);
            } else if *landmark > opening.from {
                *landmark = opening.from;
            }
        }

        self.opening = Some(Opening { lines, ..opening });
    }

    /// Takes the open line back, as if none of it had been written.
    ///
    /// For a line that turned out to be the first of a block laid out from its
    /// source instead, or the title of a heading, which is written again. Only
    /// the open line can go: a line that has ended may
    /// already be in the terminal's own scrollback, and nothing takes it out,
    /// which is why a line in doubt is kept open until it is settled.
    pub(crate) fn retract(&mut self) {
        if !self.open || !matches!(self.lines.back(), Some(Line::Flowed(_))) {
            return;
        }

        self.open = false;
        if let Some(line) = self.lines.pop_back() {
            self.weight = self.weight.saturating_sub(line.weight());
        }
        let tall = self.tall.pop_back().unwrap_or(0);
        self.ends.pop_back();
        self.rows = self.rows.saturating_sub(usize::from(tall));
        if self.top.line >= self.lines() {
            self.top = Spot {
                line: self.lines(),
                into: 0,
            };
        }
    }

    /// Add `text` to the open line, opening one if none is.
    fn extend(&mut self, slot: Slot, text: &str, link: Option<&str>) {
        if let (true, Some(Line::Flowed(row))) = (self.open, self.lines.back_mut()) {
            Self::push_run(row, slot, text, link);
            self.remeasure();
            return;
        }

        let mut row = Row::new();
        Self::push_run(&mut row, slot, text, link);
        self.put(Line::Flowed(row));
        self.open = true;
    }

    /// Appends one run, with or without somewhere for it to go.
    fn push_run(row: &mut Row, slot: Slot, text: &str, link: Option<&str>) {
        match link {
            Some(link) => row.push_linked(slot, text, link),
            None => row.push(slot, text),
        }
    }

    /// End the open line, so the next text starts a new one.
    ///
    /// A newline with no line open is a line somebody left blank, which is the
    /// row between two paragraphs and is drawn. Which is what separates this
    /// from [`Self::end`], where nothing open means nothing to do.
    fn close(&mut self) {
        if self.open {
            self.open = false;
            return;
        }
        self.put(Line::Flowed(Row::new()));
    }

    /// Add a line, dropping the oldest if the record is full.
    fn put(&mut self, line: Line) {
        self.open = false;
        let tall = Self::measure(&line, self.columns);
        self.weight = self.weight.saturating_add(line.weight());
        self.lines.push_back(line);
        self.tall.push_back(tall);
        let after = self.ends.back().copied().unwrap_or(self.before) + usize::from(tall);
        self.ends.push_back(after);
        self.rows += usize::from(tall);
        self.spill();
    }

    /// Drops oldest logical lines until the record is back under its ceiling.
    fn spill(&mut self) {
        while self.weight > MOST && self.lines.len() > 1 {
            self.drop_oldest();
        }
        self.forget_landmarks();
        self.forget_shortened();
    }

    /// Drops the oldest line held, counting it as gone.
    fn drop_oldest(&mut self) {
        if let Some(line) = self.lines.pop_front() {
            self.weight = self.weight.saturating_sub(line.weight());
            self.parted_before = Self::blank(&line) || self.parting == Some(self.gone);
            if let Line::Drawn(Drawn {
                picture: Some(_), ..
            }) = line
            {
                self.picturing.let_go = true;
            }
        }
        let tall = self.tall.pop_front().unwrap_or(0);
        self.before = self.ends.pop_front().unwrap_or(self.before);
        self.rows -= usize::from(tall);
        self.gone += 1;
    }

    /// Lets go of the prompt landmarks whose lines have gone.
    fn forget_landmarks(&mut self) {
        while self.landmarks.front().is_some_and(|line| *line < self.gone) {
            self.landmarks.pop_front();
        }
    }

    /// Lets go of the addresses kept whole whose lines have gone.
    fn forget_shortened(&mut self) {
        while self
            .shortened
            .front()
            .is_some_and(|(line, _)| *line < self.gone)
        {
            self.shortened.pop_front();
        }
    }

    /// Work out the last line's height again, after text was added to it.
    fn remeasure(&mut self) {
        let Some(line) = self.lines.back() else {
            return;
        };
        let now = Self::measure(line, self.columns);
        let Some(was) = self.tall.back_mut() else {
            return;
        };
        self.rows = self.rows - usize::from(*was) + usize::from(now);
        if let Some(after) = self.ends.back_mut() {
            *after = after.saturating_sub(usize::from(*was)) + usize::from(now);
        }
        *was = now;
    }

    /// Work out every retained line's height again after rows were rewritten.
    fn remeasure_all(&mut self) {
        self.rows = 0;
        self.tall.clear();
        self.ends.clear();
        let mut after = self.before;
        for line in &self.lines {
            let tall = Self::measure(line, self.columns);
            self.tall.push_back(tall);
            self.rows += usize::from(tall);
            after += usize::from(tall);
            self.ends.push_back(after);
        }
        self.top.into = 0;
    }

    /// How many display rows a line comes to at the current width.
    ///
    /// Saturating rather than wrapping: a line taller than a `u16` is one
    /// nobody can read anyway, and the alternative is arithmetic that is right
    /// until somebody pastes a megabyte.
    fn measure(line: &Line, columns: usize) -> u16 {
        match line {
            Line::Flowed(row) => u16::try_from(flowed_height(row, columns)).unwrap_or(u16::MAX),
            Line::Responsive { rows, .. } | Line::Drawn(Drawn { rows, .. }) => {
                u16::try_from(rows.len()).unwrap_or(u16::MAX)
            }
            Line::Set(_) => 1,
        }
    }

    /// Whether a line is still being written to.
    pub(crate) fn writing(&self) -> bool {
        self.open
    }

    /// End whatever line is open, and add nothing where none is.
    ///
    /// What a caller says before putting down something that is a line in its
    /// own right — a row a component laid out, or the end of a message.
    pub(crate) fn end(&mut self) {
        self.open = false;
    }

    /// Whether the record already ends in a blank line.
    ///
    /// What a caller asks before putting one there, so that two things that
    /// each want space around them get one row between them rather than two.
    /// A record nobody has written to is parted: there is nothing above to be
    /// parted from. One holding nothing because it let go of everything it had
    /// ends in whatever it let go of last, which is still above. A line marked
    /// by [`Self::parts`] parts as a blank one does.
    pub(crate) fn parted(&self) -> bool {
        match self.lines.back() {
            None => self.parted_before,
            Some(Line::Flowed(_)) if self.open => false,
            Some(line) => Self::blank(line) || self.parting == self.lines().checked_sub(1),
        }
    }

    /// Marks the last line as one that parts what follows it from what is
    /// above, as a blank line would.
    pub(crate) fn parts(&mut self) {
        if !self.lines.is_empty() {
            self.parting = Some(self.lines() - 1);
        }
    }

    /// Whether the last line is one [`Self::parts`] marked, with nothing open
    /// after it.
    ///
    /// Asked even once that line has gone out and been let go of: the count
    /// of lines goes on including it, so it is still the last.
    pub(crate) fn divided(&self) -> bool {
        !self.open && self.parting.is_some() && self.parting == self.lines().checked_sub(1)
    }

    /// Whether a finished line is a row of nothing.
    fn blank(line: &Line) -> bool {
        match line {
            Line::Flowed(row) | Line::Set(row) => row.text().trim().is_empty(),
            Line::Responsive { rows, .. } | Line::Drawn(Drawn { rows, .. }) => {
                rows.last().is_none_or(|row| row.text().trim().is_empty())
            }
        }
    }

    /// Drop every line, leaving the record empty and the numbering where it was.
    ///
    /// What a session picked up asks for: the transcript it replaces is the
    /// whole of the band, so the lines that were there go rather than being
    /// pushed up out of sight. Nothing else in this process is asked to forget
    /// with it — the numbering carries on past the lines it dropped and one
    /// reset boundary, so a number some other part of the program is holding
    /// still names the line it named, and names nothing once that line has gone.
    ///
    /// The opening goes too, and cannot come back: what a resize redraws is a
    /// card whose lines are still held, and these are not. So do the lines a
    /// native session kept to give again after a resize: what replaces them
    /// is the conversation a resize now gives back.
    pub(crate) fn empties(&mut self) {
        // Advance past a reset boundary as well as every removed line, so a
        // stable boundary taken before the reset cannot alias the first line of
        // the replacement conversation.
        self.gone += self.lines.len() + 1;
        let kept = self.kept_by_number().next().is_some();
        self.picturing.let_go |= kept;
        self.lines.clear();
        self.tall.clear();
        self.ends.clear();
        self.before = 0;
        self.weight = 0;
        self.rows = 0;
        self.top = Spot {
            line: self.gone,
            into: 0,
        };
        self.following = true;
        self.opening = None;
        self.open = false;
        self.parted_before = true;
        self.began = self.gone;
    }

    /// Drop every line as [`Self::empties`] does, where what was said is still
    /// above what replaces it.
    ///
    /// What emptying means in the terminal's own buffer, which keeps what went
    /// out: the next block is parted from the last row written there, not from
    /// nothing, so whether that row was blank is kept.
    pub(crate) fn empties_under(&mut self) {
        let parted = self.parted();
        self.empties();
        self.parted_before = parted;
    }

    /// How many lines the session has taken, including those since dropped.
    ///
    /// Counted from the first line of the session rather than the first still
    /// held, so a number kept by a caller goes on naming the same line after
    /// the record has spilled underneath it.
    pub(crate) fn lines(&self) -> usize {
        self.gone + self.lines.len()
    }

    /// Hangs the output written since `from` under one structural mark.
    ///
    /// The first retained display row receives the mark. Every row after it is
    /// indented to the same text column, making a multiline answer one block
    /// rather than a list of unrelated results. If the beginning spilled or the
    /// command replaced the transcript, nothing is changed: the retained rows no
    /// longer identify one complete answer block.
    ///
    /// Where [`Self::hangs`] was told of `from` before the output began, the
    /// lines it has already marked are left as they are and the rest are
    /// marked after them.
    pub(crate) fn subordinate(&mut self, from: usize, mark: &str) {
        self.end();
        let (next, mut first) = self
            .hanging
            .take()
            .filter(|hanging| hanging.from == from)
            .map_or((from, true), |hanging| (hanging.next, hanging.first));
        if next < self.first() || next >= self.lines() || mark.is_empty() {
            return;
        }
        self.mark(next..self.lines(), mark, &mut first);
    }

    /// Hangs what is written from here on under `mark`, before it is written.
    ///
    /// For a reply that may leave the record before it ends: each line of it
    /// is marked as [`Self::hangs_through`] lets it go, and
    /// [`Self::subordinate`], given the line this started at, marks the rest.
    /// Emptying the record forgets it.
    pub(crate) fn hangs(&mut self, mark: &str) {
        let from = self.lines();
        self.hanging = (!mark.is_empty()).then(|| Hanging {
            from,
            next: from,
            mark: mark.into(),
            first: true,
        });
    }

    /// Forgets the mark [`Self::hangs`] hung, marking nothing more under it.
    pub(crate) fn unhangs(&mut self) {
        self.hanging = None;
    }

    /// Marks the lines of a hung reply before `through`, which are about to
    /// be let go of.
    ///
    /// Nothing where no reply is hung, and the mark is forgotten where lines
    /// of the reply have already gone unmarked, as [`Self::subordinate`]
    /// forgets a block whose beginning spilled.
    pub(crate) fn hangs_through(&mut self, through: usize) {
        let Some(mut hanging) = self.hanging.take() else {
            return;
        };
        if hanging.next < self.first() {
            return;
        }
        let through = through.min(self.lines());
        if hanging.next < through {
            let mark = std::mem::take(&mut hanging.mark);
            self.mark(hanging.next..through, &mark, &mut hanging.first);
            hanging.mark = mark;
            hanging.next = through;
        }
        self.hanging = Some(hanging);
    }

    /// Puts `mark` on the first row of `lines` still owed it while `first`,
    /// and its indent on every other row; lines of the opening are skipped.
    fn mark(&mut self, lines: Range<usize>, mark: &str, first: &mut bool) {
        let opening = self
            .opening
            .as_ref()
            .map(|opening| opening.from..opening.from + opening.lines);
        let subordinate = Subordinate {
            mark: mark.into(),
            opening: crate::width::columns(mark),
            marks_first: true,
            keeps_first: false,
        };
        let gone = self.gone;

        for (at, line) in self
            .lines
            .iter_mut()
            .enumerate()
            .skip(lines.start - gone)
            .take(lines.len())
        {
            if opening
                .as_ref()
                .is_some_and(|opening| opening.contains(&(gone + at)))
            {
                continue;
            }
            match line {
                Line::Flowed(row) | Line::Set(row) => {
                    if *first && row.starts_structural() {
                        *first = false;
                    } else {
                        row.prepend(subordinate.row(first));
                    }
                }
                Line::Responsive { rows, prefix, .. } | Line::Drawn(Drawn { rows, prefix, .. }) => {
                    if *first && rows.first().is_some_and(Row::starts_structural) {
                        *first = false;
                        let mut retained = subordinate.clone();
                        retained.marks_first = false;
                        retained.keeps_first = true;
                        for row in rows.iter_mut().skip(1) {
                            row.prepend(subordinate.row(first));
                        }
                        *prefix = Some(retained);
                        continue;
                    }
                    let mut retained = subordinate.clone();
                    retained.marks_first = *first && !rows.is_empty();
                    retained.keeps_first = false;
                    for row in rows {
                        row.prepend(subordinate.row(first));
                    }
                    *prefix = Some(retained);
                }
            }
        }

        self.remeasure_all();
    }

    /// Edits the rows line `at` was written as, now and at every width it is
    /// laid out at again.
    ///
    /// For a row that has to say less than it did when it was written: an
    /// offer whose result has gone. The line keeps its place and its share of
    /// the record; only its rows change. A line no longer held, or not written
    /// yet, is left alone, and so is every other line.
    pub(crate) fn amend(&mut self, at: usize, edit: fn(&mut [Row])) {
        let Some(line) = at
            .checked_sub(self.gone)
            .and_then(|held| self.lines.get_mut(held))
        else {
            return;
        };

        match line {
            Line::Flowed(row) | Line::Set(row) => edit(std::slice::from_mut(row)),
            Line::Responsive { rows, lay, .. } => {
                edit(rows);
                // Kept with what lays the block out, or the next width would
                // lay it out as it was written.
                let before = std::mem::replace(lay, Box::new(|_, _| Vec::new()));
                *lay = Box::new(move |columns, glyphs| {
                    let mut laid = before(columns, glyphs);
                    edit(&mut laid);
                    laid
                });
            }
            Line::Drawn(drawn) => {
                edit(&mut drawn.rows);
                drawn.edits.push(edit);
            }
        }

        self.remeasure_all();
    }

    /// Marks the next line laid down as the start of a prompt.
    ///
    /// Called after the blank that parts blocks and before the prompt's own
    /// rows. Repeated calls at the same boundary make one landmark: replay and
    /// a live prompt use the same door, and neither owes a double mark.
    pub(crate) fn landmark(&mut self) {
        let line = self.lines();
        if self.landmarks.back() == Some(&line) {
            return;
        }
        self.landmarks.push_back(line);
        self.landed = None;
        while self.landmarks.len() > MOST_LANDMARKS {
            self.landmarks.pop_front();
        }
    }
}

/// What an address drawn shorter than it was written is called where it is
/// listed whole.
pub(crate) const ADDRESS: &str = "address";

/// A drawn block's source, or the whole of an address drawn shorter than it
/// was written, borrowed from the record that keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source<'a> {
    /// The line it is held as, numbered as the record numbers its lines.
    pub line: usize,
    /// What the block is called: `table`, `math`, `mermaid diagram`, `diff`;
    /// or `address`, for an address drawn shorter than it was written.
    pub word: &'static str,
    /// The text it was written as.
    pub text: &'a str,
}

impl Source<'_> {
    /// Whether this is an address drawn shorter, of which a line may hold
    /// several, rather than a block, which is a line of its own.
    #[must_use]
    pub fn is_address(&self) -> bool {
        self.word == ADDRESS
    }
}

/// What native mode hands over to the terminal's own scrollback.
///
/// A line written into the reader's buffer is shown by the terminal from then
/// on, and the live region no longer draws it. It is kept here all the same,
/// under the one ceiling, because the terminal cannot be trusted to put what it
/// holds at a new width, and a resize is answered by giving it all again.
impl Record {
    /// The source of the drawn block that shows at absolute display row
    /// `row`, lent from the record.
    ///
    /// None for a row of anything else, for a block of a kind with no source
    /// worth reading whole, and for a row the record no longer holds.
    pub(crate) fn source_of(&self, row: usize) -> Option<&str> {
        if row < self.before || self.last_row().is_none_or(|last| row > last) {
            return None;
        }
        let line = self.spot_at(row).line;
        match self.lines.get(line.checked_sub(self.gone)?)? {
            Line::Drawn(drawn) => drawn.source(line).map(|source| source.text),
            _ => None,
        }
    }

    /// The line, counted from the first of the session, of the block whose
    /// label absolute display row `row` is: the first row of a block drawn
    /// as text with a source to read, where it is the top edge of a frame.
    pub(crate) fn labelled(&self, row: usize) -> Option<usize> {
        if row < self.before || self.last_row().is_none_or(|last| row > last) {
            return None;
        }
        let spot = self.spot_at(row);
        if spot.into != 0 {
            return None;
        }
        match self.lines.get(spot.line.checked_sub(self.gone)?)? {
            Line::Drawn(drawn) if drawn.labelled() => Some(spot.line),
            _ => None,
        }
    }

    /// Every drawn block held that has a source to read, newest first, and
    /// then the whole of every address drawn shorter than it was written that
    /// is still held, newest first, each called [`ADDRESS`].
    pub(crate) fn sources(&self) -> impl Iterator<Item = Source<'_>> + '_ {
        let blocks = self
            .lines
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(held, line)| match line {
                Line::Drawn(drawn) => drawn.source(self.gone + held),
                _ => None,
            });
        let addresses = self
            .shortened
            .iter()
            .rev()
            .filter(|(line, _)| *line >= self.gone)
            .map(|(line, address)| Source {
                line: *line,
                word: ADDRESS,
                text: address,
            });
        blocks.chain(addresses)
    }

    /// One past the line of the newest of [`Self::sources`], block or address,
    /// and zero where there is none.
    pub(crate) fn sourced(&self) -> usize {
        let block = self
            .sources()
            .next()
            .filter(|source| source.word != ADDRESS);
        let address = self
            .shortened
            .back()
            .filter(|(line, _)| *line >= self.gone)
            .map(|(line, _)| *line);
        block
            .map(|source| source.line)
            .max(address)
            .map_or(0, |line| line.saturating_add(1))
    }

    /// The first line held that has not gone out, numbered as
    /// [`Self::lines`] numbers them.
    pub(crate) fn first(&self) -> usize {
        self.sent.max(self.gone)
    }

    /// One past the last line that can no longer change: every line held but
    /// the one still being written to.
    pub(crate) fn finished(&self) -> usize {
        self.lines() - usize::from(self.open)
    }

    /// How many display rows the held lines from `line` on come to.
    pub(crate) fn rows_from(&self, line: usize) -> usize {
        self.tall
            .iter()
            .skip(line.saturating_sub(self.gone))
            .map(|tall| usize::from(*tall))
            .sum()
    }

    /// Line `line` as the display rows it comes to now; none where it is not
    /// held.
    pub(crate) fn folded(&self, line: usize) -> Vec<Row> {
        line.checked_sub(self.gone)
            .and_then(|held| self.lines.get(held))
            .map(|held| self.fold(held))
            .unwrap_or_default()
    }

    /// Marks every line before `through` as written out, keeping it.
    ///
    /// The live region draws nothing before it from then on, and the lines
    /// stay for [`Self::kept`] until the ceiling drops them.
    pub(crate) fn lets_go(&mut self, through: usize) {
        self.sent = self.sent.max(through.min(self.lines()));
        let first = self.first();
        if self.top.line < first {
            self.top = Spot {
                line: first,
                into: 0,
            };
        }
    }

    /// Lets the rest of an opening that has partly gone out go too.
    ///
    /// For a replay, before it asks what has gone out: its lines are held as
    /// they were laid for a width that has gone, and with only its top gone
    /// out, [`Self::kept`] would give that top back as it was laid and the
    /// region would draw the rest under it the same way, a border cut at the
    /// new width. Gone out whole, it is laid whole at the width there is now.
    /// A card none of which has gone out was laid again on the resize, and
    /// one that has partly fallen off the top is no longer a card.
    pub(crate) fn lets_opening_go(&mut self) {
        let end = self
            .opening
            .as_ref()
            .filter(|opening| opening.from >= self.gone && opening.from < self.sent)
            .map(|opening| opening.from + opening.lines);
        if let Some(end) = end {
            self.lets_go(end);
        }
    }

    /// Hands `each` every line that has gone out and is still kept, as the
    /// display rows it comes to at the current width, with an opening among
    /// them laid afresh in `glyphs`.
    ///
    /// What a resize writes again once the screen and its scrollback have been
    /// cleared. The card is laid here rather than in the record, so that the
    /// lines under it keep the numbers they were handed out with. A card only
    /// partly gone out is given as the lines it already holds; a replay lets
    /// the rest of it go first, through [`Self::lets_opening_go`], so the card
    /// it is given is laid whole. Row by row rather than as one list, so
    /// giving the session back makes no second copy of it.
    pub(crate) fn kept(&self, glyphs: Glyphs, mut each: impl FnMut(Row)) {
        let first = self.gone;
        let sent = self.first();
        let opening = self
            .opening
            .as_ref()
            .filter(|opening| opening.from >= first && opening.from + opening.lines <= sent);
        let mut line = first;
        while line < sent {
            if let Some(opening) = opening.filter(|opening| opening.from == line) {
                let columns = self.columns;
                for row in (opening.lay)(columns, glyphs) {
                    each(row.clipped(columns));
                }
                line += opening.lines.max(1);
                continue;
            }
            for row in self.folded(line) {
                each(row);
            }
            line += 1;
        }
    }

    /// Whether the ceiling has dropped lines that [`Self::kept`] would
    /// otherwise have given back, since the record was last emptied.
    pub(crate) fn spilled(&self) -> bool {
        self.gone > self.began
    }
}

/// The viewport: which of the record the transcript band is showing.
impl Record {
    /// Draw the band, `rows` tall, as the display rows to put in it.
    ///
    /// Only the lines the band covers are folded, which is what keeps a frame
    /// proportional to the window rather than to the session. Fewer rows than
    /// asked for means the record does not fill the band yet, and the rest of
    /// the band is under what there is: a session that has just started reads
    /// from the top of the window down, as a terminal's own would.
    pub(crate) fn view(&self, rows: usize) -> Vec<Row> {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };
        let mut out = Vec::with_capacity(rows);
        let mut into = usize::from(from.into);
        for line in self.lines.iter().skip(from.line.saturating_sub(self.gone)) {
            for row in self.fold(line).into_iter().skip(into) {
                out.push(row);
                if out.len() == rows {
                    return out;
                }
            }
            into = 0;
        }
        out
    }

    /// Move the band `by` display rows, positive downwards, and say whether
    /// anything moved.
    ///
    /// Clamped at both ends rather than wrapping or refusing: a wheel spun hard
    /// at the foot of a session should stop there, not travel and come back.
    /// Which line is `into` rows down a band `rows` tall, if any is.
    ///
    /// What answers a click: the band draws lines and the terminal reports a
    /// row, and this is the one place both are known. Counted off [`Self::tall`]
    /// rather than by folding, because where a row lands does not need the
    /// words in it.
    pub(crate) fn at(&self, into: usize, rows: usize) -> Option<usize> {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };

        let mut left = into;
        let mut skip = usize::from(from.into);
        // Saturating because the line the reader was on can have spilled since
        // they stopped there, leaving `from` above `gone` — the state the
        // scroll tests pin as intended. Every sibling walking this state does
        // the same.
        for (at, tall) in self
            .tall
            .iter()
            .enumerate()
            .skip(from.line.saturating_sub(self.gone))
        {
            let shown = usize::from(*tall).saturating_sub(skip);
            skip = 0;
            if left < shown {
                return Some(self.gone + at);
            }
            left -= shown;
        }

        None
    }

    /// Which rows of a band `rows` tall are showing the line numbered `at`.
    ///
    /// The other half of [`Record::at`], and worked out the same way: that one
    /// says which line a row is showing, this one says every row showing a
    /// line. Empty for a line the band is not showing.
    ///
    /// What the pair is for is a cut result long enough to wrap. It is one
    /// result however many rows it folded to, so a pointer anywhere on it is
    /// pointing at the whole of it -- and at none of what is above or below.
    pub(crate) fn covering(&self, at: usize, rows: usize) -> Range<usize> {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };

        let mut start = 0;
        let mut skip = usize::from(from.into);
        for (line, tall) in self
            .tall
            .iter()
            .enumerate()
            .skip(from.line.saturating_sub(self.gone))
        {
            let shown = usize::from(*tall).saturating_sub(skip);
            skip = 0;
            if self.gone + line == at {
                return start..(start + shown).min(rows);
            }
            start += shown;
            if start >= rows {
                break;
            }
        }

        start.min(rows)..start.min(rows)
    }

    /// Whether the line numbered `at` carries a span in `slot`.
    ///
    /// Numbered as [`Record::at`] hands them back, which is what makes the two
    /// a pair: one says which line a window row is showing and this says what
    /// kind of line it is. Asked of the line rather than of the display row it
    /// folded to, so a cut result long enough to wrap is one cut result on
    /// every row of itself.
    ///
    /// `false` for a line the record has since dropped, which is the honest
    /// answer: what is no longer held is not on screen either.
    pub(crate) fn wears(&self, at: usize, slot: Slot) -> bool {
        let Some(line) = at.checked_sub(self.gone).and_then(|at| self.lines.get(at)) else {
            return false;
        };

        match line {
            Line::Flowed(row) | Line::Set(row) => row.kinds().any(|kind| kind == slot),
            Line::Responsive { rows, .. } | Line::Drawn(Drawn { rows, .. }) => {
                rows.iter().any(|row| row.kinds().any(|kind| kind == slot))
            }
        }
    }

    /// The first line of the run of cut lines `at` is in, or `at` itself where
    /// it is not in one.
    ///
    /// A result written down over several lines is one result, and the offer to
    /// open it was made on the first of them. Nothing else is written down in
    /// the middle of a result, so the run of lines wearing the cut slot around
    /// `at` is that result and no other — the same reading the pointer's light
    /// makes, so what lights under a click is what the click opens.
    pub(crate) fn heads(&self, at: usize) -> usize {
        if !self.wears(at, Slot::Cut) {
            return at;
        }

        let mut first = at;
        while first > self.gone && self.wears(first - 1, Slot::Cut) {
            first -= 1;
        }
        first
    }

    /// Move the band `by` display rows, and say whether it moved or began or
    /// stopped following the foot.
    ///
    /// Negative is towards the head of the session. A band that was following
    /// the foot starts from where the foot put it, so the first turn of a wheel
    /// moves from what the reader is looking at rather than from a spot left
    /// over from the last time anybody scrolled. Following counts as a move
    /// because what the band holds can turn on it: a band the reader left at
    /// the foot of `rows` rows comes back to following one under it, which is
    /// the same top and a different picture.
    pub(crate) fn scroll(&mut self, by: i32, rows: usize) -> bool {
        let followed = self.following;
        let foot = self.foot(rows);
        let was = if self.following { foot } else { self.top };
        let far = usize::try_from(by.unsigned_abs()).unwrap_or(usize::MAX);
        let now = match by {
            0 => return false,
            up if up < 0 => self.back(was, far),
            _ => self.on(was, far, foot),
        };
        self.following = now == foot;
        self.top = now;
        now != was || self.following != followed
    }

    /// The absolute display row shown on the first row of a band `rows` tall.
    ///
    /// What a selection's ends are named in, so that scrolling the band moves
    /// the highlight with the words rather than leaving it over the rows.
    pub(crate) fn top_row(&self, rows: usize) -> usize {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };
        self.row_of(from)
    }

    /// The last absolute display row the record has, if it has any.
    pub(crate) fn last_row(&self) -> Option<usize> {
        self.ends
            .back()
            .and_then(|end| end.checked_sub(1))
            .filter(|last| *last >= self.before)
    }

    /// The display row at absolute position `row`, if the record still holds
    /// it.
    ///
    /// Folded on demand, for the rows a selection reached that the band is
    /// not showing: a drag that scrolled past them still took them.
    pub(crate) fn row_at(&self, row: usize) -> Option<Row> {
        if row < self.before || self.last_row().is_none_or(|last| row > last) {
            return None;
        }
        let spot = self.spot_at(row);
        let line = self.lines.get(spot.line.checked_sub(self.gone)?)?;
        self.fold(line).into_iter().nth(usize::from(spot.into))
    }

    /// Where a band `rows` tall stands in what the record retains, in display
    /// rows: what the scroll rail is laid out from.
    ///
    /// The total is kept, and the top is one lookup in the cumulative ends
    /// beside the line heights — or, while the band follows the foot, a walk
    /// back over the lines the band shows — so a rail drawn on every frame
    /// costs the band's height at most, never the record's or the session's.
    pub(crate) fn place(&self, rows: usize) -> Place {
        Place {
            total: self.rows,
            top: self.top_row(rows).saturating_sub(self.before),
            height: rows,
        }
    }

    /// Where each prompt still retained starts, in display rows into what is
    /// retained, oldest first.
    ///
    /// The walk is capped by [`MOST_LANDMARKS`], and a prompt whose line has
    /// spilled off the head leaves with it.
    pub(crate) fn prompts(&self) -> impl Iterator<Item = usize> + '_ {
        self.landmarked().map(|(_, row)| row)
    }

    /// Each prompt still retained, oldest first, as its stable line number
    /// beside where it starts, in display rows into what is retained.
    ///
    /// What holds a prompt across scrolling and spills, where its row moves
    /// and its line number does not. A resize that lays the opening out again
    /// renumbers every line under it, landmarks and [`Self::landed`] with
    /// them, so only a number read back from here since the last resize is
    /// one. Capped as [`Self::prompts`] is.
    fn landmarked(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.landmarks.iter().filter_map(|line| {
            let row = self.start_of(*line)?;
            Some((*line, row.saturating_sub(self.before)))
        })
    }

    /// Lands on the prompt that starts at display row `row` of what is
    /// retained, so the scroll rail holds it as current; a row no prompt
    /// starts on lands on nothing.
    pub(crate) fn lands(&mut self, row: usize) {
        let landed = self
            .landmarked()
            .find(|(_, start)| *start == row)
            .map(|(line, _)| line);
        self.landed = landed;
    }

    /// Where the prompt last landed on starts, in display rows into what is
    /// retained, while it is still retained.
    pub(crate) fn landed(&self) -> Option<usize> {
        let line = self.landed?;
        self.landmarked()
            .find(|(landmark, _)| *landmark == line)
            .map(|(_, row)| row)
    }

    /// Moves the band's top to display row `row` of what is retained, and
    /// says whether it moved or began or stopped following the foot, as
    /// [`Self::scroll`] does.
    ///
    /// Never past the foot: a row the band cannot start on and still be full
    /// is the foot, and the band follows the record again from there, as the
    /// wheel leaves it when it reaches the end. A binary search through the
    /// same cumulative ends [`Self::place`] reads, plus a walk bounded by the
    /// band's height to find the foot.
    pub(crate) fn seek(&mut self, row: usize, rows: usize) -> bool {
        let followed = self.following;
        let foot = self.foot(rows);
        let now = self.spot_at(self.before.saturating_add(row)).min(foot);
        let was = if self.following { foot } else { self.top };
        self.top = now;
        self.following = now == foot;
        now != was || self.following != followed
    }

    /// The display-row position of `spot` in this width epoch.
    fn row_of(&self, spot: Spot) -> usize {
        let at = spot.line.saturating_sub(self.gone);
        let before = at
            .checked_sub(1)
            .and_then(|line| self.ends.get(line).copied())
            .unwrap_or(self.before);
        before + usize::from(spot.into)
    }

    /// The spot at absolute display-row position `row`.
    fn spot_at(&self, row: usize) -> Spot {
        let row = row.clamp(
            self.before,
            self.ends.back().copied().unwrap_or(self.before),
        );
        let mut low = 0;
        let mut high = self.ends.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if self.ends.get(middle).is_some_and(|end| *end <= row) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let at = low.min(self.tall.len().saturating_sub(1));
        let before = at
            .checked_sub(1)
            .and_then(|line| self.ends.get(line).copied())
            .unwrap_or(self.before);
        let into = row.saturating_sub(before);
        Spot {
            line: self.gone + at,
            into: u16::try_from(into).unwrap_or(u16::MAX),
        }
    }

    /// The display-row position where stable `line` starts.
    fn start_of(&self, line: usize) -> Option<usize> {
        let at = line.checked_sub(self.gone)?;
        if at >= self.lines.len() {
            return None;
        }
        Some(
            at.checked_sub(1)
                .and_then(|line| self.ends.get(line).copied())
                .unwrap_or(self.before),
        )
    }

    /// Whether the band is showing the foot of the record.
    ///
    /// What the renderer asks before standing the row that tells of a held
    /// block at the foot: a reader who has scrolled back is not at the foot to
    /// see it. Asserted about as well, since the flag is what decides whether
    /// arriving text moves somebody who is reading back.
    pub(crate) fn following(&self) -> bool {
        self.following
    }

    /// Follow the foot of the record again.
    pub(crate) fn follow(&mut self) {
        self.following = true;
    }

    /// Lay the record out for a window of a different width, with the opening
    /// drawn again in `glyphs`.
    ///
    /// Every height is wrong at once, so every height is worked out again —
    /// and the spot keeps its line and loses its offset into it, because the
    /// row that was third of five in a line is not the third of two.
    pub(crate) fn resized(&mut self, columns: usize, glyphs: Glyphs) {
        if columns == self.columns {
            return;
        }
        self.columns = columns;
        self.relay_opening(columns, glyphs);
        self.relay_responsive(columns, glyphs, true);
        self.relay_drawn(columns, glyphs);
        self.rows = 0;
        self.before = 0;
        self.tall.clear();
        self.ends.clear();
        for line in &self.lines {
            let tall = Self::measure(line, columns);
            self.tall.push_back(tall);
            self.rows += usize::from(tall);
            self.ends.push_back(self.rows);
        }
        self.spill();
        self.top.into = 0;
    }

    /// The spot that puts the last display row at the foot of a band `rows`
    /// tall — walking back from the end, so the cost is the band's rather than
    /// the record's.
    ///
    /// Nothing written out to a native terminal's scrollback is counted: the
    /// band there is the live region, and the terminal shows those lines.
    fn foot(&self, rows: usize) -> Spot {
        let first = self.first();
        let mut left = rows;
        for (back, &tall) in self.tall.iter().enumerate().rev() {
            if self.gone + back < first {
                break;
            }
            let tall = usize::from(tall);
            if tall >= left {
                return Spot {
                    line: self.gone + back,
                    into: u16::try_from(tall - left).unwrap_or(u16::MAX),
                };
            }
            left -= tall;
        }
        Spot {
            line: first,
            into: 0,
        }
    }

    /// `spot` moved `rows` display rows towards the head of the record.
    fn back(&self, spot: Spot, rows: usize) -> Spot {
        let mut left = rows;
        let mut at = spot.line.saturating_sub(self.gone);
        let mut into = usize::from(spot.into);
        loop {
            if into >= left {
                return Spot {
                    line: self.gone + at,
                    into: u16::try_from(into - left).unwrap_or(u16::MAX),
                };
            }
            left -= into;
            let Some(next) = at.checked_sub(1) else {
                return Spot {
                    line: self.gone,
                    into: 0,
                };
            };
            at = next;
            into = usize::from(self.tall.get(at).copied().unwrap_or(1));
        }
    }

    /// `spot` moved `rows` display rows towards the foot, never past `foot`.
    fn on(&self, spot: Spot, rows: usize, foot: Spot) -> Spot {
        let mut left = rows;
        let mut at = spot.line.saturating_sub(self.gone);
        let mut into = usize::from(spot.into);
        loop {
            let tall = usize::from(self.tall.get(at).copied().unwrap_or(1));
            let rest = tall.saturating_sub(into);
            if rest > left {
                let now = Spot {
                    line: self.gone + at,
                    into: u16::try_from(into + left).unwrap_or(u16::MAX),
                };
                return now.min(foot);
            }
            left -= rest;
            into = 0;
            at += 1;
            if at >= self.lines.len() {
                return foot;
            }
        }
    }

    /// Rebuilds responsive blocks for `columns` in `glyphs`: every one of them
    /// for a new width, and for a new glyph set only those drawn in another,
    /// answering whether any was.
    fn relay_responsive(&mut self, columns: usize, glyphs: Glyphs, every: bool) -> bool {
        let mut any = false;
        for line in &mut self.lines {
            if let Line::Responsive {
                rows,
                prefix,
                lay,
                glyphs: laid,
                ..
            } = line
            {
                if !every && laid.is_none_or(|laid| laid == glyphs) {
                    continue;
                }
                *rows = responsive_rows(lay(columns, glyphs));
                if let Some(prefix) = prefix {
                    prefix.prefix(rows);
                }
                if laid.is_some() {
                    *laid = Some(glyphs);
                }
                any = true;
            }
        }
        any
    }

    /// A line as the display rows it comes to at the current width.
    fn fold(&self, line: &Line) -> Vec<Row> {
        match line {
            Line::Flowed(row) => flowed(row, self.columns),
            Line::Responsive { rows, .. } | Line::Drawn(Drawn { rows, .. }) => rows.clone(),
            Line::Set(row) => vec![row.clipped(self.columns)],
        }
    }
}

/// The display rows a flowed line comes to at `columns`.
///
/// A line that opens with a quote's bar, an item's mark or a `<details>`
/// block's body lead continues under its words rather than under its mark,
/// which is [`markdown::hang`]'s to say: the bar is stood again and the
/// columns of a mark or a lead are left blank. Any other line is
/// folded flush, as prose is. A hang as wide as the window leaves the words no
/// room, so it is not kept and the line folds flush instead. A row of code in
/// its frame breaks wherever the room runs out, which is
/// [`markdown::framed`]'s to say.
fn flowed(row: &Row, columns: usize) -> Vec<Row> {
    if let Some(rows) = markdown::framed(row, columns) {
        return rows;
    }
    let Some((hang, room)) = hung(row, columns) else {
        return row.fold(columns);
    };

    let lead = row.between(0, hang.lead);
    words(row, &hang)
        .fold(room)
        .into_iter()
        .enumerate()
        .map(|(at, part)| {
            let front = if at == 0 { &lead } else { &hang.prefix };
            front.clone().join(part)
        })
        .collect()
}

/// How many display rows [`flowed`] gives `row`, without building them.
fn flowed_height(row: &Row, columns: usize) -> usize {
    if let Some(height) = markdown::framed_height(row, columns) {
        return height;
    }
    let Some((hang, room)) = hung(row, columns) else {
        return row.folds(columns);
    };
    words(row, &hang).folds(room)
}

/// What follows `hang`'s lead in `row`: the words that fold beside the hang,
/// folded by [`Row::fold`] and [`Row::folds`] like any other row.
fn words(row: &Row, hang: &markdown::Hang) -> Row {
    row.between(hang.lead, usize::MAX)
}

/// The hang `row` folds under at `columns` and the columns left beside it, for
/// a row that has to fold and has a hang the window has room for.
fn hung(row: &Row, columns: usize) -> Option<(markdown::Hang, usize)> {
    if columns == 0 || row.columns() <= columns {
        return None;
    }
    let hang = markdown::hang(row)?;
    (hang.columns < columns).then(|| {
        let room = columns - hang.columns;
        (hang, room)
    })
}

/// Caps one responsive layout to the record's existing display-row ceiling.
fn responsive_rows(mut rows: Vec<Row>) -> Vec<Row> {
    rows.truncate(MOST);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record holding `lines` numbered lines, each one flowed.
    fn filled(columns: usize, lines: usize) -> Record {
        let mut record = Record::new(columns);
        for line in 0..lines {
            record.write(Slot::Plain, &format!("{line}\n"), None);
        }
        record
    }

    /// What the view says, as plain text.
    fn said(record: &Record, rows: usize) -> Vec<String> {
        record.view(rows).iter().map(Row::text).collect()
    }

    /// A source that lays one row per column of the window, so what width it
    /// was called at can be read straight off what it laid.
    fn ruler() -> Box<dyn Fn(usize, Glyphs) -> Vec<Row>> {
        Box::new(|columns, _| {
            (0..columns)
                .map(|row| Row::new().then(Slot::Plain, format!("{row}")))
                .collect()
        })
    }

    #[test]
    fn the_rows_covering_a_line_are_exactly_the_rows_showing_it() {
        // The pair this makes with [`Record::at`]: one says which line a row is
        // showing and the other every row showing that line, so a pointer on
        // any row of a wrapped line is pointing at the whole of it.
        let mut record = Record::new(8);
        record.write(Slot::Plain, "one\n", None);
        record.write(Slot::Plain, "a line that takes several rows\n", None);
        record.write(Slot::Plain, "last\n", None);

        let rows = 12;
        for row in 0..rows {
            let Some(line) = record.at(row, rows) else {
                continue;
            };
            assert!(
                record.covering(line, rows).contains(&row),
                "row {row} shows line {line} and is not among the rows covering it"
            );
        }

        // And something here really did fold, or the walk above would hold just
        // as well for a record that never wrapped a line in its life.
        let wrapped = record.at(1, rows).expect("a second row");
        assert!(record.covering(wrapped, rows).len() > 1);
    }

    #[test]
    fn a_line_the_band_is_not_showing_is_covered_by_no_row_of_it() {
        let record = filled(8, 40);

        assert!(record.covering(0, 5).is_empty());
    }

    #[test]
    fn a_responsive_block_changes_height_without_renumbering_later_lines() {
        let mut record = Record::new(8);
        record.responsive(
            32,
            Box::new(|columns| {
                let rows = if columns < 6 { 3 } else { 1 };
                (0..rows)
                    .map(|row| Row::plain(format!("responsive {row}")))
                    .collect()
            }),
        );
        record.landmark();
        record.write(Slot::Plain, "after\n", None);
        let after = record.landmarks.front().copied().expect("a landmark");
        assert_eq!(after, 1, "one responsive block is one logical line");

        record.resized(5, Glyphs::Unicode);

        assert_eq!(record.landmarks.front().copied(), Some(after));
        assert_eq!(record.start_of(after), Some(3));
        assert_eq!(record.lines(), 2);
    }

    #[test]
    fn responsive_source_pays_into_the_same_bounded_record() {
        let mut record = Record::new(40);
        record.responsive(
            RETAINED_ROW_BYTES * (MOST - 1),
            Box::new(|_| vec![Row::plain("large retained source")]),
        );
        record.write(Slot::Plain, "newest\n", None);

        assert_eq!(record.weight, MOST);
        assert_eq!(record.lines.len(), 2);

        record.write(Slot::Plain, "one more\n", None);

        assert_eq!(record.weight, 2);
        assert_eq!(record.lines.len(), 2);
        assert_eq!(said(&record, 2), ["newest", "one more"]);
    }

    #[test]
    fn a_block_charged_as_laid_pays_for_the_text_of_the_rows_it_keeps() {
        // One row, so a charge by rows alone would be one. The text laid at
        // this width is 241 bytes: three charged units and part of a fourth.
        let mut record = Record::new(40);
        record.responsive_as_laid(Box::new(|columns| {
            vec![Row::plain("x".repeat(columns * 6 + 1))]
        }));

        assert_eq!(record.weight, 4);
    }

    #[test]
    fn a_record_that_was_emptied_has_nothing_left_of_what_it_held() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        record.write(Slot::Plain, "said\n", None);

        record.empties();

        assert!(said(&record, 8).is_empty());

        // The card goes with the lines and does not come back: what a resize
        // lays out again is an opening whose lines are still held, and these
        // are not.
        record.resized(5, Glyphs::Unicode);
        assert!(said(&record, 8).is_empty());
    }

    #[test]
    fn a_subordinate_block_gets_one_mark_and_aligned_continuations() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.write(Slot::Plain, "first\nsecond\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ first", "  second"]);
        let one = || std::iter::once(0..1).collect::<Vec<_>>();
        assert_eq!(
            record
                .view(8)
                .iter()
                .map(Row::structural)
                .collect::<Vec<_>>(),
            [one(), one()]
        );
    }

    #[test]
    fn a_result_already_starting_with_structural_art_gets_no_second_mark() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.lay([Row::new()
            .then_structural(Slot::Trouble, "⎿")
            .then(Slot::Trouble, " failed")]);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ failed"]);
    }

    #[test]
    fn a_premarked_responsive_result_aligns_later_rows_after_resize() {
        let mut record = Record::new(8);
        let from = record.lines();
        record.responsive(
            8,
            Box::new(|columns| {
                vec![
                    Row::new()
                        .then_structural(Slot::Trouble, "⎿")
                        .then(Slot::Trouble, format!(" failed at {columns}")),
                    Row::new().then(Slot::Trouble, "details"),
                ]
            }),
        );

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ failed at 8", "  details"]);
        record.resized(12, Glyphs::Unicode);
        assert_eq!(said(&record, 8), ["⎿ failed at 12", "  details"]);
    }

    #[test]
    fn literal_art_at_the_start_of_a_result_still_gets_the_structural_mark() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.write(Slot::Plain, "⎿ literal\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ ⎿ literal"]);
    }

    #[test]
    fn a_subordinate_block_that_starts_after_old_rows_leaves_them_alone() {
        let mut record = Record::new(80);
        record.write(Slot::Plain, "before\n", None);
        let from = record.lines();
        record.write(Slot::Plain, "answer\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["before", "⎿ answer"]);
    }

    #[test]
    fn a_subordinate_block_keeps_one_mark_after_a_responsive_resize() {
        let mut record = Record::new(8);
        let from = record.lines();
        record.responsive(
            8,
            Box::new(|columns| vec![Row::plain(format!("at {columns}"))]),
        );
        record.write(Slot::Plain, "after\n", None);
        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ at 8", "  after"]);
        record.resized(12, Glyphs::Unicode);
        assert_eq!(said(&record, 8), ["⎿ at 12", "  after"]);
    }

    #[test]
    fn an_empty_command_answer_adds_no_mark() {
        let mut record = Record::new(80);
        let from = record.lines();

        record.subordinate(from, "⎿");

        assert!(said(&record, 8).is_empty());
    }

    #[test]
    fn a_command_that_emptied_the_record_does_not_mark_the_replacement() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.empties();
        record.write(Slot::Plain, "new opening\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["new opening"]);
    }

    #[test]
    fn the_numbering_carries_on_past_the_lines_a_record_dropped() {
        let mut record = filled(8, 6);
        let numbered = record.lines();

        record.empties();
        assert_eq!(record.lines(), numbered + 1);

        // So a number some other part of the program is holding names the line
        // it named, and names nothing once that line has gone — rather than
        // quietly naming whatever was written in its place. The extra number is
        // the reset boundary itself.
        record.write(Slot::Plain, "after\n", None);
        assert_eq!(record.lines(), numbered + 2);
        assert_eq!(said(&record, 8), ["after"]);
    }

    #[test]
    fn the_opening_is_laid_out_again_when_the_window_changes() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        record.write(Slot::Plain, "after\n", None);

        record.resized(5, Glyphs::Unicode);

        let laid: Vec<String> = record.lines.iter().map(measured).collect();
        assert_eq!(laid, ["0", "1", "2", "3", "4", "after"]);
    }

    #[test]
    fn a_card_that_changes_height_leaves_the_reader_on_the_line_they_were_on() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        for line in 0..6 {
            record.write(Slot::Plain, &format!("said {line}\n"), None);
        }

        // Above the foot, so the reader's place is a spot rather than a
        // promise to follow — and below the card, which is the half of this a
        // shorter card moves.
        record.scroll(-2, 4);
        let reading = said(&record, 4);
        assert_eq!(reading.first().map(String::as_str), Some("said 0"));

        // Two lines shorter than it was, so a place counted from the top of the
        // record means two lines further down than it did. Wide enough that
        // what is under the card still folds to one row apiece, because that
        // is the other half of a resize and is not what this is about.
        record.resized(6, Glyphs::Unicode);

        assert_eq!(said(&record, 4), reading);
    }

    #[test]
    fn a_card_that_changes_height_keeps_prompt_landmarks_on_their_prompts() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        record.landmark();
        record.write(Slot::Plain, "the prompt\n", None);
        let before = record.landmarks.front().copied().expect("a landmark");

        record.resized(5, Glyphs::Unicode);

        let after = record.landmarks.front().copied().expect("the landmark");
        assert_eq!(after, before - 3);
        assert_eq!(record.start_of(after), Some(5));
    }

    #[test]
    fn a_reader_inside_a_card_that_was_laid_out_again_is_left_at_the_top_of_it() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        for line in 0..6 {
            record.write(Slot::Plain, &format!("said {line}\n"), None);
        }

        // Four rows into the card, which is a row the shorter card does not
        // have: eight rows became five. There is no line to be left on, so the
        // reader is left on the thing they were reading rather than on a
        // number that used to be inside it.
        record.scroll(-6, 4);
        assert_eq!(said(&record, 1), ["4"]);

        record.resized(5, Glyphs::Unicode);

        assert_eq!(said(&record, 1), ["0"]);
    }

    /// The text of one line, whichever kind it is.
    fn measured(line: &Line) -> String {
        match line {
            Line::Flowed(row) | Line::Set(row) => row.text(),
            Line::Responsive { rows, .. } | Line::Drawn(Drawn { rows, .. }) => {
                rows.iter().map(Row::text).collect()
            }
        }
    }

    /// What the record is tall, worked out the slow way.
    fn counted(record: &Record) -> usize {
        record
            .lines
            .iter()
            .map(|line| record.fold(line).len())
            .sum()
    }

    #[test]
    fn what_the_record_says_it_is_tall_is_what_its_lines_come_to() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "a word that will not fit in ten\n", None);
        record.write(Slot::Plain, "short\n", None);
        record.lay([set("a set row that is far wider than ten columns")]);
        record.write(Slot::Accent, "and more", None);

        assert_eq!(record.rows, counted(&record));
        let tall: usize = record.tall.iter().map(|&t| usize::from(t)).sum();
        assert_eq!(record.rows, tall);
    }

    #[test]
    fn a_line_that_grows_past_the_width_grows_the_record_with_it() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "one", None);
        assert_eq!(record.rows, 1);

        // The same line, now too long for one row: the height kept beside it
        // has to be worked out again, or the record is a row short for the
        // rest of the session.
        record.write(Slot::Plain, " two three four five", None);

        assert!(record.rows > 1);
        assert_eq!(record.rows, counted(&record));
    }

    #[test]
    fn a_delta_that_stops_mid_word_lands_in_the_line_the_word_is_in() {
        let mut record = Record::new(40);
        record.write(Slot::Plain, "hel", None);
        record.write(Slot::Plain, "lo wor", None);
        record.write(Slot::Plain, "ld", None);

        assert_eq!(said(&record, 4), ["hello world"]);
    }

    #[test]
    fn a_newline_ends_the_line_it_is_in_and_is_never_drawn() {
        let mut record = Record::new(40);
        record.write(Slot::Plain, "one\ntwo\n", None);

        // Two, not three. The trailing newline ends the line it is in and
        // opens nothing: on a screen this process owns, the row a cursor sits
        // on belongs to the box, so a blank row here would be one the reader
        // is given for nothing.
        assert_eq!(said(&record, 8), ["one", "two"]);
    }

    #[test]
    fn a_band_shows_the_foot_while_nobody_has_scrolled() {
        let record = filled(40, 10);

        assert!(record.following());
        assert_eq!(said(&record, 3), ["7", "8", "9"]);
    }

    #[test]
    fn a_band_taller_than_the_record_is_given_what_there_is() {
        let record = filled(40, 2);

        assert_eq!(said(&record, 40).len(), 2);
    }

    #[test]
    fn scrolling_up_stops_at_the_head_and_says_when_it_did_not_move() {
        let mut record = filled(40, 10);

        assert!(record.scroll(-100, 3));
        assert!(!record.scroll(-1, 3));
        assert_eq!(said(&record, 3), ["0", "1", "2"]);
    }

    #[test]
    fn scrolling_back_to_the_foot_follows_again() {
        let mut record = filled(40, 10);

        assert!(record.scroll(-4, 3));
        assert!(!record.following());
        assert!(record.scroll(100, 3));
        assert!(record.following());
        assert_eq!(said(&record, 3), ["7", "8", "9"]);
    }

    /// A record ten columns wide whose first line folds to several rows.
    fn folded() -> Record {
        let mut record = Record::new(10);
        record.write(
            Slot::Plain,
            "one two three four five six seven eight nine ten\n",
            None,
        );
        record.landmark();
        record.write(Slot::Plain, "short\n", None);
        record.write(Slot::Plain, "another short\n", None);
        record
    }

    #[test]
    fn the_rail_place_is_measured_in_folded_display_rows() {
        // A line-count measure would say three lines and a prompt on the
        // second; what the rail stands for is the rows those lines fold to.
        let record = folded();
        let rows = record.rows;
        assert!(rows > 3, "{rows}");

        assert_eq!(
            record.place(2),
            Place {
                total: rows,
                top: rows - 2,
                height: 2,
            }
        );
        assert_eq!(record.prompts().collect::<Vec<_>>(), [rows - 3]);
    }

    #[test]
    fn a_rail_seek_lands_on_the_display_row_asked_for_and_follows_at_the_foot() {
        let mut record = folded();
        let rows = record.rows;

        // Into the long first line, which a line-count seek could not reach.
        assert!(record.seek(1, 1));
        assert!(!record.following());
        assert_eq!(record.place(1).top, 1);
        assert_eq!(said(&record, 1), ["three four"]);

        // Past the foot is the foot, and the band follows it again.
        assert!(record.seek(rows + 5, 1));
        assert!(record.following());
        assert_eq!(record.place(1).top, rows - 1);
        assert!(!record.seek(rows - 1, 1));
    }

    #[test]
    fn rail_prompts_leave_with_the_lines_they_started() {
        let mut record = Record::new(40);
        for line in 0..MOST + 10 {
            if line % 5 == 0 {
                record.landmark();
            }
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }
        let prompts: Vec<usize> = record.prompts().collect();

        assert!(!prompts.is_empty());
        assert!(prompts.iter().all(|row| *row < record.rows));
        assert_eq!(prompts.last().copied(), Some(record.rows - 5));
    }

    #[test]
    fn prompt_landmarks_are_bounded_and_old_ones_leave_with_the_record() {
        let mut record = Record::new(40);
        for line in 0..MOST + MOST_LANDMARKS * 2 {
            record.landmark();
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }

        assert_eq!(record.landmarks.len(), MOST_LANDMARKS);
        assert!(record.landmarks.iter().all(|line| *line >= record.gone));
    }

    #[test]
    fn cumulative_row_ends_stay_aligned_as_lines_grow_and_spill() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "one", None);
        record.write(Slot::Plain, " two three four five", None);
        for line in 0..MOST + 20 {
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }

        assert_eq!(record.ends.len(), record.lines.len());
        assert_eq!(
            record.ends.back().copied().unwrap_or(record.before) - record.before,
            record.rows
        );
    }

    #[test]
    fn a_set_row_is_clipped_and_never_folded() {
        let mut record = Record::new(6);
        record.lay([set("far wider than six")]);

        assert_eq!(record.rows, 1);
        assert_eq!(said(&record, 4), ["far wi"]);
    }

    #[test]
    fn a_resize_keeps_the_line_the_reader_was_on_and_drops_the_row() {
        let mut record = Record::new(10);
        for line in 0..3 {
            record.write(
                Slot::Plain,
                &format!("line {line} with several words in it\n"),
                None,
            );
        }
        record.scroll(-6, 2);
        let was = record.top.line;

        // Partway into a line, which is the case a resize invalidates: the row
        // that was fourth of five in a line is not the fourth of two.
        assert!(record.top.into > 0);

        record.resized(18, Glyphs::Unicode);

        assert_eq!(record.top.line, was);
        assert_eq!(record.top.into, 0);
        // Wide enough to change every height and narrow enough that they are
        // still not one, which is what makes the new heights observable.
        assert!(record.rows > record.lines.len());
        assert_eq!(record.rows, counted(&record));
    }

    #[test]
    fn the_oldest_lines_are_dropped_and_counted() {
        let record = filled(40, MOST + 100);

        assert_eq!(record.lines.len(), MOST);
        assert_eq!(record.gone, record.lines.len() + record.gone - MOST);
        assert!(record.gone >= 100);
        assert_eq!(record.rows, counted(&record));
        assert_eq!(
            said(&record, 2),
            [format!("{}", MOST + 98), format!("{}", MOST + 99)]
        );
    }

    #[test]
    fn a_reader_looking_at_a_line_that_spills_is_left_at_the_head() {
        let mut record = filled(40, 10);
        record.scroll(-100, 3);
        assert_eq!(record.top.line, 0);

        for line in 0..MOST + 100 {
            record.write(Slot::Plain, &format!("more {line}\n"), None);
        }

        // The line they were on is gone. The head of what is left is the
        // closest thing to where they were looking, and it is not a panic.
        assert!(record.top.line < record.gone);
        assert_eq!(said(&record, 1), [format!("more {}", record.gone - 10)]);
    }

    #[test]
    fn a_pointer_resting_on_a_band_whose_line_has_spilled_still_answers() {
        // The defect this catches: `at` subtracted `gone` from the top line
        // unchecked, while every sibling walking the same state saturates. The
        // state where the top line has spilled is the one the test above pins
        // as intended, and a pointer resting on the band reaches `at` every
        // frame — a panic in debug, an empty answer in release.
        let mut record = filled(40, 10);
        record.scroll(-100, 3);

        for line in 0..MOST + 100 {
            record.write(Slot::Plain, &format!("more {line}\n"), None);
        }
        assert!(record.top.line < record.gone);

        assert_eq!(record.at(0, 3), Some(record.gone));
    }

    /// What a test hands the record to make its pictures, and what it was
    /// asked, in order.
    mod picturing {
        use std::sync::{Arc, Mutex};

        use super::super::*;
        use crate::color::Palette;
        use crate::drawing::{Draws, Laid, Svg};
        use crate::terminal::graphics::Cell;

        /// What each call to the fake was, in order.
        pub(super) type Asked = Arc<Mutex<Vec<String>>>;

        /// A drawer for `mermaid` fences that lays each as one row of text
        /// and draws it as a picture, noting each picture it draws.
        pub(super) struct Diagrams(pub(super) Asked);

        impl Draws for Diagrams {
            fn names(&self) -> &'static [&'static str] {
                &["mermaid"]
            }

            fn block(&self, source: &str, _: &Room) -> Result<Laid, Undrawn> {
                let rows: Vec<Row> = source.lines().map(Row::plain).collect();
                Ok(Laid {
                    natural: (8, rows.len()),
                    rows,
                    narrow: None,
                })
            }

            fn picture(&self, source: &str, _: &Room) -> Result<Svg, Undrawn> {
                if let Ok(mut asked) = self.0.lock() {
                    asked.push(format!("picture {}", source.trim()));
                }
                Svg::new(format!("<svg>{}</svg>", source.trim()))
            }
        }

        /// Pictures `rows` cells down and two across, each payload `bytes`
        /// long, with what the cache holds by source, and why it refuses to
        /// make one, where it does.
        pub(super) struct Fake {
            pub(super) asked: Asked,
            pub(super) bytes: usize,
            pub(super) rows: u16,
            pub(super) cached: Vec<&'static str>,
            pub(super) refuses: Option<Undrawn>,
        }

        impl Fake {
            fn payload(&self, key: &PictureKey) -> Payload {
                let height = u32::from(self.rows) * u32::from(key.cell.height);
                let width = 2 * u32::from(key.cell.width);
                Payload::new(key.protocol, vec![7; self.bytes], (width, height), key.cell)
            }

            fn note(&self, said: String) {
                if let Ok(mut asked) = self.asked.lock() {
                    asked.push(said);
                }
            }
        }

        impl Pictures for Fake {
            fn cached(&mut self, source: &str, key: &PictureKey) -> Option<(Form, Payload)> {
                self.note(format!("cached {}", source.trim()));
                self.cached
                    .contains(&source.trim())
                    .then(|| (Form::Png(vec![7; self.bytes]), self.payload(key)))
            }

            fn make(
                &mut self,
                source: &str,
                key: &PictureKey,
                _: &Form,
            ) -> Result<Payload, Undrawn> {
                self.note(format!("make {}", source.trim()));
                self.refuses.map_or_else(|| Ok(self.payload(key)), Err)
            }

            fn reencode(&mut self, _: &KeptPicture, key: &PictureKey) -> Result<Payload, Undrawn> {
                self.note("reencode".to_owned());
                Ok(self.payload(key))
            }
        }

        /// The cell every test's terminal draws in.
        pub(super) const CELL: Cell = Cell {
            width: 8,
            height: 16,
        };

        /// A record `columns` wide drawing diagrams with [`Diagrams`] and
        /// pictures with `fake`, on a Kitty terminal, in `palette`.
        pub(super) fn record(columns: usize, fake: Fake, palette: &Palette) -> Record {
            let mut record = Record::new(columns);
            record.draws_with(
                Drawers::default().with(Arc::new(Diagrams(Arc::clone(&fake.asked)))),
                Offers::Stand,
            );
            record.pictures(Box::new(fake));
            record.pictured_as(
                Capability::Graphics {
                    protocol: Protocol::Kitty,
                    cell: CELL,
                },
                palette.identity(),
            );
            record
        }

        /// A closed diagram whose one line is `name`.
        pub(super) fn diagram(name: &str) -> Block {
            Block {
                kind: Kind::Mermaid,
                source: format!("```mermaid\n{name}\n```\n"),
                closed: true,
            }
        }

        /// A palette that writes colour, in `theme`.
        pub(super) fn coloured(theme: Theme) -> Palette {
            let truecolor = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
            Palette::resolve(true, theme, None, &truecolor)
        }

        /// How many blocks the record shows as pictures, and their bytes.
        pub(super) fn kept(record: &Record) -> (usize, usize) {
            record.kept_pictures()
        }

        /// Whether line `line` is shown as a picture.
        pub(super) fn shown(record: &Record, line: usize) -> bool {
            matches!(
                record.lines.get(line),
                Some(Line::Drawn(Drawn {
                    picture: Some(_),
                    ..
                }))
            )
        }

        /// Why line `line` is drawn as text, where it said.
        pub(super) fn refused(record: &Record, line: usize) -> Option<Undrawn> {
            match record.lines.get(line) {
                Some(Line::Drawn(drawn)) => drawn.refused.as_ref().map(|refused| refused.1),
                _ => None,
            }
        }

        /// What the fake was asked, in order.
        pub(super) fn asked(asked: &Asked) -> Vec<String> {
            asked.lock().map(|asked| asked.clone()).unwrap_or_default()
        }
    }

    /// A fake making pictures `rows` down of `bytes` each, its log, and what
    /// its cache holds.
    fn fake(
        bytes: usize,
        rows: u16,
        cached: Vec<&'static str>,
    ) -> (picturing::Fake, picturing::Asked) {
        let asked = picturing::Asked::default();
        let fake = picturing::Fake {
            asked: std::sync::Arc::clone(&asked),
            bytes,
            rows,
            cached,
            refuses: None,
        };
        (fake, asked)
    }

    #[test]
    fn a_diagram_is_shown_as_its_picture_where_the_terminal_confirmed_kitty() {
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);

        assert!(picturing::shown(&record, 0));
        let folded = record.folded(0);
        assert_eq!(folded.len(), 3, "one row a cell down");
        assert!(folded.iter().all(|row| row.text() == "  "), "{folded:?}");
    }

    #[test]
    fn the_cache_is_asked_before_the_drawer_and_a_hit_draws_nothing() {
        let (fake, asked) = fake(100, 3, vec!["kept"]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

        record.drawn(picturing::diagram("new"), Glyphs::Unicode, Theme::Dark);
        record.drawn(picturing::diagram("kept"), Glyphs::Unicode, Theme::Dark);

        assert_eq!(
            picturing::asked(&asked),
            ["cached new", "picture new", "make new", "cached kept"]
        );
        assert!(picturing::shown(&record, 1));
    }

    #[test]
    fn a_diagram_drawn_as_text_is_never_shown_as_a_picture() {
        for mode in [Mode::Text, Mode::Written] {
            let (fake, asked) = fake(100, 3, vec![]);
            let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
            record.draws_in(Modes {
                math: Mode::Image,
                diagrams: mode,
            });

            record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);

            assert!(!picturing::shown(&record, 0), "{mode:?}");
            assert!(picturing::asked(&asked).is_empty(), "{mode:?}");
        }
    }

    #[test]
    fn no_picture_is_made_where_no_colour_is_written() {
        let (fake, asked) = fake(100, 3, vec![]);
        let plain = crate::color::Palette::plain();
        let mut record = picturing::record(40, fake, &plain);

        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);

        assert!(!picturing::shown(&record, 0));
        assert!(picturing::asked(&asked).is_empty());
    }

    #[test]
    fn past_the_count_the_oldest_picture_is_drawn_as_text() {
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

        for line in 0..=MOST_KEPT_IMAGES {
            record.drawn(
                picturing::diagram(&format!("d{line}")),
                Glyphs::Unicode,
                Theme::Dark,
            );
        }

        assert_eq!(picturing::kept(&record).0, MOST_KEPT_IMAGES);
        assert!(!picturing::shown(&record, 0), "the oldest fell back");
        assert_eq!(record.folded(0).len(), 1, "laid out as its text again");
        assert!((1..=MOST_KEPT_IMAGES).all(|line| picturing::shown(&record, line)));
    }

    #[test]
    fn past_the_bytes_the_oldest_falls_back_and_a_png_is_counted_once() {
        // Three pictures of a third of the bytes each fill the cap, once a
        // cached picture whose form is its own payload is counted once.
        let third = MOST_KEPT_IMAGE_BYTES / 3;
        let (fake, _) = fake(third, 3, vec!["a", "b", "c", "d"]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

        for name in ["a", "b", "c"] {
            record.drawn(picturing::diagram(name), Glyphs::Unicode, Theme::Dark);
        }
        assert_eq!(picturing::kept(&record), (3, 3 * third));

        record.drawn(picturing::diagram("d"), Glyphs::Unicode, Theme::Dark);

        assert_eq!(picturing::kept(&record), (3, 3 * third));
        assert!(!picturing::shown(&record, 0));
        assert!((1..4).all(|line| picturing::shown(&record, line)));
    }

    #[test]
    fn a_picture_larger_than_every_kept_picture_together_is_drawn_as_text() {
        let (fake, _) = fake(MOST_KEPT_IMAGE_BYTES, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

        // Drawn from its drawer's picture, so what is kept is that picture and
        // a payload as large as the cap: past it together.
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);

        assert!(!picturing::shown(&record, 0));
        assert_eq!(
            picturing::refused(&record, 0),
            Some(Undrawn::TooLargeToKeep)
        );
        assert_eq!(record.folded(0).len(), 1, "laid out as its text");
    }

    #[test]
    fn a_picture_more_than_its_marks_can_place_is_drawn_as_text() {
        // Each placeholder cell names its row by one of MOST_PLACED marks: a
        // picture that many cells down is shown, and one a cell more is not.
        let (drawer, _) = fake(100, MOST_PLACED, vec![]);
        let mut record = picturing::record(40, drawer, &picturing::coloured(Theme::Dark));
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
        assert!(picturing::shown(&record, 0));

        let (drawer, _) = fake(100, MOST_PLACED + 1, vec![]);
        let mut record = picturing::record(40, drawer, &picturing::coloured(Theme::Dark));
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
        assert!(!picturing::shown(&record, 0));
        assert_eq!(picturing::refused(&record, 0), Some(Undrawn::TooWide));
        assert_eq!(record.folded(0).len(), 1, "laid out as its text");

        // Two cells across, in a record one column wide.
        let (drawer, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(1, drawer, &picturing::coloured(Theme::Dark));
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
        assert!(!picturing::shown(&record, 0));
        assert_eq!(picturing::refused(&record, 0), Some(Undrawn::TooWide));
    }

    #[test]
    fn once_every_number_is_given_out_a_diagram_is_drawn_as_text() {
        // A record gives out as many numbers as a 24-bit colour holds: the
        // last one is given out, and the picture after it is never named by
        // one already sent.
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.numbers_left(1);

        record.drawn(picturing::diagram("last"), Glyphs::Unicode, Theme::Dark);
        record.drawn(picturing::diagram("past"), Glyphs::Unicode, Theme::Dark);

        assert!(picturing::shown(&record, 0));
        assert!(!picturing::shown(&record, 1));
        assert_eq!(picturing::refused(&record, 1), Some(Undrawn::PastCap));
    }

    #[test]
    fn numbers_go_on_from_the_largest_to_one_and_never_to_zero() {
        // A record's numbers start anywhere: from the largest a 24-bit
        // colour holds, the next picture is 1, since no cell names a
        // picture 0.
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.numbers_from(MOST_PICTURE_ID);

        record.drawn(picturing::diagram("last"), Glyphs::Unicode, Theme::Dark);
        record.drawn(picturing::diagram("first"), Glyphs::Unicode, Theme::Dark);

        assert_eq!(
            record.picture_ids().collect::<Vec<_>>(),
            [MOST_PICTURE_ID, 1]
        );
    }

    #[test]
    fn every_way_a_kept_picture_is_let_go_of_is_told() {
        type Way = fn(&mut Record, &crate::color::Palette);

        // A full screen deletes what the terminal holds of a picture once
        // the record lets it go, which it hears of only here.
        let pictured = || {
            let (fake, _) = fake(100, 3, vec![]);
            let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
            record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
            assert!(picturing::shown(&record, 0));
            assert!(!record.let_pictures_go(), "one drawn is none let go");
            record
        };
        let plain = crate::color::Palette::plain();
        let ways: [(&str, Way); 5] = [
            ("encoded again at another width", |record, _| {
                record.relay(100, Glyphs::Unicode, Theme::Dark);
            }),
            (
                "drawn as text where colour is not written",
                |record, plain| {
                    record.pictured_as(
                        Capability::Graphics {
                            protocol: Protocol::Kitty,
                            cell: picturing::CELL,
                        },
                        plain.identity(),
                    );
                    record.relay(40, Glyphs::Unicode, Theme::Dark);
                },
            ),
            ("drawn as text past the count", |record, _| {
                for line in 1..=MOST_KEPT_IMAGES {
                    record.drawn(
                        picturing::diagram(&format!("d{line}")),
                        Glyphs::Unicode,
                        Theme::Dark,
                    );
                }
            }),
            ("spilled", |record, _| {
                record.lay((0..MOST).map(|_| set("after")));
            }),
            ("emptied", |record, _| record.empties()),
        ];
        for (way, lets_go) in ways {
            let mut record = pictured();
            let first = record.picture_ids().next();
            lets_go(&mut record, &plain);
            assert_ne!(record.picture_ids().next(), first, "{way}: still kept");
            assert!(record.let_pictures_go(), "{way}: not told");
            assert!(!record.let_pictures_go(), "{way}: told twice");
        }
    }

    #[test]
    fn a_picture_shown_in_other_cells_is_told_once_and_none_laid_since_is_never_told() {
        // A yes has the frame look every picture the terminal holds up
        // again, a walk over every line, so no is the answer wherever no
        // picture has been laid out in other cells since it last asked.
        let (fake, _) = fake(100, 20, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.windowed(8, false);
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
        assert!(picturing::shown(&record, 0));
        record.pictures_reshaped();
        assert!(!record.pictures_reshaped(), "told with none laid since");

        assert!(record.cycles(0), "no picture to show larger");
        assert!(record.pictures_reshaped(), "not told");
        assert!(!record.pictures_reshaped(), "told twice");
    }

    #[test]
    fn a_diagram_refused_its_picture_is_not_drawn_again_at_another_width() {
        // A width change encodes again only the pictures kept: a block
        // refused one is laid out again as its text, and neither the cache,
        // its drawer nor the pictures are asked for it again.
        let (drawer, asked) = fake(100, MOST_PLACED + 1, vec![]);
        let mut record = picturing::record(40, drawer, &picturing::coloured(Theme::Dark));
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
        assert_eq!(picturing::refused(&record, 0), Some(Undrawn::TooWide));
        let before = picturing::asked(&asked);

        record.resized(80, Glyphs::Unicode);

        assert_eq!(picturing::asked(&asked), before);
        assert_eq!(picturing::refused(&record, 0), Some(Undrawn::TooWide));
        assert_eq!(record.folded(0).len(), 1, "laid out as its text");
    }

    #[test]
    fn a_diagram_refused_its_picture_for_its_size_is_not_drawn_again_in_a_new_palette() {
        // No palette changes a picture's size, so a block refused one for it
        // is laid out again as its text in the new colours, and nothing is
        // asked for it; one not drawn here may be drawn now, and is asked.
        let refusals = [
            (fake(100, MOST_PLACED + 1, vec![]), Undrawn::TooWide),
            (
                fake(MOST_KEPT_IMAGE_BYTES, 3, vec![]),
                Undrawn::TooLargeToKeep,
            ),
            (fake(100, 3, vec![]), Undrawn::PastCap),
            (fake(100, 3, vec![]), Undrawn::NotHere),
        ];
        let light = picturing::coloured(Theme::Light);
        for ((mut fake, asked), why) in refusals {
            if matches!(why, Undrawn::PastCap | Undrawn::NotHere) {
                fake.refuses = Some(why);
            }
            let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
            record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
            assert_eq!(picturing::refused(&record, 0), Some(why));
            let before = picturing::asked(&asked);

            record.pictured_as(
                Capability::Graphics {
                    protocol: Protocol::Kitty,
                    cell: picturing::CELL,
                },
                light.identity(),
            );
            record.relay(40, Glyphs::Unicode, Theme::Light);

            let after = picturing::asked(&asked);
            if why == Undrawn::NotHere {
                assert_eq!(
                    after.get(before.len()..),
                    Some(before.as_slice()),
                    "{why:?}: not asked again"
                );
            } else {
                assert_eq!(after, before, "{why:?}: asked again");
            }
            assert_eq!(picturing::refused(&record, 0), Some(why), "{why:?}");
            assert_eq!(record.folded(0).len(), 1, "{why:?}: laid out as its text");
        }
    }

    #[test]
    fn a_new_theme_encodes_a_kept_picture_again_without_its_drawer() {
        let (fake, asked) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);

        let light = picturing::coloured(Theme::Light);
        record.pictured_as(
            Capability::Graphics {
                protocol: Protocol::Kitty,
                cell: picturing::CELL,
            },
            light.identity(),
        );
        record.relay(40, Glyphs::Unicode, Theme::Light);

        assert_eq!(
            picturing::asked(&asked),
            ["cached a", "picture a", "make a", "reencode"]
        );
        assert!(picturing::shown(&record, 0));
    }

    #[test]
    fn a_kept_picture_first_shown_before_the_window_said_its_height_keeps_those_rows() {
        // A native session keeps a picture in the rows it was first shown in,
        // and a terminal that has not yet said how tall it is has shown it in
        // rows all the same; the height it says next places nothing again.
        let (fake, asked) = fake(100, 20, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        let placed = |record: &Record| match record.lines.front() {
            Some(Line::Drawn(Drawn {
                picture: Some(picture),
                ..
            })) => Some((picture.id, picture.key.rows, picture.kept.payload.cells())),
            _ => None,
        };
        record.windowed(0, true);
        record.drawn(picturing::diagram("a"), Glyphs::Unicode, Theme::Dark);
        let first = placed(&record);
        let folded = record.folded(0);
        assert!(first.is_some(), "the diagram is shown as a picture");

        record.windowed(8, true);
        record.relay(40, Glyphs::Unicode, Theme::Dark);

        assert_eq!(placed(&record), first, "its number, rows and cells");
        assert_eq!(record.folded(0), folded, "the rows it is laid in");
        assert_eq!(
            picturing::asked(&asked),
            ["cached a", "picture a", "make a"]
        );
    }

    #[test]
    fn a_picture_falling_back_to_text_leaves_the_reader_on_the_line_they_were_on() {
        // Every picture three rows tall, and its text one, so the oldest
        // falling back moves every row under it up by two.
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        for line in 0..MOST_KEPT_IMAGES {
            record.drawn(
                picturing::diagram(&format!("d{line}")),
                Glyphs::Unicode,
                Theme::Dark,
            );
            record.write(Slot::Plain, &format!("said {line}\n"), None);
        }

        // On the line under the second picture, above the foot.
        record.seek(rows_of(&record, 3).start, 4);
        let reading = said(&record, 4);
        assert_eq!(reading.first().map(String::as_str), Some("said 1"));

        record.drawn(picturing::diagram("last"), Glyphs::Unicode, Theme::Dark);

        assert!(!picturing::shown(&record, 0), "the oldest fell back");
        assert_eq!(said(&record, 4), reading);
    }

    /// A closed table whose only body row says `name`.
    fn table(name: &str) -> Block {
        Block {
            kind: markdown::Kind::Table,
            source: format!(
                "| name | what it does |\n|---|---|\n| {name} | lays a table out again |\n"
            ),
            closed: true,
        }
    }

    /// A block of `kind` written as `source`.
    fn block(kind: markdown::Kind, source: &str) -> Block {
        Block {
            kind,
            source: source.to_owned(),
            closed: true,
        }
    }

    /// The absolute display rows of the record's `line`.
    fn rows_of(record: &Record, line: usize) -> Range<usize> {
        let start: usize = (0..line).map(|before| record.folded(before).len()).sum();
        start..start + record.folded(line).len()
    }

    #[test]
    fn every_row_of_a_drawn_block_gives_its_source_and_prose_rows_give_none() {
        let mut record = Record::new(60);
        record.write(Slot::Plain, "before\n", None);
        record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
        record.write(Slot::Plain, "after\n", None);

        let rows = rows_of(&record, 1);
        assert!(rows.len() > 1, "a table draws more than a row: {rows:?}");
        for row in rows {
            assert_eq!(
                record.source_of(row),
                Some(table("alpha").source.as_str()),
                "row {row}"
            );
        }
        assert_eq!(record.source_of(0), None);
        assert_eq!(record.source_of(rows_of(&record, 2).start), None);
        assert_eq!(record.source_of(9_999), None);
    }

    #[test]
    fn a_rule_has_no_source_to_give() {
        let mut record = Record::new(40);
        record.drawn(
            block(markdown::Kind::Rule, "---\n"),
            Glyphs::Unicode,
            Theme::Dark,
        );

        assert_eq!(record.lines(), 1);
        assert_eq!(record.source_of(0), None);
        assert_eq!(record.sources().count(), 0);
    }

    #[test]
    fn the_sources_are_walked_newest_first_by_the_word_they_are_called() {
        let mut record = Record::new(60);
        record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
        record.write(Slot::Plain, "between\n", None);
        record.drawn(
            block(markdown::Kind::DisplayMath, "$$\nx^2\n$$\n"),
            Glyphs::Unicode,
            Theme::Dark,
        );
        record.drawn(
            block(markdown::Kind::Diff, "```diff\n-a\n+b\n```\n"),
            Glyphs::Unicode,
            Theme::Dark,
        );

        let walked: Vec<(usize, &str, &str)> = record
            .sources()
            .map(|source| (source.line, source.word, source.text))
            .collect();

        assert_eq!(
            walked,
            [
                (3, "diff", "```diff\n-a\n+b\n```\n"),
                (2, "math", "$$\nx^2\n$$\n"),
                (0, "table", table("alpha").source.as_str()),
            ]
        );
    }

    /// An image named by `path`, read by `by`.
    fn named<'a>(path: &'a str, by: Option<&'a str>) -> Named<'a> {
        Named { path, by }
    }

    #[test]
    fn an_image_is_listed_by_its_path_under_the_word_image() {
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.image(
            named("out/plot.png", None),
            Ok(vec![1]),
            Glyphs::Unicode,
            Theme::Dark,
        );
        record.image(
            named("gone.png", Some("read")),
            Err(Undrawn::ImageNotShown),
            Glyphs::Unicode,
            Theme::Dark,
        );

        let walked: Vec<(usize, &str, &str)> = record
            .sources()
            .map(|source| (source.line, source.word, source.text))
            .collect();

        assert_eq!(
            walked,
            [(1, "image", "gone.png"), (0, "image", "out/plot.png")]
        );
    }

    #[test]
    fn an_image_is_made_once_and_encoded_again_from_its_png_for_a_new_theme_or_size() {
        let (fake, asked) = fake(100, 20, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.windowed(8, false);
        record.image(
            named("out/plot.png", None),
            Ok(vec![1]),
            Glyphs::Unicode,
            Theme::Dark,
        );
        assert!(picturing::shown(&record, 0));
        let folded = record.folded(0);
        assert_eq!(
            folded.first().map(Row::text).as_deref(),
            Some("┌─ image · out/plot.png")
        );
        assert!(folded.len() > 1, "{folded:?}");

        let light = picturing::coloured(Theme::Light);
        record.pictured_as(
            Capability::Graphics {
                protocol: Protocol::Kitty,
                cell: picturing::CELL,
            },
            light.identity(),
        );
        record.relay(30, Glyphs::Unicode, Theme::Light);
        assert_eq!(picturing::asked(&asked), ["make out/plot.png", "reencode"]);
        assert!(picturing::shown(&record, 0));

        record.pictures_reshaped();
        assert!(record.cycles(0), "an image has no larger size");
        assert!(record.pictures_reshaped(), "shown larger and not told");
    }

    #[test]
    fn an_image_refused_or_let_go_keeps_its_edge_and_is_never_made_again() {
        let (mut fake, asked) = fake(100, 3, vec![]);
        fake.refuses = Some(Undrawn::TooWide);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.image(
            named("big.png", None),
            Ok(vec![1]),
            Glyphs::Unicode,
            Theme::Dark,
        );

        assert!(!picturing::shown(&record, 0));
        assert_eq!(
            record.folded(0).iter().map(Row::text).collect::<Vec<_>>(),
            ["┌─ image · big.png · too large"]
        );
        record.relay(60, Glyphs::Ascii, Theme::Dark);
        assert_eq!(picturing::asked(&asked), ["make big.png"]);
        assert_eq!(
            record.folded(0).iter().map(Row::text).collect::<Vec<_>>(),
            ["+- image - big.png - too large"]
        );
    }

    /// Every row the record holds, as plain text.
    fn all(record: &Record) -> Vec<String> {
        (0..record.lines())
            .flat_map(|line| record.folded(line))
            .map(|row| row.text())
            .collect()
    }

    #[test]
    fn a_drawn_table_is_laid_out_again_for_a_new_width_and_a_new_glyph_set() {
        let fresh = |columns, glyphs| {
            let mut record = Record::new(columns);
            record.drawn(table("alpha"), glyphs, Theme::Dark);
            all(&record)
        };
        let mut record = Record::new(60);
        record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
        assert_eq!(all(&record), fresh(60, Glyphs::Unicode));

        record.relay(20, Glyphs::Unicode, Theme::Dark);
        assert_eq!(all(&record), fresh(20, Glyphs::Unicode));
        assert_ne!(fresh(20, Glyphs::Unicode), fresh(60, Glyphs::Unicode));

        record.relay(20, Glyphs::Ascii, Theme::Dark);
        assert_eq!(all(&record), fresh(20, Glyphs::Ascii));
        assert_ne!(fresh(20, Glyphs::Ascii), fresh(20, Glyphs::Unicode));
        assert_eq!(record.rows, counted(&record));
    }

    #[test]
    fn a_new_theme_paints_a_drawn_block_without_laying_it_out_again() {
        let mut record = Record::new(40);
        record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
        let Some(Line::Drawn(before)) = record.lines.back() else {
            panic!("the table is not a drawn block: {:?}", record.lines);
        };
        let rows = before.rows.clone();

        record.relay(40, Glyphs::Unicode, Theme::Light);

        let Some(Line::Drawn(after)) = record.lines.back() else {
            panic!("the table is not a drawn block: {:?}", record.lines);
        };
        assert_eq!(after.rows, rows);
        assert_eq!(after.laid, (40, Glyphs::Unicode, Theme::Light));
    }

    #[test]
    fn the_oldest_block_leaves_under_the_ceiling_with_its_source() {
        // Laid-out forms count against the ceiling as much as their source,
        // so a record filled with nothing but tables lets the first one go
        // well before it holds MOST of them.
        let mut record = Record::new(80);
        let mut written = 0;
        while record.gone == 0 {
            record.drawn(
                table(&format!("table {written}")),
                Glyphs::Unicode,
                Theme::Dark,
            );
            written += 1;
            assert!(written <= MOST, "{MOST} tables were kept whole");
        }

        assert!(record.weight <= MOST, "{}", record.weight);
        assert_eq!(
            record.weight,
            record.lines.iter().map(Line::weight).sum::<usize>()
        );
        let sources: Vec<&str> = record
            .lines
            .iter()
            .filter_map(|line| match line {
                Line::Drawn(drawn) => Some(drawn.block.source.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(sources.len(), record.lines.len());
        assert!(!sources.iter().any(|source| source.contains("| table 0 |")));
        assert!(
            sources
                .first()
                .is_some_and(|source| source.contains("| table 1 |"))
        );
    }

    /// A row of `text`, laid out rather than flowed.
    fn set(text: &str) -> Row {
        let mut row = Row::new();
        row.push(Slot::Plain, text);
        row
    }
}
