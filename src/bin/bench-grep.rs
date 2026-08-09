//! The grep budget: the `grep` tool within 1.25× of the `rg` binary.
//!
//! A ratio rather than a duration, because the number that matters is not how
//! fast this machine is. `rg` is the fastest thing anyone will compare this
//! against, so measuring against it on the same tree, in the same cache state,
//! answers the only question worth asking.
//!
//! The corpus is generated rather than taken from the repository: the budget
//! must mean the same thing on a machine that has just cloned this and on one
//! that has been building in it for a week.

use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crucible_core::{
    Approved, Ask, Permission, Remember, Sensitivity, Settled, Tool, ToolArgs, ToolCall, ToolId,
    Verdict, Workspace,
};
use crucible_tools::Grep;

/// How far over `rg` the tool may be.
const LIMIT: f64 = 1.25;

/// Directories in the generated tree.
const DIRS: usize = 40;

/// Files in each of them.
const FILES: usize = 50;

/// Lines in each file.
const LINES: usize = 120;

/// What both tools search for. Rare enough that the time goes into scanning
/// rather than into printing.
const PATTERN: &str = "fn assemble_widget";

/// Measurements per tool. The smallest is kept: a slower run had interference
/// in it, and the fastest is the one that measures the code.
const ROUNDS: usize = 5;

fn main() -> Result<(), Problem> {
    let corpus = Corpus::build()?;

    // Warm the page cache once for each, so the first measured run is not the
    // one paying for every read.
    ours(&corpus)?;
    theirs(&corpus)?;

    let mine = fastest(ROUNDS, || ours(&corpus))?;
    let rg = fastest(ROUNDS, || theirs(&corpus))?;

    let ratio = mine.as_secs_f64() / rg.as_secs_f64();

    writeln!(io::stdout(), "{ratio:.2} x {LIMIT}")?;
    writeln!(
        io::stderr(),
        "         grep {:.1} ms, rg {:.1} ms, over {} files",
        mine.as_secs_f64() * 1000.0,
        rg.as_secs_f64() * 1000.0,
        DIRS * FILES
    )?;

    if ratio > LIMIT {
        return Err(Problem::Over { ratio });
    }

    Ok(())
}

/// The shortest of `rounds` runs.
fn fastest(
    rounds: usize,
    mut run: impl FnMut() -> Result<Duration, Problem>,
) -> Result<Duration, Problem> {
    let mut best = Duration::MAX;
    for _ in 0..rounds {
        best = best.min(run()?);
    }
    Ok(best)
}

/// One search through the tool, timed.
fn ours(corpus: &Corpus) -> Result<Duration, Problem> {
    let workspace = Workspace::open(corpus.path())?;
    let grep = Grep::new(workspace);
    let args = ToolArgs::new(format!(r#"{{"pattern":"{PATTERN}","limit":100000}}"#));

    let started = Instant::now();
    let output = grep.run(approved(&grep, args)?)?;
    let took = started.elapsed();

    if output.is_failed() {
        return Err(Problem::NoMatch {
            who: "grep",
            saw: output.text().into(),
        });
    }

    Ok(took)
}

/// One search through `rg`, timed.
///
/// Includes the process start, because that is what a user pays when they run
/// `rg` themselves — and it is a cost this tool does not have, so counting it
/// makes the comparison harder on us rather than easier.
fn theirs(corpus: &Corpus) -> Result<Duration, Problem> {
    let started = Instant::now();
    let run = Command::new("rg")
        .arg("--line-number")
        .arg("--no-heading")
        .arg(PATTERN)
        .arg(corpus.path())
        .output()
        .map_err(|source| Problem::NoRipgrep { source })?;
    let took = started.elapsed();

    if run.stdout.is_empty() {
        return Err(Problem::NoMatch {
            who: "rg",
            saw: String::from_utf8_lossy(&run.stderr).into_owned().into(),
        });
    }

    Ok(took)
}

/// The call, permitted the only way one can be. A read is allowed without
/// asking, so nothing is ever put to a user who is not there.
fn approved(grep: &Grep, args: ToolArgs) -> Result<Approved, Problem> {
    struct Nobody;

    impl Ask for Nobody {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            (Verdict::Deny, Remember::Never)
        }
    }

    let call = ToolCall {
        id: ToolId::new("bench"),
        name: grep.name().into(),
        args,
    };

    match Permission::new().decide(&call, &grep.sensitivity(&call.args), &mut Nobody) {
        Settled::Approved(approved) => Ok(approved),
        Settled::Forbidden | Settled::Refused => Err(Problem::NoGrant),
    }
}

/// A tree to search, removed when the probe ends.
struct Corpus {
    root: PathBuf,
}

impl Corpus {
    /// Writes the tree, with the pattern in one file per directory so both
    /// tools have something to report and neither can stop early.
    fn build() -> Result<Self, Problem> {
        let root = std::env::temp_dir().join(format!("crucible-bench-grep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        for dir in 0..DIRS {
            let here = root.join(format!("crate{dir}/src"));
            fs::create_dir_all(&here)?;

            for file in 0..FILES {
                let wanted = file == FILES / 2;
                fs::write(here.join(format!("part{file}.rs")), body(dir, file, wanted))?;
            }
        }

        Ok(Self { root })
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Corpus {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One file's worth of plausible source. Varied per file so the whole corpus
/// is not one page the operating system can serve from a single mapping.
fn body(dir: usize, file: usize, wanted: bool) -> String {
    let mut text = String::with_capacity(LINES * 48);

    for line in 0..LINES {
        if wanted && line == LINES / 2 {
            let _ = writeln!(text, "pub {PATTERN}_{dir}_{file}() -> usize {{ {line} }}");
        } else {
            let _ = writeln!(
                text,
                "pub fn helper_{dir}_{file}_{line}(input: usize) -> usize {{ input + {line} }}"
            );
        }
    }

    text
}

/// Why the budget could not be reported.
#[derive(thiserror::Error)]
enum Problem {
    #[error("the grep tool is {ratio:.2}x rg, over the {LIMIT}x budget")]
    Over { ratio: f64 },

    #[error("{who} found nothing to time: {saw}")]
    NoMatch { who: &'static str, saw: Box<str> },

    #[error("rg is not on PATH, and the grep budget is measured against it")]
    NoRipgrep { source: io::Error },

    #[error("a read-only call was refused a grant")]
    NoGrant,

    #[error("the corpus could not be written: {0}")]
    Corpus(#[from] io::Error),

    #[error("the workspace could not be opened: {0}")]
    Workspace(#[from] crucible_core::PathError),

    #[error("the search failed: {0}")]
    Search(#[from] crucible_core::ToolError),
}

// A `main` that returns `Err` prints the `Debug` form, and the derived one
// buries the sentence that says what to do — `NoRipgrep { source: Os { code: 2,
// .. } }` where the point is "rg is not on PATH". The messages above are
// written to be read, so `Debug` shows them.
impl fmt::Debug for Problem {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{self}")
    }
}
