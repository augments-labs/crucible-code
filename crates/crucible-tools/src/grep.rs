//! Searching file contents.
//!
//! The walk and the search are ripgrep's own crates. That is a performance
//! decision before it is a convenience one — the budget is "within 1.25× of the
//! `rg` binary", and the only way to hold it on a real tree is to skip what `rg`
//! skips and read what it reads.
//!
//! The walker does not follow links, and its entries are opened through the
//! workspace rather than handed back to `File::open` by name. Unix repeats the
//! descent against directory descriptors with no-follow at every step; Windows
//! validates the final path of the opened handle before a byte is read. A link
//! or directory swap between the walk and the open is therefore skipped rather
//! than becoming contents from outside the workspace. The ripgrep walker and
//! searcher remain intact around that boundary, which preserves the budget.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::io;
use std::str;
use std::sync::{LazyLock, Mutex};

use crucible_core::{
    Approved, Cancel, Sensitivity, Summary, Tool, ToolArgs, ToolError, ToolOutput, Watch,
    Workspace, WorkspacePath,
};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{
    BinaryDetection, MmapChoice, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch,
};
use ignore::WalkState;
use ignore::overrides::{Override, OverrideBuilder};

use crate::bound::OUTPUT;
use crate::schema::{Field, Schema, Shape, Whole};
use crate::summary;
use crate::target;

/// The name the model calls.
const NAME: &str = "grep";

/// What to look for.
const PATTERN: &str = "pattern";

/// Where to look.
const PATH: &str = "path";

/// Which files to look in.
const GLOB: &str = "glob";

/// Whether case matters.
const IGNORE_CASE: &str = "ignore_case";

/// Whether the pattern is exact text.
const FIXED: &str = "fixed";

/// Which shape of answer to give.
const MODE: &str = "mode";

/// How many lines to show around each match.
const CONTEXT: &str = "context";

/// How many results to give.
const LIMIT: &str = "limit";

/// How many results one call answers with when it does not say — matching
/// lines, or matching files where that is what was asked for.
const MATCHES: usize = 200;

/// The two answers a call can ask for, spelled as the model sends them.
const CONTENT: &str = "content";
const FILES: &str = "files";

/// The most lines a call can ask for, however large a number it sends.
///
/// [`MATCHES`] is a default and a caller may raise it; this it cannot. The
/// number arrives from the model like every other argument, and `"limit":
/// 100000` is a thing a model writes. The walk retains only the lowest `limit`
/// hits, but each may still carry up to [`WIDTH`] characters and the final
/// answer is smaller than an arbitrarily large requested set.
///
/// A thousand is where the two bounds meet. Even a match reported as
/// `src/a.rs:1:x` runs to some twenty bytes, so a thousand of them is already
/// the whole of what an answer carries — past this the bytes have cut it and a
/// larger number buys nothing but the work of finding lines nobody will read.
const CEILING: usize = 1_000;

/// Where a matching line is cut. A match inside a minified bundle is worth
/// reporting; the bundle is not worth sending.
const WIDTH: usize = 400;

/// The most lines either side of a match a call can ask for.
///
/// Capped the way `limit` is and for the same reason: the number arrives from
/// the model. Twenty is where the answer stops being about the match — a call
/// asking for forty lines around each of two hundred hits has asked for the
/// files themselves, and `read` is the tool that hands over a file. The bytes
/// bound the answer either way, so a larger number here would buy the walk the
/// work of collecting lines the answer then cuts.
const REACH: usize = 20;

/// How many files the answer names before it starts counting them instead.
const NAMED: usize = 5;

/// The most heap one searcher may hold a line in.
///
/// The searcher scans a line at a time, so its buffer has to grow to the longest
/// line in the file. Left unbounded that is the default, and the default is
/// "limited only by available memory": a committed minified bundle — one ASCII
/// line and no newline, the case [`WIDTH`] is cut for — is read whole. Nothing
/// else stops it. `MmapChoice::never` moves those bytes onto the heap rather
/// than avoiding them, and `BinaryDetection::quit` never fires on a long line of
/// text. Measured, a 200 MB bundle takes the process to 413 MB, which against
/// the 35 MB this program is allowed is not a slow search but a dead one.
///
/// A quarter of a megabyte, and the arithmetic is over the whole walk rather
/// than one file: the walk is parallel, one searcher is built per thread, and
/// `ignore` takes a thread per core up to twelve. So the worst case is 3 MB — a
/// fixed cost under a tenth of the budget, which does not move with the size of
/// the tree, the number of files or the length of the session.
///
/// Generous against a real line for the same reason it is small against the
/// budget. A matching line is cut at [`WIDTH`] characters before anyone sees it,
/// so this is some six hundred times what is ever reported, and five times the
/// longest line in a machine-written source file in this program's own
/// dependency tree. A file with a line longer than this is searched as far as
/// that line and then named in the note [`unread`] writes, which is what the
/// model needs to know to go and look for itself.
const MAX_LINE: usize = 256 * 1024;

/// The root `description` is the tool's own; everything below it describes the
/// arguments. Every ceiling is spelled by the constant the code holds it
/// with, so the sentence the model reads cannot drift from the bound the call
/// meets.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Searches the contents of files in the workspace for a regular expression, or for \
                exact text with fixed. Skips anything gitignored."
            .into(),
        fields: vec![
            Field {
                name: PATTERN,
                about: "The regular expression to search for, or the exact text to find if fixed \
                        is true."
                    .into(),
                needed: true,
                shape: Shape::Text,
            },
            Field {
                name: PATH,
                about: "A file or directory to search, relative to the workspace root. Defaults \
                        to the whole workspace."
                    .into(),
                needed: false,
                shape: Shape::Text,
            },
            Field {
                name: GLOB,
                about: "Only search files whose path matches this glob, for example **/*.rs."
                    .into(),
                needed: false,
                shape: Shape::Text,
            },
            Field {
                name: IGNORE_CASE,
                about: "Match without regard to case. Defaults to false.".into(),
                needed: false,
                shape: Shape::Flag,
            },
            Field {
                name: FIXED,
                about: "Read pattern as the exact text to find rather than as a regular \
                        expression, so characters like . ( [ * ? and | stand for themselves. Use \
                        this for anything copied out of a file. Defaults to false."
                    .into(),
                needed: false,
                shape: Shape::Flag,
            },
            Field {
                name: MODE,
                about: format!(
                    "What to answer with: {CONTENT} for the matching lines themselves, {FILES} \
                     for the name of every file holding one. Defaults to {CONTENT}."
                ),
                needed: false,
                shape: Shape::Choice(&[CONTENT, FILES]),
            },
            Field {
                name: CONTEXT,
                about: format!(
                    "How many lines to return either side of each match, the way grep -C does. \
                     Context lines are marked with dashes instead of colons and do not count \
                     towards limit. Defaults to 0, and never more than {REACH} however large a \
                     number is sent. Only {CONTENT} mode has lines to surround, so {FILES} mode \
                     ignores it."
                ),
                needed: false,
                shape: Shape::Count(Whole {
                    least: 0,
                    most: Some(REACH),
                }),
            },
            Field {
                name: LIMIT,
                about: format!(
                    "How many results to return, counting matching lines in {CONTENT} mode and \
                     matching files in {FILES} mode. Defaults to {MATCHES}, and never more than \
                     {CEILING} however large a number is sent. The answer is cut at {OUTPUT} \
                     bytes as well, whichever comes first."
                ),
                needed: false,
                shape: Shape::Count(Whole {
                    least: 1,
                    most: Some(CEILING),
                }),
            },
        ],
    }
    .text()
});

/// Searches file contents inside the workspace.
#[derive(Debug)]
pub struct Grep {
    workspace: Workspace,
    cancel: Cancel,
}

/// What one call is looking for: the pattern, the files it restricts itself
/// to, what shape of answer it wants and how much of it.
struct Query {
    matcher: RegexMatcher,
    only: Option<Override>,
    mode: Mode,
    /// Lines either side of each match. Zero is the plain search.
    context: usize,
    limit: usize,
}

/// What a call wants back.
///
/// The difference is not only in the formatting. A files answer is complete
/// once a file has matched at all, so the search of that file stops there —
/// which makes this the cheaper of the two on exactly the searches where a
/// pattern is common enough that reading every line of every hit would be the
/// expensive part.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    /// The matching lines themselves.
    Content,
    /// The name of every file holding one.
    Files,
}

impl Mode {
    /// What this mode's answer is counted in, for the notes that say it was
    /// cut. The bound is one number and it bounds two different things.
    fn counted(self) -> &'static str {
        match self {
            Self::Content => "matches",
            Self::Files => "files",
        }
    }
}

/// One line of the answer: a matching line, or one the call asked for around
/// it.
///
/// Ordered by path and then line, which is the order the answer is read in and
/// the order it has to come back in twice. `matched` sits last and decides
/// nothing: the searcher reports a line once, as a match or as context, so no
/// two hits ever share a path and a line.
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Hit {
    /// Relative to the workspace root, so the model can pass it back to `read`.
    path: String,
    line: u64,
    text: String,
    /// Whether the pattern hit this line, as against this being one of the
    /// lines around one it hit. The bound is on matches, and the answer marks
    /// the two differently.
    matched: bool,
}

/// The lowest matching lines with whatever a call asked for around them, and
/// the total seen. All of it globally bounded.
///
/// Workers add directly here instead of building one `limit`-sized vector per
/// thread. Keeping the lowest ordered set while a complete walk searches every
/// file makes that result independent of worker scheduling without retaining
/// the tree. A cancelled walk reports only what it reached.
struct Top {
    /// The matching lines, and the lines around them where a call asked for
    /// those.
    hits: BTreeSet<Hit>,
    /// Matching lines kept. Counted rather than read off `hits.len()`, which
    /// is the two together.
    kept: usize,
    /// Matching lines seen, which is what says whether more matched than the
    /// answer carries.
    total: usize,
    limit: usize,
    /// How far a match reaches, so a context line above the highest match kept
    /// can be told from one the answer still holds.
    context: u64,
}

impl Top {
    fn new(limit: usize, context: u64) -> Self {
        Self {
            hits: BTreeSet::new(),
            kept: 0,
            total: 0,
            limit,
            context,
        }
    }

    fn add(&mut self, hit: Hit) {
        let matched = hit.matched;
        if matched {
            self.total = self.total.saturating_add(1);
        }
        if self.hits.insert(hit) && matched {
            self.kept = self.kept.saturating_add(1);
        }

        while self.kept > self.limit {
            let Some(gone) = self.hits.pop_last() else {
                break;
            };
            if gone.matched {
                self.kept -= 1;
            }
        }

        // Only once the set is full. Below that every context line is one this
        // is going to keep, and the match it belongs to may not have arrived
        // yet — the searcher reports the lines before a match before the match
        // itself.
        if self.kept >= self.limit {
            self.trim();
        }
    }

    /// Drops the context left above the highest match kept.
    ///
    /// A context line is only ever reported beside a match, so one no kept
    /// match reaches belonged to a match the bound cut. They can only be at the
    /// top: nothing below the highest match is ever dropped, so what is left
    /// there stays whole.
    fn trim(&mut self) {
        let over = {
            let Some(top) = self.hits.iter().rev().find(|hit| hit.matched) else {
                // Nothing matched, so nothing here has anything to be beside.
                self.hits.clear();
                return;
            };
            let reach = top.line.saturating_add(self.context);
            self.hits
                .iter()
                .rev()
                .take_while(|hit| !hit.matched && (hit.path != top.path || hit.line > reach))
                .count()
        };

        for _ in 0..over {
            self.hits.pop_last();
        }
    }
}

/// What one search came back with.
struct Found {
    hits: Vec<Hit>,
    /// Whether more matched than the answer carries. Read off a count taken
    /// before the truncation, because afterwards `hits.len() == limit` is what
    /// "exactly this many exist" and "more exist and were cut" both look like —
    /// and telling the model to narrow a pattern that needed no narrowing costs
    /// it a turn to rediscover the same lines.
    more: bool,
    /// The files the search did not get to the end of, named so the model can
    /// go and look for itself. Sorted, because the walk is parallel and the
    /// same search has to answer the same way twice.
    partly: Partial,
    /// Whether the user stopped the turn while the walk was running, which
    /// makes everything above only what the completed portion of the walk held.
    stopped: bool,
}

/// A fixed-size account of files a search did not finish.
///
/// The total is separate from the names because an unreadable tree is input,
/// not permission to retain one allocation per file. Only the first names in
/// sorted order are useful in the answer; the rest are represented by the
/// count the answer already reports.
#[derive(Default)]
struct Partial {
    names: BTreeSet<String>,
    total: usize,
}

impl Partial {
    fn add(&mut self, name: String) {
        self.total = self.total.saturating_add(1);
        self.names.insert(name);
        if self.names.len() > NAMED {
            self.names.pop_last();
        }
    }
}

/// What one file gave up: its matching lines, and its name if the search
/// stopped before the end of it.
struct Searched {
    partly: Option<String>,
}

/// Where one file's lines go as the searcher reports them.
///
/// `grep-searcher` offers a sink that wraps a closure over matching lines and
/// leaves the rest of what a search reports at a default that drops it. The
/// lines around a match are that rest: they arrive by their own method, and
/// which of the two a line arrived by is what the answer marks.
struct Kept<'a> {
    /// The file's name as the answer gives it.
    shown: &'a str,
    mode: Mode,
    hits: &'a Mutex<Top>,
    cancel: &'a Cancel,
    /// Set when cancellation ended this file, so it is named as one the search
    /// did not reach the end of.
    halted: &'a Cell<bool>,
}

impl Kept<'_> {
    /// One line, whichever of the two ways it arrived.
    fn take(&self, line: Option<u64>, bytes: &[u8], matched: bool) -> Result<bool, io::Error> {
        if self.cancel.requested() {
            self.halted.set(true);
            return Ok(false);
        }

        // Decoded before anything is kept: a line the matcher hit is not
        // necessarily text, and the file it came from is named rather than
        // reported as searched.
        let text = str::from_utf8(bytes).map_err(io::Error::other)?;
        let Some(line) = line else {
            // The searcher is built to count lines, so this does not happen. A
            // hit with no number is one the model cannot hand back to `read`,
            // which is worth ending the file over rather than guessing at.
            return Err(io::Error::other(
                "the search reported a line with no number",
            ));
        };

        if let Ok(mut hits) = self.hits.lock() {
            hits.add(Hit {
                path: self.shown.to_owned(),
                line,
                text: cut(text.trim_end()),
                matched,
            });
        }

        Ok(self.mode == Mode::Content)
    }
}

impl Sink for Kept<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, hit: &SinkMatch<'_>) -> Result<bool, io::Error> {
        self.take(hit.line_number(), hit.bytes(), true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        around: &SinkContext<'_>,
    ) -> Result<bool, io::Error> {
        self.take(around.line_number(), around.bytes(), false)
    }
}

/// A file reader that turns cancellation into a clean, marked end of input.
struct Stopping<'a> {
    file: &'a std::fs::File,
    cancel: &'a Cancel,
    stopped: &'a Cell<bool>,
}

impl io::Read for Stopping<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.requested() {
            self.stopped.set(true);
            return Ok(0);
        }
        io::Read::read(&mut self.file, buffer)
    }
}

impl Grep {
    /// Searches inside `workspace` and nowhere else, and stops when `cancel`
    /// says to.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self { workspace, cancel }
    }

    /// Walks `from` and, until cancellation, searches every file the ignore
    /// rules keep and no rule refuses.
    ///
    /// The walk is parallel because the budget is measured against a tool that
    /// walks in parallel. Workers retain the globally lowest ordered hits, so
    /// scheduling does not choose which matches a full answer contains.
    fn hunt(&self, from: &WorkspacePath, query: Query, approved: &Approved) -> Found {
        let mut walk = crate::tree::walk(from.as_path());
        if let Some(only) = query.only {
            walk.overrides(only);
        }

        let (matcher, mode, limit) = (&query.matcher, query.mode, query.limit);
        // A files answer is a list of names, which has nothing for a line
        // beside a match to go next to. So the searcher is not asked for them,
        // and the walk does not read what the answer could not carry.
        let context = match mode {
            Mode::Content => query.context,
            Mode::Files => 0,
        };
        let hits = Mutex::new(Top::new(limit, reach(context)));
        let partly = Mutex::new(Partial::default());

        walk.build_parallel().run(|| {
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
                .before_context(context)
                .after_context(context)
                // What `rg` does by default, and the budget is measured against
                // `rg`. A checked-in font, `.so` or fixture that happens to be
                // valid UTF-8 is otherwise searched byte for byte, and its
                // "lines" go back to the model with NUL bytes inside them.
                .binary_detection(BinaryDetection::quit(b'\x00'))
                // Said out loud rather than left to the default, because the
                // default is the answer here and a later reader would otherwise
                // read the silence as an oversight: `MmapChoice::auto` is an
                // `unsafe` call — a file truncated under a live map is a SIGBUS
                // — and this workspace denies `unsafe`. What holds the budget is
                // the walk and the ignore rules, which is where `rg` wins most
                // of it too.
                .memory_map(MmapChoice::never())
                // Which is also why this has to be said: with no map, a line is
                // held on the heap, and with no limit the heap is the machine.
                .heap_limit(Some(MAX_LINE))
                .build();
            let mut files = from.walk_files();
            let (hits, partly) = (&hits, &partly);

            // `move` takes the searcher this thread just built. The shared
            // values are rebound above so it takes references to them rather
            // than the values themselves.
            Box::new(move |entry| {
                // Nothing below notices a flag on its own, and a walk is the
                // slowest thing this crate does: a tree with a million files in
                // it, or one somebody wrote to be walked slowly, is exactly
                // where Esc has to arrive. What was found before this point
                // is real and is reported, so stopping costs the turn nothing
                // it had already paid for.
                if self.cancel.requested() {
                    return WalkState::Quit;
                }

                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }

                // The call was settled about the directory. This file is one
                // nobody was asked about, so it is asked about here, before it
                // is opened rather than after its lines are in hand.
                if approved.denies(&self.workspace, from, entry.path()) {
                    return WalkState::Continue;
                }

                let Ok(Some((path, file))) = files.open_regular(entry.path()) else {
                    return WalkState::Continue;
                };
                let mine = self.lines(&mut searcher, matcher, (&path, &file), (mode, hits));
                if let Some(name) = mine.partly
                    && let Ok(mut partly) = partly.lock()
                {
                    partly.add(name);
                }
                WalkState::Continue
            })
        });

        let mut top = hits
            .into_inner()
            .unwrap_or_else(|_| Top::new(limit, reach(context)));
        // Once more with the walk over, because a match no later match can
        // arrive above is the last word on which context lines anything holds.
        top.trim();
        let partly = partly.into_inner().unwrap_or_default();
        Found {
            more: top.total > limit,
            hits: top.hits.into_iter().collect(),
            partly,
            // Read after the walk rather than recorded inside it: a request that
            // arrived is what makes this answer a prefix, whether the walk was
            // still running when it landed or had just finished.
            stopped: self.cancel.requested(),
        }
    }

    /// The matching lines of one file, and whether the search reached the end
    /// of it.
    ///
    /// Four things stop it early. The user stopped the turn, which the walk
    /// answers between files and this answers inside one — a single file can be
    /// the size of a tree, and a search of it is the wait a stopped turn would
    /// otherwise sit through. The file cannot be read — no permission, a
    /// device, gone since the walk saw it. A line the matcher hit is not text,
    /// because [`Kept`] decodes before it keeps anything and only a NUL byte
    /// quits the searcher, so a Latin-1 file is searched as the text it nearly
    /// is until one of its bytes is not. Or a line is longer than [`MAX_LINE`],
    /// which the searcher reports as the allocation it was refused.
    ///
    /// Either way what came back before that point is real and is kept, and the
    /// name goes back with it. Dropping the lot is what makes a file holding a
    /// match on its first line answer as a file holding none.
    ///
    /// A fifth thing stops it early and is none of those: a files answer is
    /// finished with a file once it has matched, so [`Mode::Files`] leaves after
    /// the first line. That one is not an unfinished search and is not reported
    /// as one — the file is already in the answer, and nothing further down it
    /// could be missing from a list of names.
    fn lines(
        &self,
        searcher: &mut Searcher,
        matcher: &RegexMatcher,
        opened: (&WorkspacePath, &std::fs::File),
        wanted: (Mode, &Mutex<Top>),
    ) -> Searched {
        let (path, file) = opened;
        let (mode, hits) = wanted;
        // Spelled by the module that owns the walk, so `glob` cannot name the
        // same file a second way.
        let shown = crate::tree::named(&self.workspace, path.as_path());

        let halted = Cell::new(false);
        let reader = Stopping {
            file,
            cancel: &self.cancel,
            stopped: &halted,
        };
        let found = searcher.search_reader(
            matcher,
            reader,
            Kept {
                shown: &shown,
                mode,
                hits,
                cancel: &self.cancel,
                halted: &halted,
            },
        );

        Searched {
            partly: (found.is_err() || halted.get()).then_some(shown),
        }
    }

    /// The glob a call restricts itself to, if it gave one.
    fn only(&self, glob: Option<&str>) -> Result<Option<Override>, io::Error> {
        let Some(glob) = glob else {
            return Ok(None);
        };

        let mut only = OverrideBuilder::new(self.workspace.root());
        only.add(glob)
            .and_then(|builder| builder.build())
            .map(Some)
            .map_err(|problem| io::Error::new(io::ErrorKind::InvalidInput, problem.to_string()))
    }
}

impl Tool for Grep {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA.as_str()
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        target::searches(&self.workspace, NAME, args, PATH)
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, PATTERN)
    }

    fn run(&self, approved: Approved, _watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = crate::args::Args::parse(NAME, approved.args())?;
        let pattern = args.text(PATTERN)?;
        let limit = args.count(LIMIT, MATCHES)?.min(CEILING);

        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(args.flag(IGNORE_CASE, false)?)
            .fixed_strings(args.flag(FIXED, false)?)
            .build(pattern);
        let Ok(matcher) = matcher else {
            return Ok(ToolOutput::failed(format!(
                "{pattern} is not a valid regular expression"
            )));
        };

        let requested = args.optional_text(PATH)?.unwrap_or(".");
        // A directory outside the workspace is walked only on the say-so the
        // `Approved` in hand carries.
        let from = match crate::target::opened(&self.workspace, &approved, requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem)),
        };

        let Ok(only) = self.only(args.optional_text(GLOB)?) else {
            return Ok(ToolOutput::failed(format!(
                "{} is not a valid glob",
                args.optional_text(GLOB)?.unwrap_or_default()
            )));
        };

        let mode = match args.choice(MODE, CONTENT, &[CONTENT, FILES])? {
            FILES => Mode::Files,
            _ => Mode::Content,
        };

        let query = Query {
            matcher,
            only,
            mode,
            context: args.whole(CONTEXT, 0)?.min(REACH),
            limit,
        };
        let found = self.hunt(&from, query, &approved);
        Ok(report(&found, pattern, (mode, limit)))
    }
}

/// The hits, as lines the model can hand straight back to `read`.
fn report(found: &Found, pattern: &str, answer: (Mode, usize)) -> ToolOutput {
    let (mode, limit) = answer;
    let note = unread(&found.partly) + halted(found.stopped);

    if found.hits.is_empty() {
        // The note belongs on this branch too. "Nothing matched" is a claim
        // about files that were read to the end, and one the search stopped
        // partway through is not among them.
        return ToolOutput::failed(format!("nothing matched {pattern}{note}"));
    }

    // Bounded in bytes here rather than in lines above, because the two are not
    // the same promise: the lines are cut at `WIDTH` characters each, so the
    // count a call sets says how many lines come back and nothing about how
    // much text that is.
    let (lines, over) = crate::bound::within(found.hits.iter().map(|hit| match mode {
        // Dashes where a match has colons, which is how `grep -C` has told the
        // two apart since before there were models to read it. The line number
        // is on both, so a gap between groups is visible in the numbers.
        Mode::Content if !hit.matched => format!("{}-{}-{}\n", hit.path, hit.line, hit.text),
        Mode::Content => format!("{}:{}:{}\n", hit.path, hit.line, hit.text),
        Mode::Files => format!("{}\n", hit.path),
    }));

    let counted = mode.counted();
    let tail = if over > 0 {
        // Raising the limit would not help: what filled up is the answer, not
        // the list. Counted in matches rather than in lines, because that is
        // what the number beside it means and context lines are neither.
        format!(
            "\n[stopped at {} {counted}: the answer was full at {} bytes, narrow the pattern]",
            matches(&found.hits, found.hits.len().saturating_sub(over)),
            crate::bound::OUTPUT
        )
    } else if found.more {
        format!("\n[showing first {limit} {counted}: narrow the pattern or raise limit]")
    } else {
        String::new()
    };

    ToolOutput::ok(lines + &tail + &note)
}

/// How many of the first `shown` hits matched, as against surrounding one that
/// did.
fn matches(hits: &[Hit], shown: usize) -> usize {
    hits.iter().take(shown).filter(|hit| hit.matched).count()
}

/// What a search the user stopped says about itself.
///
/// It answers rather than failing. Half a tree searched is half a tree
/// searched, and the lines it found are what the turn was spent on — but an
/// answer that stops early looks exactly like one that finished, so the
/// difference has to be in the text.
fn halted(stopped: bool) -> &'static str {
    if stopped {
        "\n[stopped before the walk finished: a match in a file it did not reach is not here]"
    } else {
        ""
    }
}

/// The files the search stopped partway through, said out loud.
///
/// An answer that is missing the bottom of a file looks exactly like one that
/// read the whole thing, so the difference has to be in the text. Bounded like
/// every other part of the output: a tree of files the searcher cannot decode
/// would otherwise put a line in the answer for each of them.
fn unread(partly: &Partial) -> String {
    if partly.total == 0 {
        return String::new();
    }

    let named = partly
        .names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let rest = partly.total.saturating_sub(partly.names.len());
    let more = if rest == 0 {
        String::new()
    } else {
        format!(" and {rest} more")
    };
    format!("\n[stopped partway through {named}{more}: a match below that point is not here]")
}

/// How far a match reaches, counted the way a line number is.
///
/// [`REACH`] caps the argument well below where this could saturate; it is
/// written as a conversion rather than a cast because the two types are the
/// walk's and the searcher's, and neither is free to change the other.
fn reach(context: usize) -> u64 {
    u64::try_from(context).unwrap_or(u64::MAX)
}

/// A line, cut on a character boundary if it is longer than anything worth
/// sending.
fn cut(line: &str) -> String {
    match line.char_indices().nth(WIDTH) {
        Some((at, _)) => format!("{}…", line.get(..at).unwrap_or(line)),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests;
