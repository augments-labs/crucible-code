# Changelog

Notable changes to crucible. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **The transcript is a column of blocks with a blank row between them.** What
  was asked, what was answered, each call and the line hanging under it: one
  block each, parted by a row of nothing, and the prompt box stands off the last
  of them the same way. A result still hangs directly under the call it answers,
  because the two are one block. What it costs: a row per block of the window
  and of a redirected run's output, and a session piped to a file now has blank
  lines in it where before it had none.

## [0.2.0] - 2026-08-17

### Added

- **A running turn says so, says how long it has been running, and says what it
  has cost.** One row between the answer and the box — a mark that turns, the
  word for what is being waited on (`thinking`, `writing`, `running`,
  `interrupting`), a clock counting from the moment the prompt was sent, and
  `↓` with the tokens the model has produced so far, added up across the turn
  and written the way it would be said — `840`, `1k`, `1.4k`, `128.4k`.
  A still screen and a hung one no longer look alike, and a turn that is getting
  expensive says so while there is still time to stop it. What it costs: three
  rows of the window while a turn runs, given up by the turn's own output, and
  dropped whole on a window with no room for them. The count appears only once
  the provider has reported one, so the first response of a turn shows none.

### Changed

- **Esc stops a running turn, and Ctrl-C belongs to the prompt in both loops.**
  Ctrl-C throws away the line typed so far and, against an empty box, offers to
  leave on a second press within two seconds — the same while a turn runs as
  between turns, rather than a key that had to be relearned at the moment there
  was something to lose. What it costs: Ctrl-C no longer stops a turn, so
  reaching for it out of habit clears the line instead, and twice against an
  empty one ends the session. The row under the box now reads
  `(enter queues it · esc to interrupt)`.

- **A tool call reads as the tool and what the call is about.**
  `● Read(src/main.rs)` rather than `read {"path":"src/main.rs"}` — each tool
  names the one argument a person would recognise its call by, and the result
  hangs under `└` off the mark that opened the call. Both marks, the `✗` on a
  failure and the `…` that says a line was cut come out of the `glyphs`
  setting, so `ascii` draws the pair as `*` and `+`. What it costs: any other
  argument is no longer on the row, and a call nobody could read is drawn as
  the bare tool name.

- **A call's line is written when its tool answers.** The line and the result
  hanging under it reach your scrollback one after the other, so nothing the
  turn did in between can come to stand between the two. What it costs: nothing
  about a call is written until it has finished, so a slow tool leaves the
  transcript still while it runs — the row above the box is what says a call is
  out.

- **A call stands above the box while its tool is out.** Its line waits there
  with the mark pulsing and lands in the transcript the moment the tool answers
  — the same words in the same columns, with the motion gone — so a call still
  running is told from one that has finished without reading the clock. What it
  costs: two more rows of the window while a tool is out, given up before the
  working row on a window too short for both.

- **The mark between the halves of a line comes out of the `glyphs` setting.**
  What `/clear`, `/resume` and `/model` stand between the two things a row says
  is `·` where the terminal draws it and `-` under `ascii`, rather than a
  character chosen once and left. What it costs: those rows read differently
  under `ascii` than they did.

- **The listing a run with no keyboard gets is drawn out of the same setting.**
  What `/login`, `/logout` and `/model` put between the line to type and what
  typing it reaches is `—` where the terminal draws it and `--` under `ascii`.
  A piped run is the one most likely to have the setting turned down, and it
  was the one being handed a character its terminal had no glyph for.

- **The sign-in draws every mark out of the `glyphs` setting.** The box a key is
  pasted into stands one `•` per character where `ascii` now stands a `*`, the
  sign-in's own paste box takes the prompt's `›` and hides behind the same mark,
  and the `—` between an account's plan and what taking that row does is `--`
  under `ascii` — as is the one on the row that says a sign-in is waiting. These
  are the rows somebody reads while handing over a credential, and they were the
  ones still drawn from characters chosen once and left. What it costs: those
  rows read differently under `ascii` than they did.

- **The effort ladder names its keys out of the same setting.** The row under
  the track reads `←/→ to adjust · enter to confirm · esc to cancel` where the
  terminal draws it and `</> to adjust - enter to confirm - esc to cancel` under
  `ascii`, and the mark between `Effort` and the model above it goes the same
  way. That row is the whole of what says which keys work at a ladder, so it was
  the worst one to be drawing two hollow squares on. What it costs: it reads
  differently under `ascii` than it did.

- **The last four marks drawn outside the setting now come out of it.** The `›`
  a permission answer is typed after, the `›` a piped run's prompt is typed
  after, the `·` in `(enter queues it · esc to interrupt)` under the box, and
  the `·` between a provider and where its key came from on the opening screen
  are `>`, `>`, `-` and `-` under `ascii`. Two of those are the prompt itself,
  on the rows a session stops at until somebody answers. Every mark crucible
  draws now comes out of the same set as the border, and
  `docs/configuration/configuration.md` lists them. What it costs: those rows
  read differently under `ascii` than they did.

### Removed

- **A release no longer carries `budgets.json` or `budget-environment.txt`.**
  Those are shared-runner trend numbers rather than the quiet-machine reading a
  release is decided on, and they stay on the workflow run that produced them,
  where that is plain. The bench still runs on the release path and a probe
  over its limit still stops the tag. What it costs: anything fetching either
  file by name from a release finds nothing. The archives, `install.sh`,
  `uninstall.sh` and `SHA256SUMS` are unchanged.

## [0.1.13] - 2026-08-16

### Added

- **`edit` can make several changes to a file in one call.** `edits` takes a
  list of `find`/`replace`/`all`, made in order, each looking at what the one
  before it left — ten changes to one file were ten turns. If any of them
  cannot be made, none is, and the answer says which one stopped the call.
  What it costs: nothing to a call that sends `find` and `replace` as before,
  and sending both shapes at once is now refused rather than half-read.
- **`grep` can return the lines around each match.** `context: n` gives up to
  twenty either side, the way `grep -C` does, so reading what surrounds a hit no
  longer costs a whole file through `read`. Context lines carry dashes where a
  match carries colons, and `limit` still counts matches only. What it costs:
  nothing to a call that does not ask for it, and `mode: "files"` ignores the
  argument since a list of names has nowhere to put a line.
- **`grep` can search for text rather than an expression.** `fixed: true` reads
  `pattern` as the exact characters to find, so anything copied out of a file
  goes straight into the call. `[dependencies]` is a character class to an
  expression and `unwrap_or(` is not an expression at all; escaping either by
  hand costs a turn whichever way it goes wrong. What it costs: nothing to a
  call that does not ask for it.

## [0.1.12] - 2026-08-16

### Changed

- **`/clear` starts a new session instead of emptying the one in hand.** The
  session you were in is finished and stays on `/resume`'s list, so what was
  said in it can be picked up whole rather than coming back with a hole where
  the clear was. What was allowed for the rest of the session goes with it, and
  so does the record of what has been read, since both belonged to the session
  left behind. What it costs: a `/clear` leaves a second log in the session
  directory rather than none, a call allowed for the rest of the session is
  asked about again after one, and so is a `write` to a file already read. No
  session log holds a line saying it forgot any more; one written by an earlier
  crucible still replays from that line rather than from the top.

- **`/resume` empties the record of what has been read.** The files remembered
  were read in the session being left, and the one picked up saw none of them,
  so `write` asks for the read again rather than replacing a file on another
  session's word. What it costs: the first `write` to a file after a `/resume`
  spends one `read` call, even if the session picked up is the one that read it.

## [0.1.11] - 2026-08-16

### Added

- **`grep` can answer with the names of the matching files.** `mode: "files"`
  names each file holding a match once and stops reading that file there, so
  asking where something lives no longer costs a wall of lines — and is faster
  than the search that returns them. What it costs: in that mode `limit` bounds
  files rather than matching lines, and a `mode` outside the two words is
  refused rather than quietly read as the default.

- **`glob` can answer newest first.** `sort: "modified"` orders the listing by
  modification time instead of by path, which is what finds the files a project
  has been working on rather than the ones whose names sort first. The order
  also decides which paths a `limit` keeps, so a capped call returns the newest
  matches rather than the lowest ones. What it costs: that mode reads each
  matching file's modification time, which the default does not, and a `sort`
  outside the two words is refused rather than quietly read as the default.

### Changed

- **`write` refuses to overwrite a file the session has not read.** A file the
  agent has read, or wrote itself, it may still replace; anything else comes
  back as a failure telling it to read the file first, and the turn continues.
  What it costs: a model that used to guess a path and replace whatever was
  there now spends one `read` call first, and a file another program wrote
  during the session is no longer silently discarded.

### Documentation

- **The tools have a topic of their own.** `docs/tools/` says what all six hold
  themselves to — the workspace boundary, the 30000-byte answer and what each
  tool prints when it reaches it, and why a failed tool is not a failed turn —
  then covers the three that name a file, including the read `write` now
  requires before it will replace one. Until now that surface was a six-row
  table in getting started and nothing else.

- **The topic covers the two tree walkers and the shell.** What `grep` and
  `glob` both skip and why they can never disagree about it, the two search
  modes and the two listing orders including which paths a `limit` keeps under
  each, and for `bash` the built environment a command starts with, the notes
  that say how one ended, and why it is asked about in every mode but
  `fullAccess`.

## [0.1.10] - 2026-08-16

### Internal

- **Routine dependency updates.** ureq 3.3.0 to 3.4.0, thiserror 2.0.19 to
  2.0.20 and clap 4.6.5 to 4.6.6, each through the usual gates including the
  advisory scan. No behavior changes.

## [0.1.9] - 2026-08-16

### Changed

- **A run with several usable credentials opens with nothing selected.**
  Exported keys, stored API keys and account logins all count, and where
  nothing chose between them crucible used to refuse to start; it now opens
  with no provider, model or effort preselected and says `Warning: No provider
  selected. Use /model to select a provider and model.` What it costs: a
  machine that was previously stopped by the error is now one `/model` away
  from a turn, and a fresh machine opens asking what to use rather than
  guessing. A remembered provider whose credential is gone opens dormant
  instead of falling through to another vendor's key.

- **The opening names the active credential source under the card.** A quiet
  row says which non-secret source signs requests — an environment variable
  named, a stored API key, or a stored account login — so an inherited
  environment key never reads as a stored login `/logout` could remove.

- **A stored subscription counts as an available credential at startup.** A
  machine whose only authentication is a ChatGPT or Kimi Code account login
  opens on that provider rather than being told nothing is set up.

- **Startup makes crucible's home owner-only before reading it.** The
  configuration's permission bits are tightened where they are wider; its
  contents are never written at startup.

- **Typed-ahead prompts and redirected input are bounded.** Up to 64 finished
  prompts and 1 MiB of their text wait behind a running turn; past either
  bound, Enter leaves the line in the box and the row beneath it says why. A
  redirected line past 1 MiB is refused with an error instead of being
  retained.

- **`/login` now authorizes subscription accounts as well as API keys.**
  ChatGPT offers browser PKCE with a paste-back fallback and device-code
  login; Kimi Code offers device-code login. Both write renewable credentials
  to the protected store, and a stored account login outranks an environment
  key inherited from the shell. Logging in no longer chooses a model for the
  session — `/model` remains the explicit choice. Anthropic stays
  API-key-only.

- **`/logout` removes stored credentials without claiming the environment.**
  It names an inherited variable as what keeps a provider authenticated and
  says the shell is where that one is unset; removing the active provider's
  last stored credential signs the session out instead of leaving it looking
  configured.

### Internal

- **Turn events and the release cache are bounded.** The channel a turn
  reports on holds two events and adjacent deltas already waiting are drawn as
  one batch, so a provider that outruns a slow terminal meets backpressure
  instead of growing process memory; a terminal failure now also cancels the
  in-flight provider. The cached release answer is read under a 128-byte
  ceiling and replaced atomically through an exclusively-created sibling.

### Documentation

- **The docs and rules files caught up with account login and the wider
  `/model` panel.** `/login`'s ChatGPT and Kimi Code routes were still
  described as listed but not connected, `/model` as offering only the in-force
  provider's models, and `/effort` as always offering all five rungs; the
  panel itself said the choice was written down for this workspace when it goes
  to the user configuration and holds for every run. Each now reads the way the
  shipped commands behave.

## [0.1.8] - 2026-08-15

### Changed

- **Session discovery uses a private fixed-size index.** The first frame no
  longer enumerates the session directory, so startup stays flat as recordings
  accumulate. A directory of logs written before this release is indexed once,
  after the first frame of the first run that starts or continues a session —
  until then its welcome list is empty; no log is moved or rewritten.

- **The benchmark probes measure the shipped binary through a real terminal.**
  The startup probes spawn `crucible` behind a controlling pseudo terminal,
  the RSS budget runs a twenty-turn session against a loopback provider, and
  the render burst drains a bounded kernel pipe — so the budgets now cover
  escape assembly, raw input and process memory rather than in-process
  stand-ins. The stream budget also requires the sustained rate to hold at
  least half the opening rate.

## [0.1.7] - 2026-08-15

### Added

- **Releases ship an installer and an uninstaller.** `scripts/install.sh`
  verifies the published checksum before replacing an existing binary, and
  `scripts/uninstall.sh` preserves configuration and sessions unless
  explicitly asked to purge them.

### Changed

- **Release builds are pinned, smoked and attested before publication.** Linux
  artifacts now build in a digest-pinned CentOS Stream 9 image, making the
  documented glibc 2.34 floor a build input rather than a property of
  whichever runner image is current. rustup is bootstrapped from
  a versioned, checksummed installer, the built archive passes the smoke gate
  pre-publish, and the release attests its binaries and ships `install.sh`,
  `uninstall.sh` and `budgets.json` as assets.

- **Terminal paste input is bounded before parsing.** Bracketed-paste reporting
  is disabled; immediately ready plain characters are inserted and redrawn as
  one bounded run, while embedded newlines submit just as if they were typed.
  A prompt retains at most 1 MiB of text, and what would cross that is refused
  with a note under the box rather than flooding the input path.

- **The welcome card no longer names a model.** Provider, model and effort stay
  on the live prompt status, where `/login`, `/model` and `/effort` can update
  them instead of leaving stale selection details in terminal scrollback.

- **`/model` offers every provider's models, under their product names.** The
  panel and the piped listing hold all three providers rather than only the one
  in force, and taking a row of another provider moves the session to it.
  MoonshotAI's models read K3, K3-256k, K2.7 Coding and K2.7 Coding Highspeed
  beside the wire identifiers a configuration carries, and the Kimi models take
  low, high and max — the only rungs `/effort` offers for them.

### Internal

- **Subscription logins are registered at the wiring boundary.** The binary's
  one closed list pairs each account login with the fixed audience its tokens
  are issued for, and a stored subscription now resolves to that address when
  a run names its provider and no key is set; nothing at the prompt writes one
  yet.

- **The smoke gate verifies checksums exactly and enforces the glibc floor.**
  `--offline` is renamed to `--no-provider`, a local tarball can be checked
  against `--checksum HEX`, a published artifact must match exactly one
  SHA256SUMS line, and requiring more than glibc 2.34 fails the release.

- **Every crate in the workspace is marked unpublished.** `publish = false` in
  each manifest turns an accidental `cargo publish` into an error; releases
  ship as tag-pushed GitHub Releases and nowhere else.

- **The OpenAI provider names its subscription endpoint.** The fixed address a
  `ChatGPT` subscription credential is served at sits beside the API-key one;
  which of them a credential uses stays with the wiring that hands it over.

- **OpenAI account login joins Kimi at the provider-neutral boundary.**
  `ChatGPT` sign-in offers browser PKCE with a loopback callback and a
  paste-back fallback, plus device authorization for headless terminals;
  nothing in the binary calls it yet.

- **The auth crate has its first account-login implementation.** Kimi's device
  authorization flow logs in through a browser and renews through the protected
  store; nothing in the binary calls it yet.

- **Account login has one provider-neutral boundary.** Login methods expose
  bounded, cancellable updates without making the TUI or credential store know
  which authorization protocol a provider uses.

- **The protected credential store is provider-neutral.** Its versioned,
  bounded document can retain API keys and renewable account credentials
  without exposing either secret kind through parsing or diagnostics.

- **Providers share one process-wide connection pool.** Replacing a provider
  keeps established HTTP connections instead of constructing another pool.

- **Provider transports own one serialized request.** Headers and body can move
  into cancellable setup without cloning transcript-sized data.

- **Credentials carry opaque response-redaction material.** Provider adapters
  can remove an applied secret from untrusted diagnostics without gaining an
  accessor to the credential itself.

### Fixed

- **Switching models drops the rung chosen for the previous one.** A rung is a
  property of one model's ladder, so `/model` now lifts
  `providers.<provider>.effort` out of the file when it writes a new model
  rather than letting the old rung bind the new one; a file it cannot be
  lifted out of without rewriting is left alone, with a message saying what to
  remove by hand.

- **Release discovery has a ten-second absolute lifetime.** DNS, response
  headers and a body that starts but stalls can no longer retain its detached
  startup worker indefinitely.

- **Provider DNS retains no unbounded request body.** Resolution has a
  five-second deadline; if the platform lookup outlives it, later provider
  requests fail fast until restart instead of accumulating resolver threads.

- **Cancelling interrupts model-request setup.** DNS, connection, TLS and
  response-header waits retain at most one process-wide worker and request body,
  which replacements must reap before sending another request.

- **HTTP failures no longer repeat configured request URLs.** Diagnostics keep
  the failure kind without exposing a query or path that may carry a secret.

- **Provider refusals cannot echo credentials.** Applied secrets are removed
  from transport and HTTP failure diagnostics, and cancelling interrupts a
  refusal body that is still arriving.

- **Provider stream errors cannot echo credentials.** The same request-bound
  filter now covers framing and vendor error events inside successful HTTP
  responses.

- **Grep retains one global match bound.** Parallel workers add directly to the
  ordered result instead of keeping an additional batch per worker.

- **Tool results share one 4 MiB turn boundary.** A result that would cross it
  is replaced with a bounded failure, later calls are still answered, and the
  turn stops instead of retaining unbounded tool output.

- **A turn has cumulative provider-work limits.** It stops after 32 provider
  responses, 128 tool calls, or 16 MiB of retained response data instead of
  allowing a hostile provider to keep the turn and process growing forever.

- **Provider responses are cumulatively bounded and stop is terminal.** One
  response may retain at most 8 MiB of text, 1 MiB of tool arguments, 128 tool
  calls and bounded call metadata; any delta after stop is a protocol failure.

- **Provider events have a tighter allocation bound.** One malformed or
  oversized server-sent event is refused after 1 MiB instead of being allowed
  to retain 8 MiB before cumulative response checks can run.

- **Moonshot requests avoid an intermediate JSON tree.** Transcript messages
  are written directly into the outbound body and failed-result prefixes no
  longer need a second allocation.

- **OpenAI requests avoid an intermediate JSON tree and honor their output
  ceiling.** Transcript items are written directly into the outbound body, and
  the generated-token limit is sent as `max_output_tokens`.

- **Anthropic requests use one outbound JSON allocation.** Transcript messages
  are written directly into the request body instead of first building an owned
  JSON tree, reducing peak memory as sessions grow.

- **Provider requests no longer clone the transcript.** The runner lends its
  transcript and cached tool schemas through the synchronous request boundary,
  removing one session-sized allocation from every provider pass.

- **Domain diagnostics no longer print transcript or tool contents.** `Debug`
  output for prompts, answers, tool calls, arguments and results keeps only
  structure and explicit redaction markers.

- **Slow refusal bodies now obey their ten-second whole-read deadline.** A peer
  that keeps trickling bytes can no longer hold the provider thread far beyond
  the advertised limit.

- **Authenticated model requests no longer follow redirects.** A 3xx response
  is handed back to the provider as a refusal, so it cannot choose a second
  recipient for an API key or another credential.

- **Provider addresses cannot disguise or print a credential-bearing target.**
  Endpoint parsing rejects user information and fragments, validates the exact
  host before permitting loopback HTTP, and redacts paths and queries in
  diagnostics.

- **Concurrent user-setting changes no longer overwrite one another.** A
  private lock now spans the bounded reread and atomic commit; a busy file is
  reported after five seconds instead of silently losing another process's
  update.

- **User configuration replacement is private and atomic.** Changes prepare an
  owner-only, exclusively created sibling and commit the complete document, so
  a planted temporary link cannot redirect a write or expose partial contents.

- **Configuration documents have a 1 MiB input limit.** An oversized user or
  workspace file now fails before JSON parsing instead of choosing an unbounded
  startup allocation.

- **Both workspace configuration filenames are non-authority layers.** They may
  narrow policy, but settings that allow calls, widen filesystem reach, select
  credentials or redirect requests are read only from the user configuration
  outside the checkout.

- **Permission questions offer once or session.** Durable `allow` rules now
  have to be written deliberately in the user configuration outside a
  checkout. The former `always` answer wrote authority into a conventional
  local filename that a repository could still commit.

- **Command cleanup is bounded and reaches descendants.** Output readers poll
  for cancellation, every failure path stops and reaps the command scope, Unix
  uses process groups, and Windows attaches a suspended child to a kill-on-close
  job before it can run. Narrow command rules also fail closed on interpreters,
  launchers, shell grammar and computed program names.

- **Edits are bounded, cancellable and atomic.** `edit` refuses non-regular,
  non-text, source or resulting files above 1 MiB, notices cancellation while
  reading, and commits the complete replacement without a truncated interval.

- **Whole-file writes commit atomically.** `write` prepares and flushes a
  private sibling before replacing the destination, preserves an existing
  mode, and refuses a destination whose file identity changed before commit.

- **Grep is globally bounded and cancellable inside every file.** Parallel
  workers share one ordered match bound, partial-file names are capped, and a
  no-match file now notices cancellation before reaching its end.

- **File reads have bounded input and output.** A huge line is consumed in
  fixed-size blocks, answers stay within 30 KiB, and cancellation interrupts
  scans through large offsets.

- **Workspace paths stay confined while the tree changes.** Unix operations
  descend through held descriptors, Windows validates the final opened handle,
  missing-parent proofs retain their intended leaf, and non-directory roots are
  refused before a tool can use them.

- **Private credential files on every supported platform.** The auth directory,
  store, partial write and lock are owner-only on Unix and Windows; existing
  permissions are tightened before a credential is read.

- **Credential-store input bounds.** A malformed key now refuses the complete
  store instead of silently disappearing, and stores above 64 KiB fail closed
  before they can choose startup memory.

## [0.1.6] - 2026-08-14

### Changed

- **The screen names the vendor before the model.** The welcome card and the row
  under the prompt box both read `provider/model` now — the shape `--model`
  takes back — because a model name says which model and never whose, and a
  machine holding keys for two vendors could not tell which one a turn was going
  to. The row is read off the provider the next turn would reach, so `/login`
  mid-session moves it.

## [0.1.5] - 2026-08-14

### Added

- **`provider` in the configuration says which provider to ask.** The one
  setting that chooses a vendor, written where `--model` is not naming one.
  `/model` and `/login` write it, so a machine holding a key for two vendors
  answers the question once rather than at every launch. It is refused in
  `.crucible/config.json`, the file a clone carries, for the reason
  `providers.<name>.baseUrl` is.

- **A session knows which model it is and how hard it was asked to think.**
  Both go into the prompt before every turn, so asking crucible what it is gets
  the model on the request rather than whatever the model was trained to say —
  and the answer keeps up with `/model` and `/effort` instead of describing the
  turn the session opened with.

### Changed

- **`providers.<name>.model` no longer decides which provider is asked.** It is
  what to ask that provider *for*, and reading it as a choice of provider sent a
  machine holding two keys to whichever vendor a model had been picked for
  earlier, with nothing on screen saying so. If that is how yours was being
  settled, set `provider` — the sentence a two-key machine gets now names it.

## [0.1.4] - 2026-08-14

### Removed

- **A run with no key no longer opens on the login panel.** 0.1.3 stood it in
  front of the first prompt; what a first run gets back is the screen every other
  run starts with — the welcome, the warning naming `/login` and `/model`, and
  the box. The warning was already the whole answer, and a panel in front of it
  asked a question before saying where you were.

### Changed

- **`/effort` offers the rungs the model in force serves, and no others.** The
  ladder was crucible's five whichever model was being asked, so a session on
  `claude-haiku-4-5` could walk to a rung its vendor has never served and read the
  refusal a keystroke later. Each model now carries what it takes: three rungs on
  MoonshotAI's K3, four on `gpt-5.5`, none at all on the two Kimi coding models
  and on `claude-haiku-4-5`, which are told so rather than offered a ladder. Only
  the offer narrows — `--effort` and `/effort <rung>` still go to the vendor, and
  a model this build has not heard of is offered all five.

- **MoonshotAI's models are named the way the console crucible asks spells them.**
  `/model` offered `kimi-k3` and `kimi-k2.7-code`, which are the open platform's
  names; the coding console is what crucible asks by default and it serves `k3`,
  `k3-256k`, `kimi-for-coding` and `kimi-for-coding-highspeed`. Picking one off
  the list reached a name that console does not serve. A key from the open
  platform sets `providers.moonshot.baseUrl` and types the longer name, as before.

- **What the next turn is asked of moved under the prompt box.** The model and the
  rung it is asked on sat on the welcome card, which is scrollback the moment the
  next thing is drawn — so `/model` and `/effort` changed what a session was doing
  and left the card saying what it used to do. The row under the box is redrawn
  every keystroke and now carries both at the end away from the mode. The card
  still names the model the session opened with.

- **`/login` asks how you pay, not which vendor.** Three ways: OpenAI's ChatGPT
  Plus, Pro, Business and Enterprise plans, MoonshotAI's Kimi Code, and a console
  account billed by API usage. The two plans are listed but not connected yet and
  say so when chosen; the console account is what works today and asks whose
  console before opening the box. Somebody paying for a plan and somebody holding
  a console key were never the same person, and one panel over vendor names asked
  them the same question.

- **A provider that fails a response without naming a reason says where to look.**
  `openai: error: the provider reported a failure and did not say what` was what
  a turn ended with, and there was nothing to do with it. A `"error": null` under
  the response was being read as an error rather than as the absence of one, so
  the status and the reason the response carried went unread; where there is
  genuinely nothing, all three providers now say to check that the model serves
  what was asked of it.

- **`/effort` draws a ladder instead of a panel.** One track with the five rungs
  written under it, `Faster` at one end and `Smarter` at the other, walked with
  the left and right arrows. The panel spent two rows on each rung under a
  three-row paragraph and came to twenty-four rows — the whole of an 80×24
  window, for five words. The ladder is nine. It also asks which model is being
  asked first, and sends a session with none to `/model`.

- **A panel left with escape writes one line, not its whole list.** `/login`,
  `/logout`, `/model` and `/effort` all fell through to their list of rows when a
  panel was escaped, so saying "not this" put three or five rows of it into the
  scrollback anyway. Escape is an answer now, and what it leaves is one line
  saying the question was dropped — `cancelled, no rung taken` and its like. A
  window with no room to stand a panel in still gets the rows, which is the one
  case they were for.

- **`/login` and `/logout` say what they do rather than what crucible stores.**
  "Sign in with your provider account" and "sign out from your provider account".
  The old rows named a key, which is one of the ways in and not the question the
  row is read to answer.

- **`/model` and `/effort` stop naming the file they wrote to.** It is the same
  file every time and crucible chose it, so a session reporting it on every model
  and every rung was reading its own bookkeeping out loud. The path still appears
  where the write *fails*, which is the case you have to know about.

## [0.1.3] - 2026-08-14

### Added

- **A run holding no key for any provider opens on the panel that takes one.**
  Before a first prompt is read, so a machine with nothing set up reaches a turn
  without knowing `/login` exists. Escape skips it and leaves exactly the session
  every other run starts with; a run reading a prompt down a pipe never sees it.

### Changed

- **A key given to `/login` is what the next turn is sent with.** The session is
  set up with that provider where it stands, rather than writing the key down for
  a run you had to start yourself, and the model and rung your configuration
  names for it arrive with the key wherever nothing has chosen one yet. What a
  flag or a panel already answered is left alone.

## [0.1.2] - 2026-08-14

### Added

- **`/effort` picks a rung mid-session.** On its own it stands a panel over the
  five, `/effort <rung>` takes one outright, and either way it applies from the
  next turn and is written down beside the model. All five are offered wherever
  you are: which of them a model serves is its vendor's answer, and a rung
  crucible filtered on your behalf would be a guess about somebody else's
  documentation.

- **`--effort` says how hard to think.** One of `low`, `medium`, `high`, `xhigh`
  or `max`, on every turn of the session, with `providers.<name>.effort` saying
  it once for a provider and the flag winning where both do. Left off, crucible
  asks for no rung at all and the vendor's own default for that model applies —
  which is what keeps a rung nobody chose from reaching a model that does not
  take the field.

## [0.1.1] - 2026-08-14

### Added

- **`/model` names a few models you could be asking.** It said which one was in
  force and left you to go and look up how the vendor spells the others. It now
  lists a handful of the ones your provider serves, written as the line that asks
  for each. Only that provider's, because a name goes to whichever vendor your
  key belongs to — and a model the list does not carry is still named the way it
  always was.

- **`/model` on its own opens a panel.** The same list, standing where the prompt
  box was, walked with the arrows and taken with Enter, under the name of the
  model being asked now. Escape leaves it and changes nothing. A run with no
  keyboard to walk it gets the rows instead, as before.

### Fixed

- **A crucible waiting its turn to write a key waits long enough.** Two of them
  logging in at once take the file one after the other, and the one at the back
  gave up after a second and told you to try again. The wait is now sized for the
  queue rather than for the single rename it stands for.

## [0.1.0] - 2026-08-14

The version line moves from `0.0.x` to `0.1.x`. What it promises is unchanged —
configuration, session files and the command line may still change in any release
before 1.0, with no deprecation period — and every sentence that said so now says
`0.x` rather than naming the series it happened to be written in.

### Added

- **`/login` exists, and takes a key.** A run with no key has always said to use
  it, and there was no such command. `/login <provider>` now opens a box that
  draws a dot per character — never the command line, which would leave the key
  in your shell's history and in the process listing — and writes what it takes
  down for the next run. `/login` on its own stands a panel of the providers
  there are where the prompt box was, to walk with the arrows; a run with no
  keyboard to walk it gets those names as rows, with the variable each reads
  from.

- **`/logout` takes a key back out.** The same panel over what is actually
  there — the providers a key was written down for — or `/logout <provider>` for
  one of them by name. It reaches `~/.crucible/auth.json` and nothing else: a key
  exported into your shell is untouched and goes on winning, which is what the
  line under the answer says rather than leaving you to remember it.

- **A key can be written down instead of exported.** crucible reads
  `~/.crucible/auth.json` — a file only you can read — alongside the environment,
  so a machine you would rather not keep a key on the shell profile of can still
  be set up once. An exported variable still wins, and a file that cannot be read
  is a sentence under the welcome rather than a run that ends.

- **MoonshotAI is a provider.** `MOONSHOT_API_KEY` and `--model moonshot/…`
  reach Kimi over Chat Completions, which crucible had no reader for until now.
  One thing to know before setting it up: MoonshotAI issues a key against
  either the Kimi Code Console or the Open Platform and refuses it at the other,
  and nothing in the key says which. crucible asks the coding console; an open
  platform key sets `providers.moonshot.baseUrl` to `https://api.moonshot.ai/v1`.

### Fixed

- **A word that ends exactly on the last column no longer drops to the next
  row.** Wrapped prose gave up its final column on every row where the last
  word landed on the edge, which reads as ragged text in a narrow terminal.

### Documentation

- **crucible will not log in with a vendor's chat subscription, and the docs now
  say why.** A plan is sold scoped to that vendor's own software — accounts have
  been closed for pointing another program at one — so the login is not a gap
  waiting to be filled. A plan a vendor publishes for other programs is an API
  key and a `baseUrl`, which already work.

### Internal

- **The 400-line ceiling now blocks, and blocks everywhere.** It was a CI job
  that was never in the ruleset's required list, so a pull request over it
  merged with a red mark beside the button; six did. It is required now, and the
  branch a pull request targets no longer exempts it — a collecting branch was
  measured only when it asked for `main`, which measured the collection and
  never the pieces somebody actually read.

- **`whole-module` is a second way past the ceiling, for a module that only
  compiles whole.** `-D warnings` makes an unreached function a failed build, so
  a new provider or tool arrives exported and working or it does not build; where
  that floor is already over 400 lines, the only smaller pull request is one that
  lands the code without its tests. The test is whether an intermediate pull
  request would compile, not whether the change is large. `moves-only` is
  unchanged, and both stay visible on the pull request. Adding either now
  re-measures on its own — `labeled` is not one of the events a pull request
  workflow listens to by default, so until now the documented remedy did
  nothing until the next push.

## [0.0.17] - 2026-08-13

### Added

- **`providers.<name>.baseUrl` points a provider at a gateway.** For a proxy or
  a gateway speaking the vendor's protocol. It must be `https`, or `http` on
  `localhost` — the key rides in a header on every request, so the address is
  who receives it. Refused in `.crucible/config.json` for that reason, since
  that file travels to everyone who clones the repository.

### Internal

- The whole-screen tests now drive a real turn. A provider on a loopback port
  serves a canned event stream, which `baseUrl` made reachable, so the case that
  streams an answer taller than the window is finally the case that catches the
  defect the suite was written for — proven by putting that defect back.

## [0.0.16] - 2026-08-13

### Fixed

- **A piped prompt nobody could answer no longer reports success.** With no
  model configured, `echo … | crucible` said so and exited 0, which is what a
  script reads as "it worked". It now exits non-zero. Interactively nothing
  changes: `/model` is a key away, so the warning still leaves the session
  running.

### Internal

- `grep` and `glob` are pinned to not follow a symbolic link out of the
  workspace, and `grep`'s module says what that holds and what it leaves. The
  property was already true — the walk does not follow links — but it rested on
  a default nothing tested, and what a search reads goes back to the model.

## [0.0.15] - 2026-08-13

### Fixed

- **A rule allowing one thing no longer quietly allows another.** A verdict was
  reached about a call and then applied more widely than the call it was reached
  about; it is now bound to the sensitivity it was asked about, so an allow for
  a read cannot stand in for an allow for a write.

### Internal

- The renderer is watched on a real screen. A test starts the shipped binary on
  a pseudo terminal, sends keystrokes and asserts on the screen it drew — the
  only test that sees the arithmetic turning rows into a screen, since every
  other one is handed rows and never a terminal. It is what would have caught
  the box scrolling off the bottom before a release did.

- **`cargo run` starts crucible.** The bench probes under `src/bin/` are targets
  too, so cargo could not tell which binary was meant and refused to run any.
  `default-run` names the one a person means.

### Fixed

- **A session no longer starts on a log another crucible already holds.** The
  name came from a timestamp, so two crucibles started in the same directory in
  the same second opened the same file and appended into each other's
  transcript — and `--continue` then replayed one conversation made of two. A
  log is now created exclusively: the one that loses the race takes the next
  name rather than joining what it found.

### Fixed

- **A <kbd>Ctrl-C</kbd> at the very start of a turn is no longer lost.** The
  flag was cleared by the turn, on the turn's own thread, which left a window as
  wide as a thread takes to start: a press raised in it was wiped by the turn it
  was meant to stop. It is cleared by the thread that reads the keyboard,
  before the turn's thread exists, so the only hand that can raise it is the one
  clearing it.

### Added

- **`/model <name>` chooses the model and writes the choice down.** It goes to
  `~/.crucible/config.json` under the provider serving it, so the next session
  starts on it without the command line saying so.

- **crucible says when there is a newer release.** A line under the welcome
  names the version and how to get it, and says nothing at all when you are up
  to date. The check runs at most once a day, on a thread of its own, and
  nothing waits for it — so the answer is drawn the *next* time you start.
  `updates.check` set to `never` stops it contacting GitHub entirely.

- **`output.mouse` decides who the mouse belongs to.** Left `off`, the terminal
  keeps it: the wheel scrolls scrollback, dragging selects, the middle button
  pastes. Set to `click`, a click in the box places the cursor — and the wheel
  is a button too, so it stops scrolling until crucible exits. One trade with
  two ends, which is why it is a setting and not a default.

### Changed

- **There is no model built in.** crucible used to fall back to a Claude name
  whatever provider you had a key for, so `--model openai/` asked OpenAI for a
  model OpenAI does not serve. With nothing configured and nothing on the
  command line it now starts and says so, under the welcome, rather than
  picking on your behalf. `/model` or an API key variable is the way out.

- **The prompt box takes typing while a turn is running.** Raw mode is held for
  the length of the session rather than only while a line is being read, so
  what you type during an answer is there when the answer ends instead of being
  swallowed.

### Fixed

- **Authentication can fail where it is used, not only where it is looked up.**
  A credential that cannot be renewed reaches the turn as a failed turn with the
  reason on screen, so the session is still there to fix it in.

## [0.0.14] - 2026-08-13

### Added

- **A click in the prompt box places the cursor.** `Prompt::clicked` answers
  which character of the line a click landed on; a click on the border, the
  status row or the list above moves nothing rather than jumping to the nearest
  place inside. Nothing is wired to a mouse yet — reporting is opt-in, because
  turning it on takes the wheel away from the terminal.

### Changed

- **The prompt box grows downward instead of scrolling sideways.** A line longer
  than the terminal used to slide along one row, so the beginning of what you
  had typed was gone from the screen. The box now takes as many rows as the line
  needs, up to about half the window, and scrolls inside itself past that —
  keeping the cursor's row in view. A wrapped row carries a continuation mark so
  the fold is visible rather than guessed at.

### Internal

- A component's rows can be pictured against a file checked in beside it, so a
  test about what something *looks like* is reviewed as a screen rather than as
  a `vec!` of formatted strings. It is for pictures only: an invariant such as
  "no row is wider than the terminal" stays a property test, because a snapshot
  would assert the answer instead of the rule. `menu` is the first to use it.

### Fixed

- **The prompt box and the row under it never disappear.** The live tail was
  bounded by the whole window, so once a turn had written enough rows the tail
  plus the box came to more rows than the terminal has — and every frame erased
  lines the terminal had already taken, scrolling the box off the bottom. The
  tail is now bounded by what is left after the box, which is the only figure
  that keeps both on screen no matter how long the turn runs.

## [0.0.13] - 2026-08-13

### Fixed

- **OpenAI sessions that call tools work again.** A model that reasons before
  answering refuses function tools on Chat Completions outright — the request
  came back `Function tools with reasoning_effort are not supported for
  <model>`, naming an effort crucible never sent, because it is that model's own
  default. crucible speaks the Responses API now. The two answers were to turn
  the reasoning off or to move, and turning it off would have left every OpenAI
  session running a thinking model told not to think.

- **A cut answer is marked as cut where the model can see it.** The reason a
  turn stopped is recorded, and both wire protocols now put a line after the
  answer saying so. Without it the model reads its own half-sentence as a turn
  it chose to end, every time the transcript goes back.

### Changed

- **Other vendors serving an OpenAI-compatible API are no longer reached by the
  `openai` provider.** They implement Chat Completions and not Responses. One of
  them gets a provider of its own the day it is written; the seam is already the
  right shape, since a wire protocol is a module and nothing above the provider
  crate learns which endpoint answered.

### Fixed

- **A turn that was cut short no longer comes back looking finished.** The
  transcript kept what the model said and not why it stopped, so an answer ended
  by the token ceiling, a filter, a pause or <kbd>Ctrl-C</kbd> was replayed to
  the model as a turn it had chosen to end — on the next turn of that session,
  and on every turn of a continued one. The reason is recorded with the message
  and travels in the session log, and a cut answer is marked as cut when it goes
  back.

### Changed

- **The runner runs the tool the verdict was reached about.** It looked the name
  up from the call it was holding rather than from the proof, so the tool that
  ran and the tool a user was asked about were two values a call site had to
  keep in step by hand. The proof now carries the name, and there is nothing
  left to keep in step.

## [0.0.12] - 2026-08-13

### Fixed

- **A stop reason this build has never heard of is reported as unfinished.** It
  used to fall back to "the model finished", so the day a vendor adds a word to
  its list, an answer cut short would have arrived looking complete — the one
  failure a reader cannot catch for themselves. Being wrong about a turn that
  was fine costs a line of text; being wrong the other way costs the answer.

- **A proxy's own heartbeat no longer fails the turn.** An event under a name
  this build does not know can arrive with no data line at all, and it was read
  as a payload before the name was looked at — so empty text was parsed as JSON,
  and the turn died along with the answer already on screen. The name decides
  first now.

### Fixed

- **A refusal that never finishes arriving no longer wedges the turn.** The
  message under a refused response is read to the end so it can be shown, and a
  body that stalled with the connection still open was read forever — the turn
  sat there with nothing on screen explaining it. The read is bounded now; what
  arrived is reported, and a refusal that simply pauses mid-sentence is still
  read whole.

- **<kbd>Ctrl-C</kbd> lands while a model is thinking.** The socket read had no
  bound, so a provider that stopped sending mid-answer kept the turn inside it
  and the press waited for the next token — which might never come. The read is
  now bounded, comes back saying nothing arrived, and the turn checks whether
  you asked it to stop.

### Changed

- **A provider stream can say "nothing yet".** A read used to block until the
  provider spoke, so the cancel flag was only ever looked at between events —
  and a model that went quiet mid-answer held the turn on the socket for as long
  as it stayed quiet, with <kbd>Ctrl-C</kbd> waiting behind it. A read that
  brings nothing back is now a third answer alongside an event and an ending,
  handed to the caller so it can look at its cancel and ask again.

## [0.0.11] - 2026-08-13

### Fixed

- **A `grep` walk stops when the turn does.** The same as `glob`'s: a tree
  worth searching is a tree where <kbd>Ctrl-C</kbd> has to arrive, and nothing
  in the walk was watching. A search stopped partway reports what it had, and
  says so, including when it stopped inside a file it was reading.

- **`glob` holds no more paths than it will answer with.** Every path the walk
  found was kept and then sorted, so a pattern like `**/*` in a large tree built
  a list of everything before cutting it to the few hundred it would report —
  the memory was the size of the tree rather than the size of the answer. It now
  keeps only the lowest paths it has room for, which answers the same as sorting
  all of them, and its answer is bounded in bytes the way `grep`'s is.

- **A `glob` walk stops when the turn does.** A tree large enough to be worth
  walking is a tree where <kbd>Ctrl-C</kbd> has to arrive, and nothing in the
  walk was watching. What it had found before the stop is real and is reported,
  marked so the model does not read a prefix as the whole answer.

- **Containment holds against the tree changing underneath it, on Unix.** A
  check settles where a name led at the instant it ran, and anything else that
  can write into the workspace could move it after — replace a directory above
  the file with a link and the open followed it out. A proven path is now
  reached by walking down against descriptors already held, one component at a
  time, leaving no interval for a swap. Windows keeps the check on the last
  component; `workspace/open.rs` says what that leaves.
- **`grep` bounds its answer in bytes, not just in matching lines.** The limit
  was a count, and a count is not a promise about size: two hundred matching
  lines of four hundred characters is eighty kilobytes into the next request,
  against the thirty kilobytes a command's output is held to — and the caller
  that chose the two hundred is the model. The bytes now bound the answer, the
  count is a second and smaller limit on top, and a cut answer says it was cut.

- **Stopping a command stops the pipeline it started, on Unix.** Only the shell
  was signalled, and every other process on the line is a child of it — so they
  were reparented and kept running. `yes > /dev/null | cat`, timed out or
  cancelled, burned a core for the rest of the session with nothing left holding
  a handle to it. The signal now goes to the process group the command was
  given. Windows has no process group to signal and is unchanged: a killed
  command there still leaves what it started running, and still says so.

- **`edit` holds one open file for the read and the write.** It used to name the
  path twice, and the text it wrote was decided from the text it had read a
  moment earlier — so if the name was made to lead somewhere else in between,
  the change landed on a file nobody had looked at.

### Changed

- **A proven path becomes an open file in one place.** The tools no longer open
  by path themselves; they ask the value that carries the proof. What that open
  refuses is now stated once instead of at each call site, which is what lets it
  be strengthened once.

## [0.0.10] - 2026-08-13

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

### Removed

- **The reach analysis.** crucible used to read a command line and work out
  whether everything it touched was inside the working directory, so that
  `allowEdits` could run some commands without asking. Nothing reads that
  answer now, and it could not be made sound: a shell reopens paths by name, so
  a symbolic link put there afterwards moved the write and nobody was asked.

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

[Unreleased]: https://github.com/augments-labs/crucible-code/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/augments-labs/crucible-code/compare/v0.1.13...v0.2.0
[0.1.13]: https://github.com/augments-labs/crucible-code/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/augments-labs/crucible-code/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/augments-labs/crucible-code/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/augments-labs/crucible-code/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/augments-labs/crucible-code/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/augments-labs/crucible-code/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/augments-labs/crucible-code/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/augments-labs/crucible-code/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/augments-labs/crucible-code/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/augments-labs/crucible-code/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/augments-labs/crucible-code/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/augments-labs/crucible-code/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/augments-labs/crucible-code/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/augments-labs/crucible-code/compare/v0.0.17...v0.1.0
[0.0.17]: https://github.com/augments-labs/crucible-code/compare/v0.0.16...v0.0.17
[0.0.16]: https://github.com/augments-labs/crucible-code/compare/v0.0.15...v0.0.16
[0.0.15]: https://github.com/augments-labs/crucible-code/compare/v0.0.14...v0.0.15
[0.0.14]: https://github.com/augments-labs/crucible-code/compare/v0.0.13...v0.0.14
[0.0.13]: https://github.com/augments-labs/crucible-code/compare/v0.0.12...v0.0.13
[0.0.12]: https://github.com/augments-labs/crucible-code/compare/v0.0.11...v0.0.12
[0.0.11]: https://github.com/augments-labs/crucible-code/compare/v0.0.10...v0.0.11
[0.0.10]: https://github.com/augments-labs/crucible-code/compare/v0.0.9...v0.0.10
[0.0.9]: https://github.com/augments-labs/crucible-code/compare/v0.0.8...v0.0.9
[0.0.8]: https://github.com/augments-labs/crucible-code/compare/v0.0.7...v0.0.8
[0.0.7]: https://github.com/augments-labs/crucible-code/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/augments-labs/crucible-code/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/augments-labs/crucible-code/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/augments-labs/crucible-code/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/augments-labs/crucible-code/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/augments-labs/crucible-code/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/augments-labs/crucible-code/releases/tag/v0.0.1
