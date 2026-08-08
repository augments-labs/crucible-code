//! Searching file contents.
//!
//! The walk and the search are ripgrep's own crates. That is a performance
//! decision before it is a convenience one — the budget is "within 1.25× of the
//! `rg` binary", and the only way to hold it on a real tree is to skip what `rg`
//! skips and read what it reads.

use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_core::{Grant, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::{Searcher, SearcherBuilder};
use ignore::WalkState;
use ignore::overrides::{Override, OverrideBuilder};

/// The name the model calls.
const NAME: &str = "grep";

/// How many matching lines one call answers with when it does not say.
const MATCHES: usize = 200;

/// Where a matching line is cut. A match inside a minified bundle is worth
/// reporting; the bundle is not worth sending.
const WIDTH: usize = 400;

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
      "description": "How many matching lines to return. Defaults to 200."
    }
  },
  "required": ["pattern"]
}"#;

/// Searches file contents inside the workspace.
#[derive(Debug)]
pub struct Grep {
    workspace: Workspace,
}

/// One matching line.
struct Hit {
    /// Relative to the workspace root, so the model can pass it back to `read`.
    path: String,
    line: u64,
    text: String,
}

impl Grep {
    /// Searches inside `workspace`, and nowhere else.
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }

    /// Walks `from` and searches every file the ignore rules keep.
    ///
    /// The walk is parallel because the budget is measured against a tool that
    /// walks in parallel. Order therefore means nothing until the hits are
    /// sorted, which is what makes the same search answer the same way twice.
    fn hunt(
        &self,
        from: &Path,
        matcher: &RegexMatcher,
        only: Option<Override>,
        limit: usize,
    ) -> Vec<Hit> {
        let mut walk = crate::tree::walk(from);
        if let Some(only) = only {
            walk.overrides(only);
        }

        let hits = Mutex::new(Vec::new());
        let found = AtomicUsize::new(0);

        walk.build_parallel().run(|| {
            let mut searcher = SearcherBuilder::new().line_number(true).build();
            let (hits, found) = (&hits, &found);

            // `move` takes the searcher this thread just built. The two shared
            // values are rebound above so it takes references to them rather
            // than the values themselves.
            Box::new(move |entry| {
                if found.load(Ordering::Relaxed) >= limit {
                    return WalkState::Quit;
                }

                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }

                let mine = self.lines(&mut searcher, matcher, entry.path(), limit);
                if !mine.is_empty() {
                    found.fetch_add(mine.len(), Ordering::Relaxed);
                    if let Ok(mut hits) = hits.lock() {
                        hits.extend(mine);
                    }
                }

                WalkState::Continue
            })
        });

        let mut hits = hits.into_inner().unwrap_or_default();
        hits.sort_unstable_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        hits.truncate(limit);
        hits
    }

    /// The matching lines of one file.
    ///
    /// A file that cannot be read — no permission, a broken link, a device —
    /// contributes nothing. Reporting it would fill the answer with entries the
    /// model asked nothing about and can do nothing with.
    fn lines(
        &self,
        searcher: &mut Searcher,
        matcher: &RegexMatcher,
        path: &Path,
        limit: usize,
    ) -> Vec<Hit> {
        let shown = path
            .strip_prefix(self.workspace.root())
            .unwrap_or(path)
            .display()
            .to_string();

        let mut hits = Vec::new();
        let found = searcher.search_path(
            matcher,
            path,
            UTF8(|line, text| {
                hits.push(Hit {
                    path: shown.clone(),
                    line,
                    text: cut(text.trim_end()),
                });
                Ok(hits.len() < limit)
            }),
        );

        // The sink itself never fails, so an error here is the file, not us.
        if found.is_err() { Vec::new() } else { hits }
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

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly
    }

    fn run(&self, args: ToolArgs, _grant: Grant) -> Result<ToolOutput, ToolError> {
        let args = crate::args::Args::parse(NAME, &args)?;
        let pattern = args.text("pattern")?;
        let limit = args.count("limit", MATCHES)?;

        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(args.flag("ignore_case", false)?)
            .build(pattern);
        let Ok(matcher) = matcher else {
            return Ok(ToolOutput::failed(format!(
                "{pattern} is not a valid regular expression"
            )));
        };

        let from = match args.optional_text("path")? {
            None => self.workspace.root().to_path_buf(),
            Some(requested) => match self.workspace.existing(requested) {
                Ok(path) => path.as_path().to_path_buf(),
                Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
            },
        };

        let Ok(only) = self.only(args.optional_text("glob")?) else {
            return Ok(ToolOutput::failed(format!(
                "{} is not a valid glob",
                args.optional_text("glob")?.unwrap_or_default()
            )));
        };

        Ok(report(
            &self.hunt(&from, &matcher, only, limit),
            pattern,
            limit,
        ))
    }
}

/// The hits, as lines the model can hand straight back to `read`.
fn report(hits: &[Hit], pattern: &str, limit: usize) -> ToolOutput {
    if hits.is_empty() {
        return ToolOutput::failed(format!("nothing matched {pattern}"));
    }

    let found = hits
        .iter()
        .map(|hit| format!("{}:{}:{}\n", hit.path, hit.line, hit.text))
        .collect::<Vec<_>>()
        .concat();

    let tail = if hits.len() >= limit {
        format!("\n[stopped at {limit} matches: narrow the pattern or raise limit]")
    } else {
        String::new()
    };

    ToolOutput::ok(found + &tail)
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
