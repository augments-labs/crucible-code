---
paths:
  - "crates/crucible-tools/**"
---

# Changing crucible-tools

Every tool here can be driven by a model that read its instructions from a file
in the workspace. Treat the arguments as hostile input, because the path from
"attacker writes a README" to "tool runs with the user's permissions" is short.

## A new tool

Implement `Tool`, and answer three questions in code rather than in a comment:

1. **What `Sensitivity` does this call carry?** Not the tool — the *call*.
   `bash` running `ls` and `bash` running `curl` are different answers, and the
   tool works this out from its own arguments because nothing else can parse
   them.
2. **What program is about to run?** For `SpawnsProcess`, name it the way the
   user would. The prompt shows this string and a session-wide allow remembers
   it, so a vague answer buys a vague question and a wider grant than anyone
   agreed to.
3. **What does it reach?** Every path is resolved against the workspace and
   rejected if it escapes — after symlinks are followed, not before.

## Grants

Every tool takes a `Grant`, read-only ones included. A read mints its own
without a prompt, but it still leaves through `Permission::decide`, so there is
one route to running a tool rather than a guarded route and an unguarded one.

Never take a `Verdict` as the argument instead. Nothing stops a caller passing
`Deny` and carrying on — that is the whole reason the token exists.

## Output is bounded, always

A tool's output goes into the next request, so an unbounded one is an unbounded
bill and a context window spent on a log file somebody cat'd by accident.
Truncate at a limit the tool owns, and say in the output that it was truncated —
a silently cut result reads to the model as a complete one.

## Failure is a result, not an error

A tool that cannot do the thing returns `ToolOutput::failed` with text the model
can act on. `ToolError` is for the tool being unusable — bad arguments, a denied
grant. The difference matters: a failed output continues the turn and lets the
model try something else; an error ends it.
