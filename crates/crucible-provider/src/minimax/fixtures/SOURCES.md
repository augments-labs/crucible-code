# Where these fixtures come from

`stream-chunks.json`: the 200 `text/event-stream` example named "Stream" of
the Chat Completions reference,
https://platform.minimax.io/docs/api-reference/text-chat-openai, read
2026-10-01: the chunks the page prints, in order. The page prints them as YAML
and with no framing; they are written here as JSON, values unchanged, and the
tests frame them as this wire does.

`error-thinking-disabled-2013.txt`: the note under "Thinking Control" of
https://platform.minimax.io/docs/api-reference/text-openai-api, read
2026-10-01: the error text the vendor prints for thinking switched off on a
model that requires it, copied verbatim. The page prints the text only, not
the body or status around it, so the tests place it under `base_resp` as the
vendor's other failures arrive.

No fixture here is the refusal of a request too large for the model, because
no primary source for it was found: neither the Chat Completions reference nor
the error-code pages print it. The body the tests use for it is constructed.
Its code is the reference's `2013`, "invalid params". Its words, "invalid
params, context window exceeds limit", are taken from another client's
handling of MiniMax's overflow, the Pi coding agent's overflow matcher, and
are unconfirmed. The closing `(2013)` follows the thinking refusal above. If
the vendor words the refusal otherwise, it is not recognised and stays a
refusal that ends the turn, so a mismatch fails safe.

`token-plan-remains.json`: the answer to `GET /v1/token_plan/remains` in the
shape of the vendor's own command-line client, MiniMax-AI/cli at commit
06e47c70: the field names of `QuotaModelRemain` in `src/types/api.ts`, and
the three models of `test/fixtures/quota-response.json`. It is reconstructed
from notes on those files rather than copied from them. The `MiniMax-M*`
entry's figures are the ones that fixture holds. For `speech-hd` and
`image-01` only the two totals are its own; their counts are written as the
totals, which that client reads as nothing used, and their times are the
first entry's. The optional fields `*_remaining_percent` and `*_status` are
absent there, as they are in that fixture; the tests that read them set them
on a copy. The site's own page,
https://platform.minimax.io/docs/token-plan/faq, prints the request but no
answer.
