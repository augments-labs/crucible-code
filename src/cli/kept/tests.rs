use super::*;

/// A result of `bytes` bytes, kept under `called`.
///
/// The record row it went onto is the count of what has been cut so far. Not
/// the number the loop passes, but with the property these tests need of it:
/// one per result, and no two the same.
fn kept(cut: &mut Kept, called: &str, bytes: usize) {
    let at = cut.cut();

    let call = crucible_core::ToolId::new(format!("call-{at}"));
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
    cut.finished(&crucible_core::ToolId::new("orphan"), "orphaned".into(), 0);

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
    cut.finished(&crucible_core::ToolId::new("second"), "second".into(), 1);

    let lines: Vec<_> = cut.newest().map(Whole::called).collect();
    assert_eq!(lines, ["", "Bash(ls)"]);
}

#[test]
fn interleaved_results_keep_their_own_call_lines_and_live_output() {
    // One provider response can announce every call before the runner starts
    // the first. Results and output identify themselves; their order must not
    // turn the last announced tool into the heading for all of them.
    let mut cut = Kept::default();
    let read = crucible_core::ToolId::new("read");
    let fetch = crucible_core::ToolId::new("fetch");
    let grep = crucible_core::ToolId::new("grep");

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
    let call = crucible_core::ToolId::new("cargo-test");
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
    let call = crucible_core::ToolId::new("half");
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

    let call = crucible_core::ToolId::new("release");
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
    // a click lands on rows the terminal owns, and this line is still live.
    assert!(writing.at().is_none());
    assert!(!cut.is_empty());
}

#[test]
fn the_result_replaces_what_was_held_while_the_call_ran() {
    // Otherwise the same call is standing twice in the one view — once as the
    // tail somebody watched and once as the answer, which reads as two calls.
    let mut cut = Kept::default();

    let call = crucible_core::ToolId::new("build");
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
    let call = crucible_core::ToolId::new("yes");
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
