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
