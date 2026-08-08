# Providers and models

`--model` takes a model name, optionally qualified by the provider serving it.

```bash
crucible --model claude-sonnet-5      # anthropic, because nothing said otherwise
crucible --model openai/gpt-5.2       # openai
crucible                              # the default: claude-sonnet-5
```

An unqualified name goes to Anthropic. Only the **first** slash divides the two
halves, so a model name that contains slashes of its own stays intact:
`openai/meta/llama-4` asks the `openai` provider for `meta/llama-4`.

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
openai/gpt-5.2` needs `OPENAI_API_KEY` set and does not care whether
`ANTHROPIC_API_KEY` is.

A key never appears in a log line, an error message, a session file, or
anything crucible prints. If you see one, that is a bug worth
[reporting privately](../SECURITY.md).

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
