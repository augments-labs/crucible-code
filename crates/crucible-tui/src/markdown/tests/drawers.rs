//! A closed fence handed to the drawer registered for its name: what the
//! drawer is given, what each of its answers draws, and what stays exactly as
//! it was written.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::chrome::Undrawn;
use crate::drawing::{Drawers, Draws, EXPAND, Laid, Room};
use crate::dump::dump;

/// A change to two files, as a model writes one in a fence.
const TWO_FILES: &str = "Before.\n\n```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n-let a = 1;\n+let a = 2;\n context\n--- a/y.rs\n+++ b/y.rs\n@@ -4 +4 @@\n-old\n+new\n```\n\nAfter.\n";

/// The lines between the fences of [`TWO_FILES`]: what a drawer is handed.
const TWO_FILES_BODY: &str = "--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n-let a = 1;\n+let a = 2;\n context\n--- a/y.rs\n+++ b/y.rs\n@@ -4 +4 @@\n-old\n+new\n";

/// What an empty registry draws [`TWO_FILES`] as: the fence as it was written.
const TWO_FILES_WRITTEN: &[&str] = &[
    "Before.",
    "",
    "┌─ diff",
    "│ --- a/x.rs",
    "│ +++ b/x.rs",
    "│ @@ -1,2 +1,2 @@",
    "│ -let a = 1;",
    "│ +let a = 2;",
    "│  context",
    "│ --- a/y.rs",
    "│ +++ b/y.rs",
    "│ @@ -4 +4 @@",
    "│ -old",
    "│ +new",
    "└─",
    "",
    "After.",
];

/// A drawer that answers as it is told to and remembers what it was asked.
struct Stub {
    names: &'static [&'static str],
    answer: Answer,
    asked: Mutex<Vec<(String, Room)>>,
}

impl Stub {
    /// One taking fences named `diff` and `patch`, answering with `answer`.
    fn answering(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            names: &["diff", "patch"],
            answer,
            asked: Mutex::new(Vec::new()),
        })
    }

    /// Every block it was asked for, with the room it was asked in.
    fn asked(&self) -> Vec<(String, Room)> {
        self.asked
            .lock()
            .map(|asked| asked.clone())
            .unwrap_or_default()
    }
}

impl Draws for Stub {
    fn names(&self) -> &'static [&'static str] {
        self.names
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push((source.to_owned(), *room));
        }
        match self.answer {
            Answer::Marked => Ok(marked(source, room)),
            Answer::NotHere => Err(Undrawn::NotHere),
            Answer::PastCap => Err(Undrawn::PastCap),
        }
    }
}

/// What a [`Stub`] answers every block with.
#[derive(Clone, Copy)]
enum Answer {
    /// The rows [`marked`] gives.
    Marked,
    /// That the block is not something it draws.
    NotHere,
    /// That the block is past its caps.
    PastCap,
}

/// One row for each line it was handed, under a mark no reader draws and cut
/// to the room, so a block handed over short or long draws other rows.
fn marked(source: &str, room: &Room) -> Laid {
    let mark = match room.glyphs {
        Glyphs::Unicode => "»",
        Glyphs::Ascii => ">",
    };
    let rows: Vec<Row> = source
        .lines()
        .map(|line| {
            Row::new()
                .then_structural(Slot::Quiet, mark)
                .then(Slot::Added, format!(" {line}"))
                .clipped(room.columns)
        })
        .collect();
    Laid {
        natural: (rows.iter().map(Row::columns).max().unwrap_or(0), rows.len()),
        rows,
        narrow: None,
    }
}

/// The registry holding `stub` alone.
fn holding(stub: &Arc<Stub>) -> Drawers {
    Drawers::default().with(Arc::clone(stub) as Arc<dyn Draws>)
}

/// The rows `deltas` come to at `room` in `glyphs`, each block the reader
/// hands on laid out by `drawers`, as the record lays one out.
fn laid(deltas: &[&str], room: usize, glyphs: Glyphs, drawers: &Drawers) -> Vec<Row> {
    let mut markdown = Markdown::new(glyphs);
    let mut rows = vec![Row::new()];
    let mut say = |said: Said<'_>| match said {
        Said::Run { slot, text, .. } => {
            let mut pieces = text.split('\n');
            if let (Some(piece), Some(row)) = (pieces.next(), rows.last_mut()) {
                row.push(slot, piece);
            }
            for piece in pieces {
                rows.push(Row::new().then(slot, piece));
            }
        }
        Said::Block(block) => {
            if rows.last().is_some_and(Row::is_empty) {
                rows.pop();
            }
            rows.extend(lay(&block, room, glyphs, &Links::default(), drawers));
            rows.push(Row::new());
        }
        Said::Retract => {
            rows.pop();
            rows.push(Row::new());
        }
        Said::Aside(text) => {
            if let Some(row) = rows.last_mut() {
                row.push(Slot::Quiet, text);
            }
        }
        Said::Shortened(_) => {}
    };
    for delta in deltas {
        markdown.read(delta, room, &mut say);
    }
    markdown.finish(room, &mut say);
    if rows.last().is_some_and(Row::is_empty) {
        rows.pop();
    }
    rows
}

/// What `rows` say, a row to a line.
fn texts(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// `rows` as what a reader sees: each row's words under the slot they wear,
/// with neighbours in one slot run together, since where a delta ended is no
/// part of what was drawn.
fn seen(rows: &[Row]) -> Vec<Vec<(Slot, String)>> {
    rows.iter()
        .map(|row| {
            let mut runs: Vec<(Slot, String)> = Vec::new();
            for (slot, text) in row.spans() {
                match runs.last_mut() {
                    Some((last, said)) if *last == slot => said.push_str(text),
                    _ => runs.push((slot, text.to_owned())),
                }
            }
            runs
        })
        .collect()
}

#[test]
fn a_closed_fence_a_registered_drawer_takes_is_drawn_by_that_drawer() {
    for (glyphs, mark) in [(Glyphs::Unicode, "»"), (Glyphs::Ascii, ">")] {
        let stub = Stub::answering(Answer::Marked);

        let rows = laid(&[TWO_FILES], 40, glyphs, &holding(&stub));

        let mut expected = vec!["Before.".to_owned(), String::new()];
        expected.extend(TWO_FILES_BODY.lines().map(|line| format!("{mark} {line}")));
        expected.extend([String::new(), "After.".to_owned()]);
        assert_eq!(texts(&rows), expected, "{glyphs:?}");
        // The rows are the drawer's own, slot for slot: nothing reads them
        // again as code, and no frame is drawn around them.
        assert_eq!(
            rows.get(2),
            Some(
                &Row::new()
                    .then_structural(Slot::Quiet, mark)
                    .then(Slot::Added, " --- a/x.rs")
            ),
            "{glyphs:?}"
        );
        // It was handed the lines between the fences and neither fence, in
        // the room and the glyph set the block was laid out for.
        assert_eq!(
            stub.asked(),
            [(
                TWO_FILES_BODY.to_owned(),
                Room {
                    columns: 40,
                    glyphs
                }
            )],
            "{glyphs:?}"
        );
    }
}

#[test]
fn a_fence_is_handed_to_the_drawer_its_first_word_names_in_any_case() {
    for (info, taken) in [
        ("diff", true),
        ("Diff", true),
        ("PATCH", true),
        ("diff a/x.rs b/x.rs", true),
        ("~~~", true),
        // Held by the reader, and named for no drawer registered here.
        ("mermaid", false),
        ("math", false),
    ] {
        let stub = Stub::answering(Answer::Marked);
        let answer = if info == "~~~" {
            "~~~diff\n+new\n~~~\n".to_owned()
        } else {
            format!("```{info}\n+new\n```\n")
        };

        let rows = laid(&[&answer], ROOM, Glyphs::Unicode, &holding(&stub));

        if taken {
            assert_eq!(texts(&rows), ["» +new"], "{info:?}");
            assert_eq!(stub.asked().len(), 1, "{info:?}");
        } else {
            assert_eq!(
                texts(&rows),
                texts(&laid(
                    &[&answer],
                    ROOM,
                    Glyphs::Unicode,
                    &Drawers::default()
                )),
                "{info:?}"
            );
            assert!(stub.asked().is_empty(), "{info:?}");
        }
    }
}

#[test]
fn display_math_between_dollars_is_no_fence_and_is_handed_to_no_drawer() {
    let stub = Arc::new(Stub {
        names: &["math", "$$", ""],
        answer: Answer::Marked,
        asked: Mutex::new(Vec::new()),
    });
    let answer = "$$\nx + y\n$$\n";

    let rows = laid(&[answer], ROOM, Glyphs::Unicode, &holding(&stub));

    assert_eq!(texts(&rows), ["$$", "x + y", "$$"]);
    assert!(stub.asked().is_empty());
}

#[test]
fn with_no_drawer_registered_every_fence_is_drawn_exactly_as_it_was_written() {
    let none = Drawers::default();

    let rows = laid(&[TWO_FILES], ROOM, Glyphs::Unicode, &none);

    assert_eq!(texts(&rows), TWO_FILES_WRITTEN);
    // Slot for slot too: the frame quiet, the gutter quiet, the code in the
    // colours the fence's language gives it.
    assert_eq!(
        seen(&rows).get(2..7),
        Some(
            &[
                vec![(Slot::Quiet, "┌─ diff".to_owned())],
                vec![
                    (Slot::Quiet, "│ ".to_owned()),
                    (Slot::Plain, "--- a/x.rs".to_owned())
                ],
                vec![
                    (Slot::Quiet, "│ ".to_owned()),
                    (Slot::Plain, "+++ b/x.rs".to_owned())
                ],
                vec![
                    (Slot::Quiet, "│ ".to_owned()),
                    (Slot::Plain, "@@ -1,2 +1,2 @@".to_owned())
                ],
                vec![
                    (Slot::Quiet, "│ ".to_owned()),
                    (Slot::Plain, "-let a = ".to_owned()),
                    (Slot::Figure, "1".to_owned()),
                    (Slot::Plain, ";".to_owned())
                ],
            ][..]
        )
    );
    assert_eq!(
        texts(&laid(&[TWO_FILES], ROOM, Glyphs::Ascii, &none)).get(2..4),
        Some(&["+- diff".to_owned(), "| --- a/x.rs".to_owned()][..])
    );
    // The other fences the reader holds, and one it does not.
    for (answer, written) in [
        (
            "```patch\n-old\n+new\n```\n",
            &["┌─ patch", "│ -old", "│ +new", "└─"][..],
        ),
        (
            "```mermaid\ngraph TD\n```\n",
            &["┌─ mermaid", "│ graph TD", "└─"][..],
        ),
        ("```math\nx + y\n```\n", &["┌─ math", "│ x + y", "└─"][..]),
        ("```text\nwords\n```\n", &["┌─ text", "│ words", "└─"][..]),
    ] {
        assert_eq!(
            texts(&laid(&[answer], ROOM, Glyphs::Unicode, &none)),
            written,
            "{answer:?}"
        );
    }
}

#[test]
fn a_fence_its_drawer_says_is_not_here_is_drawn_as_written_and_says_nothing_of_it() {
    let stub = Stub::answering(Answer::NotHere);

    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = laid(&[TWO_FILES], ROOM, glyphs, &holding(&stub));

        // Row for row and slot for slot what no drawer at all leaves: no
        // refusal on the frame, and no row saying one.
        assert_eq!(
            rows,
            laid(&[TWO_FILES], ROOM, glyphs, &Drawers::default()),
            "{glyphs:?}"
        );
        assert!(
            texts(&rows).iter().all(|row| !row.contains("not drawn")),
            "{rows:?}"
        );
    }
    assert_eq!(
        texts(&laid(&[TWO_FILES], ROOM, Glyphs::Unicode, &holding(&stub))),
        TWO_FILES_WRITTEN
    );
    assert_eq!(
        stub.asked().len(),
        3,
        "asked once each time it was laid out"
    );
}

#[test]
fn a_fence_past_its_drawers_caps_is_drawn_as_written_in_the_frame_a_held_block_past_its_cap_has() {
    let stub = Stub::answering(Answer::PastCap);

    let rows = laid(&[TWO_FILES], ROOM, Glyphs::Unicode, &holding(&stub));

    let written: Vec<String> = TWO_FILES_WRITTEN
        .iter()
        .map(|row| match *row {
            "┌─ diff" => "┌─ diff · longer than this can hold · shown as written".to_owned(),
            row => row.to_owned(),
        })
        .collect();
    assert_eq!(texts(&rows), written);
    // The words are the ones the reader draws for a block it held past its
    // own cap, in the slot it draws them in.
    let held_too_long = past_its_cap("diff", "+added\n", DIFF_CAP);
    let reader = laid(
        &[&held_too_long],
        ROOM,
        Glyphs::Unicode,
        &Drawers::default(),
    );
    assert_eq!(rows.get(2), reader.first());
    assert_eq!(
        texts(&laid(&[TWO_FILES], ROOM, Glyphs::Ascii, &holding(&stub))).get(2),
        Some(&"+- diff - longer than this can hold - shown as written".to_owned())
    );
}

#[test]
fn a_fence_the_answer_ended_inside_is_drawn_as_written_and_handed_to_no_drawer() {
    for answer in [
        "```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n",
        "```diff\n-old\n+new",
        "```diff\n-old\n+new\n``",
        "```diff\n",
    ] {
        let stub = Stub::answering(Answer::Marked);

        let rows = laid(&[answer], ROOM, Glyphs::Unicode, &holding(&stub));

        assert_eq!(
            rows,
            laid(&[answer], ROOM, Glyphs::Unicode, &Drawers::default()),
            "{answer:?}"
        );
        assert_eq!(texts(&rows).first().map(String::as_str), Some("┌─ diff"));
        assert!(stub.asked().is_empty(), "{answer:?}");
    }
}

#[test]
fn a_drawn_fence_is_asked_for_again_at_every_room_it_is_laid_out_in() {
    let stub = Stub::answering(Answer::Marked);
    let drawers = holding(&stub);
    let fence = "```diff\n+a line longer than the narrow room\n```\n";
    let (held, _) = blocks(fence);
    assert_eq!(held.len(), 1, "{fence:?}");
    let Some(block) = held.first() else {
        panic!("{fence:?} was not held");
    };

    let wide = lay(block, 60, Glyphs::Unicode, &Links::default(), &drawers);
    let narrow = lay(block, 12, Glyphs::Ascii, &Links::default(), &drawers);

    assert_eq!(texts(&wide), ["» +a line longer than the narrow room"]);
    assert_eq!(texts(&narrow), ["> +a line lo"]);
    assert_eq!(
        stub.asked()
            .iter()
            .map(|(_, room)| *room)
            .collect::<Vec<_>>(),
        [
            Room {
                columns: 60,
                glyphs: Glyphs::Unicode
            },
            Room {
                columns: 12,
                glyphs: Glyphs::Ascii
            }
        ]
    );
}

#[test]
fn a_page_printed_once_is_told_the_rows_the_record_lays_a_drawn_fence_out_as() {
    for answer in [Answer::Marked, Answer::NotHere, Answer::PastCap] {
        let drawers = holding(&Stub::answering(answer));
        let fence = "```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n```\n";
        let (held, _) = blocks(fence);
        assert_eq!(held.len(), 1, "{fence:?}");
        let Some(block) = held.first() else {
            panic!("{fence:?} was not held");
        };
        for (room, glyphs) in [(ROOM, Glyphs::Unicode), (9, Glyphs::Ascii)] {
            let mut told = vec![Row::new()];
            tell(
                block,
                Room {
                    columns: room,
                    glyphs,
                },
                &Links::default(),
                &drawers,
                &mut |slot, text, _| {
                    let mut pieces = text.split('\n');
                    if let (Some(piece), Some(row)) = (pieces.next(), told.last_mut()) {
                        row.push(slot, piece);
                    }
                    for piece in pieces {
                        told.push(Row::new().then(slot, piece));
                    }
                },
            );
            if told.last().is_some_and(Row::is_empty) {
                told.pop();
            }

            let rows = lay(block, room, glyphs, &Links::default(), &drawers);

            assert!(!rows.is_empty());
            assert_eq!(seen(&told), seen(&rows), "{room} columns in {glyphs:?}");
        }
    }
}

/// A drawer that takes `diff` and draws one row, how many lines it was handed
/// and an offer of the key that would show them. It declines a block that
/// says `not a change`, takes the offer back out of rows when it is asked to,
/// and counts how often it was asked.
#[derive(Default)]
struct Offering {
    withdrawn: AtomicUsize,
}

impl Draws for Offering {
    fn names(&self) -> &'static [&'static str] {
        &["diff"]
    }

    fn block(&self, source: &str, _: &Room) -> Result<Laid, Undrawn> {
        if source.contains("not a change") {
            return Err(Undrawn::NotHere);
        }
        let row = Row::new()
            .then(
                Slot::Quiet,
                format!("{} lines held · ", source.lines().count()),
            )
            .then(Slot::Cut, EXPAND);
        Ok(Laid {
            natural: (row.columns(), 1),
            rows: vec![row],
            narrow: None,
        })
    }

    fn unoffered(&self, rows: &mut [Row]) {
        self.withdrawn.fetch_add(1, Ordering::Relaxed);
        for row in rows {
            row.rewrite(|_, slot, text| {
                (slot != Slot::Cut).then(|| (slot, text.trim_end_matches(" · ").to_owned()))
            });
        }
    }
}

/// What a page printed once is told of the one block `fence` is, by `drawers`
/// at [`ROOM`], as rows.
fn told(fence: &str, drawers: &Drawers) -> Vec<Row> {
    let (held, _) = blocks(fence);
    let Some(block) = held.first() else {
        panic!("{fence:?} was not held");
    };
    let room = Room {
        columns: ROOM,
        glyphs: Glyphs::Unicode,
    };
    let mut told = vec![Row::new()];
    tell(
        block,
        room,
        &Links::default(),
        drawers,
        &mut |slot, text, _| {
            let mut pieces = text.split('\n');
            if let (Some(piece), Some(row)) = (pieces.next(), told.last_mut()) {
                row.push(slot, piece);
            }
            for piece in pieces {
                told.push(Row::new().then(slot, piece));
            }
        },
    );
    if told.last().is_some_and(Row::is_empty) {
        told.pop();
    }
    told
}

#[test]
fn a_drawer_takes_its_offer_of_a_key_out_of_what_a_page_printed_once_is_told_and_no_other_rows() {
    let offering = Arc::new(Offering::default());
    let drawers = Drawers::default().with(Arc::clone(&offering) as Arc<dyn Draws>);
    let asked = || offering.withdrawn.load(Ordering::Relaxed);
    let fence = "```diff\n-old\n+new\n```\n";
    let (held, _) = blocks(fence);
    let Some(block) = held.first() else {
        panic!("{fence:?} was not held");
    };

    // Laid out for a screen the offer stands, and the drawer is not asked.
    let rows = lay(block, ROOM, Glyphs::Unicode, &Links::default(), &drawers);
    assert_eq!(
        seen(&rows),
        [[
            (Slot::Quiet, "2 lines held · ".to_owned()),
            (Slot::Cut, EXPAND.to_owned())
        ]]
    );
    assert_eq!(asked(), 0);

    // No key is read on a page printed once: the drawer takes it out.
    assert_eq!(
        seen(&told(fence, &drawers)),
        [[(Slot::Quiet, "2 lines held".to_owned())]]
    );
    assert_eq!(asked(), 1);

    // A fence it declined is written as it was, and is not its rows to edit.
    let declined = "```diff\nnot a change\n```\n";
    assert_eq!(
        texts(&told(declined, &drawers)),
        ["┌─ diff", "│ not a change", "└─"]
    );
    assert_eq!(asked(), 1);
}

#[test]
fn a_fence_cut_anywhere_is_handed_to_its_drawer_whole_and_draws_what_the_whole_answer_draws() {
    let whole_stub = Stub::answering(Answer::Marked);
    let whole = laid(&[TWO_FILES], 30, Glyphs::Unicode, &holding(&whole_stub));
    assert_eq!(whole.len(), 15, "{whole:?}");

    // Inside a file header, a hunk header, a changed line and the closing
    // fence: each is one of the cuts below.
    let inside = |of: &str, by: usize| TWO_FILES.find(of).map(|at| at + by);
    let named = [
        inside("+++ b/y.rs", 5),
        inside("@@ -1,2", 4),
        inside("-let a", 6),
        inside("```\n\nAfter", 2),
    ];
    let mut cuts = Vec::new();

    for (at, _) in TWO_FILES.char_indices().skip(1) {
        let (before, after) = TWO_FILES.split_at(at);
        let stub = Stub::answering(Answer::Marked);

        let rows = laid(&[before, after], 30, Glyphs::Unicode, &holding(&stub));

        assert_eq!(seen(&rows), seen(&whole), "cut at {at}: {before:?}");
        assert_eq!(
            stub.asked(),
            whole_stub.asked(),
            "cut at {at}: the drawer is handed the block once, whole"
        );
        cuts.push(Some(at));
    }
    for cut in named {
        assert!(cut.is_some() && cuts.contains(&cut), "{cut:?}");
    }

    // And a character at a time, which cuts it everywhere at once.
    let characters: Vec<String> = TWO_FILES.chars().map(String::from).collect();
    let pieces: Vec<&str> = characters.iter().map(String::as_str).collect();
    let stub = Stub::answering(Answer::Marked);
    let rows = laid(&pieces, 30, Glyphs::Unicode, &holding(&stub));
    assert_eq!(seen(&rows), seen(&whole));
    assert_eq!(stub.asked(), whole_stub.asked());
}

#[test]
fn a_fence_is_handed_to_its_drawer_up_to_the_most_the_reader_holds_and_not_past_it() {
    let line = "+added\n";
    let framing = "```diff\n".len() + "```\n".len();
    for (bytes, held) in [
        (DIFF_CAP - 1, true),
        (DIFF_CAP, true),
        (DIFF_CAP + 1, false),
    ] {
        // A fence of exactly `bytes` bytes, fence lines and all.
        let between = bytes - framing;
        let mut body = line.repeat(between / line.len());
        let short = between % line.len();
        if short > 0 {
            body.push_str(&" ".repeat(short - 1));
            body.push('\n');
        }
        let answer = format!("```diff\n{body}```\n");
        assert_eq!(answer.len(), bytes);
        let stub = Stub::answering(Answer::Marked);

        let rows = laid(&[&answer], ROOM, Glyphs::Unicode, &holding(&stub));

        if held {
            // Its length and then its text, so a failure prints no 16 KiB.
            let asked = stub.asked();
            let room = Room {
                columns: ROOM,
                glyphs: Glyphs::Unicode,
            };
            assert_eq!(asked.len(), 1, "{bytes}");
            assert!(
                asked
                    .iter()
                    .all(|(handed, at)| *handed == body && *at == room),
                "{bytes}"
            );
            assert_eq!(
                texts(&rows).first().map(String::as_str),
                Some("» +added"),
                "{bytes}"
            );
        } else {
            assert!(stub.asked().is_empty(), "{bytes}");
            assert_eq!(
                texts(&rows).first().map(String::as_str),
                Some("┌─ diff · longer than this can hold · shown as written"),
                "{bytes}"
            );
        }
    }
}

#[test]
fn what_a_reader_links_to_is_compared_by_its_forge_and_its_files_alone() {
    fn compared<T: std::fmt::Debug + Clone + Default + PartialEq + Eq>() {}
    compared::<Links>();

    let counting = Links {
        forge: Some(Forge::new("https://github.com", "owner/repo", "/issues/")),
        files: None,
    };
    assert_eq!(
        Links::default(),
        Links {
            forge: None,
            files: None
        }
    );
    assert_eq!(counting, counting.clone());
    assert_ne!(counting, Links::default());
    // A registry of drawers is no part of it: the drawers a block is laid out
    // by are handed beside the links, and two readers that link alike compare
    // alike whatever draws their fences.
    let drawers = holding(&Stub::answering(Answer::Marked));
    let fence = "```diff\n+new\n```\n";
    let (held, _) = blocks(fence);
    assert_eq!(held.len(), 1, "{fence:?}");
    let Some(block) = held.first() else {
        panic!("{fence:?} was not held");
    };
    let before = counting.clone();
    let rows = lay(block, ROOM, Glyphs::Unicode, &counting, &drawers);
    assert_eq!(texts(&rows), ["» +new"]);
    assert_eq!(counting, before);
}

#[test]
fn a_fence_drawn_by_a_stub_drawer_and_the_same_fence_with_none_registered_are_pictured() {
    let stub = Stub::answering(Answer::Marked);
    insta::with_settings!({ snapshot_path => "../snapshots" }, {
        insta::assert_snapshot!(
            "a_fence_a_stub_drawer_draws",
            dump(&laid(&[TWO_FILES], 40, Glyphs::Unicode, &holding(&stub)), 40)
        );
        insta::assert_snapshot!(
            "a_fence_as_written_with_no_drawer_registered",
            dump(&laid(&[TWO_FILES], 40, Glyphs::Unicode, &Drawers::default()), 40)
        );
    });
}
