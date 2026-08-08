# Changelog

Notable changes to crucible. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **`0.0.x` is unstable.** Configuration files, session files and the
> command-line surface may change in any `0.0.x` release with no deprecation
> period. Nothing in this line carries a compatibility guarantee.

## [0.0.1] - 2026-08-08

The first release: a coding agent you can hold a session with, and the gates
that say what it is allowed to become.

### Added

- A session that runs. `crucible` reads a prompt, streams the model's answer
  inline, runs tools, and asks before anything that changes a file or starts a
  process. `--continue` carries on the most recent session started in the
  current directory. An answer the provider cut short says so under the turn,
  with the token ceiling, the content filter and a paused turn named apart
  because the remedy differs for each. The bound on that: a stop reason this
  build has not heard of reads as an ordinary finish, so a vendor adding one is
  the case where a cut-short answer can still arrive looking complete.
- A startup that fails leaves no session behind. Everything that can fail on the
  way in runs before the session is started, so a wrong `--model` or an unset
  key writes nothing: an empty session would otherwise be the newest one for the
  directory, and `--continue` would offer it instead of the last real session.
- Two providers, chosen by `--model [provider/]model`: `anthropic` (the default
  for an unqualified name, keyed by `ANTHROPIC_API_KEY`) and `openai` (keyed by
  `OPENAI_API_KEY`). Authentication is a separate axis from the wire protocol —
  a provider is handed a resolved credential and never learns which kind it was.
- Six tools: `read`, `grep`, `glob`, `edit`, `write`, `bash`. Every one of them
  takes a permission token that only a verdict can mint, so code that has not
  obtained one cannot call the operation; a read mints its own, and a file
  change or a command asks first. `always` remembers the tool for a file change
  and the tool *and program* for a command, and is never written to disk. What a
  tool returns is bounded, and a result that is short says so in the result
  itself — more lines follow, a line was cut at a width, a listing stopped at
  its limit, output was still arriving, the command was stopped for running too
  long — because a silently trimmed result reads to the model as a complete one.
- Session log: one JSON object per line, one file per session, under
  `$XDG_DATA_HOME/crucible/sessions`. A log from a build with a different format
  is refused rather than half-understood. The log is `0600` and its directory
  `0700`, narrowed on every start rather than only at creation, because a
  transcript holds what was typed, what files were read and what commands
  printed — and a group-writable directory would let another account drop a log
  in for `--continue` to replay. A log torn mid-line by a crash costs that line;
  one damaged in the middle is refused outright rather than silently returning a
  session with a hole in it.
- `docs/` — getting started, providers and models, permission, and sessions.
- Cargo workspace: `crucible-core`, `crucible-provider`, `crucible-tools`,
  `crucible-runner`, `crucible-tui`, and the `crucible` binary. Dependencies
  point down only, enforced by cargo.
- `scripts/check.sh` — formatting, clippy with `-D warnings`, tests, a
  400-line-per-file cap, pinning checks for dependencies, GitHub Actions and the
  agent instruction files, a comment above every dependency saying why it is
  needed, and a check that CI stops excusing a failing budget once the first
  bench probe exists. CI runs the same script.
- `scripts/bench.sh` — one probe per performance budget, selectable by mode
  (`startup`, `mem`, `grep`, `stream`, or all of them). Writes a JSON document
  to stdout and a readable summary to stderr, so one run serves a pipeline and a
  human. Every budget reports `UNMEASURED` until its probe exists, and the script
  fails, so a release cannot claim a number nobody measured.
- Lint configuration encoding the project rules: no panicking paths, no ad-hoc
  terminal output, `forbid(unsafe_code)`, function-length and complexity limits.
- `CLAUDE.md` — the rules a gate cannot check, with `AGENTS.md` symlinked to it
  so every agent tool reads one file.
- `.claude/rules/` — one file per crate carrying the obligations that bind only
  inside it, scoped by `paths:` frontmatter so each is read when a file it
  claims is opened. They state what a change must do rather than restating what
  the module documentation already explains, which is what keeps them from
  becoming a second copy. `scripts/check.sh` fails a rule with no frontmatter or
  one aimed at a directory that no longer exists — either way nothing loads it,
  and it fails by staying silent.
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
- `deny.toml` and a weekly `audit` workflow — the other half of pinning. Nothing
  here moves on its own, so an advisory published against a version already
  pinned would never surface; a scan on a clock finds it, and the same scan
  refuses a licence that would make the MIT on the binary untrue or a dependency
  from anywhere but crates.io. It runs apart from `scripts/check.sh` because its
  answer changes when somebody else publishes rather than when you edit.
- Contributor Covenant 3.0 code of conduct, contribution guide, and a documented
  release procedure.
- `crucible_tui::Title` — sets the terminal tab title to `▽ crucible` and
  restores the terminal when dropped.

### Known limits

- A window resized mid-turn is noticed at the next prompt, not as it happens.
  Catching the signal a resize sends needs `unsafe`, which this workspace
  forbids, so what a resize costs is the turn it lands in.
- <kbd>Ctrl-C</kbd> ends the process rather than the turn, for the same reason.
  The session log is written as the turn goes, so `--continue` picks it up.
- Path containment resolves a path and the tool then acts on it, which is two
  steps rather than one. A path swapped for a symbolic link in between would be
  followed, so the check bounds a model working in the tree and not an attacker
  who can already write to it concurrently.
- A provider that pauses a turn is reported and left there. Sending the
  transcript back to carry on is a decision for the user, not something 0.0.x
  does by itself.
- A sessions directory or log left more open than `0700`/`0600` is narrowed on
  start, and a filesystem that refuses to narrow it fails the start rather than
  writing a transcript somewhere the whole machine can read. One already at the
  right mode is not touched, so this costs nothing on the ordinary path.
- Linux x86-64 only. The release builds one artifact.

[0.0.1]: https://github.com/augments-labs/crucible-code/releases/tag/v0.0.1
