# Providers and models

`--model` takes a model name, optionally qualified by the provider serving it.

```bash
crucible --model claude-sonnet-5      # the provider whose key this machine holds
crucible --model openai/gpt-5.6-terra # openai
crucible --model openai/              # openai, asking for the model it is configured with
crucible                              # both halves left to the machine and your configuration
```

Only the **first** slash divides the two halves, so a model name that contains
slashes of its own stays intact: `openai/meta/llama-4` asks the `openai`
provider for `meta/llama-4`.

## Which provider

Nothing but the qualified form names a provider outright. Everything else is
settled by which key this machine holds, and by nothing else — **no provider is
written into the build**. Exactly one of the variables in [Keys](#keys) holding
a key is the provider asked, whatever model name you went on to type. That is
what stops a machine set up for one vendor from sending a turn to the other.

A variable exported empty holds no key, so it does not compete: a shell carrying
`ANTHROPIC_API_KEY=` alongside a real `OPENAI_API_KEY` asks OpenAI. A provider
pointed at another variable by `apiKeyEnv` is looked for under that name.

Two keys and nothing choosing between them is a question rather than a coin
toss:

```
crucible: more than one provider holds a key (ANTHROPIC_API_KEY, OPENAI_API_KEY),
so which to ask is not decided; qualify the name as --model provider/model, or
set providers.<name>.model for one of them
```

A model already chosen for exactly one of them — which is what `/model` writes
down — answers it, so this is asked once and not every run.

## Which model

There is **no model built in**, and none of these rungs is a guess:

1. `--model`, where it names one.
2. `providers.<name>.model` in your
   [configuration](../configuration/configuration.md), for the provider being
   asked. A provider and a bare slash — `--model openai/` — is how you reach
   this rung with the flag present.
3. Nothing. crucible starts anyway and says so under the welcome — naming
   whichever half of setting it up is still missing:

```
Warning: No model selected. Use /model to select the model to ask.
```

```
Warning: No models available. Use /login or set an API key environment
variable. Then use /model to select a model.
```

Everything except taking a turn works in that state, which is what leaves
somewhere to type the answer. Down a pipe there is nobody to type it, so a
prompt arriving there cannot be answered and the run ends non-zero rather than
reading every remaining line and answering none of them.

`/model <name>` asks for that model from the next
turn on and writes it to `~/.crucible/config.json` under the provider this run
is set up for, so the next run starts with it. `/model` on its own says which
model is being asked.

A model belongs to the provider serving it. crucible never writes a name under
one provider and sends it to another — the pairing is settled once, from the key
that was found, and the model rungs above are all read for that same provider.

Naming a provider this build does not have is a startup failure that says which
ones it has:

```
crucible: no provider called gemini; this build has anthropic, moonshot, openai
```

## Keys

A key is read at startup and goes no further than the header it signs a request
with. Which variable is read follows from the provider:

| Provider | Variable | Sent as |
| --- | --- | --- |
| `anthropic` | `ANTHROPIC_API_KEY` | `x-api-key` |
| `moonshot` | `MOONSHOT_API_KEY` | `authorization: Bearer …` |
| `openai` | `OPENAI_API_KEY` | `authorization: Bearer …` |

Only the chosen provider's variable is read. Running `crucible --model
openai/gpt-5.6-terra` needs `OPENAI_API_KEY` set and does not care whether
`ANTHROPIC_API_KEY` is.

A configuration file can point a provider at a different variable, which is what
a second key for the same vendor needs:

```json
{ "providers": { "anthropic": { "apiKeyEnv": "WORK_ANTHROPIC_KEY" } } }
```

That is a variable **name**, and pointing crucible at one points it away from
the other: `ANTHROPIC_API_KEY` is then not read at all.

A key never appears in a log line, an error message, a session file, or
anything crucible prints. If you see one, that is a bug worth
[reporting privately](../../SECURITY.md).

### A key written down instead of exported

The other place crucible looks is `~/.crucible/auth.json`, a file it creates
readable by nobody else and tightens to that if it finds it otherwise. A key
kept there is set up once and needs nothing from your shell afterwards, which is
what a machine you do not want a key on the profile of wants:

```json
{ "version": 1, "keys": { "openai": "sk-…" } }
```

`/login <provider>` inside a session is what writes it, into a box that draws a
dot per character rather than the key; `/login` on its own asks which provider
first. `/logout <provider>` takes one back out again, and `/logout` on its own
offers the providers a key is written down for. Editing the file by hand works
too — crucible only reads what is there, and a name under `keys` that this build
does not serve is left alone rather than offered for logging out of.

The names under `keys` are provider names, the same ones `--model openai/…`
takes. `version` says which crucible wrote the file, so one from a later version
is left alone rather than guessed at.

**The variable wins.** A key exported into a run is the one you chose for that
run — a second account, a work key, one rotated an hour ago — and it lasts as
long as the shell it was exported in. What is written down is the standing
answer underneath it, so `OPENAI_API_KEY=` turns off the *variable* and leaves
the file's key doing its job. A provider holding a key both ways is still one
provider, not two — and `/logout` reaches the written-down half of it, which is
why what it says names the other.

A file crucible cannot read is a sentence under the welcome — `! auth.json could
not be read: …` — and not the end of the run: nobody is logged in for that run,
which leaves the environment, and ending it would take away the session the file
gets fixed from.

## Authentication is a separate axis

A provider is a wire protocol — how a request is shaped and how a response is
read. How you prove who you are is a different question, and crucible keeps them
apart: a provider is handed an already-resolved credential and never learns what
kind it was.

Every provider above uses the same kind of credential — an API key in a header —
pointed at different headers with different prefixes. That is why a different
way of proving who you are is a new credential rather than an edit to any
provider.

### Why there is no "log in with your subscription"

crucible authenticates by API key, and a vendor's chat subscription is not one
of the keys it accepts. That is deliberate and it is not a gap waiting to be
filled.

A subscription is sold scoped to the vendor's own software. Anthropic's terms
permit a Claude Pro or Max plan in Claude Code and not in another program, and
accounts have been closed for pointing something else at one. Other vendors say
the same thing more quietly, by listing their own CLI, their own editor
extension and their own app as what a plan covers and pricing everything else at
API rates. crucible will not put your account in that position, so it does not
offer the login at all.

A plan a vendor *does* publish for other programs is a different thing, and
crucible takes it. That path is an API key and the base URL the key belongs to,
which is a key in your environment and a `baseUrl` in your configuration —
nothing new to learn. crucible also identifies itself as crucible on every
request, and will not claim to be another program to reach a plan that way.

## What differs between them

Nothing you have to think about. The protocols disagree about where the system
prompt goes, whether a transcript is a list of messages or a flat list of items,
whether a tool call belongs to the message that made it, whether a tool is
declared nested or flat, how a failed result is marked, and how a stream ends.
All of that is handled inside the provider; the same session behaves the same way
on any of them.

One difference is worth knowing about because it decides which OpenAI models
work at all. crucible talks to OpenAI over `/v1/responses` rather than
`/v1/chat/completions`, because a model that reasons before answering refuses
function tools on the older endpoint — and a harness whose whole purpose is
calling tools cannot answer that by telling the model not to think. The cost is
that other vendors serving an "OpenAI-compatible" API implement the older
endpoint and not this one, so `openai` means OpenAI here rather than anything
that speaks its shape. `moonshot` is that older endpoint, read by a provider of
its own.

Two consequences you can see:

- Nothing is retained by the vendor. Requests are sent with `store` off, so a
  response is not kept for later retrieval.
- No token ceiling is sent. On that endpoint one number bounds the reasoning and
  the visible answer together, so a figure chosen for an answer is one the model
  can spend entirely on thinking; the model's own ceiling applies instead.

## MoonshotAI issues a key against one console or the other

This is the one provider where a working key can still be refused, and the
refusal does not say why.

MoonshotAI sells two products with separate consoles, and a key from one is not
accepted by the other:

| Where the key came from | Address it is accepted at |
| --- | --- |
| Kimi Code Console | `https://api.kimi.com/coding/v1` |
| Open Platform | `https://api.moonshot.ai/v1` |

Nothing in the key itself says which, so crucible cannot read it and decide. It
asks the coding console, that being the plan sold for what crucible does. A key
from the open platform says so in
[configuration](../configuration/configuration.md):

```json
{ "providers": { "moonshot": { "baseUrl": "https://api.moonshot.ai/v1" } } }
```

Requests to this provider identify crucible by name in the `user-agent` header.
MoonshotAI's terms require a client to say truthfully what it is, and treat a
tampered identifier as a violation.
