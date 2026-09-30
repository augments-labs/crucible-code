use super::*;

/// A result of `bytes` bytes, kept under `called`.
///
/// The record row it went onto is the count of what has been cut so far. Not
/// the number the loop passes, but with the property these tests need of it:
/// one per result, and no two the same.
fn kept(cut: &mut Kept, called: &str, bytes: usize) {
    let at = cut.cut();

    let call = crucible_types::ToolId::new(format!("call-{at}"));
    cut.calling(call.clone(), called.to_owned());
    cut.finished(&call, "x".repeat(bytes).into_boxed_str(), at);
}

#[test]
fn a_result_is_held_under_the_line_the_call_was_committed_under() {
    // The two arrive one event apart and the reader knows the call by its line,
    // so pairing them is the whole job. A result held under the wrong line
    // would be the right text about the wrong call, which reads as neither.
    let mut cut = Kept::default();
    kept(&mut cut, "Bash(cargo test)", 4);

    let held: Vec<_> = cut.newest().collect();
    assert_eq!(held.len(), 1);
    let whole = held.first().expect("the one result held");
    assert_eq!(whole.called(), "Bash(cargo test)");
    assert_eq!(whole.text(), "xxxx");
}

#[test]
fn results_come_back_newest_first() {
    // The order somebody is looking for them in. What a reader wants to see is
    // almost always what just went past, so the list opens on it.
    let mut cut = Kept::default();
    kept(&mut cut, "Read(one)", 1);
    kept(&mut cut, "Read(two)", 1);
    kept(&mut cut, "Read(three)", 1);

    let lines: Vec<_> = cut.newest().map(Whole::called).collect();
    assert_eq!(lines, ["Read(three)", "Read(two)", "Read(one)"]);
}

#[test]
fn a_result_that_arrived_without_a_call_line_is_still_held() {
    // Nothing in the loop guarantees the pair — a call line is committed by one
    // branch and a result drawn by another, and a turn that ended between them
    // is a result with nothing in front of it. Holding it without a name beats
    // dropping the text somebody asked for.
    let mut cut = Kept::default();
    cut.finished(&crucible_types::ToolId::new("orphan"), "orphaned".into(), 0);

    let held: Vec<_> = cut.newest().collect();
    assert_eq!(held.len(), 1);
    let whole = held.first().expect("the one result held");
    assert_eq!(whole.called(), "");
    assert_eq!(whole.text(), "orphaned");
}

#[test]
fn a_call_line_is_spent_by_the_result_that_follows_it() {
    // Otherwise the next result with no line of its own would take the last
    // one's, and a reader would be shown one call's text under another call's
    // name with nothing on screen to say so.
    let mut cut = Kept::default();
    kept(&mut cut, "Bash(ls)", 1);
    cut.finished(&crucible_types::ToolId::new("second"), "second".into(), 1);

    let lines: Vec<_> = cut.newest().map(Whole::called).collect();
    assert_eq!(lines, ["", "Bash(ls)"]);
}

#[test]
fn interleaved_results_keep_their_own_call_lines_and_live_output() {
    // One provider response can announce every call before the runner starts
    // the first. Results and output identify themselves; their order must not
    // turn the last announced tool into the heading for all of them.
    let mut cut = Kept::default();
    let read = crucible_types::ToolId::new("read");
    let fetch = crucible_types::ToolId::new("fetch");
    let grep = crucible_types::ToolId::new("grep");

    cut.calling(read.clone(), "Read(src/main.rs)".to_owned());
    cut.calling(fetch.clone(), "WebFetch(https://example.com)".to_owned());
    cut.calling(grep.clone(), "Grep(needle)".to_owned());

    // Live text may arrive while several headings are pending too.
    cut.wrote(&read, "ordinary output\n");
    cut.wrote(&fetch, "HTTP 500\n");

    // Finish out of request order to prove identity, not adjacency, pairs them.
    cut.finished(&grep, "nothing matched needle".into(), 1);
    cut.finished(&fetch, "web source moonshot: HTTP 500".into(), 2);
    cut.finished(&read, "ordinary output".into(), 3);

    let held: Vec<_> = cut
        .newest()
        .map(|whole| (whole.called(), whole.text()))
        .collect();
    assert_eq!(
        held,
        [
            ("Read(src/main.rs)", "ordinary output"),
            (
                "WebFetch(https://example.com)",
                "web source moonshot: HTTP 500"
            ),
            ("Grep(needle)", "nothing matched needle"),
        ]
    );
    assert!(cut.writing().next().is_none());
}

#[test]
fn live_outputs_keep_request_order_rather_than_tool_id_order() {
    let mut cut = Kept::default();
    for (id, called) in [
        ("z", "Read(first)"),
        ("a", "Read(second)"),
        ("m", "Read(third)"),
    ] {
        let call = ToolId::new(id);
        cut.calling(call.clone(), called.to_owned());
        cut.wrote(&call, &format!("{called} output\n"));
    }

    let called: Vec<_> = cut.writing().map(Whole::called).collect();
    assert_eq!(called, ["Read(first)", "Read(second)", "Read(third)"]);
}

#[test]
fn unknown_live_output_creates_no_state_and_steals_no_heading() {
    let mut cut = Kept::default();
    let known = ToolId::new("known");
    cut.calling(known.clone(), "Read(known)".to_owned());

    cut.wrote(&ToolId::new("unknown"), "orphaned output\n");

    assert!(cut.writing().next().is_none());
    cut.finished(&known, "known result".into(), 0);
    assert_eq!(cut.newest().next().map(Whole::called), Some("Read(known)"));
}

#[test]
fn what_is_held_stays_under_the_ceiling_however_long_the_session_runs() {
    // The rule the whole renderer is built to keep: nothing may be proportional
    // to how long the session has run. Sixteen results of a third of the
    // ceiling each is five times the ceiling arriving, and what is held after
    // it is what was held after the first few.
    let mut cut = Kept::default();
    for turn in 0..16 {
        kept(&mut cut, &format!("Bash({turn})"), HELD / 3);
    }

    let held: usize = cut.newest().map(|whole| whole.text().len()).sum();
    assert!(held <= HELD, "{held} bytes held");

    // And what survived is the end of the session rather than the start of it.
    let newest = cut.newest().next().expect("something is held");
    assert_eq!(newest.called(), "Bash(15)");
}

#[test]
fn large_headings_share_the_retained_byte_ceiling_with_output() {
    let mut cut = Kept::default();
    for turn in 0..16 {
        kept(
            &mut cut,
            &format!("Bash({turn} {})", "x".repeat(HELD / 3)),
            0,
        );
    }

    let held: usize = cut
        .newest()
        .map(|whole| whole.called().len() + whole.text().len())
        .sum();
    assert!(held <= HELD, "{held} bytes held");
    assert_eq!(
        cut.newest().count(),
        2,
        "eviction must subtract heading bytes too"
    );
    assert!(
        cut.newest()
            .next()
            .unwrap()
            .called()
            .starts_with("Bash(15 ")
    );
}

#[test]
fn a_result_bigger_than_the_ceiling_on_its_own_is_still_the_one_held() {
    // It is over the bound the moment it arrives, so a queue that emptied until
    // it fitted would empty completely — and the result nobody could ever see
    // would be the largest one, which is the one somebody is most likely to be
    // asking about.
    let mut cut = Kept::default();
    kept(&mut cut, "Bash(cat big)", HELD * 2);

    assert_eq!(cut.newest().count(), 1);
    assert_eq!(
        cut.newest().next().map(Whole::called),
        Some("Bash(cat big)")
    );
}

#[test]
fn the_count_of_what_was_cut_goes_on_counting_past_the_ceiling() {
    // A view standing over what was cut works out what arrived under it from
    // the difference between this and what it read when it opened. The queue's
    // own length answers that only until the ceiling starts dropping from the
    // other end, at which point it stops going up and the view starts stepping
    // over rows nobody added.
    let mut cut = Kept::default();
    assert_eq!(cut.cut(), 0);

    for turn in 0..16 {
        kept(&mut cut, &format!("Bash({turn})"), HELD / 3);
    }

    assert_eq!(cut.cut(), 16);
    assert!(cut.newest().count() < 16, "nothing was dropped");
}

#[test]
fn a_result_is_found_by_the_record_row_that_offered_it() {
    // What a click is answered from. The pointer lands on a row of the screen,
    // the renderer turns that into a row of the record, and this is the other
    // end of it: the row the offer was written on, and the result behind it.
    let mut cut = Kept::default();
    let call = crucible_types::ToolId::new("cargo-test");
    cut.calling(call.clone(), "Bash(cargo test)".to_owned());
    cut.finished(&call, "what it said".into(), 41);

    assert!(cut.offered(41));
    assert_eq!(
        cut.newest().next().and_then(Whole::at),
        Some(41),
        "the row the offer went onto is not the row it is held under"
    );

    // Every other row of the record offered nothing, which is most of them.
    assert!(!cut.offered(40));
    assert!(!cut.offered(42));
    assert!(!cut.offered(0));
}

#[test]
fn a_row_whose_result_was_dropped_under_the_ceiling_offers_nothing() {
    // The offer is still on screen — the row said so and the transcript keeps
    // it —
    // and the text behind it has gone. Answering the click with the newest
    // result instead would be showing somebody a different call's output under
    // the row they pointed at, which reads as this call having said it.
    let mut cut = Kept::default();
    for turn in 0..16 {
        kept(&mut cut, &format!("Bash({turn})"), HELD / 3);
    }

    assert!(!cut.offered(0), "a dropped result is still being offered");
    assert!(cut.offered(15), "the newest result cannot be found");
}

#[test]
fn nothing_cut_is_nothing_to_offer() {
    // The key that asks for this reads it before it draws anything, because a
    // session where no result was ever cut has no offer on screen to have
    // prompted the press.
    let mut cut = Kept::default();
    assert!(cut.is_empty());

    // A call whose result has not arrived is not one either. What is held is
    // text, and half a pair is none of it.
    let call = crucible_types::ToolId::new("half");
    cut.calling(call.clone(), "Read(half)".to_owned());
    assert!(cut.is_empty());

    cut.finished(&call, "here".into(), 0);
    assert!(!cut.is_empty());
}

#[test]
fn a_call_that_has_not_answered_is_reachable_under_the_line_it_is_running_on() {
    // The whole point of holding it: a build that will take two minutes is
    // something a reader wants to open now rather than in two minutes.
    let mut cut = Kept::default();
    assert!(cut.is_empty());

    let call = crucible_types::ToolId::new("release");
    cut.calling(call.clone(), "Bash(cargo build --release)".to_owned());
    cut.wrote(&call, "   Compiling crucible-core v0.5.0\n");
    cut.wrote(&call, "   Compiling crucible-tui v0.5.0\n");

    let writing = cut.writing().next().expect("the running call was not held");
    assert_eq!(writing.called(), "Bash(cargo build --release)");
    assert_eq!(
        writing.text(),
        "   Compiling crucible-core v0.5.0\n   Compiling crucible-tui v0.5.0\n"
    );

    // No row of the record offered it, because no row for it has been committed:
    // a click lands on rows the record holds, and this line is still live.
    assert!(writing.at().is_none());
    assert!(!cut.is_empty());
}

#[test]
fn the_result_replaces_what_was_held_while_the_call_ran() {
    // Otherwise the same call is standing twice in the one view — once as the
    // tail somebody watched and once as the answer, which reads as two calls.
    let mut cut = Kept::default();

    let call = crucible_types::ToolId::new("build");
    cut.calling(call.clone(), "Bash(cargo build)".to_owned());
    cut.wrote(&call, "Compiling\n");
    cut.finished(&call, "Compiling\nFinished in 1m 52s".into(), 4);

    assert!(cut.writing().next().is_none());
    assert_eq!(cut.newest().count(), 1);
}

#[test]
fn what_is_held_of_a_running_call_is_its_end_and_is_bounded() {
    // A command printing without stopping has no result yet to bound it against,
    // so this is the bound — and it keeps the end, because where a build has got
    // to is the question and its first lines are the part already watched.
    let mut cut = Kept::default();
    let call = crucible_types::ToolId::new("yes");
    cut.calling(call.clone(), "Bash(yes)".to_owned());

    for line in 0..40_000 {
        cut.wrote(&call, &format!("line {line}\n"));
    }

    let writing = cut.writing().next().expect("the running call was not held");
    assert!(writing.text().len() <= WRITING, "{}", writing.text().len());
    assert!(
        writing.text().ends_with("line 39999\n"),
        "the end of the output was the part dropped"
    );
    // And it opens on a whole line rather than the tail of one.
    assert!(
        writing.text().starts_with("line "),
        "{:?}",
        writing.text().get(..12)
    );
}

/// A log that holds what each result said at the place it was written, and
/// says where results went as the session's writer would.
#[derive(Debug, Default)]
struct Logged {
    /// What is at each place.
    held: std::collections::HashMap<(crucible_types::ToolId, u64), Box<str>>,
    /// Where results went since this was last asked.
    went: std::rc::Rc<std::cell::RefCell<Vec<(crucible_types::ToolId, u64)>>>,
    /// How many times this was asked to wait for the writer.
    settles: std::rc::Rc<std::cell::Cell<usize>>,
    /// Whether the writer has stopped placing what it writes.
    stopped: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Log for Logged {
    fn landed(&self) -> Vec<(crucible_types::ToolId, u64)> {
        self.went.take()
    }

    fn settled(&self) -> Vec<(crucible_types::ToolId, u64)> {
        self.settles.set(self.settles.get() + 1);
        self.went.take()
    }

    fn places(&self) -> bool {
        !self.stopped.get()
    }

    fn read(&self, call: &crucible_types::ToolId, position: u64) -> Option<Box<str>> {
        self.held.get(&(call.clone(), position)).cloned()
    }
}

/// Forty results of a quarter of the ceiling each, every one placed in a log
/// at the position its number says, and the first fifteen of them let go of.
///
/// Replayed, each place is said just before its result is drawn; live, the
/// log says where all forty went before the first is drawn, which is the
/// furthest ahead of the screen its writer can be.
fn forty_placed(replayed: bool) -> Kept {
    let mut logged = Logged::default();
    for turn in 0..40_u64 {
        let call = crucible_types::ToolId::new(format!("call-{turn}"));
        logged
            .held
            .insert((call.clone(), turn * 100), format!("result {turn}").into());
        if !replayed {
            logged.went.borrow_mut().push((call, turn * 100));
        }
    }

    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    for turn in 0..40_u64 {
        let call = crucible_types::ToolId::new(format!("call-{turn}"));
        cut.calling(call.clone(), format!("Read({turn})"));
        if replayed {
            cut.placing(&call, turn * 100);
        }
        let at = usize::try_from(turn).unwrap();
        cut.finished(&call, format!("result {turn}").repeat(HELD / 40).into(), at);
    }
    cut
}

/// The words a read back came to, where it came to any.
fn said(back: &Back) -> Option<&str> {
    match back {
        Back::Said(text) => Some(text),
        Back::Unread | Back::Unplaced => None,
    }
}

/// The row let go of that `at` offered.
fn let_go(cut: &Kept, at: usize) -> &Placed {
    cut.older()
        .find(|placed| placed.at() == at)
        .expect("a row let go of")
}

#[test]
fn a_result_let_go_of_is_still_offered_and_read_back_from_the_log() {
    // The ceiling on what is held does not move. What moves is what a row that
    // lost its text can still do: the log holds the result, so the row still
    // opens, and what it opens is read back from there.
    for replayed in [true, false] {
        let mut cut = forty_placed(replayed);

        assert!(
            cut.newest().all(|whole| whole.at() != Some(0)),
            "the oldest result is still held; the test says nothing"
        );
        for at in 0..40 {
            assert!(cut.offered(at), "row {at} stopped offering ({replayed})");
        }

        let oldest = let_go(&cut, 0);
        assert_eq!(oldest.called(), "Read(0)");
        assert_eq!(said(&cut.read_back(oldest)), Some("result 0"), "{replayed}");
        for placed in cut.older() {
            assert!(
                matches!(cut.read_back(placed), Back::Said(_)),
                "row {} ({replayed})",
                placed.at()
            );
        }
        assert!(
            cut.withdrawn().is_empty(),
            "nothing is withdrawn with a log"
        );
    }
}

#[test]
fn a_read_that_fails_says_so_and_the_row_still_offers() {
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(Logged::default())));
    for turn in 0..16 {
        let call = crucible_types::ToolId::new(format!("call-{turn}"));
        cut.calling(call.clone(), format!("Bash({turn})"));
        cut.placing(&call, turn);
        cut.finished(
            &call,
            "x".repeat(HELD / 3).into(),
            usize::try_from(turn).unwrap(),
        );
    }

    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unread);
    assert!(cut.offered(0), "a failed read leaves the row as it was");
}

#[test]
fn with_no_log_a_row_whose_result_was_let_go_of_is_withdrawn() {
    // Nothing can open it, so the row has to stop saying something can. The
    // rows that go are handed to what draws them, once.
    let mut cut = Kept::default();
    for turn in 0..16 {
        kept(&mut cut, &format!("Bash({turn})"), HELD / 3);
    }

    let gone = cut.withdrawn();
    assert!(
        !gone.is_empty(),
        "nothing was let go of; the test says nothing"
    );
    for at in &gone {
        assert!(!cut.offered(*at), "row {at} still offers");
    }
    assert!(cut.withdrawn().is_empty(), "handed over once");
}

#[test]
fn a_row_let_go_of_holds_no_text_and_one_place_for_each_offer() {
    // What is held stays under the ceiling however many rows still offer: a
    // row whose text went keeps where the log holds it and the line of its
    // call, and nothing of the result.
    let cut = forty_placed(true);

    assert!(cut.held <= HELD, "{} held", cut.held);
    assert_eq!(cut.placed.len(), 40 - cut.newest().count());
}

#[test]
fn forgetting_keeps_the_log_and_nothing_it_placed() {
    // `/clear` and `/resume` forget what the rows on screen offered; the log
    // belongs to the session, which is set again when it changes.
    let mut cut = forty_placed(true);
    cut.forget();

    assert!(!cut.offered(0));
    assert!(cut.placed.is_empty());
    let call = crucible_types::ToolId::new("again");
    cut.calling(call.clone(), "Read(again)".to_owned());
    cut.placing(&call, 7);
    cut.finished(&call, "said again".into(), 3);
    assert!(cut.log.is_some());
}

#[test]
fn a_result_arriving_takes_what_the_log_has_placed_and_waits_for_nothing() {
    // The screen asks the log where results went as each one arrives, so what
    // the writer holds for it never builds up; and it never waits for the
    // writer to do it. Only reading a result back waits.
    let logged = Logged::default();
    let went = std::rc::Rc::clone(&logged.went);
    let settles = std::rc::Rc::clone(&logged.settles);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));

    for turn in 0..40_u64 {
        let call = crucible_types::ToolId::new(format!("call-{turn}"));
        cut.calling(call.clone(), format!("Read({turn})"));
        went.borrow_mut().push((call.clone(), turn));
        cut.finished(
            &call,
            "x".repeat(HELD / 20).into(),
            usize::try_from(turn).unwrap(),
        );
        assert!(went.borrow().is_empty(), "turn {turn} left places behind");
    }
    assert_eq!(settles.get(), 0, "a result arriving waited for the writer");

    // Every row let go of was placed without waiting, so reading one back
    // does not wait either.
    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unread);
    assert_eq!(settles.get(), 0);
}

#[test]
fn a_row_let_go_of_before_its_place_came_waits_for_it_when_read() {
    let logged = Logged::default();
    let went = std::rc::Rc::clone(&logged.went);
    let settles = std::rc::Rc::clone(&logged.settles);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(Logged {
        held: [((crucible_types::ToolId::new("call-0"), 9), "said".into())].into(),
        ..logged
    })));
    for turn in 0..4 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }
    let placed = let_go(&cut, 0);

    went.borrow_mut()
        .push((crucible_types::ToolId::new("call-0"), 9));
    assert_eq!(said(&cut.read_back(placed)), Some("said"));
    assert_eq!(settles.get(), 1);
}

#[test]
fn a_place_for_a_result_that_had_no_row_is_not_taken_by_a_later_call_of_the_same_name() {
    // A call's name is not promised to be new every turn. A result that fitted
    // on its row keeps nothing, and its place still comes; that place is its
    // own, so a later result of the same name waits for the one that is.
    let call = crucible_types::ToolId::new("functions.read:0");
    let logged = Logged {
        held: [
            ((call.clone(), 100), "the first said this".into()),
            ((call.clone(), 200), "the second said this".into()),
        ]
        .into(),
        ..Logged::default()
    };
    let went = std::rc::Rc::clone(&logged.went);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));

    cut.calling(call.clone(), "Read(first)".to_owned());
    cut.answered(&call);
    went.borrow_mut().push((call.clone(), 100));

    cut.calling(call.clone(), "Read(second)".to_owned());
    cut.finished(&call, "x".repeat(HELD / 2).into(), 10);
    went.borrow_mut().push((call.clone(), 200));
    for turn in 0..4 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 2);
    }

    assert_eq!(
        said(&cut.read_back(let_go(&cut, 10))),
        Some("the second said this")
    );
}

#[test]
fn places_nothing_claims_are_bounded() {
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(Logged::default())));
    for place in 0..(UNCLAIMED as u64 * 3) {
        cut.placing(
            &crucible_types::ToolId::new(format!("never-{place}")),
            place,
        );
    }
    for turn in 0..(UNCLAIMED * 3) {
        let call = crucible_types::ToolId::new(format!("rowless-{turn}"));
        cut.calling(call.clone(), String::new());
        cut.answered(&call);
    }

    assert_eq!(cut.early.len(), UNCLAIMED);
    assert_eq!(cut.rowless.len(), UNCLAIMED);
    assert!(cut.pending.is_empty(), "a place made a call nobody made");
}

#[test]
fn another_log_takes_nothing_the_last_one_placed() {
    // A place is a position in one file. Set to another session's log, or to
    // none, the rows let go of under the last one stop offering, and nothing
    // learned from it is kept.
    for next in [Some(Box::new(Logged::default()) as Box<dyn Log>), None] {
        let mut cut = forty_placed(false);
        let let_go: Vec<usize> = cut.older().map(Placed::at).collect();
        assert!(!let_go.is_empty(), "nothing was let go of");
        let with = next.is_some();

        cut.logging(next);

        let mut gone = cut.withdrawn();
        gone.sort_unstable();
        let mut expected = let_go.clone();
        expected.sort_unstable();
        assert_eq!(gone, expected, "{with}");
        assert!(let_go.iter().all(|at| !cut.offered(*at)), "{with}");
        assert!(cut.early.is_empty() && cut.rowless.is_empty(), "{with}");
        assert!(
            cut.whole.iter().all(|whole| whole.position.is_none()),
            "{with}"
        );
        assert!(cut.held <= HELD);
    }
}

#[test]
fn one_result_past_the_ceiling_on_its_own_lets_every_row_before_it_go() {
    // The newest result is held whatever it costs, and the rows let go of
    // before it count against the same ceiling, so they go too, oldest first,
    // and stop offering.
    let mut cut = forty_placed(true);
    let before: Vec<usize> = (0..40).collect();
    let call = crucible_types::ToolId::new("huge");
    cut.calling(call.clone(), "Bash(cat huge)".to_owned());
    cut.finished(&call, "x".repeat(HELD + 1).into(), 40);

    assert!(cut.placed.is_empty());
    assert_eq!(cut.newest().count(), 1);
    let mut gone = cut.withdrawn();
    gone.sort_unstable();
    assert_eq!(gone, before);
    assert!(cut.offered(40));
}

#[test]
fn a_result_drawn_with_no_call_line_takes_its_own_place_and_not_the_next_ones() {
    // A replayed result whose call was never drawn still had its place said
    // before it; that place is its own, and a later result of the same name
    // opens on the place said for it.
    let call = crucible_types::ToolId::new("functions.read:0");
    let logged = Logged {
        held: [
            ((call.clone(), 100), "the first said this".into()),
            ((call.clone(), 200), "the second said this".into()),
        ]
        .into(),
        ..Logged::default()
    };
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));

    cut.placing(&call, 100);
    cut.answered(&call);
    cut.calling(call.clone(), "Read(second)".to_owned());
    cut.placing(&call, 200);
    cut.finished(&call, "x".repeat(HELD / 2).into(), 10);
    for turn in 0..4 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 2);
    }

    assert_eq!(
        said(&cut.read_back(let_go(&cut, 10))),
        Some("the second said this")
    );
}

#[test]
fn a_row_let_go_of_before_its_batch_was_written_is_not_placed_yet_rather_than_unreadable() {
    // A turn draws its results before it writes their batch, so a row can be
    // let go of while the log has nothing to say about it. That is not a log
    // that lost it, and a later read finds it.
    let logged = Logged {
        held: [((crucible_types::ToolId::new("call-0"), 9), "said".into())].into(),
        ..Logged::default()
    };
    let went = std::rc::Rc::clone(&logged.went);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    for turn in 0..4 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }

    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unplaced);
    went.borrow_mut()
        .push((crucible_types::ToolId::new("call-0"), 9));
    assert_eq!(said(&cut.read_back(let_go(&cut, 0))), Some("said"));
}

#[test]
fn letting_go_of_results_smaller_than_their_place_keeps_their_rows_offering() {
    // A row let go of keeps its call's line and its place, which can weigh
    // more than a short result did. Charged at that, letting one go would
    // raise what is held, and one arrival would push out every row there was.
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(Logged::default())));
    for _ in 0..15_000 {
        kept(&mut cut, "R", 40);
    }

    assert!(cut.held <= HELD, "{} held", cut.held);
    let offering = (0..15_000).filter(|at| cut.offered(*at)).count();
    assert!(offering > 10_000, "{offering} rows still offer");
}

/// A log holding `result n` for `call-n` at `n * 100`, the store reading from it.
fn logged_store(count: u64) -> Kept {
    let mut logged = Logged::default();
    for n in 0..count {
        logged.held.insert(
            (crucible_types::ToolId::new(format!("call-{n}")), n * 100),
            format!("result {n}").into(),
        );
    }
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    cut
}

#[test]
fn a_short_result_among_long_ones_leaves_every_row_offering_and_opening() {
    // Letting go of a result shorter than its place costs nothing held, and
    // takes nothing from the rows let go of before it.
    let mut cut = logged_store(61);
    for n in 0..61_u64 {
        let call = crucible_types::ToolId::new(format!("call-{n}"));
        cut.calling(call.clone(), format!("Read({n})"));
        cut.placing(&call, n * 100);
        let bytes = if n == 10 { 40 } else { 25_600 };
        cut.finished(&call, "y".repeat(bytes).into(), usize::try_from(n).unwrap());
    }

    assert!(cut.held <= HELD, "{} held", cut.held);
    assert!(cut.withdrawn().is_empty(), "rows stopped offering");
    for at in 0..61 {
        assert!(cut.offered(at), "row {at} stopped offering");
    }
    for placed in cut.older() {
        assert!(
            matches!(cut.read_back(placed), Back::Said(_)),
            "row {} does not open",
            placed.at()
        );
    }
}

#[test]
fn every_call_of_a_folded_run_is_behind_its_row_however_short() {
    let mut cut = logged_store(41);
    for (n, called, bytes) in [(0_u64, "Grep(nothing)", 16), (1, "Read(big)", 25_600)] {
        let call = crucible_types::ToolId::new(format!("call-{n}"));
        cut.calling(call.clone(), called.to_owned());
        cut.placing(&call, n * 100);
        cut.gathered(&call, "z".repeat(bytes).into(), Some(0));
    }
    for n in 2..41_u64 {
        let call = crucible_types::ToolId::new(format!("call-{n}"));
        cut.calling(call.clone(), format!("Read({n})"));
        cut.placing(&call, n * 100);
        cut.finished(
            &call,
            "y".repeat(25_600).into(),
            usize::try_from(n).unwrap(),
        );
    }

    let mut behind: Vec<&str> = cut
        .older()
        .filter(|placed| placed.at() == 0)
        .map(Placed::called)
        .chain(
            cut.newest()
                .filter(|whole| whole.at() == Some(0))
                .map(Whole::called),
        )
        .collect();
    behind.sort_unstable();
    assert!(cut.offered(0));
    assert_eq!(behind, ["Grep(nothing)", "Read(big)"]);
}

#[test]
fn a_row_the_log_will_never_place_is_unreadable_rather_than_waiting() {
    // A writer that has stopped placing, and a row drawn after this one that
    // the log has placed already: either way no place is coming for it.
    let logged = Logged::default();
    let stopped = std::rc::Rc::clone(&logged.stopped);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    for turn in 0..4 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }
    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unplaced);
    stopped.set(true);
    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unread);

    let logged = Logged::default();
    let went = std::rc::Rc::clone(&logged.went);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    for turn in 0..4 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }
    went.borrow_mut()
        .push((crucible_types::ToolId::new("call-3"), 9));
    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unread);
}

#[test]
fn what_the_rows_keep_is_under_the_ceiling_however_short_their_results() {
    // A row let go of keeps its call's line, its call and its place. What all
    // of them keep, and the text still held, stays under the ceiling: a short
    // result is no cheaper to keep as a place than as itself.
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(Logged::default())));
    // Empty results, which stay held, among results long enough to be let
    // go of, which are kept as places.
    for n in 0..10_000_usize {
        let call = crucible_types::ToolId::new(format!("call-with-a-long-id-{n:08}"));
        cut.calling(call.clone(), "Glob(*.rs)".to_owned());
        let text = if n % 2 == 0 {
            String::new()
        } else {
            "x".repeat(200)
        };
        cut.gathered(&call, text.into(), Some(n));
    }
    assert!(
        cut.older().count() > 1000,
        "too few rows were let go of to say anything"
    );

    let kept: usize = cut
        .older()
        .map(|placed| placed.called().len() + placed.call.as_str().len() + size_of::<Placed>())
        .chain(
            cut.newest()
                .map(|whole| whole.text().len() + whole.called().len()),
        )
        .sum();
    assert!(kept <= HELD, "{kept} kept against {HELD}");
}

#[test]
fn a_row_waiting_for_its_batch_is_not_given_up_for_an_older_row_being_placed() {
    let logged = Logged::default();
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    let first = crucible_types::ToolId::new("call-0");
    cut.calling(first.clone(), "Read(0)".to_owned());
    cut.placing(&first, 9);
    cut.finished(&first, "x".repeat(HELD / 3).into(), 0);
    for turn in 1..5 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }

    assert_eq!(cut.read_back(let_go(&cut, 1)), Back::Unplaced);
}

#[test]
fn a_row_with_no_place_behind_a_later_row_that_has_one_is_unreadable() {
    let logged = Logged::default();
    let went = std::rc::Rc::clone(&logged.went);
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(logged)));
    for turn in 0..3 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }
    // The next result's place lands, and the result after it files it.
    went.borrow_mut()
        .push((crucible_types::ToolId::new("call-3"), 9));
    for turn in 3..6 {
        kept(&mut cut, &format!("Read({turn})"), HELD / 3);
    }

    assert_eq!(cut.read_back(let_go(&cut, 0)), Back::Unread);
}

#[test]
fn a_row_stops_offering_only_with_every_result_it_offered() {
    // Where nothing held would make room, the oldest row goes, and a row
    // counting a folded run goes with all of its calls rather than some.
    let mut cut = Kept::default();
    cut.logging(Some(Box::new(Logged::default())));
    for member in ["Grep(a)", "Glob(b)"] {
        let call = crucible_types::ToolId::new(member);
        cut.calling(call.clone(), member.to_owned());
        // Short enough to cost less held than placed, long enough that taking
        // one of them alone would make room for the next arrival.
        cut.gathered(&call, "m".repeat(60).into(), Some(0));
    }
    assert!(
        cut.newest().filter(|whole| whole.at() == Some(0)).count() == 2,
        "the run is not held; the test says nothing"
    );
    let behind = |cut: &Kept| {
        cut.newest().filter(|whole| whole.at() == Some(0)).count()
            + cut.older().filter(|placed| placed.at() == 0).count()
    };
    for arrived in 1..20_000 {
        kept(&mut cut, "R", 40);
        let left = behind(&cut);
        if left < 2 {
            assert_eq!(
                left, 0,
                "after {arrived} arrivals the row lost some of its calls"
            );
            break;
        }
    }

    assert!(cut.withdrawn().contains(&0), "the oldest row still offers");
    assert!(!cut.offered(0));
    assert!(cut.newest().all(|whole| whole.at() != Some(0)));
    assert!(cut.older().all(|placed| placed.at() != 0));
    assert!(cut.held <= HELD);
}
