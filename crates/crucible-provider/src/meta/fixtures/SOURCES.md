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
