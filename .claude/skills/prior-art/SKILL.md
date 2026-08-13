---
name: prior-art
description: >-
  What you may take from another coding agent and what you may not. Use before
  reusing a prompt, a help page or an asset, whenever a change reproduces
  another tool's behaviour closely, and after reading another harness closely
  enough that you could reproduce it. It also says which strings are too short
  to be in scope.
---

# Prior art

This repository is public, MIT-licensed, and claims on the first line of
`CLAUDE.md` to be original. That claim is what the line here protects — and the
line falls between an *idea* and the *expression* of it.

It used to fall somewhere else. Until v0.0.10 this rule also kept another
harness's source shut, to defend a stronger claim: clean-room, meaning nobody
who wrote crucible had read the original. That claim is gone, deliberately, and
the reason it went is worth keeping: refusing to read prior art costs real
understanding, and it bought a defence against a risk — a copyright claim — that
was never the likely one. The likely risk is reproducing something without
noticing, and a closed door was only ever a crude guard against that. The guard
is now explicit, at the foot of this page.

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
- **Another harness's source.** How it works is knowledge, and knowledge is
  what you are there for. Read it to understand a protocol, a failure mode, a
  shape you cannot derive from the spec. Then see "After reading closely".

## Do not reproduce

No code, prompt text, system prompt, tool description, help or documentation
page, ASCII art, icon or colour palette from any other harness — whether it
reaches you from a repository, a decompiled binary, a leaked dump, a screenshot,
or a blog post quoting it verbatim.

Adapting counts. Renaming the variables in someone else's prompt is still their
prompt; reordering the paragraphs of someone else's help page is still their
help page.

A permissive licence does not change this. oh-my-pi is MIT and would let you
copy with attribution. crucible still does not, because what is at stake is the
claim at the top of `CLAUDE.md`, not a licence obligation.

## Short functional strings are not in scope

`press ctrl+c again to exit`. `no such file`. `1 file changed`. The name of a
key, a flag or a command. These are outside the rule, and were never really
inside it: copyright does not reach a phrase that short, there is only so much
room to say what a keystroke does, and the check nobody can run is whether two
programs arrived at eight obvious words separately.

The cost of pretending otherwise falls on the reader. Somebody who has used a
terminal for twenty years knows what `press ctrl+c again to exit` means, and
making them read our paraphrase of it buys nothing but our own scruple.

So: where the words are forced, use the words. Where there is a choice worth
making — a welcome screen, an error that has to explain something, anything long
enough to have a voice — crucible makes its own. That second half is taste
rather than law, and taste loses to the reader when the two disagree.

The boundary is length and function together. Eight words naming a keystroke is
a fact. A paragraph explaining why a permission was refused is writing, and
writing is what the section above is about.

## After reading closely

Reading is allowed. Carrying the text away is not, and the difficulty is that
the two feel identical from the inside — you do not notice the sentence you
absorbed, which is exactly why it survives into your own writing.

So the rule that replaced the closed door is a spoken one. Having read
something closely enough that you could reproduce it, say so *before* you write
that part: in the pull request, or in the conversation. It is not a
disciplinary matter and it is not fatal. It means that part gets written from
the spec instead, or by somebody else, or gets written and then read back
against what you saw.

What is fatal is finding out afterwards.

Two practical habits make that easier than it sounds. Read for the *mechanism*
and write down what you learned in your own words before you write any code —
the note is the thing you work from, and it is already yours. And where a
published source answers the same question, prefer it: a vendor's API
documentation and its own generated type definitions say what a wire format is
without anybody's prose about it.

## The line, in practice

| Question | Answer |
| --- | --- |
| "Does oh-my-pi keep a Python kernel alive between turns, and should we?" | Read its README and decide. A capability is a fact. |
| "How does it keep that kernel alive?" | Read its source if that is the shortest way to understand it, then write crucible's own from what you understood. Say you read it. |
| "Claude Code writes `ESC ]0;…BEL` for its tab title — may I?" | Yes. That is OSC 0 from the terminal spec; every program that sets a title emits it. The *string inside it* must be crucible's own. |
| "May I match its keybindings?" | Yes for conventions the terminal already owns — Ctrl-C, Ctrl-D, arrow history. No for a scheme that is recognisably one product's design. |
| "May I use the same crate it uses?" | Yes. A public crate is a public crate. |
| "It prints `press ctrl+c again to exit`. May I?" | Yes. Eight words naming a keystroke is a fact about the keystroke, not somebody's writing. |
| "It has a paragraph explaining why a tool was refused. May I?" | No. That is long enough to have a voice, and the voice is theirs. |
| "It is MIT — may I vendor one function?" | No. The licence permits it with attribution; the claim at the top of `CLAUDE.md` does not, and vendoring is the one thing reading was never a licence for. |
| "I want the same welcome banner shape." | No. Draw crucible's own. |
| "Their docs name a feature well. May I use that name?" | A common noun, yes — `checkpoint`, `subagent`, `resume` are the vocabulary of the problem. A coined phrase that is recognisably theirs, no. |

## Where original work is expected to look similar

Terminal agents converge because terminals constrain them. A prompt line, a
streaming transcript, a permission question and a tool result block are the
shape of the problem, not anybody's invention. Convergent solutions are fine,
and so is convergent phrasing where the phrase is forced — a keystroke has one
obvious sentence and every terminal that mentions it lands near the same words.

Where there is room to choose, choose. A welcome screen, a refusal that has to
explain itself, anything the reader spends more than a glance on: write it as
crucible would, not as a memory of another agent.
