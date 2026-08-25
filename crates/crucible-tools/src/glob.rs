//! Finding files by the shape of their path.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::SystemTime;

use crucible_core::{
    Approved, Cancel, Sensitivity, Summary, Tool, ToolArgs, ToolError, ToolOutput, Watch, Workspace,
};
use globset::GlobBuilder;

use std::sync::LazyLock;

use crate::args::Args;
use crate::bound::OUTPUT;
use crate::schema::{Field, Schema, Shape, Whole};
use crate::summary;
use crate::target;

/// The name the model calls.
const NAME: &str = "glob";

/// The glob to match.
const PATTERN: &str = "pattern";

/// Where to search — and, as an answer to [`SORT`], the alphabetical order.
/// One spelling, two questions, which is the model's vocabulary rather than a
/// collision here.
const PATH: &str = "path";

/// How many paths to give.
const LIMIT: &str = "limit";

/// What order to answer in.
const SORT: &str = "sort";

/// The other answer to [`SORT`]: most recently changed first.
const MODIFIED: &str = "modified";

/// How many paths one call answers with when it does not say.
const PATHS: usize = 200;

/// The most paths a call can ask for, however large a number it sends.
///
/// [`PATHS`] is a default and a caller may raise it; this it cannot. The number
/// arrives from the model like every other argument, and it is the one figure
/// in the call that decides how much the walk holds — a heap of that many
/// strings, kept for as long as the tree takes to finish.
///
/// A thousand is where the count stops being the bound that matters. Paths in a
/// real tree run to some thirty bytes, so a thousand of them is already the
/// whole of what an answer carries and [`crate::bound`] is what cuts the rest.
/// A listing that long is also not a listing anybody reads: past this the model
/// wants a narrower pattern, not a longer answer.
const CEILING: usize = 1_000;

/// The root `description` is the tool's own; everything below it describes the
/// arguments. Every ceiling is spelled by the constant the code holds it
/// with, so the sentence the model reads cannot drift from the bound the call
/// meets.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Lists files in the workspace whose path matches a glob. Skips anything \
                gitignored."
            .into(),
        fields: vec![
            Field {
                name: PATTERN,
                about: "The glob to match, for example **/*.rs or src/**/mod.rs.".into(),
                needed: true,
                shape: Shape::Text,
            },
            Field {
                name: PATH,
                about: "A directory to search under, relative to the workspace root. Defaults to \
                        the whole workspace."
                    .into(),
                needed: false,
                shape: Shape::Text,
            },
            Field {
                name: LIMIT,
                about: format!(
                    "How many paths to return. Defaults to {PATHS}, and never more than \
                     {CEILING} however large a number is sent. The answer is cut at {OUTPUT} \
                     bytes as well, whichever comes first."
                ),
                needed: false,
                shape: Shape::Count(Whole {
                    least: 1,
                    most: Some(CEILING),
                }),
            },
            Field {
                name: SORT,
                about: format!(
                    "What order to answer in, and therefore which paths a limit keeps. {PATH} is \
                     alphabetical and the default; {MODIFIED} is most recently changed first, for \
                     finding what a project has been working on."
                ),
                needed: false,
                shape: Shape::Choice(&[PATH, MODIFIED]),
            },
        ],
    }
    .text()
});

/// Finds files in the workspace by path.
#[derive(Debug)]
pub struct Glob {
    workspace: Workspace,
    cancel: Cancel,
}

/// What order an answer is in, and therefore which paths a limit keeps.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Sort {
    /// Alphabetical, and what a call gets when it does not say.
    Path,
    /// Most recently changed first.
    Modified,
}

impl Sort {
    /// What the walk has to learn about a file to place it.
    ///
    /// Under [`Sort::Path`] nothing: the name is already in hand and the
    /// modification time would be a `stat` per match spent on a figure that
    /// decides nothing. So the cost of the second order is paid only by the
    /// calls that ask for it.
    ///
    /// A file whose time cannot be read is dated to the epoch rather than
    /// dropped, which places it last among files that have one. That is the
    /// safe direction: it is a file that vanished mid-walk or one on a
    /// filesystem that keeps no times, and either way putting it at the top of
    /// an answer about recent work would be the wrong claim.
    fn aged(self, entry: &ignore::DirEntry) -> SystemTime {
        match self {
            Self::Path => SystemTime::UNIX_EPOCH,
            Self::Modified => entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH),
        }
    }

    /// What a walk the user stopped no longer promises about its answer.
    fn lowest(self) -> &'static str {
        match self {
            Self::Path => "the lowest paths it reached, not the lowest there are",
            Self::Modified => "the newest paths it reached, not the newest there are",
        }
    }
}

/// One matched path and what places it, ordered worst first.
///
/// Worst first is what lets one heap serve both orders: the top is always the
/// entry the next match displaces, and `into_sorted_vec` hands them back in the
/// order they are reported. `age` is reversed so that an older file sorts
/// greater — the oldest is the one a newer match pushes out — and under
/// [`Sort::Path`] every entry carries the same epoch, where it decides nothing
/// and the path below it is the whole order.
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Ranked {
    age: Reverse<SystemTime>,
    path: String,
}

/// What one walk came back with: the best paths it matched, and how many it
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
    /// The best paths matched so far, worst at the top — the next one to fall
    /// out when a better one arrives. Never longer than `limit`.
    kept: BinaryHeap<Ranked>,
    limit: usize,
    seen: usize,
    /// Whether the user stopped the turn while the walk was running, which
    /// costs the answer the one property it is otherwise sorted for: these are
    /// then the best paths reached rather than the best in the tree.
    stopped: bool,
}

impl Found {
    /// Room for `limit` paths and no more.
    fn new(limit: usize) -> Self {
        Self {
            kept: BinaryHeap::new(),
            limit,
            seen: 0,
            stopped: false,
        }
    }

    /// Takes one matched path, keeping it only if it belongs in the answer.
    fn keep(&mut self, ranked: Ranked) {
        self.seen += 1;

        if self.kept.len() < self.limit {
            self.kept.push(ranked);
            return;
        }

        // The top is the worst entry in hand, so it is the one this displaces.
        // `pop` before `push` is what holds the heap at `limit` rather than
        // letting it grow by one on every match after the first `limit`.
        if self.kept.peek().is_some_and(|worst| *worst > ranked) {
            self.kept.pop();
            self.kept.push(ranked);
        }
    }

    /// The paths, in the order they are reported: best first.
    fn sorted(self) -> Vec<String> {
        self.kept
            .into_sorted_vec()
            .into_iter()
            .map(|ranked| ranked.path)
            .collect()
    }

    /// How many matched that the answer does not carry.
    fn more(&self) -> usize {
        self.seen.saturating_sub(self.kept.len())
    }
}

impl Glob {
    /// Searches inside `workspace` and nowhere else, and stops when `cancel`
    /// says to.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self { workspace, cancel }
    }
}

impl Tool for Glob {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA.as_str()
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: target::searched(&self.workspace, NAME, args, PATH),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, PATTERN)
    }

    fn run(&self, approved: Approved, _watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let pattern = args.text(PATTERN)?;
        let limit = args.count(LIMIT, PATHS)?.min(CEILING);
        let sort = match args.choice(SORT, PATH, &[PATH, MODIFIED])? {
            MODIFIED => Sort::Modified,
            _ => Sort::Path,
        };

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

        let requested = args.optional_text(PATH)?.unwrap_or(".");
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
        //
        // Which is exactly why the user has to be able to stop it: running to
        // the end is a promise about the answer, not about how long a tree may
        // take. A stopped walk keeps what it had and says what that cost.
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
            if self.cancel.requested() {
                found.stopped = true;
                break;
            }

            // Matched against the name it will be reported under, which is the
            // one `grep` reports too. Dropping every entry that would not strip
            // to a relative path answered "no path matched" for a directory the
            // workspace was widened to reach and `grep` searched happily.
            let shown = crate::tree::named(&self.workspace, entry.path());
            if glob.is_match(&shown) {
                found.keep(Ranked {
                    age: Reverse(sort.aged(&entry)),
                    path: shown,
                });
            }
        }

        Ok(report(found, pattern, sort))
    }
}

/// The paths, one per line.
fn report(found: Found, pattern: &str, sort: Sort) -> ToolOutput {
    // Read before the paths are taken out of it, while it can still tell a
    // walk that matched nothing from one whose answer is full.
    let matched = found.more();
    let note = halted(found.stopped, sort);
    let shown = found.sorted();

    if shown.is_empty() {
        // The note belongs on this branch too: "no path matched" is a claim
        // about a tree that was walked to the end.
        return ToolOutput::failed(format!("no path matched {pattern}{note}"));
    }

    // Bounded in bytes as well as in paths, because the length of a path is
    // the depth of the tree and not something this tool sets: a thousand of
    // them is thirty kilobytes in this repository and four megabytes in one
    // written to make it so.
    let (lines, over) = crate::bound::within(shown.into_iter().map(|path| path + "\n"));
    let more = matched + over;

    let tail = if more == 0 {
        String::new()
    } else if over > 0 {
        // Raising the limit would not help: what filled up is the answer, not
        // the list.
        format!(
            "\n[{more} more: the answer was full at {} bytes, narrow the pattern]",
            crate::bound::OUTPUT
        )
    } else {
        format!("\n[{more} more: narrow the pattern or raise limit]")
    };

    ToolOutput::ok(lines + &tail + &note)
}

/// What a walk the user stopped says about itself.
///
/// It answers rather than failing: the paths it had are paths, and they are
/// what the turn was spent on. What goes with them is the property they lost —
/// the answer is sorted, so it reads as the best paths in the tree, and a walk
/// that stopped short only reached some of the tree to be best in. Which
/// property that is depends on what was asked for, which is why the sentence
/// comes from the sort rather than being written here.
fn halted(stopped: bool, sort: Sort) -> String {
    if stopped {
        format!(
            "\n[stopped before the walk finished: these are {}]",
            sort.lowest()
        )
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::Unwatched;

    use super::*;
    use crucible_core::Disposition;

    use crate::sample::{Sample, allowed, under};

    fn glob(sample: &Sample, args: &str) -> ToolOutput {
        let tool = Glob::new(sample.workspace(), Cancel::new());
        tool.run(allowed(&tool, args), &Unwatched).unwrap()
    }

    /// One entry the way `Sort::Path` makes them: every age the same, so the
    /// path below it is the whole order.
    fn ranked(path: &str) -> Ranked {
        Ranked {
            age: Reverse(SystemTime::UNIX_EPOCH),
            path: path.to_owned(),
        }
    }

    /// One entry the way `Sort::Modified` makes them.
    fn dated(path: &str, seconds: u64) -> Ranked {
        Ranked {
            age: Reverse(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
            path: path.to_owned(),
        }
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

    /// Gives a file a modification time, so a test can say what order the tree
    /// is in rather than hoping the filesystem's clock separated three writes.
    fn aged(sample: &Sample, at: &str, seconds: u64) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(sample.root().join(at))
            .expect("a file the sample wrote");
        file.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
        )
        .expect("a filesystem that keeps modification times");
    }

    #[test]
    fn asking_for_modified_answers_newest_first() {
        // The order a model wants when it is looking for what changed, and the
        // one it cannot get by asking for paths and reading them.
        let sample = tree("glob-modified");
        aged(&sample, "src/main.rs", 3_000);
        aged(&sample, "src/cli/parse.rs", 1_000);
        aged(&sample, "README.md", 2_000);

        let output = glob(&sample, r#"{"pattern":"**/*","sort":"modified"}"#);

        assert_eq!(
            output.text(),
            "src/main.rs\nREADME.md\nsrc/cli/parse.rs\n",
            "{}",
            output.text()
        );
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
            found.keep(ranked(&format!("src/file{n:04}.rs")));
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
    fn keeping_the_newest_files_answers_the_same_as_sorting_all_of_them_by_time() {
        // The same promise the path order owes, under the order that reverses
        // the key: a bounded heap that displaces the wrong end keeps the oldest
        // files and still looks like a sorted answer.
        let dates: Vec<(String, u64)> = (0..500)
            .map(|n| (format!("f{n:03}.rs"), (n * 37) % 500))
            .collect();

        let mut found = Found::new(7);
        for (path, seconds) in dates.clone() {
            found.keep(dated(&path, seconds));
        }

        let mut newest = dates;
        newest.sort_unstable_by(|(left, one), (right, two)| two.cmp(one).then(left.cmp(right)));
        newest.truncate(7);
        assert_eq!(
            found.sorted(),
            newest.into_iter().map(|(path, _)| path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_files_changed_at_the_same_moment_still_come_back_in_a_settled_order() {
        // A tree checked out in one go has one timestamp on every file, which
        // is the common case rather than a corner: without a tiebreak the
        // answer would depend on the order the walk reached them.
        let mut found = Found::new(3);
        for path in ["c.rs", "a.rs", "b.rs"] {
            found.keep(dated(path, 1_000));
        }

        assert_eq!(found.sorted(), vec!["a.rs", "b.rs", "c.rs"]);
    }

    #[test]
    fn a_stopped_walk_says_what_the_order_it_was_asked_for_no_longer_promises() {
        // The sentence is about the property the answer lost, so an answer
        // sorted by time that says "not the lowest there are" is describing a
        // walk somebody else ran.
        let mut found = Found::new(5);
        found.keep(dated("src/main.rs", 1_000));
        found.stopped = true;

        let output = report(found, "**/*.rs", Sort::Modified);

        assert!(
            output.text().contains("not the newest there are"),
            "{}",
            output.text()
        );
    }

    #[test]
    fn a_sort_nobody_offers_is_refused_and_the_words_that_are_offered_are_named() {
        let sample = tree("glob-sort-unknown");
        let tool = Glob::new(sample.workspace(), Cancel::new());

        let problem = tool
            .run(
                allowed(&tool, r#"{"pattern":"**/*","sort":"size"}"#),
                &Unwatched,
            )
            .unwrap_err();

        assert_eq!(
            problem.to_string(),
            "glob: sort must be one of path, modified"
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
            found.keep(ranked(&path));
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
    fn a_limit_past_the_ceiling_is_the_ceiling() {
        // A number the model wrote is an argument like any other. Left
        // unbounded it decides how many strings the walk holds and how long an
        // answer the next request carries.
        let sample = Sample::new("glob-ceiling");
        for n in 0..CEILING + 50 {
            sample.write(&format!("src/f{n:04}.rs"), "");
        }

        let output = glob(&sample, r#"{"pattern":"**/*.rs","limit":100000}"#);

        assert_eq!(
            output
                .text()
                .lines()
                .filter(|line| line.starts_with("src/"))
                .count(),
            CEILING
        );
        assert!(output.text().contains("[50 more"), "{}", output.text());
    }

    #[test]
    fn an_answer_is_bounded_in_bytes_as_well_as_in_paths() {
        // The length of a path is the depth of the tree, which is the model's
        // to decide: a thousand of them is thirty kilobytes here and megabytes
        // in a tree written to make it so.
        let sample = Sample::new("glob-bytes");
        for n in 0..150 {
            sample.write(&format!("src/{}{n:03}.rs", "long".repeat(60)), "");
        }

        let output = glob(&sample, r#"{"pattern":"**/*.rs","limit":100000}"#);

        assert!(
            output.text().len() < crate::bound::OUTPUT + 200,
            "the answer was {} bytes",
            output.text().len()
        );
        assert!(
            output.text().contains("the answer was full"),
            "{}",
            output.text()
        );
    }

    #[test]
    fn a_walk_the_user_stopped_never_reaches_the_files() {
        // The walk deliberately runs to the end even once the answer is full,
        // so that it can report the lowest paths rather than the first ones
        // reached — which made it the one thing here Esc could not stop.
        let sample = tree("glob-stopped");
        let cancel = Cancel::new();
        cancel.request();

        let tool = Glob::new(sample.workspace(), cancel);
        let output = tool
            .run(allowed(&tool, r#"{"pattern":"**/*.rs"}"#), &Unwatched)
            .unwrap();

        assert!(output.is_failed());
        assert!(
            output.text().starts_with("no path matched"),
            "{}",
            output.text()
        );
        assert!(
            output.text().contains("stopped before the walk finished"),
            "{}",
            output.text()
        );
    }

    #[test]
    fn a_walk_the_user_stopped_answers_with_the_paths_it_had() {
        // Failing would throw away paths the turn was already spent finding.
        // What goes with them is the property they lost: they are the lowest
        // paths the walk reached rather than the lowest in the tree.
        let mut found = Found::new(5);
        found.keep(ranked("src/main.rs"));
        found.stopped = true;

        let output = report(found, "**/*.rs", Sort::Path);

        assert!(!output.is_failed());
        assert!(
            output.text().starts_with("src/main.rs\n"),
            "{}",
            output.text()
        );
        assert!(
            output.text().contains("not the lowest there are"),
            "{}",
            output.text()
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

        let tool = Glob::new(sample.workspace(), Cancel::new());
        let output = tool
            .run(
                under(
                    &tool,
                    r#"{"pattern":"**/*.rs"}"#,
                    &[(Disposition::Deny, "glob(private/**)")],
                ),
                &Unwatched,
            )
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
        let listing = Glob::new(workspace.clone(), Cancel::new());
        let listed = listing
            .run(
                allowed(
                    &listing,
                    &format!(r#"{{"pattern":"**/*.md","path":"{beside}"}}"#),
                ),
                &Unwatched,
            )
            .unwrap();

        let search = crate::Grep::new(workspace, Cancel::new());
        let searched = search
            .run(
                allowed(
                    &search,
                    &format!(r#"{{"pattern":"needle","path":"{beside}"}}"#),
                ),
                &Unwatched,
            )
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
        let tool = Glob::new(sample.workspace(), Cancel::new());

        let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"src"}"#));

        assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
        assert_eq!(sensitivity.to_string(), "read src");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_out_of_the_workspace_is_not_listed() {
        // The same property `grep` holds, for the same reason: the walk does
        // not follow links, so a link's own type is not a file and the entry
        // never reaches the answer. A path listed here is one the model will
        // hand back to `read`, so listing one that leads outside would be
        // offering it a way out.
        let sample = Sample::new("glob-link-out");
        let secret = sample.outside("secret.txt", "");
        crate::sample::symlink(&secret, sample.root().join("innocent.txt"));
        sample.write("real.txt", "");

        let output = glob(&sample, r#"{"pattern":"**/*.txt"}"#);

        assert!(output.text().contains("real.txt"), "{}", output.text());
        assert!(
            !output.text().contains("innocent"),
            "a link out of the workspace was listed: {}",
            output.text()
        );
    }
}
