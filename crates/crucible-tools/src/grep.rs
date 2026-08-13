//! Searching file contents.
//!
//! The walk and the search are ripgrep's own crates. That is a performance
//! decision before it is a convenience one — the budget is "within 1.25× of the
//! `rg` binary", and the only way to hold it on a real tree is to skip what `rg`
//! skips and read what it reads.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_core::{
    Approved, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace, WorkspacePath,
};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, MmapChoice, Searcher, SearcherBuilder};
use ignore::WalkState;
use ignore::overrides::{Override, OverrideBuilder};

use crate::target;

/// The name the model calls.
const NAME: &str = "grep";

/// How many matching lines one call answers with when it does not say.
const MATCHES: usize = 200;

/// The most lines a call can ask for, however large a number it sends.
///
/// [`MATCHES`] is a default and a caller may raise it; this it cannot. The
/// number arrives from the model like every other argument, and `"limit":
/// 100000` is a thing a model writes — where what it costs is not the answer,
/// which [`crate::bound`] cuts, but the walk that builds one: every hit is held
/// until the sort, at up to [`WIDTH`] characters each.
///
/// A thousand is where the two bounds meet. Even a match reported as
/// `src/a.rs:1:x` runs to some twenty bytes, so a thousand of them is already
/// the whole of what an answer carries — past this the bytes have cut it and a
/// larger number buys nothing but the work of finding lines nobody will read.
const CEILING: usize = 1_000;

/// Where a matching line is cut. A match inside a minified bundle is worth
/// reporting; the bundle is not worth sending.
const WIDTH: usize = 400;

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
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Searches the contents of files in the workspace for a regular expression. Skips anything gitignored.",
  "type": "object",
  "properties": {
    "pattern": {
      "type": "string",
      "description": "The regular expression to search for."
    },
    "path": {
      "type": "string",
      "description": "A file or directory to search, relative to the workspace root. Defaults to the whole workspace."
    },
    "glob": {
      "type": "string",
      "description": "Only search files whose path matches this glob, for example **/*.rs."
    },
    "ignore_case": {
      "type": "boolean",
      "description": "Match without regard to case. Defaults to false."
    },
    "limit": {
      "type": "integer",
      "minimum": 1,
      "description": "How many matching lines to return. Defaults to 200, and never more than 1000 however large a number is sent. The answer is cut at 30000 bytes as well, whichever comes first."
    }
  },
  "required": ["pattern"]
}"#;

/// Searches file contents inside the workspace.
#[derive(Debug)]
pub struct Grep {
    workspace: Workspace,
    cancel: Cancel,
}

/// What one call is looking for: the expression, the files it restricts itself
/// to, and how many lines are wanted back.
struct Query {
    matcher: RegexMatcher,
    only: Option<Override>,
    limit: usize,
}

/// One matching line.
struct Hit {
    /// Relative to the workspace root, so the model can pass it back to `read`.
    path: String,
    line: u64,
    text: String,
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
    partly: Vec<String>,
    /// Whether the user stopped the turn while the walk was running, which
    /// makes everything above a prefix of what the tree holds.
    stopped: bool,
}

/// What one file gave up: its matching lines, and its name if the search
/// stopped before the end of it.
struct Searched {
    hits: Vec<Hit>,
    partly: Option<String>,
}

impl Grep {
    /// Searches inside `workspace` and nowhere else, and stops when `cancel`
    /// says to.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self { workspace, cancel }
    }

    /// Walks `from` and searches every file the ignore rules keep and no rule
    /// refuses.
    ///
    /// The walk is parallel because the budget is measured against a tool that
    /// walks in parallel. Order therefore means nothing until the hits are
    /// sorted, which is what makes the same search answer the same way twice.
    fn hunt(&self, from: &WorkspacePath, query: Query, approved: &Approved) -> Found {
        let mut walk = crate::tree::walk(from.as_path());
        if let Some(only) = query.only {
            walk.overrides(only);
        }

        let (matcher, limit) = (&query.matcher, query.limit);
        let hits = Mutex::new(Vec::new());
        let partly = Mutex::new(BTreeSet::new());
        let so_far = AtomicUsize::new(0);

        walk.build_parallel().run(|| {
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
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
            let (hits, partly, so_far) = (&hits, &partly, &so_far);

            // `move` takes the searcher this thread just built. The shared
            // values are rebound above so it takes references to them rather
            // than the values themselves.
            Box::new(move |entry| {
                // Nothing below notices a flag on its own, and a walk is the
                // slowest thing this crate does: a tree with a million files in
                // it, or one somebody wrote to be walked slowly, is exactly
                // where Ctrl-C has to arrive. What was found before this point
                // is real and is reported, so stopping costs the turn nothing
                // it had already paid for.
                if self.cancel.requested() {
                    return WalkState::Quit;
                }

                // One past the limit, so what comes back can say whether it was
                // cut. Quitting *at* the limit leaves files unvisited and no way
                // to tell a complete answer from a truncated one.
                if so_far.load(Ordering::Relaxed) > limit {
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

                let mine = self.lines(&mut searcher, matcher, entry.path(), limit);
                if let Some(name) = mine.partly
                    && let Ok(mut partly) = partly.lock()
                {
                    partly.insert(name);
                }
                if !mine.hits.is_empty() {
                    so_far.fetch_add(mine.hits.len(), Ordering::Relaxed);
                    if let Ok(mut hits) = hits.lock() {
                        hits.extend(mine.hits);
                    }
                }

                WalkState::Continue
            })
        });

        let mut hits = hits.into_inner().unwrap_or_default();
        hits.sort_unstable_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));

        // Counted here, while the extra hit that proves there was more is still
        // in hand.
        let more = hits.len() > limit;
        hits.truncate(limit);
        let partly = partly
            .into_inner()
            .unwrap_or_default()
            .into_iter()
            .collect();
        Found {
            hits,
            more,
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
    /// because `UTF8` decodes before it hands anything over and only a NUL byte
    /// quits the searcher, so a Latin-1 file is searched as the text it nearly
    /// is until one of its bytes is not. Or a line is longer than [`MAX_LINE`],
    /// which the searcher reports as the allocation it was refused.
    ///
    /// Either way what came back before that point is real and is kept, and the
    /// name goes back with it. Dropping the lot is what makes a file holding a
    /// match on its first line answer as a file holding none.
    fn lines(
        &self,
        searcher: &mut Searcher,
        matcher: &RegexMatcher,
        path: &Path,
        limit: usize,
    ) -> Searched {
        // Spelled by the module that owns the walk, so `glob` cannot name the
        // same file a second way.
        let shown = crate::tree::named(&self.workspace, path);

        let mut hits = Vec::new();
        let mut halted = false;
        let found = searcher.search_path(
            matcher,
            path,
            UTF8(|line, text| {
                hits.push(Hit {
                    path: shown.clone(),
                    line,
                    text: cut(text.trim_end()),
                });
                if self.cancel.requested() {
                    halted = true;
                    return Ok(false);
                }
                // One past the limit for the same reason the walk goes one file
                // past it: the answer has to be able to say it was cut, and a
                // file stopped exactly at the limit cannot tell anyone whether
                // the next line matched.
                Ok(hits.len() <= limit)
            }),
        );

        Searched {
            partly: (found.is_err() || halted).then_some(shown),
            hits,
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
        SCHEMA
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: target::searched(&self.workspace, NAME, args, "path"),
        }
    }

    fn run(&self, approved: Approved) -> Result<ToolOutput, ToolError> {
        let args = crate::args::Args::parse(NAME, approved.args())?;
        let pattern = args.text("pattern")?;
        let limit = args.count("limit", MATCHES)?.min(CEILING);

        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(args.flag("ignore_case", false)?)
            .build(pattern);
        let Ok(matcher) = matcher else {
            return Ok(ToolOutput::failed(format!(
                "{pattern} is not a valid regular expression"
            )));
        };

        let requested = args.optional_text("path")?.unwrap_or(".");
        let from = match self.workspace.existing(requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        let Ok(only) = self.only(args.optional_text("glob")?) else {
            return Ok(ToolOutput::failed(format!(
                "{} is not a valid glob",
                args.optional_text("glob")?.unwrap_or_default()
            )));
        };

        let query = Query {
            matcher,
            only,
            limit,
        };
        Ok(report(&self.hunt(&from, query, &approved), pattern, limit))
    }
}

/// The hits, as lines the model can hand straight back to `read`.
fn report(found: &Found, pattern: &str, limit: usize) -> ToolOutput {
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
    let (lines, over) = crate::bound::within(
        found
            .hits
            .iter()
            .map(|hit| format!("{}:{}:{}\n", hit.path, hit.line, hit.text)),
    );

    let tail = if over > 0 {
        // Raising the limit would not help: what filled up is the answer, not
        // the list.
        format!(
            "\n[stopped at {} matches: the answer was full at {} bytes, narrow the pattern]",
            found.hits.len().saturating_sub(over),
            crate::bound::OUTPUT
        )
    } else if found.more {
        format!("\n[stopped at {limit} matches: narrow the pattern or raise limit]")
    } else {
        String::new()
    };

    ToolOutput::ok(lines + &tail + &note)
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
fn unread(partly: &[String]) -> String {
    if partly.is_empty() {
        return String::new();
    }

    let named = partly
        .iter()
        .take(NAMED)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let rest = partly.len().saturating_sub(NAMED);
    let more = if rest == 0 {
        String::new()
    } else {
        format!(" and {rest} more")
    };
    format!("\n[stopped partway through {named}{more}: a match below that point is not here]")
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
