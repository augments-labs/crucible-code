//! Finding files by the shape of their path.

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

        let mut found: Vec<String> = crate::tree::walk(from.as_path())
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            // A listing is decided about the directory, so a rule about a file
            // under it is honoured here — where the file is reached. `grep`
            // does the same, which is what keeps the two from disagreeing
            // about what is in the workspace.
            .filter(|entry| !approved.denies(&self.workspace, &from, entry.path()))
            .filter_map(|entry| {
                let relative = entry.path().strip_prefix(self.workspace.root()).ok()?;
                glob.is_match(relative)
                    .then(|| crucible_core::written(relative))
            })
            .collect();

        found.sort_unstable();
        Ok(report(&found, pattern, limit))
    }
}

/// The paths, one per line.
fn report(found: &[String], pattern: &str, limit: usize) -> ToolOutput {
    if found.is_empty() {
        return ToolOutput::failed(format!("no path matched {pattern}"));
    }

    let shown = found
        .iter()
        .take(limit)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";

    let tail = if found.len() > limit {
        let more = found.len() - limit;
        format!("\n[{more} more: narrow the pattern or raise limit]")
    } else {
        String::new()
    };

    ToolOutput::ok(shown + &tail)
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
    fn listing_names_the_directory_it_would_walk() {
        let sample = Sample::new("glob-sensitivity");
        sample.write("src/main.rs", "");
        let tool = Glob::new(sample.workspace());

        let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"src"}"#));

        assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
        assert_eq!(sensitivity.to_string(), "read src");
    }
}
