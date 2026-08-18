---
paths:
  - "crates/crucible-config/**"
  - "schema/**"
---

# Changing crucible-config

This crate reads configuration files into settings. It decides what a document
may say, what a refusal reads like, and which layer wins — and it never applies
any of it. Applying is the wiring's job, in `src/`.

Each layer has an explicit byte ceiling before JSON parsing. A project file is
input from the checkout, so it may not choose an unbounded startup allocation.

## Adding a setting is one edit

A key is declared in `shape.rs` and nowhere else. The parser walks values
against that declaration and the schema is generated from it, so adding a
`Field` there is the whole change: the parser accepts the key, the schema gains
a property, and the editor completes it.

Never add a key by teaching `check.rs` about it, and never hand-edit
`schema/crucible-code-schema.json`. That file is output.

What holds that is a test, `the_checked_in_schema_is_what_this_generates`, at the
foot of `shape/schema.rs` — not a section of `scripts/check.sh`, though the gate
runs it along with everything else `cargo test` runs. Knowing which it is matters
because of what the test does when the two disagree: it **rewrites the checked-in
file and then fails**. So a hand-edit is reverted, a stale schema is repaired,
and the run that repaired it is the only one that says so — the next run is
green, with the file already changed underneath you. `scripts/check.sh` is
read-only over the whole tree except here, which is why it has a `schema` section
that reports the rewrite by name rather than leaving it to be found in
`git status`.

The remedy is always the same: read the diff, satisfy yourself it is what your
`shape.rs` change should have produced, and commit it with that change.

Every `Field` needs an `about` that reads as a sentence to somebody who has
never opened this repository — it is what an editor shows in the completion
popup, and it is the only documentation most people will see.

## The home directory

`home.rs` resolves where crucible's own files are, and it is the only place that
asks. Everything else — including the session log — is handed a path.
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

`shape.rs` says a value is one of a fixed set; the module under `settings/` that
owns that block says what each answer means. Those are two lists, so they are
tested against each other. If you add an answer to a `Choice`, add its arm to
the matching `read` — the test that walks every accepted string will tell you if
you forget, and without it the schema would accept a value the program then
drops on the floor.

A block whose values mean something other than the strings they were written as
gets a module under `settings/`, so its keys, the types they become and the
tests for both sit in one place. `settings.rs` keeps the layering, the merge,
and the blocks that are read straight back out.

## Merging

A scalar takes the nearest layer that set it. An object is merged key by key. A
list is concatenated. The shape decides which, so no rule is written down twice
— do not add a special case to `merge` for a particular block.

A nearer layer can add to a list and can never shorten one. That is forced
rather than chosen: `permissions.deny` is a list, and a checked-out repository
that replaced it would silently drop what somebody denied on their own machine.
Anything you add that holds several values inherits that rule, so a list whose
entries are meant to *remove* something is a list that cannot be written here.

## Rules and the position they were written at

A permission rule is read in `rules.rs`, per document, while the file it came
from is still known. Do not move that read up to the resolved settings: by then
a rule is one line among the lines every layer contributed, and a refusal could
no longer name the file or the position — which is the one thing an error in
this crate owes its reader.

## The workspace layers

Either project filename can be committed, whatever its ignore convention says.
Both may set crucible's own variables — the `CRUCIBLE_CODE_` namespace, whose
meanings this program fixes — and no others. Neither may allow calls, widen
filesystem reach, select a credential source, or redirect a request. Those
refusals are structural, in `check.rs`, rather than warnings. Do not add a way
around them, including a "trusted project" setting: the property holds only
because there is no such path.

An error message may name a variable. It may never quote the value beside it.

## Errors are read by somebody with the file open

A refusal names the file, the dotted path, the position where one can be given,
and what was accepted instead. Adding an error variant that says only that
something is invalid is a regression even though it compiles. Where a key
appears more than once, give no position rather than a plausible wrong one.
