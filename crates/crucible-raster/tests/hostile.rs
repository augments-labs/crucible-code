//! The hostile corpus: files that are bombs, truncations, noise and nested
//! garbage, and SVGs that point outside themselves.
//!
//! Every entry of `tests/corpus/` is drawn or refused as the table below
//! records, inside a deadline, without a panic. The references are
//! the other half: an SVG that names a file, a URL, a style sheet or a font is
//! drawn as if the reference were not there, and what it names is a sentinel
//! that records being touched. A sentinel that records nothing is only worth
//! something once a stand-in that does touch it has made it record, so each
//! planted stand-in below is run against the same checks and must be caught.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a fixture that cannot be made, or a check that fails, is the test's failure; the clippy setting that allows both reaches only the functions marked as tests, not the helpers beside them"
)]

use crucible_raster::{
    MOST_DECODED, MOST_PNG, MOST_SIXEL_WIDTH, Pixels, Refused, SixelPalette, decode, png,
    rasterise, sixel,
};
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");

/// Long enough for the slowest entry in a debug build on a loaded machine, and
/// short against a hang.
const DEADLINE: Duration = Duration::from_mins(1);

const WIDTH: u32 = 200;
const INKS: &[(&str, [u8; 3])] = &[("ink", [20, 40, 60])];
const PALETTE: SixelPalette = SixelPalette {
    ground: [0, 0, 0],
    colours: 256,
};

/// The decode slot is the process's, and a wait for it is bounded: tests that
/// draw take turns rather than make each other wait out the bound.
fn alone() -> MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// The runner: a panic and a hang are findings, not the test's own end.

#[derive(Debug, PartialEq, Eq)]
enum Ended<T> {
    Returned(T),
    Panicked,
    Hung,
}

/// `work` on a thread of its own, given `deadline` to answer. A thread that
/// outlives its deadline is left behind: nothing can stop it.
fn guarded<T: Send + 'static>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Ended<T> {
    let (answer, heard) = mpsc::channel();
    thread::spawn(move || {
        // A send that fails means the runner has already given up.
        let _ = answer.send(work());
    });
    match heard.recv_timeout(deadline) {
        Ok(value) => Ended::Returned(value),
        Err(RecvTimeoutError::Timeout) => Ended::Hung,
        Err(RecvTimeoutError::Disconnected) => Ended::Panicked,
    }
}

#[test]
fn a_planted_panic_is_found_by_the_runner() {
    let ended = guarded(Duration::from_secs(5), || -> u8 {
        panic!("planted: a parser that panics")
    });
    assert_eq!(ended, Ended::Panicked);
}

#[test]
fn a_planted_unbounded_loop_is_found_by_the_runner() {
    let begun = Instant::now();
    let ended = guarded(Duration::from_millis(300), move || {
        // Unbounded as far as the runner can tell. It ends itself after a few
        // seconds, so a runner that fails to cut it off cannot hang the suite.
        while begun.elapsed() < Duration::from_secs(4) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    assert_eq!(ended, Ended::Hung);
}

#[test]
fn a_planted_wait_that_ends_is_not_a_finding() {
    let ended = guarded(Duration::from_secs(5), || 7_u8);
    assert_eq!(ended, Ended::Returned(7));
}

// ---------------------------------------------------------------------------
// The corpus.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Drawn,
    Refused(Refused),
}

use Expect::{Drawn, Refused as No};

/// Each file of the corpus and what becomes of it. A file added without a row,
/// or a row left without its file, fails the check beside this table.
///
/// The 800 by 800 pictures are `Refused::TooLarge`: they are 2 560 000 bytes
/// of pixels, past `MOST_DECODED`. So are `many-results.svg` and the three
/// `filter-work-*.svg` files, by the other half of that ceiling: each asks for
/// a filter region so large that the pixmaps it would hold are past it before
/// any step of work is priced. The `form-*.svg` files are
/// `Refused::NotAnImage`, each for the one element outside those drawn that
/// it holds, and so are the `probe-*.svg` files that hold a mask, a style
/// sheet, a reference to text or an image. Thirteen more `probe-*.svg` files
/// are `Refused::NotAnImage` for what they say: an arc, a circle and, through
/// font sizes in `em` and `inherit`, a circle, each with a radius past the
/// largest size a number may have; a filter image or a gradient that names an
/// element; an id given twice; a chain of nested views, and one of uses of
/// symbols, each sized in a percentage of the one around it; and circles whose
/// radius is a percentage of a view the outermost `<svg>` sizes in a
/// percentage, or a `<use>` sizes for the symbol it draws, past that largest
/// size. The fourteen `work-*.svg` files, `worst.svg` and the other twelve
/// `probe-*.svg` files are refused by the work drawing them would take and by
/// nothing else: the two `work-copied-*.svg` and two `work-nested-uses*.svg`
/// files, and the probes of copies, by the copies the parser would make of
/// what their uses, markers, clips, text and gradient stops name; the four
/// `probe-stroked-*.svg` files by the outlines the parser would build of their
/// strokes, one of them by the outline it builds again of a stroke the page
/// turns, at its size on the page; the two `probe-nested-*.svg` files by a marker inside a marker and
/// a clip inside a clip, whose copies no count bounds; and the rest by the
/// work budget, the steps the renderer would take, a stroke reaching too far
/// among them. The test after the corpus check draws each of them with that
/// work made small, the one after it each file refused for an element with
/// that element taken out, the one after that each file refused for what it
/// says with that made plain, and the one after that the two refused for the
/// size of their view with that view made plain, with every circle and with
/// one.
const ENTRIES: &[(&str, Expect)] = &[
    ("image/anim.gif", Drawn),
    ("image/bomb.jpg", No(Refused::TooLarge)),
    ("image/bomb.png", No(Refused::TooLarge)),
    ("image/capped.png", No(Refused::TooLarge)),
    ("image/photo.jpg", No(Refused::TooLarge)),
    ("image/photo.webp", No(Refused::TooLarge)),
    ("image/random.bin", No(Refused::NotAnImage)),
    ("image/small.png", Drawn),
    ("image/truncated.gif", No(Refused::NotAnImage)),
    ("image/truncated.jpg", No(Refused::NotAnImage)),
    ("image/truncated.png", No(Refused::TooLarge)),
    ("image/truncated.webp", No(Refused::TooLarge)),
    ("svg/drawn/flowchart-16n-24e.svg", Drawn),
    ("svg/drawn/sequence-8p-64e.svg", Drawn),
    ("svg/filter-work-morphology.svg", No(Refused::TooLarge)),
    (
        "svg/filter-work-small-turbulence.svg",
        No(Refused::TooLarge),
    ),
    ("svg/filter-work-turbulence.svg", No(Refused::TooLarge)),
    ("svg/form-a.svg", No(Refused::NotAnImage)),
    ("svg/form-foreignObject.svg", No(Refused::NotAnImage)),
    ("svg/form-image.svg", No(Refused::NotAnImage)),
    ("svg/form-mask.svg", No(Refused::NotAnImage)),
    ("svg/form-pattern.svg", No(Refused::NotAnImage)),
    ("svg/form-style.svg", No(Refused::NotAnImage)),
    ("svg/form-switch.svg", No(Refused::NotAnImage)),
    ("svg/form-textPath.svg", No(Refused::NotAnImage)),
    ("svg/form-tref.svg", No(Refused::NotAnImage)),
    ("svg/garbage.svg", No(Refused::NotAnImage)),
    ("svg/many-results.svg", No(Refused::TooLarge)),
    ("svg/nested.svg", Drawn),
    ("svg/probe-arc-huge.svg", No(Refused::NotAnImage)),
    ("svg/probe-box-clips.svg", No(Refused::TooLarge)),
    ("svg/probe-circles-huge.svg", No(Refused::NotAnImage)),
    ("svg/probe-em-compounded.svg", No(Refused::NotAnImage)),
    ("svg/probe-feimage-chain.svg", No(Refused::NotAnImage)),
    ("svg/probe-feimage-text.svg", No(Refused::NotAnImage)),
    ("svg/probe-gradient-chain.svg", No(Refused::NotAnImage)),
    ("svg/probe-inherit-compounded.svg", No(Refused::NotAnImage)),
    ("svg/probe-marker-copies.svg", No(Refused::TooLarge)),
    ("svg/probe-nested-clips.svg", No(Refused::TooLarge)),
    ("svg/probe-nested-markers.svg", No(Refused::TooLarge)),
    ("svg/probe-nested-masks.svg", No(Refused::NotAnImage)),
    ("svg/probe-percent-root.svg", No(Refused::NotAnImage)),
    ("svg/probe-percent-svg-chain.svg", No(Refused::NotAnImage)),
    (
        "svg/probe-percent-symbol-chain.svg",
        No(Refused::NotAnImage),
    ),
    ("svg/probe-percent-use-symbol.svg", No(Refused::NotAnImage)),
    ("svg/probe-scaled-stroke.svg", No(Refused::TooLarge)),
    ("svg/probe-stops-object-box.svg", No(Refused::TooLarge)),
    ("svg/probe-stops-user-space.svg", No(Refused::TooLarge)),
    ("svg/probe-stroked-circles-each.svg", No(Refused::TooLarge)),
    ("svg/probe-stroked-circles-most.svg", No(Refused::TooLarge)),
    (
        "svg/probe-stroked-circles-turned.svg",
        No(Refused::TooLarge),
    ),
    ("svg/probe-stroked-circles.svg", No(Refused::TooLarge)),
    ("svg/probe-style-copies.svg", No(Refused::NotAnImage)),
    ("svg/probe-svg-image.svg", No(Refused::NotAnImage)),
    ("svg/probe-tref.svg", No(Refused::NotAnImage)),
    ("svg/probe-twice-given-id.svg", No(Refused::NotAnImage)),
    ("svg/probe-undrawn-letters.svg", No(Refused::TooLarge)),
    ("svg/probe-useloop-feimage.svg", No(Refused::NotAnImage)),
    ("svg/small.svg", Drawn),
    ("svg/work-clip-shapes.svg", No(Refused::TooLarge)),
    ("svg/work-copied-path.svg", No(Refused::TooLarge)),
    ("svg/work-copied-text.svg", No(Refused::TooLarge)),
    ("svg/work-dashes.svg", No(Refused::TooLarge)),
    ("svg/work-gradient-crossover.svg", No(Refused::TooLarge)),
    ("svg/work-gradient-stops.svg", No(Refused::TooLarge)),
    ("svg/work-merge-inputs.svg", No(Refused::TooLarge)),
    ("svg/work-nested-uses-deeper.svg", No(Refused::TooLarge)),
    ("svg/work-nested-uses.svg", No(Refused::TooLarge)),
    ("svg/work-paths-tiny.svg", No(Refused::TooLarge)),
    ("svg/work-segment-rows.svg", No(Refused::TooLarge)),
    ("svg/work-shapes-filled.svg", No(Refused::TooLarge)),
    ("svg/work-shapes-stroked.svg", No(Refused::TooLarge)),
    ("svg/work-shapes-tiny.svg", No(Refused::TooLarge)),
    ("svg/work-turbulence-octaves.svg", No(Refused::TooLarge)),
    ("svg/worst.svg", No(Refused::TooLarge)),
];

fn corpus_files() -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, into: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("a corpus directory") {
            let entry = entry.expect("a corpus entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("a file type").is_dir() {
                walk(&entry.path(), &format!("{name}/"), into);
            } else {
                into.push(name);
            }
        }
    }
    let mut files = Vec::new();
    for kind in ["image", "svg"] {
        walk(
            &Path::new(CORPUS).join(kind),
            &format!("{kind}/"),
            &mut files,
        );
    }
    files.sort();
    files
}

/// What this crate does with a file of the corpus: a picture is drawn, an image
/// is decoded.
fn draw(name: &str, bytes: &[u8]) -> Result<Pixels, Refused> {
    if name.starts_with("image/") {
        decode(bytes)
    } else {
        rasterise(bytes, WIDTH, INKS)
    }
}

/// Everything a drawn picture is held to, and the payloads made of it.
fn check_bounded(name: &str, pixels: &Pixels) {
    let bytes = pixels.rgba().len();
    assert!(bytes <= MOST_DECODED, "{name}: {bytes} bytes of pixels");
    assert_eq!(
        bytes,
        pixels.width() as usize * pixels.height() as usize * 4,
        "{name}: the pixels are not the size claimed"
    );
    match png(pixels) {
        Ok(payload) => assert!(payload.len() <= MOST_PNG, "{name}: a PNG past its ceiling"),
        Err(refused) => assert_eq!(
            refused,
            Refused::TooLarge,
            "{name}: PNG refused as {refused}"
        ),
    }
    match sixel(pixels, &PALETTE) {
        Ok(_) => assert!(
            pixels.width() <= MOST_SIXEL_WIDTH,
            "{name}: a Sixel past its width"
        ),
        Err(refused) => {
            assert_eq!(
                refused,
                Refused::TooLarge,
                "{name}: Sixel refused as {refused}"
            );
            assert!(
                pixels.width() > MOST_SIXEL_WIDTH,
                "{name}: a narrow picture refused"
            );
        }
    }
}

#[test]
fn every_file_of_the_corpus_is_drawn_or_refused_as_recorded() {
    let _alone = alone();
    for (name, expect) in ENTRIES {
        let bytes = fs::read(Path::new(CORPUS).join(name)).expect("a corpus file");
        let owned = (*name).to_owned();
        let begun = Instant::now();
        let ended = guarded(DEADLINE, move || draw(&owned, &bytes));
        let Ended::Returned(result) = ended else {
            panic!("{name}: {ended:?}");
        };
        match (expect, &result) {
            (Drawn, Ok(pixels)) => check_bounded(name, pixels),
            (No(refused), Err(got)) => assert_eq!(got, refused, "{name}"),
            _ => panic!("{name}: expected {expect:?}, got {result:?}"),
        }
        if *expect == No(Refused::TooLarge) {
            assert!(
                begun.elapsed() < Duration::from_secs(10),
                "{name}: a bomb took {:?} to refuse",
                begun.elapsed()
            );
        }
    }
}

#[test]
fn no_file_of_the_corpus_is_missing_from_the_record_or_the_record_from_the_corpus() {
    let recorded: Vec<String> = ENTRIES.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(corpus_files(), recorded);
}

#[test]
fn a_file_refused_for_its_work_is_drawn_once_the_work_is_made_small() {
    let _alone = alone();
    let read = |name: &str| {
        String::from_utf8(fs::read(Path::new(CORPUS).join(name)).expect("a corpus file"))
            .expect("a corpus file that is text")
    };
    // The file with every copy of `unit` but one taken out.
    let once = |original: &str, unit: &str| {
        let copies = original.matches(unit).count();
        assert!(copies > 1, "many copies of {unit}");
        let one = original.replacen(&unit.repeat(copies), unit, 1);
        assert_eq!(one.matches(unit).count(), 1, "the copies are in a row");
        one
    };
    // Each is refused for the copies the parser would make or for what the
    // renderer does with every pixel, and the same file with that made small
    // is drawn: the pixmaps it holds were never the reason.
    let merge = read("svg/work-merge-inputs.svg");
    let turbulence = read("svg/work-turbulence-octaves.svg");
    let filled = read("svg/work-shapes-filled.svg");
    let stroked = read("svg/work-shapes-stroked.svg");
    let dashes = read("svg/work-dashes.svg");
    let clip = read("svg/work-clip-shapes.svg");
    let uses = read("svg/work-nested-uses.svg");
    let deeper = read("svg/work-nested-uses-deeper.svg");
    let copied_path = read("svg/work-copied-path.svg");
    let copied_text = read("svg/work-copied-text.svg");
    let tiny = read("svg/work-shapes-tiny.svg");
    let paths = read("svg/work-paths-tiny.svg");
    let rows = read("svg/work-segment-rows.svg");
    let stops = read("svg/work-gradient-stops.svg");
    let crossover = read("svg/work-gradient-crossover.svg");
    let nested_markers = read("svg/probe-nested-markers.svg");
    let nested_clips = read("svg/probe-nested-clips.svg");
    let marker_copies = read("svg/probe-marker-copies.svg");
    let box_clips = read("svg/probe-box-clips.svg");
    let letters = read("svg/probe-undrawn-letters.svg");
    let object_box = read("svg/probe-stops-object-box.svg");
    let user_space = read("svg/probe-stops-user-space.svg");
    let scaled = read("svg/probe-scaled-stroke.svg");
    let stroked_circles = read("svg/probe-stroked-circles.svg");
    let stroked_most = read("svg/probe-stroked-circles-most.svg");
    let stroked_each = read("svg/probe-stroked-circles-each.svg");
    let stroked_turned = read("svg/probe-stroked-circles-turned.svg");
    let worst = read("svg/worst.svg");
    // The path with its first hundred edges and no more.
    let fewer = |original: &str| {
        let (head, data) = original.split_once(r#"<path d=""#).expect("a path");
        let (data, tail) = data.split_once('"').expect("the end of its data");
        let kept: Vec<&str> = data.split('L').take(100).collect();
        assert!(
            kept.len() < data.split('L').count(),
            "more than a hundred edges"
        );
        format!(r#"{head}<path d="{}"{tail}"#, kept.join("L"))
    };
    // The gradient with its first stop and its last and none between.
    let ends = |original: &str| {
        let (head, rest) = original.split_once("<stop").expect("a stop");
        let (stops, tail) = rest
            .split_once("</linearGradient>")
            .expect("the gradient's end");
        let all: Vec<&str> = stops.split_inclusive("/>").collect();
        assert!(all.len() > 2, "many stops");
        let (first, last) = (all.first().expect("a stop"), all.last().expect("a stop"));
        format!(
            "{head}<stop{first}<stop{}</linearGradient>{tail}",
            last.trim_start_matches("<stop")
        )
    };
    // The uses with their first two levels and no more.
    let two_levels = |original: &str| {
        let (head, rest) = original
            .split_once(r#"<g id="l3">"#)
            .expect("a third level");
        let (_, tail) = rest.split_once("</defs>").expect("the definitions' end");
        let (_, tail) = tail.split_once("/>").expect("the use drawn");
        format!(r##"{head}</defs><use xlink:href="#l2"/>{tail}"##)
    };
    // The picture with its lines and none of its text.
    let lines = |original: &str| {
        let mut kept = String::new();
        let mut rest = original;
        while let Some((before, after)) = rest.split_once("<text") {
            kept.push_str(before);
            rest = after.split_once("</text>").expect("the text's end").1;
        }
        kept.push_str(rest);
        assert!(kept.len() < original.len(), "text to take out");
        kept
    };
    // The file with the references to its first `levels` levels taken out,
    // which are the ones made inside a marker or a clip, and the one to the
    // last level, from outside them, kept.
    let unnested = |original: &str, levels: u32, reference: &dyn Fn(u32) -> String| {
        let mut kept = original.to_owned();
        for level in 0..levels {
            let inner = reference(level);
            assert!(kept.contains(&inner), "a reference to level {level}");
            kept = kept.replace(&inner, "");
        }
        assert!(kept.contains(&reference(levels)), "the reference drawn");
        kept
    };
    let small = [
        once(&merge, r#"<feMergeNode in="SourceGraphic"/>"#),
        turbulence.replace(r#"numOctaves="10""#, r#"numOctaves="1""#),
        once(&filled, r#"<rect width="100" height="100"/>"#),
        once(
            &stroked,
            r#"<path d="M0 50H100" fill="none" stroke="black" stroke-width="100"/>"#,
        ),
        dashes.replace(
            r#"stroke-dasharray="0.01 0.01""#,
            r#"stroke-dasharray="512""#,
        ),
        once(&clip, r#"<rect width="360" height="360"/>"#),
        two_levels(&uses),
        once(&tiny, r#"<rect width="1" height="1"/>"#),
        once(&paths, r#"<path d="M0 0L1 1" stroke="black"/>"#),
        fewer(&rows),
        ends(&stops),
        unnested(&nested_markers, 4, &|level| {
            format!(
                r#" marker-start="url(#m{level})" marker-mid="url(#m{level})" marker-end="url(#m{level})""#
            )
        }),
        lines(&worst),
        two_levels(&deeper),
        once(&copied_path, r##"<use xlink:href="#copied" x="-200"/>"##),
        once(&copied_text, r##"<use xlink:href="#copied" x="-200"/>"##),
        ends(&crossover),
        unnested(&nested_clips, 5, &|level| {
            format!(r#" clip-path="url(#c{level})""#)
        }),
        once(&marker_copies, r#"<rect x="-900" width="1" height="1"/>"#),
        once(&box_clips, r#"<rect x="-900" width="1" height="1"/>"#),
        letters.replace(&"x ".repeat(25_000), "x "),
        ends(&object_box),
        ends(&user_space),
        scaled.replacen(r#"<g transform="scale(1e5)">"#, "<g>", 3),
        once(&stroked_circles, r#"<circle r="1e5"/>"#),
        once(&stroked_most, r#"<circle r="1e5"/>"#),
        once(
            &stroked_each,
            r##"<circle r="100000" fill="none" stroke="#000"/>"##,
        ),
        stroked_turned
            .replace("rotate(30) scale(100000)", "rotate(30)")
            .replace(r#"stroke-width="100000""#, r#"stroke-width="1""#),
    ];
    for (name, original, small) in [
        ("svg/work-merge-inputs.svg", &merge, &small[0]),
        ("svg/work-turbulence-octaves.svg", &turbulence, &small[1]),
        ("svg/work-shapes-filled.svg", &filled, &small[2]),
        ("svg/work-shapes-stroked.svg", &stroked, &small[3]),
        ("svg/work-dashes.svg", &dashes, &small[4]),
        ("svg/work-clip-shapes.svg", &clip, &small[5]),
        ("svg/work-nested-uses.svg", &uses, &small[6]),
        ("svg/work-shapes-tiny.svg", &tiny, &small[7]),
        ("svg/work-paths-tiny.svg", &paths, &small[8]),
        ("svg/work-segment-rows.svg", &rows, &small[9]),
        ("svg/work-gradient-stops.svg", &stops, &small[10]),
        ("svg/probe-nested-markers.svg", &nested_markers, &small[11]),
        ("svg/worst.svg", &worst, &small[12]),
        ("svg/work-nested-uses-deeper.svg", &deeper, &small[13]),
        ("svg/work-copied-path.svg", &copied_path, &small[14]),
        ("svg/work-copied-text.svg", &copied_text, &small[15]),
        ("svg/work-gradient-crossover.svg", &crossover, &small[16]),
        ("svg/probe-nested-clips.svg", &nested_clips, &small[17]),
        ("svg/probe-marker-copies.svg", &marker_copies, &small[18]),
        ("svg/probe-box-clips.svg", &box_clips, &small[19]),
        ("svg/probe-undrawn-letters.svg", &letters, &small[20]),
        ("svg/probe-stops-object-box.svg", &object_box, &small[21]),
        ("svg/probe-stops-user-space.svg", &user_space, &small[22]),
        ("svg/probe-scaled-stroke.svg", &scaled, &small[23]),
        (
            "svg/probe-stroked-circles.svg",
            &stroked_circles,
            &small[24],
        ),
        (
            "svg/probe-stroked-circles-most.svg",
            &stroked_most,
            &small[25],
        ),
        (
            "svg/probe-stroked-circles-each.svg",
            &stroked_each,
            &small[26],
        ),
        (
            "svg/probe-stroked-circles-turned.svg",
            &stroked_turned,
            &small[27],
        ),
    ] {
        assert_ne!(original, small, "{name}");
        assert_eq!(
            rasterise(original.as_bytes(), WIDTH, INKS),
            Err(Refused::TooLarge),
            "{name}"
        );
        let drawn = rasterise(small.as_bytes(), WIDTH, INKS);
        assert!(drawn.is_ok(), "{name} with its work made small: {drawn:?}");
    }
}

/// Longer than judging any file of the corpus takes, and shorter than drawing
/// or building those judged here: 100 ms.
const SOON_REFUSED: Duration = Duration::from_millis(100);

/// The files refused before the renderer or the parser does the work they
/// ask for, each by a price, a count, a size or an element the renderer or
/// the parser would otherwise have reached long after the ceiling.
const REFUSED_SOON: &[(&str, Refused)] = &[
    ("svg/work-gradient-crossover.svg", Refused::TooLarge),
    ("svg/probe-arc-huge.svg", Refused::NotAnImage),
    ("svg/probe-box-clips.svg", Refused::TooLarge),
    ("svg/probe-circles-huge.svg", Refused::NotAnImage),
    ("svg/probe-em-compounded.svg", Refused::NotAnImage),
    ("svg/probe-feimage-chain.svg", Refused::NotAnImage),
    ("svg/probe-feimage-text.svg", Refused::NotAnImage),
    ("svg/probe-gradient-chain.svg", Refused::NotAnImage),
    ("svg/probe-inherit-compounded.svg", Refused::NotAnImage),
    ("svg/probe-marker-copies.svg", Refused::TooLarge),
    ("svg/probe-nested-clips.svg", Refused::TooLarge),
    ("svg/probe-nested-markers.svg", Refused::TooLarge),
    ("svg/probe-nested-masks.svg", Refused::NotAnImage),
    ("svg/probe-percent-root.svg", Refused::NotAnImage),
    ("svg/probe-percent-svg-chain.svg", Refused::NotAnImage),
    ("svg/probe-percent-symbol-chain.svg", Refused::NotAnImage),
    ("svg/probe-percent-use-symbol.svg", Refused::NotAnImage),
    ("svg/probe-scaled-stroke.svg", Refused::TooLarge),
    ("svg/probe-stops-object-box.svg", Refused::TooLarge),
    ("svg/probe-stops-user-space.svg", Refused::TooLarge),
    ("svg/probe-stroked-circles-each.svg", Refused::TooLarge),
    ("svg/probe-stroked-circles-most.svg", Refused::TooLarge),
    ("svg/probe-stroked-circles-turned.svg", Refused::TooLarge),
    ("svg/probe-stroked-circles.svg", Refused::TooLarge),
    ("svg/probe-style-copies.svg", Refused::NotAnImage),
    ("svg/probe-svg-image.svg", Refused::NotAnImage),
    ("svg/probe-tref.svg", Refused::NotAnImage),
    ("svg/probe-twice-given-id.svg", Refused::NotAnImage),
    ("svg/probe-undrawn-letters.svg", Refused::TooLarge),
    ("svg/probe-useloop-feimage.svg", Refused::NotAnImage),
];

#[test]
fn a_file_refused_for_its_work_is_refused_before_the_work_is_done() {
    let _alone = alone();
    for (name, refused) in REFUSED_SOON {
        let bytes = fs::read(Path::new(CORPUS).join(name)).expect("a corpus file");
        let began = Instant::now();
        let drawn = rasterise(&bytes, WIDTH, INKS);
        let took = began.elapsed();
        assert_eq!(drawn, Err(*refused), "{name}");
        assert!(took < SOON_REFUSED, "{name} took {took:?} to refuse");
    }
}

/// The files refused for holding an element outside those drawn, each with
/// the one element that refuses it.
const REFUSED_FOR: &[(&str, &str)] = &[
    ("svg/form-a.svg", "a"),
    ("svg/form-foreignObject.svg", "foreignObject"),
    ("svg/form-image.svg", "image"),
    ("svg/form-mask.svg", "mask"),
    ("svg/form-pattern.svg", "pattern"),
    ("svg/form-style.svg", "style"),
    ("svg/form-switch.svg", "switch"),
    ("svg/form-textPath.svg", "textPath"),
    ("svg/form-tref.svg", "tref"),
    ("svg/probe-nested-masks.svg", "mask"),
    ("svg/probe-style-copies.svg", "style"),
    ("svg/probe-svg-image.svg", "image"),
    ("svg/probe-tref.svg", "tref"),
];

#[test]
fn a_file_refused_for_an_element_is_drawn_once_the_element_is_a_group() {
    let _alone = alone();
    for (name, element) in REFUSED_FOR {
        let original = fs::read_to_string(Path::new(CORPUS).join(name)).expect("a corpus file");
        let opened = format!("<{element}");
        let closed = format!("</{element}>");
        assert!(original.contains(&opened), "{name} holds {element}");
        // An element that pointed somewhere gives that up as a group: a
        // reference is refused on anything but a use or a filter image.
        let grouped = unlinked(&original.replace(&opened, "<g").replace(&closed, "</g>"));
        assert_eq!(
            rasterise(original.as_bytes(), WIDTH, INKS),
            Err(Refused::NotAnImage),
            "{name}"
        );
        let drawn = rasterise(grouped.as_bytes(), WIDTH, INKS);
        assert!(
            drawn.is_ok(),
            "{name} with its {element} a group: {drawn:?}"
        );
    }
}

/// `svg` with the reference taken off each group that gives one.
fn unlinked(svg: &str) -> String {
    let mut pieces = svg.split("<g ");
    let mut kept = pieces.next().unwrap_or_default().to_owned();
    for piece in pieces {
        kept.push_str("<g ");
        match piece.split_once(r#"xlink:href=""#) {
            Some((before, after)) if !before.contains('>') => {
                let (_, after) = after.split_once('"').expect("the end of a reference");
                kept.push_str(before);
                kept.push_str(after);
            }
            _ => kept.push_str(piece),
        }
    }
    kept
}

/// The files refused for what they say, each with what makes it plain: every
/// replacement of the first text by the second.
const REFUSED_FOR_SAYING: &[(&str, &[(&str, &str)])] = &[
    ("svg/probe-arc-huge.svg", &[("A1e154 1", "A1 1")]),
    (
        "svg/probe-circles-huge.svg",
        &[(r#"r="3e38""#, r#"r="30""#)],
    ),
    ("svg/probe-em-compounded.svg", &[(r#""1e4em""#, r#""1em""#)]),
    (
        "svg/probe-inherit-compounded.svg",
        &[(r#""1e4em""#, r#""1em""#)],
    ),
    (
        "svg/probe-feimage-chain.svg",
        &[(r##"<feImage xlink:href="#"##, r#"<feImage xlink:href=""#)],
    ),
    (
        "svg/probe-feimage-text.svg",
        &[(r##"<feImage xlink:href="#"##, r#"<feImage xlink:href=""#)],
    ),
    (
        "svg/probe-gradient-chain.svg",
        &[(r##" xlink:href="#"##, r##" class="#"##)],
    ),
    (
        "svg/probe-useloop-feimage.svg",
        &[
            (r##"xlink:href="#x""##, r#"xlink:href="x""#),
            (r#"<rect id="z" width="10" height="10"/>"#, ""),
        ],
    ),
    (
        "svg/probe-twice-given-id.svg",
        &[(r#"<rect id="a" x="20""#, r#"<rect id="b" x="20""#)],
    ),
    (
        "svg/probe-percent-svg-chain.svg",
        &[(
            r#"width="100000%" height="100000%""#,
            r#"width="1" height="1""#,
        )],
    ),
    (
        "svg/probe-percent-symbol-chain.svg",
        &[
            (
                r#"width="100000%" height="100000%""#,
                r#"width="1" height="1""#,
            ),
            (r#"width="100%" height="100%""#, r#"width="1" height="1""#),
        ],
    ),
];

#[test]
fn a_file_refused_for_what_it_says_is_drawn_once_that_is_made_plain() {
    let _alone = alone();
    for (name, plain) in REFUSED_FOR_SAYING {
        let original = fs::read_to_string(Path::new(CORPUS).join(name)).expect("a corpus file");
        let mut made_plain = original.clone();
        for (said, instead) in *plain {
            assert!(made_plain.contains(said), "{name} says {said}");
            made_plain = made_plain.replace(said, instead);
        }
        assert_eq!(
            rasterise(original.as_bytes(), WIDTH, INKS),
            Err(Refused::NotAnImage),
            "{name}"
        );
        let drawn = rasterise(made_plain.as_bytes(), WIDTH, INKS);
        assert!(drawn.is_ok(), "{name} made plain: {drawn:?}");
    }
}

/// The files refused for circles sized in a percentage of a view that is
/// itself sized past the largest size a number may have, each with the circle
/// it repeats, the size of that view, and that size made plain.
const REFUSED_FOR_A_VIEW: &[(&str, &str, &str, &str)] = &[
    (
        "svg/probe-percent-root.svg",
        r#"<circle r="1e5%"/>"#,
        r#"width="1e5%" height="1e5%""#,
        r#"width="100" height="100""#,
    ),
    (
        "svg/probe-percent-use-symbol.svg",
        r#"<circle r="1e5%"/>"#,
        r#"width="1e5" height="1e5""#,
        r#"width="100" height="100""#,
    ),
];

#[test]
fn a_file_refused_for_the_view_its_sizes_are_taken_of_is_drawn_once_that_is_made_plain() {
    let _alone = alone();
    for (name, circle, view, plain) in REFUSED_FOR_A_VIEW {
        let original = fs::read_to_string(Path::new(CORPUS).join(name)).expect("a corpus file");
        let copies = original.matches(circle).count();
        assert!(copies > 1, "{name}: many copies of {circle}");
        let one = original.replacen(&circle.repeat(copies), circle, 1);
        assert_eq!(
            one.matches(circle).count(),
            1,
            "{name}: the copies are in a row"
        );
        assert_eq!(original.matches(view).count(), 1, "{name} sizes its view");
        // The view refuses the file however few circles are taken of it.
        for refused in [&original, &one] {
            assert_eq!(
                rasterise(refused.as_bytes(), WIDTH, INKS),
                Err(Refused::NotAnImage),
                "{name}"
            );
        }
        // With the view plain, so many circles are no longer refused for
        // what they say, though copied by a use they are more work than
        // the parser may do, and one is drawn.
        assert_ne!(
            rasterise(original.replace(view, plain).as_bytes(), WIDTH, INKS),
            Err(Refused::NotAnImage),
            "{name} with its view plain"
        );
        let drawn = rasterise(one.replace(view, plain).as_bytes(), WIDTH, INKS);
        assert!(
            drawn.is_ok(),
            "{name} with its view plain and one circle: {drawn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The sentinels: what a reference points at.

static MADE: AtomicUsize = AtomicUsize::new(0);

/// A sentinel that records being touched. It records from a thread of its own,
/// and `end` stops it and gives what it saw.
struct Watch {
    seen: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    fn seen(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }

    /// Waits up to `within` for the sentinel to record something.
    fn seen_within(&self, within: Duration) -> bool {
        let begun = Instant::now();
        while begun.elapsed() < within {
            if self.seen() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.seen()
    }

    fn end(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A TCP listener on loopback, in every platform's reach: it records an
/// accepted connection.
struct Listener {
    port: u16,
    watch: Watch,
}

impl Listener {
    fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a listener that does not block");
        let port = listener.local_addr().expect("an address").port();
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (heard, told) = (Arc::clone(&seen), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            while !told.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok(_) => heard.store(true, Ordering::SeqCst),
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            port,
            watch: Watch {
                seen,
                stop,
                thread: Some(thread),
            },
        }
    }
}

/// A named pipe the file system calls a file: on Unix a FIFO whose writer
/// records an open, on Windows a pipe whose server records a connect.
struct Pipe {
    path: PathBuf,
    watch: Watch,
}

#[cfg(unix)]
impl Pipe {
    fn new(dir: &Path) -> Self {
        let path = dir.join("sentinel.fifo");
        let made = Command::new("mkfifo").arg(&path).status().expect("mkfifo");
        assert!(made.success(), "a FIFO to point at");
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (heard, told, at) = (Arc::clone(&seen), Arc::clone(&stop), path.clone());
        // Opening a FIFO for writing waits for a reader: it returns only when
        // something opens the FIFO to read it.
        let thread = thread::spawn(move || {
            if fs::OpenOptions::new().write(true).open(at).is_ok() && !told.load(Ordering::SeqCst) {
                heard.store(true, Ordering::SeqCst);
            }
        });
        Self {
            path,
            watch: Watch {
                seen,
                stop,
                thread: Some(thread),
            },
        }
    }

    /// The writer waits in `open` for a reader, and a reader of our own lets it
    /// go. It is told to stop first, so it records nothing for ours.
    fn end(&mut self) {
        self.watch.end();
        let at = self.path.clone();
        let release = thread::spawn(move || {
            // Blocks for ever if the writer has already gone: a thread left
            // behind, not a hang of the test.
            drop(fs::File::open(at));
        });
        let begun = Instant::now();
        while begun.elapsed() < Duration::from_secs(2)
            && self
                .watch
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
        {
            thread::sleep(Duration::from_millis(5));
        }
        drop(release);
    }
}

#[cfg(windows)]
impl Pipe {
    fn new(_dir: &Path) -> Self {
        use tokio::net::windows::named_pipe::ServerOptions;
        let path = PathBuf::from(format!(
            r"\\.\pipe\crucible-raster-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::SeqCst)
        ));
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (heard, told, at) = (Arc::clone(&seen), Arc::clone(&stop), path.clone());
        let (ready, created) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the pipe");
            runtime.block_on(async {
                let server = ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(&at)
                    .expect("a named pipe to point at");
                ready.send(()).expect("the test is waiting");
                while !told.load(Ordering::SeqCst) {
                    // A connect that is cut off here is still a connect to
                    // wait for on the next turn.
                    let waited =
                        tokio::time::timeout(Duration::from_millis(20), server.connect()).await;
                    if matches!(waited, Ok(Ok(()))) {
                        heard.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            });
        });
        created
            .recv_timeout(Duration::from_secs(10))
            .expect("the pipe is created");
        Self {
            path,
            watch: Watch {
                seen,
                stop,
                thread: Some(thread),
            },
        }
    }

    fn end(&mut self) {
        self.watch.end();
    }
}

/// A valid SVG file, style sheet and font, each of which would change the
/// output if it were used.
struct Sentinels {
    dir: PathBuf,
    svg: PathBuf,
    css: PathBuf,
    font: PathBuf,
    listener: Listener,
    pipe: Pipe,
}

const SENTINEL_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="#ff0000"/></svg>"##;
const SENTINEL_CSS: &str = "rect { fill: #ff0000 }";

impl Sentinels {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "crucible-raster-sentinels-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).expect("a directory for the sentinels");
        let svg = dir.join("sentinel.svg");
        let css = dir.join("sentinel.css");
        let font = dir.join("sentinel.ttf");
        fs::write(&svg, SENTINEL_SVG).expect("a sentinel");
        fs::write(&css, SENTINEL_CSS).expect("a sentinel");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts/DejaVuSans.ttf"),
            &font,
        )
        .expect("a sentinel");
        let pipe = Pipe::new(&dir);
        Self {
            dir,
            svg,
            css,
            font,
            listener: Listener::new(),
            pipe,
        }
    }

    fn url_of(path: &Path) -> String {
        let slashes = path.display().to_string().replace('\\', "/");
        format!("file:///{}", slashes.trim_start_matches('/'))
    }

    /// A template with the sentinels put in. `None` for a template whose
    /// sentinel this platform has no way to make.
    fn fill(&self, template: &str) -> Option<String> {
        let pipe = self.pipe.path.display().to_string();
        let mut out = template.to_owned();
        for (name, value) in [
            ("@SVG_URL@", Some(Self::url_of(&self.svg))),
            ("@SVG@", Some(self.svg.display().to_string())),
            ("@CSS@", Some(self.css.display().to_string())),
            ("@FONT@", Some(self.font.display().to_string())),
            ("@PORT@", Some(self.listener.port.to_string())),
            ("@PIPE@", cfg!(windows).then(|| pipe.clone())),
            ("@FIFO@", cfg!(unix).then_some(pipe)),
        ] {
            if out.contains(name) {
                out = out.replace(name, &value?);
            }
        }
        assert!(
            ["@SVG", "@CSS@", "@FONT@", "@PORT@", "@PIPE@", "@FIFO@"]
                .iter()
                .all(|name| !out.contains(name)),
            "a placeholder no sentinel fills: {out}"
        );
        Some(out)
    }

    /// The sentinels that recorded something, by name.
    fn touched(&self, within: Duration) -> Vec<&'static str> {
        let mut touched = Vec::new();
        if self.listener.watch.seen_within(within) {
            touched.push("a connection was accepted on 127.0.0.1");
        }
        if self.pipe.watch.seen_within(within) {
            touched.push(if cfg!(windows) {
                "the named pipe was connected to"
            } else {
                "the FIFO was opened"
            });
        }
        touched
    }
}

impl Drop for Sentinels {
    fn drop(&mut self) {
        self.listener.watch.end();
        self.pipe.end();
        if let Some(thread) = self.listener.watch.thread.take() {
            drop(thread.join());
        }
        drop(fs::remove_dir_all(&self.dir));
    }
}

/// The templates of `tests/corpus/references/`, by file name.
fn templates() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = fs::read_dir(Path::new(CORPUS).join("references"))
        .expect("the references")
        .map(|entry| {
            let entry = entry.expect("a reference");
            let text = fs::read_to_string(entry.path()).expect("a reference as text");
            (entry.file_name().to_string_lossy().into_owned(), text)
        })
        .collect();
    found.sort();
    found
}

/// The template as it is with the reference in it, and as it is with the
/// reference taken out.
fn with_and_without(filled: &str) -> (String, String) {
    let (Some((before, rest)), Some((_, after))) = (
        filled.split_once("[[reference]]"),
        filled.split_once("[[/reference]]"),
    ) else {
        panic!("a template without its reference marked: {filled}");
    };
    let (inside, _) = rest.split_once("[[/reference]]").expect("a closed marker");
    (
        format!("{before}{inside}{after}"),
        format!("{before}{after}"),
    )
}

/// What a way of drawing a picture does with the text of an SVG.
type Drawer = dyn Fn(&Sentinels, &str) -> Result<Pixels, Refused>;

fn this_crate(_: &Sentinels, svg: &str) -> Result<Pixels, Refused> {
    rasterise(svg.as_bytes(), 100, INKS)
}

/// Every reference drawn as though it were not there, and what it was
/// pointing at, touched or not. What went wrong, in words.
fn what_a_drawer_does_with_the_references(sentinels: &Sentinels, drawer: &Drawer) -> Vec<String> {
    let mut wrong = Vec::new();
    let mut drawn = 0;
    for (name, template) in templates() {
        let Some(filled) = sentinels.fill(&template) else {
            continue;
        };
        let (with, without) = with_and_without(&filled);
        let (with, without) = (drawer(sentinels, &with), drawer(sentinels, &without));
        match (with, without) {
            // A template that holds an element outside those drawn is refused
            // with its reference and without it alike, and nothing is drawn.
            (Err(with), Err(without)) if with == without => continue,
            (Ok(with), Ok(without)) if with == without => {}
            (with, without) => wrong.push(format!(
                "{name}: drew {with:?} where the reference removed draws {without:?}"
            )),
        }
        drawn += 1;
    }
    // The four `href-*.svg` templates each platform fills; the two that hold a
    // style sheet are refused.
    assert!(drawn >= 4, "only {drawn} templates drawn");
    wrong
}

#[test]
fn every_reference_is_drawn_as_if_it_were_not_there_and_what_it_names_records_nothing() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &this_crate);
    assert!(wrong.is_empty(), "{wrong:#?}");
    let touched = sentinels.touched(Duration::from_millis(300));
    assert!(touched.is_empty(), "{touched:?}");
}

// A stand-in for a resolver that does what this crate must not, wrapped
// around the real drawing so that the same checks judge it.

fn percent_encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Reads the sentinel a reference names and puts it into the picture.
fn uses_what_it_reads(sentinels: &Sentinels, svg: &str) -> Result<Pixels, Refused> {
    let inline = format!(
        "data:image/svg+xml;charset=utf8,{}",
        percent_encoded(SENTINEL_SVG)
    );
    let svg = svg
        .replace(
            &format!("\"{}\"", sentinels.svg.display()),
            &format!("\"{inline}\""),
        )
        .replace(
            &format!("@import url(\"{}\");", sentinels.css.display()),
            SENTINEL_CSS,
        );
    this_crate(sentinels, &svg)
}

/// Opens what a reference names and throws away what it finds.
fn reads_and_discards(sentinels: &Sentinels, svg: &str) -> Result<Pixels, Refused> {
    for path in [&sentinels.pipe.path, &sentinels.svg, &sentinels.css] {
        if svg.contains(&path.display().to_string()) {
            drop(fs::read(path));
        }
    }
    this_crate(sentinels, svg)
}

/// Connects to the address a reference names.
fn connects(sentinels: &Sentinels, svg: &str) -> Result<Pixels, Refused> {
    if svg.contains(&format!("127.0.0.1:{}", sentinels.listener.port)) {
        drop(TcpStream::connect(("127.0.0.1", sentinels.listener.port)));
    }
    this_crate(sentinels, svg)
}

#[test]
fn a_planted_resolver_that_uses_the_sentinel_changes_the_output_and_is_found() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &uses_what_it_reads);
    assert!(
        wrong.iter().any(|line| line.starts_with("href-file.svg")),
        "{wrong:#?}"
    );
}

#[test]
fn a_planted_resolver_that_reads_and_discards_is_found_by_the_pipe_that_was_opened() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &reads_and_discards);
    assert!(
        wrong.is_empty(),
        "discarding what is read changes no output, so the pipe is what finds it: {wrong:#?}"
    );
    let touched = sentinels.touched(Duration::from_secs(5));
    let expected = if cfg!(windows) {
        "the named pipe was connected to"
    } else {
        "the FIFO was opened"
    };
    assert!(touched.contains(&expected), "{touched:?}");
}

#[test]
fn a_planted_resolver_that_connects_is_found_by_the_listener_that_accepted() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &connects);
    assert!(wrong.is_empty(), "{wrong:#?}");
    let touched = sentinels.touched(Duration::from_secs(5));
    assert!(
        touched.contains(&"a connection was accepted on 127.0.0.1"),
        "{touched:?}"
    );
}

// ---------------------------------------------------------------------------
// The corpus in a child process, which must print nothing.

const CHILD_REFERENCES: &str = "CRUCIBLE_RASTER_CHILD_REFERENCES";
const CHILD_RAN: &str = "ran";

/// Draws the corpus and the references the parent wrote, and says nothing. It
/// is run by its parent, which has the sentinels the references name.
fn the_whole_corpus() {
    let Ok(references) = std::env::var(CHILD_REFERENCES) else {
        return;
    };
    let mut drawn = 0_usize;
    for (name, _) in ENTRIES {
        let bytes = fs::read(Path::new(CORPUS).join(name)).expect("a corpus file");
        drop(draw(name, &bytes));
        drawn += 1;
    }
    for entry in fs::read_dir(&references).expect("the references of the parent") {
        let bytes = fs::read(entry.expect("a reference").path()).expect("a reference");
        drop(rasterise(&bytes, 100, INKS));
        drawn += 1;
    }
    fs::write(Path::new(&references).join(CHILD_RAN), drawn.to_string()).expect("a record");
}

#[test]
#[ignore = "run by its parent, in a child process, with the references written"]
fn child_draws_the_corpus() {
    let _alone = alone();
    the_whole_corpus();
}

// The stand-in for a dependency that prints: it draws the same corpus and then
// writes to the standard error behind an allow.
#[test]
#[ignore = "run by its parent, in a child process, to be found printing"]
fn child_draws_the_corpus_and_prints() {
    let _alone = alone();
    the_whole_corpus();
    #[allow(
        clippy::print_stderr,
        reason = "planted: the parent must find this line"
    )]
    {
        eprintln!("planted: a dependency that prints a diagnostic");
    }
}

// The same on the standard output.
#[test]
#[ignore = "run by its parent, in a child process, to be found printing"]
fn child_draws_the_corpus_and_prints_a_line() {
    let _alone = alone();
    the_whole_corpus();
    #[allow(
        clippy::print_stdout,
        reason = "planted: the parent must find this line"
    )]
    {
        println!("planted: a dependency that prints a line");
    }
}

/// What a test binary prints around one test that prints nothing.
fn only_the_harness_printed(stdout: &str) -> bool {
    stdout.lines().all(|line| {
        line.is_empty()
            || line == "running 1 test"
            || (line.starts_with("test child_draws_the_corpus") && line.ends_with(" ... ok"))
            || line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;")
    })
}

impl Child {
    fn was_silent(&self) -> bool {
        self.stderr.is_empty() && only_the_harness_printed(&self.stdout)
    }
}

struct Child {
    ok: bool,
    stdout: String,
    stderr: String,
    drawn: Option<usize>,
}

fn run_child(test: &str, sentinels: &Sentinels) -> Child {
    let references = sentinels.dir.join("references");
    fs::create_dir_all(&references).expect("a directory for the references");
    for (name, template) in templates() {
        if let Some(filled) = sentinels.fill(&template) {
            let (with, _) = with_and_without(&filled);
            fs::write(references.join(name), with).expect("a reference");
        }
    }
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--ignored",
            "--exact",
            test,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_REFERENCES, &references)
        .output()
        .expect("a child process");
    Child {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        drawn: fs::read_to_string(references.join(CHILD_RAN))
            .ok()
            .and_then(|text| text.parse().ok()),
    }
}

#[test]
fn the_whole_corpus_drawn_in_a_child_process_prints_nothing() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let child = run_child("child_draws_the_corpus", &sentinels);
    assert!(child.ok, "{}\n{}", child.stdout, child.stderr);
    assert!(
        child.drawn.is_some_and(|drawn| drawn > ENTRIES.len()),
        "the child drew {:?}: it did not run the corpus",
        child.drawn
    );
    assert!(
        child.was_silent(),
        "stdout: {:?}, stderr: {:?}",
        child.stdout,
        child.stderr
    );
    let touched = sentinels.touched(Duration::from_millis(300));
    assert!(touched.is_empty(), "{touched:?}");
}

#[test]
fn a_planted_diagnostic_behind_an_allow_is_found_by_the_child_process_check() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let child = run_child("child_draws_the_corpus_and_prints", &sentinels);
    assert!(child.ok, "{}\n{}", child.stdout, child.stderr);
    assert!(!child.was_silent(), "the planted line was not seen");
    assert!(child.stderr.contains("planted"), "{}", child.stderr);
}

#[test]
fn a_planted_line_on_the_standard_output_is_found_by_the_child_process_check() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let child = run_child("child_draws_the_corpus_and_prints_a_line", &sentinels);
    assert!(child.ok, "{}\n{}", child.stdout, child.stderr);
    assert!(!child.was_silent(), "the planted line was not seen");
    assert!(child.stdout.contains("planted"), "{}", child.stdout);
}
