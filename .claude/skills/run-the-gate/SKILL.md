---
name: run-the-gate
description: >-
  Run Crucible's local checks and act on failures. Use before claiming a change
  is done, and when adding or changing a lint, test or CI policy.
---

# Run the gate

For today's Rust-only tree:

```bash
scripts/check.sh
```

That compatibility command runs:

```bash
scripts/rust-checks.sh   # rustfmt, all-feature clippy, tests, rustdoc, generated agreement
scripts/repo-checks.sh   # cross-file repository policy
```

CI calls the named scripts through separate reusable workflows. Future Python,
JS/TS or other code gets a peer script and workflow; do not hide another
language's setup inside the Rust gate.

Every deterministic local section should answer from the checked-out tree alone.
CI supplies tools such as cargo-deny for checks not required on a contributor
machine. Checks whose answer changes on a clock, such as newly published
advisories, run in their own scheduled workflow.

## Reading a failure

All independent sections run and the summary names each failed section. Work
from the first useful `FAIL` line rather than the final count. Tests may rewrite
`schema/crucible-code-schema.json` when its generator disagrees; review that
diff before rerunning.

Performance is separate because shared runners are noisy:

```bash
scripts/bench.sh
```

Each probe owns its threshold and exits non-zero when it misses. Run it for
startup, rendering, search or retained-memory changes, and before a release on a
quiet machine.

## Adding a gate

A passing check is unfinished until its failure has been observed:

1. Write the smallest check at the owner of the property: compiler lint, unit or
   integration test, language gate, or repository gate, in that order.
2. Break the tree in exactly the way it should catch.
3. Run the narrow command and read the message. It must identify the property
   and remedy, not merely return non-zero.
4. Restore by diff or hash, not memory.
5. Run the narrow command and then the complete local gate.

Also falsify the quiet branch: remove or rename what the check walks and confirm
it reports that it measured nothing. Test an awkward legal case too; a false
positive teaches contributors to bypass the gate.

## Choosing the owner

- `Cargo.toml [workspace.lints]` or `clippy.toml`: syntax and Rust semantic
  patterns the compiler can see.
- Rust test: behavior, generated agreement, security boundary or a closed set
  the type system can exercise.
- `scripts/repo-checks.sh`: deterministic cross-file or repository structure.
- A language-specific script: formatting, linting and tests for that ecosystem.
- A dedicated workflow: scheduled/network-dependent scans, platform matrices,
  performance, release and other independently owned CI concerns.
- Skill: a multi-step procedure or architectural judgement CI cannot prove.
- Always-loaded rule: only a policy that applies before any file trigger and
  cannot be mechanically observed.
