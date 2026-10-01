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
