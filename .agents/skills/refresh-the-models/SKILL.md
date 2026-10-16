---
name: refresh-the-models
description: >-
  Refresh the models, rungs and defaults crucible offers for each provider.
  Use when a model-drift issue is open, when a vendor releases or retires a
  model, or when a default model or effort is to change.
---

# Refresh the models

The providers and their models are written by hand in
`crates/crucible-app/src/providers.rs`, and the vendor's own documentation is
the authority on them. The models.dev database is a lead: it says where to
look, and is never copied in unread. No provider and no model family is added
here; those are product decisions made before this skill is used.

## Read what drifted

The `model-drift` workflow runs on the 1st and 15th and keeps one open issue
labelled `model-drift`. To read the same report locally:

```bash
curl -sSfL https://models.dev/api.json | cargo run --quiet --bin generate-models -- --drift
```

It exits 0 with nothing to report, 1 with a report, and 2 on unreadable input.

## Check each finding against the vendor

Read the provider's own pages for each line of the report:

| Provider | Pages |
| --- | --- |
| anthropic | platform.claude.com: models overview, effort |
| deepseek | api-docs.deepseek.com: pricing, thinking mode |
| google | ai.google.dev: models, pricing, thinking |
| meta | dev.meta.ai: models |
| mimo | the Xiaomi MiMo platform's model list |
| minimax | platform.minimax.io: models, text generation |
| moonshot | platform.kimi.ai: pricing, chat, thinking models; kimi.com: Kimi Code membership |
| openai | developers.openai.com: models, reasoning |
| qwen | alibabacloud.com Model Studio: models, text generation, deep thinking |
| xai | docs.x.ai: models, reasoning |
| zai | docs.z.ai: pricing, the current GLM page |

## Settle each finding

1. **A newer model.** If the vendor serves it to coding agents through the API
   crucible speaks, add it to the provider's `models`, newest first, with the
   rungs the vendor documents, and add it to `OFFERED` in
   `src/bin/generate-models.rs`. Its provider module in
   `crates/crucible-provider` then needs the model's own records, read from
   the same pages: prompt caching and its prices, and where the module keeps
   them, its fast form and the models that may replay each other's state. The
   tests beside the registry fail for a model offered without them. Otherwise
   add it to `PASSED` there with the reason, so the next report is quiet about
   it.
2. **A model gone or deprecated.** Take it out of `models` and `OFFERED` once
   the vendor retires it. A default is never a retired model.
3. **Rungs that differ.** Write the rungs the vendor documents. Where they
   still differ from the database, say why beside the model and add it to
   `KEPT` in `src/bin/generate-models.rs`, so the next report is quiet about
   it.
4. **The default.** `default` is the vendor's latest middle-tier model; its
   rung is `default_rung`'s, medium or the nearest above it. Move it only when
   the vendor's middle tier moved.
5. Set the provider's `reviewed` to the date the pages were read.

## Regenerate and prove

```bash
curl -sSfL https://models.dev/api.json | cargo run --quiet --bin generate-models
scripts/sh/check.sh
```

The first rewrites `crates/crucible-app/src/models.rs` for every model in
`OFFERED`. The tests beside the catalogue hold each offered model to a row
there and each default to its provider's models. Close the drift issue from
the pull request that settles it.
