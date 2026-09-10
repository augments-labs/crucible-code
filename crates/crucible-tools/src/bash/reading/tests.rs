//! What the model can learn about a command nobody is waiting for.

use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::bash::Bash;
use crate::sample::{Sample, allowed};

fn started(sample: &Sample, left: &Background, command: &str) -> Bash {
    let tool = Bash::new(sample.workspace())
        .sandboxing(
            std::sync::Arc::new(crucible_sandbox_local::LocalSandbox::new()),
            false,
        )
        .leaving(left.clone());
    let context = crate::sample::context();
    let args = format!(r#"{{"command":{command},"background":true}}"#);
    let output = tool
        .run(allowed(&tool, &args), &context)
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
    let left = Background::new();
    let _tool = started(
        &sample,
        &left,
        r#""printf 'listening on 5173\n'; sleep 30""#,
    );

    let tool = BashOutput::new(left.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let output = tool
            .run(allowed(&tool, r#"{"number":1}"#), &crate::sample::context())
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
fn a_number_nothing_answers_to_says_what_is_running() {
    // The ordinary way to be wrong here is to be one moment late: a command
    // that ended has left the registry, and its output is already on its way in
    // the note about the ending. A refusal that said only "no" would send the
    // model looking for another way to ask.
    let sample = Sample::new("bash-output-gone");
    let left = Background::new();
    let _tool = started(&sample, &left, r#""sleep 30""#);

    let tool = BashOutput::new(left.clone());
    let output = tool
        .run(allowed(&tool, r#"{"number":9}"#), &crate::sample::context())
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
    let left = Background::new();
    let _tool = started(&sample, &left, r#""sleep 30""#);

    let tool = BashOutput::new(left.clone());
    let output = tool
        .run(allowed(&tool, r#"{"number":1}"#), &crate::sample::context())
        .expect("the registry answered");

    assert!(!output.is_failed(), "{}", output.text());
    assert!(
        output.text().contains("printed nothing yet"),
        "a running command answered with silence: {:?}",
        output.text()
    );
    let _ = left.stop(1);
}
