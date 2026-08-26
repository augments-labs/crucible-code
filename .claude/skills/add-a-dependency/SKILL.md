---
name: add-a-dependency
description: >-
  Add a crate to the workspace — the ladder to walk first, the =-pin, the
  comment the gate requires, and the two files it has to be declared in. Use
  whenever a change would introduce a new crate, or widen an existing one.
---

# Add a dependency

A crate added for one function is a permanent cost for a temporary convenience.
Everything below assumes you already decided you cannot avoid it.

And when you cannot: **add it, and build the feature.** The ladder decides which
rung the answer is on, never whether the work ships. Cutting a feature because
it would need a dependency is under-delivery wearing discipline as a costume.

## Before you add it

Stop at the first rung that holds:

1. **Does the task actually need it**, or is it for a case nobody has hit?
2. **Standard library.** `std` covers more than it is given credit for.
3. **A crate already in the tree.** Check `[workspace.dependencies]` first —
   widening a feature flag is cheaper than a new name.
4. **A few lines of your own.** If the answer is under about thirty lines and has
   no protocol, no parser and no platform branching, write it.
5. **Then the crate.**

## Adding it

Two files, always both:

```toml
# Cargo.toml — [workspace.dependencies]

# Why this crate is here, in a sentence or two that a reviewer who has never
# heard of it can act on. Say what it does that std does not.
some-crate = "=1.2.3"
```

```toml
# crates/<crate>/Cargo.toml — [dependencies]
some-crate.workspace = true
```

The `=` is not decoration. It keeps a release reproducible and makes a version
bump a reviewed change rather than a side effect of somebody else's publish.
The repository gate fails on a caret. Dependabot moves the pin as a pull request
that CI has to pass.

The workspace entry is where the pin and the reason live, which is why the
member manifest names neither: `some-crate.workspace = true` inherits both. Both
gates read every manifest, so declaring the version in the member instead — a
plausible shortcut, and one that used to pass — fails on the pin and on the
missing comment together.

## Where it may go

Read the current internal graph from the manifests and add the narrowest edge
that supports the feature. The repository gate requires a deliberate graph
update, so a new provider, tool, sandbox or other plugin crate cannot acquire an
unreviewed dependency on another implementation crate.

Adding a crate to `crucible-core` puts it in every other crate's build. Push it
as far up the graph as it will go — if only one provider adapter needs it, it
belongs with that adapter, not in `core`.

## What the choice has to satisfy

- **No panicking API on a shipped path.** `unwrap_used`, `expect_used`, `panic`
  and `indexing_slicing` are denied. A crate whose only interface panics is a
  crate you will wrap anyway.
- **Errors stay typed.** The crate's error must be something a `thiserror` enum
  can hold with `#[from]`. Nothing that erases the type.
- **It must not print.** `print_stdout` and `print_stderr` are denied; a crate
  that writes to the terminal itself will corrupt the transcript.
- **Weight is a feature decision.** Compile time, binary size and startup cost
  all count against the budgets in `scripts/bench.sh`.
- **Its licence has to be on the list, and so does everything it drags in.** The
  binary is distributed under MIT, and that promise is only as true as what it
  statically links. `deny.toml` names the permissive licences this tree uses; a
  licence not on it is a decision to argue in the pull request, not a line to
  add quietly.
- **crates.io only.** A git dependency has no version field, so the pinning gate
  has nothing to look at and passes it in silence — while the revision it names
  can be force-pushed underneath you.

## After

```bash
cargo build          # refresh Cargo.lock
scripts/check.sh
```

Blocking CI supplies cargo-deny and checks licenses and sources. `audit.yml`
checks advisories when dependencies change and weekly regardless — because
pinning is exactly what stops that answer from changing on its own. Install the
pinned cargo-deny version only when you want to reproduce those CI checks
locally.

Commit the lockfile with the change. `chore(deps): add <crate> for <reason>`.
