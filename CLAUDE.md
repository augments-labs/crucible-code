# crucible — contributor guide

A terminal coding agent in Rust. Original, clean-room implementation.

This is the always-on agent-facing rules file. `AGENTS.md` is a symlink to it,
so Claude Code, Codex and anything else that looks for either name reads the
same text. It holds the rules a gate cannot check; everything a gate *can* check
lives in `Cargo.toml`
`[workspace.lints]`, `clippy.toml`, and `scripts/check.sh` — those files are the
standard, not a description of it.

## Where agent instructions live

Three homes. Which one a thing belongs in is decided by who has to be able to
see it.

| Kind | Home | Read by |
| --- | --- | --- |
| True whatever you are touching | this file | both harnesses, via the symlink |
| Applies to one part of the tree | `.claude/rules/*.md`, with `paths:` frontmatter | Claude Code, when it opens a matching file |
| A procedure with steps | `.claude/skills/<name>/SKILL.md`, symlinked from `.agents/skills/` | either harness, on demand |

Codex concatenates `AGENTS.md` and resolves nothing else — no `@path` imports,
no rules directory. So anything under `.claude/rules/` is amplification for one
corner of the tree and may never be something a contributor has to know to be
correct; that belongs here, where the symlink carries it to both. Keep this file
under 200 lines — a longer one is followed less.

There is one rules file per crate, carrying the obligations that only bind when
you are inside it: what a new provider has to touch, what a new tool has to
declare, which word is allowed where. They say what to *do*, not what the code
already says about itself — a rule that restates a module doc is a second copy
waiting to drift. `scripts/check.sh` checks each one still has `paths:`
frontmatter aimed at something that exists, because a rule that loads for
nothing fails by staying silent.

## Gate

```bash
scripts/check.sh
```

Run it before every commit. It is exactly what CI runs.

## Layout

```
src/            the binary — wiring only, the sole place concrete types meet
  main.rs
  cli/          argument parsing and dispatch
  bin/          auxiliary binaries: bench probes, not shipped
crates/
  crucible-core/       domain types + traits. Depends on nothing.
  crucible-provider/   wire protocols (Anthropic, OpenAI).      -> core
  crucible-tools/      read write edit bash grep glob.          -> core
  crucible-runner/     the turn loop, over traits only.         -> core
  crucible-tui/        inline renderer, prompt, transcript.     -> core
scripts/        gates and benchmarks
docs/           user-facing documentation
```

Dependencies point **down only**. Cargo enforces it: a crate reaches only what
its own `[dependencies]` names, and cycles are rejected. `crucible-runner`
depending on core alone is deliberate — the loop drives `dyn Provider` and
`dyn Tool` and must never name a concrete one.

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
   or spawns a process takes a `Grant`. A `Grant` has a private field and is
   minted only by the permission engine, so it cannot be forged and a `Deny`
   cannot be passed off as an allow. Code without one cannot call the operation.
4. **Secrets never surface.** Not in logs, errors, `Display`, `Debug`, session
   files or panic payloads. Types holding a key implement `Debug` by hand and
   redact. Config stores env var *names*, never values.
5. **Clean-room.** No code, prompt text, UI string or asset copied or adapted
   from Claude Code, jcode, opencode, codex or any other harness. This repo is
   public; this is a legal boundary.
6. **Performance is the feature.** First frame ≤20 ms, first input ≤60 ms, peak
   RSS ≤35 MB after a 20-turn session, grep within 1.25× of `rg`, ≥30 render
   commits/s under burst. No blocking I/O on the startup path or the render
   path. The transcript is held whole and is what that RSS figure bounds;
   nothing *else* may grow with it, and a `.clone()` of a transcript-sized
   value needs a comment saying why.
7. **No process memory in shipped artifacts.** Comments explain the code.
   No requirement IDs, no design-doc citations, no references to planning
   directories. Traceability lives in commit messages and test names.
8. **Dependencies are `=`-pinned and justified.** A new one needs a comment in
   `Cargo.toml` saying why it is needed; `scripts/check.sh` fails without both.
   Pinning is also what hides an advisory published afterwards, so `deny.toml`
   is scanned on a clock instead — that check cannot live in a script whose
   whole promise is the same answer for the same tree.

## Vocabulary

One word per concept, in names, comments, docs and commit messages. The synonym
column gets sent back in review.

| Concept | Use | Never |
| --- | --- | --- |
| One prompt plus the exchange until the agent yields | **turn** | exchange, round, iteration, loop |
| The ordered record of turns | **transcript** | history, conversation, messages, backlog |
| A conversation bound to a working directory | **session** | chat, thread, context |
| One piece of streamed output | **delta** | chunk, token, fragment |
| The model asking to run a tool | **tool call** | function call, invocation, action |
| A permission decision | **verdict** | approval, grant, decision |
| An LLM backend adapter | **provider** | backend, client, vendor, model |
| What drives turns to completion | **runner** | engine, orchestrator, driver, executor |
| One record in the session log | **event** | entry, record, item |

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
- **Rendering is inline today, and that is a mechanism rather than a law.**
  Scrollback belongs to the terminal, which is what keeps rendering free as a
  transcript grows. A full-screen renderer would move that job into this
  process; what it may not move is the budget, so it would owe a virtualized
  viewport in exchange. The budget is the rule; inline is how 0.0.x meets it.

## Conventions

- `0.0.x` formats are unstable. Config, session files and CLI flags may change
  in any 0.0.x release with no deprecation period, and say so in their docs.
- Commits: `feat(scope): …`, `fix(scope): …`, `chore(scope): …`.
- New ideas go to the parking lot, never silently into the current release.
