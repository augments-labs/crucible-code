---
paths:
  - "crates/crucible-provider/**"
---

# Changing crucible-provider

This crate turns one vendor's wire format into the `Delta` stream the rest of
the program understands. It is the only place a vendor's vocabulary is allowed
to appear.

## Adding a provider

A new provider is a new module here, plus four edits in `src/cli.rs`: the
`const <NAME>_KEY` naming its environment variable, a match arm in `provider`,
the name in `PROVIDERS`, and the `--model` help text. It is never an edit to
`crucible-core`. If adding one seems to need a new core enum variant or a new
trait method, the abstraction is wrong — say so rather than widening core to
fit.

Those four are deliberately together in one file. A provider the parser accepts
and the help text never mentions is the failure worth designing against.

How a provider module divides into parts is stated in that module's own doc
comment, which is the copy to keep current.

## chunk stops at this crate's edge

`CLAUDE.md` says why **chunk** is allowed here and nowhere else. The part that
binds while you are editing: nothing this crate *returns* is called a chunk. If
a name with `chunk` in it is about to cross into `core`, `runner` or `tui`, it
is a delta and was named wrong.

## Credentials

A provider that needs a *header name* rather than a credential kind is asking
the right question — that is what `HeaderKey` carries. Wanting to know which
kind is behind it means a seam is about to move into the wrong crate.

## Parsing

A vendor field that is absent, null, or a type nobody expected becomes a typed
error at this seam or it becomes a panic three layers up. Deserialize straight
into what the code uses; a struct that mirrors the vendor's shape and then gets
converted is two things to keep in sync.

An unrecognised event is not an error. Vendors add fields, and a stream that
dies on an unknown one fails every turn at the last moment. Ignore what has no
meaning here; error only on what claims to have meaning and does not parse.

A stop reason is the exception, and the reason this section is not simply "be
lenient". One this build has not heard of reads as a finish, so an answer that
was cut short arrives looking complete — the one failure the user cannot see
for themselves. A new reason in a vendor's list is an edit here, not a case the
fallback arm can be trusted to cover.
