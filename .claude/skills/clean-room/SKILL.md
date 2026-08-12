---
name: clean-room
description: >-
  What you may read while writing crucible and what you may not. Use before
  looking at another coding agent for reference, before copying a prompt, a UI
  string, an escape sequence or an asset, and whenever a change reproduces
  another tool's behaviour closely.
---

# Clean room

This repository is public, MIT-licensed, and claims on the first line of
`CLAUDE.md` to be original. That claim is what the line here protects — and the
line falls between an *idea* and the *expression* of it.

Copyright covers expression. It does not cover a feature, a capability, a
workflow or a method of operation. That a harness offers a `/login` command,
keeps a Python interpreter alive between turns, or reaches merge conflicts
through a URI scheme is a fact about the world: learning it costs nothing and
infringes nothing. The sentence somebody wrote to describe it is theirs.

So the rule is not "do not look". It is:

**Learn what another harness does. Never reproduce what it wrote.**

## Read freely

- **Standards and specifications.** ECMA-48, the XTerm control sequences
  document, OSC and CSI references, the Anthropic and OpenAI HTTP API docs, JSON
  Schema, SemVer, Keep a Changelog.
- **The Rust standard library and its docs**, and the docs of any crate already
  in `[workspace.dependencies]`.
- **Terminal emulator documentation** — what Ghostty, kitty, WezTerm, Windows
  Terminal or tmux say they support.
- **What another harness publishes about itself** — README, documentation site,
  changelog, release notes, issue tracker, recorded talks. This is where "what
  features exist" lives, and it is published in order to be read.
- **Another harness running.** Installing it, using it, timing it and writing
  down what it did is observation, and an observation is yours.

## Do not reproduce

No code, prompt text, system prompt, tool description, UI string, help text,
error message, ASCII art, icon or colour palette from any other harness —
whether it reaches you from a repository, a decompiled binary, a leaked dump, a
screenshot, or a blog post quoting it verbatim.

Adapting counts. Renaming the variables in someone else's prompt is still their
prompt; paraphrasing someone else's error message is still their error message.

A permissive licence does not change this. oh-my-pi is MIT and would let you
copy with attribution. crucible still does not, because what is at stake is the
claim at the top of `CLAUDE.md`, not a licence obligation.

## Source and prompt files stay closed

Another harness's source code and prompt files are the one category still shut,
and the reason is different from everything above. Reading them is not unlawful.
It is that *access* cannot be undone. "Clean-room" means the implementers had no
access to the original's source; it is worth more than any single technique you
would pick up, and once it is spent for one contributor it is spent for good.

The practical cost is small, because what you actually want from another harness
is almost always *what it does* — and that is in its README.

If a capability genuinely cannot be understood from published behaviour, that is
a signal to derive it from the spec and your own measurements, not a licence to
open the tab.

## The line, in practice

| Question | Answer |
| --- | --- |
| "Does oh-my-pi keep a Python kernel alive between turns, and should we?" | Read its README and decide. A capability is a fact. |
| "How does it keep that kernel alive?" | Work it out from the spec, the crate docs and your own measurements. Do not open its source to find out. |
| "Claude Code writes `ESC ]0;…BEL` for its tab title — may I?" | Yes. That is OSC 0 from the terminal spec; every program that sets a title emits it. The *string inside it* must be crucible's own. |
| "May I match its keybindings?" | Yes for conventions the terminal already owns — Ctrl-C, Ctrl-D, arrow history. No for a scheme that is recognisably one product's design. |
| "May I use the same crate it uses?" | Yes. A public crate is a public crate. |
| "It is MIT — may I vendor one function?" | No. The licence permits it; the claim at the top of `CLAUDE.md` does not. |
| "I want the same welcome banner shape." | No. Draw crucible's own. |
| "Their docs name a feature well. May I use that name?" | A common noun, yes — `checkpoint`, `subagent`, `resume` are the vocabulary of the problem. A coined phrase that is recognisably theirs, no. |

## If you have already seen something

Say so, plainly, before you write that part — in the pull request, or in the
conversation. It is not a disciplinary matter and it is not fatal. It means that
part gets written by somebody else, or gets written from the spec with the
memory deliberately set aside, or gets written and then read back against what
you saw.

What is fatal is finding out afterwards.

## Where original work is expected to look similar

Terminal agents converge because terminals constrain them. A prompt line, a
streaming transcript, a permission question and a tool result block are the
shape of the problem, not anybody's invention. Convergent solutions are fine.
Convergent *text* is not — write every user-visible string as if the reader has
never used another agent.
