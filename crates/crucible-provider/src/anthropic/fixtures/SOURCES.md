# Where these fixtures come from

`error-400-prompt-too-long.json`: constructed, not copied. Its envelope, a
`type` of `error` around an `error` with a `type` and a `message`, is the one
Anthropic's API errors page, https://platform.claude.com/docs/en/api/errors,
describes for every refusal. Anthropic's context windows page,
https://platform.claude.com/docs/en/build-with-claude/context-windows, says
that when the input alone exceeds the model's context window "the API returns a
400 `invalid_request_error` ("prompt is too long") on every model". The counts
after those words, "N tokens > M maximum", follow the form the refusal is seen
with in practice and are not printed on either page, so the match requires only
the documented opening words.
