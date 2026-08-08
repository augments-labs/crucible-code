---
paths:
  - "crates/crucible-core/**"
---

# Changing crucible-core

Core is the only crate everything else depends on, and it depends on nothing.
Every line added here is compiled into every other crate, so the bar is what
*must* be shared rather than what is convenient to share.

## Before adding anything

Ask which crate would break if this lived one level up. If the answer is "only
the provider", it belongs to `crucible-provider`. A type used by exactly one
crate is not a domain type yet.

## Open sets are traits, closed sets are enums

- **Trait** when a new implementation should be addable without touching core:
  `Provider`, `DeltaStream`, `Tool`, `Credential`, `Ask`, `Post`. Adding an
  OpenAI adapter must not require an edit here. If it does, the seam is in the
  wrong place.
- **Enum** when a new variant *should* break every `match` that handles it:
  `Delta`, `Event`, `Message`, `Verdict`, `Sensitivity`, `StopReason`, and every
  error.
  That breakage is the feature — it is what makes the compiler enumerate the
  places a new case has to be thought about.

Never add `#[non_exhaustive]` to a core enum, and never end a `match` on one
with a `_ =>` arm. Both convert a compile error into a silent wrong answer,
which is the exact thing the enum was chosen to prevent.

## Newtypes

Anything with domain meaning gets one — `SessionId`, `ToolId`, `ApiKey`,
`WorkspacePath`, `ToolArgs`. A `String` parameter that means something
particular is a bug waiting for two arguments to be swapped at a call site the
compiler is happy with.

A newtype that wraps a secret writes `Debug` by hand and redacts. Deriving it
puts the value in every error, log line and panic payload that formats it.

## Grants

`Grant`'s field is private to `grant.rs` — not to the crate, to the module.
Widening it to `pub(crate)` would let any core module mint one, which ends the
guarantee that a verdict was reached. If a new call site needs a grant, it
takes one as an argument.
