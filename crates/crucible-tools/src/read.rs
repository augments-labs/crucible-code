//! Reading a file.

use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Read as _};
use std::sync::LazyLock;

use crucible_core::{
    Approved, Attachment, Cancel, Kind, Modality, Remembered, Sensitivity, Summary, Tool, ToolArgs,
    ToolError, ToolOutput, Watch, Workspace, WorkspacePath, kind, written,
};
use sha2::{Digest as _, Sha256};

use crate::args::Args;
use crate::bound::OUTPUT;
use crate::ledger::Ledger;
use crate::schema::{Field, Schema, Shape, Whole};
use crate::summary;
use crate::target;

/// The name the model calls.
const NAME: &str = "read";

/// The file to read. The one argument a call must send.
const PATH: &str = "path";

/// The first line to return.
const OFFSET: &str = "offset";

/// How many lines to return.
const LIMIT: &str = "limit";

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

/// One program that turns a file into something a model can take in, and how to
/// ask it to.
struct Converter {
    /// The name a shell would find it under, with no platform suffix and no
    /// directory. What a command line must actually say is worked out at the
    /// point of use, because on two of the three platforms this is not on the
    /// `PATH` and the answer is an absolute path.
    program: &'static str,
    /// What follows the program, with `{}` where the file's own name goes.
    arguments: &'static str,
    /// Whether this one writes the file's pictures out beside the text.
    ///
    /// True of `pandoc` and of nothing else, because it is true of nothing
    /// else: every other converter here reduces a picture to its caption, its
    /// alt text or nothing at all. The refusal says so only where this is set,
    /// so a converter that flattens a diagram is never described as keeping it.
    extracts: bool,
    /// A program run first, whose answer decides an argument of this one.
    ///
    /// `ffprobe` before `ffmpeg`, and nothing before a document converter:
    /// converting a document needs no measurement, and sampling a video does.
    /// One frame a second is thirty frames of a clip and seven thousand of a
    /// recording, so a rate suggested without a duration is a guess — and the
    /// guess that is wrong fills a directory and is discovered afterwards.
    ///
    /// Looked up the same way as the converter's own program rather than
    /// assumed to be spellable: `ffprobe` ships in the same package and lands
    /// in the same directory, but on two of the three platforms that directory
    /// is not on the `PATH`, and a bare name there is a command line the shell
    /// cannot run. Where it is named and not found, the whole converter is
    /// passed over — half a suggestion is worse than the next one down.
    measure: Option<Measure>,
}

/// The program that answers what a converter needs to know first, and what to
/// ask it.
struct Measure {
    /// The name a shell would find it under, as [`Converter::program`].
    program: &'static str,
    /// What follows it, with `{}` where the file's own name goes.
    arguments: &'static str,
}

/// What a file this tool cannot read turns into, once something on this machine
/// has been pointed at it.
///
/// Two, and a match rather than a flag, so a third arrives as a compiler error
/// in every place a sentence is written rather than as a document sentence
/// about something that is not a document.
enum Becomes {
    /// Text, which the model reads back.
    Text,
    /// Pictures, which the model looks at — attached to a prompt, the way any
    /// other picture is.
    Pictures,
}

/// One kind of file this tool cannot read, and what would turn it into one it
/// can.
struct Unreadable {
    /// The extension, lowercase and without its dot.
    suffix: &'static str,
    /// What to call it in the sentence a person or a model reads.
    what: &'static str,
    /// What it turns into, which decides which sentence is written about it.
    becomes: Becomes,
    /// What does the turning, best first.
    converters: &'static [Converter],
}

/// What pulls frames out of a video, shared by every video suffix.
///
/// One entry, because one program does this and the alternatives are that same
/// program under another name. Named rather than repeated six times: a rate
/// that is right for `.mp4` is right for `.mkv`, and six copies of it is six
/// chances for one to drift.
///
/// The frame cap and the rate are numbers in a suggestion, not policy in code.
/// crucible samples nothing, installs nothing and chooses no frames — the same
/// bargain as every other converter here. What the cap is *for* is that a
/// request carries about that many pictures, so a command that extracted
/// everything would spend a minute of video on 1 800 files of which twenty
/// could be looked at.
const SAMPLED: &[Converter] = &[Converter {
    program: "ffmpeg",
    arguments: "-i {} -vf fps=1 -frames:v 20 frame-%03d.jpg",
    extracts: false,
    measure: Some(Measure {
        program: "ffprobe",
        arguments: "-v error -show_entries format=duration -of csv=p=0 {}",
    }),
}];

/// The directory an extracting converter is told to write pictures into.
///
/// Named here and asked for in an `arguments` template, which is two places for
/// one fact. `every_converter_that_extracts_asks_for_the_directory_the_sentence_names`
/// is what keeps them the same one.
const EXTRACTED_INTO: &str = "converted-media";

/// What a file this tool cannot read is, and what would turn it into one it can.
///
/// A document is not a modality: no vendor this program speaks to accepts one
/// of these on the wire, and the answer everywhere is to convert it to text
/// first. The shell to do that with is already a tool, so what was missing was
/// never the capability — it was that nothing said so, at the one moment the
/// question is live.
///
/// Several programs per format, and the order is what survives rather than what
/// is likeliest: `pandoc` keeps headings, lists and tables as Markdown, and the
/// others flatten a document into prose. Naming one that is not on this machine
/// would spend a turn and a permission prompt on a command that cannot run, so
/// what is installed decides which is named, and where none is, the answer says
/// that rather than sending the model to find out.
///
/// `textutil` is macOS's own and is on no other platform, so it needs no
/// condition around it — the lookup finds it there and nowhere else. Every
/// entry that is not `pandoc` loses the pictures, charts and layout, which is
/// the honest ceiling on reading a document this way.
///
/// `pandoc` is told to extract media rather than only mention it. Left to
/// itself it writes a picture's *reference* into the Markdown and never writes
/// the picture, so the file it names is not there — and a link to nothing reads
/// exactly like a link to something, which costs a call to find out. Extracted,
/// the pictures are files, and a file is a thing the agent has somewhere to go
/// with.
///
/// A video is on the list for the same reason and by a different road. Nothing
/// crucible speaks to reads one, and everything it speaks to reads a picture —
/// so the answer is not a converter at all but a sampler, and what comes out is
/// attached rather than read back. It is the same shape: what the file is, and
/// what on this machine would do something about it.
///
/// An extension not on this list gets the plain refusal. A suggestion that does
/// not fit costs more than none at all.
const UNREADABLE: &[Unreadable] = &[
    Unreadable {
        suffix: "docx",
        what: "a Word document",
        becomes: Becomes::Text,
        converters: &[
            Converter {
                program: "pandoc",
                arguments: "{} --extract-media=converted-media -o converted.md",
                extracts: true,
                measure: None,
            },
            Converter {
                program: "textutil",
                arguments: "-convert txt {}",
                extracts: false,
                measure: None,
            },
            Converter {
                program: "soffice",
                arguments: "--headless --convert-to txt {}",
                extracts: false,
                measure: None,
            },
        ],
    },
    Unreadable {
        suffix: "odt",
        what: "an OpenDocument text document",
        becomes: Becomes::Text,
        converters: &[
            Converter {
                program: "pandoc",
                arguments: "{} --extract-media=converted-media -o converted.md",
                extracts: true,
                measure: None,
            },
            Converter {
                program: "textutil",
                arguments: "-convert txt {}",
                extracts: false,
                measure: None,
            },
            Converter {
                program: "soffice",
                arguments: "--headless --convert-to txt {}",
                extracts: false,
                measure: None,
            },
        ],
    },
    Unreadable {
        suffix: "rtf",
        what: "a rich text document",
        becomes: Becomes::Text,
        converters: &[
            Converter {
                program: "pandoc",
                arguments: "{} --extract-media=converted-media -o converted.md",
                extracts: true,
                measure: None,
            },
            Converter {
                program: "textutil",
                arguments: "-convert txt {}",
                extracts: false,
                measure: None,
            },
            Converter {
                program: "soffice",
                arguments: "--headless --convert-to txt {}",
                extracts: false,
                measure: None,
            },
        ],
    },
    Unreadable {
        suffix: "epub",
        what: "an e-book",
        becomes: Becomes::Text,
        converters: &[Converter {
            program: "pandoc",
            arguments: "{} --extract-media=converted-media -o converted.md",
            extracts: true,
            measure: None,
        }],
    },
    Unreadable {
        suffix: "xlsx",
        what: "a spreadsheet",
        becomes: Becomes::Text,
        converters: &[
            Converter {
                program: "soffice",
                arguments: "--headless --convert-to csv {}",
                extracts: false,
                measure: None,
            },
            Converter {
                program: "xlsx2csv",
                arguments: "{} converted.csv",
                extracts: false,
                measure: None,
            },
        ],
    },
    Unreadable {
        suffix: "xls",
        what: "a spreadsheet",
        becomes: Becomes::Text,
        converters: &[Converter {
            program: "soffice",
            arguments: "--headless --convert-to csv {}",
            extracts: false,
            measure: None,
        }],
    },
    Unreadable {
        suffix: "ods",
        what: "a spreadsheet",
        becomes: Becomes::Text,
        converters: &[Converter {
            program: "soffice",
            arguments: "--headless --convert-to csv {}",
            extracts: false,
            measure: None,
        }],
    },
    Unreadable {
        suffix: "pptx",
        what: "a slide deck",
        becomes: Becomes::Text,
        converters: &[Converter {
            program: "soffice",
            arguments: "--headless --convert-to txt {}",
            extracts: false,
            measure: None,
        }],
    },
    Unreadable {
        suffix: "odp",
        what: "a slide deck",
        becomes: Becomes::Text,
        converters: &[Converter {
            program: "soffice",
            arguments: "--headless --convert-to txt {}",
            extracts: false,
            measure: None,
        }],
    },
    Unreadable {
        suffix: "mp4",
        what: "a video",
        becomes: Becomes::Pictures,
        converters: SAMPLED,
    },
    Unreadable {
        suffix: "mov",
        what: "a video",
        becomes: Becomes::Pictures,
        converters: SAMPLED,
    },
    Unreadable {
        suffix: "mkv",
        what: "a video",
        becomes: Becomes::Pictures,
        converters: SAMPLED,
    },
    Unreadable {
        suffix: "webm",
        what: "a video",
        becomes: Becomes::Pictures,
        converters: SAMPLED,
    },
    Unreadable {
        suffix: "avi",
        what: "a video",
        becomes: Becomes::Pictures,
        converters: SAMPLED,
    },
    Unreadable {
        suffix: "m4v",
        what: "a video",
        becomes: Becomes::Pictures,
        converters: SAMPLED,
    },
    Unreadable {
        suffix: "pdf",
        what: "a PDF",
        becomes: Becomes::Text,
        converters: &[
            Converter {
                program: "pdftotext",
                arguments: "{} converted.txt",
                extracts: false,
                measure: None,
            },
            Converter {
                program: "soffice",
                arguments: "--headless --convert-to txt {}",
                extracts: false,
                measure: None,
            },
        ],
    },
];

/// What to say after the refusal, where the name says what the file is.
///
/// Matched on the name rather than the bytes on purpose: this is reached only
/// once the file has already failed to decode, so the extension is being asked
/// what somebody meant by it, not what the file is.
fn conversion(requested: &str, named: impl Fn(&str) -> Option<String>) -> Option<String> {
    let suffix = requested.rsplit_once('.')?.1.to_ascii_lowercase();
    let known = UNREADABLE.iter().find(|one| one.suffix == suffix)?;
    let what = known.what;

    // A converter whose measurement is missing is passed over rather than
    // suggested half-spelled, so `found` answers for the whole command line and
    // not only its first word.
    let found = known.converters.iter().find_map(|converter| {
        let program = command(&named(converter.program)?, converter.arguments, requested);
        let measured = match &converter.measure {
            Some(measure) => Some(command(
                &named(measure.program)?,
                measure.arguments,
                requested,
            )),
            None => None,
        };
        Some((program, measured, converter))
    });

    let Some((command, measured, converter)) = found else {
        let programs = known
            .converters
            .iter()
            .map(|converter| converter.program)
            .collect::<Vec<_>>()
            .join(" or ");
        // The measuring program is left out: it arrives with the one named, and
        // an answer about what to install should name one thing to install.
        let doing = match known.becomes {
            Becomes::Text => "converts one",
            Becomes::Pictures => "pulls frames out of one",
        };
        return Some(format!(
            ". It is {what}, and nothing installed here {doing} — {programs} would."
        ));
    };

    Some(match known.becomes {
        // Said only where the converter actually found is one that extracts. A
        // document whose whole content is a diagram converts to a heading and a
        // caption, and the pictures beside the text are the half that survives
        // it — but promising them of `soffice`, which reduces a picture to its
        // alt text, would send the model looking for files that are not there.
        Becomes::Text => {
            let extracted = if converter.extracts {
                format!(
                    ", and its pictures come out beside it into {EXTRACTED_INTO}/ \
                     where each one can be attached to a prompt and looked at"
                )
            } else {
                String::new()
            };
            format!(
                ". It is {what} — convert it and read what comes out{extracted}, \
                 for example: {command}"
            )
        }
        // Two commands and a rule for choosing between the rates they allow,
        // because the wrong rate is not an error — it is a directory of files
        // covering the first twenty seconds of a two-hour recording, and
        // finding that out costs the turn that ran it.
        Becomes::Pictures => {
            let measured = measured.unwrap_or_default();
            format!(
                ". It is {what} — nothing here reads one and everything here reads \
                 a picture, so pull frames out and attach those. {measured} says how \
                 long it is, and {command} takes one a second and stops at twenty, \
                 which is about as many as one request carries. Sample a long \
                 recording sparsely rather than sampling the start of it."
            )
        }
    })
}

/// One command line, with the file's own name in it and the program spelled the
/// way a shell here would have to be told.
///
/// Quoted where the spelling has a space in it, which is what an absolute path
/// under `Program Files` is and what a bare name never is.
fn command(program: &str, arguments: &str, requested: &str) -> String {
    let spelled = if program.contains(' ') {
        format!("\"{program}\"")
    } else {
        program.to_owned()
    };
    format!("{spelled} {}", arguments.replace("{}", requested))
}

/// The root `description` is the tool's own; everything below it describes the
/// arguments. The provider moves it out before sending the rest as the schema.
///
/// What the tool will not do is stated here rather than left to be discovered,
/// which is the same reason every ceiling below is written beside the argument
/// that reaches it: a bound met is one wasted call, and a bound read is none.
/// The ceilings are spelled by the constants the code holds them with, so the
/// sentence the model reads cannot drift from the bound the call meets.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Reads a text file from the workspace and returns it with line numbers. A picture \
                comes back attached, to be looked at rather than read. A Word document, \
                spreadsheet, slide deck, e-book, PDF or video must be turned into text or \
                pictures by a command first, and the answer says which one where the file's name \
                gives it away."
            .into(),
        fields: vec![
            Field {
                name: PATH,
                about: "The file to read, relative to the workspace root.".into(),
                needed: true,
                shape: Shape::Text,
            },
            Field {
                name: OFFSET,
                about: "The first line to return, counting from 1. Defaults to the start of the \
                        file."
                    .into(),
                needed: false,
                shape: Shape::Count(Whole {
                    least: 1,
                    most: None,
                }),
            },
            Field {
                name: LIMIT,
                about: format!(
                    "How many lines to return. Defaults to {LINES}, and never more than {CEILING} \
                     however large a number is sent. The answer is also cut at {OUTPUT} bytes, \
                     whichever comes first."
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
                // breakdown of the mechanism — and where the name says what the
                // file is, the answer carries the next move as well.
                Err(problem) if problem.kind() == ErrorKind::InvalidData => {
                    let mut said = format!("{requested} is not a text file");
                    if let Some(how) = conversion(requested, crate::program::installed) {
                        said.push_str(&how);
                    }
                    return Ok((ToolOutput::failed(said), 0));
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

impl Read {
    /// The file as something to look at, where that is what it is.
    ///
    /// `None` means this is not a picture after all, and the caller goes on to
    /// read it as text: a `.png` holding text gets the text, and a `.png`
    /// holding something else gets the refusal the text reader already writes.
    /// Guessing at either from the name would be a sentence about a file that
    /// is not there.
    ///
    /// Only pictures. A PDF is attachable at a prompt and is deliberately not
    /// handed back here: the model that cannot read one needs the sentence
    /// naming a converter, and that sentence is worth more than an attachment
    /// that would stand down on half the models crucible offers.
    fn looked_at(
        approved: &Approved,
        kind: &Kind,
        requested: &str,
        path: &WorkspacePath,
    ) -> Option<ToolOutput> {
        // A match rather than an equality, so a modality added to the core enum
        // arrives here as a compiler error asking whether `read` hands it back.
        match kind.modality {
            Modality::Image => {}
            Modality::Text | Modality::Pdf | Modality::Video | Modality::Audio => return None,
        }

        // Asked of the metadata, so a file too large to carry is never read
        // into this process to find that out.
        let about = fs::metadata(path.as_path()).ok()?;
        if about.len() > crucible_core::CEILING as u64 {
            return Some(ToolOutput::failed(format!(
                "{requested} is larger than the {} MB a request may carry, so it is not attached. \
                 A smaller copy of it would be.",
                crucible_core::CEILING / (1024 * 1024),
            )));
        }

        let bytes = fs::read(path.as_path()).ok()?;
        if !(kind.confirms)(&bytes) {
            return None;
        }

        Some(
            ToolOutput::ok(format!(
                "{requested} is attached as {} rather than read as text.",
                kind.spoken()
            ))
            .with_attachments(
                approved,
                [Attachment {
                    // The resolved path, for the reason `seen` records one: the
                    // request reads the file again when it goes out, and it
                    // must find the file this call looked at.
                    path: written(path.as_path()).into_boxed_str(),
                    modality: kind.modality,
                    media_type: kind.media_type.into(),
                    hash: <[u8; 32]>::from(Sha256::digest(&bytes)),
                }],
            ),
        )
    }
}

impl Tool for Read {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA.as_str()
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        target::reads(&self.workspace, NAME, args, PATH)
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, PATH)
    }

    fn remember(&self, args: &ToolArgs) -> Option<Remembered> {
        summary::remembered(NAME, args, false)
    }

    fn run(&self, approved: Approved, _watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let requested = args.text(PATH)?;
        let from = args.count(OFFSET, 1)?;
        let limit = args.count(LIMIT, LINES)?.min(CEILING);

        // A path that is not there is something the model can correct by
        // sending a different path. One outside the workspace is opened only
        // on the say-so the `Approved` in hand carries.
        let path = match target::opened(&self.workspace, &approved, requested) {
            Ok(path) => path,
            Err(problem) => return Ok(ToolOutput::failed(problem)),
        };

        if path.as_path().is_dir() {
            return Ok(ToolOutput::failed(format!("{requested} is a directory")));
        }

        // Asked before the file is opened for lines, because a picture is not
        // made of them and nothing a decoder found in it would change the
        // answer. A file that turns out not to be one falls through.
        if let Some(output) =
            kind(requested).and_then(|kind| Self::looked_at(&approved, kind, requested, &path))
        {
            if !output.is_failed() {
                self.seen.record(path.as_path());
            }
            return Ok(output);
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
    use crucible_core::Unwatched;

    use super::*;
    use crate::sample::{Sample, allowed};

    fn read(sample: &Sample, args: &str) -> ToolOutput {
        reading(sample, args, &Ledger::new())
    }

    fn reading(sample: &Sample, args: &str, seen: &Ledger) -> ToolOutput {
        let tool = Read::new(sample.workspace(), Cancel::new(), seen.clone());
        tool.run(allowed(&tool, args), &Unwatched).unwrap()
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
    fn a_path_outside_the_workspace_is_read_once_the_user_says_yes() {
        let sample = Sample::new("read-outside");
        let outside = sample.outside("secret.txt", "classified");

        let output = read(&sample, &format!(r#"{{"path":"{outside}"}}"#));

        assert!(!output.is_failed(), "{}", output.text());
        assert!(output.text().contains("classified"));
    }

    #[test]
    fn a_link_retargeted_after_the_verdict_leads_outside_and_is_refused() {
        // The defect this catches: `run` fell back to resolving outside the
        // workspace whenever a path escaped, whether or not the verdict in
        // hand was reached about an outside read. A link pointing inside when
        // the call was decided and outside by the time it ran would have been
        // read with nobody asked.
        let sample = Sample::new("read-retarget");
        sample.write("inside.txt", "the file the verdict was about\n");
        let secret = sample.outside("secret.txt", "nobody was asked about this\n");
        crate::sample::symlink("inside.txt", sample.root().join("door.txt"));

        let tool = Read::new(sample.workspace(), Cancel::new(), Ledger::new());
        let approved = allowed(&tool, r#"{"path":"door.txt"}"#);

        std::fs::remove_file(sample.root().join("door.txt")).expect("the link is there");
        crate::sample::symlink(&secret, sample.root().join("door.txt"));

        let output = tool.run(approved, &Unwatched).unwrap();
        assert!(output.is_failed(), "{}", output.text());
        assert!(
            !output.text().contains("nobody was asked"),
            "{}",
            output.text()
        );
    }

    #[test]
    fn an_outside_path_is_put_to_the_user_before_it_is_read() {
        use crucible_core::{Mode, Permission, Remember, Rules, Settled, ToolCall, Verdict};

        struct Counting(usize);
        impl crucible_core::Ask for Counting {
            fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
                self.0 += 1;
                (Verdict::Allow, Remember::Never)
            }
        }

        let sample = Sample::new("read-outside-asked");
        let outside = sample.outside("secret.txt", "classified");
        let tool = Read::new(sample.workspace(), Cancel::new(), Ledger::new());
        let call = ToolCall {
            id: crucible_core::ToolId::new("outside-1"),
            name: tool.name().into(),
            args: ToolArgs::new(format!(r#"{{"path":"{outside}"}}"#)),
        };

        let sensitivity = tool.sensitivity(&call.args);
        let mut counting = Counting(0);
        let settled = Permission::with(Mode::default(), Rules::new()).decide(
            &call,
            &sensitivity,
            &mut counting,
        );

        assert_eq!(
            counting.0, 1,
            "a read that leaves the workspace is asked about"
        );
        let Settled::Approved(approved) = settled else {
            panic!("the answer above was yes");
        };
        let output = tool.run(approved, &Unwatched).unwrap();
        assert!(output.text().contains("classified"));
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
    fn a_document_names_a_converter_that_is_actually_installed() {
        let said = conversion("report.docx", |program| {
            (program == "soffice").then(|| program.to_owned())
        })
        .expect("a docx is a document this knows");

        assert_eq!(
            said,
            ". It is a Word document \u{2014} convert it and read what comes out, \
             for example: soffice --headless --convert-to txt report.docx"
        );
    }

    #[test]
    fn the_first_converter_installed_is_the_one_named() {
        let said = conversion("report.docx", |program| Some(program.to_owned()))
            .expect("a docx is a document this knows");

        assert!(
            said.ends_with("pandoc report.docx --extract-media=converted-media -o converted.md"),
            "said: {said}"
        );
    }

    #[test]
    fn a_converter_that_extracts_says_the_pictures_can_be_attached() {
        let said = conversion("report.docx", |program| Some(program.to_owned()))
            .expect("a docx is a document this knows");

        assert_eq!(
            said,
            ". It is a Word document \u{2014} convert it and read what comes out, and its \
             pictures come out beside it into converted-media/ where each one can be \
             attached to a prompt and looked at, for example: pandoc report.docx \
             --extract-media=converted-media -o converted.md"
        );
    }

    #[test]
    fn a_converter_that_flattens_a_picture_promises_nothing_about_one() {
        let said = conversion("report.docx", |program| {
            (program == "soffice").then(|| program.to_owned())
        })
        .expect("a docx is a document this knows");

        assert!(!said.contains("converted-media"), "said: {said}");
        assert!(!said.contains("attached"), "said: {said}");
    }

    /// The sentence names a directory the command has to ask for, and the two
    /// are written in different places. Either without the other is a lie a
    /// reader only finds out about after running it.
    #[test]
    fn every_converter_that_extracts_asks_for_the_directory_the_sentence_names() {
        for document in UNREADABLE {
            for converter in document.converters {
                assert_eq!(
                    converter.extracts,
                    converter.arguments.contains(EXTRACTED_INTO),
                    "{} {}",
                    converter.program,
                    converter.arguments
                );
            }
        }
    }

    #[test]
    fn a_video_says_to_pull_frames_out_and_attach_them() {
        let said = conversion("clip.mp4", |program| Some(program.to_owned()))
            .expect("an mp4 is a file this knows");

        assert_eq!(
            said,
            ". It is a video \u{2014} nothing here reads one and everything here reads a \
             picture, so pull frames out and attach those. ffprobe -v error \
             -show_entries format=duration -of csv=p=0 clip.mp4 says how long it is, \
             and ffmpeg -i clip.mp4 -vf fps=1 -frames:v 20 frame-%03d.jpg takes one a \
             second and stops at twenty, which is about as many as one request \
             carries. Sample a long recording sparsely rather than sampling the start \
             of it."
        );
    }

    #[test]
    fn a_video_with_nothing_to_sample_it_names_the_one_program_to_install() {
        let said = conversion("clip.mkv", |_| None).expect("an mkv is a file this knows");

        assert_eq!(
            said,
            ". It is a video, and nothing installed here pulls frames out of one \
             \u{2014} ffmpeg would."
        );
    }

    /// Half a suggestion is worse than the next one down, and worse still than
    /// none: a rate picked without a duration is picked blind, and the model
    /// finds that out by filling a directory.
    #[test]
    fn a_converter_whose_measure_is_missing_is_not_suggested_at_all() {
        let said = conversion("clip.mov", |program| {
            (program == "ffmpeg").then(|| program.to_owned())
        })
        .expect("a mov is a file this knows");

        assert!(!said.contains("frames:v"), "said: {said}");
        assert!(said.ends_with("ffmpeg would."), "said: {said}");
    }

    /// The rate and the cap are the two numbers that decide whether running the
    /// suggestion is worth a turn, and both live in a template rather than in
    /// code. A row that lost either would still produce a fluent sentence, and
    /// the sentence would name a command that fills a directory.
    #[test]
    fn every_sampling_suggestion_carries_a_rate_a_cap_and_something_to_measure_with() {
        for known in UNREADABLE {
            if !matches!(known.becomes, Becomes::Pictures) {
                continue;
            }
            assert!(!known.converters.is_empty(), "{} has none", known.suffix);
            for converter in known.converters {
                let arguments = converter.arguments;
                assert!(arguments.contains("fps="), "{arguments}");
                assert!(arguments.contains("-frames:v "), "{arguments}");
                assert!(converter.measure.is_some(), "{}", converter.program);
            }
        }
    }

    #[test]
    fn a_document_with_nothing_to_convert_it_says_so_rather_than_a_command_that_fails() {
        let said = conversion("budget.xlsx", |_| None).expect("an xlsx is a document this knows");

        assert_eq!(
            said,
            ". It is a spreadsheet, and nothing installed here converts one \u{2014} \
             soffice or xlsx2csv would."
        );
    }

    #[test]
    fn the_suggestion_reads_the_name_whatever_case_it_is_written_in() {
        let said = conversion("BUDGET.XLSX", |program| {
            (program == "soffice").then(|| program.to_owned())
        })
        .expect("an extension is the same word however it is shouted");

        assert!(
            said.ends_with("soffice --headless --convert-to csv BUDGET.XLSX"),
            "said: {said}"
        );
    }

    #[test]
    fn a_name_that_says_nothing_is_offered_nothing() {
        assert!(conversion("core.dump", |program| Some(program.to_owned())).is_none());
        assert!(conversion("blob", |program| Some(program.to_owned())).is_none());
    }

    #[test]
    fn a_document_that_is_not_text_carries_its_next_move_into_the_answer() {
        let sample = Sample::new("read-document");
        sample.write_bytes("report.docx", &[0x50, 0x4b, 0x03, 0x04, 0x00, 0xff]);

        let output = read(&sample, r#"{"path":"report.docx"}"#);

        assert!(output.is_failed());
        assert!(
            output
                .text()
                .starts_with("report.docx is not a text file. It is a Word document"),
            "said: {}",
            output.text()
        );
    }

    /// The eight bytes that make a file a PNG, which is all `confirms` reads.
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    #[test]
    fn a_picture_is_handed_back_to_be_looked_at_rather_than_refused() {
        let sample = Sample::new("read-picture");
        sample.write_bytes("shot.png", PNG);

        let output = read(&sample, r#"{"path":"shot.png"}"#);

        assert!(!output.is_failed(), "{}", output.text());
        let [attached] = output.attachments() else {
            panic!("the picture did not come back: {}", output.text())
        };
        assert_eq!(attached.modality, Modality::Image);
        assert_eq!(attached.media_type.as_ref(), "image/png");
        assert!(attached.path.ends_with("shot.png"), "{}", attached.path);
        assert_eq!(
            output.text(),
            "shot.png is attached as an image rather than read as text."
        );
    }

    #[test]
    fn a_picture_handed_back_is_a_file_the_agent_has_now_seen() {
        // The same record a numbered read leaves. Being shown a file is being
        // shown it, and `write` asks this ledger and not how it was shown.
        let sample = Sample::new("read-picture-seen");
        sample.write_bytes("shot.png", PNG);
        let seen = Ledger::new();

        reading(&sample, r#"{"path":"shot.png"}"#, &seen);

        assert!(seen.holds(sample.workspace().existing("shot.png").unwrap().as_path()));
    }

    #[test]
    fn a_file_named_a_picture_that_holds_text_is_read_as_the_text_it_is() {
        // The name is what somebody meant, the bytes are what is there, and
        // where they disagree the bytes win — a guess from the name would be a
        // refusal about a file that reads perfectly well.
        let sample = Sample::new("read-picture-liar");
        sample.write("notes.png", "alpha\nbeta\n");

        let output = read(&sample, r#"{"path":"notes.png"}"#);

        assert!(!output.is_failed(), "{}", output.text());
        assert!(output.attachments().is_empty());
        assert!(output.text().contains("alpha"), "{}", output.text());
    }

    #[test]
    fn a_picture_too_large_to_carry_is_refused_before_it_is_read() {
        let sample = Sample::new("read-picture-large");
        let mut bytes = PNG.to_vec();
        bytes.resize(crucible_core::CEILING + 1, 0);
        sample.write_bytes("huge.png", &bytes);

        let output = read(&sample, r#"{"path":"huge.png"}"#);

        assert!(output.is_failed());
        assert_eq!(
            output.text(),
            "huge.png is larger than the 4 MB a request may carry, so it is not attached. A \
             smaller copy of it would be."
        );
    }

    #[test]
    fn a_pdf_is_still_told_what_would_turn_it_into_something_readable() {
        // Attachable at a prompt and deliberately not handed back here: the
        // sentence naming a converter works on every model crucible offers,
        // and an attachment does not.
        // A real one: the header, then bytes no decoder makes text of, which
        // is what sends it down the path that names a converter.
        let sample = Sample::new("read-pdf");
        sample.write_bytes("paper.pdf", &[b"%PDF-".as_slice(), &[0xff, 0xfe]].concat());

        let output = read(&sample, r#"{"path":"paper.pdf"}"#);

        assert!(output.is_failed());
        assert!(output.attachments().is_empty());
        assert!(output.text().contains("PDF"), "{}", output.text());
    }

    #[test]
    fn a_binary_nobody_can_name_gets_the_refusal_and_no_guess() {
        let sample = Sample::new("read-unnamed");
        sample.write_bytes("core.dump", &[0xff, 0xfe, 0x00, 0x01]);

        let output = read(&sample, r#"{"path":"core.dump"}"#);

        assert!(output.is_failed());
        assert_eq!(output.text(), "core.dump is not a text file");
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
        let problem = tool.run(allowed(&tool, "{}"), &Unwatched).unwrap_err();

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

    #[test]
    fn the_schema_declares_what_the_parser_reads_with_the_bounds_the_code_holds() {
        let sample = Sample::new("read-schema");
        let tool = Read::new(sample.workspace(), Cancel::new(), Ledger::new());
        let value: serde_json::Value = serde_json::from_str(tool.schema()).unwrap();
        let at = |path: &str| value.pointer(path).expect(path);

        assert_eq!(at("/type"), "object");
        assert_eq!(at("/required"), &serde_json::json!(["path"]));
        let named: Vec<&String> = at("/properties").as_object().unwrap().keys().collect();
        assert_eq!(
            named,
            ["limit", "offset", "path"].iter().collect::<Vec<_>>()
        );
        assert_eq!(at("/properties/offset/minimum"), 1);
        assert_eq!(at("/properties/limit/minimum"), 1);

        // The default, the ceiling and the byte cut are stated where the model
        // reads them, spelled by the constants the code holds them with.
        let told = at("/properties/limit/description").as_str().unwrap();
        assert!(told.contains(&LINES.to_string()), "{told}");
        assert!(told.contains(&CEILING.to_string()), "{told}");
        assert!(told.contains(&OUTPUT.to_string()), "{told}");
    }
}
