# Workflow ownership

`blocking-ci.yml` is the pull-request entrypoint. It runs on every pull request
and on every push to `dev` and `main`. It calls focused reusable workflows and
exposes `CI required` as the single merge result. `dev` and `main` both merge
directly once `CI required` is green on a head that is up to date with the
base.

A pull request tests on Linux, Apple silicon macOS and x86_64 Windows, and
only compiles for Windows ARM64. macOS Intel and Windows ARM64 were the slowest
platforms to test, so their tests and rollback drills run on the push a merge
makes to `dev` or `main`, where `CI required` covers all five. A failure there
is fixed on `dev` like any other red run, and a release is tagged only after
the push run on `main` passes.

A pull request that changes only documentation, as `scripts/sh/docs-only.sh`
decides, runs no macOS or Windows job at all: no test that runs only there
reads a document, so its merge's push run covers them. Linux still runs every
test, and `CI required` fails if the classifying job does.

macOS machines are the ones a run waits longest for, so a macOS platform takes
as few as it can: Apple silicon runs its tests in one job rather than in parts,
and each macOS checks job runs that platform's rollback drill, through
`.github/actions/rollback-drill`, instead of a drill job of its own.

| Workflow | Owns |
| --- | --- |
| `rust-ci.yml` | Rust formatting on Linux; the tests in hashed parts, one machine each; all-feature linting, tests and rustdoc on supported CI platforms; install tests on macOS and in a FreeBSD guest, and the Windows installer's tests under Windows PowerShell 5.1 and PowerShell 7; the rollback drill and its self-test; which of the five platforms a run covers, through its `all-platforms` and `docs-only` inputs |
| `repo-checks.yml` | Deterministic cross-file repository policy |
| `python-ci.yml` | Python canary and campaign harness syntax, fixtures and report validation |
| `dependency-policy.yml` | Blocking Cargo usage, license, source and ban policy |
| `performance.yml` | Blocking startup, typed-tool, memory, search and rendering budgets with a JSON artifact |
| `build-observations.yml` | Weekly/manual same-runner clean and incremental Cargo comparison; observational only |
| `provider-canaries.yml` | Weekly/manual non-blocking multi-turn typed-tool and normalized usage/cache canaries |
| `release-canary.yml` | Weekly/manual install, execute and uninstall check of the newest published release on Linux and Windows, and that the release container digest still resolves |
| `task-campaign.yml` | Manual baseline/candidate coding-task campaign with independent fixture verification |
| `audit.yml` | Advisories whose answer changes as databases are published |
| `codeql.yml` | GitHub code scanning |
| `release.yml` | Dispatched: tag validation, artifacts, and the install, upgrade and rollback cells on those staged bytes. On a tag: promotion of a staged run's bytes by digest, attestations and publication |

A new language gets a peer reusable workflow such as `python-ci.yml` or
`js-ci.yml`, then one call and one dependency in `blocking-ci.yml`. Do not add
another language's setup to `rust-ci.yml` or `repo-checks.yml`.

Successful read-only jobs finish with
`.github/actions/check-clean-worktree`, which rejects tracked edits or untracked
files left by a check. Ignored build output is outside that invariant.

Every Linux job that runs the Rust tests first runs
`.github/actions/enforcing-sandbox` and sets
`CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX`, so the enforcing sandbox tests are
exercised there rather than skipped. The macOS jobs that run the tests (Apple
silicon on every run, Intel on a push) set the same requirement and exercise
the built-in Seatbelt backend. The Windows jobs that run the tests (x86_64 on
every run, ARM64 on a push) provision their versioned dedicated sandbox account
and WFP policy, require the native backend for the full test run, and remove
that machine state in an always-run cleanup step. The release gate and `rust-ci.yml`
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
executables are gone. Its Windows job runs `scripts/ps1/release-canary.ps1`,
which does the same with the published `install.ps1`. It complements the
hermetic archive and rollback matrix; public release availability is
intentionally not a merge condition. Its `container` job runs
`scripts/sh/release-container.sh`, which asks the registry for the container
digest `release.yml` pins and fails once the registry has collected it, so a
dead pin is repointed before a tag needs it.

`task-campaign.yml` is manual-only. Supply a provider-qualified model and a
baseline release version. It runs the versioned suite in
`benchmarks/coding-tasks/suite.json` against both binaries, invokes each task's
independent verifier, and publishes pass count, duration, normalized token
and per-currency cost comparisons. It can spend provider funds and requires at
least one provider canary secret; ordinary pull requests never invoke it.
