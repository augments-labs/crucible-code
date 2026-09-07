# Workflow ownership

`blocking-ci.yml` is the pull-request and `main` entrypoint. It calls focused
reusable workflows and exposes `CI required` as the single merge result.

| Workflow | Owns |
| --- | --- |
| `rust-ci.yml` | Rust formatting, all-feature linting, tests and rustdoc on supported CI platforms |
| `repo-checks.yml` | Deterministic cross-file repository policy |
| `python-ci.yml` | Python canary and campaign harness syntax, fixtures and report validation |
| `dependency-policy.yml` | Blocking Cargo usage, license, source and ban policy |
| `performance.yml` | Blocking startup, typed-tool, memory, search and rendering budgets with a JSON artifact |
| `build-observations.yml` | Weekly/manual same-runner clean and incremental Cargo comparison; observational only |
| `provider-canaries.yml` | Weekly/manual non-blocking multi-turn typed-tool and normalized usage/cache canaries |
| `release-canary.yml` | Weekly/manual install, execute and uninstall check of the newest published release |
| `task-campaign.yml` | Manual baseline/candidate coding-task campaign with independent fixture verification |
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

Windows tests run under Git Bash so their concurrent shell children share an
initialized MSYS runtime. Starting them independently from PowerShell can race
MSYS mount-table initialization before a command runs. Sandbox setup and removal
still use PowerShell, and test concurrency and native enforcement stay enabled.

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
Each configured row asks the release-mode binary to make one harmless local
file through its typed `write` tool, then read it in a second turn. The driver
requires both answer markers, the exact file effect, a successful invocation
journal record, and normalized usage facts for multiple provider requests. Its
artifact records cache outcomes, tokens and normalized cost when the provider
reports them; an exact token count or cache hit is evidence, never a gate.

## Scheduled observations and manual campaigns

`build-observations.yml` checks a base and candidate revision on the same Linux
runner with separate target directories. It records clean, no-op, leaf-touch
and root-touch Cargo checks, peak RSS and Cargo's timing pages. No absolute
threshold or pull-request dependency is attached to it; compare the two sides
of one artifact rather than readings from different machines.

`release-canary.yml` installs the newest public release into a temporary prefix,
runs `--version`, uninstalls it with the repository script and proves all owned
executables are gone. It complements the hermetic archive and rollback matrix;
public release availability is intentionally not a merge condition.

`task-campaign.yml` is manual-only. Supply a provider-qualified model and a
baseline release version. It runs the versioned suite in
`benchmarks/coding-tasks/suite.json` against both binaries, invokes each task's
independent verifier, and publishes pass count, duration, normalized token
and per-currency cost comparisons. It can spend provider funds and requires at
least one provider canary secret; ordinary pull requests never invoke it.
