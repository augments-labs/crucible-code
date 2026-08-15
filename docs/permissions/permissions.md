# Permissions

Nothing changes a file or starts a process without a decision. The decision
comes from three places, tried in order: [rules](rules.md) you wrote speak
first, the [mode](modes.md) answers for calls no rule mentions, and when what
they settle on is "ask", the question below appears. With nothing configured,
that is every change and every command. Reading is the exception in every mode:
a read is allowed, or refused by a rule, and never asked about.

The row under the prompt box always shows the mode in force, and names the key
that steps it. At the far end of the same row is what the next turn is asked of:

```
╭──────────────────────────────────────────────────────────────────────────────╮
│ ›                                                                            │
╰──────────────────────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)                anthropic/claude-sonnet-5 · high
```

It is there every time rather than said once at the top, because the moment it
matters is hours in, when the top has scrolled away. A full-access session must
never be distinguishable from an asking one only by what you remember starting.
Where there is no box — a redirected run — the mode is written in front of the
prompt instead, spelled the way configuration spells it: `ask › `.

## The question

When a call comes down to asking, the turn stops:

```
? write wants to change: src/main.rs
  [y]es  [s]ession  [n]o ›
```

```
? bash wants to run: cargo test
  [y]es  [s]ession  [n]o ›
```

A file change names the file it would touch — the resolved path, after
symbolic links, spelled relative to the working directory when it is inside
it. A command shows what is about to run, and one written as several commands
shows all of them: `git add ., then git commit`. Either line is clipped to fit
and control characters in it become spaces, because the text is the model's to
choose: a newline left in it would commit a second row, and the question you
answer would be one the model wrote rather than the one crucible asked.
Nothing runs while the question is on screen.

## The three answers

| You type | What happens |
| --- | --- |
| `y`, `Y`, `yes` | Runs, this once. The next call like it asks again. |
| `s`, `S`, `session` | Runs, and stops asking for calls like this one until crucible exits. |
| anything else | Does not run. |

**Anything else** means exactly that: `n`, `no`, an empty line, a typo, or the
input ending. There are two ways to say yes and both are explicit;
everything that is not one of them leaves the tool unrun.

`s` is the answer for a command you will run twenty times this afternoon and
never think about again. Durable standing policy is edited as a rule with the
configuration file open, where its scope can be reviewed before it takes
effect.

## Two kinds of no

Your no ends the turn. The refusal is written into the transcript as that
call's result, so the transcript stays one a provider will accept and the
model sees the refusal on your next prompt — but it does not carry on and try
something else within the turn you stopped. That is deliberate: a model that
can keep going after a no gets to ask the same question in a different shape
until one of them is answered yes.

A `deny` rule's no does not end the turn. A rule is standing policy: the call
comes back to the model as a failed result saying the policy will not change,
and the turn carries on. That costs you nothing — a retry hits the same wall
without a question — while ending the turn on a rule match would let one stray
call throw away a piece of work. In a sentence: a rule stops a call, you stop
a turn.

## What "calls like this one" means

Both durations remember exactly what the question named.

- For a file change, it is the tool. `session` on a `write` stops asking about
  `write`, and `edit` is still asked about separately. Both are already
  confined to the [directories the session reaches](directories.md), so
  remembering the name does not widen what they can touch.
- For a command, it is the tool **and the whole command**, with runs of
  whitespace collapsed. `session` on `cargo test` stops asking about
  `cargo test`; `cargo build` — same program, different command — asks again.
  Standing permission for a family of commands is a job for an
  [allow rule](rules.md), which is written down where you can read it back.

A session-long allow lives as long as the process that made it and is never
written to disk, so `--continue` starts with none — resuming a session does
not resume its permissions, and the mode comes fresh from configuration at
every start.

## Durable rules

A lasting `allow` is written deliberately in `~/.crucible/config.json`, outside
the checkout. Neither workspace configuration filename can carry authority:
an ignored-by-convention file is still a filename a repository can commit, and
crucible cannot distinguish the two sources safely. Project files may add
`ask` and `deny` policy because both narrow what can happen.

## The files no tool may write

No tool may change the files permissions are configured from: `config.json`
and `config.local.json` inside any directory named `.crucible`, including the
one in your home directory. Not in any mode, and not under any rule — a single
write there could put an allow for everything into the next start, so that
refusal cannot be entrusted to the rules and modes it would defeat. Reading
them stays ordinary; it is how a session begins.

## The guarantee underneath

A verdict is not advice that a tool may take or leave. Inside crucible, running
any tool at all takes proof that a decision was made and that it was yes, and
there is no way to construct that proof except by reaching a verdict first.
Code without one cannot call a tool, which is why this is a property of the
program rather than a rule contributors remember.

Reading is not an exception to that; it is a question answered without being
asked. The permission engine mints the proof for a read-only call itself
instead of putting it to you — after the rules have spoken, which is how a
`deny` still reaches a read — so a tool that reported the wrong sensitivity
would be one that got the wrong question, never one that skipped the check.
