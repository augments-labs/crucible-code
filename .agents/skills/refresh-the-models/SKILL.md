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
look, and is never copied in unread. No provider is added here, and a model
family is offered only by a hand-made product decision, never because the
database lists it.

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
| mistral | docs.mistral.ai: models, reasoning, prompt caching; legal.mistral.ai: commercial terms |
| moonshot | platform.kimi.ai: pricing, chat, thinking models; kimi.com: Kimi Code membership |
| openai | developers.openai.com: models, reasoning |
| qwen | alibabacloud.com Model Studio: models, text generation, deep thinking |
| xai | docs.x.ai: models, reasoning |
| zai | docs.z.ai: pricing, the current GLM page |

## Settle each finding

1. **A newer model.** If the vendor serves it to coding agents through the API
   crucible speaks, offer it. Otherwise add it to `PASSED` in
   `src/bin/generate-models.rs` with the reason, so the next report is quiet
   about it. Offering it is one change that reaches each of these:
   - **The registry.** Add it to the provider's `models` in
     `crates/crucible-app/src/providers.rs`, newest first, with the rungs the
     vendor documents. Use `Model::shown` where the product name the vendor's
     pages give differs from the id it is sent as, so `/model` shows both.
   - **The table.** Add it to `OFFERED`, with the key the database lists it
     under, `None` where the database has no entry, and a divisor only where
     the vendor serves a fraction of the window the database lists.
   - **The provider module.** Its module in `crates/crucible-provider` needs
     the model's own records, read from the same pages: prompt caching and its
     prices, and where the module keeps them, its fast form and the models that
     may replay each other's state. The tests beside the registry fail for a
     model offered without them.
   - **The content-use hold.** Where the vendor's terms say it may use what is
     sent to this model to train or improve its models, as for a Preview, labs
     or contributor model, add a `model:<provider>/<model>` route to `WARNED`
     in `crates/crucible-app/src/content_use.rs`. It carries the vendor's
     words, the page and the day it was read. Add the same row to
     `docs/providers/content-use.md`, and the route to the set in
     `the_routes_warned_are_those_whose_vendor_says_so`. Those tests hold a
     row to its docs once it exists, but no test reads a vendor's terms, so
     nothing fails for a model left out, and it is sent content before
     anyone agreed to that.
   - **The window.** Add the model and the window it starts at to
     `a_model_this_build_knows_starts_at_its_native_window` in
     `crates/crucible-app/src/startup/tests/windows.rs`.
   - **The docs.** Update the provider's row in `docs/providers/providers.md`,
     and the model's line in `prompt-caching.md`, `fast.md` and `reading.md`
     beside it, as well as the native windows in
     `docs/configuration/configuration.md`.
2. **A model gone or deprecated.** Take it out of `models` and `OFFERED` once
   the vendor retires it, and out of every place 1 lists. A default is never a
   retired model.
3. **Rungs that differ.** Write the rungs the vendor documents. Where they
   still differ from the database, say why beside the model and add it to
   `KEPT` in `src/bin/generate-models.rs`, so the next report is quiet about
   it.
4. **The default.** `default` is the vendor's latest middle-tier model; its
   rung is `default_rung`'s, medium or the nearest above it. Move it only when
   the vendor's middle tier moved.
5. Set the provider's `reviewed` to the date the pages were read.
6. **A family crucible does not offer.** Read the vendor's page for the model.
   Offer it as in 1, or decline the family: add the provider, the family the
   report names and the reason to `PASSED_FAMILIES`. That quiets every model of
   it, one released later included, until the row is removed, so remove it
   when the family is worth another look. To decline only one model of the
   family, add that model to `PASSED` with the reason instead; a `labs-` model
   of Mistral is settled by 8, not by a `PASSED` row. The check never adds a
   family: it is chosen by hand.
7. **A revised model.** The database dates an offered model after the
   provider's `reviewed` day, so the vendor released or revised it under the
   same name. Re-read the model's records against the vendor's pages as in 1,
   then set the provider's `reviewed` as in 5.
8. **A Preview model by its name.** Mistral's terms count a `labs-` model as a
   Preview model it may train on, so the check reports each one the database
   lists, whatever it can do, until it is held or declined; a family declined
   in 6 does not quiet it. Hold it by name as the content-use hold in 1 says,
   and, where crucible does not offer it, add it to `HELD_UNOFFERED` in
   `crates/crucible-app/src/content_use/tests.rs`. A name typed in `/model` is
   sent as typed, so a model only declined is sent with nothing asked first:
   add it to `PASSED` with the reason only where the vendor's terms no longer
   count it a Preview model.

## Regenerate and prove

```bash
curl -sSfL https://models.dev/api.json | cargo run --quiet --bin generate-models
scripts/sh/check.sh
```

The first rewrites `crates/crucible-app/src/models.rs` for every model in
`OFFERED`. The tests beside the catalogue hold each offered model to a row
there and each default to its provider's models.

A changed model list redraws the `/model` screens. Read each redrawn snapshot
under `tests/whole_screen/snapshots/` as a terminal screen before accepting
it. Then run `python3 scripts/python/screen-baseline.py --record`,
so `scripts/screen-baseline.json` names the accepted picture.

The pull request gives a changelog entry for each model offered or removed,
and closes the drift issue it settles.
