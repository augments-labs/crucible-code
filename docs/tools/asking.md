# Asking you

`ask_user` puts a question to you and waits. The agent uses it when the work
forks on something only you can settle — which of several shapes to build, which
of several directions to take — and guessing would put the whole turn's output
on the wrong side of the fork.

It is one of the two tools that reach nothing outside crucible: no file, no
process, nothing that leaves the machine. So there is no question about
permission and no rule to write about it.

| Argument | What it is |
| --- | --- |
| `questions` | Every question to put, in the order to answer them. Required, and at most 4. |
| `heading` | Two or three words naming one question, shown in a row of all of them. Required, and at most 24 bytes — the row holds every heading at once. |
| `question` | The question itself. Required, and at most 500 bytes. |
| `several` | Whether more than one of its answers may be chosen. Left out means one. |
| `answers` | The answers it offers, best first. Required, at least 2 and at most 8, and no two the same. |
| `answer` | What one answer is called. Required, and at most 200 bytes. |
| `says` | One line saying what choosing it means, where the name does not say it. |
| `shows` | What the answer would look like, row by row. At most 10 rows of at most 200 bytes each. |

## What you see

A panel where the prompt box was: the questions across the top, the one you are
on marked, then that question and its answers numbered under it.

```
╭──────────────────────────────────────────────────────────────╮
│  Questions for you                                           │
│                                                              │
│  › ✓ Language   □ Support   □ Status line   Review           │
│                                                              │
│  Which language should the examples be written in?           │
│                                                              │
│  › 1. Rust                                                   │
│       crucible's own implementation language                 │
│    2. Python                                                 │
│    3. Something else                                         │
│                                                              │
│  ──────────────────────────────────────────────────────────  │
│    4. Say it in the prompt instead                           │
╰──────────────────────────────────────────────────────────────╯
  esc to cancel · ←→ between questions · n for a note
```

A `□` is a question you have not answered and a `✓` is one you have, so the row
across the top is both where you are and how much is left. It appears only where
there is more than one question.

Where a question takes several answers, each one carries a bracketed mark of its
own — `[ ]` and `[✓]`. Bracketed on purpose: the row above says whether a
*question* is answered and these say whether an *answer* is chosen, and on that
question both are on screen at once.

Two answers are always there and the agent does not write them. **Something
else** is a line you type yourself, and **Say it in the prompt instead** leaves
the whole thing — which is what `esc` does too.

## The keys

| Key | What it does |
| --- | --- |
| `↑` `↓` | Moves down the answers. Stops at each end. |
| `←` `→` | Steps to the next question, or the one before. Stops at each end. |
| `enter` | Takes what is marked and moves on. On the last stop, sends. |
| `space` | Where several answers may be chosen: chooses the marked one, or unchooses it. |
| `1`–`9` | Takes that answer. |
| `n` | Adds a line of your own beside the answer. |
| `esc` | Stops typing, if you are. Otherwise leaves the whole thing unanswered. |

Going back to a question you have answered finds it as you left it, which is the
reason the arrows go both ways.

## Where an answer is a shape

Some questions are about what something will look like rather than what it is
called. Those answers carry a specimen, drawn under the one you are on:

```
│    1. Compact                                                │
│  › 2. With the workspace and the spend                       │
│                                                              │
│    ┌────────────────────────────────────────────────────┐    │
│    │ › what shall we do about the flaky test?           │    │
│    │                                                    │    │
│    │ crucible · opus-5 · main* · ~/src/crucible · 12.4k  │    │
│    └────────────────────────────────────────────────────┘    │
```

The box is the size of the largest specimen in that question and an answer with
nothing to show says so inside it, so the panel does not change height as you
move. A specimen runs to ten rows, and one longer is cut with a row saying how
much was left.

## Several questions at once

An ask of more than one question ends on a stop that reads every answer back
before it goes, so nothing is sent that you have not seen:

```
│  These are the answers that go back:                         │
│                                                              │
│  ✓ Which language should the examples be written in?         │
│      Rust                                                    │
│  ✓ Which of these should crucible support later?             │
│      Reading images, Pulling text out of PDFs                │
│                                                              │
│  Send them?                                                  │
│                                                              │
│  › 1. Send                                                   │
│    2. Cancel                                                 │
```

An ask of one question has no such stop — a screen reading back one answer says
what the screen before it said — unless that question takes several answers.
There, `enter` would otherwise mean both *choose this one* and *I am done*, and
a key meaning two things does the wrong one; the last stop is where being done
happens instead.

## Nobody answering is an answer

Leaving the ask is not an error and does not end the turn. The call comes back
saying nobody answered and to ask in the prompt instead, and the agent carries
on — usually by asking you the same thing in its own words, where you can reply
however you like.

The same is true where the window is too short to stand the panel in. The
questions are put into the scrollback a row at a time instead, one key each.

## Where it is not there

A run whose output is not a terminal has nobody to ask, so the tool is not
registered at all — not even for `tool_search` to find. Piping crucible's output
to a file is one of those runs.
