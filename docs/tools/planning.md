# Writing down the plan

`todo_write` puts down the plan the agent is working to, and you read it as a
panel above the prompt. It is the one tool that reaches nothing outside
crucible: no file, no process, and so no question and no rule.

| Argument | What it is |
| --- | --- |
| `tasks` | Every task, in the order they should be read. Required, and at most 64. |
| `task` | What one task is, in a line. Required, and at most 256 bytes. |
| `state` | Where that task is: `open`, `doing` or `done`. A task with no state is read as `open`, and at most one task in a plan may be `doing`. |

## A call puts down a whole plan

There is no argument for changing one task, because a call that named two tasks
to change would be a call about a plan the model cannot see. Every write
replaces what was there, so the model's idea of the plan and crucible's are one
value rather than two that part company the first time one of them is wrong.

Which is also why there is no tool for reading it back. The call answers with
the plan as it now stands, counts first and then a line per task:

```
1 done · 1 doing · 1 open
done: Build the gate script
doing: Run the validation spikes
open: Design the architecture
```

The counts are the line a transcript hangs under the call, so a reader who is
not going to open the result still learns what the plan did. A state nothing is
in is left out of them — `0 done` is a word and a number spent saying that a
plan nobody has finished anything in is a plan nobody has finished anything in.

An empty list is how a plan is put away. It answers `the plan is empty`, and the
panel comes down.

## Both bounds are refusals

A plan of ten thousand tasks and a plan with three of them under way are calls
the model can correct, so they come back as failed results rather than ending
the turn. Each says which bound was missed and by how much:

```
a plan holds at most 64 tasks and this one has 91: write down the work rather than every step of it
a task is at most 256 bytes and tasks[1] is 402: one line each
a plan has at most one task under way and this one has 2: leave one doing and mark the rest open or done
```

Sixty-four is far past the point a plan stops being one — a list that long is
the work itself rather than a plan for it. The figure is not what a reader can
take in; it is where a call has plainly stopped meaning to write a plan at all.
The plan that was there is the one still standing after any of the three: none
of a refused call is kept.

A word outside `open`, `doing` and `done` is the other kind of failure and ends
the turn, the way an unreadable argument does anywhere else —
`todo_write: tasks[0] state must be one of open, doing, done`. Reading it as
`open` would be the mistake the model cannot see.

## What you read

A rule with air on both sides of it, the counts, and a row per task:

```
────────────────────────────────────────────────────────────────

3 tasks (1 done · 1 doing · 1 open)
■ Run the validation spikes
□ Design the architecture
✓ Build the gate script
```

The mark says which state a task is in and the colour says it again, so a
terminal with no colour still has three different marks: `■` for the task under
way, `□` for one nobody has started, `✓` for one that is finished. Where the
terminal has colour, the one under way is the one warm mark on the screen and
the only row in a weight of its own, and a finished one is struck through and
toned down. [`glyphs`](../configuration/configuration.md#output) set to `ascii`
writes the three as `*`, `-` and `x`.

The order is the order the panel gives rows up in, which is one decision rather
than two: the task under way first, because *what is the agent on* is what the
panel is for; then what is open, as the plan wrote it; then what is finished,
most recently ticked off first.

## Seven rows, and the key that gives the rest back

The panel is read at a glance above a box somebody is typing into, so it shows
seven tasks and counts the rest on a line of its own:

```
… +4 more · ctrl+t to expand
```

<kbd>Ctrl+T</kbd> takes the bound off and puts it back. What it adds goes
*underneath* the rows already on screen, so nothing you were reading moves —
which is what makes the key worth pressing in the middle of a turn. Open, the
same line reads `ctrl+t to collapse`, and a plan that fits either way is offered
neither, since the press would do nothing. Where everything left over is
finished work the line says so — `… +4 completed` — because that is a different
thing from four tasks nobody has reached.

A window too short for the panel takes rows from the same end, and a window with
no room for the rule, the counts and one task between them has no panel in it at
all. The panel is measured before the rows the turn draws around it, so what a
short window drops first is the call line and the queued prompt rather than what
the agent is working to.

The panel is not written into the transcript. It stands in the rows above the
box, so a plan rewritten twenty times in one turn costs twenty redraws of the
same rows rather than twenty copies down the transcript — and it
stays there when the turn ends, because what the agent was working to is what
the next prompt is typed against.

## What it survives

A session resumed by [`/resume`](../sessions/index.md) opens with the plan it
stopped at. Nothing about a session file has to hold one: the call that wrote it
is already in the transcript being replayed, and the plan is read back out of
that call the same way the tool read it — so a call the tool refused is refused
again rather than seeding a plan that was never written.

`/clear` puts it away with the session it belonged to. A plan that outlived one
would be a panel above the prompt describing work the agent has no memory of.
