---
paths:
  - "crates/crucible-provider/**"
---

# Changing crucible-provider

This crate turns one vendor's wire format into the `Delta` stream the rest of
the program understands. It is the only place a vendor's vocabulary is allowed
to appear.

## Adding a provider

A new provider is a new module here, plus three edits in `src/cli.rs`: a match
arm in `provider`, the name in `PROVIDERS`, and the `--model` help text. It is
never an edit to `crucible-core`. If adding one seems to need a new core enum
variant or a new trait method, the abstraction is wrong — say so rather than
widening core to fit.

Those three are deliberately together in one file. A provider the parser accepts
and the help text never mentions is the failure worth designing against.

Each provider module follows the same three-way split, because the pieces fail
differently and are tested differently:

- `body.rs` — the request going out. Pure construction, no I/O.
- `wire.rs` — one chunk in, however many deltas out. Pure parsing, no state
  beyond what a single chunk carries.
- `stream.rs` — the delivery loop, cancellation, and what happens when the
  chunks run out.

## chunk is a wire word, and it stops at this crate's edge

`CLAUDE.md` bans **chunk** as a synonym for **delta** everywhere. Inside this
crate it is not a synonym: it names the object the vendor sends, and OpenAI's is
literally typed `chat.completion.chunk`. One chunk can yield zero deltas (a
keep-alive) or several.

So: `chunk` may name a vendor's object in this crate and nowhere else. Nothing
this crate *returns* is called a chunk. If a name with `chunk` in it is about to
cross into `core`, `runner` or `tui`, it is a delta and was named wrong.

## Credentials

A provider receives a resolved `Credential` and applies it. It never learns
which kind it was, never reads an environment variable, and never branches on
whether a key or a subscription token is behind it. Adding a login method is a
new `impl Credential`, not an edit to any provider.

A provider that needs a *header name* rather than a credential kind is asking
the right question — that is what `HeaderKey` carries.

## Parsing

Parse once, here, at the boundary. A vendor field that is absent, null, or a
type nobody expected becomes a typed error at this seam or it becomes a panic
three layers up. Deserialize straight into what the code uses; a struct that
mirrors the vendor's shape and then gets converted is two things to keep in
sync.

An unrecognised event is not an error. Vendors add fields, and a stream that
dies on an unknown one fails every turn at the last moment. Ignore what has no
meaning here; error only on what claims to have meaning and does not parse.
