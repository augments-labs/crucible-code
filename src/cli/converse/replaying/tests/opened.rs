//! Every row a session put back offers to expand opens.
//!
//! A long session holds more results than the store keeps, so the rows a
//! replay drew first have had their text let go of by the time the last is
//! drawn. Where the session has a log those rows still open, with the result
//! read back from it; where it has none they stop offering. The fixtures, and
//! the helpers that find the offering rows on the screen without asking the
//! store, are the ones the defect was first shown with.

use crucible_tools::ToolOutput;
use crucible_tui::Pressed;

use super::*;
use crate::cli::converse::expanding::{self, Standing};
use crate::cli::sample::Sample;

/// How wide the terminal in the tests below is: wide enough that a result row
/// has room for the first words of its result and for the offer after them.
const BROAD: usize = 120;

/// How many rows its window has: more than any session below is put back as,
/// so every row of the record is a row of the window and a row of the window
/// can be asked which line of the record it is showing.
const TALL: usize = 400;

/// Lines in each result: 25,600 bytes, under the 30,000 a recorded result may
/// be once it is encoded, and forty of them twice what the store holds.
const LINES: usize = 400;

/// What stands in the view in place of a result the log could not give back.
const UNREAD: &str = "! this result could not be read back from the session log";

/// What result `number` of a session said: `lines` lines of sixty-four bytes,
/// each naming the result it is a line of.
fn printed(number: usize, lines: usize) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(lines * 64);
    for line in 1..=lines {
        let said = format!("result {number:03} line {line:04} ");
        let _ = writeln!(text, "{said:-<63}");
    }
    text
}

/// A session that ran `count` commands, one call and one result to a round
/// trip, every result `lines` lines long.
///
/// A tool this build does not name, so each call keeps a row of its own and the
/// row is the bare name: what is held for each is the result and four bytes.
fn ran_commands(count: usize, lines: usize) -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("run every one of them"))
        .expect("valid fixture transcript");

    for number in 1..=count {
        let id = ToolId::new(format!("b-{number}"));
        transcript
            .push(Message::Agent {
                continuation: None,
                text: String::new().into(),
                calls: vec![ToolCall {
                    id: id.clone(),
                    name: "bash".into(),
                    args: ToolArgs::new(r#"{"command":"make"}"#),
                }],
                stop: Some(StopReason::WantsTools),
            })
            .expect("valid fixture transcript");
        transcript
            .push(Message::ToolResults(vec![ToolResult {
                id,
                output: RecordedToolOutput::ok(printed(number, lines)),
            }]))
            .expect("valid fixture transcript");
    }

    transcript
}

/// One row of the window that offers to open a result.
struct Offer {
    /// The line of the record a click on the row lands on, asked of the
    /// renderer the way a click asks it.
    line: usize,
    /// What the row says.
    says: String,
    /// Whether the row is drawn in the slot a cut result is drawn in.
    cut: bool,
}

/// What a session put back left on the screen and left held.
struct PutBack {
    /// Every row that offers to open a result, from the top of the window down.
    offers: Vec<Offer>,
    /// What is held for the key to open.
    kept: Kept,
    /// The terminal it was put back on, for the view to stand on.
    renderer: Renderer<Recording>,
}

/// Every row of the window that offers to open a result, read off the screen.
///
/// Which line each offer is on is read from the renderer and the window, and
/// nothing is read from what is held: the rows are the record's own, the line
/// is the one the renderer answers a click on that row with, and the window the
/// terminal was left with has to say the same words on the same row.
fn offers(renderer: &Renderer<Recording>) -> Vec<Offer> {
    let rows = renderer.tail(TALL);
    assert!(
        rows.len() < TALL,
        "the window is not tall enough for the test to read every row: {}",
        rows.len()
    );
    let picture = renderer.terminal().picture();

    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.text().contains("ctrl+o to expand"))
        .map(|(at, row)| {
            let says = row.text();
            assert!(
                picture.row(at).contains(says.trim_end()),
                "window row {at} says {:?} and the record says {says:?}",
                picture.row(at)
            );
            let Some(crucible_tui::Aimed::Line(line)) = renderer.aimed(at) else {
                panic!("window row {at} is no line of the record: {says:?}");
            };
            Offer {
                line,
                cut: row.kinds().any(|slot| slot == Slot::Cut),
                says,
            }
        })
        .collect()
}

/// Puts `transcript` back on a terminal, from `session`'s log where it has one,
/// and reads the offers off the screen.
fn put_back(transcript: Transcript, session: &Arc<Session>) -> PutBack {
    let runner = resumed(transcript);
    let mut kept = Kept::default();
    let mut renderer = Renderer::new(Recording::new(BROAD, TALL));
    renderer.wears(Style::plain().palette());
    assert!(
        renderer.is_terminal(),
        "the recording claims to be a terminal"
    );

    replayed(
        &mut renderer,
        &against(&runner, &Pruned::default()),
        session,
        &mut kept,
    )
    .expect("a recording cannot fail");

    PutBack {
        offers: offers(&renderer),
        kept,
        renderer,
    }
}

/// What the screen has to say before anything is asked of what is held: one
/// offer for each result put back, in the order they were put back, each on a
/// line of its own.
fn one_offer_to_a_result(back: &PutBack, replayed: usize) {
    assert_eq!(
        back.offers.len(),
        replayed,
        "a row that offers to open it, for each result put back"
    );
    for (at, offer) in back.offers.iter().enumerate() {
        let names = format!("result {:03} line 0001", at + 1);
        assert!(offer.says.contains(&names), "{names}: {:?}", offer.says);
        assert!(
            offer.cut,
            "not in the slot of a cut result: {:?}",
            offer.says
        );
    }
    assert!(
        back.offers
            .windows(2)
            .all(|pair| matches!(pair, [above, below] if above.line < below.line)),
        "the offers are not on lines of their own"
    );
}

/// Records `transcript` into a session log and picks that session back up, as
/// a session continued from the command line is.
fn logged(sample: &Sample, transcript: &Transcript) -> Arc<Session> {
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    for message in transcript.messages() {
        session.append(message);
    }
    // Dropping waits for the queue, and lets go of the claim the pick-up takes.
    drop(session);

    Arc::new(
        Session::resume(&sample.logs(), &sample.workspace())
            .expect("the session")
            .0,
    )
}

/// Stands the view over the row the record line `line` is, the way a click on
/// it does, and says what the window then shows.
fn opened(back: &mut PutBack, line: usize) -> String {
    let mut standing = Standing::default();
    standing.one(&back.kept, line);
    assert!(standing.is_open(), "row {line} opened nothing");

    assert!(
        expanding::under(
            &mut back.renderer,
            Style::plain(),
            &back.kept,
            &mut standing
        )
        .expect("a recording cannot fail"),
        "row {line} opened a view with no room to stand"
    );
    back.renderer.terminal().picture().rows().join("\n")
}

/// A result's own words that only the view shows: the row under its call
/// shows its first line and nothing after it.
fn inside(number: usize) -> String {
    format!("result {number:03} line 0002")
}

#[test]
fn every_row_a_logged_session_put_back_opens_with_its_own_result() {
    // Forty results of 25,600 bytes, twice what the store holds: the first rows
    // drawn have had their text let go of by the time the last is drawn, and
    // they open all the same, each on its own result, read back from the log.
    let sample = Sample::new("opened-every-row");
    let commands = ran_commands(40, LINES);
    let session = logged(&sample, &commands);
    let mut back = put_back(commands, &session);

    one_offer_to_a_result(&back, 40);
    assert!(
        back.kept.newest().count() < 40,
        "the store held every result; the test says nothing"
    );

    let lines: Vec<usize> = back.offers.iter().map(|offer| offer.line).collect();
    for (number, line) in (1..).zip(lines) {
        let shown = opened(&mut back, line);
        assert!(
            shown.contains(&inside(number)),
            "row {line} did not open result {number}:\n{shown}"
        );
        assert!(!shown.contains(UNREAD), "{shown}");
    }
}

#[test]
fn opening_every_row_leaves_the_log_as_it_was() {
    let sample = Sample::new("opened-reads-only");
    let commands = ran_commands(40, LINES);
    let session = logged(&sample, &commands);
    let before = std::fs::read(session.path()).expect("the log");
    let mut back = put_back(commands, &session);

    let lines: Vec<usize> = back.offers.iter().map(|offer| offer.line).collect();
    for line in lines {
        opened(&mut back, line);
    }

    assert_eq!(std::fs::read(session.path()).expect("the log"), before);
}

/// One way a log comes to hold less than was read from it, by name.
type Breaking = (&'static str, fn(&std::path::Path));

#[test]
fn a_result_the_log_cannot_give_back_is_said_to_be_missing_and_the_others_still_open() {
    // The log cut short, a line taken out of it, and the log gone altogether:
    // each is a result that cannot be read back, which the view says in its
    // place, and none of them stops a result the store still holds opening.
    let broken: [Breaking; 3] = [
        ("cut-short", |log| {
            let text = std::fs::read_to_string(log).expect("the log");
            let at = text.find(r#""b-1""#).expect("the first call") + 20;
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(log)
                .expect("the log");
            file.set_len(u64::try_from(at).expect("a short log"))
                .expect("cut");
        }),
        ("line-removed", |log| {
            let text = std::fs::read_to_string(log).expect("the log");
            let kept: Vec<&str> = text
                .lines()
                .filter(|line| !(line.contains(r#""b-1""#) && line.contains("result 001")))
                .collect();
            std::fs::write(log, kept.join("\n") + "\n").expect("rewritten");
        }),
        ("gone", |log| std::fs::remove_file(log).expect("removed")),
    ];

    for (how, breaking) in broken {
        let sample = Sample::new(&format!("opened-{how}"));
        let commands = ran_commands(40, LINES);
        let session = logged(&sample, &commands);
        let mut back = put_back(commands, &session);
        breaking(session.path());

        let first = back
            .offers
            .first()
            .map(|offer| offer.line)
            .expect("an offer");
        let shown = opened(&mut back, first);
        assert!(shown.contains(UNREAD), "{how}: {shown}");

        let last = back
            .offers
            .last()
            .map(|offer| offer.line)
            .expect("an offer");
        let shown = opened(&mut back, last);
        assert!(shown.contains(&inside(40)), "{how}: {shown}");
    }
}

#[test]
fn with_no_log_a_row_whose_result_went_stops_offering_and_every_row_that_offers_opens() {
    // A session recording nowhere has nothing to read a result back from, so a
    // row the store let go of has nothing behind it: it keeps its words and
    // loses the offer, and the rows still offering are exactly the ones that
    // open.
    let commands = ran_commands(40, LINES);
    let mut back = put_back(commands, &Arc::new(Session::nowhere()));

    assert!(
        !back.offers.is_empty() && back.offers.len() < 40,
        "{} rows offer",
        back.offers.len()
    );
    for offer in &back.offers {
        assert!(back.kept.offered(offer.line), "{:?}", offer.says);
        assert!(offer.cut, "{:?}", offer.says);
    }

    // The first results' rows are still there, saying what they said, and
    // lighting nothing.
    let rows = back.renderer.tail(TALL);
    let first = rows
        .iter()
        .find(|row| row.text().contains("result 001 line 0001"))
        .expect("the first result's row is still on the screen");
    assert!(
        !first.text().contains("ctrl+o to expand"),
        "{:?}",
        first.text()
    );
    assert!(first.kinds().all(|slot| slot != Slot::Cut), "{first:?}");

    let lines: Vec<usize> = back.offers.iter().map(|offer| offer.line).collect();
    let newest = 40 - lines.len();
    for (number, line) in (newest + 1..).zip(lines) {
        let shown = opened(&mut back, line);
        assert!(shown.contains(&inside(number)), "row {line}:\n{shown}");
    }
}

#[test]
fn a_live_result_the_store_let_go_of_opens_once_the_last_has_arrived() {
    // The same forty, arriving in a session that records them as they come: the
    // store lets the first go while the turn is still running, and the log's
    // writer says where it put each one, so the oldest row opens once the last
    // has arrived.
    let sample = Sample::new("opened-live");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a session"));
    let mut kept = Kept::default();
    logging(&mut kept, &session);
    let mut renderer = Renderer::new(Recording::new(BROAD, TALL));
    renderer.wears(Style::plain().palette());

    session.append(&Message::said("run every one of them"));
    for number in 1..=40 {
        let id = ToolId::new(format!("b-{number}"));
        session.append(&Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: vec![ToolCall {
                id: id.clone(),
                name: "bash".into(),
                args: ToolArgs::new(r#"{"command":"make"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        });
        kept.calling(id.clone(), "bash".to_owned());
        draw::returned(&mut renderer, "bash", Style::plain()).expect("a recording cannot fail");
        let output = ToolOutput::ok(printed(number, LINES));
        draw::came_back(
            &mut renderer,
            &mut kept,
            &id,
            draw::Shown::live(output.clone()),
            Style::plain(),
        )
        .expect("a recording cannot fail");
        session.append(&Message::ToolResults(vec![ToolResult {
            id,
            output: output.into_recorded(),
        }]));
    }

    let mut back = PutBack {
        offers: offers(&renderer),
        kept,
        renderer,
    };
    one_offer_to_a_result(&back, 40);
    assert!(
        back.kept.newest().count() < 40,
        "the store held every result"
    );

    let oldest = back
        .offers
        .first()
        .map(|offer| offer.line)
        .expect("an offer");
    let shown = opened(&mut back, oldest);
    assert!(shown.contains(&inside(1)), "{shown}");
}

/// One way a result is taken out of what the model is sent, by name.
type Clearing = (&'static str, fn(&Session));

#[test]
fn a_result_a_clearing_took_from_the_model_opens_with_the_words_its_row_showed() {
    // A pruning and a restriction each take the words out of what the model is
    // sent and leave them in the log. The row was drawn with them, so the row
    // opens with them.
    let clearings: [Clearing; 2] = [
        ("pruned", |session| {
            session.pruned(25, &[ToolId::new("b-1")]);
        }),
        ("restricted", |session| {
            session.restricted(25, &[ToolId::new("b-1")], "[cleared]");
        }),
    ];

    for (how, clearing) in clearings {
        let sample = Sample::new(&format!("opened-after-{how}"));
        let commands = ran_commands(40, LINES);
        let recording =
            Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
        for message in commands.messages() {
            recording.append(message);
        }
        clearing(&recording);
        drop(recording);
        let (session, _) =
            Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

        let mut back = put_back(commands, &Arc::new(session));
        let first = back
            .offers
            .first()
            .map(|offer| offer.line)
            .expect("an offer");
        let shown = opened(&mut back, first);
        assert!(shown.contains(&inside(1)), "{how}:\n{shown}");
    }
}

/// Waits until the log of `session` holds `text`, which the writer puts there
/// in its own time.
fn landed(session: &Session, text: &str) {
    let since = std::time::Instant::now();
    while !std::fs::read_to_string(session.path()).is_ok_and(|log| log.contains(text)) {
        assert!(
            since.elapsed() < std::time::Duration::from_secs(5),
            "{text:?} never reached the log"
        );
        std::thread::yield_now();
    }
}

/// A live session recording to its log, the store reading back from it, and
/// a terminal tall enough to read every row off.
fn live(name: &str) -> (Sample, Arc<Session>, Kept, Renderer<Recording>) {
    let sample = Sample::new(name);
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a session"));
    let mut kept = Kept::default();
    logging(&mut kept, &session);
    let mut renderer = Renderer::new(Recording::new(BROAD, TALL));
    renderer.wears(Style::plain().palette());
    session.append(&Message::said("run every one of them"));
    (sample, session, kept, renderer)
}

/// One call of `bash`, asked for in a message of its own.
fn asked_for(ids: &[ToolId]) -> Message {
    Message::Agent {
        continuation: None,
        text: String::new().into(),
        calls: ids
            .iter()
            .map(|id| ToolCall {
                id: id.clone(),
                name: "bash".into(),
                args: ToolArgs::new(r#"{"command":"make"}"#),
            })
            .collect(),
        stop: Some(StopReason::WantsTools),
    }
}

/// Draws the call line and result `number` of `lines` lines, and says which
/// line of the record its offer went on.
fn drawn(
    renderer: &mut Renderer<Recording>,
    kept: &mut Kept,
    id: &ToolId,
    number: usize,
    lines: usize,
) -> usize {
    draw::returned(renderer, "bash", Style::plain()).expect("a recording cannot fail");
    let line = renderer.lines();
    draw::came_back(
        renderer,
        kept,
        id,
        draw::Shown::live(ToolOutput::ok(printed(number, lines))),
        Style::plain(),
    )
    .expect("a recording cannot fail");
    line
}

/// The results of `ids`, numbered from `first`, as the log records them.
fn results(ids: &[ToolId], first: usize, lines: usize) -> Message {
    Message::ToolResults(
        (first..)
            .zip(ids)
            .map(|(number, id)| ToolResult {
                id: id.clone(),
                output: ToolOutput::ok(printed(number, lines)).into_recorded(),
            })
            .collect(),
    )
}

#[test]
fn a_batch_the_log_took_before_its_rows_were_drawn_still_opens_every_row() {
    // A turn records a batch of results once it has handed them to the screen,
    // and the screen draws them in its own time, so the log can say where a
    // result went before its row is drawn. The ceiling is crossed in the
    // middle of such a batch, and the rows drawn after that, let go of in
    // their turn, still open.
    let (_sample, session, mut kept, mut renderer) = live("opened-batch-first");

    let mut lines = Vec::new();
    for turn in 0..6 {
        let ids: Vec<ToolId> = (1..=8)
            .map(|call| ToolId::new(format!("b-{}", turn * 8 + call)))
            .collect();
        session.append(&asked_for(&ids));
        for id in &ids {
            kept.calling(id.clone(), "bash".to_owned());
        }
        session.append(&results(&ids, turn * 8 + 1, LINES));
        landed(&session, &inside(turn * 8 + 8));

        for (number, id) in (turn * 8 + 1..).zip(&ids) {
            lines.push(drawn(&mut renderer, &mut kept, id, number, LINES));
        }
    }
    assert!(
        kept.newest().count() < 24,
        "the store held too much to say anything"
    );

    let mut back = PutBack {
        offers: offers(&renderer),
        kept,
        renderer,
    };
    one_offer_to_a_result(&back, 48);
    for (number, line) in (1..).zip(lines) {
        let shown = opened(&mut back, line);
        assert!(
            shown.contains(&inside(number)),
            "row {line} did not open result {number}:\n{shown}"
        );
    }
}

#[test]
fn a_row_kept_long_before_the_first_let_go_still_opens() {
    // More results than the log's writer keeps places for arrive before the
    // store lets any go, each short and each cut. The first of them is let go
    // of later, and still opens.
    let (_sample, session, mut kept, mut renderer) = live("opened-kept-long");

    let mut first = None;
    for number in 1..=1100 {
        let id = ToolId::new(format!("b-{number}"));
        session.append(&asked_for(std::slice::from_ref(&id)));
        kept.calling(id.clone(), "bash".to_owned());
        let line = drawn(&mut renderer, &mut kept, &id, number, 3);
        first.get_or_insert(line);
        session.append(&results(std::slice::from_ref(&id), number, 3));
    }
    for number in 1101..=1125 {
        let id = ToolId::new(format!("b-{number}"));
        session.append(&asked_for(std::slice::from_ref(&id)));
        kept.calling(id.clone(), "bash".to_owned());
        drawn(&mut renderer, &mut kept, &id, number, LINES);
        session.append(&results(std::slice::from_ref(&id), number, LINES));
    }
    assert!(
        kept.newest()
            .all(|whole| !whole.text().contains(&inside(1))),
        "the first result is still held"
    );

    let first = first.expect("a first row");
    let mut back = PutBack {
        offers: Vec::new(),
        kept,
        renderer,
    };
    let shown = opened(&mut back, first);
    assert!(shown.contains(&inside(1)), "{shown}");
}

#[test]
fn ctrl_o_reaches_every_result_the_rows_offer() {
    // The key the rows name opens every result they offer, held or let go
    // of: walked to its end, the view has shown each one's own words, and
    // none of them is a line saying it could not be read back.
    let sample = Sample::new("opened-ctrl-o");
    let commands = ran_commands(40, LINES);
    let session = logged(&sample, &commands);
    let mut back = put_back(commands, &session);
    assert!(
        back.kept.newest().count() < 40,
        "the store held every result"
    );

    let mut standing = Standing::default();
    standing.open(&back.kept);
    let mut seen = std::collections::BTreeSet::new();
    loop {
        assert!(
            expanding::under(
                &mut back.renderer,
                Style::plain(),
                &back.kept,
                &mut standing
            )
            .expect("a recording cannot fail"),
            "the view closed"
        );
        let shown = back.renderer.terminal().picture().rows().join("\n");
        assert!(!shown.contains(UNREAD), "{shown}");
        seen.extend((1..=40).filter(|number| shown.contains(&inside(*number))));
        if !standing.against(Pressed::Scrolled { back: false }, 300) {
            break;
        }
    }

    let missing: Vec<usize> = (1..=40).filter(|number| !seen.contains(number)).collect();
    assert!(missing.is_empty(), "never shown: {missing:?}");
}
