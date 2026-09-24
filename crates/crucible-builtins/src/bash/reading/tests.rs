//! What the model can learn about a command nobody is waiting for.

use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::bash::Bash;
use crate::sample::{Sample, allowed};

fn started(sample: &Sample, left: &Background, command: &str) -> Bash {
    let tool = Bash::new(
        sample.workspace(),
        std::sync::Arc::new(crate::sample::sandbox()),
    )
    .sandboxing(false)
    .leaving(left.clone());
    let context = crate::sample::context();
    let args = format!(r#"{{"command":{command},"background":true}}"#);
    let output = crate::bash::tests::awaited(tool.run(allowed(&tool, &args), &context))
        .expect("the command started");
    crate::sample::finalize_call_result(&context, &output);
    assert!(!output.is_failed(), "{}", output.text());
    tool
}

#[test]
fn a_command_still_running_says_what_it_has_printed() {
    // The reason a model runs a second command to find out how the first one
    // is going: until now this answer existed and only the reader could see it.
    let sample = Sample::new("bash-output-running");
    let left = crate::sample::background();
    let _tool = started(
        &sample,
        &left,
        r#""printf 'listening on 5173\n'; sleep 30""#,
    );

    let tool = BashOutput::new(left.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let output = crucible_runtime::answered!(
            tool.run(allowed(&tool, r#"{"number":1}"#), &crate::sample::context())
        )
        .expect("the registry answered");
        assert!(!output.is_failed(), "{}", output.text());
        if output.text().contains("listening on 5173") || Instant::now() >= deadline {
            break output.text().to_owned();
        }
        thread::sleep(Duration::from_millis(20));
    };

    assert!(
        text.contains("listening on 5173"),
        "the model could not read what a running command printed: {text}"
    );
    let _ = left.stop(1);
}

#[test]
fn a_command_still_running_says_where_bytes_were_omitted() {
    // `wrote` glued `Left::text` with no dropped-byte count, and this tool cut
    // the glued string with `excerpt`, which passes `already = 0`. A running
    // command that printed more than a stream's head and tail before it was
    // read here therefore reached the model as a silent splice, or with a cut
    // note counting only what this reader itself kept.
    const FLOOD: usize = crate::bound::OUTPUT * 3;
    let sample = Sample::new("bash-output-flood");
    let left = crate::sample::background();
    let _tool = started(
        &sample,
        &left,
        &format!(r#""yes 0123456789abcdef | head -c {FLOOD}; sleep 30""#),
    );

    let marker = format!(
        "[process output was {FLOOD} bytes; {} bytes omitted from the middle during capture]",
        FLOOD - super::super::output::CAPTURE_TEXT
    );

    // Polled rather than read once: the pipeline still has to finish writing
    // `FLOOD` bytes after `started` returns, so an early read can catch it
    // part-way through and see a smaller, still-correct count for what has
    // arrived so far. The wait is for the exact marker a finished flood
    // produces, not merely for some cut having happened.
    let tool = BashOutput::new(left.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let output = crucible_runtime::answered!(
            tool.run(allowed(&tool, r#"{"number":1}"#), &crate::sample::context())
        )
        .expect("the registry answered");
        assert!(!output.is_failed(), "{}", output.text());
        let text = output.text().to_owned();
        if text.contains(&marker) || Instant::now() >= deadline {
            break text;
        }
        thread::sleep(Duration::from_millis(20));
    };

    assert!(
        text.contains(&marker),
        "the model was told less than what the reader actually dropped, or nothing at all: {text}"
    );
    let _ = left.stop(1);
}

#[test]
fn a_command_still_running_survives_the_runners_own_result_ceiling() {
    // `bash_output`'s answer already carries a marker once the reader's own
    // cut applies; the runner then applies its own encoded-size ceiling,
    // `limit_encoded`, to every result before it reaches the model — and a
    // flood this dense with newlines (`yes` ends every line it prints) encodes
    // past that ceiling on top of the reader's own cut. Without
    // `with_capture_elision` carrying the process counts along, that second
    // cut has no process byte count of its own: it removes the reader's
    // exact marker and names only encoded result bytes in its place.
    const FLOOD: usize = crate::bound::OUTPUT * 3;
    let sample = Sample::new("bash-output-flood-ceiling");
    let left = crate::sample::background();
    let _tool = started(
        &sample,
        &left,
        &format!(r#""yes 0123456789abcdef | head -c {FLOOD}; sleep 30""#),
    );

    let marker = format!(
        "[process output was {FLOOD} bytes; {} bytes omitted from the middle during capture]",
        FLOOD - super::super::output::CAPTURE_TEXT
    );
    let process_counts = format!(
        "process output was {FLOOD} bytes; {} bytes omitted during capture",
        FLOOD - super::super::output::CAPTURE_TEXT
    );

    let tool = BashOutput::new(left.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = loop {
        let output = crucible_runtime::answered!(
            tool.run(allowed(&tool, r#"{"number":1}"#), &crate::sample::context())
        )
        .expect("the registry answered");
        assert!(!output.is_failed(), "{}", output.text());
        if output.text().contains(&marker) || Instant::now() >= deadline {
            break output;
        }
        thread::sleep(Duration::from_millis(20));
    };

    // The runner's own ceiling applies to every result on the way to the
    // model; nothing here should need it to fit already.
    let _ = output.limit_encoded(crucible_types::TOOL_RESULT_BYTES);

    assert!(
        output.text().contains(&process_counts),
        "the runner's own result ceiling cut through the reader's marker with no process byte \
         count of its own: {}",
        output.text()
    );
    let _ = left.stop(1);
}

#[test]
fn a_number_nothing_answers_to_says_what_is_running() {
    // The ordinary way to be wrong here is to be one moment late: a command
    // that ended has left the registry, and its output is already on its way in
    // the note about the ending. A refusal that said only "no" would send the
    // model looking for another way to ask.
    let sample = Sample::new("bash-output-gone");
    let left = crate::sample::background();
    let _tool = started(&sample, &left, r#""sleep 30""#);

    let tool = BashOutput::new(left.clone());
    let output = crucible_runtime::answered!(
        tool.run(allowed(&tool, r#"{"number":9}"#), &crate::sample::context())
    )
    .expect("the registry answered");

    assert!(output.is_failed(), "{}", output.text());
    assert!(
        output.text().contains("#1 sleep 30"),
        "the refusal never said what is running: {}",
        output.text()
    );
    assert!(
        output.text().contains("arrives on its own when it ends"),
        "the refusal left the model somewhere to go but here: {}",
        output.text()
    );
    let _ = left.stop(1);
}

#[test]
fn a_command_that_has_printed_nothing_says_so_rather_than_nothing() {
    // An empty answer reads as a broken tool, and the next move after one is
    // to run something else. Saying it is running and silent is the answer.
    let sample = Sample::new("bash-output-silent");
    let left = crate::sample::background();
    let _tool = started(&sample, &left, r#""sleep 30""#);

    let tool = BashOutput::new(left.clone());
    let output = crucible_runtime::answered!(
        tool.run(allowed(&tool, r#"{"number":1}"#), &crate::sample::context())
    )
    .expect("the registry answered");

    assert!(!output.is_failed(), "{}", output.text());
    assert!(
        output.text().contains("printed nothing yet"),
        "a running command answered with silence: {:?}",
        output.text()
    );
    let _ = left.stop(1);
}
