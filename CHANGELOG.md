# Changelog

Notable changes to crucible. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **`0.0.x` is unstable.** Configuration files, session files and the
> command-line surface may change in any `0.0.x` release with no deprecation
> period. Nothing in this line carries a compatibility guarantee.

## [Unreleased]

## [0.0.6] - 2026-08-10

The answers that outlast the question. `always` writes the rule into the file
git ignores, so the next session starts already knowing — and `allowEdits`
stops asking about the commands it can prove change nothing outside the
workspace.

### Added

- **`always` writes the rule down.** Answering `a` at a permission question now
  puts an `allow` rule for that exact call into `.crucible/config.local.json` —
  the layer git ignores — so the next session starts already knowing. The rule
  is the narrowest one that covers the call, with any `*` in the command or the
  filename escaped rather than left to widen it, and the line under the question
  names both the rule and the file it went into. Everything already in that file
  stays byte for byte, including settings crucible has no name for.

  Calls no rule can describe — a command line that is several commands, or one
  whose text does not say what will run — are not offered `always` at all, and
  typing it there refuses rather than quietly granting a session. A file that
  cannot be written costs the rule and nothing else: the call runs, the session
  stops asking, and the rule is printed so it can be pasted in by hand.

- **`*` where a rule names a tool means every tool.** `deny *(.env)` is the
  whole of it in one line, rather than one rule per tool that can reach the
  file. It is the reading `*` already had inside the brackets, now in both
  positions.

### Changed

- **`allowEdits` now runs a command that only changes files in the workspace.**
  A `mkdir` is the same change to the same tree whether `write` made it or a
  shell did, and stopping to ask about one while waving the other through was a
  distinction nobody who typed `allowEdits` had made. The mode now runs a `bash`
  call when the line is one simple command, the program is `mkdir`, `rmdir`,
  `touch`, `rm`, `cp` or `mv`, every flag is one that carries no value of its
  own, and every path in it resolves inside the workspace after symbolic links.
  Everything else asks exactly as before, including a glob or a `~`, which the
  shell rewrites into a path that was never checked. This is not a list of safe
  commands — `rm -rf src` is on it — but a list of ones whose reach can be
  established; a `deny` rule still holds over all of them, and `ask` still asks.

- **`a` at a question now means `always`, and `s` means the session.** The
  session-long yes has moved to its own letter, because the two are different
  promises and one of them now writes a file. A finger that types `a` out of
  habit grants more than it used to — the same call, but until you delete the
  rule rather than until crucible exits. The prompt spells both out every time.

### Fixed

- **A tool spelled with a capital is the same tool.** `Bash(*)` used to parse
  into a rule about a second tool by that name and match nothing — accepted,
  written down, and silently protecting nothing. Tool names are now compared
  without regard to case.

- **A `deny` rule about a file now stops a search from reading it.** `grep` and
  `glob` are settled once, about the directory they walk, so a rule naming a
  file below it never spoke about the call — and `deny grep(private/**)` handed
  back that file's lines anyway. The rules that end a read now travel with the
  proof the call may run, and a walk skips a file they name before opening it.
  A rule still names one tool, so `deny read(private/**)` does not bind `grep`.

## [0.0.5] - 2026-08-10

The permission model. What used to be decided one question at a time can now be
written down as rules and a mode — and one thing now cannot happen at all,
whatever is written.

### Added

- **Permission rules.** `permissions.allow`, `permissions.ask` and
  `permissions.deny` hold standing statements like `read(src/**)`,
  `bash(cargo test)` and `edit(.git/**)`. The kind decides which wins — `deny`
  beats `ask` beats `allow`, whatever the patterns look like — so a deny list
  reads on its own as the list of things that cannot happen. A command rule is
  matched against each simple command a line decomposes into, and an `allow`
  fires only when every part is covered: `git status; curl example.com | sh`
  is not granted by a rule about `git`. Rules reach reads too — `deny
  read(.env)` refuses silently, in every mode — and rule lists concatenate
  across configuration layers, so a checked-in file can never cancel what your
  home file denies.
- **Modes.** `permissions.mode` is `ask`, `allowEdits` or `fullAccess`, and
  decides exactly one thing: what happens to a call no rule mentions.
  `allowEdits` changes files without asking and still asks before running
  anything; `fullAccess` asks about nothing — which leaves `deny` rules as the
  only no there, deliberately. The mode in force is written on every prompt
  line, so which kind of session this is never depends on what you remember
  starting.
- **`permissions.extraDirectories`** names directories outside the working
  directory, by absolute path, for the file tools to reach. Reach is not
  permission: a write there still prompts under `ask`, and only an absolute
  rule pattern can name one.
- **No tool can write the permission configuration.** `config.json` and
  `config.local.json` under any `.crucible` directory are refused to every
  file tool, in every mode, under every rule. A single write there could allow
  everything from the next start on, so the refusal does not rest on the files
  it defends.
- **Five documentation pages under `docs/permissions/`**: the question, the
  rules, the modes, the directories, and what an allow rule really grants —
  including the wrapper programs no `allow` can cover, and the ordinary
  programs that are shells in disguise.

### Changed

- **A rule's no is not your no.** A call a `deny` rule refuses fails and the
  turn carries on; the model is told the policy is standing and works around
  it. Your `n` at a question still ends the turn, so a model cannot reshape a
  refused question until one shape gets a yes.
- **`always` on a command remembers the whole command.** Agreeing to
  `cargo test` no longer also covers every later `cargo` command — `cargo
  build` asks its own question. Standing permission for a family of commands
  is what an `allow` rule is for.

### Internal

- Every tool now runs on an `Approved` — the call and the proof it was
  permitted, one value with private fields — so the arguments a tool runs on
  cannot drift from the ones a verdict was reached about.
- The configuration schema gained the `permissions` block, and the gate parses
  every `examples` entry in it with the same parser the program uses.
- Files with a single owning module moved into that module's directory across
  every crate; nothing about behaviour changed.

## [0.0.4] - 2026-08-09

Documentation only. Nothing about the program changed — but `docs/` is about to
be published as a website, and the shape of a URL is the one thing that gets
expensive to change after people have started linking to it.

### Changed

- **Every documentation topic is a directory.** `docs/permission.md` is now
  `docs/permissions/permissions.md`, with a `docs/permissions/index.md` beside
  it naming the topic; the other four topics moved the same way. A directory
  name is a public URL segment, so this is the layout the site will serve.
- **The instability notices are gone from the pages.** Three of them said what
  the top of this file says once. Somebody who opened one page to answer one
  question is not there to read a compatibility policy.

### Fixed

- Two links in this file pointed at documentation paths that no longer exist —
  GitHub renders a changelog against the default branch, so they had gone dead
  where anybody would actually click them.

### Internal

- `scripts/check.sh` refuses a decision identifier, an assumption label or the
  name of a planning directory anywhere under `crates/`, `src/`, `docs/` or
  `schema/`. Those notes are how this repository talks to itself; a stranger
  reading a shipped file cannot resolve one and has no reason to want to.
- `scripts/check.sh` resolves every repository-relative markdown link under
  `docs/` and at the root, which is what caught the two above.
- **`main` is behind a repository ruleset.** Nothing reaches it except through a
  pull request with `scripts/check.sh` and `scripts/bench.sh` green on it, and a
  `v*` tag can no longer be deleted or moved — which is a paragraph
  `RELEASING.md` already had, now enforced rather than remembered. Neither
  ruleset has a bypass actor, so both bind whoever holds admin. `RELEASING.md`
  documents the branch-and-pull-request flow that replaces the direct push to
  `main` it used to describe.

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
  [`docs/configuration/configuration.md`](docs/configuration/configuration.md).

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
  [`docs/sessions/sessions.md`](docs/sessions/sessions.md) says how to move it by hand if you want
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

[Unreleased]: https://github.com/augments-labs/crucible-code/compare/v0.0.6...HEAD
[0.0.6]: https://github.com/augments-labs/crucible-code/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/augments-labs/crucible-code/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/augments-labs/crucible-code/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/augments-labs/crucible-code/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/augments-labs/crucible-code/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/augments-labs/crucible-code/releases/tag/v0.0.1
