---
paths:
  - "crates/**/*.rs"
  - "src/**/*.rs"
  - "scripts/bench.sh"
---

# Performance is the feature

First frame ≤20 ms, first input ≤60 ms, peak RSS ≤35 MB after a 20-turn session,
grep's worst paired median ≤1.25× `rg` with p95 and dispersion as evidence, and
≥30 rendered frames/s under burst.

No blocking I/O on the startup path or the render path. The transcript is held
whole and is what that RSS figure bounds; nothing *else* may grow with it, and a
`.clone()` of a transcript-sized value needs a comment saying why.

## What redraws mid-turn is coalesced, and the rate is measured rather than asserted

Two things move while a turn runs and neither is caused by a keystroke: text
arriving in a run, and a clock. Text in a run — the model's prose, and what a
running command has printed — is merged into one frame before it reaches the
terminal, up to a byte ceiling, and never across anything else, so the terminal
still sees what the runner reported in the order it reported it.

A redraw with no event behind it happens only where the picture it would draw has
changed, which means every fact a live row states is part of the value that
decides it: a segment left out of that value reaches the screen only when
something else on the row happens to change with it.

Each rate is owned by a probe under `src/bin/` with a floor and a
sustained-to-opening ratio, because the way this gets slow is not a constant
factor — it is a redraw that grows with what came before, and that is fast in the
first second and hopeless in the hundredth.

The floor is against the clock. The ratio is not, and may not be: a probe runs a
fixed yardstick beside the frames of every timed window and compares what a frame
cost in *those*. A machine that is not the same speed at the start of a run and
the end of one therefore says nothing, and only a frame that got dearer does.
