//! Markdown, read as an answer streams.
//!
//! A model answers in markdown whether or not anything asked it to, so the
//! choice is between showing the markers and reading them. This reads them: a
//! marker is recognised, dropped, and the run it covered is handed on wearing a
//! [`Slot`] instead. What it does *not* do is turn the model's text into escape
//! sequences — the run and its slot arrive separately and the slot is applied as
//! the row is drawn, which is what keeps the rule that no byte from a model is
//! ever written through as an instruction.
//!
//! The scan is one character at a time and its state outlives a delta, because a
//! delta is a piece of the wire rather than a piece of the answer: a marker
//! arrives split across two of them as often as not. Nothing looks ahead past
//! the single character that ends a run of markers — an answer that stopped
//! mid-sentence has already been drawn up to where it stopped.
//!
//! Two things are held back, and both are bounded and both are given up on. A
//! line inside a fence is held because a highlighter reads whole lines, and a
//! link's words are held because nothing says they were a link's words until
//! the `](` after them. Either one that runs past its bound, or past the line
//! it is on, is handed out as the text it turned out to be.

use crate::color::Slot;
use crate::forge::Forge;
use crate::glyphs::Glyphs;

mod table;

use crate::syntax::Syntax;
use table::Table;

/// The longest run of one marker character read as a marker.
///
/// Six because that is the deepest heading. Past it the run is capped rather
/// than rejected, so `#######` is a heading — the alternative is spending a
/// counter on a line nobody writes.
const RUN: usize = 6;

/// What a run is written back as, where it turned out not to be a marker.
///
/// Static, so putting one back allocates nothing and does not borrow the delta
/// it arrived in — which matters, since a run can start in one delta and be
/// settled by the next.
const HASHES: &str = "######";
const STARS: &str = "******";
const SCORES: &str = "______";
const TICKS: &str = "``````";
const DASHES: &str = "------";
const PLUSES: &str = "++++++";
const ANGLES: &str = ">>>>>>";
const TILDES: &str = "~~~~~~";

/// A run of one marker character, waiting for the character that says what it
/// was.
#[derive(Debug, Clone, Copy)]
struct Held {
    mark: char,
    count: usize,
    /// Whether the run began before anything else on the line.
    ///
    /// Kept on the run rather than read when it settles, because settling is
    /// what makes the line started: by then every run looks like one in the
    /// middle of a line. It is what tells `* item` from `a *word*`, and it is
    /// the whole of what makes a bullet a bullet.
    opened: bool,
}

/// A bulleted item's mark, through the life of the item it belongs to.
///
/// A mark is not drawn where its bullet settles, because what the mark *is*
/// depends on what comes after it: `- [ ] a thing` is a task and a task's mark
/// is a box. A scan reading one character at a time cannot know that yet, so
/// the mark is owed until the first thing that is not a box arrives, and paid
/// immediately before it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Marked {
    /// Nothing on this line is a bulleted item.
    #[default]
    Nothing,
    /// A bullet whose mark is owed.
    Owed,
    /// A mark already drawn: a plain bullet, or a task nobody has finished.
    Drawn,
    /// A task somebody has finished.
    ///
    /// Its words wear [`Slot::Done`] whatever else is written into them: a
    /// finished task is behind you, and the emphasis inside one is something
    /// leant on while it was still ahead.
    Done,
}

/// What is open on the line being read.
///
/// Most of it ends where the line does. Emphasis is the exception, because a
/// model writes a bold phrase and lets it wrap, and a run closed on the line
/// after the one that opened it is the case that actually happens -- read
/// per-line, the opening marker is eaten and the closing one is printed into
/// the prose, which is worse than either answer.
///
/// So emphasis crosses a line break and nothing else does, and it crosses only
/// where the paragraph does: a blank line ends it, a block mark opening the
/// next line ends it, and a fence ends it. An unclosed marker then costs the
/// paragraph it was written in rather than the rest of the answer.
#[derive(Debug, Default, Clone, Copy)]
struct Line {
    heading: bool,
    code: bool,
    /// What the line's bullet is marked with, where it has one.
    marked: Marked,
    /// Whether the line opened with a quote mark, so the rest of it is
    /// somebody else's words.
    quoted: bool,
    emphasis: Emphasis,
}

/// The emphasis open on the line being read.
///
/// Two of them, because markdown spells both with the same character and tells
/// them apart by how many there are: one marker leans on a phrase and two raise
/// it, and a phrase written with three is under both at once.
#[derive(Debug, Default, Clone, Copy)]
struct Emphasis {
    /// One marker around the phrase.
    leant: bool,
    /// Two or more.
    raised: bool,
    /// Two tildes around it: written, and then taken back.
    struck: bool,
}

/// What the scan is in the middle of, across lines.
///
/// The fence is the one thing a line break does not end — that is what a fence
/// is for, and a block of code is the one place where running past its end is
/// less wrong than stopping at the first line of it. So one the model never
/// closes does run to the end of the message; the message boundary is where it
/// is cleared.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Inside {
    #[default]
    Prose,
    /// The rest of an opening fence's line, which names a language.
    Opening,
    /// A fenced block, until a fence at the start of a line closes it.
    Fence,
    /// The rest of a closing fence's line.
    Closing,
}

/// Which part of a link's shape is arriving.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Between `[` and the `]` that ends it.
    #[default]
    Label,
    /// Just past the `]`, where only a `(` carries on being a link.
    Between,
    /// Between `(` and the `)` that ends it.
    Target,
}

/// A link's words and its address, while the shape that decides them is still
/// arriving.
///
/// The one thing here that cannot be handed on as it is read. Nothing says the
/// words were a link's words until the `](` after them, so they are held until
/// it arrives -- and put back exactly as they were written where it does not,
/// which is what leaves `[TODO] fix this` alone.
#[derive(Debug, Default, Clone)]
struct Link {
    part: Part,
    label: String,
    target: String,
}

impl Link {
    /// Whether there is room for another character.
    ///
    /// Bounded for the reason the line of code beside it is: a buffer that
    /// grows with the answer is the one thing this crate may not have. Past the
    /// bound the shape stops being a link and is put back as the text it is.
    fn holds(&self) -> bool {
        self.label.len().saturating_add(self.target.len()) < MOST
    }
}

/// A number the answer wrote, while the characters that decide it arrive.
///
/// Held for the reason an address is: `#487` is one word to the reader and a
/// marker followed by digits to a scan reading characters, and whether it is a
/// reference at all is not known until something that is not a digit turns up.
#[derive(Debug, Clone, Default)]
struct Reference {
    /// The word the answer put in front of the number to say what kind of
    /// thing it counts -- `PR `, `issue ` -- where it put one. Peeled off the
    /// run for the reason the repository is: it is part of what the reader
    /// takes the reference to be, so it is part of the words they click.
    lead: Option<Box<str>>,
    /// The repository the reference named in front of its hash, where it named
    /// one. Peeled off the run rather than read here, because it arrived
    /// before the hash that said it was a repository at all.
    slug: Option<Box<str>>,
    /// The digits after the hash, as far as they have arrived.
    number: String,
}

/// How many digits a number is read as, at most.
///
/// A forge counts in the thousands and this is room for a great deal more than
/// that. The bound is here for the reason every other one in this file is:
/// what is held comes off a wire, and a run of digits nobody meant to end is
/// not a reason to grow a string without limit.
const DIGITS: usize = 12;

/// What a repository is named with, either side of the slash.
fn named(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '_' | '-')
}

/// The words an answer puts in front of a number to say what it counts.
///
/// Matched without regard to case, and only as whole words: `PR #12` and
/// `issue #12` are read the way they are written, as one thing, where `prepr
/// #12` names nothing. Short, on purpose -- a word here goes into the link's
/// words, so each is one a reader would expect to be part of the reference.
const LEADS: [&str; 3] = ["pull request", "issue", "pr"];

/// How many bytes at the end of `pending` are a lead word and the one space
/// after it, where they are.
///
/// Zero where there is none. Whatever is in front of the word has to be a word
/// boundary -- the start of the run, or a character no word is spelled with --
/// so the end of a longer word is not taken for the lead it happens to end in.
fn lead(pending: &str) -> usize {
    let Some(words) = pending.strip_suffix(' ') else {
        return 0;
    };
    let lower = words.to_ascii_lowercase();

    LEADS
        .into_iter()
        .filter(|one| lower.ends_with(one))
        .map(|one| one.len() + 1)
        .find(|back| {
            words
                .get(..words.len() + 1 - back)
                .and_then(|before| before.chars().next_back())
                .is_none_or(|before| !before.is_alphanumeric())
        })
        .unwrap_or(0)
}

/// How many bytes at the end of `pending` name a repository, where they do.
///
/// `owner/repo` and nothing else: one slash, a name either side of it, and
/// nothing but a word boundary in front. Two slashes is a path somebody wrote
/// and a path is not a repository, which is what keeps `src/cli/draw.rs#L20`
/// out of this.
fn repository(pending: &str) -> Option<usize> {
    let back = pending.len() - pending.trim_end_matches(|c| named(c) || c == '/').len();
    let slug = pending.get(pending.len() - back..)?;
    let (owner, repo) = slug.split_once('/')?;

    let spelled = |part: &str| !part.is_empty() && part.chars().all(named);
    (spelled(owner) && spelled(repo)).then_some(back)
}

/// The schemes an address is recognised by.
///
/// Two, and both of them the web's: an answer about code is full of words with
/// a colon in them, and the ones a reader can act on in a terminal are these.
const SCHEMES: [&str; 2] = ["https://", "http://"];

/// How much of a scheme a run of characters has spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelled {
    /// Not a scheme and not on the way to one.
    Nothing,
    /// A scheme so far, with more of it to come.
    Partly,
    /// A scheme, and everything after it belongs to the address.
    Whole,
}

/// How much of a scheme `text` has spelled.
fn spelled(text: &str) -> Spelled {
    if SCHEMES.iter().any(|scheme| text.starts_with(scheme)) {
        Spelled::Whole
    } else if SCHEMES.iter().any(|scheme| scheme.starts_with(text)) {
        Spelled::Partly
    } else {
        Spelled::Nothing
    }
}

/// Reads markdown out of a stream of deltas.
#[derive(Debug, Default)]
pub struct Markdown {
    /// A run of markers whose meaning the next character decides.
    held: Option<Held>,
    /// Something other than a space has arrived on this line.
    ///
    /// A heading is only one at the start of a line; anywhere else it is a hash.
    started: bool,
    /// The character before the run being held, for the one rule that needs it.
    previous: char,
    /// A bare address being read, from the first letter of its scheme on.
    ///
    /// Held for the same reason a link's words are: an address is one word to
    /// the reader and a run of markers to a scan reading characters. Held from
    /// the first letter rather than from the scheme, because by the time the
    /// scheme is spelled out the letters that spelled it have gone -- so a word
    /// that merely starts like one is held for as long as it could still turn
    /// into one, which is never more than a few characters.
    address: Option<String>,
    /// A number being read, from the hash that opened it on.
    reference: Option<Reference>,
    /// The repository a bare number is counted against, where the session
    /// knows one.
    ///
    /// Optional because a checkout with no forge behind it is an ordinary
    /// place to work, and because a number pointing at the wrong repository is
    /// worse than a number pointing nowhere. Held rather than passed with each
    /// delta, because unlike the room this cannot change while a message is
    /// arriving: the checkout is the one it was when the session opened.
    forge: Option<Forge>,
    /// A backslash is standing, waiting to see what it was put in front of.
    ///
    /// Held rather than written, because what it does is decided by the next
    /// character: a marker after it is drawn as itself and the backslash is
    /// gone, and anything else gives the backslash back. It outlives a delta
    /// for the reason everything here does -- a delta is a piece of the wire,
    /// and the pair arrives split as often as not.
    escaped: bool,
    /// The spaces this line opened with, held for whatever follows them.
    ///
    /// Whether they were indentation at all is the first marker's to say: the
    /// ones in front of a fence are part of the fence and go with its line,
    /// and every other marker keeps them, which is what nests one list inside
    /// another. Counted rather than kept, since a space is a space.
    indent: usize,
    line: Line,
    inside: Inside,
    /// The marker the open block was fenced with.
    ///
    /// A block is closed by the marker that opened it and by no other, so a
    /// row of backticks inside a block fenced with tildes is a row of
    /// backticks. Read only while a block is open, which is the only time it
    /// has been written.
    fenced: char,
    /// What the opening fence said the block is written in, while that fence's
    /// own line is still being read.
    language: String,
    /// The reader for this block, where the language was one this build knows.
    ///
    /// `None` for a fence that named nothing, named something unrecognised, or
    /// is not open at all — and then a block is drawn exactly as it was before
    /// any of this existed, quiet and whole.
    syntax: Option<Syntax>,
    /// A link whose shape is still arriving, and the text held back for it.
    link: Option<Link>,
    /// The block of bars being gathered, where one is.
    ///
    /// The only thing here that outlives a line without being an [`Inside`]:
    /// what a table is waiting for is not a character that ends it but a line
    /// that is not one of its own, which is a question asked at the start of
    /// every line rather than settled once.
    table: Option<Table>,
    /// The line of code collected so far, waiting for the break that completes
    /// it.
    ///
    /// A highlighter reads whole lines, and a delta is a piece of the wire — so
    /// the two are joined here, which is the only place that knows where a line
    /// ends. Bounded: past [`MOST`] the line is handed on as it stands and the
    /// rest of it follows plain, because a buffer that grows with the answer is
    /// the one thing this crate may not have.
    code: String,
    /// Which characters a bullet and a quote bar are drawn with.
    ///
    /// The one thing here that is a setting rather than a piece of the scan,
    /// and it is here because a marker that is dropped has to be replaced by
    /// something a font actually has. Carried across the reset at the end of
    /// every line, since it is a fact about the terminal rather than about the
    /// answer.
    glyphs: Glyphs,
}

/// The most of one line of code that is held back to be read.
///
/// Four thousand columns is far past any line anybody wrote and far short of
/// anything that matters to the budget. A line longer than this is drawn plain
/// rather than dropped or held.
const MOST: usize = 4096;

/// The spaces a held indentation is handed on from, taken as a slice rather
/// than built: an indent is a handful of columns and this is the render path.
const SPACES: &str = "                                ";

impl Markdown {
    /// A reader drawing its bullets and quote bars with `glyphs`.
    #[must_use]
    pub fn new(glyphs: Glyphs) -> Self {
        Self {
            glyphs,
            ..Self::default()
        }
    }

    /// The same reader, counting bare numbers against `forge`.
    #[must_use]
    pub fn counting(mut self, forge: Option<Forge>) -> Self {
        self.forge = forge;
        self
    }

    /// Reads `delta`, handing each run of text to `say` under its slot.
    ///
    /// Runs rather than characters: the text between two markers is one call,
    /// so a delta with no markers in it is one call for the whole delta.
    /// Markers themselves are never handed on.
    ///
    /// `room` is how many columns the caller has to draw into. An argument
    /// rather than something this holds, because it is the one fact here that
    /// changes without a delta arriving: a window resized between two of them
    /// would leave a field stale, and the caller knows the size at every call
    /// anyway.
    pub fn read(
        &mut self,
        delta: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        // Where the text not yet handed on begins.
        let mut run = 0;

        for (at, character) in delta.char_indices() {
            let next = at.saturating_add(character.len_utf8());

            // A block of bars takes every character in it, so nothing below
            // runs while one is open and the run stays empty until it closes.
            if self.table.is_some() {
                if self.tabled(character, room, say) {
                    run = next;
                    continue;
                }
                run = at;
            }

            match self.held {
                // More of the run being held. Not text, and not decided yet.
                Some(held) if held.mark == character => {
                    self.held = Some(Held {
                        count: held.count.saturating_add(1).min(RUN),
                        ..held
                    });
                    run = next;
                    continue;
                }
                // The run is over, and this character says what it was.
                Some(held) => {
                    if self.settle(held, character, room, say) {
                        run = next;
                        continue;
                    }
                    run = at;
                }
                None => {}
            }

            // Held text, not a run: a link's words are the one thing here that
            // cannot be handed on where they stand, so while one is arriving
            // the run is empty and every character is decided below.
            if self.link.is_some() {
                if self.links(character, say) {
                    run = next;
                    continue;
                }
                // Not a link after all. What was held has gone back into the
                // stream and this character is ordinary, so it starts the run.
                run = at;
            }

            // A backslash is standing and this is the character it was put in
            // front of.
            // Held text, not a run, for the reason a link's words are.
            if self.address.is_some() {
                if self.addressed(character, say) {
                    run = next;
                    continue;
                }
                // Not an address after all, or one that has just ended. What
                // was held has been drawn or put back, and this character is
                // ordinary, so it starts the run.
                run = at;
            }

            // Held text, not a run, for the reason an address is.
            if self.reference.is_some() {
                if self.referenced(character, say) {
                    run = next;
                    continue;
                }
                // The number has ended, or there was never one behind the
                // hash. Either way what was held has been drawn or put back,
                // and this character starts the run.
                run = at;
            }

            if std::mem::take(&mut self.escaped) {
                if Self::escapes(character) {
                    self.say(delta.get(at..next).unwrap_or_default(), say);
                    self.started = true;
                    self.previous = character;
                    run = next;
                    continue;
                }

                // In front of nothing this scan would have acted on, so it was
                // a character of the answer rather than a word about the next
                // one. The run starts at this character, which is where the
                // backslash left it.
                self.say("\\", say);
                self.started = true;
                self.previous = '\\';
            }

            if character == '\n' {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.end_line(room, say);
                run = next;
            } else if self.opens_link(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.link = Some(Link::default());
                self.started = true;
                run = next;
            } else if self.opens_table(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.table = Some(Table::opening());
                self.tabled(character, room, say);
                run = next;
            } else if self.opens_address(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.address = Some(String::from(character));
                run = next;
            } else if let Some(back) =
                self.opens_reference(character, delta.get(run..at).unwrap_or_default())
            {
                // What the repository was named with is the reference's, not
                // the prose's: it goes into the hold rather than out, so the
                // words the reader clicks are the words the answer wrote.
                let pending = delta.get(run..at).unwrap_or_default();
                let (text, slug) = pending.split_at(pending.len() - back);
                // The word saying what the number counts is the reference's
                // too, where the answer wrote one, and for the same reason.
                let (text, lead) = text.split_at(text.len() - lead(text));
                self.say(text, say);
                self.reference = Some(Reference {
                    lead: (!lead.is_empty()).then(|| lead.into()),
                    slug: (!slug.is_empty()).then(|| slug.into()),
                    number: String::new(),
                });
                self.started = true;
                run = next;
            } else if self.escaping(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.escaped = true;
                run = next;
            } else if self.marks(character) {
                // Nothing has been written on the line yet, so what stands in
                // front of this marker is the whitespace that indented it, and
                // it is held rather than said until the marker says what it was.
                if self.started {
                    self.say(delta.get(run..at).unwrap_or_default(), say);
                } else {
                    self.indent += delta.get(run..at).map_or(0, str::len);
                }
                self.held = Some(Held {
                    mark: character,
                    count: 1,
                    opened: !self.started,
                });
                run = next;
            } else if matches!(self.inside, Inside::Opening | Inside::Closing) {
                // What is left of a fence's own line names the language it is
                // written in, or nothing at all. Either way it is not the
                // answer, so it goes the way the fence went — but the opening
                // one is kept, because it is what decides how the block is read.
                if self.inside == Inside::Opening && self.language.len() < MOST {
                    self.language.push(character);
                }
                run = next;
            } else {
                self.started |= character != ' ';
                self.previous = character;
            }
        }

        self.say(delta.get(run..).unwrap_or_default(), say);
    }

    /// Whether `character` starts a run of markers where the scan stands.
    fn marks(&self, character: char) -> bool {
        match self.inside {
            // Inside a fence the only marker left is the fence that closes it,
            // and only at the start of a line. Code is full of the others.
            Inside::Fence => character == self.fenced && !self.started,
            // A fence's own line is dropped whole; nothing on it marks anything.
            Inside::Opening | Inside::Closing => false,
            Inside::Prose => match character {
                '`' => true,
                // Inside a span the backtick above is the only marker left,
                // for the reason a fence is: the words in there are code, and
                // code is full of the others. `*ptr` is a pointer, `_private`
                // is a name, and `**kwargs` is how one language spells an
                // argument -- every one of them a thing a coding agent writes
                // far more often than it writes emphasis inside a span.
                _ if self.line.code => false,
                '*' | '_' | '~' => true,
                // Only where nothing else has been written on the line. A
                // hash is a heading there and a comment everywhere else; a
                // dash is a bullet there and a minus sign everywhere else.
                '#' | '-' | '+' | '>' => !self.started,
                _ => false,
            },
        }
    }

    /// Whether `character` is a backslash standing in front of something.
    ///
    /// Prose only. In code a backslash is a character like any other, and a
    /// span is where a pattern goes to be written down exactly.
    fn escaping(&self, character: char) -> bool {
        character == '\\' && self.inside == Inside::Prose && !self.line.code
    }

    /// Whether a standing backslash was put in front of this to say it means
    /// nothing.
    ///
    /// Only what this scan would otherwise have acted on, rather than every
    /// piece of punctuation a stricter reading escapes: `\d` and `\.` are a
    /// pattern far more often than they are prose that meant a `d` or a stop,
    /// and dropping the backslash there would change what somebody copies off
    /// the screen into something that no longer matches.
    fn escapes(character: char) -> bool {
        matches!(
            character,
            '\\' | '*' | '_' | '`' | '~' | '#' | '-' | '+' | '>' | '|' | '['
        )
    }

    /// Decides what a finished run of markers was, given the character that
    /// ended it. Answers whether that character was part of the marker.
    fn settle(
        &mut self,
        held: Held,
        next: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        self.held = None;
        self.started = true;

        match held.mark {
            // Three or more of one marker with nothing else on the line: a
            // rule between the blocks either side of it. Drawn across the room
            // the caller has rather than as the three characters it was
            // written with, because what the model meant by them is a
            // separator and a separator that stops after three columns reads
            // as a stray. This stands above every arm below it: a line that is
            // nothing but markers cannot also be emphasis, a bullet, or the
            // start of anything.
            '-' | '*' | '_' if held.opened && held.count >= 3 && next == '\n' => {
                self.indent = 0;
                for _ in 0..room {
                    say(Slot::Quiet, self.glyphs.horizontal(), None);
                }
                false
            }
            // The hashes and the one space after them are the marker, and what
            // is left of the line is the heading itself.
            '#' if next == ' ' => {
                self.opens_block();
                self.line.heading = true;
                true
            }
            // Three or more open a block that outlives the line it is on, or
            // close the one already open.
            // Either marker, because a model reaches for tildes exactly when
            // the block is full of backticks -- and a block read as prose is
            // code with its markers taken out of it, which is the worst thing
            // this reader can do to an answer.
            '`' | '~' if held.count >= 3 && self.fences(held.mark) => {
                // A fence's own line goes whole, and the spaces in front of it
                // are part of that line: an item's block would otherwise open
                // with the item's indentation stitched to its first row.
                self.indent = 0;
                self.inside = if self.inside == Inside::Fence {
                    Inside::Closing
                } else {
                    self.fenced = held.mark;
                    Inside::Opening
                };
                false
            }
            '`' if self.inside == Inside::Prose => {
                self.line.code = !self.line.code;
                false
            }
            // A bullet, and the space after it, are the marker — so what is
            // drawn in their place is a mark and a space of this crate's own,
            // out of the set the rest of the interface is drawn from. The
            // indentation before it has already gone out, so a nested item
            // stays nested. Owed rather than drawn here: see [`Line::owed`].
            '-' | '+' | '*' if held.opened && held.count == 1 && next == ' ' => {
                self.opens_block();
                self.line.marked = Marked::Owed;
                true
            }
            // A quote is a bar down the left and the words beside it, which is
            // what a reader already knows a quote looks like. The whole line
            // goes quiet: the point of a quote is that the words are somebody
            // else's.
            '>' if held.opened && held.count == 1 && next == ' ' => {
                self.opens_block();
                self.line.quoted = true;
                self.spend(say);
                say(Slot::Quiet, self.glyphs.vertical(), None);
                say(Slot::Quiet, " ", None);
                true
            }
            // Exactly two, because that is the only run markdown has ever
            // meant a retraction by -- which is also what keeps `~/Projects` a
            // path: one tilde falls through and is written back as itself,
            // and three are the fence handled above.
            '~' if held.count == 2 && self.strikes(next) => {
                self.line.emphasis.struck = !self.line.emphasis.struck;
                false
            }
            // One marker is emphasis and two are weight, which is what markdown
            // has always meant by them and what a model writes expecting to be
            // read that way. Three or more are both, and the louder of the two
            // is the one worth a reader's attention.
            '*' | '_' if self.emphasises(held, next) => {
                let worn = if held.count >= 2 {
                    &mut self.line.emphasis.raised
                } else {
                    &mut self.line.emphasis.leant
                };
                *worn = !*worn;
                false
            }
            _ => {
                self.previous = held.mark;
                self.say(written(held), say);
                false
            }
        }
    }

    /// Drops the emphasis carried out of the line before this one.
    ///
    /// A block mark opens something of its own, and what it opens is not the
    /// paragraph an unclosed marker was left open in. See [`Line`].
    fn opens_block(&mut self) {
        self.line.emphasis = Emphasis::default();
    }

    /// Whether a run of three or more `mark` is this line's fence.
    ///
    /// One opens a block wherever a block is not already open, and closes only
    /// the block its own marker opened. See [`Markdown::fenced`].
    fn fences(&self, mark: char) -> bool {
        self.inside != Inside::Fence || self.fenced == mark
    }

    /// Whether a run of two tildes followed by `next` turns a retraction on or
    /// off.
    ///
    /// The same rule the stars are held to, for the same reason: something has
    /// to follow on the same line for a run to open, so a pair left dangling at
    /// the end of one strikes nothing.
    fn strikes(&self, next: char) -> bool {
        self.line.emphasis.struck || !next.is_whitespace()
    }

    /// Whether a run of markers followed by `next` turns emphasis on or off.
    fn emphasises(&self, held: Held, next: char) -> bool {
        // Whichever of the two this run would close, since what a marker may do
        // depends on whether its own is already open.
        let open = if held.count >= 2 {
            self.line.emphasis.raised
        } else {
            self.line.emphasis.leant
        };

        if held.mark == '_' {
            return if open {
                // Closes at the end of a word rather than inside one, so
                // `_borrowed_ from` closes and `_borrowed_from` does not.
                !next.is_alphanumeric()
            } else {
                // Opens only between two words rather than beside one: a
                // word must not already be under way, and one has to start
                // here. `read_to_string` is a function far more often than it
                // is a sentence with emphasis in the middle of it -- and
                // `Ok(_)` is a pattern far more often than it is the start of
                // one, which the whitespace this used to ask about could not
                // tell apart, since the bracket that closes it is not
                // whitespace either.
                !self.previous.is_alphanumeric() && next.is_alphanumeric()
            };
        }

        // Something has to follow on the same line for a run to open. That is
        // what leaves `* item` a bullet instead of turning the rest of the
        // paragraph bold.
        open || !next.is_whitespace()
    }

    /// Whether `character` opens a block of bars where the scan stands.
    ///
    /// Only where nothing else has been written on the line, and not inside a
    /// span of code: a bar in the middle of a line is `a | b` in a shell and
    /// `Ok(_) | Err(_)` in a match, and neither is a table.
    fn opens_table(&self, character: char) -> bool {
        character == '|' && self.inside == Inside::Prose && !self.started && !self.line.code
    }

    /// Reads `character` into the block of bars being held. Answers whether it
    /// belonged to the block.
    ///
    /// `false` says the block is over: what it turned out to be has already
    /// gone out, and `character` is the caller's again — the first character of
    /// a line that is not part of any table, which is why the line state goes
    /// back to what it is at the start of one.
    fn tabled(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut table) = self.table.take() else {
            return false;
        };

        // A line that does not open with a bar is not the table's, and the
        // table ended above it.
        if table.fresh() && character != '|' {
            table.laid(self.glyphs, room, say);
            self.line = Line::default();
            self.started = false;
            return false;
        }

        // Held longer than a table is worth waiting for. Out it goes as the
        // model wrote it, and the rest of the block is read as the prose it
        // now is.
        if !table.takes(character) {
            table.spilt(say);
            return true;
        }

        // The delimiter row is the second line or there is no table here.
        if !table.possible() {
            table.spilt(say);
            return true;
        }

        self.table = Some(table);
        true
    }

    /// Draws the bullet's own mark, where one is owed.
    ///
    /// Called immediately before the first thing on the item reaches the
    /// terminal, whichever path that is — a run of prose, a link that arrived
    /// whole, a bracket that turned out not to be one, or the break of an item
    /// with nothing after its marker at all.
    fn pay(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.line.marked != Marked::Owed {
            return;
        }

        self.line.marked = Marked::Drawn;
        say(Slot::Quiet, self.glyphs.bullet(), None);
        say(Slot::Quiet, " ", None);
    }

    /// Whether what a bullet is holding is a task's box rather than a link.
    ///
    /// Only where the mark is still owed, so `- [ ] a thing` is a task and
    /// `- see [ ] in the grammar` is a bracket somebody wrote. `character` is
    /// the space that follows the box, which is part of the marker and goes
    /// with it.
    fn boxed(&self, link: &Link, character: char) -> bool {
        self.line.marked == Marked::Owed
            && link.part == Part::Between
            && character == ' '
            && matches!(link.label.as_str(), "" | " " | "x" | "X")
    }

    /// Draws a task's box in place of the bullet's mark, and says whether the
    /// task is one somebody has finished.
    fn tick(&mut self, label: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if matches!(label, "x" | "X") {
            self.line.marked = Marked::Done;
            say(Slot::DoneMark, self.glyphs.done(), None);
        } else {
            self.line.marked = Marked::Drawn;
            say(Slot::Quiet, self.glyphs.open(), None);
        }

        say(Slot::Quiet, " ", None);
    }

    /// Whether `character` opens a link where the scan stands.
    ///
    /// Prose only, and not inside a span of code: a bracket in `` `arr[0]` ``
    /// is an index somebody wrote, and reading it as a link would take the
    /// index off the screen.
    fn opens_link(&self, character: char) -> bool {
        character == '[' && self.inside == Inside::Prose && !self.line.code
    }

    /// Reads `character` into the link being held. Answers whether it belonged
    /// to the link.
    ///
    /// `false` says the shape was not a link: what was held has already gone
    /// back out as the text it turned out to be, and `character` is the
    /// caller's again.
    fn links(&mut self, character: char, say: &mut dyn FnMut(Slot, &str, Option<&str>)) -> bool {
        let Some(mut link) = self.link.take() else {
            return false;
        };

        // A link does not cross a line break. Markdown allows one inside the
        // brackets; a model that opened a bracket and never closed it is the
        // case that actually happens, and it costs one line here rather than
        // the rest of the answer. Past the bound, the same.
        let carries = character != '\n' && link.holds();

        match (link.part, character) {
            (Part::Label, ']') if carries => link.part = Part::Between,
            (Part::Between, '(') if carries => link.part = Part::Target,
            (Part::Target, ')') if carries => {
                self.wrote_link(&link, say);
                return true;
            }
            (Part::Label, held) if carries => link.label.push(held),
            (Part::Target, held) if carries => link.target.push(held),
            // A box takes the bullet's mark rather than following it, which is
            // why this stands above the arm that puts a bracket back: the
            // space after `]` is the marker's own and goes with it.
            _ if self.boxed(&link, character) => {
                self.tick(&link.label, say);
                return true;
            }
            // `[TODO] fix this`, and every other bracket somebody meant.
            _ => {
                self.pay(say);
                spill(&link, say);
                return false;
            }
        }

        self.link = Some(link);
        true
    }

    /// Whether an address could start here.
    ///
    /// At the start of a word only: `shttps://` is not an address, and neither
    /// is the second half of one a link already named.
    fn opens_address(&self, character: char) -> bool {
        SCHEMES.iter().any(|scheme| scheme.starts_with(character))
            && self.inside == Inside::Prose
            && !self.line.code
            && !self.previous.is_alphanumeric()
    }

    /// Reads one character into the address being held. Answers whether that
    /// character was part of it.
    fn addressed(
        &mut self,
        character: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut address) = self.address.take() else {
            return false;
        };

        // An address ends where the word does. Bounded like everything else
        // held here: past the bound it stops being an address and is put back
        // as the text it is.
        let carries = !character.is_whitespace() && address.len() < MOST;
        if carries && spelled(&address) == Spelled::Whole {
            address.push(character);
            self.address = Some(address);
            return true;
        }

        if carries {
            address.push(character);
            if spelled(&address) != Spelled::Nothing {
                self.address = Some(address);
                return true;
            }
            // A word that started like a scheme and turned out to be a word.
            // The character that decided it is part of that word, and goes
            // back with it rather than to the caller.
            self.say(&address, say);
            self.started = true;
            self.previous = character;
            return true;
        }

        self.wrote_address(&address, say);
        false
    }

    /// Hands on an address the answer wrote bare.
    ///
    /// Drawn as the link it is, so what can be acted on in a terminal looks
    /// like it can. What ends a sentence is not part of it: a full stop after
    /// an address belongs to the prose, and a reader who copies the row gets
    /// an address that still resolves.
    fn wrote_address(&mut self, address: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let ends = address.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"']);

        // A scheme and nothing after it is a word somebody wrote, not somewhere
        // anybody can go.
        if spelled(ends) != Spelled::Whole || SCHEMES.contains(&ends) {
            self.say(address, say);
            self.started = true;
            self.previous = address.chars().next_back().unwrap_or('/');
            return;
        }

        self.pay(say);
        say(Slot::Link, ends, Some(ends));
        self.started = true;
        self.previous = ends.chars().next_back().unwrap_or('/');
        self.say(&address[ends.len()..], say);
    }

    /// How many bytes in front of `character` belong to a reference opening
    /// here, or `None` where none is.
    ///
    /// Prose only, and not inside a span of code, for the reason a link is:
    /// `#[derive(Debug)]` is an attribute and `#define` is a directive, and
    /// both are things a coding agent writes far more often than it writes a
    /// number about a repository. Not where a line has yet to start either --
    /// there a hash is a heading, and headings were spoken for first.
    ///
    /// Nothing at all without a repository in hand, which is the check that
    /// leaves every other terminal drawing exactly what it drew before.
    fn opens_reference(&self, character: char, pending: &str) -> Option<usize> {
        if character != '#'
            || self.inside != Inside::Prose
            || self.line.code
            || !self.started
            || self.forge.is_none()
        {
            return None;
        }

        // `owner/repo#12` is a number counted somewhere else, and the name in
        // front of the hash is part of the reference. Where there is no name,
        // the hash has to start a word: `abc#def` is an identifier, a fragment
        // or an anchor, and none of those is a number anybody can open.
        repository(pending).or_else(|| (!self.previous.is_alphanumeric()).then_some(0))
    }

    /// Reads one character into the reference being held. Answers whether that
    /// character was part of it.
    fn referenced(
        &mut self,
        character: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut reference) = self.reference.take() else {
            return false;
        };

        if character.is_ascii_digit() && reference.number.len() < DIGITS {
            reference.number.push(character);
            self.reference = Some(reference);
            return true;
        }

        self.wrote_reference(&reference, say);
        false
    }

    /// Hands on a number the answer wrote, as the link it is.
    ///
    /// What ends a sentence is not part of it and never was: the number ended
    /// at the first character that was not a digit, so the stop after `#487`
    /// is prose that has not been read yet rather than something to trim.
    ///
    /// A hash with no digits behind it goes back as the characters it was
    /// written with. So does one the session has nowhere to point -- the same
    /// answer drawn on a checkout with no forge behind it, which is why this
    /// asks again rather than trusting the hold to have been opened wisely.
    fn wrote_reference(
        &mut self,
        reference: &Reference,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let lead = reference.lead.as_deref().unwrap_or_default();
        let slug = reference.slug.as_deref().unwrap_or_default();
        let words = format!("{lead}{slug}#{}", reference.number);
        self.previous = words.chars().next_back().unwrap_or('#');
        self.started = true;

        let address = self
            .forge
            .as_ref()
            .filter(|_| !reference.number.is_empty())
            .map(|forge| forge.address(reference.slug.as_deref(), &reference.number));

        let Some(address) = address else {
            self.say(&words, say);
            return;
        };

        // The mark the line owes, then the words -- the order every other run
        // on a line reaches the terminal in. The indentation is not owed here:
        // a reference only opens once something has been written on the line,
        // and whatever that was spent it.
        self.pay(say);
        say(Slot::Link, &words, Some(&address));
    }

    /// Hands on a link that arrived whole.
    ///
    /// The words wear the link's own slot and carry the address, which is not
    /// written out beside them: the sentence the answer wrote goes on reading
    /// as a sentence, and the address is the row's to hand to a terminal that
    /// opens links -- which is how a reader reaches it, the same way they
    /// reach one on any page. A link with no words is its address, since that
    /// is the one thing there is to show. Nothing here is ever written as
    /// anything but text: an address is bytes a model chose, and the rule this
    /// file opens with is that none of them leaves as an instruction.
    fn wrote_link(&mut self, link: &Link, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.pay(say);

        let target = target(&link.target);
        let words = if link.label.is_empty() {
            target
        } else {
            &link.label
        };

        say(Slot::Link, words, (!target.is_empty()).then_some(target));
        self.previous = words.chars().next_back().unwrap_or(')');
    }

    /// Lets go of whatever was still being held when the message ended.
    ///
    /// A link goes back as the characters it was written with; a table is drawn
    /// as the table it turned out to be. `room` is what it is drawn against, and
    /// is the caller's for the reason it is on [`Markdown::read`].
    ///
    /// The last moment there is: the reader is dropped between messages, and
    /// text still held when that happens is text the reader never sees.
    pub fn finish(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        // A run of markers is held for the character that says what it was,
        // and the end of a message is that character never arriving. Settled
        // against a line break, because the end of a message ends everything a
        // line break ends and one more thing besides -- so a marker that meant
        // nothing comes back as itself, exactly as it would have one character
        // later, and one that opened something is consumed exactly as it would
        // have been there too.
        if let Some(held) = self.held.take() {
            self.settle(held, '\n', room, say);
        }

        // The word that would have ended the address never arrived, and the end
        // of the message ends it exactly as a space would have.
        if let Some(address) = self.address.take() {
            self.wrote_address(&address, say);
        }

        // The digit that would have carried the number on never arrived, and
        // the end of the message ends it exactly as a space would have.
        if let Some(reference) = self.reference.take() {
            self.wrote_reference(&reference, say);
        }

        // The character that would have said what it was in front of never
        // arrived, so it was in front of nothing.
        if std::mem::take(&mut self.escaped) {
            self.say("\\", say);
        }

        // After the settle above, so a fence at the very end of a message has
        // had its chance to take the spaces in front of it with it.
        self.spend(say);
        self.pay(say);

        if let Some(link) = self.link.take() {
            spill(&link, say);
        }
        if let Some(table) = self.table.take() {
            table.laid(self.glyphs, room, say);
        }
    }

    /// Ends the line, and with it everything markdown ends at one.
    fn end_line(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let ended = self.inside;
        // Belt and braces: the line break puts a link back before it gets here,
        // and the reset below would drop what one was holding without a sound.
        self.finish(room, say);

        // The line of code is complete, so this is the moment it can be read.
        self.read_code(say);

        let inside = match ended {
            Inside::Opening => Inside::Fence,
            Inside::Closing => Inside::Prose,
            inside => inside,
        };

        // A block is read in whatever its opening fence named, and the reader
        // is made here because this is where that name is finished. A block
        // that closes drops its reader with it: the next one names its own.
        // Carried only out of a line of prose that had something on it: a
        // blank line is the end of the paragraph, and a fence has no emphasis
        // to speak of. Put back below rather than here -- see the line break.
        let emphasis = if ended == Inside::Prose && self.started {
            self.line.emphasis
        } else {
            Emphasis::default()
        };

        let syntax = match ended {
            Inside::Opening => Syntax::of(&self.language),
            Inside::Closing => None,
            _ => self.syntax.take(),
        };

        *self = Self {
            // The fence is the one thing carried over, and a fence's own line
            // is where its effect begins or ends. The glyphs are not carried
            // over so much as never given up: they are what the terminal can
            // draw, which no line of an answer changes.
            inside,
            fenced: self.fenced,
            syntax,
            glyphs: self.glyphs,
            ..Self::default()
        };

        // A fence's own line is a marker, and no marker is drawn — the line
        // break with it, or every block would arrive with a blank line stitched
        // to each end of it. After the reset otherwise, so the row that ends
        // carries no slot into the row that follows it.
        if !matches!(ended, Inside::Opening | Inside::Closing) {
            say(self.slot(), "\n", None);
        }

        // After the break, so the slot the row ends under is the one it would
        // have worn before emphasis learnt to cross a line at all.
        self.line.emphasis = emphasis;
    }

    /// Hands on a run of text, if there is any.
    ///
    /// Inside a block being read, the run is held instead: a highlighter reads
    /// whole lines and this is a piece of one. Everywhere else it goes straight
    /// out, which is every run in every answer that is not code.
    fn say(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if text.is_empty() {
            return;
        }

        // The line's own order: what indented it, then the mark it owes, then
        // the words themselves.
        self.spend(say);
        self.pay(say);
        self.wears(text, say);
    }

    /// Hands on the indentation the line opened with, where it kept any.
    ///
    /// Called wherever the first thing on a line reaches the terminal, which
    /// is the moment the marker that could have claimed those spaces has
    /// either claimed them or gone. See [`Markdown::indent`].
    fn spend(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let mut indent = std::mem::take(&mut self.indent);
        while indent > 0 {
            let spaces = indent.min(SPACES.len());
            self.wears(&SPACES[..spaces], say);
            indent -= spaces;
        }
    }

    /// Hands on a run under the slot the line is wearing, or into the block
    /// being read where one is open.
    fn wears(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.syntax.is_some() && self.inside == Inside::Fence && self.code.len() < MOST {
            self.code.push_str(text);
            return;
        }

        let slot = self.slot();
        say(slot, text, None);
    }

    /// Hands on the line of code collected so far, read.
    ///
    /// Every byte comes back exactly once and in order, which is the property
    /// the whole thing rests on — a byte dropped is code that quietly changed
    /// meaning on screen, and a byte doubled is a row wider than it measured.
    fn read_code(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.code.is_empty() {
            return;
        }

        let mut code = std::mem::take(&mut self.code);
        // The reader wants the break as part of the line: a great many rules in
        // a syntax definition are written against the end of one.
        code.push('\n');

        match self.syntax.as_mut() {
            Some(syntax) => syntax.read(&code, &mut |slot, text| {
                // The break is the caller's to write, once, below.
                let text = text.strip_suffix('\n').unwrap_or(text);
                if !text.is_empty() {
                    say(slot, text, None);
                }
            }),
            None => say(Slot::Quiet, code.trim_end_matches('\n'), None),
        }
    }

    /// The slot everything read right now is written under.
    fn slot(&self) -> Slot {
        if self.inside != Inside::Prose || self.line.quoted {
            Slot::Quiet
        } else if self.line.code {
            // Above everything a phrase can otherwise be: backticks say the
            // reader is being handed something to copy or go and find, and
            // that is worth more to them than the weight around it.
            Slot::Code
        } else if self.line.marked == Marked::Done {
            Slot::Done
        } else if self.line.emphasis.struck {
            // Above weight and emphasis both. A slot says one thing, and of the
            // things a phrase can be at once, the one a reader most needs is
            // that it was taken back -- bold words the answer has retracted are
            // worse than plain ones it has not.
            Slot::Struck
        } else if self.line.heading || self.line.emphasis.raised {
            Slot::Strong
        } else if self.line.emphasis.leant {
            Slot::Emphasis
        } else {
            Slot::Plain
        }
    }
}

/// Writes a link back as the characters it was written with.
///
/// Exactly them, in order, so a bracket that was never a link costs the reader
/// nothing at all.
fn spill(link: &Link, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
    say(Slot::Plain, "[", None);
    if !link.label.is_empty() {
        say(Slot::Plain, &link.label, None);
    }
    if link.part != Part::Label {
        say(Slot::Plain, "]", None);
    }
    if link.part == Part::Target {
        say(Slot::Plain, "(", None);
        if !link.target.is_empty() {
            say(Slot::Plain, &link.target, None);
        }
    }
}

/// The address out of what stood between the brackets.
///
/// Markdown lets a title follow the address, and lets the address itself be
/// wrapped in angle brackets. Neither is the destination, and the destination
/// is the whole of what is worth a reader's columns.
fn target(between: &str) -> &str {
    between
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|character| character == '<' || character == '>')
}

/// The text a run is put back as, where it meant nothing.
fn written(held: Held) -> &'static str {
    let whole = match held.mark {
        '#' => HASHES,
        '*' => STARS,
        '`' => TICKS,
        '-' => DASHES,
        '+' => PLUSES,
        '>' => ANGLES,
        '~' => TILDES,
        _ => SCORES,
    };

    whole.get(..held.count).unwrap_or(whole)
}

#[cfg(test)]
mod tests;
