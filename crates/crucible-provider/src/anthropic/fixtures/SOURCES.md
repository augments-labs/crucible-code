# Where these fixtures come from

`error-400-prompt-too-long.json`: constructed, not copied. Its envelope, a
`type` of `error` around an `error` with a `type` and a `message`, is the one
Anthropic's API errors page, https://platform.claude.com/docs/en/api/errors,
describes for every refusal, and its `type` is that page's
`invalid_request_error` for a 400. Its message follows the form "prompt is too
long: N tokens > M maximum", with counts chosen here. No copy of that page is
kept in this repository and it was not read for this fixture, so the message
form is not confirmed against a primary source. If Anthropic words the refusal
otherwise, it is not recognised and stays a refusal that ends the turn, so a
mismatch fails safe.
