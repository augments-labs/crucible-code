# Where these fixtures come from

Every file here is a 400 body printed on Meta's error-handling page,
https://dev.meta.ai/docs/error-handling (page source read from
https://dev.meta.ai/docs/error-handling.md), read 2026-10-01, copied verbatim
with nothing added or changed.

- `error-400-context-window.json`: "Common issues > Context window exceeded".
- `error-400-call-id.json`: "Common issues > Invalid conversation structure".
- `error-400-reasoning-order.json`: "Common issues > Invalid conversation
  structure".
- `error-400-validation.json`: "Error response format".

No fixture comes from the pages below, which were read 2026-11-05 for the
per-model prompt cache record in `meta.rs` and the offered rungs:
https://dev.meta.ai/docs/prompt-caching (prefix caching, `cached_tokens`, and
no smallest cached prefix for any model), https://dev.meta.ai/docs/models,
https://dev.meta.ai/docs/pricing-rate-limits and
https://dev.meta.ai/docs/reasoning.

The preview status of `muse-wonder-1.0` was read on 2026-11-05: the models page
says "Muse Wonder is in preview". Meta's terms of service,
https://dev.meta.ai/legal/terms-of-service (last updated 2026-10-09), give
preview models no separate rule, and section 5.1 says Meta "will not use Content
submitted in a request to the Standard Services to train Meta Models". Its
designation policy, https://dev.meta.ai/legal/geographic-use-policy (last
updated 2026-10-12), lists Muse Wonder and Muse Wonder Full as Standard Models.
