# Permissions

Nothing changes a file or starts a process without a decision. The decision
comes from three places, tried in order: [rules](rules.md) you wrote speak
first, the [mode](modes.md) answers for calls no rule mentions, and when what
they settle on is "ask", the question below appears. With nothing configured,
that is every change and every command. Reading is the exception in every mode:
a read is allowed, or refused by a rule, and never asked about.

The row under the prompt box always shows the mode in force, and names the key
that steps it:

```
╭────────────────────────────────────────────────────────────╮
│ ›                                                          │
╰────────────────────────────────────────────────────────────╯
ask mode on (shift+tab to cycle)
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
  [y]es  [s]ession  [a]lways  [n]o ›
```

```
? bash wants to run: cargo test
  [y]es  [s]ession  [a]lways  [n]o ›
```

A file change names the file it would touch — the resolved path, after
symbolic links, spelled relative to the working directory when it is inside
it. A command shows what is about to run, and one written as several commands
shows all of them: `git add ., then git commit`. Either line is clipped to fit
and control characters in it become spaces, because the text is the model's to
choose: a newline left in it would commit a second row, and the question you
answer would be one the model wrote rather than the one crucible asked.
Nothing runs while the question is on screen.

## The four answers

| You type | What happens |
| --- | --- |
| `y`, `Y`, `yes` | Runs, this once. The next call like it asks again. |
| `s`, `S`, `session` | Runs, and stops asking for calls like this one until crucible exits. |
| `a`, `A`, `always` | Runs, stops asking, and [writes the rule down](#what-always-writes) so the next session starts with it. |
| anything else | Does not run. |

**Anything else** means exactly that: `n`, `no`, an empty line, a typo, or the
input ending. There are three ways to say yes and all of them are explicit;
everything that is not one of them leaves the tool unrun.

The two durations are separate words because they are separate promises, and
one of them costs a file. `s` is the answer for a command you will run twenty
times this afternoon and never think about again; `a` is the answer for one you
will still be running next month.

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

## What `always` writes

`always` is the same allow, put where the next start will find it. The line
under the question says where it went:

```
· remembered bash(cargo test) in /home/you/api/.crucible/config.local.json
```

That is an `allow` [rule](rules.md), and it is the narrowest one that covers
the call: the tool, and exactly what the question named, spelled so that it
matches itself and nothing else. A `*` in the command or the filename is
escaped rather than left to widen the rule — `rm *.tmp` is remembered as the
command you saw, not as `rm` on anything ending in `.tmp`.

It goes into `.crucible/config.local.json`, the layer
[git ignores](../configuration/configuration.md#the-file-that-travels), so an
answer you gave on your machine stays on it and does not reach everyone who
clones. Everything already in that file is left where it was, byte for byte,
including the parts crucible has no setting for.

Since it is an ordinary allow rule in an ordinary file, taking the permission
back is opening the file and deleting the line.

### When there is nothing to write

Some calls have no rule that describes them. A command line that is several
commands, or one whose text does not say what will run — a substitution, an
expansion, a redirection, a background `&`, a leading `VAR=value`, a
[wrapper program](allowing.md) — could only be written down as something wider
than what you were asked about. So `always` is not offered:

```
? bash wants to run: git add ., then git commit
  [y]es  [s]ession  [n]o ›
```

Typing `a` at that question is a word the prompt has no answer for, and
anything the prompt has no answer for does not run. `s` still works: a session
remembers the call itself and needs no rule text.

### When the file cannot be written

```
! bash(cargo test) was not remembered: /home/you/api/.crucible/config.local.json is not valid JSON at line 2, column 1
```

The call still runs and the session still stops asking about calls like it.
What is lost is the part that outlives the process — so the rule is printed
exactly as it would have been written, and adding it by hand is a copy and a
paste.

## The files no tool may write

No tool may change the files permissions are configured from: `config.json`
and `config.local.json` inside any directory named `.crucible`, including the
one in your home directory. Not in any mode, and not under any rule — a single
write there could put an allow for everything into the next start, so that
refusal cannot be entrusted to the rules and modes it would defeat. Reading
them stays ordinary; it is how a session begins.

`always` writes to one of those files, and is not an exception to this: it is
not a tool call. The model can ask for a command; it cannot ask for a rule, and
the only thing that puts one in the file is the letter you typed at a question
about a call you were shown.

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
