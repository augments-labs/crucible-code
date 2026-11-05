# Where these fixtures come from

`stream-thinking.sse`: assembled from the reasoning page,
https://docs.mistral.ai/studio/conversations/reasoning, read 2026-11-05,
which prints no stream's bytes. Its sections "Handling thinking chunks" and
"Streaming" give the shapes: while thinking, `delta.content` is a list
holding a `ThinkChunk` (`type: "thinking"`, whose `thinking` is a list of
`TextChunk`s); at the transition, one list holding the closing `ThinkChunk`
and the first `TextChunk` (`type: "text"`); after it, a plain string. The
counts are the example of the caching page,
https://docs.mistral.ai/inference/conversations/advanced/prompt-caching,
section "Check cached tokens", read the same day. The words are made up.
