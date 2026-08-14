---
paths:
  - "crates/crucible-auth/**"
---

# Changing crucible-auth

This crate holds one file: the keys `/login` was given, kept so the next launch
does not have to ask again. It is the only place in the workspace that a secret
meets a disk, which is the whole reason it is a crate of its own rather than a
module inside the wiring.

## A key leaves as an `ApiKey` or it does not leave

`Keys::get` hands back `crucible_core::ApiKey`, which can be applied to a
request and cannot be read. Nothing here may add an accessor that returns the
string, and nothing may implement `Display` — the type exists so a key cannot
reach a log line by accident, and one function returning `&str` is all it takes
to undo that. `Debug` is written by hand for anything that can reach a key, and
a test greps every `Debug` output in the crate for a sentinel.

## Reading never fails

`Store::read` returns `Keys`, not `Result<Keys>`. Absent, truncated, or written
by a version that does not exist yet all mean *nobody is logged in*, and most
launches need no stored key at all — so a damaged file may not be the reason
somebody cannot start crucible. What could not be done comes out of
`Keys::trouble` as one sentence for the user, and reading never rewrites what it
could not understand.

## What does not belong here

No network, no clock, no flow, no token. An API key does not expire, so there is
nothing to renew and nothing that has to mutate itself in the middle of a
request. Where the file lives is not decided here either: `Store::in_home` is
handed the directory, because `crucible_config::Home` is the one place that
answers where anything is.
