---
paths:
  - "crates/crucible-runner/**"
---

# Changing crucible-runner

The runner drives turns to completion over `dyn Provider` and `dyn Tool`. Its
`[dependencies]` names `crucible-core` and nothing else, and that is deliberate
rather than incidental.

## Never name a concrete provider or tool

If the loop needs to know it is talking to Anthropic, the difference belongs
behind the trait. A `if provider.name() == "anthropic"` branch here is the
failure mode this crate's dependency list exists to make impossible — adding
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

## The session log is append-only

One JSON object per line, written as events happen rather than assembled at the
end — a crash mid-turn must leave a readable session, not a truncated array.

A format change is a format *version* change. A log written by a different build
is refused rather than half-understood; guessing produces a session that looks
fine and is missing turns.

## Nothing here grows with the transcript

The loop holds what the current turn needs. Anything proportional to how long
the session has run belongs on disk or in the terminal's scrollback, and a
`.clone()` of a transcript-sized value needs a comment saying why it is not one.
