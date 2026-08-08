---
paths:
  - "crates/crucible-tui/**"
---

# Changing crucible-tui

## The budget is the rule; inline is only how it is kept today

Nothing in this crate may be proportional to how long the session has run. Peak
RSS ≤35 MB after a 20-turn session is a budget in `CONTRIBUTING.md` and
`scripts/bench.sh` measures it. That constraint does not move.

Inline rendering is the *mechanism* currently chosen to satisfy it: this process
draws into the terminal's normal buffer and never takes the alternate screen, so
scrollback belongs to the terminal — the user keeps their history, their scroll
position and their selection, and this process holds only a bounded live tail.

A full-screen renderer is on the roadmap and would replace that mechanism. It
does not replace the budget. Taking the alternate screen means this process
becomes the owner of scrollback, so it inherits the job the terminal was doing:
the viewport is virtualized, only visible rows are materialized, and the
transcript above lives on disk or in a bounded window rather than in a `Vec`
that grows all session. Rewrite this section when that lands; do not soften the
paragraph above it.

Until then, the live tail is bounded — once there are more rows than the bound,
the oldest are written out once and forgotten. A change that holds the whole
transcript to re-render it has broken the budget even if it looks correct on a
short session. Test it on a long one.

## Wrapping is this crate's job

Text is wrapped into display rows here rather than being left to the terminal,
because the renderer has to know how many rows it drew in order to move back
over them. A row the terminal wrapped on its own leaves the cursor somewhere
this process did not predict, and the next frame erases the wrong lines.

So: measure with display width, not `len()` or `chars().count()`. A CJK glyph is
two columns wide, a combining mark is zero, and getting this wrong corrupts the
screen rather than merely looking off.

## The render path

No blocking I/O, no allocation that can be hoisted, no formatting that runs per
frame when it could run per change. The budget is ≥30 render commits/s under
token burst, and `scripts/bench.sh` measures it.

Never `print!`/`println!` — `print_stdout` and `print_stderr` are denied by lint
for this crate's sake. Output goes through the `Terminal` seam so the tests can
assert on what was drawn, and so nothing writes into the middle of a frame.

## Restore what you change

Setting terminal state is a borrow, not a write: the terminal keeps whatever
this process last set, and keeps it after the process exits. So anything put
into a non-default state is handed back by a guard's `Drop` rather than by an
exit path that has to remember. `Title` is the pattern to copy — it holds the
tab title for exactly as long as the value is alive.

Restoring in a normal-exit branch is not enough. The panic, the `?` and the
Ctrl-C all have to leave the terminal usable.
