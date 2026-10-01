# Where these fixtures come from

`stream-chunks.json`: the 200 `text/event-stream` example named "Stream" of
the Chat Completions reference,
https://platform.minimax.io/docs/api-reference/text-chat-openai, read
2026-10-01: the chunks the page prints, in order. The page prints them as YAML
and with no framing; they are written here as JSON, values unchanged, and the
tests frame them as this wire does.
