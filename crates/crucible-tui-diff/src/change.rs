//! What a change to files is, however it arrived: files, each with its path,
//! its counts and the lines a reader is shown.
//!
//! Built two ways. [`crate::read()`] parses a unified diff into one, and a tool
//! that edited a file hands its lines over through [`File::numbered`], numbered
//! as a tool numbers them: a removed line by where it was, every other line by
//! where it is now. Both are held to the same bounds here, because a `File` is
//! public and its builder is the one door its lines come through.

/// The most rows a file draws whole. One more and it folds.
pub const FOLD: usize = 32;

/// The rows a folded file draws before the row that counts the rest.
pub const HEAD: usize = 24;

/// The rows a folded file draws after the row that counts the rest.
pub const TAIL: usize = 4;

// A folded file draws fewer rows than one drawn whole, with room for its count.
const _: () = assert!(HEAD + TAIL < FOLD);

/// The most lines one file keeps. Past it a line is counted as dropped.
pub const MOST_LINES: usize = 2048;

/// The most files one change keeps. Past it a file is counted as not shown,
/// and a diff that names more is not read at all.
pub const MOST_FILES: usize = 8;

/// The most bytes of a path or a section name kept: a path keeps its end,
/// where the file's name is, and a section name its start.
pub const MOST_PATH: usize = 1024;

/// The character that stands where a path held one that could reorder,
/// hide or move what is drawn around it.
const REPLACED: char = '\u{FFFD}';

/// Which way a line went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// It is in the file before and after.
    Kept,
    /// It was in the file and is not now.
    Removed,
    /// It is in the file and was not before.
    Added,
}

/// One row of a file's body.
#[derive(Clone, PartialEq, Eq)]
pub enum Line {
    /// A line of the file, with the number a reader would find it at.
    Text {
        /// Which way it went.
        side: Side,
        /// Where it was, for a removed line; where it is, for any other.
        number: usize,
        /// What it says.
        text: String,
    },
    /// Lines of the file the change never held, between two that it did.
    Gap {
        /// How many.
        lines: usize,
        /// The section a diff named them in (`fn parse`), where it named one.
        section: Option<String>,
    },
}

/// One file a change touched.
#[derive(Clone, PartialEq, Eq)]
pub struct File {
    path: Option<String>,
    from: Option<String>,
    lines: Vec<Line>,
    added: usize,
    removed: usize,
    dropped: usize,
    binary: bool,
}

impl std::fmt::Debug for Line {
    /// By hand: a line is a file's contents, and a file can hold a credential,
    /// so its text and a section's name are said to be there and never shown.
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text { side, number, text } => into
                .debug_struct("Text")
                .field("side", side)
                .field("number", number)
                .field("bytes", &text.len())
                .finish(),
            Self::Gap { lines, section } => into
                .debug_struct("Gap")
                .field("lines", lines)
                .field("section", &section.as_ref().map(|_| "[redacted]"))
                .finish(),
        }
    }
}

impl std::fmt::Debug for File {
    /// By hand: a path can name a credential and the lines are the file's
    /// contents, so the paths are said to be there and the lines counted.
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        into.debug_struct("File")
            .field("path", &self.path.as_ref().map(|_| "[redacted]"))
            .field("from", &self.from.as_ref().map(|_| "[redacted]"))
            .field("lines", &self.lines.len())
            .field("added", &self.added)
            .field("removed", &self.removed)
            .field("dropped", &self.dropped)
            .field("binary", &self.binary)
            .finish()
    }
}

impl File {
    /// A file with nothing in it yet, at `path` where one is known.
    #[must_use]
    pub fn new(path: Option<&str>) -> Self {
        Self {
            path: path.map(safe_path),
            from: None,
            lines: Vec::new(),
            added: 0,
            removed: 0,
            dropped: 0,
            binary: false,
        }
    }

    /// A file from the lines a tool kept of its edit, numbered as a tool
    /// numbers them.
    ///
    /// Where a line that is still in the file jumps past the one after the
    /// last such line, the lines between are a [`Line::Gap`] placed straight
    /// after that last line, so the removed lines that came before the jump
    /// sit under it.
    #[must_use]
    pub fn numbered<'a>(
        path: Option<&str>,
        lines: impl IntoIterator<Item = (usize, Side, &'a str)>,
    ) -> Self {
        let mut file = Self::new(path);
        // Where the last line still in the file is, and the index just past
        // it, where a gap before the next one goes.
        let mut last: Option<(usize, usize)> = None;

        for (number, side, text) in lines {
            if side != Side::Removed
                && let Some((at, after)) = last
                && number > at.saturating_add(1)
            {
                file.insert(
                    after,
                    Line::Gap {
                        lines: number - at - 1,
                        section: None,
                    },
                );
            }
            file.push(Line::Text {
                side,
                number,
                text: text.to_owned(),
            });
            if side != Side::Removed {
                last = Some((number, file.lines.len()));
            }
        }

        file
    }

    /// The same file, counted as `added` and `removed` lines whatever its
    /// rows say: what a tool counted is the whole of the edit, and the rows
    /// are only what it kept.
    #[must_use]
    pub const fn counted(mut self, added: usize, removed: usize) -> Self {
        self.added = added;
        self.removed = removed;
        self
    }

    /// The same file with `lines` more that the change never kept.
    #[must_use]
    pub const fn dropping(mut self, lines: usize) -> Self {
        self.dropped = self.dropped.saturating_add(lines);
        self
    }

    /// Appends one row, counting it. Past [`MOST_LINES`] it is counted as
    /// dropped instead of kept.
    pub fn push(&mut self, line: Line) {
        if let Line::Text { side, .. } = &line {
            match side {
                Side::Added => self.added = self.added.saturating_add(1),
                Side::Removed => self.removed = self.removed.saturating_add(1),
                Side::Kept => {}
            }
        }
        self.keep(line, None);
    }

    /// Puts a row at `at` without counting it; a gap counts nothing.
    fn insert(&mut self, at: usize, line: Line) {
        self.keep(line, Some(at));
    }

    fn keep(&mut self, line: Line, at: Option<usize>) {
        if self.lines.len() >= MOST_LINES {
            if matches!(line, Line::Text { .. }) {
                self.dropped = self.dropped.saturating_add(1);
            }
            return;
        }
        let line = match line {
            Line::Gap { lines, section } => Line::Gap {
                lines,
                section: section.map(|section| safe(&section)),
            },
            text @ Line::Text { .. } => text,
        };
        match at {
            Some(at) if at <= self.lines.len() => self.lines.insert(at, line),
            _ => self.lines.push(line),
        }
    }

    /// Says the file was renamed from `from`.
    pub(crate) fn renamed(&mut self, from: &str) {
        self.from = Some(safe_path(from));
    }

    /// Names the file `path`.
    pub(crate) fn named(&mut self, path: &str) {
        self.path = Some(safe_path(path));
    }

    /// Says the file's contents are not text.
    pub(crate) const fn binary(&mut self) {
        self.binary = true;
    }

    /// Where the file is, drawn safe; `None` where nothing said.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Where the file was before a rename, drawn safe.
    #[must_use]
    pub fn from(&self) -> Option<&str> {
        self.from.as_deref()
    }

    /// Every row, folded or not.
    #[must_use]
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Lines the change added.
    #[must_use]
    pub const fn added(&self) -> usize {
        self.added
    }

    /// Lines the change removed.
    #[must_use]
    pub const fn removed(&self) -> usize {
        self.removed
    }

    /// Lines the change moved but never kept.
    #[must_use]
    pub const fn dropped(&self) -> usize {
        self.dropped
    }

    /// Whether the file's contents are not text.
    #[must_use]
    pub const fn is_binary(&self) -> bool {
        self.binary
    }

    /// Whether the file has anything to draw under its header.
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty() && !self.binary
    }
}

/// A change to files: the files, in the order they were changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    files: Vec<File>,
    unshown: usize,
}

impl Change {
    /// A change of `files`. Past [`MOST_FILES`] the rest are not kept, and
    /// are counted so the box can say how many it leaves out.
    #[must_use]
    pub fn new(mut files: Vec<File>) -> Self {
        let unshown = files.len().saturating_sub(MOST_FILES);
        files.truncate(MOST_FILES);
        Self { files, unshown }
    }

    /// The files it touched, as far as [`MOST_FILES`].
    #[must_use]
    pub fn files(&self) -> &[File] {
        &self.files
    }

    /// The files it touched past [`MOST_FILES`], which are not kept.
    #[must_use]
    pub const fn unshown(&self) -> usize {
        self.unshown
    }
}

/// `text` as a section name may be kept: its first [`MOST_PATH`] bytes, with
/// every character that could move, hide or reorder what is drawn around it
/// replaced by one that says something was there.
pub(crate) fn safe(text: &str) -> String {
    let mut kept = String::with_capacity(text.len().min(MOST_PATH));
    for character in text.chars().map(shown) {
        if kept.len() + character.len_utf8() > MOST_PATH {
            break;
        }
        kept.push(character);
    }
    kept
}

/// `path` as it may be kept: made safe as [`safe`] makes a section name, but
/// bounded from its start, so the file's own name, which is at its end and
/// which a narrow header keeps, is what survives a path past the bound.
pub(crate) fn safe_path(path: &str) -> String {
    let mut kept = Vec::new();
    let mut bytes = 0_usize;
    for character in path.chars().rev().map(shown) {
        if bytes + character.len_utf8() > MOST_PATH {
            break;
        }
        bytes += character.len_utf8();
        kept.push(character);
    }
    kept.iter().rev().collect()
}

/// `character`, or the one drawn in its place when it must not be drawn.
const fn shown(character: char) -> char {
    if hidden(character) {
        REPLACED
    } else {
        character
    }
}

/// Whether `character` is one a path must not draw: a control character, one
/// that reorders text around it, or one no font agrees on.
const fn hidden(character: char) -> bool {
    matches!(
        character,
        '\u{0}'..='\u{1F}'
            | '\u{7F}'..='\u{9F}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{E000}'..='\u{F8FF}'
            | '\u{F0000}'..='\u{10FFFF}'
    )
}
