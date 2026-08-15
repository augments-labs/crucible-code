---
paths:
  - "crates/crucible-runner/**"
---

# Changing crucible-runner

The runner drives turns to completion over `dyn Provider` and `dyn Tool`. Its
`[dependencies]` names `crucible-core` and two crates that hold no opinion about
the loop — `serde_json` for the session log and `thiserror` for its error. No
sibling crate belongs in that list, and that is deliberate rather than
incidental.

## Never name a concrete provider or tool

If the loop needs to know it is talking to Anthropic, the difference belongs
behind the trait. A `if provider.name() == "anthropic"` branch here is the
failure mode that dependency list exists to make impossible — adding
`crucible-provider` to `Cargo.toml` to write it is not the fix.

This is what keeps the loop testable without a network: the tests drive it with
a fake that answers from a script.

## A turn ends once

Every turn reaches exactly one terminal state, and the transcript has to say
which. A stream that stopped because the provider hit a token ceiling or a
content filter is *not* the same as one that finished, and a truncated answer
that reads as complete is worse than a visible failure.

Cancellation is the same shape: everything already produced is delivered before
the loop stops, so the transcript never ends mid-sentence with no explanation.

## A format change is a format version change

A log written by a different build is refused rather than half-understood;
guessing produces a session that looks fine and is missing turns. So a change
to what a line holds is a change to `wire::FORMAT` in the same commit — the
append-only shape is described where it is implemented, but the version is the
part a change to this crate has to remember.

## The transcript is the only thing that grows

It is held whole and lent to the provider while one request is serialized. The
provider stream cannot retain that borrowed request after `stream` returns.
Nothing *else* here may become proportional to how long the session has run:
what the loop holds besides it is what the current turn needs, and a `.clone()`
of a transcript-sized value needs a comment saying why.

One provider response is bounded before appending: 8 MiB of visible text,
1 MiB of tool arguments, 128 tool calls, and explicit per-field and cumulative
metadata ceilings. Stop is terminal; accepting anything after it would let a
malformed stream keep growing a response the model already ended.
