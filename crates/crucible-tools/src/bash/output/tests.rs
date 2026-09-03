//! What is kept of a command's output, and what is said about the rest.

use super::{CAPTURE_HEAD, FRESH, Finished, Kept, OUTPUT, cut};

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
        original: "half a build".len(),
        omitted: 0,
        arriving: true,
        expired: true,
        output_limited: false,
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
fn a_command_stopped_for_output_says_which_ceiling_it_crossed() {
    let report = Finished {
        code: None,
        out: "bounded prefix".to_owned(),
        original: 100,
        omitted: 86,
        arriving: false,
        expired: false,
        output_limited: true,
    }
    .report();

    assert!(report.is_failed());
    assert!(
        report.text().contains("captured-output ceiling"),
        "{}",
        report.text()
    );
    assert!(!report.text().contains("ran too long"), "{}", report.text());
}

#[test]
fn more_output_than_anything_can_use_keeps_both_ends() {
    let text = format!("{}{}", "start", "x".repeat(OUTPUT * 2));
    let short = cut(&text, 0);

    assert!(short.starts_with("start"), "the beginning went");
    assert!(short.ends_with('x'));
    assert!(short.len() < text.len());
    assert!(short.contains("omitted from the middle"), "{short}");
    assert!(
        short.contains(&format!("process output was {} bytes", text.len())),
        "{short}"
    );
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
        short.contains("process output was 1060000 bytes"),
        "{short}"
    );
    assert!(
        short.contains("bytes omitted from the middle during capture"),
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

    assert!(kept.bytes().len() <= OUTPUT, "{}", kept.bytes().len());
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

    assert_eq!(kept.dropped, OUTPUT * 10);
    assert_eq!(kept.bytes().len(), OUTPUT);
}

#[test]
fn byte_counts_saturate_instead_of_wrapping() {
    let mut kept = Kept {
        dropped: usize::MAX - 1,
        ..Kept::default()
    };

    kept.push(&vec![b'x'; OUTPUT * 3]);

    assert_eq!(kept.dropped, usize::MAX);
    let shown = cut(&"x".repeat(OUTPUT * 2), usize::MAX);
    assert!(shown.contains(&usize::MAX.to_string()), "{shown}");
}

#[cfg(unix)]
#[test]
fn a_live_child_is_never_reaped_with_an_unbounded_wait() {
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "sleep 5"]);
    let mut process =
        crate::sandbox::process::testing(command, crucible_core::SandboxSpeech::Closed).unwrap();
    let started = std::time::Instant::now();

    let status = super::reap(process.as_mut(), std::time::Duration::from_millis(40)).unwrap();

    assert!(status.is_none());
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    process.stop().unwrap();
}

#[cfg(unix)]
#[test]
fn an_early_return_guard_stops_the_whole_process_group() {
    let base = std::env::temp_dir().join(format!("crucible-bash-guard-{}", std::process::id()));
    let _ = std::fs::remove_file(&base);
    let mut command = std::process::Command::new("sh");
    command
        .args(["-c", "(sleep 0.3; printf x > \"$MARKER\") & wait"])
        .env("MARKER", &base);
    let process =
        crate::sandbox::process::testing(command, crucible_core::SandboxSpeech::Closed).unwrap();

    drop(super::Waited::new(process));
    std::thread::sleep(std::time::Duration::from_millis(450));

    assert!(!base.exists(), "a descendant survived early-return cleanup");
}

#[test]
fn a_character_split_across_two_reads_is_handed_over_whole() {
    // A pipe is read in fixed blocks, so this is not a rare case: any
    // command printing anything but ASCII meets it. Handed over as it
    // arrived, the reader would see a replacement mark and then another,
    // for a character nothing damaged.
    let mut kept = Kept::default();
    let euro = "€".as_bytes();
    let (front, rest) = euro.split_at(1);

    kept.push(b"cost: ");
    kept.push(front);
    assert_eq!(kept.hand_over(), "cost: ");

    kept.push(rest);
    assert_eq!(kept.hand_over(), "€");
}

#[test]
fn bytes_that_are_not_utf8_at_all_are_handed_over_rather_than_held_forever() {
    // The difference that matters: an incomplete sequence may still be
    // arriving, and a wrong one never will. Holding the second back would
    // stop the window for as long as the command ran.
    let mut kept = Kept::default();
    kept.push(&[0xff, b'o', b'k']);

    let said = kept.hand_over();
    assert!(said.ends_with("ok"), "{said}");
    assert!(kept.fresh.is_empty(), "a bad byte stalled the handover");
}

#[test]
fn what_one_handover_carries_is_bounded_however_much_arrived() {
    // The window is a window. A command emitting megabytes between two ticks
    // must not make this grow, and what it loses is lost to the reader
    // alone — the answer below still holds its head and its tail.
    let mut kept = Kept::default();
    kept.push(&b"x".repeat(FRESH * 4));

    assert_eq!(kept.hand_over().len(), FRESH);
    assert!(kept.fresh.is_empty());
    assert_eq!(kept.head.len(), CAPTURE_HEAD);
}

#[test]
fn nothing_arriving_hands_nothing_over() {
    let mut kept = Kept::default();
    assert!(kept.hand_over().is_empty());

    kept.push(b"one");
    assert_eq!(kept.hand_over(), "one");
    assert!(
        kept.hand_over().is_empty(),
        "the same bytes were handed twice"
    );
}
