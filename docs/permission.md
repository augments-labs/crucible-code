# Permission

Reading never asks. Changing a file or starting a process always does.

When the model wants to do either, the turn stops and the question appears:

```
? write wants to change a file: {"path":"src/main.rs","text":"fn main() {…
  [y]es  [a]lways  [n]o › 
```

```
? bash wants to run: cargo
  [y]es  [a]lways  [n]o › 
```

A file change shows the arguments as the model wrote them, clipped to fit. A
command shows the program it is about to start. Nothing runs while the question
is on screen.

## The three answers

| You type | What happens |
| --- | --- |
| `y`, `Y`, `yes` | Runs, this once. The next call like it asks again. |
| `a`, `A`, `always` | Runs, and stops asking for calls like this one until the session ends. |
| anything else | Does not run. |

**Anything else** means exactly that: `n`, `no`, an empty line, a typo, or the
input ending. There are two ways to say yes and both are explicit; everything
that is not one of them leaves the tool unrun. A denied call is not a failed
turn — the model is told it was not allowed and carries on from there.

## What "calls like this one" means

`always` remembers a shape, not a string.

- For a file change, it is the tool. `always` on a `write` stops asking about
  `write`, and `edit` is still asked about separately. Both are already confined
  to the workspace, so remembering the name does not widen what they can reach.
- For a command, it is the tool **and the program**. `always` on `cargo test`
  stops asking about `cargo`; a later `curl` still asks. A session-wide grant to
  run anything at all is exactly the hole this avoids.

A grant lives as long as the process that made it and is never written to disk,
so `--continue` starts with none — resuming a conversation does not resume its
permissions.

## The guarantee underneath

A verdict is not advice that a tool may take or leave. Inside crucible, a
function that mutates a file or spawns a process takes proof that a decision was
made and that it was yes; there is no way to construct that proof except by
reaching a verdict first. Code without one cannot call those tools, which is why
this is a property of the program rather than a rule contributors remember.
