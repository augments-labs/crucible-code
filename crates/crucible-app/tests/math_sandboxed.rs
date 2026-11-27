//! The TeX engine on this machine's own sandbox, with this machine's own TeX.
//!
//! Each test reads the host first, without asking the check it tests: TeX is
//! present when `latex` and `dvisvgm` are on the `PATH` and `/etc/texmf` and
//! `/var/lib/texmf` exist, and the backend is whatever the local sandbox's own
//! `probe()` says, its claims compared with what the math policy needs. With
//! both, a test runs its TeX-present half and requires the engine; without
//! them, it requires the check to give the first absence in the order the
//! check meets it. It prints which half it ran, so neither half is a skip.
//!
//! The TeX-present half runs hostile formulas through the real engine: the
//! escape inputs under `tests/math/escape/` with their sentinels, a file-size
//! bomb, a disk filled across many files, writes to `/dev` and `/dev/shm`, a
//! loop, a picture too large to read, and formulas a shell would read as
//! commands. Each is answered within the engine's own deadlines, and nothing
//! outside the sandbox changes.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "a fixture that cannot be made, or a check that fails, is the test's failure, and the half a test ran is printed for the retained run; the clippy setting that allows these reaches only the functions marked as tests, not the helpers beside them"
)]

use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crucible_app::math::{
    COMMAND_TIME, MARGIN, MathPictures, STAGING, Svg, TexEngine, Unavailable, Unmade, available,
};
use crucible_sandbox::{
    SandboxCapability, SandboxError, SandboxFeature, SandboxService, SandboxViolation,
};
use crucible_sandbox_local::LocalSandbox;
use tokio::runtime::Runtime;

/// Where the escape inputs and their sentinels are kept.
const ESCAPE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/math/escape");

/// The variable a parent sets for the child it runs to stay silent.
const CHILD: &str = "CRUCIBLE_MATH_CHILD_REPORT";

/// One binary's tests start confined commands one at a time, so a test never
/// meets the per-user ceiling another test in the same binary holds.
fn alone() -> MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// The host, read without the check.

/// What the math policy needs of the backend, in the order the sandbox's own
/// requirement list puts them, each with its minimum. Spelled here, not read
/// from the sandbox, so that a change to either disagrees with the other.
fn needs() -> Vec<(SandboxFeature, SandboxCapability)> {
    let enforced = SandboxCapability::Enforced;
    let mut needs = vec![
        (SandboxFeature::Filesystem, enforced),
        (SandboxFeature::DescriptorIsolation, enforced),
        (SandboxFeature::ProcessIsolation, enforced),
        (SandboxFeature::KernelSurface, enforced),
        (SandboxFeature::PrivilegeIsolation, enforced),
        (SandboxFeature::NetworkDeny, enforced),
    ];
    // The confining limits every policy starts from hold a CPU limit except
    // on macOS and an open-file limit except on Windows.
    if cfg!(not(target_os = "macos")) {
        needs.push((SandboxFeature::CpuLimit, enforced));
    }
    needs.extend([
        (SandboxFeature::MemoryLimit, enforced),
        (SandboxFeature::DiskLimit, enforced),
        (SandboxFeature::ProcessLimit, enforced),
    ]);
    if cfg!(not(target_os = "windows")) {
        needs.push((SandboxFeature::OpenFileLimit, enforced));
    }
    needs.extend([
        (SandboxFeature::CommandTimeLimit, enforced),
        (SandboxFeature::OutputLimit, enforced),
        (SandboxFeature::Audit, enforced),
        (SandboxFeature::Usage, SandboxCapability::Observed),
    ]);
    needs
}

/// Whether `program` is a file in one of the `PATH`'s directories.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
    })
}

/// What this host should make of the check, read in the order the check runs.
#[derive(Debug, Clone, Copy)]
enum Reading {
    /// TeX and a backend that holds every limit are here: the engine.
    Present,
    /// The first absence the check meets.
    Absent(Unavailable),
}

fn reading(runtime: &Runtime, service: &LocalSandbox) -> Reading {
    if !Path::new("/etc/texmf").is_dir() || !Path::new("/var/lib/texmf").is_dir() {
        return Reading::Absent(Unavailable::ProgramMissing);
    }
    let capabilities = match runtime.block_on(service.probe()) {
        Ok((_, capabilities)) => capabilities,
        Err(SandboxError::BackendUnavailable { reason }) => {
            // The same switch the sandbox's own admission tests honour: where
            // a backend is required, its absence is a failure, not a half.
            assert!(
                std::env::var_os("CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX").is_none(),
                "the math engine's tests require a backend here: {reason}"
            );
            return Reading::Absent(Unavailable::NoBackend);
        }
        Err(other) => panic!("sandbox probe: {other}"),
    };
    if let Some((feature, _)) = needs()
        .into_iter()
        .find(|(feature, minimum)| capabilities.claim(*feature) < *minimum)
    {
        return Reading::Absent(Unavailable::Refused(feature));
    }
    if !on_path("latex") || !on_path("dvisvgm") {
        return Reading::Absent(Unavailable::ProgramMissing);
    }
    Reading::Present
}

/// A runtime, the service a conversation would hand the check, and the
/// host's reading.
struct Here {
    runtime: Runtime,
    service: Arc<dyn SandboxService>,
    reading: Reading,
}

impl Here {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let local = LocalSandbox::new().watching_on(runtime.handle().clone());
        let reading = reading(&runtime, &local);
        Self {
            runtime,
            service: Arc::new(local),
            reading,
        }
    }

    fn checked(&self) -> Result<TexEngine, Unavailable> {
        self.runtime
            .block_on(available(Arc::clone(&self.service), COMMAND_TIME))
    }

    fn made(&self, engine: &TexEngine, formula: &str) -> Result<Svg, Unmade> {
        self.runtime.block_on(engine.make(formula))
    }
}

/// Which half ran, with the engine where it was the present one.
enum Half {
    Present(TexEngine),
    Absent(Unavailable),
}

/// The check on this host, held to the host's reading.
fn half(here: &Here) -> Half {
    match (here.reading, here.checked()) {
        (Reading::Present, Ok(engine)) => Half::Present(engine),
        (Reading::Present, Err(why)) => {
            panic!("TeX and a backend that holds every limit are here, yet the check says {why:?}")
        }
        (Reading::Absent(expected), Err(why)) => {
            assert_eq!(
                why, expected,
                "the check's answer is not the host's first absence"
            );
            Half::Absent(why)
        }
        (Reading::Absent(expected), Ok(engine)) => {
            panic!("the host lacks what {expected:?} names, yet the check gave {engine:?}")
        }
    }
}

/// Runs `present` with the engine where the host has TeX and a backend, and
/// otherwise holds the check to the host's first absence; prints which.
fn halves(name: &str, present: impl FnOnce(&Here, &TexEngine)) {
    let _alone = alone();
    let here = Here::new();
    match half(&here) {
        Half::Present(engine) => {
            println!("{name}: ran the TeX-present half ({})", engine.toolchain());
            present(&here, &engine);
        }
        Half::Absent(why) => {
            println!("{name}: ran the TeX-absent half: {why:?}, so every formula stays text");
        }
    }
}

// ---------------------------------------------------------------------------
// Formulas and the host around them.

/// A directory of the host's own, outside every sandbox, removed on drop.
struct Outside(PathBuf);

impl Outside {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "crucible-math-outside-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("a directory outside the sandbox");
        Self(path)
    }

    /// Every file in it, by name, with its bytes.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(&self.0)
            .expect("the outside directory")
            .map(|entry| {
                let entry = entry.expect("an entry");
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).expect("an outside file"),
                )
            })
            .collect();
        files.sort();
        files
    }
}

impl Drop for Outside {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A formula that writes `lines` lines of 78 bytes to `name`, then has
/// `last` written as its last line.
fn writing(name: &str, lines: usize, last: &str) -> String {
    format!(
        r"\immediate\openout15={name} \count255=0 \loop\immediate\write15{{{line}}}\advance\count255 by 1 \ifnum\count255<{lines} \repeat\immediate\write15{{{last}}}\immediate\closeout15 ",
        line = r"\noexpand\relax".repeat(11),
    )
}

/// What a file a formula wrote ends with, so that reading it back defines
/// `\held` only if all of it was kept.
const HELD: &str = r"\noexpand\global\noexpand\let\noexpand\held\noexpand\relax";

/// A formula that reads `name` back and draws only if its last line came
/// through, so a refused or cut write leaves the formula as text.
fn read_back(name: &str) -> String {
    format!(r"\input {name} \ifdefined\held x\else\errmessage{{not held}}\fi")
}

/// How many 78-byte lines make `bytes`.
const fn lines_of(bytes: usize) -> usize {
    bytes / 78
}

const MIB: usize = 1024 * 1024;

/// The escape inputs, sorted by name, with `outside` put in for `@OUTSIDE@`.
fn escapes(outside: &Path) -> Vec<(String, String)> {
    let mut inputs: Vec<_> = fs::read_dir(ESCAPE)
        .expect("the escape inputs")
        .filter_map(|entry| {
            let path = entry.expect("an entry").path();
            (path.extension()? == "tex").then(|| {
                let text = fs::read_to_string(&path).expect("an escape input");
                (
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    text.replace("@OUTSIDE@", &outside.to_string_lossy())
                        .replace("@JOB@", "/tmp")
                        .trim()
                        .to_owned(),
                )
            })
        })
        .collect();
    inputs.sort();
    inputs
}

/// The paths escape inputs name for a write on the host itself, which must
/// not exist after them. None may exist before.
const HOST_WRITES: [&str; 1] = ["/tmp/t34-escape-written.txt"];

/// What no picture may hold: each sentinel's text, and the first line of the
/// host's `/etc/passwd`.
const SECRETS: [&str; 4] = [
    "SENTINEL-OUTSIDE-9b4e",
    "SENTINEL-IN-ROOT-71c2",
    "SENTINEL-DOT-55d0",
    "root:x:0:0",
];

/// Holds `picture`, if one was drawn, to holding nothing of [`SECRETS`].
fn holds_no_secret(name: &str, made: &Result<Svg, Unmade>) {
    let Ok(picture) = made else {
        return;
    };
    let text = String::from_utf8_lossy(picture.bytes());
    for secret in SECRETS {
        assert!(!text.contains(secret), "{name}'s picture holds {secret}");
    }
}

/// The escape inputs that try to read, each of which must stay text.
///
/// Three of them, `03` and `06` reading `../secret-in-root.txt` and `04`
/// reading `.dotsecret`, name paths relative to the run's working directory,
/// the sandbox's own `/tmp`. That directory and the root above it are a
/// fresh scratch each run, which no host path reaches, so no sentinel can be
/// planted where they read; they are kept as T34 wrote them, and what holds
/// TeX's reads to its own tree is asserted by the `\openin` formulas of
/// `tex_reads_nothing_of_the_host_and_writes_nothing_above_its_directory`.
const READS: [&str; 9] = [
    "01-read-abs-visible.tex",
    "02-read-abs-outside.tex",
    "03-read-dotdot.tex",
    "04-read-dotfile.tex",
    "05-read-csname-string.tex",
    "06-read-expandafter-built.tex",
    "07-read-openin-built.tex",
    "19-ps-special-read.tex",
    "20-pdfximage-read.tex",
];

/// What each escape input may come to on this engine: `None` for a picture,
/// else the reason it stays text. The retained run of the TeX-present half
/// prints which one it was.
///
/// Exact, because any other answer may be the escape the row tries:
///
/// - the reads, `01` to `07`, `19` and `20`, and the writes above TeX's
///   directory, `09` and `10`;
/// - `12` and `loopback-pipe`, where `Empty` could be a piped command's
///   output read back as the formula;
/// - `14`, whose deadline is the bound it tests, and `16`, stopped when its
///   log reaches the `ulimit -f` bound: a `Timeout` would be that bound not
///   holding;
/// - `18`, whose `%` starts a comment in every TeX, so its special never
///   closes; `Empty` would be a special that reached `dvisvgm`.
///
/// The rest accept what a TeX build may differ in, none of it a way out:
///
/// - `08` writes into the sandbox's own `/tmp`, which `TEXMFOUTPUT` names. A
///   kpathsea that lets it through sets nothing, one that refuses an absolute
///   path stops, and [`HOST_WRITES`] holds the host's `/tmp` to never seeing
///   it.
/// - `11`, `13` and `loopback-write18` ask for `\write18`, which with shell
///   escape off goes to the log and sets nothing, or stops a TeX that treats
///   it as an error. The restricted pipe after them, the loopback listener
///   and the sentinels show that nothing ran.
/// - `15` grows until TeX's main memory, a size the build's `texmf.cnf` sets,
///   runs out, or until the deadline; either stops it.
/// - `17` and `21` to `25` are large rules, a large glyph and a tall formula,
///   drawn here. A TeX or `dvisvgm` that refuses one, or draws nothing of it
///   for want of a font's outlines, leaves it as text, which is where every
///   formula falls back to; [`holds_no_secret`] reads every picture drawn.
///
/// No row accepts a `Limit`, the output bound standing in for what the row
/// tests, or `NotReached`, a formula no TeX read, which shows nothing.
const ANSWERS: [(&str, &[Option<Unmade>]); 27] = [
    ("01-read-abs-visible.tex", &[Some(Unmade::Rejected)]),
    ("02-read-abs-outside.tex", &[Some(Unmade::Rejected)]),
    ("03-read-dotdot.tex", &[Some(Unmade::Rejected)]),
    ("04-read-dotfile.tex", &[Some(Unmade::Rejected)]),
    ("05-read-csname-string.tex", &[Some(Unmade::Rejected)]),
    ("06-read-expandafter-built.tex", &[Some(Unmade::Rejected)]),
    ("07-read-openin-built.tex", &[Some(Unmade::Rejected)]),
    (
        "08-write-abs.tex",
        &[Some(Unmade::Empty), Some(Unmade::Rejected)],
    ),
    ("09-write-dotdot.tex", &[Some(Unmade::Rejected)]),
    ("10-write-dotfile.tex", &[Some(Unmade::Rejected)]),
    (
        "11-shell-write18.tex",
        &[Some(Unmade::Empty), Some(Unmade::Rejected)],
    ),
    ("12-shell-pipe-input.tex", &[Some(Unmade::Rejected)]),
    (
        "13-network-wget.tex",
        &[Some(Unmade::Empty), Some(Unmade::Rejected)],
    ),
    ("14-loop-forever.tex", &[Some(Unmade::Timeout)]),
    (
        "15-memory-typeset-growth.tex",
        &[Some(Unmade::Rejected), Some(Unmade::Timeout)],
    ),
    ("16-log-bomb.tex", &[Some(Unmade::Rejected)]),
    ("17-size-bomb-rule.tex", DRAWN_OR_TEXT),
    ("18-ps-special-pipe.tex", &[Some(Unmade::Rejected)]),
    ("19-ps-special-read.tex", &[Some(Unmade::Empty)]),
    ("20-pdfximage-read.tex", &[Some(Unmade::Rejected)]),
    ("21-size-bomb-moderate.tex", DRAWN_OR_TEXT),
    ("22-memory-pixel-bomb.tex", DRAWN_OR_TEXT),
    ("23-memory-pixel-bomb-large.tex", DRAWN_OR_TEXT),
    ("24-size-bomb-glyph.tex", DRAWN_OR_TEXT),
    ("25-tall-formula.tex", DRAWN_OR_TEXT),
    (
        "loopback-write18",
        &[Some(Unmade::Empty), Some(Unmade::Rejected)],
    ),
    ("loopback-pipe", &[Some(Unmade::Rejected)]),
];

/// A large but harmless formula: drawn, or refused, or drawn as nothing.
const DRAWN_OR_TEXT: &[Option<Unmade>] = &[None, Some(Unmade::Rejected), Some(Unmade::Empty)];

// ---------------------------------------------------------------------------
// The check.

#[test]
fn the_check_on_this_host_is_what_the_hosts_own_reading_says() {
    halves("check", |here, engine| {
        let picture = here.made(engine, r"\frac{a}{b} = \sqrt{x^2 + 1}");
        let Ok(picture) = picture else {
            panic!("an ordinary formula is not drawn: {picture:?}");
        };
        assert!(picture.bytes().starts_with(b"<?xml"), "{picture:?}");
    });
}

// ---------------------------------------------------------------------------
// Hostile formulas.

#[test]
fn every_escape_input_leaves_its_sentinels_and_the_host_unchanged() {
    halves("escape", |here, engine| {
        let outside = Outside::new();
        for sentinel in ["secret.txt", "secret-in-root.txt", ".dotsecret"] {
            fs::copy(
                Path::new(ESCAPE).join("sentinels").join(sentinel),
                outside.0.join(sentinel),
            )
            .expect("a sentinel");
        }
        let sentinels = outside.files();
        assert_eq!(sentinels.len(), 3, "{sentinels:?}");
        for path in HOST_WRITES {
            assert!(!Path::new(path).exists(), "{path} exists before the run");
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let port = listener.local_addr().expect("its address").port();
        let mut inputs = escapes(&outside.0);
        assert_eq!(inputs.len(), 25, "an escape input went missing");
        inputs.extend([
            (
                "loopback-write18".to_owned(),
                format!(
                    r"\csname immediate\endcsname\csname write\endcsname18{{/usr/bin/wget -T 3 -t 1 -O- http://127.0.0.1:{port}/}}"
                ),
            ),
            (
                "loopback-pipe".to_owned(),
                format!(r#"\csname input\endcsname|"/usr/bin/wget -T 3 -t 1 -O- http://127.0.0.1:{port}/""#),
            ),
        ]);
        let names: Vec<&str> = inputs.iter().map(|(name, _)| name.as_str()).collect();
        let listed: Vec<&str> = ANSWERS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, listed, "an input has no expected answer");
        let mut wrong = Vec::new();
        for ((name, formula), (_, expected)) in inputs.iter().zip(ANSWERS) {
            let start = Instant::now();
            let made = here.made(engine, formula);
            let took = start.elapsed();
            println!("  {name}: {} in {took:?}", answer(&made));
            holds_no_secret(name, &made);
            if READS.contains(&name.as_str()) {
                assert!(made.is_err(), "the read {name} was drawn");
            }
            if !expected.contains(&made.as_ref().err().copied()) {
                wrong.push((name.clone(), answer(&made), expected));
            }
            assert!(
                took < STAGING + COMMAND_TIME + MARGIN,
                "{name} took {took:?}, past every deadline"
            );
            assert_eq!(outside.files(), sentinels, "{name} changed a sentinel");
            for path in HOST_WRITES {
                assert!(!Path::new(path).exists(), "{name} wrote {path} on the host");
            }
        }
        assert!(
            wrong.is_empty(),
            "answered otherwise than expected: {wrong:#?}"
        );
        match listener.accept() {
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            accepted => panic!("the loopback listener was reached: {accepted:?}"),
        }

        // The sandbox holds every sentinel whatever TeX runs, so they cannot
        // tell whether shell escape is off. A program TeX's restricted shell
        // escape would still run, read back as the formula, can: off, the
        // pipe is a file name that is not there and the formula stays text.
        let piped = here.made(
            engine,
            r#"\csname input\endcsname|"kpsewhich -var-value=openout_any""#,
        );
        println!("  restricted pipe: {}", answer(&piped));
        assert!(piped.is_err(), "a restricted shell escape ran");
    });
}

/// How a formula was answered, for the retained run.
fn answer(made: &Result<Svg, Unmade>) -> String {
    match made {
        Ok(picture) => format!("drawn, {} bytes", picture.bytes().len()),
        Err(why) => format!("text, {why:?}"),
    }
}

#[test]
fn a_file_past_the_file_size_bound_ends_the_run() {
    halves("file size", |here, engine| {
        // A megabyte in one file is drawn, so what stops three is the bound
        // on one file, which lies between them, not the disk's 8 MiB.
        let within = format!(
            "{}{}",
            writing("one.txt", lines_of(MIB), HELD),
            read_back("one.txt")
        );
        let made = here.made(engine, &within);
        assert!(made.is_ok(), "a megabyte in one file: {made:?}");
        let past = format!(
            "{}{}",
            writing("bomb.txt", lines_of(3 * MIB), HELD),
            read_back("bomb.txt")
        );
        let made = here.made(engine, &past);
        assert!(made.is_err(), "three megabytes in one file were drawn");
        println!("  three megabytes in one file: {}", answer(&made));
    });
}

#[test]
fn many_files_under_the_file_bound_stop_at_the_disk_bound_and_dev_is_refused() {
    halves("disk", |here, engine| {
        // Every file is 1.5 MiB, under the bound on one. Four of them, 6 MiB,
        // are all kept; six, 9 MiB, never are: the last one's last line,
        // read back, is never there, so the bytes held never pass 8 MiB.
        let lines = lines_of(MIB + MIB / 2);
        let files = |count: usize| -> String {
            let mut formula = String::new();
            for file in 1..count {
                formula.push_str(&writing(&format!("part{file}.txt"), lines, "x"));
            }
            formula.push_str(&writing(&format!("part{count}.txt"), lines, HELD));
            formula.push_str(&read_back(&format!("part{count}.txt")));
            formula
        };
        let within = here.made(engine, &files(4));
        assert!(within.is_ok(), "6 MiB in four files: {within:?}");
        let past = here.made(engine, &files(6));
        assert!(past.is_err(), "9 MiB in six files were all kept");
        println!("  9 MiB in six files: {}", answer(&past));

        let shared = format!("crucible-math-written-{}.txt", std::process::id());
        let host_shm = Path::new("/dev/shm").join(&shared);
        for target in [
            "/written-root.txt".to_owned(),
            "/crucible-home/written-home.txt".to_owned(),
            "/dev/written-dev.txt".to_owned(),
            format!("/dev/shm/{shared}"),
        ] {
            let formula = format!("{}{}", writing(&target, 1, HELD), read_back(&target));
            let made = here.made(engine, &formula);
            assert!(made.is_err(), "a write to {target} was read back");
            println!("  a write to {target}: {}", answer(&made));
            assert!(
                !host_shm.exists(),
                "a write reached the host's {}",
                host_shm.display()
            );
            assert!(
                !Path::new("/written-root.txt").exists(),
                "a write reached the host's /written-root.txt"
            );
        }
    });
}

#[test]
fn tex_reads_nothing_of_the_host_and_writes_nothing_above_its_directory() {
    halves("guards", |here, engine| {
        // TeX is told to open files only beneath its working directory, so
        // an absolute path is never opened: the file reads as at its end and
        // the formula is drawn. Opened, it would stop the formula.
        for host in ["/etc/passwd", "/proc/self/status"] {
            let made = here.made(
                engine,
                &format!(r"\newread\f \openin\f={host} \ifeof\f x\else\errmessage{{read}}\fi"),
            );
            println!("  an \\openin of {host}: {}", answer(&made));
            assert!(made.is_ok(), "TeX opened {host}: {}", answer(&made));
            holds_no_secret(host, &made);
        }

        // It is told to write only beneath its working directory: a file
        // there is written and read back, and one above it is refused.
        let within = here.made(
            engine,
            &format!("{}{}", writing("w.txt", 1, HELD), read_back("w.txt")),
        );
        println!("  a write to w.txt: {}", answer(&within));
        assert!(
            within.is_ok(),
            "a write beneath TeX's directory: {}",
            answer(&within)
        );
        let above = here.made(engine, &writing("../w.txt", 1, "x"));
        println!("  a write to ../w.txt: {}", answer(&above));
        assert_eq!(
            above.as_ref().err(),
            Some(&Unmade::Rejected),
            "a write above TeX's directory: {}",
            answer(&above)
        );

        // A raw special alone makes no picture whether specials pass or not:
        // `-b min` sizes the picture by what TeX set, and a special sets
        // nothing.
        let raw = here.made(
            engine,
            r#"\special{dvisvgm:raw <rect width="50" height="50"/>}"#,
        );
        println!("  a raw rectangle alone: {}", answer(&raw));
        assert_eq!(
            raw.as_ref().err(),
            Some(&Unmade::Empty),
            "a raw rectangle alone: {}",
            answer(&raw)
        );
        // `dvisvgm` is told to pass no special through, so SVG a formula
        // writes for it is never in the picture: beside `x`, which is drawn,
        // this group would be.
        let injected = here.made(engine, r#"x\special{dvisvgm:raw <g id="injected"/>}"#);
        println!("  a raw group beside x: {}", answer(&injected));
        let Ok(picture) = injected else {
            panic!("x beside a raw group is not drawn: {}", answer(&injected));
        };
        assert!(
            !String::from_utf8_lossy(picture.bytes()).contains("injected"),
            "a raw group reached the picture"
        );
    });
}

#[test]
fn a_loop_is_killed_at_its_deadline_and_stays_text() {
    halves("loop", |here, engine| {
        let start = Instant::now();
        let made = here.made(engine, r"\def\a{\a}\a");
        let took = start.elapsed();
        assert_eq!(
            made.as_ref().err(),
            Some(&Unmade::Timeout),
            "{}",
            answer(&made)
        );
        assert!(took >= COMMAND_TIME, "ended in {took:?}, before its time");
        assert!(
            took < STAGING + COMMAND_TIME + MARGIN,
            "ended in {took:?}, past every deadline"
        );
        println!("  the loop ended in {took:?}");
    });
}

#[test]
fn a_picture_past_what_is_read_of_it_stops_the_run_as_incomplete() {
    halves("output", |here, engine| {
        // Six thousand glyphs, each drawn back over the last, are a picture
        // whose every glyph is an element: far past the 128 KiB read of it.
        let made = here.made(
            engine,
            r"\count255=0 \loop x\kern-5pt\advance\count255 by 1 \ifnum\count255<6000 \repeat",
        );
        assert_eq!(
            made.as_ref().err(),
            Some(&Unmade::Limit(SandboxViolation::Output)),
            "{}",
            answer(&made)
        );
    });
}

#[test]
fn shell_words_and_a_leading_dash_reach_tex_as_the_formula() {
    halves("shell", |here, engine| {
        // Each is drawn, so each reached TeX as the formula it is: neither the
        // script that runs TeX nor TeX's own options read it.
        for formula in [
            "-shell-escape x",
            "--version",
            "a;b|c<d>e",
            "x' ; exit 0 ; echo '",
            "`exit 0` \\backslash n",
        ] {
            let made = here.made(engine, formula);
            assert!(made.is_ok(), "{formula:?}: {}", answer(&made));
        }
        // A formula of the script's own words is TeX's to refuse.
        let made = here.made(engine, "$(exit 0)\n'\\]'\n\\end{document}\nexit 0");
        println!("  the script's own words: {}", answer(&made));
    });
}

// ---------------------------------------------------------------------------
// Crucible's own output.

/// Every line libtest itself writes for the one child test, and nothing else.
fn only_the_harness_printed(stdout: &str) -> bool {
    stdout.lines().all(|line| {
        line.is_empty()
            || line == "running 1 test"
            || line == "test child_makes_formulas ... ok"
            || line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;")
    })
}

#[test]
fn the_engine_writes_nothing_to_crucibles_own_output() {
    let _alone = alone();
    let outside = Outside::new();
    let report = outside.0.join("child");
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--ignored",
            "--exact",
            "child_makes_formulas",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, &report)
        .output()
        .expect("a child process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    let ran = fs::read_to_string(&report).expect("the child's report");
    println!("output: the child ran {ran}");
    assert!(
        ran == "the TeX-absent half" || ran == "three formulas",
        "the child did not run: {ran:?}"
    );
    assert!(
        stderr.is_empty() && only_the_harness_printed(&stdout),
        "stdout: {stdout:?}, stderr: {stderr:?}"
    );
}

#[test]
#[ignore = "run by its parent, in a child process whose output is read"]
fn child_makes_formulas() {
    let Some(report) = std::env::var_os(CHILD) else {
        return;
    };
    let _alone = alone();
    let here = Here::new();
    let ran = match half(&here) {
        Half::Present(engine) => {
            for formula in [
                r"x^2",
                r"\undefined",
                r"\count255=0 \loop x\kern-5pt\advance\count255 by 1 \ifnum\count255<6000 \repeat",
            ] {
                let _ = here.made(&engine, formula);
            }
            "three formulas"
        }
        Half::Absent(_) => "the TeX-absent half",
    };
    fs::write(report, ran).expect("the child's report");
}
