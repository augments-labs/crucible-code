//! What is kept of a command's output, and what is said about the rest.

use super::{Finished, Kept, OUTPUT, cut};

#[test]
fn a_command_stopped_for_running_too_long_says_so_once() {
    // Both facts hold here: it was killed for taking too long, and something it
    // left running holds the pipe open still. Reported as two notes they read
    // as two problems, the second of which names a cause that is not why this
    // stopped.
    //
    // Assembled rather than run, because the process that can do this is one
    // that left the group the command was killed with — a daemon that called
    // `setsid` for itself — and no command line makes one of those on every
    // platform this ships to.
    let report = Finished {
        code: None,
        out: "half a build".to_owned(),
        arriving: true,
        expired: true,
    }
    .report();

    assert!(report.is_failed());
    assert_eq!(
        report.text().matches("\n\n[").count(),
        1,
        "one marker, not two: {}",
        report.text()
    );
    assert!(report.text().contains("ran too long"), "{}", report.text());
    assert!(
        report.text().contains("still holds the output open"),
        "{}",
        report.text()
    );
}

#[test]
fn more_output_than_anything_can_use_keeps_both_ends() {
    let text = format!("{}{}", "start", "x".repeat(OUTPUT * 2));
    let short = cut(&text, 0);

    assert!(short.starts_with("start"), "the beginning went");
    assert!(short.ends_with('x'));
    assert!(short.len() < text.len());
    assert!(short.contains("cut from the middle"), "{short}");
}

#[test]
fn a_cut_never_lands_inside_a_character() {
    // Multi-byte characters at an offset that puts the halfway mark inside one:
    // slicing there would yield nothing at all rather than a shorter head.
    let text = format!("a{}", "€".repeat(OUTPUT));
    let short = cut(&text, 0);

    assert!(short.starts_with('a'), "the head was dropped whole");
    assert!(short.ends_with('€'));
    assert!(short.len() < text.len());
}

#[test]
fn output_that_fits_comes_back_untouched() {
    assert_eq!(cut("small enough\n", 0), "small enough");
}

#[test]
fn what_the_reader_let_go_is_counted_in_the_same_gap() {
    // There is one gap in the middle, so there is one number for it. Reporting
    // only what this end could see would name a figure smaller than the truth,
    // about a command whose whole problem was how much it printed.
    let text = "x".repeat(OUTPUT * 2);

    let short = cut(&text, 1_000_000);

    assert!(
        short.contains(&format!(
            "[{} bytes of output cut from the middle]",
            1_030_000
        )),
        "{short}"
    );
}

#[test]
fn a_stream_that_never_stops_is_bounded_where_it_is_read() {
    // The cut runs once, at the end. The reader runs for as long as the command
    // does, so `yes` or `cat /dev/urandom` filled memory for two minutes with
    // bytes that were always going to be thrown away — and the 30 KB the model
    // finally saw said nothing about the gigabyte it took to choose them.
    let mut kept = Kept::default();
    let batch = vec![b'x'; 8192];
    for _ in 0..1_000 {
        kept.push(&batch);
    }

    assert!(kept.bytes().len() <= OUTPUT * 2, "{}", kept.bytes().len());
    assert_eq!(kept.dropped + kept.bytes().len(), 8192 * 1_000);
}

#[test]
fn the_ends_a_reader_keeps_are_the_first_bytes_and_the_last_ones() {
    // Bounding it at the source is only correct if what it keeps is what the
    // cut would have chosen anyway.
    let mut kept = Kept::default();
    kept.push(b"start");
    kept.push(&vec![b'x'; OUTPUT * 4]);
    kept.push(b"end");

    let bytes = kept.bytes();

    assert!(bytes.starts_with(b"start"), "the beginning went");
    assert!(bytes.ends_with(b"end"), "the end went");
}

#[test]
fn a_batch_larger_than_the_ring_is_cut_down_in_one_step() {
    // A command emitting megabytes a second is exactly the one that must not be
    // charged per byte here, so a batch is reduced to its own tail before it
    // ever reaches the ring. What that may not change is the count.
    let mut kept = Kept::default();

    kept.push(&vec![b'a'; OUTPUT]);
    kept.push(&vec![b'b'; OUTPUT * 10]);

    assert_eq!(kept.dropped, OUTPUT * 9);
    assert_eq!(kept.bytes().len(), OUTPUT * 2);
}
