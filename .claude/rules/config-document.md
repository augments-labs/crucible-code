---
paths:
  - "crates/crucible-config/**"
  - "schema/**"
---

# Changing crucible-config

This crate reads configuration files into settings. It decides what a document
may say, what a refusal reads like, and which layer wins — and it never applies
any of it. Applying is the wiring's job, in `src/`.

## Adding a setting is one edit

A key is declared in `shape.rs` and nowhere else. The parser walks values
against that declaration and the schema is generated from it, so adding a
`Field` there is the whole change: the parser accepts the key, the schema gains
a property, and the editor completes it.

Never add a key by teaching `check.rs` about it, and never hand-edit
`schema/crucible-code-schema.json`. That file is output. `scripts/check.sh`
regenerates it and compares, so an edit to it is reverted by the next run at
best and fails the gate at worst.

Every `Field` needs an `about` that reads as a sentence to somebody who has
never opened this repository — it is what an editor shows in the completion
popup, and it is the only documentation most people will see.

## The home directory

`home.rs` resolves where crucible's own files are, and it is the only place that
asks. Everything else — including the runner's session log — is handed a path.
Do not read `HOME`, `CRUCIBLE_CODE_HOME` or an XDG variable anywhere else; a
second answer to "where do the files live" is a bug that only shows up on
somebody else's machine.

A tree that is already on disk is read where it is. Never copy, move or delete
one to reach a tidier layout: `--continue` has to keep working for somebody who
upgrades in the middle of a piece of work, and those operations cannot be undone
on files the docs call theirs.

A variable crucible reads *before* it opens a file cannot be set from one. Add
such a variable to `env::too_late` in the same commit that starts reading it,
or it becomes a setting that looks applied and does nothing.

## Where a value's meaning is decided

`shape.rs` says a value is one of a fixed set; `settings.rs` says what each
answer means. Those are two lists, so they are tested against each other. If you
add an answer to a `Choice`, add its arm to the matching `read` — the test that
walks every accepted string will tell you if you forget, and without it the
schema would accept a value the program then drops on the floor.

## Merging

A scalar takes the nearest layer that set it. An object is merged key by key.
The shape decides which, so neither rule is written down twice — do not add a
special case to `merge` for a particular block. Nothing in the document is a
list yet; when the first one arrives, decide the rule deliberately and write it
in `docs/configuration/configuration.md` before implementing it.

## The layer that travels

`.crucible/config.json` is checked in. It may set crucible's own variables —
the `CRUCIBLE_CODE_` namespace, whose meanings this program fixes — and no
others. That refusal is structural, in `check.rs`, rather than a warning, so a
key cannot reach everyone who clones a repository by being committed in a file
nobody read. Do not add a way around it, including a "trusted project" setting:
the property holds only because there is no such path.

An error message may name a variable. It may never quote the value beside it.

## Errors are read by somebody with the file open

A refusal names the file, the dotted path, the position where one can be given,
and what was accepted instead. Adding an error variant that says only that
something is invalid is a regression even though it compiles. Where a key
appears more than once, give no position rather than a plausible wrong one.
