# Changelog

Notable changes to crucible. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **`0.0.x` is unstable.** Configuration files, session files and the
> command-line surface may change in any `0.0.x` release with no deprecation
> period. Nothing in this line carries a compatibility guarantee.

## [Unreleased]

## [0.0.3] - 2026-08-09

Configuration files. Everything crucible could only be told on the command line
or through the environment can now be written down.

### Added

- **Configuration in JSON**, read from three files, nearest to the work last:

  ```
  ~/.crucible/config.json          yours, everywhere
  .crucible/config.json            this project's, checked in
  .crucible/config.local.json      this project's, yours alone
  ```

  A scalar takes the nearest layer that set it; an object is merged key by key,
  so a project naming one provider leaves your other one alone. The command line
  is nearer than all three. Every file is optional and a machine with none of
  them behaves exactly as before.

  Three blocks: `providers`, keyed by provider name, each taking a `model` and
  an `apiKeyEnv`; `env`, the variables the commands crucible runs are given; and
  `output`, holding `color` and `toolDetail`. See
  [`docs/configuration.md`](docs/configuration.md).

- **`apiKeyEnv`**, which points a provider at a different environment variable —
  what a second key for the same vendor needs. It takes a variable *name*, and a
  key still has no path into a file crucible reads or writes.

- **A checked-in file may not set an arbitrary `env` variable.** Anything in
  `.crucible/config.json` reaches everyone who clones the repository, so a name
  that is not crucible's own is refused there and pointed at
  `.crucible/config.local.json` instead. crucible's own names — the
  `CRUCIBLE_CODE_` prefix — are allowed, because those are knobs this program
  declares rather than somewhere a secret could hide. The refusal is structural
  and there is no setting that turns it off.

- **A JSON schema**, at
  [`schema/crucible-code-schema.json`](schema/crucible-code-schema.json). Adding
  `"$schema": "https://www.schemastore.org/crucible-code-schema.json"` to a
  file gets completion, validation and a sentence about each key from your
  editor. It is generated from the same declaration the parser walks and a gate
  compares it against the checked-in copy, so an editor that accepts a document
  and a crucible that refuses it would have to disagree with itself.

- **Refusals written for somebody with the file open.** A rejected document
  names the file, the dotted path, the line and column, and what was accepted
  instead — and where a key appears more than once it gives no position rather
  than a plausible wrong one:

  ```
  crucible: /home/you/api/.crucible/config.json: output.colour is not a setting
  crucible has at line 3, column 5 — accepted here: color, toolDetail
  ```

### Changed

- **crucible keeps its own files in `~/.crucible/`** — the configuration file
  and the session logs together. Sessions used to live under
  `$XDG_DATA_HOME/crucible/sessions`, which means nothing on Windows and is the
  wrong place on macOS.

  **Nothing is moved for you.** A sessions directory already at the old path
  keeps being used, so `--continue` still finds the work you were in the middle
  of; the new location is taken only by a machine that has neither.
  [`docs/sessions.md`](docs/sessions.md) says how to move it by hand if you want
  it moved.

  `CRUCIBLE_CODE_HOME` relocates the whole directory, as an absolute path, and
  turns off looking at the old tree. Because it is read to *find* the
  configuration file, it is the one setting of crucible's own that a
  configuration file cannot carry — writing it in one is refused rather than
  accepted and quietly ignored.

- **`--model` is optional and takes a bare provider.** `crucible --model
  openai/` names the provider and leaves the model to `providers.openai.model`;
  `crucible` on its own does the same for Anthropic. With nothing configured
  either way the model is still `claude-sonnet-5`.

## [0.0.2] - 2026-08-09

The gate that runs against a published artifact rather than a build of this
tree, and the defect it found the first time it ran.

### Added

- `scripts/smoke.sh` — the release gate that could not run before there was a
  release. It takes the published tarball, checks it against the published
  checksum, and runs it in a sandbox holding the binary, the dynamic loader and
  the libraries the binary itself names: no shell, no toolchain, no certificate
  bundle, no source tree, and a home directory a moment old. What that proves is
  not that the libraries it needs are there, which binding them guarantees, but
  the half no other gate can see — that nothing else on the build machine was
  holding it up. It reports the glibc floor, refuses a run whose `--version`
  disagrees with `Cargo.toml`, and requires that a machine with no key be told
  which variable it wants rather than left with a blank screen. Wired into
  `RELEASING.md` on both sides of the tag.

### Fixed

- A run whose output is redirected no longer writes the terminal title.
  `crucible > log` and `crucible | tee` were both getting the OSC sequence that
  names a tab — once on the way in and once when the guard handed the title
  back — and neither is a title once something other than a terminal has read
  it; they are twenty-two bytes in the middle of somebody's file. Setting one
  now goes through the only constructor there is, and it asks standard output
  whether it is a terminal before writing anything, so a caller cannot aim a
  title at a pipe. `scripts/smoke.sh` fails a release whose redirected run
  writes any escape sequence at all, which is how this was found: in the
  published 0.0.1 artifact rather than in the source.

### Documented

- The released binary needs **glibc 2.34 or newer** and nothing else from the
  system — no certificate store, no runtime. Measured from the binary rather
  than assumed, ignoring weak symbols, which is the difference between a floor
  and a version number that retires distributions this runs on perfectly well.

### Internal

- The report that a session has stopped being recorded is now gated from the
  binary's own tests. A log that fails every write cannot be built from outside
  the runner — every public way in ends at a real file — so the case where the
  last turn is still queued when input ends had no test, and deleting the code
  that reports it would have gone unnoticed. `crucible-runner` gains a `proof`
  feature that only the binary's `[dev-dependencies]` turns on, so the seam is
  absent from a release build; a `compile_error!` behind the feature is what
  proved that rather than cargo's documented behaviour being taken on trust.

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
  `0700`, set on every start and every `--continue` rather than only at
  creation, because a transcript holds what was typed, what files were read and
  what commands printed — and a group-writable directory would let another
  account drop a log in for `--continue` to replay. A log torn mid-line by a
  crash costs that line, and the torn bytes are dropped from the file before the
  continued session appends to it; one damaged in the middle is refused outright
  rather than silently returning a session with a hole in it.
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
- A sessions directory or log left at anything other than `0700`/`0600` is set
  to it on start and on `--continue`, and a filesystem that refuses fails the
  run rather than writing a transcript somewhere the whole machine can read. One
  already at the right mode is not touched, so this costs nothing on the
  ordinary path and leaves a sticky bit where it was.
- Linux x86-64 only. The release builds one artifact.

[Unreleased]: https://github.com/augments-labs/crucible-code/compare/v0.0.3...HEAD
[0.0.3]: https://github.com/augments-labs/crucible-code/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/augments-labs/crucible-code/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/augments-labs/crucible-code/releases/tag/v0.0.1
