# crucible — contributor guide

A terminal coding agent in Rust. Original implementation.

This is the always-on agent-facing rules file. `AGENTS.md` is a symlink to it,
so Claude Code, Codex and anything else that looks for either name reads the
same text. It holds the rules a gate cannot check; everything a gate *can* check
lives in `Cargo.toml`
`[workspace.lints]`, `clippy.toml`, and `scripts/check.sh` — those files are the
standard, not a description of it.

## Gate

```bash
scripts/check.sh
```

Run it before every commit. It is exactly what CI runs.

Its thresholds are ceilings, not targets: 2000 lines for a file, 100 for a
function. What a file owes is one reason to change, under a name that says what
it holds, and its length follows from that. The ceiling is set where a file has
plainly lost that name rather than where a careful one lands, because the
opposite failure is the one no number can see — a directory of files too small
to have a subject, each naming the next, where learning what one of them does
means opening all of them.

A threshold moves when the standard changes, and never so that a file fits.
That is the one edit no gate can catch, because the gate is the thing being
edited.

## Layout

```
src/            the binary — wiring only, the sole place concrete types meet
  main.rs
  cli/          argument parsing and dispatch
  bin/          auxiliary binaries: bench probes, not shipped
crates/
  crucible-core/       domain types + traits. Depends on nothing.
  crucible-privacy/    owner-only local file primitives. Depends on nothing.
  crucible-auth/       credential store and account logins.     -> core, privacy
  crucible-config/     configuration documents -> settings.     -> core
  crucible-provider/   wire protocols (Anthropic, Moonshot, OpenAI). -> core
  crucible-session/    session logs and bounded replay.  -> core, privacy
  crucible-tools/      read write edit bash grep glob.          -> core
  crucible-runner/     the turn loop, over traits only.  -> core, session
  crucible-tui/        inline renderer, prompt, transcript. Depends on nothing.
schema/         the configuration schema, generated from the shape the parser
                walks and checked in beside it
scripts/        gates and benchmarks
docs/           the published site. One directory per topic, `index.md` beside
                the pages; a directory name is a public URL segment.
```

Dependencies point **down only**. Cargo enforces it: a crate reaches only what
its own `[dependencies]` names, and cycles are rejected. `crucible-runner`
naming no concrete provider or tool is deliberate — the loop drives
`dyn Provider` and `dyn Tool` and must never name one.

## Hard rules

1. **Result, never panic.** Every fallible function returns `Result<T, E>` with
   a module-owned `thiserror` enum; `?` propagates. No `anyhow` — it erases the
   type and invites string errors. `main` is the only place an error becomes an
   exit code. Denied by lint; tests are exempt.
2. **Parse once, at the boundary.** Model output, tool arguments, config, env
   and file contents are parsed into domain types at the edge. Inner layers
   never re-validate. Anything with domain meaning gets a newtype —
   `SessionId`, `WorkspacePath`, `ApiKey` — never a bare `String`.
3. **Permission is an argument, not a question.** A function that mutates a file
   or spawns a process takes an `Approved`: the call itself, carried together
   with the `Grant` that says a verdict was reached about *that* call. `Grant`
   has a private field and is minted only by the permission engine, so it cannot
   be forged, a `Deny` cannot be passed off as an allow, and proof reached about
   one call cannot arrive beside another call's arguments. Code without one
   cannot call the operation.
4. **Secrets never surface.** Not in logs, errors, `Display`, `Debug`, session
   files or panic payloads. Types holding a key implement `Debug` by hand and
   redact. Config stores env var *names*, never values.
5. **Ideas travel; a body of expression does not.** What another harness *does*
   is free to learn from — its features, its documentation, its behaviour. Read
   it to understand how it works; that understanding is yours. What it wrote is
   not: its code, its prompts, its help pages, its art. Learn from it, never
   copy it.
6. **Performance is the feature.** First frame ≤20 ms, first input ≤60 ms, peak
   RSS ≤35 MB after a 20-turn session, grep's worst paired median ≤1.25× `rg`
   with p95 and dispersion as evidence, and ≥30 rendered frames/s under burst.
   No blocking I/O on the startup path or the render
   path. The transcript is held whole and is what that RSS figure bounds;
   nothing *else* may grow with it, and a `.clone()` of a transcript-sized
   value needs a comment saying why.
7. **No process memory in shipped artifacts.** Comments explain the code.
   No requirement IDs, no design-doc citations, no references to planning
   directories. Traceability lives in commit messages and test names. `docs/`
   is shipped — it is published as a website — so this binds every page: one
   documents what exists today and never what a later release will add.
   `scripts/check.sh` greps the shipped tree for those shapes, because the
   files that legitimately hold them sit one directory away.
8. **Dependencies are `=`-pinned and justified.** A new one needs a comment in
   `Cargo.toml` saying why it is needed; `scripts/check.sh` fails without both.
   Pinning is also what hides an advisory published afterwards, so `deny.toml`
   is scanned on a clock instead — that check cannot live in a script whose
   whole promise is the same answer for the same tree.
9. **The changelog and the commit message are brief.** A changelog entry is a
   bold lead and a sentence or two: what changed, and what it costs the person
   deciding whether to upgrade. A commit message is a subject line and a short
   paragraph saying why, since the diff already says what. Neither carries the
   alternatives weighed or the threat model — those have readers who went
   looking for them, in the code comment and the docs page.
10. **A change owes its documentation in the same commit.** `docs/` for anything
    a user meets, `README.md` for the first minute of it, `CONTRIBUTING.md` and
    `docs/building/` for what a contributor has to install, the changelog for
    anything that ships. The module doc comment is on that list and is the one
    most often left behind: this project states its invariants in the prose at
    the top of a file, so one the code has outgrown is a false statement of the
    invariant sitting where the next reader goes to learn it. Read the prose
    above what you changed, and above whatever now behaves differently because
    you changed it.
11. **A pull request over 400 changed lines is sent back, whatever it targets.**
    Additions plus deletions, generated files aside. Past that a review stops
    being one, and that is true of the reader in front of a diff into a
    collecting branch as much as one into `main`. The remedy is a sequence of
    pull requests that each stand on their own. Two diffs are measured wrongly
    and each takes a label, which is the only way past and stays visible on the
    pull request afterwards: `moves-only` for code that only moves, and
    `whole-module` for a module whose parts do not compile apart — `-D warnings`
    makes an unreached function a failed build, so where a module's floor is
    already over the ceiling, the only smaller pull request is one that lands
    code without the tests that prove it. The test of the second is whether an
    intermediate pull request would compile, not whether the change is large or
    awkward to split. `CONTRIBUTING.md` has the rest.
## Vocabulary

One word per concept, in names, comments, docs and commit messages. The synonym
column gets sent back in review.

| Concept | Use | Never |
| --- | --- | --- |
| One prompt and everything that follows from it until the agent yields | **turn** | exchange, round, iteration, loop |
| The ordered record of turns | **transcript** | history, conversation, messages, backlog |
| A conversation bound to a working directory | **session** | chat, thread, context |
| One piece of streamed output | **delta** | chunk, token, fragment |
| The model asking to run a tool | **tool call** | function call, invocation, action |
| A permission decision | **verdict** | approval, grant, decision |
| An LLM backend adapter | **provider** | backend, client, vendor, model |
| What drives turns to completion | **runner** | engine, orchestrator, driver, executor |
| One thing that happened, reported as it happens | **event** | entry, record, item |
| One prompt, answer or tool result in a transcript, and one line of the session log | **message** | entry, record, item |
| What a response or a turn has cost, counted in tokens produced | **spend** | usage, cost, consumption, spending |

Banned type suffixes where a domain word exists: `Manager`, `Service`,
`Handler`, `Helper`, `Util`, `Processor`, `Data`, `Info`, `Base*`, `Abstract*`.

The one exception: inside `crucible-provider`, **chunk** names the wire object a
vendor sends — OpenAI's is literally typed `chat.completion.chunk`. It is not a
synonym for a delta, it is a layer below one, and one chunk can yield several.
Using a vendor's word for a vendor's object is what keeps that a distinction
rather than a coincidence. Everywhere above the wire, a delta is a delta.

## Design notes worth knowing before you change something

- **Auth is a separate axis from the wire protocol.** A `Provider` receives an
  already-resolved `Credential` and never learns which kind it was. Adding a
  subscription login is a new `impl Credential`, not an edit to any provider.
- **Open sets are traits, closed sets are enums.** Providers and tools are open —
  adding one must not edit `core`. Events, verdicts and errors are core-owned
  enums *because* a new variant should break every `match`.
- **The schema has a second home, and it does not follow this one.**
  `schema/crucible-code-schema.json` is generated from the parser and gated, so
  it is always right here — by a test rather than by a section of the gate, and
  that test rewrites the file before it fails. It is the one thing a run of
  `scripts/check.sh` writes; everything else about that script only reads. So a
  stale schema fails once and is green the second time, with the file changed
  underneath you: read the diff before you commit it. SchemaStore serves its own
  copy, and nothing in this
  repository can make that one move. A release that changes the file owes a pull
  request against `SchemaStore/schemastore`, raised from the fork
  `NjoyimPeguy/schemastore`, which already exists — `RELEASING.md` has the
  commands and the formatter their gate insists on.
- **Rendering is inline today, and that is a mechanism rather than a law.**
  Scrollback belongs to the terminal, which is what keeps rendering free as a
  transcript grows. A full-screen renderer would move that job into this
  process; what it may not move is the budget, so it would owe a virtualized
  viewport in exchange. The budget is the rule; inline is how 0.x meets it.
