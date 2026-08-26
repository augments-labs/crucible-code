use super::*;

use crate::sample::Sample;

/// What a workspace holds after `said` has been asked in it, oldest first.
fn walked(sample: &Sample, workspace: &crucible_core::Workspace) -> Vec<String> {
    prompts(&sample.logs(), workspace).expect("a readable temporary directory")
}

#[test]
fn a_prompt_is_offered_back_to_the_directory_it_was_asked_in() {
    let sample = Sample::new("prompts-asked");
    let here = sample.workspace();

    remember(&sample.logs(), &here, "rename the tail's bound").expect("a writable directory");

    assert_eq!(walked(&sample, &here), vec!["rename the tail's bound"]);
}

#[test]
fn what_was_asked_elsewhere_is_never_offered_here() {
    // The file is one for the machine, because a session directory is. What a
    // reader reaches back through is this directory's work: a prompt about
    // another checkout arriving under the arrow key is a line they would send
    // to the wrong agent before they had finished reading it.
    let sample = Sample::new("prompts-elsewhere");
    let here = sample.workspace();
    let there = sample.elsewhere();

    remember(&sample.logs(), &here, "the one asked here").expect("a writable directory");
    remember(&sample.logs(), &there, "the one asked there").expect("a writable directory");

    assert_eq!(walked(&sample, &here), vec!["the one asked here"]);
    assert_eq!(walked(&sample, &there), vec!["the one asked there"]);
}

#[test]
fn the_prompts_come_back_oldest_first() {
    // The order the arrow walks. Newest last, so the place a walk starts at is
    // the count itself and the number falls as it goes back.
    let sample = Sample::new("prompts-order");
    let here = sample.workspace();

    for said in ["first", "second", "third"] {
        remember(&sample.logs(), &here, said).expect("a writable directory");
    }

    assert_eq!(walked(&sample, &here), vec!["first", "second", "third"]);
}

#[test]
fn a_prompt_of_many_lines_comes_back_as_the_one_prompt_it_was() {
    // A break is a character in a prompt, so the file cannot be a list of
    // lines: read back naively, one prompt of three lines is three prompts,
    // and the arrow walks two thirds of a paragraph into the box.
    let sample = Sample::new("prompts-broken");
    let here = sample.workspace();
    let said = "why does the probe walk the tree\n\nbefore it reports the first hit";

    remember(&sample.logs(), &here, said).expect("a writable directory");

    assert_eq!(walked(&sample, &here), vec![said]);
}

#[test]
fn only_the_newest_kept_for_a_workspace_are_offered_back() {
    // The bound is the whole reason the file can be read at start-up. What it
    // costs is the oldest prompt, which is the one nobody was going to reach.
    let sample = Sample::new("prompts-window");
    let here = sample.workspace();

    for nth in 0..PROMPTS + 5 {
        remember(&sample.logs(), &here, &format!("prompt {nth}")).expect("a writable directory");
    }

    let walked = walked(&sample, &here);
    assert_eq!(walked.len(), PROMPTS);
    assert_eq!(walked.first().map(String::as_str), Some("prompt 5"));
    assert_eq!(
        walked.last().map(String::as_str),
        Some(format!("prompt {}", PROMPTS + 4).as_str())
    );
}

#[test]
fn a_prompt_too_long_to_retain_is_not_retained_at_all() {
    // Rather than kept in part. Half a prompt put back into the box by an
    // arrow is a line somebody sends without noticing what is missing from
    // it, which is worse than an arrow that finds nothing.
    let sample = Sample::new("prompts-long");
    let here = sample.workspace();
    let long = "x".repeat(LONGEST + 1);

    remember(&sample.logs(), &here, "short enough").expect("a writable directory");
    remember(&sample.logs(), &here, &long).expect("a writable directory");

    assert_eq!(walked(&sample, &here), vec!["short enough"]);
}

#[test]
fn a_prompt_of_exactly_the_retained_length_is_retained() {
    let sample = Sample::new("prompts-longest");
    let here = sample.workspace();
    let long = "x".repeat(LONGEST);

    remember(&sample.logs(), &here, &long).expect("a writable directory");

    assert_eq!(walked(&sample, &here), vec![long]);
}

#[test]
fn a_directory_nothing_was_ever_asked_in_offers_nothing() {
    // Rather than failing. The file is written by the first prompt somebody
    // finishes, so every session before that one reads a name that is not
    // there — which is the ordinary state and not a fault to report.
    let sample = Sample::new("prompts-empty");

    assert!(walked(&sample, &sample.workspace()).is_empty());
}

#[test]
fn a_line_the_file_ends_mid_way_through_costs_that_prompt_and_no_other() {
    // A crash between the write and the rename cannot leave one of these, but
    // a disk that filled can. What is read back is every prompt that is whole.
    let sample = Sample::new("prompts-torn");
    let here = sample.workspace();

    remember(&sample.logs(), &here, "whole").expect("a writable directory");

    let path = sample.logs().join(NAME);
    let mut text = std::fs::read_to_string(&path).expect("the file just written");
    text.push_str("{\"root\":\"");
    std::fs::write(&path, text).expect("a writable temporary directory");

    assert_eq!(walked(&sample, &here), vec!["whole"]);
}

#[test]
fn a_file_written_in_a_format_this_build_does_not_know_is_left_alone() {
    // Read, it would be guessed at; replaced, it would take an older build's
    // history away from the build that is still using it. Neither, so this one
    // reaches back through nothing and says nothing about it.
    let sample = Sample::new("prompts-format");
    let here = sample.workspace();
    let path = sample.logs().join(NAME);
    std::fs::write(&path, "crucible-prompts-99\n").expect("a writable temporary directory");

    assert!(prompts(&sample.logs(), &here).is_err());
}

#[test]
fn the_file_never_grows_past_the_prompts_it_retains() {
    // Two workspaces sharing one file share one bound, so a directory nobody
    // has typed in for a month gives its lines up to the one somebody is
    // working in. What may never happen is the file growing with the session.
    let sample = Sample::new("prompts-bound");
    let here = sample.workspace();
    let there = sample.elsewhere();
    let path = sample.logs().join(NAME);

    // Planted rather than typed, because the point is what the next write
    // does to a full file and not how long a thousand durable writes take.
    let full = (0..RETAINED)
        .map(|nth| Entry {
            root: there.root().display().to_string(),
            said: format!("there {nth}"),
        })
        .collect::<Vec<_>>();
    replace(&path, &full).expect("a writable directory");

    remember(&sample.logs(), &here, "the newest").expect("a writable directory");

    let text = std::fs::read_to_string(&path).expect("the file just written");
    assert_eq!(
        text.lines().count(),
        RETAINED + 1,
        "the header and what it bounds"
    );
    assert_eq!(walked(&sample, &here), vec!["the newest"]);
    assert!(
        !text.contains("there 0\""),
        "the oldest prompt is what the newest one cost"
    );
}

#[test]
fn a_directory_past_its_window_gives_up_its_oldest_prompt_in_the_file_itself() {
    // Not only on the way back out. What the window is for is that a session
    // holds a hundred lines and no more; a file that went on holding the rest
    // would be paying for a history nothing can reach.
    let sample = Sample::new("prompts-forgotten");
    let here = sample.workspace();

    for nth in 0..=PROMPTS {
        remember(&sample.logs(), &here, &format!("prompt {nth}")).expect("a writable directory");
    }

    let text = std::fs::read_to_string(sample.logs().join(NAME)).expect("the file just written");
    assert_eq!(
        text.lines().count(),
        PROMPTS + 1,
        "the header and its window"
    );
    assert!(
        !text.contains("prompt 0\""),
        "the oldest prompt is what the newest one cost"
    );
    assert!(
        text.contains(&format!("prompt {PROMPTS}\"")),
        "the newest prompt is there"
    );
}

#[test]
fn one_directory_filling_its_window_leaves_another_directory_its_own() {
    // The two windows are separate, so the arrows in one checkout are never
    // shortened by how much work was done in the other.
    let sample = Sample::new("prompts-separate");
    let here = sample.workspace();
    let there = sample.elsewhere();

    remember(&sample.logs(), &there, "the one asked there").expect("a writable directory");
    for nth in 0..=PROMPTS {
        remember(&sample.logs(), &here, &format!("prompt {nth}")).expect("a writable directory");
    }

    assert_eq!(walked(&sample, &there), vec!["the one asked there"]);
    assert_eq!(walked(&sample, &here).len(), PROMPTS);
}
