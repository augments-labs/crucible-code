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
3. Nothing. crucible starts anyway and says so under the welcome:

```
Warning: No models available. Use /login or set an API key environment
variable. Then use /model to select a model.
```

Everything except taking a turn works in that state, which is what leaves
somewhere to type the answer. `/model <name>` asks for that model from the next
turn on and writes it to `~/.crucible/config.json` under the provider this run
is set up for, so the next run starts with it. `/model` on its own says which
model is being asked.

A model belongs to the provider serving it. crucible never writes a name under
one provider and sends it to another — the pairing is settled once, from the key
that was found, and the model rungs above are all read for that same provider.

Naming a provider this build does not have is a startup failure that says which
ones it has:

```
crucible: no provider called gemini; this build has anthropic, openai
```

## Keys

A key is read from the environment at startup and never written anywhere. Which
variable is read follows from the provider:

| Provider | Variable | Sent as |
| --- | --- | --- |
| `anthropic` | `ANTHROPIC_API_KEY` | `x-api-key` |
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

## Authentication is a separate axis

A provider is a wire protocol — how a request is shaped and how a response is
read. How you prove who you are is a different question, and crucible keeps them
apart: a provider is handed an already-resolved credential and never learns what
kind it was.

Both providers above use the same kind of credential — an API key in a header —
pointed at different headers with different prefixes. That is why adding a
subscription login later is a new credential rather than an edit to either
provider.

## What differs between them

Nothing you have to think about. The two protocols disagree about where the
system prompt goes, whether tool arguments are an object or text, whether a
turn's tool results are one message or several, how a failed result is marked,
what the token ceiling field is called, and how a stream ends. All of that is
handled inside the provider; the same session behaves the same way on either.
