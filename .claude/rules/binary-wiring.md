---
paths:
  - "src/**"
---

# Changing the binary

This is the wiring: the only place a provider, a tool and a runner are allowed
to know about each other. Every crate below is reached as a trait object, and
what makes that hold is that the arrows are drawn here and nowhere else.

## A concrete type may be named here and may not leave

`Anthropic`, `OpenAi`, `Https`, `Bash`, `Read` and the rest are named in
`cli/startup.rs` and immediately become `Box<dyn Provider>` and `Tools`. A
concrete type that travels further — into a function signature elsewhere in
`src/`, or worse, into a crate — has moved a decision out of the one file that
is allowed to make it.

The reverse direction is the one to watch while editing: if wiring something up
seems to need a new variant in a core enum or a new method on a core trait, the
seam is in the wrong place. Say so rather than widening core to fit.

## The parser and the help text are one change

`PROVIDERS`, the `long_about` on `Cli` and the arm in `startup::provider` all
answer the same question, and a user meets whichever of them is wrong. A
provider the parser accepts and the help text never mentions, or a name in the
help text with no arm behind it, is the failure this section exists for. Adding
or renaming one is an edit to all of them in the same commit.

The names of the environment variables live beside that arm. The *names* are
what is configured; a value is read once, applied to a header, and appears in no
log, no error and no session file.

## `main` is where an error becomes an exit code

Everything below returns `Result`. `Fatal` is this package's error enum and the
only thing that turns into an `ExitCode` — a fallible call in `src/` that
handles its own failure by ending the process has taken that decision away from
the one function that is allowed to make it.

## One thread writes to the terminal

The turn runs on its own thread and reports through a channel; the thread that
drew the prompt is the thread that draws everything after it. That is why no
lock appears on the render path. Anything new that wants to write to the
terminal arrives as an `Event` on that channel instead — a second writer is a
corrupted transcript, and it will not look like a locking bug when it happens.

## A redraw nobody asked for draws only when the picture has changed

Most frames are caused by something — a key, an event — and end when the thing
that caused them has been drawn. The live foot is the exception: its working row
counts the clock and turns the mark while nothing arrives, so the loop has to
redraw on its own, and the loop it redraws on looks at the keyboard sixty times
a second. Prompt facts that change with an event belong to that same candidate
because prompt, working rows, and transcript map are replaced together.

So the live foot is a function of a small value, and that value is what the loop asks
about. `Turning::moved` holds the `Drawn` the candidate is drawn from and answers
whether it has changed since the last frame; a redraw with no cause behind it
happens only when the answer is yes. That is what keeps an animated row four frames a
second rather than sixty, and the box under it laid out four times rather than
sixty.

Which makes `Drawn` the thing to check when the live foot gains a fact. Anything
the candidate says and that value does not carry changes on screen only when
something *else* happens to change with it — stale state, arriving late, in the
foot somebody is reading to find out what is going on.

## `src/bin/` is not shipped

Nothing under it reaches a release. Most are bench probes, existing so
`scripts/bench.sh` has something to measure; one writes a table that is checked
in, run by hand and never by a build. Code shared between them belongs in a
module under `src/bin/`, not in the binary's own tree, and nothing in the binary
may reach for it.

A generator there reads its input rather than fetching it. What reaches the
network is a `curl` in the script beside it, written where somebody can see it —
this program decides to talk to a server in one place, and that place is a turn.
