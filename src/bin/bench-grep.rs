//! The grep budget: the tool's worst paired median within 1.25× of `rg`.
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
//! Three workloads cover the shapes that change a search: a rare match where
//! walking dominates, no match where the whole tree must be disproved, and
//! high output where collecting and formatting matter. Each is timed with and
//! without permission rules.
//!
//! Ratios are paired inside every round. Crucible and `rg` see the same workload
//! and adjacent machine state, and their order alternates so neither always gets
//! the warmer cache. The median owns the budget; p95 and its distance from the
//! median are emitted as evidence of noise rather than hidden by independent
//! best-case samples.

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

/// Rare enough that the time goes into scanning rather than printing.
const RARE: &str = "fn assemble_widget";

/// Present once in every file, so output collection is part of the reading.
const DENSE: &str = "fn streamed_result";

/// Absent by construction, so both implementations have to disprove it.
const ABSENT: &str = "fn widget_that_does_not_exist";

/// Twenty makes p95 the second-worst paired reading while keeping the complete
/// three-workload matrix under a few seconds on the CI corpus.
const ROUNDS: usize = 20;

/// Every shape of search this budget covers.
const WORKLOADS: [Workload; 3] = [
    Workload {
        name: "rare-match",
        pattern: RARE,
        expected: Expected::Matches,
    },
    Workload {
        name: "no-match",
        pattern: ABSENT,
        expected: Expected::None,
    },
    Workload {
        name: "high-output",
        pattern: DENSE,
        expected: Expected::Matches,
    },
];

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
    let mut worst_median = 0.0_f64;
    let mut worst_p95 = 0.0_f64;
    let mut widest = 0.0_f64;

    for workload in WORKLOADS {
        let evidence = rounds(&corpus, workload)?;
        worst_median = worst_median
            .max(evidence.open.median)
            .max(evidence.ruled.median);
        worst_p95 = worst_p95.max(evidence.open.p95).max(evidence.ruled.p95);
        widest = widest
            .max(evidence.open.dispersion)
            .max(evidence.ruled.dispersion);

        writeln!(
            io::stderr(),
            "         {:11} no rules {}, with {} rules {}",
            workload.name,
            evidence.open,
            DENIED.len(),
            evidence.ruled,
        )?;
    }

    writeln!(
        io::stdout(),
        "{worst_median:.2} x {LIMIT} p95={worst_p95:.2} dispersion={widest:.1}"
    )?;
    writeln!(
        io::stderr(),
        "         {ROUNDS} paired rounds per workload over {} files",
        DIRS * FILES,
    )?;

    if worst_median > LIMIT {
        return Err(Problem::Over {
            ratio: worst_median,
        });
    }

    Ok(())
}

/// One generated search shape and what its result should look like.
#[derive(Debug, Clone, Copy)]
struct Workload {
    name: &'static str,
    pattern: &'static str,
    expected: Expected,
}

#[derive(Debug, Clone, Copy)]
enum Expected {
    Matches,
    None,
}

/// Both permission paths through one workload.
#[derive(Debug, Clone, Copy)]
struct Evidence {
    open: Stats,
    ruled: Stats,
}

/// A paired ratio distribution.
#[derive(Debug, Clone, Copy)]
struct Stats {
    median: f64,
    p95: f64,
    /// Percentage by which p95 stands above the median.
    dispersion: f64,
}

impl Stats {
    fn from(mut ratios: Vec<f64>) -> Result<Self, Problem> {
        ratios.sort_by(f64::total_cmp);
        let median = percentile(&ratios, 50)?;
        let p95 = percentile(&ratios, 95)?;
        Ok(Self {
            median,
            p95,
            dispersion: (p95 / median - 1.0).max(0.0) * 100.0,
        })
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "median {:.2}x, p95 {:.2}x, +{:.0}%",
            self.median, self.p95, self.dispersion
        )
    }
}

/// Paired readings, with order alternating each round.
fn rounds(corpus: &Corpus, workload: Workload) -> Result<Evidence, Problem> {
    // Fill page caches and worker pools before the distribution starts.
    let _ = ours(corpus, workload, false)?;
    let _ = ours(corpus, workload, true)?;
    let _ = theirs(corpus, workload)?;

    let mut open = Vec::with_capacity(ROUNDS);
    let mut ruled = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let ours_first = round % 2 == 0;
        open.push(paired(corpus, workload, false, ours_first)?);
        ruled.push(paired(corpus, workload, true, !ours_first)?);
    }

    Ok(Evidence {
        open: Stats::from(open)?,
        ruled: Stats::from(ruled)?,
    })
}

fn paired(
    corpus: &Corpus,
    workload: Workload,
    ruled: bool,
    ours_first: bool,
) -> Result<f64, Problem> {
    let (mine, reference) = if ours_first {
        (ours(corpus, workload, ruled)?, theirs(corpus, workload)?)
    } else {
        (theirs(corpus, workload)?, ours(corpus, workload, ruled)?)
    };
    if mine.is_zero() || reference.is_zero() {
        return Err(Problem::Clock);
    }
    Ok(if ours_first {
        mine.as_secs_f64() / reference.as_secs_f64()
    } else {
        reference.as_secs_f64() / mine.as_secs_f64()
    })
}

fn percentile(values: &[f64], percent: usize) -> Result<f64, Problem> {
    let rank = values.len().saturating_mul(percent).div_ceil(100).max(1) - 1;
    values.get(rank).copied().ok_or(Problem::NoReadings)
}

/// One search through the tool, timed, with rules standing behind it or not.
fn ours(corpus: &Corpus, workload: Workload, ruled: bool) -> Result<Duration, Problem> {
    let workspace = Workspace::open(corpus.path())?;
    let grep = Grep::new(workspace, crucible_core::Cancel::new());
    let args = ToolArgs::new(format!(
        r#"{{"pattern":"{}","limit":100000}}"#,
        workload.pattern
    ));

    // Read outside the clock. What the two runs are being compared on is what a
    // walk pays per file, and reading four patterns once is neither.
    let mut engine = engine(ruled)?;

    let started = Instant::now();
    let output = grep.run(approved(&grep, args, &mut engine)?)?;
    let took = started.elapsed();

    let expected = match workload.expected {
        Expected::Matches => !output.is_failed() && !output.text().is_empty(),
        Expected::None => output.is_failed() && output.text().starts_with("nothing matched"),
    };
    if !expected {
        return Err(Problem::Unexpected {
            who: "grep",
            workload: workload.name,
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
fn theirs(corpus: &Corpus, workload: Workload) -> Result<Duration, Problem> {
    let started = Instant::now();
    let run = Command::new("rg")
        .arg("--line-number")
        .arg("--no-heading")
        .arg(workload.pattern)
        .arg(corpus.path())
        .output()
        .map_err(|source| Problem::NoRipgrep { source })?;
    let took = started.elapsed();

    let expected = match workload.expected {
        Expected::Matches => run.status.success() && !run.stdout.is_empty(),
        Expected::None => run.status.code() == Some(1) && run.stdout.is_empty(),
    };
    if !expected {
        return Err(Problem::Unexpected {
            who: "rg",
            workload: workload.name,
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
    /// Writes rare, absent and high-output patterns into one stable tree.
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
            let _ = writeln!(text, "pub {RARE}_{dir}_{file}() -> usize {{ {line} }}");
        } else if line == LINES / 3 {
            let _ = writeln!(text, "pub {DENSE}_{dir}_{file}() -> usize {{ {line} }}");
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
    #[error("the grep tool's worst median is {ratio:.2}x rg, over the {LIMIT}x budget")]
    Over { ratio: f64 },

    #[error("{who} returned the wrong result for {workload}: {saw}")]
    Unexpected {
        who: &'static str,
        workload: &'static str,
        saw: Box<str>,
    },

    #[error("the benchmark clock returned a zero-duration search")]
    Clock,

    #[error("no paired grep readings were collected")]
    NoReadings,

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
