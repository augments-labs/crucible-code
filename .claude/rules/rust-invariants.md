---
paths:
  - "crates/**/*.rs"
  - "src/**/*.rs"
---

# The four invariants every Rust file here holds

## Result, never panic

Every fallible function returns `Result<T, E>` with a module-owned `thiserror`
enum; `?` propagates. No `anyhow` — it erases the type and invites string
errors. `main` is the only place an error becomes an exit code.

Denied by lint; tests are exempt.

## Parse once, at the boundary

Model output, tool arguments, config, env and file contents are parsed into
domain types at the edge. Inner layers never re-validate.

Anything with domain meaning gets a newtype — `SessionId`, `WorkspacePath`,
`ApiKey` — never a bare `String`.

## Permission is an argument, not a question

A function that mutates a file or spawns a process takes an `Approved`: the call
itself, carried together with the `Grant` that says a verdict was reached about
*that* call.

`Grant` has a private field and is minted only by the permission engine, so it
cannot be forged, a `Deny` cannot be passed off as an allow, and proof reached
about one call cannot arrive beside another call's arguments. Code without one
cannot call the operation.

## Secrets never surface

Not in logs, errors, `Display`, `Debug`, session files or panic payloads. Types
holding a key implement `Debug` by hand and redact. Config stores env var
*names*, never values.
