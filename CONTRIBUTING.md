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

## Make a change

1. Branch from `main` and keep one reason to change per pull request.
2. Read the module documentation beside the code being changed.
3. Start new behavior with a failing test; reproduce a bug before fixing it.
4. Run the narrow test while working, then the complete local gate.
5. Update user documentation and the changelog when shipped behavior changes.
6. Open a pull request and state what changed and how it was verified.

Coding agents begin in [`CLAUDE.md`](CLAUDE.md), which routes implementation,
dependency, writing and gate work to focused skills. Human contributors can read
the same skill files under [`.claude/skills/`](.claude/skills/).

## Local gates

```bash
scripts/check.sh
```

This compatibility command runs all deterministic checks expected on a normal
contributor machine. Its current children can also be run independently:

```bash
scripts/rust-checks.sh   # formatting, all-feature clippy, tests and rustdoc
scripts/repo-checks.sh   # cross-file repository policy
```

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

Use the pull-request template. Call out security boundaries, generated files,
platform-specific behavior and performance-sensitive paths when they moved.
`CHANGELOG.md` is for user-visible changes, written for someone deciding whether
to upgrade.

## Dependencies

Use the [`add-a-dependency`](.claude/skills/add-a-dependency/SKILL.md) procedure
when adding a crate or widening its features. Repository checks enforce exact
pins, manifest justification and the current internal crate graph. Blocking CI
installs `cargo-deny` and enforces license and source policy; contributors do not
need that tool for the ordinary local gate.

## Performance

Performance-sensitive changes must run:

```bash
scripts/bench.sh
```

The probes and thresholds are owned by that script. Shared CI runners provide a
trend; release measurements are taken on a quiet machine as described in
[`RELEASING.md`](RELEASING.md).

## Commit messages

Use a conventional subject with the affected area when useful:

```text
feat(tools): add glob tool
fix(runner): stop an in-flight stream on escape
chore(ci): split Rust and repository checks
```

Use the `write-the-change` skill for the repository's commit, changelog and
pull-request procedure rather than maintaining another copy here.
