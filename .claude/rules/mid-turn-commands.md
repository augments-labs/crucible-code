---
paths:
  - "src/cli/converse/command.rs"
  - "src/cli/converse/command/**"
---

# A slash command run mid-turn never reaches the running turn

A turn owns the runner for its whole length: `sent()` moves it onto the worker
thread and `join()` brings it back, so for the time a turn is on screen there
is no runner on the thread that is reading the keyboard. A slash command typed
while a turn runs is answered on the drawing thread against what is there —
the screen, the session's shared terms, the catalog — and the runner is none
of those.

Every command in `command.rs` declares which of three things it is mid-turn,
in `Command::mid_turn`, beside the name and the one-line blurb it already
carries:

- **Live** — it moves nothing but the screen. `/theme` and `/help` draw and
  are answered from `Terms` and the catalog alone, so they open and apply
  while the turn runs; the transcript goes on rendering behind the panel.
- **Deferred** — it changes what the *next* request is asked with, and the
  running turn keeps what it started with. `/model` is the one of these: its
  picker reads the static catalog, the pick is held, and the loop applies it
  when it starts the next turn. Because the next turn is the one that re-reads
  the history against the new model, the reader is told that and asked before
  the pick is held.
- **Refused** — it would act on the runner, the session, or the credential the
  in-flight request is signed with, and touching any of those mid-turn breaks
  the turn. Each carries its own one-line reason, said on a panel that stands
  in for the working row, the box, the status and the map until `esc` closes
  it.

The deciding question for a command added later is which of the three its
effect touches. If it draws and changes nothing the running turn is using, it
is Live. If it only changes what the next request is built from, it is
Deferred. If it would reach the runner, end or replace the session, or pull a
credential out from under the request now streaming, it is Refused — and the
reason it refuses is written beside it, in the same place, when it is added.
