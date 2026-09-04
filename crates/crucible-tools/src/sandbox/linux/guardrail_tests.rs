//! Where a command guardrail stops reading, and what still bounds the command.
//!
//! A guardrail is a policy over the invocation crucible was asked to launch. It
//! reads that invocation's words and nothing further, so a program a script
//! reaches, or one the script makes for itself, is not a word it can refuse.
//! These start such a command against the enforcing backend and ask what
//! happened: the guardrail let it through, and the kernel is what bounded it.

use std::ffi::OsString;

use crucible_core::{
    Ancestry, SandboxCommand, SandboxCommandPolicy, SandboxCommandRule, SandboxEnvironment,
    SandboxError, SandboxGuardrailEffect, SandboxId, SandboxManifest, SandboxPolicy,
    SandboxRequest, SandboxService, ToolId,
};

use super::tests::{command, finish};
use crate::LocalSandbox;
use crate::sample::{Sample, skipped_without_enforcement};

/// A secret one directory over from the workspace, granted to nothing.
fn ungranted_secret(sample: &Sample) -> std::path::PathBuf {
    let outside = sample
        .root()
        .parent()
        .expect("sample parent")
        .join("outside");
    std::fs::create_dir_all(&outside).expect("ungranted directory");
    let secret = outside.join("secret");
    std::fs::write(&secret, b"not for the sandbox").expect("ungranted secret");
    secret
}

/// A confined session whose guardrail refuses every invocation matching `rule`.
fn session_denying(sample: &Sample, rule: &[&str]) -> Box<dyn crucible_core::SandboxSession> {
    let commands = SandboxCommandPolicy::new([SandboxCommandRule::anchored(
        SandboxGuardrailEffect::Deny,
        rule.iter().copied(),
    )
    .expect("rule")])
    .expect("command policy");
    let policy = SandboxPolicy::standard(&sample.workspace())
        .expect("policy")
        .with_command_policy(commands);
    let service = LocalSandbox::new();
    let mut session = service
        .prepare(SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("guardrail"),
            policy,
            SandboxManifest::empty(),
        ))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    session
}

#[test]
fn a_denied_program_reached_through_a_shell_is_bounded_by_confinement_instead() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-guardrail-nested-shell");
    let secret = ungranted_secret(&sample);
    let path = secret.display().to_string();

    // Named as the program, the rule matches and nothing is launched.
    let refused = session_denying(&sample, &["*/cat", "*"]).start(
        SandboxCommand::new(
            "/bin/cat",
            [OsString::from(&path)],
            SandboxEnvironment::empty(),
        )
        .expect("command"),
    );
    assert!(matches!(refused, Err(SandboxError::Guardrail)));

    // Reached through the shell, the same program is not a word in the
    // invocation. The command starts, and what refuses it is that the path was
    // never granted rather than anything the guardrail said.
    let (status, output, errors) = finish(
        session_denying(&sample, &["*/cat", "*"])
            .start(command(&format!("cat {path}")))
            .expect("started command"),
    );

    assert!(!status.success(), "an ungranted path was read");
    assert!(!errors.is_empty(), "the refusal was silent");
    let output = String::from_utf8(output).expect("utf8");
    assert!(!output.contains("not for the sandbox"), "{output}");
}

#[test]
fn a_helper_the_script_makes_is_never_a_word_the_guardrail_reads() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-guardrail-helper");
    let secret = ungranted_secret(&sample);
    let path = secret.display().to_string();

    // The helper does not exist when the guardrail decides, and never will as
    // far as the guardrail is concerned: the command it read was a shell. It
    // says so itself before it reaches for the path, because a helper that
    // failed to run at all would leave this test proving nothing.
    let (status, output, errors) = finish(
        session_denying(&sample, &["*/helper", "*"])
            .start(command(&format!(
                "cp /bin/sh ./helper && ./helper -c 'echo ran; cat {path}'"
            )))
            .expect("started command"),
    );

    let output = String::from_utf8(output).expect("utf8");
    assert!(output.starts_with("ran"), "the helper never ran: {output}");
    assert!(!status.success(), "an ungranted path was read");
    assert!(!errors.is_empty(), "the refusal was silent");
    assert!(!output.contains("not for the sandbox"), "{output}");
}
