---
paths:
  - "crates/crucible-session/**"
---

# Changing crucible-session

One append-only log per session. The line format, the claim that says a
session is still open, the fixed index that names the recent ones, and the
platform code that holds the tree owner-only all live here, so the runner
records through `Session` and holds none of them. Its `[dependencies]` names
`crucible-core`, `crucible-privacy` for the index's durable replacement,
`serde_json` for the line format and `thiserror` for `SessionError` — plus
`windows-sys` on Windows, where the access-list call is FFI.

## A format change is a format version change

A log written by a different build is refused rather than half-understood;
guessing produces a session that looks fine and is missing turns. So a change
to what a line holds is a change to `wire::FORMAT` in the same commit.

## Session content is exact and private

Prompts, model text, tool arguments and tool results are transcript content
and are serialized exactly, so replay cannot change what happened. The
directory, every log and every claim mark are owner-only: a file mode on
Unix, an access list on Windows. The Windows side is FFI, and
`privacy/windows.rs` is the one module in this crate that opts out of
`unsafe_code` — precisely so that nothing else has to.

## One bounded index names recent sessions

The first migration may scan legacy logs, and it runs after the first frame.
Ordinary startup reads the fixed-size index. A change must preserve both
paths: old flat directories remain usable, while new sessions never make
first-frame work proportional to the number of logs.

## A claim is how an open session says so

The operating system holds the claim and releases it when the process ends,
however it ends, so a crash leaves nothing to clean up. The mark sits beside
the log rather than on it, because continuing opens the log again to read it,
cut it and append to it — and on Windows a lock on the file itself would bar
all three. A mark is never deleted: one a crashed process left behind holds
no lock, and deleting one is what would let two processes hold two different
files of the same name.
