# Workflow ownership

`blocking-ci.yml` is the pull-request and `main` entrypoint. It calls focused
reusable workflows and exposes `CI required` as the single merge result.

| Workflow | Owns |
| --- | --- |
| `rust-ci.yml` | Rust formatting, all-feature linting, tests and rustdoc on supported CI platforms |
| `repo-checks.yml` | Deterministic cross-file repository policy |
| `dependency-policy.yml` | Blocking Cargo license, source and ban policy |
| `performance.yml` | Performance probes and their JSON artifact |
| `audit.yml` | Advisories whose answer changes as databases are published |
| `codeql.yml` | GitHub code scanning |
| `release.yml` | Tag validation, artifacts, attestations and publication |

A new language gets a peer reusable workflow such as `python-ci.yml` or
`js-ci.yml`, then one call and one dependency in `blocking-ci.yml`. Do not add
another language's setup to `rust-ci.yml` or `repo-checks.yml`.

Successful read-only jobs finish with
`.github/actions/check-clean-worktree`, which rejects tracked edits or untracked
files left by a check. Ignored build output is outside that invariant.

Every Linux job that runs the Rust gate first runs
`.github/actions/enforcing-sandbox` and sets
`CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX`, so the enforcing sandbox tests are
exercised there rather than skipped. The release gate and `rust-ci.yml` share
that one action so they cannot drift apart.

Actions are pinned to full commit SHAs. A trailing comment records the release
name for maintainers; the SHA is what executes.
