# Prompt caching

Prompt caching lets a provider reuse an identical logical prompt prefix. It is
an optimization inside a provider request: crucible still sends the request,
still asks the model for a new answer, and still preserves the complete logical
instructions, visible tools, transcript, attachments and model parameters.
It is not response caching, conversation storage, compaction,
`previous_response_id` or connection reuse.

## The default

Prompt caching is on by default through each provider's own reviewed mechanism.
The default policy is `prefer`, session-isolated, provider-default retention,
and forbids persistent cached-content resources.

| Provider | Default behavior | Request change |
| --- | --- | --- |
| OpenAI | Provider-managed implicit prefix caching | An opaque, derived routing key scoped to the session by default. |
| Anthropic | Automatic short-lived prefix caching | Top-level `cache_control: {"type":"ephemeral"}`. |
| Moonshot/Kimi | Provider-managed automatic context caching | An opaque, derived `prompt_cache_key` scoped to the session by default; Kimi manages cache creation and lifetime. |

Only exact built-in endpoint/model records advertise support. A custom
`baseUrl`, proxy route or unreviewed model is `unknown`; crucible sends no
speculative cache field. `prefer` then sends the unchanged request.

`observeOnly` explicitly asks crucible to add no cache control. It does not
promise that a provider with unavoidable automatic caching will stop caching.
`require` fails before a request when no reviewed, policy-permitted mechanism is
eligible. `prohibit` also fails before sending unless the exact provider/model
record has a real cache opt-out that the adapter can encode.

## Configuration

The `promptCaching` object is policy, not a place for provider wire fields:

```json
{
  "promptCaching": {
    "mode": "prefer",
    "allowedMechanisms": ["automaticPrefix", "explicitBreakpoints"],
    "isolationScope": "session",
    "requestedRetention": {
      "class": "ephemeral",
      "maxSeconds": 1800
    },
    "persistentResources": { "mode": "forbid" },
    "namespace": "personal-agent"
  }
}
```

The canonical values are:

- `mode`: `observeOnly`, `prefer`, `require`, `prohibit`
- `allowedMechanisms`: `providerManagedUsageOnly`, `automaticPrefix`,
  `explicitBreakpoints`, `persistentContent`
- `isolationScope`: `run`, `session`, `workspace`, `user`
- retention `class`: `providerDefault`, `ephemeral`, `extended`
- persistent-resource `mode`: `forbid`, `reuse`, `create`, `require`

`maxSeconds` is a hard ceiling, not a promise that a provider offers that exact
TTL. Extended retention and resource creation must originate in the user
configuration. Project and local project files can only narrow inherited mode,
mechanisms, isolation, retention and resource authority; they cannot turn
caching on, broaden sharing, lengthen retention, choose a namespace or
authorize a remote resource.

Persistent resources are a separate opt-in. The default performs no startup
request, creates no cleanup worker and creates no local cache directory. When
explicitly authorized, crucible stores only bounded resource metadata in an
owner-only user directory. Prompt text, responses and credentials are never
stored there. Provider and owner identities are retained only as redacted
digests. Model, provider and active-credential switches run one bounded cleanup
pass for the current run/session's exclusive resources before the switch;
workspace- and user-shared resources remain until explicit cleanup or expiry.

Use `/cache` or `/cache inspect` to see the resolved policy, declared support,
predicted eligibility, actual wire encoding, request disposition,
provider-reported outcome, normalized usage/cost and redacted resource state.
Use `/cache cleanup` for one bounded, cancellable cleanup pass. A cache read is
called a hit only when provider-reported usage says so; a local fingerprint
match or accepted request is not a hit.

## Shipped capability records

These records were reviewed on 2026-08-31. Their URLs and record versions are
compiled into the adapters so ordinary startup never scrapes mutable
documentation.

| Adapter and exact reviewed models | Mechanisms | Official source | Record version |
| --- | --- | --- | --- |
| OpenAI Responses: `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | implicit automatic caching; up to four explicit input-content breakpoints on the public API | [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching) | `openai-prompt-cache-2026-08-31` |
| OpenAI Responses: `gpt-5.5` | implicit automatic caching; optional documented 24-hour retention | [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching) | `openai-prompt-cache-2026-08-31` |
| Anthropic Messages: `claude-fable-5`, `claude-opus-5`, `claude-sonnet-5`, `claude-haiku-4-5` | top-level automatic control and up to four explicit block breakpoints; 5-minute and 1-hour classes | [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching), [tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-use-with-prompt-caching) | `anthropic-prompt-cache-2026-08-31` |
| Moonshot/Kimi Chat Completions: `k3`, `k3-256k`, `kimi-for-coding`, `kimi-for-coding-highspeed` | provider-managed automatic caching after the documented prefix minimum, with a stable routing key for agent sessions | [Kimi context caching](https://platform.kimi.ai/docs/guide/use-context-caching-feature-of-kimi-api) and [Chat Completions API](https://platform.kimi.ai/docs/api/chat) | `kimi-prompt-cache-2026-08-31` |

The OpenAI public pricing record uses the current [OpenAI pricing](https://developers.openai.com/api/docs/pricing)
and model-specific documentation. The Anthropic record uses the pricing table in
the prompt-caching guide. Pricing is selected only for an exact protocol,
endpoint, model, revision, date, retention class and input band. Subscription
billing and Moonshot membership billing remain unknown rather than being
invented as token prices.

Persistent-storage quantities, when a future lifecycle adapter reports them,
are priced in token-hours rather than raw tokens. A same-currency total may
therefore combine token and token-hour rate provenance; missing storage usage
or pricing keeps the dependent total unknown.

## Implementation-source ledger

The implementation re-derived behavior from these pinned source revisions;
no runtime dependency or copied implementation was introduced:

| Source | Revision | Cache-relevant paths reviewed |
| --- | --- | --- |
| crucible-code phase baseline | `b4c01b4099f87f68a220faae0011f8f9c6732323` | core provider contract, shipped provider bodies/wires, configuration shape |
| pi-mono | `6c87d9a026677b601e8278030dcf1ad97fe0bd86` | `anthropic-messages.ts`, `openai-prompt-cache.ts`, `openai-responses-shared.ts`, `bedrock-converse-stream.ts`, `google-generative-ai.ts`, `types.ts` |
| OpenAI Codex | `3ae4225b1761c135c6d3bbc1ea0cfcfc95752cdc` | `codex-rs/core/src/client.rs`, including prompt-cache identity and the separate response-continuation paths |
| jcode | `a5f17d2f8e33bf7469fc72d6f2a8e57aa647bc5f` | projection-aware message hashes, cache-relevant hashes, KV-cache events and stable/dynamic prompt splitting |
| Philharmonica ADK | `df69de3411e78b61faf7bb4a4d641b02f53d0bc8` | Anthropic cache applicator, policy resolution, cached-content references and normalized token details |

The following official protocol documents were also re-read on 2026-08-31 as
conformance context for future adapters. Their presence here does not claim
that crucible ships those adapters:

- [Azure OpenAI](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/prompt-caching)
- [Gemini API](https://ai.google.dev/gemini-api/docs/caching) and
  [Vertex AI](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/context-cache/context-cache-overview)
- [Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html)
- [DeepSeek](https://api-docs.deepseek.com/guides/kv_cache/)
- [Mistral](https://docs.mistral.ai/studio/conversations/advanced/prompt-caching)
- [Groq](https://console.groq.com/docs/prompt-caching)
- [xAI](https://docs.x.ai/developers/advanced-api-usage/prompt-caching)
- [OpenRouter](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

## Adapter conformance

An adapter must keep vendor fields inside its own wire module, intersect its
current encoding/parsing ability with an exact endpoint/model record, and leave
custom routes unknown until their upstream semantics are resolved. It must
prove default and `observeOnly` wire behavior, legal bounded controls, inclusive
versus disjoint usage accounting, unknown-preserving pricing, retry/cancellation
attempt identity, and redaction of keys, handles, prompt content and
credentials. Stateful response continuation remains a separate capability and
never satisfies prompt-cache support.
