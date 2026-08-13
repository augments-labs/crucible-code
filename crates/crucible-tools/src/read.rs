//! Reading a file.

use std::io::{BufRead, BufReader, ErrorKind};

use crucible_core::{Approved, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};

use crate::args::Args;
use crate::target;

/// The name the model calls.
const NAME: &str = "read";

/// How many lines one call answers with when it does not say.
const LINES: usize = 2_000;

/// The most lines a call can ask for, however large a number it sends.
///
/// `LINES` is a default and a caller can raise it; this it cannot. Without it a
/// model told a file was truncated can ask for the rest by naming a number, and
/// a vendored bundle comes back whole — one `String` the size of the file, into
/// a transcript that is what the memory budget is measured on. The notice below
/// already says how to ask for the next page, so nothing is unreachable; it just
/// takes another call.
const CEILING: usize = 10_000;

/// Where a single line is cut. One minified bundle on one line would otherwise
/// fill the whole answer with text nobody — model or user — can read.
const WIDTH: usize = 2_000;

/// The root `description` is the tool's own; everything below it describes the
/// arguments. The provider moves it out before sending the rest as the schema.
const SCHEMA: &str = r#"{
  "description": "Reads a text file from the workspace and returns it with line numbers.",
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "The file to read, relative to the workspace root."
    },
    "offset": {
      "type": "integer",
      "minimum": 1,
      "description": "The first line to return, counting from 1. Defaults to the start of the file."
    },
    "limit": {
      "type": "integer",
      "minimum": 1,
      "description": "How many lines to return. Defaults to 2000, and never more than 10000 however large a number is sent."
    }
  },
  "required": ["path"]
}"#;

/// Reads a file from the workspace.
#[derive(Debug)]
pub struct Read {
    workspace: Workspace,
}

impl Read {
    /// Reads inside `workspace`, and nowhere else.
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }

    /// Numbers the lines the call asked for.
    ///
    /// Reads one line past the limit rather than counting the file: knowing
    /// that more follows is what the model needs, and knowing how many more
    /// would mean reading a gigabyte to answer a question about its first
    /// page.
    fn numbered(
        lines: impl BufRead,
        requested: &str,
        from: usize,
        limit: usize,
    ) -> Result<ToolOutput, ToolError> {
        let mut out = String::new();
        let mut shown = 0;
        let mut more = false;

        for (index, line) in lines.lines().enumerate().skip(from - 1) {
            let line = match line {
                Ok(line) => line,
                // Not text. That is an answer the model should have, not a
                // breakdown of the mechanism.
                Err(problem) if problem.kind() == ErrorKind::InvalidData => {
                    return Ok(ToolOutput::failed(format!(
                        "{requested} is not a text file"
                    )));
                }
                Err(source) => {
                    return Err(ToolError::Io {
                        tool: NAME,
                        problem: format!("could not read {requested}").into(),
                        source,
                    });
                }
            };

            if shown == limit {
                more = true;
                break;
            }

            out.push_str(&numbered_line(index + 1, &line));
            shown += 1;
        }

        if shown == 0 {
            return Ok(ToolOutput::ok(format!("{requested} has no line {from}")));
        }

        let tail = if more {
            let next = from + limit;
            format!("\n[more follows: call {NAME} again with offset {next}]")
        } else {
            String::new()
        };

        Ok(ToolOutput::ok(out + &tail))
    }
}

impl Tool for Read {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: target::existing(&self.workspace, NAME, args, "path"),
        }
    }

    fn run(&self, approved: Approved) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let requested = args.text("path")?;
        let from = args.count("offset", 1)?;
        let limit = args.count("limit", LINES)?.min(CEILING);

        // A path outside the workspace, or one that is not there, is something
        // the model can correct by sending a different path.
        let path = match self.workspace.existing(requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        if path.as_path().is_dir() {
            return Ok(ToolOutput::failed(format!("{requested} is a directory")));
        }

        // Through the workspace rather than by name, so a last component
        // replaced with a symbolic link since the check above is refused rather
        // than read out of the tree and into the transcript, where the answer
        // to a question about a file in the project would be a file elsewhere.
        let file = match path.open() {
            Ok(file) => file,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        Self::numbered(BufReader::new(file), requested, from, limit)
    }
}

/// One line, numbered the way `cat -n` numbers them, and cut if it is longer
/// than anything worth sending.
fn numbered_line(number: usize, line: &str) -> String {
    match line.char_indices().nth(WIDTH) {
        Some((at, _)) => {
            let kept = line.get(..at).unwrap_or(line);
            format!("{number:>6}\t{kept}[line cut at {WIDTH} characters]\n")
        }
        None => format!("{number:>6}\t{line}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{Sample, allowed};

    fn read(sample: &Sample, args: &str) -> ToolOutput {
        let tool = Read::new(sample.workspace());
        tool.run(allowed(&tool, args)).unwrap()
    }

    #[test]
    fn a_file_comes_back_numbered_from_one() {
        let sample = Sample::new("read-plain");
        sample.write("one.txt", "alpha\nbeta\n");

        let output = read(&sample, r#"{"path":"one.txt"}"#);

        assert_eq!(output.text(), "     1\talpha\n     2\tbeta\n");
        assert!(!output.is_failed());
    }

    #[test]
    fn an_offset_starts_where_it_says_and_keeps_the_real_line_numbers() {
        // Renumbering from 1 would make the model quote a line that is not
        // there when it comes back to edit the file.
        let sample = Sample::new("read-offset");
        sample.write("one.txt", "a\nb\nc\nd\n");

        let output = read(&sample, r#"{"path":"one.txt","offset":3}"#);

        assert_eq!(output.text(), "     3\tc\n     4\td\n");
    }

    #[test]
    fn a_limit_stops_and_says_how_to_ask_for_the_rest() {
        let sample = Sample::new("read-limit");
        sample.write("one.txt", "a\nb\nc\nd\n");

        let output = read(&sample, r#"{"path":"one.txt","limit":2}"#);

        assert_eq!(
            output.text(),
            "     1\ta\n     2\tb\n\n[more follows: call read again with offset 3]"
        );
    }

    #[test]
    fn a_limit_larger_than_the_ceiling_is_the_ceiling() {
        // The default is a default and the caller may raise it; this it may not.
        // A model told a file was truncated would otherwise ask for the rest by
        // naming a number, and a vendored bundle would come back whole.
        let sample = Sample::new("read-ceiling");
        sample.write("many.txt", &"x\n".repeat(CEILING + 5));

        let output = read(&sample, r#"{"path":"many.txt","limit":1000000}"#);

        assert_eq!(
            output.text().lines().filter(|l| l.contains('\t')).count(),
            CEILING
        );
        assert!(
            output.text().ends_with(&format!(
                "[more follows: call read again with offset {}]",
                CEILING + 1
            )),
            "{}",
            output.text()
        );
    }

    #[test]
    fn a_file_that_ends_exactly_on_the_limit_says_nothing_follows() {
        let sample = Sample::new("read-exact");
        sample.write("one.txt", "a\nb\n");

        let output = read(&sample, r#"{"path":"one.txt","limit":2}"#);

        assert_eq!(output.text(), "     1\ta\n     2\tb\n");
    }

    #[test]
    fn an_offset_past_the_end_says_so_rather_than_answering_with_nothing() {
        let sample = Sample::new("read-past");
        sample.write("one.txt", "a\n");

        let output = read(&sample, r#"{"path":"one.txt","offset":9}"#);

        assert_eq!(output.text(), "one.txt has no line 9");
    }

    #[test]
    fn a_missing_file_is_a_result_the_model_can_act_on() {
        let sample = Sample::new("read-missing");

        let output = read(&sample, r#"{"path":"absent.txt"}"#);

        assert!(output.is_failed());
        assert!(
            output.text().contains("does not exist"),
            "{}",
            output.text()
        );
    }

    #[test]
    fn a_path_outside_the_workspace_is_refused_without_reading_it() {
        let sample = Sample::new("read-escape");
        let outside = sample.outside("secret.txt", "classified");

        let output = read(&sample, &format!(r#"{{"path":"{outside}"}}"#));

        assert!(output.is_failed());
        assert!(!output.text().contains("classified"));
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let sample = Sample::new("read-dir");
        sample.write("sub/one.txt", "a\n");

        let output = read(&sample, r#"{"path":"sub"}"#);

        assert!(output.is_failed());
        assert_eq!(output.text(), "sub is a directory");
    }

    #[test]
    fn something_that_is_not_text_says_so_instead_of_producing_rubbish() {
        let sample = Sample::new("read-binary");
        sample.write_bytes("blob.bin", &[0xff, 0xfe, 0x00, 0x01]);

        let output = read(&sample, r#"{"path":"blob.bin"}"#);

        assert!(output.is_failed());
        assert_eq!(output.text(), "blob.bin is not a text file");
    }

    #[test]
    fn a_line_too_long_to_be_useful_is_cut_and_says_that_it_was() {
        let sample = Sample::new("read-wide");
        sample.write("wide.txt", &format!("{}\n", "x".repeat(WIDTH + 50)));

        let output = read(&sample, r#"{"path":"wide.txt"}"#);

        assert!(
            output
                .text()
                .ends_with(&format!("[line cut at {WIDTH} characters]\n"))
        );
        assert!(output.text().len() < WIDTH + 200);
    }

    #[test]
    fn a_cut_lands_on_a_character_and_not_inside_one() {
        // Cutting by bytes would split a multi-byte character in half and send
        // the model text that is no longer valid.
        let sample = Sample::new("read-utf8");
        sample.write("wide.txt", &format!("{}\n", "é".repeat(WIDTH + 10)));

        let output = read(&sample, r#"{"path":"wide.txt"}"#);

        assert_eq!(output.text().matches('é').count(), WIDTH);
    }

    #[test]
    fn a_call_with_no_path_says_what_is_missing() {
        let sample = Sample::new("read-nopath");

        let tool = Read::new(sample.workspace());
        let problem = tool.run(allowed(&tool, "{}")).unwrap_err();

        assert_eq!(problem.to_string(), "read: path is required");
    }

    #[test]
    fn reading_names_the_file_it_would_read() {
        // Read-only, so it is never put to the user — but a rule can still deny
        // it, and a rule is about a path.
        let sample = Sample::new("read-sensitivity");
        sample.write("one.txt", "alpha\n");
        let tool = Read::new(sample.workspace());

        let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"one.txt"}"#));

        assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
        assert_eq!(sensitivity.to_string(), "read one.txt");
    }
}
