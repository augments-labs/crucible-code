---
name: clean-room
description: >-
  What you may read while writing crucible and what you may not. Use before
  looking at any other coding agent for reference, before copying a prompt, a
  UI string, an escape sequence or an asset, and whenever a change reproduces
  another tool's behaviour closely.
---

# Clean room

This repository is public and MIT-licensed. Nothing in it may be copied or
adapted from Claude Code, jcode, opencode, codex, aider, or any other harness —
not code, not prompt text, not a UI string, not an asset. That is a legal
boundary, not a style preference, and no gate can check it. It is why the rule
is in `CLAUDE.md` and why this procedure exists.

## Read freely

- **Standards and specifications.** ECMA-48, the XTerm control sequences
  document, OSC and CSI references, the Anthropic and OpenAI HTTP API docs, JSON
  Schema, SemVer, Keep a Changelog.
- **The Rust standard library and its docs**, and the docs of any crate already
  in `[workspace.dependencies]`.
- **Terminal emulator documentation** — what Ghostty, kitty, WezTerm, Windows
  Terminal or tmux say they support.
- **Your own measurements.** Running another tool and timing it is observation.
  Reading its source to find out why is not.

## Do not read for reference

Any other coding agent's source, prompt files, system prompts, tool
descriptions, error strings, help text, ASCII art, colour palettes or icons —
whether from a repository, a decompiled binary, a leaked dump, or a blog post
quoting them verbatim.

The rule covers *adapting* as well as copying. Renaming the variables in someone
else's prompt is still their prompt.

## The line, in practice

| Question | Answer |
| --- | --- |
| "Claude Code writes `ESC ]0;…BEL` for its tab title — may I?" | Yes. That is the OSC 0 sequence from the terminal spec; every program that sets a title emits it. The *string inside it* must be crucible's own. |
| "May I match its keybindings?" | Yes for conventions the terminal already owns — Ctrl-C, Ctrl-D, arrow history. No for a scheme that is recognisably one product's design. |
| "May I use the same crate it uses?" | Yes. A public crate is a public crate. |
| "It solves streaming this way and I want to too." | Solve it from the spec and your own measurements. If you have already read their code, say so before you write yours. |
| "I want the same welcome banner shape." | No. Draw crucible's own. |

## If you have already seen something

Say so, plainly, in the pull request or before you write the code. It is not a
disciplinary matter and it is not fatal — it just means someone other than you
should write that part, or that part should be written from the spec with the
memory of it explicitly set aside. What is fatal is finding out afterwards.

## Where original work is expected to look similar

Terminal agents converge because terminals constrain them. A prompt line, a
streaming transcript, a permission question and a tool result block are the
shape of the problem. Convergent solutions are fine. Convergent *text* is not —
write every user-visible string as if the reader has never used another agent.
