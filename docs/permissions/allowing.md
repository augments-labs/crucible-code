# What an allow rule really grants

An allow rule does not make a call safe; it removes the question. Before
writing one it is worth being precise about how much question is being
removed, because the text of a rule can read far narrower than its reach.

## A command is everything it runs

`allow: ["bash(make)"]` reads as a statement about `make`. It is a statement
about every recipe in the makefile — which is to say, about anything at all.
An allow rule for a build command is transitively an allow rule for whatever
that build executes, in every mode including `ask`. The same holds for a test
runner, a package manager's install step, anything that takes its instructions
from files in the workspace — and those files are exactly what a coding agent
edits.

crucible has no built-in list of paths that always ask, and that is
deliberate: every build tool ships a hook file, so such a list can never be
complete, and an incomplete list that looks like protection stops being
thought about. If a hook or a build script deserves guarding in your project,
write the `ask` or `deny` [rule](rules.md) about it — visible, and yours.

## Wrapper programs cannot be allowed

For some programs the thing that runs is an argument: `timeout 5 curl
example.com` is a `curl` call, and `sudo cargo test` is a `cargo` call with
the safety off. A rule matched against the program's name would say `timeout`
and mean anything at all, so a command containing one of these is treated as a
command whose text does not say what will run: no rule short of a blanket
covers it, and it is asked about every time.

The programs treated this way: `doas`, `env`, `find`, `nice`, `nohup`, `ssh`,
`su`, `sudo`, `time`, `timeout`, `watch`, `xargs` — and the shells themselves,
`bash`, `sh` and `zsh`, since `sh -c '…'` is one word to a pattern and a whole
second command line to the machine. All three of `sudo`, `su` and `doas`,
because `su -c '…' root` launders a command exactly the way `sudo` does and a
list naming only the familiar one would be a rule about spelling.

`allow: ["bash(timeout *)"]` therefore never fires. That is the point: a rule
whose author could not have known what they authorised is worse than a
question.

## Some programs are shells in disguise

The wrapper list is short because it is structural. The longer list cannot be
enforced, only known about: many ordinary programs will run an arbitrary
command if asked the right way.

- `git` — hooks, aliases, `core.pager`, `-c`
- `cargo` — `build.rs`, runners and aliases in `.cargo/config.toml`
- `npm` — lifecycle scripts
- `make` — every recipe
- `awk` — `system()`
- `tar` — `--checkpoint-action`
- `perl` — a language; whatever the one-liner says

`allow: ["bash(git *)"]` grants a shell, one subcommand away. crucible does
not pretend to police this — it would mean reimplementing each program's
argument grammar, wrongly. What it keeps instead is the rule text honest: you
wrote `git *`, and everything `git` can be told to do is what it covers.

## The advice

Write `deny` for what must never happen; it holds in every mode and no other
list can qualify it. Write `allow` about exact commands — `bash(git status)`,
`bash(cargo test)` — and prefer answering `y` a few extra times to a wildcard
you would have to reason about. Leave everything you are less sure of to the
question. The question is the mechanism; the rules are shortcuts through it.
