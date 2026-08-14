---
paths:
  - "crates/crucible-core/**"
---

# Changing crucible-core

Every line added here is compiled into every other crate, so the bar is what
*must* be shared rather than what is convenient to share.

## Before adding anything

Ask which crate would break if this lived one level up. If the answer is "only
the provider", it belongs to `crucible-provider`. A type used by exactly one
crate is not a domain type yet.

## Which side of the open/closed line a new type is on

`CLAUDE.md` says why the split exists. What it does not say is where the
existing types sit, and a new one has to join a side:

- **Trait**, so a new implementation needs no edit here: `Provider`,
  `DeltaStream`, `Tool`, `Credential`, `Ask`, `Post`. If adding an adapter
  requires a change to this crate, the seam is in the wrong place.
- **Enum**, so a new variant breaks every `match` that handles it: `Delta`,
  `Event`, `Message`, `Verdict`, `Settled`, `Sensitivity`, `Mode`,
  `Disposition`, `StopReason`, and every error.

Never add `#[non_exhaustive]` to a core enum, and never end a `match` on one
with a `_ =>` arm — not in this crate and not in any crate that reads one. Both
convert a compile error into a silent wrong answer, which is the exact thing
the enum was chosen to prevent. A `match` that genuinely treats several
variants alike spells them out with `|`, so the next variant added still stops
the build.

## Newtypes

A `String` parameter that means something particular is a bug waiting for two
arguments to be swapped at a call site the compiler is happy with. That is the
whole argument; the list of which ones exist is in the modules that define
them.

A newtype that wraps a secret writes `Debug` by hand and redacts. Deriving it
puts the value in every error, log line and panic payload that formats it.

## Grants

`Grant`'s field is private to `permission/grant.rs` — not to the crate, to the
module. Widening it to `pub(crate)` would let any core module mint one, which
ends the guarantee that a verdict was reached. A grant leaves the engine only
inside an `Approved`, bound to the call it was reached about; a new call site
that needs one takes an `Approved` as an argument — never a bare `Verdict`,
which any caller can construct.
