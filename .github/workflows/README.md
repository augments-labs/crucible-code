# Workflow ownership

`blocking-ci.yml` is the pull-request and `main` entrypoint. It calls focused
reusable workflows and exposes `CI required` as the single merge result.

| Workflow | Owns |
| --- | --- |
| `rust-ci.yml` | Rust formatting, all-feature linting, tests and rustdoc on supported CI platforms |
| `repo-checks.yml` | Deterministic cross-file repository policy |
| `dependency-policy.yml` | Blocking Cargo license, source and ban policy |
| `performance.yml` | Performance probes and their JSON artifact |
| `provider-canaries.yml` | Weekly/manual non-blocking live turns against configured provider accounts |
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
exercised there rather than skipped. The Intel and Apple silicon macOS jobs set
the same requirement and exercise the built-in Seatbelt backend. The x86_64 and
ARM64 Windows jobs provision their versioned dedicated sandbox account and WFP
policy, require the native backend for the full test run, and remove that
machine state in an always-run cleanup step. The release gate and `rust-ci.yml`
share the Linux setup action so they cannot drift apart.

Actions are pinned to full commit SHAs. A trailing comment records the release
name for maintainers; the SHA is what executes.

## Live provider canaries

`provider-canaries.yml` is deliberately outside `blocking-ci.yml`: external API
availability, account state and provider spend are not pull-request verdicts.
It runs weekly or by hand, and each provider row reports failures independently.
The workflow can go red, but is not called by the pull-request gate and cannot
block one. Add any of these repository secrets to enable its
row; an absent secret is a recorded skip:

- `ANTHROPIC_CANARY_API_KEY`
- `MOONSHOT_CANARY_API_KEY`
- `OPENAI_CANARY_API_KEY`

The workflow uses API keys only. Browser/device account login needs dedicated
automated accounts and is not inferred from a developer's stored credentials.
Each configured row sends one tiny prompt through the release-mode binary and
requires the marker in the streamed answer within 90 seconds.
