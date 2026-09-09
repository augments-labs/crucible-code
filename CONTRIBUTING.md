# Contributing

Thanks for helping improve Crucible. Participation is covered by the
[Code of Conduct](CODE_OF_CONDUCT.md); report security issues through
[SECURITY.md](SECURITY.md), never a public issue.

## Set up

The Rust version and components are pinned in `rust-toolchain.toml`, so rustup
installs them on first use.

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build
cargo run -- --help
```

[Building](docs/building/index.md) lists platform packages and cross-compilation
options.

## Branches

`dev` is where work lands and `main` is what shipped. Every commit on `main`
came through `dev` first, except a hotfix, so `main` is always a state that was
released or is about to be.

Branch from `dev`, and open the pull request against `dev`. One opened against
`main` is asked to retarget before anyone reviews it — not as ceremony, but
because merging it would put an unreleased change into the branch a tag is cut
from. Only two kinds of branch target `main`: a release branch carrying the
version bump, and a hotfix for something already published.
[`RELEASING.md`](RELEASING.md) owns both.

Both branches carry the same ruleset: no direct pushes, no force-pushes, no
deletion, and `CI required` green before a merge. Everything else is a task
branch, and merging its pull request deletes it — the work is on `dev` by then,
and the ruleset is what keeps the same rule from reaching `dev` or `main`.

## Make a change

1. Branch from `dev` and keep one reason to change per pull request.
2. Read the module documentation beside the code being changed.
3. Start new behavior with a failing test; reproduce a bug before fixing it.
4. Run the narrow test while working, then the complete local gate.
5. Update user documentation and the changelog when shipped behavior changes.
6. Open a pull request and state what changed and how it was verified.

Coding agents begin in [`AGENTS.md`](AGENTS.md), which holds the repository
constraints for implementation, dependencies and writing. Human contributors
can read the same guide; remaining skills live under
[`.agents/skills/`](.agents/skills/).

## Local gates

```bash
scripts/sh/check.sh
```

This compatibility command runs all deterministic checks expected on a normal
contributor machine. Its current children can also be run independently:

```bash
scripts/sh/rust-checks.sh     # formatting, package isolation, clippy, tests, required cases and rustdoc
scripts/sh/repo-checks.sh     # cross-file repository policy and crate layering
scripts/sh/python-checks.sh   # canary and campaign harness fixtures and reports
```

A script lives under `scripts/sh` if a shell runs it and `scripts/python` if
python3 does, so looking for one language means opening one directory. A new
script goes in the directory for its language; `scripts/sh/repo-checks.sh`
fails on one left anywhere else, because a file outside those two directories
is one no gate compiles or runs.

What those scripts write lands under `generated/`, which git ignores whole: a
budget measurement, a campaign report or a canary report is evidence of one run
on one machine, not something the repository carries. A single document goes
under the directory for its format, `generated/json` or `generated/txt`, and a
command that writes a tree gets a directory of its own. A generated file the
repository does keep, such as the configuration schema, is committed where it
is read instead, and `scripts/sh/repo-checks.sh` fails on anything tracked
under `generated/`.

Run the checks from a checkout whose directories are not group-writable, which
is what `umask 022` produces. Linux sandboxing refuses a broker image that a
group member could rewrite, and it walks the whole path to it, so a tree
created under `umask 002` fails the sandbox tests for its mode rather than for
anything in the change under test. The error names the directory.

`scripts/required-cases.json` names the obligations that must keep running
whatever the tests are called: `scripts/sh/rust-checks.sh` checks that each one
is still discovered by the same selection the suite runs under, is not ignored,
still hashes to the source recorded for it, and passes when run by exact name.
Moving a case is a `source` edit. Changing what one asserts is a `body_sha256`
edit, and the reviewer is agreeing to the new assertion, not to a green total.

A `doc` entry names a documentation example instead of a function, and its hash
covers the fenced block including the opening fence. That is where the assertion
lives for one of those: `compile_fail` is what makes the example a proof, and
`ignore`, `no_run`, `text` or another edition would leave it listed and green
while it proves nothing. rustdoc does not check the error code written beside
`compile_fail`, so that code records which failure the example is about rather
than enforcing it, and the hash is what keeps both from changing unreviewed.

The Rust tests include the whole-screen pseudo-terminal suite. Run that suite on
its own with:

```bash
cargo test --test whole_screen
```

Snapshot changes must be reviewed as terminal screens, not accepted merely to
make a test green.

CI has additional owners for supported-platform tests, dependency licenses and
sources, performance, advisories and release artifacts. The workflow map is in
[`.github/workflows/README.md`](.github/workflows/README.md). Advisories remain
separate because their answer can change without a source change.

## Pull requests

Scope is decided by purpose, not changed-line arithmetic. A pull request that
needs two independent summaries is usually two changes; a module whose code and
proof do not compile apart remains one.

Use the pull-request template and answer every section it asks for. A blank
section, kept placeholder text or bundled unrelated changes get the pull
request closed rather than reviewed. The surfaces it lists — security
boundaries, durable formats, generated files, platform-specific behavior,
terminal rendering, performance-sensitive paths and required-case obligations —
are the ones a reviewer cannot recover from the diff alone. `CHANGELOG.md` is
for user-visible changes, written for someone deciding whether to upgrade.

It opens by asking who made the change, because a reviewer reads a generated
diff with different questions than a hand-written one, and which model, harness
and plugins produced it is part of reproducing the work. It closes by asking
whether a person has read the complete diff. A green gate is evidence about the
checks that ran, not about whether the change answers the right problem, so
that reading is a separate thing a pull request either has or is still waiting
for — and one nobody has read yet leaves the box empty rather than claiming
otherwise.

## Dependencies

Use the [dependency guidance](AGENTS.md#dependencies) when adding a crate or
widening its features. Repository checks enforce exact
pins, manifest justification and the current internal crate graph. Blocking CI
installs `cargo-deny` and enforces license and source policy; contributors do not
need that tool for the ordinary local gate.

## Performance

Performance-sensitive changes must run:

```bash
scripts/sh/bench.sh
```

The probes and thresholds are owned by that script. Use its `startup`,
`tools`, `mem`, `grep`, `stream`, or `live` argument to run one family. Shared
CI runners provide a trend; release measurements are taken on a quiet machine
as described in [`RELEASING.md`](RELEASING.md).

Compile-time changes can also be compared without an absolute gate:

```bash
scripts/sh/build-comparison.sh BASE CANDIDATE
```

That command requires a clean checkout, checks both revisions on the same
machine with independent Cargo targets, and writes clean, no-op, leaf-touch and
root-touch timing/RSS evidence plus Cargo timing reports. Live release,
provider, and coding-task campaigns are scheduled or manual workflows rather
than local or pull-request requirements; their ownership and credentials are
listed in [the workflow map](.github/workflows/README.md).

## Commit messages

Use a conventional subject with the affected area when useful:

```text
feat(tools): add glob tool
fix(runner): stop an in-flight stream on escape
chore(ci): split Rust and repository checks
```

Use [Writing the change](AGENTS.md#writing-the-change) for the repository's
commit, changelog and pull-request conventions.
