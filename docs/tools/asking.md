# Asking you

`ask_user` puts a question to you. The agent uses it when the work forks on
something only you can settle (which of several shapes to build, which of
several directions to take) and guessing would put the whole turn's output on
the wrong side of the fork.

It asks in one of two ways. By default it waits: the turn stops on a panel
until you answer or leave it. Where the answer is needed later and there is
work to do that does not depend on it, the agent can instead
[ask without waiting](#asking-without-waiting): the questions wait above the
prompt box, the turn carries on, and your answer reaches the agent when you
send it.

It is one of the four tools that reach nothing outside crucible: no file, no
process, nothing that leaves the machine. So no mode asks about it, and a rule
can only be about the whole tool: `deny ask_user` refuses every call to it.

| Argument | What it is |
| --- | --- |
| `questions` | Every question to put, in the order to answer them. Required, and at most 4. |
| `heading` | Two or three words naming one question, shown in a row of all of them. Required, at most 24 bytes, because the row holds every heading at once, and on one line, with no line break or control character. |
| `question` | The question itself. Required, and at most 500 bytes. |
| `several` | Whether more than one of its answers may be chosen. Left out means one. |
| `answers` | The answers it offers, best first. Required, at least 2 and at most 8, and no two the same. |
| `answer` | What one answer is called. Required, at most 500 bytes, and on one line, with no line break or control character, since what you choose is said back under it. |
| `says` | What choosing it means, where the name does not say it. At most 500 bytes. |
| `recommended` | Marks the one answer the agent would pick. At most one per question, and the first listed; a call that marks two, or one that is not first, is refused so the agent can correct it. |
| `shows` | What the answer would look like, row by row. At most 10 rows of at most 500 bytes each. |
| `mode` | `blocking`, the default, waits for the answer. `deferred` returns at once and the answer comes later; see [Asking without waiting](#asking-without-waiting). |

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

In the list of answers, the one the agent would pick is the first, and its name
ends with `(Recommended)`. The label is only drawn: it is not part of the name
you choose or that is sent back, so the read-back of several answers shows the
name alone.

Where a question takes several answers, each one carries a bracketed mark of its
own, `[ ]` and `[✓]`. Bracketed on purpose: the row above says whether a
*question* is answered and these say whether an *answer* is chosen, and on that
question both are on screen at once.

Two answers are always there and the agent does not write them. **Something
else** is a line you type yourself, sent after whatever else you chose where
several answers may be chosen. **Say it in the prompt instead** leaves the whole
thing, which is what `esc` does too.

## The keys

| Key | What it does |
| --- | --- |
| `↑` `↓` | Moves down the answers. Stops at each end. |
| `←` `→` | Steps to the next question, or the one before. Stops at each end. |
| `enter` | Takes what is marked and moves on, or sends where there is nowhere left to move on to. On **Something else** with nothing written yet, it opens the line to write in instead. Where several answers may be chosen, it moves on without choosing the marked one: `space` does the choosing. |
| `space` | Where several answers may be chosen: chooses the marked one, or unchooses it. |
| `1`–`9` | Does what `enter` would on the answer with that number. Where several answers may be chosen, it chooses or unchooses that one instead, as `space` does. The row under the rule leaves the whole thing, when its number is 9 or lower. |
| `n` | Adds a line of your own beside the answer. |
| `esc` | Stops typing, if you are. Otherwise leaves the whole thing unanswered. |

Going back to a question you have answered finds it as you left it, which is the
reason the arrows go both ways.

## Where an answer is a shape

Some questions are about what something will look like rather than what it is
called. Those answers carry a specimen, drawn beside the answers where the
window holds both whole, every answer unfolded and the widest specimen uncut:

```
│    1. Compact                     ┌────────────────────────────────────┐  │
│  › 2. With the workspace          │ › what shall we do about the test? │  │
│    3. Nothing at all              │                                    │  │
│                                   │ crucible · opus-5 · main* · 12.4k  │  │
│                                   └────────────────────────────────────┘  │
```

A window too narrow for that draws the box under the answers instead, at the
full width, where it keeps most of the picture.

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

An ask of one question has no such stop (a screen reading back one answer says
what the screen before it said) unless that question takes several answers.
There, `enter` would otherwise mean both *choose this one* and *I am done*, and
a key meaning two things does the wrong one; the last stop is where being done
happens instead.

## Nobody answering is an answer

Leaving the ask is not an error and does not end the turn. The call comes back
saying nobody answered and to ask in the prompt instead, and the agent carries
on, usually by asking you the same thing in its own words, where you can reply
however you like.

The same is true where the window is too short to stand the panel in. The
questions are written into the transcript a row at a time instead, one key
each.

## Asking without waiting

With `mode` set to `deferred`, the call comes back at once saying the questions
were asked as a batch with an id, such as `q0`, and that the answer will come
later. The agent goes on with what does not depend on it. Whenever the set of
waiting batches changes, it is told which are still waiting, so it neither asks
again nor acts on an answer it has not had.

The questions wait in a row over the prompt box, which you can leave there
while you type:

```
□ 1 question waiting · Language · /questions
╭──────────────────────────────────────────────────────────────────────────────╮
│ ›                                                                            │
╰──────────────────────────────────────────────────────────────────────────────╯
```

`/questions` opens them, and runs again to close them; with nothing waiting it
says `no questions are waiting`. Open, the dock shows the oldest batch, a
question at a time, with every answer marked `( )` until you pick one, or `[ ]`
where several may be chosen:

```
╭──────────────────────────────────────────────────────────────────────────────╮
│  □ 1 question waiting                                                        │
│                                                                              │
│  › □ Language                                                                │
│                                                                              │
│  Which language?                                                             │
│                                                                              │
│  › 1. ( ) Rust                                                               │
│    2. ( ) Python                                                             │
│    3. ( ) Write your own answer                                              │
│                                                                              │
│  ──────────────────────────────────────────────────────────────────────────  │
│    4. ( ) Decline question                                                   │
╰──────────────────────────────────────────────────────────────────────────────╯
  enter choose · n note · tab composer · esc close
```

The row under the dock names the keys that work there. Where the window is
short, the dock gives up its blank rows before that row; where it is narrow,
that row folds between two keys before it cuts any key short.

Nothing in the dock moves by itself. New questions arriving add to the count
and change neither what you are on nor what you have written, and the turn
goes on under the dock while it is open.

### The keys in the dock

| Key | What it does |
| --- | --- |
| `↑` `↓` | Moves down the answers, or the actions on the last stop. |
| `←` `→` | Steps to the next question in the batch, or the one before. |
| `enter` | Chooses the marked answer and moves on. Where several answers may be chosen, it moves on and `space` does the choosing. On **Write your own answer** with nothing written yet, it opens a line to write in; once a line is written, it takes it and moves on. While you are writing the line, it only closes it. |
| `space` | Where several answers may be chosen: chooses the marked one, or unchooses it. |
| `n` | Adds a note of your own beside the answer. |
| `tab` | Moves the keys between the dock and the prompt box, so you can type to the agent with the questions still open. The row under the dock says `tab questions` while the prompt box has them. |
| `pgup` `pgdn` | Scrolls a question or a read-back taller than the room for it. |
| `esc` | Leaves the line you are writing, with what you wrote kept. While a decline asks once more, it keeps the question. Anywhere else it closes the dock. |

Closing the dock is not declining. Every answer you picked and every line you
wrote stays where you left it, and `/questions` opens the dock on it again.

**Decline question** is how you say you will not decide one. It asks once more
before it counts, with the row under the dock reading
`enter to decline this question · esc keep it`. A declined question goes back
as declined with nothing beside it.

### Sending the answers

A batch ends on a stop that reads every answer back, **Review before you
submit**, with **Submit batch**, **Keep editing** and **Close** under it.
Nothing is sent before you choose **Submit batch**, and a batch is taken whole
or not at all: each of its questions answered or declined.

Once it is taken, the dock says `Waiting for the model's next step.` and the
answer reaches the agent as a message from you that names its batch. If the
agent is in the middle of a turn, that turn picks it up; if nothing is running,
sending it starts the turn that does.

A batch that cannot be taken says why, and keeps everything you wrote:

```
│  ✗ Answer not accepted                                                       │
│  question 1 takes one answer and was given 0                                 │
│  Your draft is still here.                                                   │
```

**Retry submission** sends it again, **Keep editing** goes back to the
questions and **Close** leaves it waiting.

### What is kept

- **Bounds.** At most 8 batches wait at once, counting any already answered
  that the agent has not yet been given, holding at most 64 KiB of question
  text between them. A call that would go over either is refused, and the
  agent is told to ask with `mode` `blocking` instead. A note or a line you
  write yourself is at most 4 KiB; a longer one is refused when you send it,
  with the draft kept.
- **Interrupting a turn** leaves the questions waiting. **`/clear`** starts
  again with none.
- **Across a restart.** The questions are kept in the session, so `--continue`
  or `--resume` brings them back, once the first turn of the resumed session
  starts. What you had picked or written in the dock is not kept. An answer
  that was taken but had not yet reached the agent when crucible stopped is
  not kept either: its batch is waiting again, and you answer it again.
- **Nobody to hold them.** Where the run has nowhere to keep questions for
  later, the agent cannot ask without waiting, and the call is refused rather
  than waiting instead. A resumed session whose questions cannot be shown is
  told they will not be answered, and that it should ask again if it still
  needs to.

## From a program with no terminal

A front end with no terminal, driving crucible through its client protocol,
answers the same questions. The protocol is revision 6. A front end that holds
questions for later is offered `ask_user` without a terminal. A request that
claims the `questions` capability is told what waits by a `waiting` progress
report carrying an `era` and the waiting batches. An answer that shows a
specimen carries its rows, in order, under `shows`, each a `text` with its own
`truncated` flag, there and on a question a turn stops on, so the front end
can draw what the terminal draws; an answer showing none has no `shows`. The
front end answers one batch with an `answer` command:

```json
{"kind":"answer","era":{"kind":"session","session":"…"},"id":"q0",
 "replies":[{"kind":"answered","chosen":["Rust"],"note":""}]}
```

Each reply is `answered`, with the answers chosen and a note, or `declined`.
The `era` is the one the batch was reported under: the session's id where the
session is recorded, otherwise `{"kind":"stint","stint":n}`. Batch ids repeat
from one session to the next, and the era is what tells them apart. An answer
under another session's era is refused, and an answer in the same session,
resumed, is accepted once its first turn has brought the questions back;
before that it is refused as stale.

Two codes belong to answers, and neither settles anything:

| Code | Means |
| --- | --- |
| `stale_questions` | The command is refused, because nothing would deliver the answer: the batch was never asked, it was answered or cleared already, it belongs to another era, or the conversation it was asked in has ended. |
| `invalid_answer` | The command's outcome is `invalid`, with a problem under this code, rather than a refusal. The batch is waiting and the replies do not fit it: the wrong number, or one its question cannot take. The batch still waits, and the problem's `message` says what did not fit. |

A request that did not claim `questions` is sent nothing of what waits, and an
`answer` without it is refused as `needs_capability`.

A `prompt` command may carry `"by":"application"` to say the application wrote
it rather than the person, as for a note waking the agent when commands left
running have ended. It is taken as a turn like any prompt, but is never kept as
[what you asked for](../sessions/context.md#your-request).

## Where it is not there

A run that is not a terminal at both ends has nobody to ask, so the tool is not
registered at all, not even for `tool_search` to find. Piping crucible's output
to a file is one of those runs, and so is piping its input in from another
program while it draws in a terminal. A program driving crucible through its
client protocol is the exception: one that holds questions for later is offered
the tool, as above.
