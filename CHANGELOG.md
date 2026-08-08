# Changelog

Notable changes to crucible. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **`0.0.x` is unstable.** Configuration files, session files and the
> command-line surface may change in any `0.0.x` release with no deprecation
> period. Nothing in this line carries a compatibility guarantee.

## [Unreleased]

### Added

- A session that runs. `crucible` reads a prompt, streams the model's answer
  inline, runs tools, and asks before anything that changes a file or starts a
  process. `--continue` carries on the most recent session started in the
  current directory. An answer the provider cut short — a token ceiling or a
  content filter — says so under the turn, so a truncated answer cannot be
  mistaken for a complete one.
- A startup that fails leaves no session behind. Everything that can fail on the
  way in runs before the session is started, so a wrong `--model` or an unset
  key writes nothing: an empty session would otherwise be the newest one for the
  directory, and `--continue` would offer it instead of the last conversation.
- Two providers, chosen by `--model [provider/]model`: `anthropic` (the default
  for an unqualified name, keyed by `ANTHROPIC_API_KEY`) and `openai` (keyed by
  `OPENAI_API_KEY`). Authentication is a separate axis from the wire protocol —
  a provider is handed a resolved credential and never learns which kind it was.
- Six tools: `read`, `grep`, `glob`, `edit`, `write`, `bash`. Every one of them
  takes a permission token that only a verdict can mint, so code that has not
  obtained one cannot call the operation; a read mints its own, and a file
  change or a command asks first. `always` remembers the tool for a file change
  and the tool *and program* for a command, and is never written to disk.
- Session log: one JSON object per line, one file per session, under
  `$XDG_DATA_HOME/crucible/sessions`. A log from a build with a different format
  is refused rather than half-understood.
- `docs/` — getting started, providers and models, permission, and sessions.
- Cargo workspace: `crucible-core`, `crucible-provider`, `crucible-tools`,
  `crucible-runner`, `crucible-tui`, and the `crucible` binary. Dependencies
  point down only, enforced by cargo.
- `scripts/check.sh` — formatting, clippy with `-D warnings`, tests, a
  400-line-per-file cap, pinning checks for dependencies, GitHub Actions and the
  agent instruction files, and a check that CI stops excusing a failing budget
  once the first bench probe exists. CI runs the same script.
- `scripts/bench.sh` — one probe per performance budget, selectable by mode
  (`startup`, `mem`, `grep`, `stream`, or all of them). Writes a JSON document
  to stdout and a readable summary to stderr, so one run serves a pipeline and a
  human. Every budget reports `UNMEASURED` until its probe exists, and the script
  fails, so a release cannot claim a number nobody measured.
- Lint configuration encoding the project rules: no panicking paths, no ad-hoc
  terminal output, `forbid(unsafe_code)`, function-length and complexity limits.
- `CLAUDE.md` — the rules a gate cannot check, with `AGENTS.md` symlinked to it
  so every agent tool reads one file.
- Agent skills for the procedures a rules file cannot carry: running and
  extending the gate, adding a dependency, and staying clean-room. Written once
  under `.claude/skills/` and symlinked from `.agents/skills/`, so Claude Code
  and Codex read the same text.
- `.codex/config.toml` — how Codex should run here. `network_access` is on
  because cargo cannot resolve crates.io without it.
- `.github/`: a CI workflow running `scripts/check.sh` and `scripts/bench.sh`
  — the second uploading its JSON as a build artifact, so a budget trend exists
  from the first pull request — plus a tag-triggered release workflow,
  Dependabot for cargo and actions, and issue and pull request templates.
- Contributor Covenant 3.0 code of conduct, contribution guide, and a documented
  release procedure.
- `crucible_tui::Title` — sets the terminal tab title to `▽ crucible` and
  restores the terminal when dropped.

[Unreleased]: https://github.com/augments-labs/crucible-code/commits/main
