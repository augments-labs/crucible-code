# What the model is told

Every request crucible sends carries two kinds of text besides the conversation:
instructions, which say how to work, and facts, which say what is true of the
session right now. They travel separately, and only the facts that changed are
sent again.

## Instructions

The instructions are crucible's own — how to hold a task, read before changing,
and say where the work stands — in the tone you chose, plus whatever `append`
adds or `custom` puts in their place; all three are
[`systemPrompt`](../configuration/configuration.md#systemprompt) keys. They are
fixed when crucible starts and go in the provider's system field. Nothing about
the session enters them, so a new model, a new tool or the date rolling over
never rewrites them.

## Facts

The facts come in six sections, sent in this order:

| Section | What it says |
| --- | --- |
| Where you are working | The workspace root every tool path is relative to. |
| Skills you can open | The skills that can be opened. Crucible does not look for skills yet, so this says none were discovered. |
| Environment | The UTC date, the operating system and the processor architecture. |
| What is answering | The model's name and the effort it was asked at, or that the vendor's default applies. |
| What you have | The names of the tools advertised for this request. What each does travels in its own schema. |
| Permissions | The [permission mode](../permissions/modes.md) and the scopes you allowed for the rest of this session. |

The permissions section reports; it does not authorize. It ends by saying so,
and a call is still checked by the permission engine as if the section were not
there. Your [rules](../permissions/rules.md) are not in it. Each section is
bounded: at most 64 skills and 64 allowed scopes are named, each cut to 200
characters, and the rest are counted.

Each section is sent as a user-role message of its own, not in the system
field, and stays in the conversation like anything else that was said. They
are first sent with your first prompt, so the first request reads your
question, then the six sections.

## Only what changed

crucible keeps what it last told the model, section by section, and compares
before every request, including each one inside a turn:

- **Nothing changed**: nothing is sent.
- **A section changed**: only that section is sent, and it says what changed —
  `## Model changed` with the new model and effort after `/model`, or
  `## What you have changed` with the tools added and removed.
- **The model has never been told**: the whole section is sent. That is every
  section at the start of a session.

A turn that makes thirty tool calls under the same model, permissions and tools
sends the six sections once. Answering "Yes, and don't ask again this session"
partway through sends one short `## Permissions changed` before the next
request, and the request after that sends none. A mode change made while a turn
runs is [held for the next turn](../permissions/modes.md#stepping-it-while-you-type),
and is told to the model when that turn starts.

## After compaction

A [compaction](sessions.md#when-the-window-fills) replaces the middle of the
conversation with a recap, and the facts that were said there go with it. So
compaction removes every fact section, including any in the turns it keeps
whole, and the next request restates all six in full. A change note that
survived without the full statement it amended would leave the model knowing
only half of it.

## When a session is picked up

What the model was told is written to the session log as the words it was
sent, followed by a line recording the state behind them. `--continue` and
`--resume` read both back, so a resumed session carries on comparing against
what the model already knows: if nothing changed while it was closed, nothing is
restated; if the date moved on or you resume under another model, those
sections are sent as changes. The mode and any session-long allow
[are not carried over](../permissions/modes.md#--continue-resumes-the-transcript-not-the-mode),
so the model is told when they differ from what it last heard.

The words are written before the state. If crucible stops between the two, the
next request finds words it has no recorded state for and restates that section
whole, opening with a line that tells the model it supersedes every earlier
version. A session written before crucible recorded that state is told all six
in full.

## Why it is split this way

A provider can reuse the start of a request it has seen before, and
[prompt caching](../providers/prompt-caching.md) depends on that start staying
the same bytes. Instructions that never change come first; facts follow in
the conversation, with the ones most likely to change mid-session last.
Changing the model adds a short note at the end rather than rewriting the top.
