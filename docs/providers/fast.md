# Fast

Some vendors serve a model faster for a higher price. `/fast` asks the model
in force for that speed, where its vendor serves one, and says what it costs
before you choose it.

## Choosing it

`/fast` opens a panel over the model in force: its provider, its name and the
credential it is served by, what the vendor says of fast, and two rows,
`Standard` and `Fast`, with the price and the vendor's speed beneath `Fast`.
The mark opens on the speed in force, so Enter changes nothing by accident;
Escape leaves it as it was.

```text
Speed · openai · gpt-6-astra · OpenAI key

OpenAI may serve a fast request at standard speed when fast capacity is short;
it is then billed at the standard price.

  Standard
  The standard price and speed

› Fast
  2x the price · up to 2.5x faster

enter to choose · esc to cancel
```

`/fast on` and `/fast off` choose without the panel. A window with no room for
the panel, and a run with no keyboard, print the speed in force and those two
lines, the price beside `on`. While a turn runs, `/fast` opens as `/model`
does, and the speed taken is asked for once the turn ends.

A model with no fast form, and a model that is itself a fast model, answer
`/fast` with one line and no panel:

```text
⎿ anthropic · claude-sonnet-5 has no fast form
⎿ moonshot · kimi-for-coding-highspeed is a fast model of its own; /model lists the others
```

In `/model`, a model with a fast form says `fast` at the end of its row, after
`no rung` where it has both.

## What you are told afterwards

The label under the box ends in `· fast` only after an answer the vendor says
it served fast. A vendor that serves a fast request at standard speed says so
in its answer, and the label then leaves `fast` off until one is served fast
again; nothing is added to the transcript.

## Where it is kept

`/fast` writes the model in force and `"fast": true` together under the
provider in the configuration file in your home directory, since the speed was
chosen at that model's price, and `/fast off` takes `fast` out:

```json
{ "providers": { "openai": { "model": "gpt-6-astra", "fast": true } } }
```

The model written is the one in force, even where `--model` or a project's
file chose it, so it becomes the one a plain start asks for, and `/fast off`
leaves it there.

Only that file is read for it. A project file that sets `fast` stops crucible
before it draws anything, as every key a checkout may not set does: fast costs
more on every request, and a repository could otherwise spend your money.

A request carries the fast form only where all of these hold: your file says
`fast` for the provider beside the model in force; the model in force has a
fast form in the table below for the credential in force; and no `baseUrl` is
set for the provider. A `fast` left in the file where one of these does not
hold is left there and asks for standard. Making room in the model's window
asks at the same speed as a turn.

## When it goes back to standard

The speed goes back to standard, and `fast` is taken out of the file, when:

- another model is chosen for the provider in `/model`. Choosing the model in
  force again keeps it;
- a credential is stored, removed or replaced for the provider and the one in
  force changes by it: a `/login` on another row, a key stored where the
  environment served, a `/logout` that leaves the environment serving, or a
  second credential taken out at a start. What fast costs was shown for the
  credential it was chosen under. A key written again on the same row, and a
  change of the environment between two runs, move nothing;
- the vendor refuses the fast form, as below.

## When the vendor refuses it

Where a vendor answers a fast request with a refusal of fast before anything
else, crucible sends the same message once more at standard speed, turns fast
off, and says why in one line:

```text
⎿ openai refused fast: Invalid service_tier argument: The requested service tier is not allowed for this project. Sent again at standard speed; fast is off.
```

Stopping the turn while the message goes again leaves the line ending `Fast is
off.` instead.

Any other error is reported as it would be at standard speed, and nothing is
sent again. What counts as a refusal of fast:

- **OpenAI with a key**: a 400 whose error is `invalid_request_error` for the
  parameter `service_tier`, as OpenAI's error reference documents.
- **OpenAI with a ChatGPT sign-in**: none is documented, so none is taken for
  one.
- **Anthropic**: a 400 `invalid_request_error` whose message names fast mode or
  `speed`, and a 429 saying "Usage credits are required for fast mode.".
  Anthropic says only that such a request "returns an error"; this rule is
  taken from open harnesses that are not Anthropic's.
- **Google**: none is documented. Congestion does not refuse; it serves the
  request at standard speed, which the answer then says.
- **Kimi Code**: `kimi-for-coding-highspeed` is a model of its own and asks for
  nothing, so there is nothing to refuse. A plan without it answers with an
  error like any other, and nothing is sent again.

## The fast forms

Each price, caveat and speed is the vendor's, as the panel shows it: the speed
beside the price under `Fast`, where the vendor states one.

| Provider | Credential | Models | Price | Caveat | Speed | Source | Read |
| --- | --- | --- | --- | --- | --- | --- | --- |
| OpenAI | key | `gpt-6-astra`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | 2x the price | OpenAI may serve a fast request at standard speed when fast capacity is short; it is then billed at the standard price. | up to 2.5x faster | [OpenAI fast mode](https://developers.openai.com/api/docs/guides/fast-mode), [OpenAI pricing](https://developers.openai.com/api/docs/pricing) | 30 Sep 2026 |
| OpenAI | key | `gpt-5.5` | 2.5x the price | OpenAI may serve a fast request at standard speed when fast capacity is short; it is then billed at the standard price. | up to 2.5x faster | [OpenAI fast mode](https://developers.openai.com/api/docs/guides/fast-mode), [OpenAI pricing](https://developers.openai.com/api/docs/pricing) | 30 Sep 2026 |
| OpenAI | sign-in | `gpt-6-astra` | 2.5x your plan's usage; 2x purchased credits | | Not stated | [ChatGPT speed](https://learn.chatgpt.com/docs/agent-configuration/speed) | 30 Sep 2026 |
| OpenAI | sign-in | `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | 2.5x your plan's usage; 2x purchased credits | | 1.5x faster | [ChatGPT speed](https://learn.chatgpt.com/docs/agent-configuration/speed) | 30 Sep 2026 |
| Anthropic | key | `claude-opus-5` | $10 / $50 per million input / output tokens | Fast mode is a research preview; Anthropic turns it on per organization. | up to 2.5x higher output tokens per second | [Anthropic fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode) | 30 Sep 2026 |
| Google | key | `gemini-3.8-flash`, `gemini-3.7-flash`, `gemini-3.6-flash`, `gemini-3.1-pro-preview` | 75-100% more than Standard | For Tier 2 and Tier 3 accounts only. Google serves a priority request at standard speed when priority is congested, and bills it at the standard price. | Not stated | [Gemini priority inference](https://ai.google.dev/gemini-api/docs/priority-inference) | 30 Sep 2026 |
| MoonshotAI | any | `kimi-for-coding-highspeed`, a fast model of its own | 3x the quota | | 6x the speed | [Kimi Code models](https://www.kimi.com/code/docs/en/kimi-code/models.html) | 29 Sep 2026 |

OpenAI states twice the price for GPT-6 and GPT-5.6 Sol; for GPT-5.6 Terra and
Luna, and for GPT-5.5, the multiple is worked out from the prices on its
pricing page. The ChatGPT speed page states no speed for GPT-6 Astra.
