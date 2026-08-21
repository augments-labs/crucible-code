---
paths:
  - "crates/crucible-tui/**"
  - "src/cli/draw.rs"
  - "src/cli/draw/**"
  - "src/cli/converse/**"
---

# Every component is drawn for the window it is drawn on

Not for eighty columns, not for the window the process started in, and not for
the one the last frame was drawn on. A component takes the size as an argument
and lays itself out against it; every caller hands it the size the terminal has
now.

Two halves. A component is responsive only where it keeps both.

## It fits the size it was given

A component answers with rows. No row is wider than the columns it was handed,
and one handed a room answers with no more rows than that room holds. Why
measuring is this crate's job at all is next door in `tui-render.md`, and is not
repeated here.

What a component does with a window too small for it is its own business — give
up spacing, then prose, then rows, and at the end draw nothing at all. What it
may not do is draw something that does not fit and leave the terminal to wrap
it: a band is a fixed rectangle of rows, so a row given one takes two, and the
second comes out of the band drawn under it.

The failure is always the same shape and never looks like one. A component folds
its prose correctly at every width and then puts down a single sentence it built
with `format!` and never clipped — a footer, a count of what was cut, the words
for an empty box. It is right at eighty columns and past the edge at twenty.

`crates/crucible-tui/src/fits.rs` holds every component to this at once, at
every width and at every height one of them decides anything at. A new component
goes into that file in the commit that writes it, and the gate fails until it
does. It is a floor: that a picture *fits* is a different question from whether
it is *right*, and the second is answered beside the component, where its own
layout is tested.

## The size changes under it

Raw mode is what reports that: a resize arrives as `Pressed::Resized` on the
same queue as a key, which is why the loops that read keys are the ones that act
on it, and why the size is otherwise read only where
`crates/crucible-tui/src/terminal/system.rs` says it is. That module says what a
read costs. A poll on the beat is not a second way to notice a resize — it is
blocking I/O on the path the frame budget bounds, and `performance-budgets.md`
refuses it.

**A resize is news about the window, not a key aimed at whatever is standing.**
So a loop acts on it before it offers the press to a view, a list or a box.
Everything standing lays its rows out against the size the renderer holds; hand
one the press first and it lays itself out for a window that has gone, and goes
on being drawn at that size until something else changes it.
