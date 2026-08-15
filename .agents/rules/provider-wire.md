---
paths:
  - "crates/crucible-provider/**"
---

# Changing crucible-provider

This crate turns one vendor's wire format into the `Delta` stream the rest of
the program understands. It is the only place a vendor's vocabulary is allowed
to appear.

## Adding a provider

A new provider is a new module here, plus four edits in the binary — two that
build it and two that tell the user it exists:

- `src/cli/startup.rs`: the `const <NAME>_KEY` naming the environment variable
  its key is read from, and an arm in `provider` pairing that key with the
  header this vendor authenticates by. That arm is the one place a provider's
  name becomes a type.
- `src/cli.rs`: the name in `PROVIDERS`, which is the sentence a wrong name gets
  back, and the `long_about` on `Cli`, where the provider names and their
  environment variables are spelled out for the user.

Two files, two pairs, and the pairs can disagree — a provider the parser accepts
and the help text never mentions is the failure worth designing against. They
move in one commit.

It is never an edit to `crucible-core`. If adding one seems to need a new core
enum variant or a new trait method, the abstraction is wrong — say so rather
than widening core to fit. It is not an edit to `crucible-config` either: the
`providers` block is a map looked up by name, so a provider can be configured
the day its arm exists.

How a provider module divides into parts is stated in that module's own doc
comment, which is the copy to keep current.

## chunk stops at this crate's edge

`CLAUDE.md` says why **chunk** is allowed here and nowhere else. The part that
binds while you are editing: nothing this crate *returns* is called a chunk. If
a name with `chunk` in it is about to cross into `core`, `runner` or `tui`, it
is a delta and was named wrong.

## Credentials

A provider that needs a *header name* rather than a credential kind is asking
the right question — that is what `HeaderKey` carries. Wanting to know which
kind is behind it means a seam is about to move into the wrong crate.

Every credential representation written into an outgoing header is also passed
to `Outgoing::protect`. Refusal and stream errors are provider-controlled text;
they pass through the resulting opaque `Redactions` before either `Display` or
`Debug` can reach a terminal or log.

That indifference is what makes the next rule a rule rather than an accident of
the current code. **A vendor subscription login ships only where the vendor
publishes a third-party authorization contract.** OpenAI publishes one for
ChatGPT plans and MoonshotAI for Kimi Code, so those two ship — `OpenAiOAuth`
and `KimiOAuth` in `crucible-auth`, each a new `impl Credential` rather than an
edit to any provider here. Anthropic publishes none: its terms permit a Claude
Pro or Max plan in Claude Code and not in another harness, and people have been
banned for pointing one elsewhere, so Anthropic is reached with a Console API
key and that is not a gap waiting to be filled. Where a vendor lists their own
CLI, their own IDE extension and their own app as what a plan covers, and prices
everything else at API rates, that is the same scoping said in a quieter voice —
absence of a quoted prohibition is not permission, and a credential this program
mints is a credential this program is answerable for.

A subscription with no published third-party contract reaches crucible as an API
key and the base URL the key is bound to, or not at all — a provider module and
an arm in `startup::provider`, not a new credential kind. An `impl Credential`
that opens a browser against a contract the vendor never published is the one
that gets sent back, however cleanly it is written.

The other half of arriving honestly is the header. **crucible identifies itself
as crucible.** It never sends another harness's client identifier or user agent,
to reach a plan or for any other reason. MoonshotAI writes that into its terms;
it would be the rule here if nobody had written it anywhere.

## Parsing

A vendor field that is absent, null, or a type nobody expected becomes a typed
error at this seam or it becomes a panic three layers up. Deserialize straight
into what the code uses; a struct that mirrors the vendor's shape and then gets
converted is two things to keep in sync.

An unrecognised event is not an error. Vendors add fields, and a stream that
dies on an unknown one fails every turn at the last moment. Ignore what has no
meaning here; error only on what claims to have meaning and does not parse.

A stop reason is the exception, and the reason this section is not simply "be
lenient". One this build has not heard of reads as a finish, so an answer that
was cut short arrives looking complete — the one failure the user cannot see
for themselves. A new reason in a vendor's list is an edit here, not a case the
fallback arm can be trusted to cover.
