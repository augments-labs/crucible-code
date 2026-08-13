//! Finding files by the shape of their path.

use std::collections::BinaryHeap;

use crucible_core::{Approved, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};
use globset::GlobBuilder;

use crate::args::Args;
use crate::target;

/// The name the model calls.
const NAME: &str = "glob";

/// How many paths one call answers with when it does not say.
const PATHS: usize = 200;

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Lists files in the workspace whose path matches a glob. Skips anything gitignored.",
  "type": "object",
  "properties": {
    "pattern": {
      "type": "string",
      "description": "The glob to match, for example **/*.rs or src/**/mod.rs."
    },
    "path": {
      "type": "string",
      "description": "A directory to search under, relative to the workspace root. Defaults to the whole workspace."
    },
    "limit": {
      "type": "integer",
      "minimum": 1,
      "description": "How many paths to return. Defaults to 200."
    }
  },
  "required": ["pattern"]
}"#;

/// Finds files in the workspace by path.
#[derive(Debug)]
pub struct Glob {
    workspace: Workspace,
}

/// What one walk came back with: the lowest paths it matched, and how many it
/// matched in all.
///
/// Bounded while the walk is still running, because a limit applied afterwards
/// bounds the answer and not the work: `**` over a large tree allocates a string
/// per matching file and then throws all but two hundred of them away, which
/// makes the figure the caller set the one thing in the call holding nothing
/// down.
///
/// Counted as well as kept. The count is what lets the answer say how many more
/// there were, and a `usize` is what it costs to keep saying it exactly rather
/// than saying "some".
struct Found {
    /// The lowest paths matched so far, largest at the top — the next one to
    /// fall out when a lower one arrives. Never longer than `limit`.
    kept: BinaryHeap<String>,
    limit: usize,
    seen: usize,
}

impl Found {
    /// Room for `limit` paths and no more.
    fn new(limit: usize) -> Self {
        Self {
            kept: BinaryHeap::new(),
            limit,
            seen: 0,
        }
    }

    /// Takes one matched path, keeping it only if it belongs in the answer.
    fn keep(&mut self, path: String) {
        self.seen += 1;

        if self.kept.len() < self.limit {
            self.kept.push(path);
            return;
        }

        // The top is the highest path in hand, so it is the one this displaces.
        // `pop` before `push` is what holds the heap at `limit` rather than
        // letting it grow by one on every match after the first `limit`.
        if self.kept.peek().is_some_and(|highest| *highest > path) {
            self.kept.pop();
            self.kept.push(path);
        }
    }

    /// The paths, in the order they are reported: lowest first.
    fn sorted(self) -> Vec<String> {
        self.kept.into_sorted_vec()
    }

    /// How many matched that the answer does not carry.
    fn more(&self) -> usize {
        self.seen.saturating_sub(self.kept.len())
    }
}

impl Glob {
    /// Searches inside `workspace`, and nowhere else.
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for Glob {
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
        let args = Args::parse(NAME, approved.args())?;
        let pattern = args.text("pattern")?;
        let limit = args.count("limit", PATHS)?;

        // `literal_separator` is what makes `*` stop at a directory boundary,
        // so `src/*.rs` means the files in `src` and `**/*.rs` means the ones
        // below it. Without it the two patterns mean the same thing.
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map(|glob| glob.compile_matcher());
        let Ok(glob) = glob else {
            return Ok(ToolOutput::failed(format!("{pattern} is not a valid glob")));
        };

        let requested = args.optional_text("path")?.unwrap_or(".");
        let from = match self.workspace.existing(requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        // The walk runs to the end even once `limit` paths are in hand, which
        // is deliberate and is the one cost this bound does not remove. The
        // answer is the lowest paths in the tree rather than the first ones
        // reached, so a walk that stopped early would answer with whichever
        // files the directory order happened to reach first — and could not say
        // how many more there were, only that there were some. The walk is the
        // one `grep` runs and the ignore rules are what bound it.
        let mut found = Found::new(limit);
        for entry in crate::tree::walk(from.as_path())
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            // A listing is decided about the directory, so a rule about a file
            // under it is honoured here — where the file is reached. `grep`
            // does the same, which is what keeps the two from disagreeing
            // about what is in the workspace.
            .filter(|entry| !approved.denies(&self.workspace, &from, entry.path()))
        {
            // Matched against the name it will be reported under, which is the
            // one `grep` reports too. Dropping every entry that would not strip
            // to a relative path answered "no path matched" for a directory the
            // workspace was widened to reach and `grep` searched happily.
            let shown = crate::tree::named(&self.workspace, entry.path());
            if glob.is_match(&shown) {
                found.keep(shown);
            }
        }

        Ok(report(found, pattern))
    }
}

/// The paths, one per line.
fn report(found: Found, pattern: &str) -> ToolOutput {
    // Read before the paths are taken out of it, while it can still tell a
    // walk that matched nothing from one whose answer is full.
    let more = found.more();
    let shown = found.sorted();

    if shown.is_empty() {
        return ToolOutput::failed(format!("no path matched {pattern}"));
    }

    let lines = shown.join("\n") + "\n";
    let tail = if more == 0 {
        String::new()
    } else {
        format!("\n[{more} more: narrow the pattern or raise limit]")
    };

    ToolOutput::ok(lines + &tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_core::Disposition;

    use crate::sample::{Sample, allowed, under};

    fn glob(sample: &Sample, args: &str) -> ToolOutput {
        let tool = Glob::new(sample.workspace());
        tool.run(allowed(&tool, args)).unwrap()
    }

    /// A tree with files at two depths.
    fn tree(name: &str) -> Sample {
        let sample = Sample::new(name);
        sample.write("src/main.rs", "");
        sample.write("src/cli/parse.rs", "");
        sample.write("README.md", "");
        sample
    }

    #[test]
    fn a_pattern_matches_at_every_depth_below_it() {
        let sample = tree("glob-deep");

        let output = glob(&sample, r#"{"pattern":"**/*.rs"}"#);

        assert_eq!(output.text(), "src/cli/parse.rs\nsrc/main.rs\n");
    }

    #[test]
    fn a_star_stops_at_a_directory_boundary() {
        // Otherwise `src/*.rs` and `src/**/*.rs` would mean the same thing and
        // there would be no way to ask for one directory.
        let sample = tree("glob-shallow");

        let output = glob(&sample, r#"{"pattern":"src/*.rs"}"#);

        assert_eq!(output.text(), "src/main.rs\n");
    }

    #[test]
    fn paths_come_back_in_a_settled_order() {
        let sample = tree("glob-order");

        let first = glob(&sample, r#"{"pattern":"**/*"}"#);
        let again = glob(&sample, r#"{"pattern":"**/*"}"#);

        assert_eq!(first.text(), again.text());
    }

    #[test]
    fn nothing_matching_is_a_result_and_not_an_empty_answer() {
        let sample = tree("glob-none");

        let output = glob(&sample, r#"{"pattern":"**/*.py"}"#);

        assert!(output.is_failed());
        assert_eq!(output.text(), "no path matched **/*.py");
    }

    #[test]
    fn a_gitignored_file_is_not_listed() {
        let sample = tree("glob-ignored");
        sample.write(".gitignore", "target/\n");
        sample.write("target/debug/build.rs", "");

        let output = glob(&sample, r#"{"pattern":"**/*.rs"}"#);

        assert!(!output.text().contains("target/"), "{}", output.text());
    }

    #[test]
    fn a_path_narrows_the_search_to_one_directory() {
        let sample = tree("glob-path");

        let output = glob(&sample, r#"{"pattern":"**/*.rs","path":"src/cli"}"#);

        assert_eq!(output.text(), "src/cli/parse.rs\n");
    }

    #[test]
    fn a_limit_stops_and_says_how_many_are_left() {
        let sample = Sample::new("glob-limit");
        for n in 0..5 {
            sample.write(&format!("f{n}.txt"), "");
        }

        let output = glob(&sample, r#"{"pattern":"*.txt","limit":2}"#);

        assert_eq!(
            output.text(),
            "f0.txt\nf1.txt\n\n[3 more: narrow the pattern or raise limit]"
        );
    }

    #[test]
    fn a_walk_holds_no_more_paths_than_the_answer_will_carry() {
        // The bound is on the work, not only on the answer: a tree with far
        // more matches than the limit may not put a string in memory for each
        // of them. Asserted after every path, because a structure that keeps
        // everything and trims at the end passes any assertion made at the end.
        let mut found = Found::new(5);

        for n in 0..2_000 {
            found.keep(format!("src/file{n:04}.rs"));
            assert!(
                found.kept.len() <= 5,
                "the walk was holding {} paths at file {n}",
                found.kept.len()
            );
        }

        assert_eq!(found.more(), 1_995);
        assert_eq!(
            found.sorted(),
            vec![
                "src/file0000.rs",
                "src/file0001.rs",
                "src/file0002.rs",
                "src/file0003.rs",
                "src/file0004.rs",
            ]
        );
    }

    #[test]
    fn keeping_the_lowest_paths_answers_the_same_as_sorting_all_of_them() {
        // The bounded structure replaced a sort of the whole vector, so what it
        // owes is that answer and not merely a bounded one.
        let paths: Vec<String> = (0..500)
            .map(|n| format!("d{}/f{n:03}.rs", (n * 37) % 11))
            .collect();

        let mut found = Found::new(7);
        for path in paths.clone() {
            found.keep(path);
        }

        let mut sorted = paths;
        sorted.sort_unstable();
        sorted.truncate(7);
        assert_eq!(found.sorted(), sorted);
    }

    #[test]
    fn a_tree_far_larger_than_the_limit_answers_with_the_lowest_paths_and_the_count() {
        let sample = Sample::new("glob-many");
        for n in 0..400 {
            sample.write(&format!("src/f{n:03}.rs"), "");
        }

        let output = glob(&sample, r#"{"pattern":"**/*.rs","limit":3}"#);

        assert_eq!(
            output.text(),
            "src/f000.rs\nsrc/f001.rs\nsrc/f002.rs\n\n[397 more: narrow the pattern or raise limit]"
        );
    }

    #[test]
    fn a_pattern_that_is_not_a_glob_says_so() {
        let sample = tree("glob-bad");

        let output = glob(&sample, r#"{"pattern":"[unclosed"}"#);

        assert!(output.is_failed());
        assert!(output.text().contains("not a valid glob"));
    }

    #[test]
    fn a_path_outside_the_workspace_is_refused() {
        let sample = tree("glob-escape");
        sample.outside("secret.txt", "");
        let outside = format!("{}/../outside", sample.named());

        let output = glob(
            &sample,
            &format!(r#"{{"pattern":"**/*","path":"{outside}"}}"#),
        );

        assert!(output.is_failed());
        assert!(!output.text().contains("secret.txt"));
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let sample = tree("glob-dirs");

        let output = glob(&sample, r#"{"pattern":"src"}"#);

        assert!(output.is_failed(), "{}", output.text());
    }

    #[test]
    fn a_file_a_deny_rule_names_is_never_listed() {
        // `grep` and `glob` walk the same tree and may not disagree about what
        // is in it: a file one of them refuses to open is not one the other
        // announces the existence of.
        let sample = tree("glob-denied");
        sample.write("private/key.rs", "");

        let tool = Glob::new(sample.workspace());
        let output = tool
            .run(under(
                &tool,
                r#"{"pattern":"**/*.rs"}"#,
                &[(Disposition::Deny, "glob(private/**)")],
            ))
            .unwrap();

        assert!(!output.text().contains("private/"), "{}", output.text());
        assert!(output.text().contains("src/main.rs"), "{}", output.text());
    }

    #[test]
    fn grep_and_glob_name_a_file_in_a_reached_directory_the_same_way() {
        // `extraDirectories` is a place the tools work. `glob` spelled a hit
        // relative to the primary root and dropped anything that would not
        // strip, so a file `grep` returned was one `glob` reported did not
        // exist — the disagreement the shared walk exists to prevent.
        let sample = Sample::new("glob-reaching");
        let beside = sample.beside("notes");
        std::fs::write(std::path::Path::new(&beside).join("plan.md"), "needle\n")
            .expect("a writable temporary directory");

        let workspace = sample.reaching(&beside);
        let listing = Glob::new(workspace.clone());
        let listed = listing
            .run(allowed(
                &listing,
                &format!(r#"{{"pattern":"**/*.md","path":"{beside}"}}"#),
            ))
            .unwrap();

        let search = crate::Grep::new(workspace);
        let searched = search
            .run(allowed(
                &search,
                &format!(r#"{{"pattern":"needle","path":"{beside}"}}"#),
            ))
            .unwrap();

        assert!(!listed.is_failed(), "{}", listed.text());
        let named = listed.text().trim_end();
        assert!(named.ends_with("plan.md"), "{named}");
        assert_eq!(searched.text(), format!("{named}:1:needle\n"));
    }

    #[test]
    fn listing_names_the_directory_it_would_walk() {
        let sample = Sample::new("glob-sensitivity");
        sample.write("src/main.rs", "");
        let tool = Glob::new(sample.workspace());

        let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"src"}"#));

        assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
        assert_eq!(sensitivity.to_string(), "read src");
    }
}
