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
//!
//! The search is timed twice, because a walk costs different things depending
//! on whether anything was written about the tool. With no rules the check each
//! walked file goes through answers on the spot; with rules it resolves that
//! file's path first. Both are what somebody runs, so both are measured, and
//! the ratio reported is whichever of the two is worse.
//!
//! Every side is timed many times and the fastest run of each is what the ratio
//! divides. A search that took longer had something else on the machine in it,
//! and the fastest run is the one that timed the code. The rounds interleave
//! for the same reason: what a ratio must not do is move with the load.

use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crucible_core::{
    Approved, Ask, Disposition, Mode, Permission, Remember, RuleError, Rules, Sensitivity, Settled,
    Tool, ToolArgs, ToolCall, ToolId, Verdict, Workspace,
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

/// Rounds per side. The fastest of them is the measurement.
///
/// Thirty is where the reported ratio stops moving. Repeating this probe on one
/// machine, the ratio varies by about a seventh of itself at ten rounds and a
/// tenth at thirty, and the next tenth costs as much again — while the round
/// that turns out to hold a side's fastest time lands past the fifteenth more
/// often than not, so a smaller budget stops short of the floor rather than
/// finding it early.
///
/// Keeping the fastest is also why no warm-up round is run and thrown away. A
/// minimum already ignores a first run that paid for a cold page cache, without
/// having to be told which run that was — and measured here the first run was
/// not reliably the slow one, so discarding it deliberately would have been
/// paying for a correction that had nothing to correct.
const ROUNDS: usize = 30;

/// Rules about `grep`, written the way a project writes them: a directory, a
/// suffix, and one file by name.
///
/// None of them names the root the search starts from — a rule that did would
/// forbid the call outright, and what is wanted is a call that runs with rules
/// standing behind it. Three of the four name nothing in the corpus, which is
/// the point: the cost being measured is asking about every walked file, not
/// leaving some of them out.
const DENIED: [&str; 4] = [
    "grep(**/vendor/**)",
    "grep(**/target/**)",
    "grep(**/*.lock)",
    "grep(crate7/src/part13.rs)",
];

fn main() -> Result<(), Problem> {
    let corpus = Corpus::build()?;
    let (open, ruled, rg) = rounds(&corpus)?;

    // The worse of the two against the budget. A figure taken on the cheaper
    // path is a figure nobody with a rule written down is held to.
    let ratio = open.ratio(rg).max(ruled.ratio(rg));

    writeln!(io::stdout(), "{ratio:.2} x {LIMIT}")?;
    writeln!(
        io::stderr(),
        "         grep {open} with no rules, {ruled} with {} rules, rg {rg}, \
         best of {ROUNDS} interleaved rounds over {} files",
        DENIED.len(),
        DIRS * FILES
    )?;

    if ratio > LIMIT {
        return Err(Problem::Over { ratio });
    }

    Ok(())
}

/// Every side, `ROUNDS` times, a round at a time rather than a side at a time.
///
/// Interleaving is what makes comparing the fastest runs fair. Timing all of
/// one side and then all of another puts the whole of one block of work between
/// the two numbers the ratio divides, so whatever else the machine was doing
/// during one block is charged to that side alone — and the ratio then moves
/// with the load instead of with the code. A round that times all three sides
/// in turn hands them the same machine, so interference lands on both sides of
/// the division and mostly cancels: measured here, each side's fastest time
/// varies across repeats of this probe several times as much as the ratio built
/// out of them does.
fn rounds(corpus: &Corpus) -> Result<(Spread, Spread, Spread), Problem> {
    let mut open = Spread::new();
    let mut ruled = Spread::new();
    let mut rg = Spread::new();

    for _ in 0..ROUNDS {
        open.saw(ours(corpus, false)?);
        ruled.saw(ours(corpus, true)?);
        rg.saw(theirs(corpus)?);
    }

    Ok((open, ruled, rg))
}

/// What one side's rounds came to.
///
/// The fastest is the measurement. The slowest is carried alongside it because
/// a red build is read by somebody who was not there: the two together say
/// whether the machine was quiet while the number was taken, and a slowest
/// several times the fastest is the first thing to suspect before the code.
#[derive(Clone, Copy)]
struct Spread {
    fastest: Duration,
    slowest: Duration,
}

impl Spread {
    fn new() -> Self {
        Self {
            fastest: Duration::MAX,
            slowest: Duration::ZERO,
        }
    }

    fn saw(&mut self, took: Duration) {
        self.fastest = self.fastest.min(took);
        self.slowest = self.slowest.max(took);
    }

    /// How this side compares with another, fastest against fastest.
    fn ratio(self, other: Self) -> f64 {
        self.fastest.as_secs_f64() / other.fastest.as_secs_f64()
    }
}

impl fmt::Display for Spread {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "{:.1} ms (slowest {:.1})",
            self.fastest.as_secs_f64() * 1000.0,
            self.slowest.as_secs_f64() * 1000.0
        )
    }
}

/// One search through the tool, timed, with rules standing behind it or not.
fn ours(corpus: &Corpus, ruled: bool) -> Result<Duration, Problem> {
    let workspace = Workspace::open(corpus.path())?;
    let grep = Grep::new(workspace);
    let args = ToolArgs::new(format!(r#"{{"pattern":"{PATTERN}","limit":100000}}"#));

    // Read outside the clock. What the two runs are being compared on is what a
    // walk pays per file, and reading four patterns once is neither.
    let mut engine = engine(ruled)?;

    let started = Instant::now();
    let output = grep.run(approved(&grep, args, &mut engine)?)?;
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

/// The engine one run decides through: nothing written down, or the four rules
/// above.
///
/// The difference is not cosmetic. A grant carrying no rule about the tool
/// answers the walk's per-file question the moment it is asked; one carrying a
/// rule has to work out what that file is called first, for every file the walk
/// reaches. An empty set therefore never reaches that code at all.
fn engine(written: bool) -> Result<Permission, Problem> {
    if !written {
        return Ok(Permission::new());
    }

    let mut rules = Rules::new();
    for text in DENIED {
        rules.add(Disposition::Deny, text)?;
    }

    Ok(Permission::with(Mode::default(), rules))
}

/// The call, permitted the only way one can be. A read is allowed without
/// asking, so nothing is ever put to a user who is not there.
fn approved(grep: &Grep, args: ToolArgs, engine: &mut Permission) -> Result<Approved, Problem> {
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

    match engine.decide(&call, &grep.sensitivity(&call.args), &mut Nobody) {
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

    #[error("a rule this probe is written with could not be read: {0}")]
    Rule(#[from] RuleError),

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
