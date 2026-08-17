use super::*;

/// A result of `bytes` bytes, kept under `called`.
///
/// The record row it went onto is the count of what has been cut so far. Not
/// the number the loop passes, but with the property these tests need of it:
/// one per result, and no two the same.
fn kept(cut: &mut Kept, called: &str, bytes: usize) {
    let at = cut.cut();

    cut.calling(called.to_owned());
    cut.finished("x".repeat(bytes).into_boxed_str(), at);
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
    cut.finished("orphaned".into(), 0);

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
    cut.finished("second".into(), 1);

    let lines: Vec<_> = cut.newest().map(Whole::called).collect();
    assert_eq!(lines, ["", "Bash(ls)"]);
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
    cut.calling("Bash(cargo test)".to_owned());
    cut.finished("what it said".into(), 41);

    assert!(cut.offered(41));
    assert_eq!(
        cut.newest().next().map(Whole::at),
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
    // The offer is still on screen — the row said so and scrollback keeps it —
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
    cut.calling("Read(half)".to_owned());
    assert!(cut.is_empty());

    cut.finished("here".into(), 0);
    assert!(!cut.is_empty());
}
