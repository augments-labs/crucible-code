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
use crate::glyphs::Glyphs;
use crate::syntax::Syntax;

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

/// What is open on the line being read.
///
/// All of it ends where the line does. Markdown lets emphasis cross a line
/// break and this does not: an unclosed marker then costs one line rather than
/// the rest of the answer, and a model that opened one by accident is the case
/// that actually happens.
#[derive(Debug, Default, Clone, Copy)]
struct Line {
    heading: bool,
    code: bool,
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
    line: Line,
    inside: Inside,
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

impl Markdown {
    /// A reader drawing its bullets and quote bars with `glyphs`.
    #[must_use]
    pub fn new(glyphs: Glyphs) -> Self {
        Self {
            glyphs,
            ..Self::default()
        }
    }

    /// Reads `delta`, handing each run of text to `say` under its slot.
    ///
    /// Runs rather than characters: the text between two markers is one call,
    /// so a delta with no markers in it is one call for the whole delta.
    /// Markers themselves are never handed on.
    pub fn read(&mut self, delta: &str, say: &mut dyn FnMut(Slot, &str)) {
        // Where the text not yet handed on begins.
        let mut run = 0;

        for (at, character) in delta.char_indices() {
            let next = at.saturating_add(character.len_utf8());

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
                    if self.settle(held, character, say) {
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

            if character == '\n' {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.end_line(say);
                run = next;
            } else if self.opens_link(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.link = Some(Link::default());
                self.started = true;
                run = next;
            } else if self.marks(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
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
            Inside::Fence => character == '`' && !self.started,
            // A fence's own line is dropped whole; nothing on it marks anything.
            Inside::Opening | Inside::Closing => false,
            Inside::Prose => match character {
                '*' | '_' | '`' | '~' => true,
                // Only where nothing else has been written on the line. A
                // hash is a heading there and a comment everywhere else; a
                // dash is a bullet there and a minus sign everywhere else.
                '#' | '-' | '+' | '>' => !self.started,
                _ => false,
            },
        }
    }

    /// Decides what a finished run of markers was, given the character that
    /// ended it. Answers whether that character was part of the marker.
    fn settle(&mut self, held: Held, next: char, say: &mut dyn FnMut(Slot, &str)) -> bool {
        self.held = None;
        self.started = true;

        match held.mark {
            // The hashes and the one space after them are the marker, and what
            // is left of the line is the heading itself.
            '#' if next == ' ' => {
                self.line.heading = true;
                true
            }
            // Three or more open a block that outlives the line it is on, or
            // close the one already open.
            '`' if held.count >= 3 => {
                self.inside = if self.inside == Inside::Fence {
                    Inside::Closing
                } else {
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
            // stays nested.
            '-' | '+' | '*' if held.opened && held.count == 1 && next == ' ' => {
                say(Slot::Quiet, self.glyphs.dot());
                say(Slot::Quiet, " ");
                true
            }
            // A quote is a bar down the left and the words beside it, which is
            // what a reader already knows a quote looks like. The whole line
            // goes quiet: the point of a quote is that the words are somebody
            // else's.
            '>' if held.opened && held.count == 1 && next == ' ' => {
                self.line.quoted = true;
                say(Slot::Quiet, self.glyphs.vertical());
                say(Slot::Quiet, " ");
                true
            }
            // Exactly two, because that is the only run markdown has ever
            // meant a retraction by -- which is also what keeps `~/Projects` a
            // path and `~~~` the fence somebody wrote: one tilde and three
            // both fall through and are written back as themselves.
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
                // Opens only where a word is not already under way:
                // `read_to_string` is a function far more often than it is a
                // sentence with emphasis in the middle of it.
                !self.previous.is_alphanumeric() && !next.is_whitespace()
            };
        }

        // Something has to follow on the same line for a run to open. That is
        // what leaves `* item` a bullet instead of turning the rest of the
        // paragraph bold.
        open || !next.is_whitespace()
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
    fn links(&mut self, character: char, say: &mut dyn FnMut(Slot, &str)) -> bool {
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
            // `[TODO] fix this`, and every other bracket somebody meant.
            _ => {
                spill(&link, say);
                return false;
            }
        }

        self.link = Some(link);
        true
    }

    /// Hands on a link that arrived whole.
    ///
    /// The words wear the link's own slot and the address follows them in
    /// brackets, quietly. Both, because a terminal is where the address is the
    /// part that can be acted on -- copied, or clicked by a terminal that finds
    /// its own links -- and words alone would be a destination the reader
    /// cannot reach. Neither is ever written as anything but text: an address
    /// is bytes a model chose, and the rule this file opens with is that none
    /// of them leaves as an instruction.
    fn wrote_link(&mut self, link: &Link, say: &mut dyn FnMut(Slot, &str)) {
        let target = target(&link.target);
        let words = if link.label.is_empty() {
            target
        } else {
            &link.label
        };

        say(Slot::Link, words);
        self.previous = words.chars().next_back().unwrap_or(')');

        // Said once. A link written `[https://example.com](https://example.com)`
        // is the address twice, and so is one whose words a model copied out of
        // it.
        if !target.is_empty() && target != words {
            say(Slot::Quiet, " (");
            say(Slot::Quiet, target);
            say(Slot::Quiet, ")");
            self.previous = ')';
        }
    }

    /// Puts back whatever a link was holding, where the message ended in the
    /// middle of one.
    ///
    /// The last moment there is: the reader is dropped between messages, and
    /// text still held when that happens is text the reader never sees.
    pub fn finish(&mut self, say: &mut dyn FnMut(Slot, &str)) {
        if let Some(link) = self.link.take() {
            spill(&link, say);
        }
    }

    /// Ends the line, and with it everything markdown ends at one.
    fn end_line(&mut self, say: &mut dyn FnMut(Slot, &str)) {
        let ended = self.inside;
        // Belt and braces: the line break puts a link back before it gets here,
        // and the reset below would drop what one was holding without a sound.
        self.finish(say);

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
            syntax,
            glyphs: self.glyphs,
            ..Self::default()
        };

        // A fence's own line is a marker, and no marker is drawn — the line
        // break with it, or every block would arrive with a blank line stitched
        // to each end of it. After the reset otherwise, so the row that ends
        // carries no slot into the row that follows it.
        if !matches!(ended, Inside::Opening | Inside::Closing) {
            say(self.slot(), "\n");
        }
    }

    /// Hands on a run of text, if there is any.
    ///
    /// Inside a block being read, the run is held instead: a highlighter reads
    /// whole lines and this is a piece of one. Everywhere else it goes straight
    /// out, which is every run in every answer that is not code.
    fn say(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str)) {
        if text.is_empty() {
            return;
        }

        if self.syntax.is_some() && self.inside == Inside::Fence && self.code.len() < MOST {
            self.code.push_str(text);
            return;
        }

        let slot = self.slot();
        say(slot, text);
    }

    /// Hands on the line of code collected so far, read.
    ///
    /// Every byte comes back exactly once and in order, which is the property
    /// the whole thing rests on — a byte dropped is code that quietly changed
    /// meaning on screen, and a byte doubled is a row wider than it measured.
    fn read_code(&mut self, say: &mut dyn FnMut(Slot, &str)) {
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
                    say(slot, text);
                }
            }),
            None => say(Slot::Quiet, code.trim_end_matches('\n')),
        }
    }

    /// The slot everything read right now is written under.
    fn slot(&self) -> Slot {
        if self.inside != Inside::Prose || self.line.code || self.line.quoted {
            Slot::Quiet
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
fn spill(link: &Link, say: &mut dyn FnMut(Slot, &str)) {
    say(Slot::Plain, "[");
    if !link.label.is_empty() {
        say(Slot::Plain, &link.label);
    }
    if link.part != Part::Label {
        say(Slot::Plain, "]");
    }
    if link.part == Part::Target {
        say(Slot::Plain, "(");
        if !link.target.is_empty() {
            say(Slot::Plain, &link.target);
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
