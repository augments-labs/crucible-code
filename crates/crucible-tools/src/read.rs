//! Reading a file.

use std::io::{self, BufRead, BufReader, ErrorKind, Read as _};

use crucible_core::{
    Approved, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace,
};

use crate::args::Args;
use crate::bound::OUTPUT;
use crate::ledger::Ledger;
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

/// The size of each input read while looking for the end of one line.
const BLOCK: usize = 8 * 1_024;

/// Room kept for the pagination note while output lines are added.
const NOTICE: usize = 128;

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
      "description": "How many lines to return. Defaults to 2000, and never more than 10000 however large a number is sent. The answer is also cut at 30000 bytes, whichever comes first."
    }
  },
  "required": ["path"]
}"#;

/// Reads a file from the workspace.
#[derive(Debug)]
pub struct Read {
    workspace: Workspace,
    cancel: Cancel,
    seen: Ledger,
}

impl Read {
    /// Reads inside `workspace`, and nowhere else, telling `seen` about every
    /// file it shows.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel, seen: Ledger) -> Self {
        Self {
            workspace,
            cancel,
            seen,
        }
    }

    /// Numbers the lines the call asked for, and says how many it showed.
    ///
    /// Reads one line past the limit rather than counting the file: knowing
    /// that more follows is what the model needs, and knowing how many more
    /// would mean reading a gigabyte to answer a question about its first
    /// page.
    ///
    /// The count comes back because `write` is downstream of it. An answer of
    /// no lines at all is a file this call did not show the agent — an offset
    /// past the end reads that way — and a file nobody was shown is not one
    /// anybody may replace.
    fn numbered(
        &self,
        mut lines: impl BufRead,
        requested: &str,
        from: usize,
        limit: usize,
    ) -> Result<(ToolOutput, usize), ToolError> {
        let mut out = String::new();
        let mut shown = 0;
        let mut number = 0;
        let mut more = None;

        loop {
            let line = match bounded_line(&mut lines, &self.cancel) {
                Ok(NextLine::Line(line)) => line,
                Ok(NextLine::End) => break,
                Ok(NextLine::Cancelled) => return Err(ToolError::Cancelled(NAME)),
                // Not text. That is an answer the model should have, not a
                // breakdown of the mechanism.
                Err(problem) if problem.kind() == ErrorKind::InvalidData => {
                    return Ok((
                        ToolOutput::failed(format!("{requested} is not a text file")),
                        0,
                    ));
                }
                Err(source) => {
                    return Err(ToolError::Io {
                        tool: NAME,
                        problem: format!("could not read {requested}").into(),
                        source,
                    });
                }
            };
            number += 1;

            if number < from {
                continue;
            }

            if shown == limit {
                more = Some(number);
                break;
            }

            let rendered = numbered_line(number, &line);
            if out.len() + rendered.len() + NOTICE > OUTPUT {
                more = Some(number);
                break;
            }

            out.push_str(&rendered);
            shown += 1;
        }

        if shown == 0 {
            return Ok((ToolOutput::ok(format!("{requested} has no line {from}")), 0));
        }

        let tail = more.map_or_else(String::new, |next| {
            format!("\n[more follows: call {NAME} again with offset {next}]")
        });

        Ok((ToolOutput::ok(out + &tail), shown))
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
        let file = match path.open_regular() {
            Ok(file) => file,
            Err(problem) => return Ok(ToolOutput::failed(problem.to_string())),
        };

        let (output, shown) = self.numbered(BufReader::new(file), requested, from, limit)?;

        // The resolved path rather than the requested one, because `write` asks
        // with a resolved path too — otherwise `./one.txt` and `one.txt` would
        // be two different files to a record that exists to say they are one.
        if shown > 0 {
            self.seen.record(path.as_path());
        }

        Ok(output)
    }
}

/// One line, numbered the way `cat -n` numbers them, and cut if it is longer
/// than anything worth sending.
fn numbered_line(number: usize, line: &Line) -> String {
    if line.cut {
        format!(
            "{number:>6}\t{}[line cut at {WIDTH} characters]\n",
            line.text
        )
    } else {
        format!("{number:>6}\t{}\n", line.text)
    }
}

/// One line whose storage is bounded however far away its newline is.
struct Line {
    text: String,
    cut: bool,
}

/// One bounded step through the input, including cancellation as data rather
/// than an I/O error manufactured to cross the helper boundary.
enum NextLine {
    Line(Line),
    End,
    Cancelled,
}

/// Reads one line in fixed-size pieces, validating even the part not retained.
fn bounded_line(lines: &mut impl BufRead, cancel: &Cancel) -> io::Result<NextLine> {
    let mut text = String::new();
    let mut carry = Vec::with_capacity(BLOCK + 3);
    let mut block = Vec::with_capacity(BLOCK);
    let mut seen = 0;
    let mut last = None;
    let mut any = false;

    loop {
        if cancel.requested() {
            return Ok(NextLine::Cancelled);
        }
        block.clear();
        let read = lines
            .by_ref()
            .take(BLOCK as u64)
            .read_until(b'\n', &mut block)?;
        if read == 0 {
            if !any {
                return Ok(NextLine::End);
            }
            finish_utf8(&mut carry, &mut text, &mut seen, &mut last)?;
            break;
        }

        if cancel.requested() {
            return Ok(NextLine::Cancelled);
        }

        any = true;
        let ended = block.last() == Some(&b'\n');
        if ended {
            block.pop();
        }
        carry.extend_from_slice(&block);
        finish_utf8(&mut carry, &mut text, &mut seen, &mut last)?;

        if ended {
            break;
        }
    }

    if !carry.is_empty() {
        return Err(ErrorKind::InvalidData.into());
    }

    let carriage = last == Some('\r');
    if carriage && seen <= WIDTH && text.ends_with('\r') {
        text.pop();
    }
    let characters = seen.saturating_sub(usize::from(carriage));
    Ok(NextLine::Line(Line {
        text,
        cut: characters > WIDTH,
    }))
}

/// Moves every complete character from `bytes` into one bounded line prefix.
fn finish_utf8(
    bytes: &mut Vec<u8>,
    text: &mut String,
    seen: &mut usize,
    last: &mut Option<char>,
) -> io::Result<()> {
    let valid = match std::str::from_utf8(bytes) {
        Ok(valid) => valid.len(),
        Err(problem) if problem.error_len().is_none() => problem.valid_up_to(),
        Err(_) => return Err(ErrorKind::InvalidData.into()),
    };

    let decoded = std::str::from_utf8(bytes.get(..valid).unwrap_or_default())
        .map_err(|_| ErrorKind::InvalidData)?;
    for character in decoded.chars() {
        *last = Some(character);
        if *seen < WIDTH {
            text.push(character);
        }
        // Keep one extra state beyond "over width": a trailing carriage
        // return is not part of the line, so `WIDTH + 1` characters followed
        // by `\r` must remain distinguishable from exactly `WIDTH` plus `\r`.
        *seen = seen.saturating_add(1).min(WIDTH + 2);
    }

    bytes.copy_within(valid.., 0);
    bytes.truncate(bytes.len() - valid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{Sample, allowed};

    fn read(sample: &Sample, args: &str) -> ToolOutput {
        reading(sample, args, &Ledger::new())
    }

    fn reading(sample: &Sample, args: &str, seen: &Ledger) -> ToolOutput {
        let tool = Read::new(sample.workspace(), Cancel::new(), seen.clone());
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
    fn the_shared_byte_ceiling_stops_before_a_larger_line_limit() {
        let sample = Sample::new("read-ceiling");
        sample.write("many.txt", &"x\n".repeat(CEILING + 5));

        let output = read(&sample, r#"{"path":"many.txt","limit":1000000}"#);

        let shown = output
            .text()
            .lines()
            .filter(|line| line.contains('\t'))
            .count();
        assert!(shown < CEILING, "{shown}");
        assert!(output.text().len() <= OUTPUT, "{}", output.text().len());
        assert!(
            output.text().contains("[more follows:"),
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
    fn a_call_that_showed_no_lines_is_not_a_file_anybody_looked_at() {
        // An offset past the end answers, and answers successfully — but the
        // agent has been shown nothing, so `write` must still refuse to replace
        // it. Otherwise `{"offset":999999}` is a one-call way past the refusal.
        let sample = Sample::new("read-unseen");
        sample.write("one.txt", "work nobody looked at\n");
        let seen = Ledger::new();

        let output = reading(&sample, r#"{"path":"one.txt","offset":9}"#, &seen);

        assert!(!output.is_failed(), "{}", output.text());
        assert!(
            !seen.holds(sample.workspace().existing("one.txt").unwrap().as_path()),
            "a call that showed nothing counted as a read"
        );
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
    fn a_huge_line_with_no_newline_never_becomes_a_huge_answer() {
        let sample = Sample::new("read-huge-line");
        sample.write("huge.txt", &"x".repeat(OUTPUT * 100));

        let output = read(&sample, r#"{"path":"huge.txt"}"#);

        assert!(output.text().len() < WIDTH + 200, "{}", output.text().len());
        assert!(output.text().contains("[line cut"), "{}", output.text());
    }

    #[test]
    fn a_huge_offset_stops_scanning_at_the_next_bounded_read() {
        struct Stops {
            cancel: Cancel,
        }

        impl std::io::Read for Stops {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let Some(place) = buffer.get_mut(..2) else {
                    return Err(io::Error::other("the test reader buffer is too small"));
                };
                place.copy_from_slice(b"x\n");
                self.cancel.request();
                Ok(place.len())
            }
        }

        let sample = Sample::new("read-cancel-offset");
        let cancel = Cancel::new();
        let input = BufReader::new(Stops {
            cancel: cancel.clone(),
        });
        let tool = Read::new(sample.workspace(), cancel, Ledger::new());

        let problem = tool
            .numbered(input, "huge.txt", usize::MAX, CEILING)
            .unwrap_err();

        assert!(matches!(problem, ToolError::Cancelled(NAME)));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_waiting_for_a_writer() {
        let sample = Sample::new("read-fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(sample.root().join("waiting"))
            .status()
            .unwrap();
        assert!(made.success());

        let output = read(&sample, r#"{"path":"waiting"}"#);

        assert!(output.is_failed());
        assert!(output.text().contains("is not a regular file"));
    }

    #[test]
    fn a_bad_byte_past_the_kept_prefix_still_makes_the_file_non_text() {
        let sample = Sample::new("read-wide-binary");
        let mut bytes = vec![b'x'; WIDTH + BLOCK];
        bytes.push(0xff);
        sample.write_bytes("wide.bin", &bytes);

        let output = read(&sample, r#"{"path":"wide.bin"}"#);

        assert!(output.is_failed());
        assert_eq!(output.text(), "wide.bin is not a text file");
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
    fn a_trailing_carriage_return_does_not_hide_that_a_line_was_cut() {
        let sample = Sample::new("read-wide-crlf");
        sample.write("wide.txt", &format!("{}\r\n", "x".repeat(WIDTH + 1)));

        let output = read(&sample, r#"{"path":"wide.txt"}"#);

        assert!(output.text().contains("[line cut"), "{}", output.text());
    }

    #[test]
    fn a_call_with_no_path_says_what_is_missing() {
        let sample = Sample::new("read-nopath");

        let tool = Read::new(sample.workspace(), Cancel::new(), Ledger::new());
        let problem = tool.run(allowed(&tool, "{}")).unwrap_err();

        assert_eq!(problem.to_string(), "read: path is required");
    }

    #[test]
    fn reading_names_the_file_it_would_read() {
        // Read-only, so it is never put to the user — but a rule can still deny
        // it, and a rule is about a path.
        let sample = Sample::new("read-sensitivity");
        sample.write("one.txt", "alpha\n");
        let tool = Read::new(sample.workspace(), Cancel::new(), Ledger::new());

        let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"one.txt"}"#));

        assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
        assert_eq!(sensitivity.to_string(), "read one.txt");
    }
}
