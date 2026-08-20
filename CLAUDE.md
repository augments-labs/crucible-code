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

Its thresholds are ceilings rather than targets, and each is explained beside
the number it governs in `scripts/check.sh` — which is where the number can
change and the reasoning cannot be left behind.

The one thing that file cannot say about itself: a threshold moves when the
standard changes, and never so that a file fits. That is the edit no gate can
catch, because the gate is the thing being edited.

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

## Rules

The rules live in `.claude/rules/`, one topic per file. `.agents/rules` is a
symlink to that directory, so a harness looking for either name reads the same
files — as with `AGENTS.md`, and with the skills.

**If your harness does not load them, read them now, before anything else.**
Two are unscoped and apply to every change — `writing-it-down.md` and
`borrowed-ideas.md`. The rest name the files they govern in their own
frontmatter: `rust-invariants.md`, `performance-budgets.md`,
`shipped-artifacts.md`, `dependencies.md`, and one per crate beside them.

They are not repeated here. A rule stated twice is a rule that will disagree
with itself the first time one copy is edited.

## Vocabulary

One word per concept, in names, comments, docs and commit messages. The synonym
column gets sent back in review.

| Concept | Use | Never |
| --- | --- | --- |
| One prompt and everything that follows from it until the agent yields | **turn** | exchange, round, iteration, loop |
| The ordered record of turns | **transcript** | history, conversation, messages, backlog |
| A conversation bound to a working directory | **session** | chat, thread, context |
| One piece of streamed output from the model | **delta** | chunk, token, fragment |
| One piece of what a tool has printed, arriving while its call is still out | **wrote** | delta, chunk, log, tail, progress |
| A command that goes on running after its call has answered | **background command** | job, task, daemon, service, process |
| The model asking to run a tool | **tool call** | function call, invocation, action |
| A permission decision | **verdict** | approval, grant, decision |
| A call whose effect is not on this machine | **reaches the network** | online, remote, external, outbound |
| What answers a web search or a fetch | **source** | backend, engine, searcher, service, index |
| One thing a search handed back | **result** | hit, match, document, snippet |
| An LLM backend adapter | **provider** | backend, client, vendor, model |
| What drives turns to completion | **runner** | engine, orchestrator, driver, executor |
| One thing that happened, reported as it happens | **event** | entry, record, item |
| One prompt, answer or tool result in a transcript, and one line of the session log | **message** | entry, record, item |
| What a response or a turn has cost, counted in tokens produced | **spend** | usage, cost, consumption, spending |
| What one call puts to the user, and everything it gets back | **ask** | dialog, form, survey, poll, questionnaire, wizard |
| One thing a question offers to be chosen | **answer** | option, choice, item, candidate |
| The questions of an ask, drawn across the top of it | **the questions row** | tabs, stepper, breadcrumbs, wizard |
| What an answer would look like, drawn under it | **specimen** | preview, sample, example, mock |
| The stop where every answer is read back before it is sent | **review** | summary, confirm, submit |
| How much a model accepts at once, counted in tokens | **window** | context, context window, context size, budget |
| What one request carried to the model, counted in tokens | **carried** | input, prompt tokens, context length |
| What the next request would carry, held by the turn while it runs | **load** | usage, fill, pressure, occupancy |
| What compaction leaves standing in place of what it replaced | **recap** | digest, précis, synopsis — and *summary*, except on screen |

Banned type suffixes where a domain word exists: `Manager`, `Service`,
`Handler`, `Helper`, `Util`, `Processor`, `Data`, `Info`, `Base*`, `Abstract*`.

A second, smaller concession of the same kind: `SearchResult` is spelled out
because `Result` is Rust's own, and `SourceError`'s field naming a source is
`named` because `thiserror` reserves `source` for the underlying error. Both are
the *word* losing to a language feature rather than to a synonym, and in prose
each is still a result and a source.

Three exceptions, all of the same kind — a word kept for whoever owns the thing
it names.

Inside `crucible-provider`, **chunk** names the wire object a
vendor sends — OpenAI's is literally typed `chat.completion.chunk`. It is not a
synonym for a delta, it is a layer below one, and one chunk can yield several.
Using a vendor's word for a vendor's object is what keeps that a distinction
rather than a coincidence. Everywhere above the wire, a delta is a delta.

On screen, the recap is called a **summary**, and only there. It is the word a
person arrives already knowing, and the row offering it has one line to be
understood in — while `Summary` in this codebase is what a tool says about a
call, so the code cannot have the word back. The two never meet: no type, field
or function is named for it, and what a user reads is the one place the domain
word gives way to theirs.

Inside `crucible-tui`, **window** means the terminal this process is drawing
into — the reader's own word for their own screen, and the thing every component
is laid out against. That crate names no domain type and is never told what a
model accepts, so the two meanings cannot meet in one file: it is handed a
number of columns, or a percentage already worked out. Everywhere else, a window
is the model's.

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
- **A feature that needs a dependency gets the dependency.** The ladder in
  `add-a-dependency` decides which rung the answer is on; it never decides
  whether the work ships. Where the honest answer is a crate — a protocol, a
  parser, platform branching, a timeout `std` cannot spell — add it, walk the
  skill, and build the thing. Quietly scoping a feature down to avoid a
  dependency is under-delivery, and it is worse than the dependency because
  nothing records that it happened.
- **Rendering is inline today, and that is a mechanism rather than a law.**
  Scrollback belongs to the terminal, which is what keeps rendering free as a
  transcript grows. A full-screen renderer would move that job into this
  process; what it may not move is the budget, so it would owe a virtualized
  viewport in exchange. The budget is the rule; inline is how 0.x meets it.
