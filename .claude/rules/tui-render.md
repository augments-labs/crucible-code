---
paths:
  - "crates/crucible-tui/**"
---

# Changing crucible-tui

## The budget is the rule, and this crate is now what keeps it

Nothing in this crate may be proportional to how long the session has run. Peak
RSS ≤35 MB after a 20-turn session is a budget in `CONTRIBUTING.md` and
`scripts/bench.sh` measures it. That constraint does not move.

A session takes the alternate screen, so scrollback is this process's. That is
the whole of what changed: the terminal used to hold what had been said, for
free, and now the record does. Three things are what keep the budget under that,
and each of them is load-bearing rather than an optimisation.

The record is bounded — past its ceiling the oldest lines are dropped, and a
session that runs all day costs what one that ran a minute did. Only the lines
the viewport covers are folded into display rows, so a resize or a scroll costs
the window and not the session. And the screen buffers are the window's size, so
a frame diffs rows and writes the ones whose text is not already there.

A change that folds the whole record to draw one frame, or holds a second copy
of it to scroll faster, has broken the budget even where it looks correct on a
short session. Test it on a long one — `scripts/bench.sh` is where the long one
is, and a probe that only takes twenty turns is measuring the wrong thing.

## Wrapping is this crate's job

Text is wrapped into display rows here rather than being left to the terminal,
because a band is a fixed rectangle of a screen this process is addressing by
row number. A row the terminal wrapped on its own is a second row where one was
allowed for, which pushes every band below it a row from where it belongs.

So: measure with display width, not `len()` or `chars().count()`. A CJK glyph is
two columns wide, a combining mark is zero, and getting this wrong corrupts the
screen rather than merely looking off.

## The render path

No blocking I/O, no allocation that can be hoisted, no formatting that runs per
frame when it could run per change. The budget is ≥30 rendered frames/s under
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

The screen itself is the largest of those borrows, and the order matters: the
guards are taken outermost-first so they are given back innermost-first, and raw
mode is left while the alternate screen is still standing. A sequence that
leaves raw mode after the screen has gone is written onto the reader's own.

Restoring in a normal-exit branch is not enough. The panic, the `?` and the
Ctrl-C all have to leave the terminal usable.

A borrow also has to *have* something to borrow. Every guard checks
`is_terminal()` before it changes any state and does nothing when output is
redirected — the escape would not be state there, it would be bytes in the
middle of somebody's file, and the restore on the way out would be more of them.
`Title::set` is the pattern: the check is inside the only constructor, so a
caller cannot aim one at a pipe.
