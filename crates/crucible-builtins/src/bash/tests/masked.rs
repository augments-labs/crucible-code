//! What a call answers when its command prints the credential its proxy was
//! given: the masked form only, whether the command ended its own output or
//! crucible cut it short.
//!
//! Read end to end, through the tasks that collect a command's output, so a
//! collector that read anything but the stream the sandbox hands over would
//! show the credential here. Linux only, because the proxy that carries a
//! credential into a command is the enforcing backend's.

use std::sync::Arc;

use crucible_sandbox::{
    SandboxDomainPolicy, SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy,
};

use super::super::Bash;
use super::{Tool, awaited};
use crate::sample::{Sample, allowed, enforcing};

/// How many bytes the proxy's password is: 32 random bytes, in hex.
const PASSWORD: usize = 64;

/// A tool whose commands are confined with a proxy they may send nothing
/// through, so each is started with the proxy's credential in its
/// environment. `None` where this machine cannot enforce confinement.
fn proxied(sample: &Sample) -> Option<(Bash, std::sync::MutexGuard<'static, ()>)> {
    let service = crate::sample::sandbox();
    let guard = enforcing(&service)?;
    let standard = SandboxPolicy::standard(&sample.workspace()).expect("a standard policy");
    let network = SandboxDomainPolicy::new(
        Vec::new(),
        [],
        false,
        Vec::new(),
        SandboxNetworkProvenance::User,
    )
    .expect("a domain policy that grants nothing");
    let policy = SandboxPolicy::new(
        true,
        standard.filesystem().iter().cloned(),
        standard.working_directory(),
        SandboxNetworkPolicy::Domains(network),
        standard.limits(),
    )
    .expect("a policy with a proxy");
    Some((
        Bash::new(sample.workspace(), Arc::new(service)).under_policy(policy),
        guard,
    ))
}

#[test]
fn a_command_that_prints_its_proxy_credential_is_answered_with_it_masked() {
    let sample = Sample::new("bash-masked-whole");
    let Some((tool, _enforcing)) = proxied(&sample) else {
        return;
    };

    let output = awaited(tool.run(
        allowed(&tool, r#"{"command":"printf '%s' \"$HTTP_PROXY\""}"#),
        &crate::sample::context(),
    ))
    .expect("a confined command ran");

    assert!(
        output
            .text()
            .starts_with(&format!("http://crucible:{}@", "*".repeat(PASSWORD))),
        "the proxy's password reached the answer unmasked: {}",
        output.text()
    );
}

#[test]
fn a_command_cut_short_after_printing_the_start_of_its_credential_is_answered_with_it_masked() {
    // The start of a credential is held back until it cannot be one, and a
    // command stopped for running too long can never finish it: what was
    // held is masked whole rather than released, so the incomplete answer
    // carries nothing of the password either.
    let sample = Sample::new("bash-masked-cut");
    let Some((tool, _enforcing)) = proxied(&sample) else {
        return;
    };

    let output = awaited(tool.run(
        allowed(
            &tool,
            r#"{"command":"printf 'id=%.20s' \"${HTTP_PROXY#http://crucible:}\"; exec sleep 30","timeout":1}"#,
        ),
        &crate::sample::context(),
    ))
    .expect("a confined command ran");

    assert!(
        output.text().contains("[stopped: the command ran too long"),
        "{}",
        output.text()
    );
    assert!(
        output
            .text()
            .starts_with(&format!("id={}\n\n[stopped:", "*".repeat(20))),
        "the start of the proxy's password reached the answer unmasked: {}",
        output.text()
    );
}
