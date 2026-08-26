---
name: change-crucible
description: >-
  Change Crucible's Rust code while preserving crate boundaries, typed security
  seams, bounded data, provider neutrality and terminal ownership. Use for any
  implementation change under crates/ or src/, especially when adding a
  provider, tool, command, sandbox, subagent, skill or other pluggable feature.
---

# Change Crucible

Start with the module documentation above the code being changed. It is kept
beside the implementation and tests, so it is the current statement of local
invariants; do not maintain a second architectural specification here.

## Put the feature at the right seam

- Open sets are traits. A provider, tool, sandbox, subagent, skill loader or
  future adapter should be addable without editing `crucible-core` merely to
  name the implementation.
- Closed domain state is an enum and is matched exhaustively where a new case
  must force every consumer to decide.
- `src/` is composition: concrete implementations meet there and become trait
  objects before they travel downward.
- `crucible-runner` drives domain traits. It must not depend on a concrete
  provider, tool or future plugin implementation.
- Push a dependency up the graph to the narrowest crate that needs it; use the
  `add-a-dependency` skill when the graph gains or widens a third-party crate.

The repository gate enforces current crate edges. It cannot tell whether a new
abstraction belongs in core; answer that by asking which independent
implementations need it now or are made possible by it.

## Preserve the typed boundaries

- Parse external text once at the owner of its format: provider wire objects in
  provider modules, tool arguments in the tool, configuration in config, and
  session lines in session.
- Give distinct domain meanings distinct types. Do not pass permission as a
  bool or a constructible verdict: operations run from `Approved`, bound to the
  exact call the permission engine settled.
- Secrets are applied, never exposed for convenience. Any value that can hold a
  credential redacts `Debug`, is absent from `Display`, errors and session logs,
  and registers exact outgoing representations for response redaction.
- Treat model output and checked-out files as hostile input. Validate path reach
  through `Workspace`; process execution is the explicit exception and must be
  classified by what it will run.

Tests and compile-fail examples already protect the current grant, replay,
credential and workspace seams. Add the equivalent test when a new plugin type
creates another privileged route.

## Keep growth and output bounded

Provider responses, tool output, configuration documents, prompts, session
indexes and screen records already have owner-specific ceilings. New retained
or streamed data needs a bound before bytes are stored, and a truncated result
must say it is incomplete. The transcript is the runner's intentionally growing
value; do not make a second session-sized copy.

Run `scripts/bench.sh` when startup, rendering, searching, retained session data
or hot-path allocation changes. A budget changes only as an explicit product
decision, never to make a regression fit.

## Keep one terminal owner

Only the drawing thread writes terminal output. Other threads report events.
Components lay out against the current width and, where given one, current room;
add new components to the fit sweep. A terminal mode is borrowed through a guard
that restores it from `Drop` and does nothing for redirected output.

## Make coupled changes explicit in code

Prefer one registry or exhaustive match over lists synchronized by prose. Where
one source cannot own both sides, add an agreement test. Current examples are
the configuration shape and generated schema, provider registry and
construction, session line shape and format number, component signatures and
the fit sweep, and performance budgets and their probes.

Use the `run-the-gate` skill when adding or changing a mechanical check.
