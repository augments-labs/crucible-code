use super::*;

/// A result of `bytes` bytes, kept under `called`.
fn kept(cut: &mut Kept, called: &str, bytes: usize) {
    cut.calling(called.to_owned());
    cut.finished("x".repeat(bytes).into_boxed_str());
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
    cut.finished("orphaned".into());

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
    cut.finished("second".into());

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

    cut.finished("here".into());
    assert!(!cut.is_empty());
}
