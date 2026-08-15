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

`StoredCredentials::get` hands back `crucible_core::ApiKey`, which can be applied to a
request and cannot be read. Nothing here may add an accessor that returns the
string, and nothing may implement `Display` — the type exists so a key cannot
reach a log line by accident, and one function returning `&str` is all it takes
to undo that. `Debug` is written by hand for anything that can reach a key, and
a test greps every `Debug` output in the crate for a sentinel.

## Reading never fails, writing always may

`Store::read` returns `StoredCredentials`, not `Result<StoredCredentials>`. Absent, truncated, or written
by a version that does not exist yet all mean *nobody is logged in*, and most
launches need no stored key at all — so a damaged file may not be the reason
somebody cannot start crucible. What could not be done comes out of
`StoredCredentials::trouble` as one sentence for the user, and reading never rewrites what it
could not understand.

Writing is the opposite and owes a `Result`. In particular it refuses to write
over a store it could not parse: a read-modify-write over an unreadable file is
not a modification, it is a replacement, and what it replaces is the only copy
of somebody's other logins.

## The file is created private, not tightened afterwards

The mode is set at open time through `OpenOptions`, because the window between
creating a file at the umask's mode and narrowing it afterwards is long enough
to read a key out of. The directory is created `0700` only when crucible is the
one creating it — a `~/.crucible` the user already had is theirs, and what
protects the key is the file's own mode.

A store found readable by others is tightened, reported, and used. Refusing the
way `ssh` refuses a loose private key would leave a user unable to log in
without shell surgery, which is worse than the sentence.

## Every write is lock, read, rename

Two crucibles logging in at once must not each write a file that has forgotten
the other's provider. The lock is advisory, on a sibling of the store, because a
lock on the store itself would not survive the rename that replaces it. The
temporary is a sibling too, or `rename` is a cross-device copy that is not
atomic — which is what "write it to the system temporary directory" gets wrong
every time.

## What does not belong here

No network, no clock, no flow, no token. An API key does not expire, so there is
nothing to renew and nothing that has to mutate itself in the middle of a
request. Where the file lives is not decided here either: `Store::in_home` is
handed the directory, because `crucible_config::Home` is the one place that
answers where anything is.
