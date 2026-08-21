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

When a call comes down to asking, the turn stops and a panel takes the place of
the prompt box:

```
╭──────────────────────────────────────────────────────────────────────────────╮
│  Bash command                                                                │
│                                                                              │
│    cargo test --workspace --all-features                                     │
│    Run the suite before touching the module                                  │
│                                                                              │
│  This command needs your verdict.                                            │
│                                                                              │
│  Do you want to proceed?                                                     │
│  › 1. Yes, once                                                              │
│    2. Yes, and don't ask again this session                                  │
│    3. No, and end the turn                                                   │
╰──────────────────────────────────────────────────────────────────────────────╯
  esc to cancel · ctrl+e to explain
```

The line over the frame names the tool and what the call is about — a file for
`read`, `write` and `edit`, a command for `bash`. Inside the frame is the thing
being consented to. A file change names the file it would touch: the resolved
path, after symbolic links, spelled relative to the working directory when it
is inside it. A command shows the line as it was sent, operators and all,
because `a && b` is two commands *if the first one worked*, and consent to a
list of commands is consent to something nobody sent.

That line is folded to the window rather than cut, and control characters in it
become spaces, because the text is the model's to choose: a name with its tail
missing is one you agree to without having read it, and a control character
left in it moves the cursor inside a row nothing measured. Nothing runs while
the panel is on screen.

Under it, where the call gave one, is the model's own line saying what the call
is for. It is a caption on the command and not a claim standing beside it —
there is no blank between them, which is what says whose sentence it is about.
Read it as what the model *says* it is doing; what it *would* do is the line
above, and that one was read out of the arguments rather than written. The
caption is cut to one row however much arrived, so nothing off-screen decides
how tall this panel is, and a call that offered none leaves the row out
entirely. It is spelled `description`, and the tools that invite one are the
three whose calls stop here: [`bash`](../tools/commands.md),
[`write` and `edit`](../tools/files.md). A read is allowed or refused without
being asked about, so there would be nowhere to draw one.

`↑` and `↓` move the mark, and stop at each end rather than wrapping. `enter`
takes the answer the mark stands on, and `1`, `2` and `3` take theirs directly.
`esc` refuses, and `ctrl+e` opens [what the call said about
itself](#the-long-version) where it said more than a line.

Where there is no keyboard — a redirected run — or no room to stand a panel in,
the same question is asked a row at a time and the answer is typed:

```
? bash wants to run: cargo test
  [y]es  [s]ession  [n]o ›
```

What each leaves in the transcript is what was on screen. The typed question
was written there, so it stays there, and the letter typed under it stays with
it. The panel stood in the rows above the box and those rows were given back,
so it leaves nothing — not the question, and not the answer either. What a yes leaves
behind is the call's own result on the row beneath it, which is the row the
reader was waiting for; a line saying "Yes, and don't ask again this session"
between a call and its result says nothing the result does not.

A no leaves its trace the same way, because a refusal comes back as that call's
result and is drawn as one. So the record is the same shape however the verdict
was reached, and the same shape as one where nothing was asked at all.

## The long version

The same three tools invite paragraphs as well, in `explanation`. The footer
names the key where a call sent them, and `ctrl+e` opens them under the command
or the path:

```
╭──────────────────────────────────────────────────────────────────────────────╮
│  Bash command                                                                │
│                                                                              │
│    cargo test --workspace --all-features                                     │
│    Run the suite before touching the module                                  │
│                                                                              │
│    bash's own account of this call:                                          │
│                                                                              │
│    Runs every test in the workspace, including the ones behind a             │
│    feature flag. It reads the tree and writes under target/ only, so         │
│    · 5 more rows of explanation                                              │
│                                                                              │
│  This command needs your verdict.                                            │
│                                                                              │
│  Do you want to proceed?                                                     │
│  › 1. Yes, once                                                              │
│    2. Yes, and don't ask again this session                                  │
│    3. No, and end the turn                                                   │
╰──────────────────────────────────────────────────────────────────────────────╯
  esc to cancel · ctrl+e to hide · ↑↓ to see more
```

The row over them says whose words they are, and it is crucible's rather than
the model's. Everything else inside the frame was read out of the arguments;
this is the one part of the panel the thing asking for the permission wrote,
and it is the case *for* the call, made by whatever wants it run. Worth
reading, and not evidence: what would actually happen is the line at the top.

The paragraphs are the first thing on the panel to give way. The command, the
sentence and the three answers are its floor and are never traded for room, so
where the window is short it is the prose that gets less of it — down to none,
which is a call whose explanation cannot be opened in a terminal that size.
Where they run past the room there is, the last row of them counts what is
underneath, `↑` and `↓` move the window, and the count falls as you go until
the end, where the row goes back to being a paragraph. `ctrl+e` again closes
them.

Where the paragraphs fit whole there is nothing to scroll, so the arrows go on
moving the mark. And a call that sent none gets the panel that was there before
any of this, footer included — the key is named where it does something, and
nowhere else.

## The three answers

| The answer | Typed | What happens |
| --- | --- | --- |
| Yes, once | `y`, `Y`, `yes` | Runs, this once. The next call like it asks again. |
| Yes, and don't ask again this session | `s`, `S`, `session` | Runs, and stops asking for calls like this one until crucible exits. |
| No, and end the turn | anything else | Does not run. |

**Anything else** is the typed question's third answer, and it means exactly
that: `n`, `no`, an empty line, a typo, or the input ending. There are two ways
to say yes and both are explicit; everything that is not one of them leaves the
tool unrun. At the panel there is no anything-else — every key that answers is
one of the three, and `esc` is how to refuse without choosing one.

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
